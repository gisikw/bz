//! PTY socket handler for chaperone
//!
//! Each PTY gets a Unix socket for direct I/O with bz.
//! Supports multiple sequential client connections with history replay.
//!
//! Used by bzc binary.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use color_eyre::eyre::{Result, WrapErr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};

use crate::protocol::{ClientMessage, DaemonMessage, decode, encode};

/// PTY socket server
///
/// Accepts multiple sequential client connections, replaying history on each connect.
/// Runs until the PTY process exits.
pub struct PtySocket {
    /// Socket path
    path: PathBuf,
    /// Listener for client connections
    listener: UnixListener,
}

impl PtySocket {
    /// Create a new PTY socket
    pub fn new(chaperone_dir: &PathBuf, pty_id: &str) -> Result<Self> {
        let path = chaperone_dir.join(format!("{}.sock", pty_id));

        // Remove stale socket
        let _ = std::fs::remove_file(&path);

        let listener = std::os::unix::net::UnixListener::bind(&path)
            .wrap_err_with(|| format!("Failed to bind PTY socket: {}", path.display()))?;
        listener.set_nonblocking(true)?;

        let listener = UnixListener::from_std(listener)?;

        Ok(Self { path, listener })
    }

    /// Get the socket path
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Run the socket server
    ///
    /// Accepts clients sequentially, replaying history on each connect.
    /// Returns when the PTY process exits (output_rx closes).
    pub async fn run(
        self,
        initial_history: Vec<u8>,
        output_rx: mpsc::Receiver<Vec<u8>>,
        input_tx: mpsc::Sender<PtyInput>,
    ) -> Result<PtyExitReason> {
        // Shared state between relay task and client handlers
        let relay_state = Arc::new(RelayState::new(initial_history));

        // Channel for PTY exit notification
        let (exit_tx, mut exit_rx) = watch::channel(false);

        // Spawn relay task: drains output_rx → history + client
        let relay_state_for_task = Arc::clone(&relay_state);
        let relay_handle = tokio::spawn(async move {
            relay_task(output_rx, relay_state_for_task, exit_tx).await
        });

        // Accept loop: handle clients until PTY exits
        loop {
            tokio::select! {
                biased;

                // Check if PTY exited
                _ = exit_rx.changed() => {
                    if *exit_rx.borrow() {
                        break;
                    }
                }

                // Accept new client
                accept_result = self.listener.accept() => {
                    match accept_result {
                        Ok((stream, _)) => {
                            // Handle this client (blocks until they disconnect)
                            handle_client(
                                stream,
                                Arc::clone(&relay_state),
                                input_tx.clone(),
                                &mut exit_rx,
                            ).await;
                        }
                        Err(e) => {
                            eprintln!("bzc: PTY socket accept error: {}", e);
                        }
                    }
                }
            }
        }

        // Wait for relay task to finish
        let _ = relay_handle.await;

        Ok(PtyExitReason::ProcessExited)
    }
}

impl Drop for PtySocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Input from client to PTY
#[derive(Debug)]
pub enum PtyInput {
    /// Keyboard input
    Data(Vec<u8>),
    /// Resize request
    Resize { rows: u16, cols: u16 },
}

/// Why the PTY socket server stopped
#[derive(Debug)]
pub enum PtyExitReason {
    /// The PTY process exited
    ProcessExited,
}

/// Shared state for relay pattern
struct RelayState {
    inner: Mutex<RelayStateInner>,
}

struct RelayStateInner {
    /// Accumulated history buffer
    history: Vec<u8>,
    /// Channel to send live output to current client (if connected)
    client_tx: Option<mpsc::Sender<Vec<u8>>>,
}

impl RelayState {
    fn new(initial_history: Vec<u8>) -> Self {
        Self {
            inner: Mutex::new(RelayStateInner {
                history: initial_history,
                client_tx: None,
            }),
        }
    }

    /// Append data to history and forward to client if connected
    fn append_and_forward(&self, data: Vec<u8>) {
        let mut inner = self.inner.lock().unwrap();
        inner.history.extend(&data);

        if let Some(tx) = &inner.client_tx {
            // Try to send; if channel full or closed, that's ok
            let _ = tx.try_send(data);
        }
    }

    /// Get a copy of the current history
    fn get_history(&self) -> Vec<u8> {
        self.inner.lock().unwrap().history.clone()
    }

    /// Set the client channel for live output forwarding
    fn set_client(&self, tx: mpsc::Sender<Vec<u8>>) {
        self.inner.lock().unwrap().client_tx = Some(tx);
    }

    /// Clear the client channel (client disconnected)
    fn clear_client(&self) {
        self.inner.lock().unwrap().client_tx = None;
    }
}

/// Relay task: drains output_rx and appends to history + forwards to client
async fn relay_task(
    mut output_rx: mpsc::Receiver<Vec<u8>>,
    state: Arc<RelayState>,
    exit_tx: watch::Sender<bool>,
) {
    while let Some(data) = output_rx.recv().await {
        state.append_and_forward(data);
    }

    // output_rx closed = PTY process exited
    let _ = exit_tx.send(true);
}

/// Handle a single client connection
async fn handle_client(
    mut stream: UnixStream,
    state: Arc<RelayState>,
    input_tx: mpsc::Sender<PtyInput>,
    exit_rx: &mut watch::Receiver<bool>,
) {
    // Get current history snapshot
    let history = state.get_history();

    // Send history
    if !history.is_empty() {
        let msg = DaemonMessage::History {
            pty_id: 0,
            data: history,
        };
        if let Ok(encoded) = encode(&msg) {
            if stream.write_all(&encoded).await.is_err() {
                return; // Client disconnected during history send
            }
        }
    }

    // Send history end marker
    let end_msg = DaemonMessage::HistoryEnd { pty_id: 0 };
    if let Ok(encoded) = encode(&end_msg) {
        if stream.write_all(&encoded).await.is_err() {
            return;
        }
    }

    // Set up live output channel
    let (client_tx, mut client_rx) = mpsc::channel::<Vec<u8>>(256);
    state.set_client(client_tx);

    // Split stream for concurrent read/write
    let (mut read_half, mut write_half) = stream.into_split();

    // Task to forward live output to client
    let output_task = tokio::spawn(async move {
        loop {
            match client_rx.recv().await {
                Some(data) => {
                    let msg = DaemonMessage::Output { pty_id: 0, data };
                    if let Ok(encoded) = encode(&msg) {
                        if write_half.write_all(&encoded).await.is_err() {
                            break; // Client disconnected
                        }
                    }
                }
                None => break, // Channel closed
            }
        }
        write_half // Return for potential PtyExited message
    });

    // Read input from client until they disconnect or PTY exits
    loop {
        tokio::select! {
            biased;

            // Check if PTY exited
            _ = exit_rx.changed() => {
                if *exit_rx.borrow() {
                    eprintln!("DEBUG pty_socket: PTY exited, sending PtyExited to client");
                    // Send PtyExited to client
                    let exit_msg = DaemonMessage::PtyExited {
                        pty_id: 0,
                        exit_code: None,
                    };
                    if let Ok(encoded) = encode(&exit_msg) {
                        // Try to get write_half back from output_task
                        state.clear_client(); // This will close client_rx
                        if let Ok(mut write_half) = output_task.await {
                            eprintln!("DEBUG pty_socket: writing PtyExited to client");
                            let _ = write_half.write_all(&encoded).await;
                            eprintln!("DEBUG pty_socket: PtyExited written");
                        } else {
                            eprintln!("DEBUG pty_socket: failed to get write_half from output_task");
                        }
                    }
                    return;
                }
            }

            // Read from client
            result = read_length_prefixed(&mut read_half) => {
                match result {
                    Ok(payload) => {
                        if let Ok(msg) = decode::<ClientMessage>(&payload) {
                            match msg {
                                ClientMessage::Input { data, .. } => {
                                    let _ = input_tx.send(PtyInput::Data(data)).await;
                                }
                                ClientMessage::Resize { rows, cols, .. } => {
                                    let _ = input_tx.send(PtyInput::Resize { rows, cols }).await;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(_) => break, // Client disconnected
                }
            }
        }
    }

    // Client disconnected - clean up
    state.clear_client();
    output_task.abort();
}

/// Read a length-prefixed message from the stream
async fn read_length_prefixed(
    read_half: &mut tokio::net::unix::OwnedReadHalf,
) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    read_half.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    read_half.read_exact(&mut payload).await?;

    Ok(payload)
}
