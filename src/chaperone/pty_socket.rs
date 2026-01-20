//! PTY socket handler for chaperone
//!
//! Each PTY gets a Unix socket for direct I/O with bz.
//!
//! Used by bzc binary.

#![allow(dead_code)]

use std::path::PathBuf;

use color_eyre::eyre::{Result, WrapErr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::protocol::{DaemonMessage, ClientMessage, encode, decode};

/// PTY socket server
///
/// Handles a single client connection for PTY I/O.
pub struct PtySocket {
    /// Socket path
    path: PathBuf,
    /// Listener for client connections
    listener: UnixListener,
    /// PTY ID
    pty_id: usize,
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

        // Parse pty_id as usize for protocol compatibility
        // Using 0 as fallback since new protocol uses string IDs
        let pty_id_num = 0;

        Ok(Self {
            path,
            listener,
            pty_id: pty_id_num,
        })
    }

    /// Get the socket path
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Run the socket server
    ///
    /// Accepts one client at a time, handles history replay and live I/O.
    /// Note: This takes ownership and runs until the PTY exits.
    pub async fn run(
        self,
        history: Vec<u8>,
        output_rx: mpsc::Receiver<Vec<u8>>,
        input_tx: mpsc::Sender<PtyInput>,
    ) -> Result<()> {
        // Accept a single connection
        let (stream, _) = self.listener.accept().await?;

        // Handle this client (takes ownership of channels)
        handle_client(stream, history, output_rx, input_tx).await
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

/// Handle a single client connection
async fn handle_client(
    mut stream: UnixStream,
    history: Vec<u8>,
    mut output_rx: mpsc::Receiver<Vec<u8>>,
    input_tx: mpsc::Sender<PtyInput>,
) -> Result<()> {
    // Send history
    if !history.is_empty() {
        let msg = DaemonMessage::History {
            pty_id: 0, // Legacy protocol uses usize
            data: history,
        };
        let encoded = encode(&msg)?;
        stream.write_all(&encoded).await?;
    }

    // Send history end marker
    let end_msg = DaemonMessage::HistoryEnd { pty_id: 0 };
    let encoded = encode(&end_msg)?;
    stream.write_all(&encoded).await?;

    // Split stream for concurrent read/write
    let (mut read_half, mut write_half) = stream.into_split();

    // Task to forward output to client
    let output_task = tokio::spawn(async move {
        while let Some(data) = output_rx.recv().await {
            let msg = DaemonMessage::Output { pty_id: 0, data };
            if let Ok(encoded) = encode(&msg) {
                if write_half.write_all(&encoded).await.is_err() {
                    break;
                }
            }
        }
    });

    // Read input from client
    loop {
        // Read length prefix
        let mut len_buf = [0u8; 4];
        match read_half.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(_) => break, // Client disconnected
        }
        let len = u32::from_le_bytes(len_buf) as usize;

        // Read payload
        let mut payload = vec![0u8; len];
        if read_half.read_exact(&mut payload).await.is_err() {
            break;
        }

        // Decode and handle
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

    output_task.abort();
    Ok(())
}
