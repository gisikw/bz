//! Control socket handling for the chaperone
//!
//! Manages the control socket that bzd uses to send commands.
//!
//! Used by bzc binary.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use color_eyre::eyre::{Result, WrapErr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use super::protocol::{self, ControlMessage};
use super::pty::{ManagedPty, PtySpawnConfig};
use super::pty_socket::{PtyInput, PtySocket};

/// Base directory for chaperone data
fn chaperone_base_dir() -> PathBuf {
    crate::env::data_dir().join("chaperones")
}

/// Directory for a specific chaperone
pub fn chaperone_dir(name: &str) -> PathBuf {
    chaperone_base_dir().join(name)
}

/// Control socket path for a chaperone
pub fn control_socket_path(name: &str) -> PathBuf {
    chaperone_dir(name).join("control.sock")
}

/// Handle to a running PTY for cleanup
struct PtyHandle {
    /// Shared PTY for input handling
    pty: Arc<Mutex<ManagedPty>>,
    /// Socket path for cleanup
    socket_path: PathBuf,
    /// Room ID for detach notification
    room_id: String,
}

/// PTY exit event for notification
struct PtyExitEvent {
    pty_id: String,
    room_id: String,
}

/// The main Chaperone runtime
pub struct Chaperone {
    /// Chaperone name
    name: String,
    /// Managed PTYs (keyed by PTY ID)
    ptys: HashMap<String, PtyHandle>,
    /// Default terminal size
    default_rows: u16,
    default_cols: u16,
}

impl Chaperone {
    /// Create a new chaperone
    pub fn new(name: String) -> Self {
        Self {
            name,
            ptys: HashMap::new(),
            default_rows: 24,
            default_cols: 80,
        }
    }

    /// Get the control socket path
    pub fn control_socket_path(&self) -> PathBuf {
        control_socket_path(&self.name)
    }

    /// Run the chaperone main loop
    pub async fn run(mut self) -> Result<()> {
        let socket_path = self.control_socket_path();

        // Ensure directory exists
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)
                .wrap_err_with(|| format!("Failed to create directory: {}", parent.display()))?;

            // Clean up stale PTY sockets from previous crashed sessions.
            // These are UUID-named .sock files left behind when chaperone dies abruptly.
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map_or(false, |ext| ext == "sock") {
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }

        let listener = UnixListener::bind(&socket_path)
            .wrap_err_with(|| format!("Failed to bind control socket: {}", socket_path.display()))?;

        // Accept connections
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    if let Err(e) = self.handle_connection(stream).await {
                        eprintln!("bzc: connection error: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("bzc: accept error: {}", e);
                }
            }
        }
    }

    /// Handle a single connection
    async fn handle_connection(&mut self, mut stream: UnixStream) -> Result<()> {
        // Send Ready message
        let ready = protocol::encode(&ControlMessage::Ready)?;
        stream.write_all(&ready).await?;

        // Channel for PTY exit notifications
        let (exit_tx, mut exit_rx) = mpsc::channel::<PtyExitEvent>(16);

        // Read and handle messages
        loop {
            tokio::select! {
                biased;

                // Handle PTY exit events (check first)
                Some(event) = exit_rx.recv() => {
                    // Send PtyDetached notification
                    let detach = protocol::encode(&ControlMessage::PtyDetached {
                        pty_id: event.pty_id.clone(),
                        room_id: event.room_id,
                    })?;
                    let _ = stream.write_all(&detach).await;

                    // Clean up handle
                    if let Some(handle) = self.ptys.remove(&event.pty_id) {
                        let _ = std::fs::remove_file(&handle.socket_path);
                    }
                }

                // Handle control messages
                result = read_control_message(&mut stream) => {
                    match result {
                        Ok(msg) => {
                            self.handle_message(msg, &mut stream, &exit_tx).await?;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                        Err(e) => return Err(e.into()),
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle a control message
    async fn handle_message(
        &mut self,
        msg: ControlMessage,
        stream: &mut UnixStream,
        exit_tx: &mpsc::Sender<PtyExitEvent>,
    ) -> Result<()> {
        match msg {
            ControlMessage::SpawnPty {
                room_id,
                cwd,
                command,
                rows,
                cols,
            } => {
                let cmd = command.unwrap_or_else(|| {
                    std::env::var("SHELL").unwrap_or_else(|_| "bash".into())
                });

                let config = PtySpawnConfig {
                    command: cmd,
                    cwd,
                    rows,
                    cols,
                };

                match ManagedPty::spawn(config) {
                    Ok(mut pty) => {
                        let pty_id = pty.id().to_string();
                        let dir = chaperone_dir(&self.name);

                        // Get history and take output channel from PTY
                        let history = pty.get_history();
                        let output_rx = pty.take_output_rx().expect("output_rx already taken");

                        // Create input channel for client -> PTY
                        let (input_tx, mut input_rx) = mpsc::channel::<PtyInput>(256);

                        // Create PTY socket
                        match PtySocket::new(&dir, &pty_id) {
                            Ok(pty_socket) => {
                                let socket_path = pty_socket.path().clone();
                                let socket_path_str = socket_path.display().to_string();

                                // Wrap PTY in Arc<Mutex<>> for shared access
                                let pty = Arc::new(Mutex::new(pty));
                                let pty_for_input = Arc::clone(&pty);

                                // Store handle for cleanup
                                self.ptys.insert(
                                    pty_id.clone(),
                                    PtyHandle {
                                        pty,
                                        socket_path: socket_path.clone(),
                                        room_id: room_id.clone(),
                                    },
                                );

                                // Spawn task to run socket server (notifies on exit)
                                let exit_tx = exit_tx.clone();
                                let exit_pty_id = pty_id.clone();
                                let exit_room_id = room_id.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = pty_socket.run(history, output_rx, input_tx).await {
                                        eprintln!("bzc: PTY socket error: {}", e);
                                    }
                                    // Notify that PTY has exited
                                    let _ = exit_tx.send(PtyExitEvent {
                                        pty_id: exit_pty_id,
                                        room_id: exit_room_id,
                                    }).await;
                                });

                                // Spawn task to handle input from socket -> PTY
                                tokio::spawn(async move {
                                    while let Some(input) = input_rx.recv().await {
                                        let mut pty = pty_for_input.lock().unwrap();
                                        match input {
                                            PtyInput::Data(data) => {
                                                if let Err(e) = pty.write(&data) {
                                                    eprintln!("bzc: PTY write error: {}", e);
                                                    break;
                                                }
                                            }
                                            PtyInput::Resize { rows, cols } => {
                                                if let Err(e) = pty.resize(rows, cols) {
                                                    eprintln!("bzc: PTY resize error: {}", e);
                                                }
                                            }
                                        }
                                    }
                                });

                                let response = protocol::encode(&ControlMessage::PtyAttached {
                                    pty_id,
                                    room_id,
                                    socket: socket_path_str,
                                })?;
                                stream.write_all(&response).await?;
                            }
                            Err(e) => {
                                eprintln!("bzc: failed to create PTY socket: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("bzc: failed to spawn PTY: {}", e);
                    }
                }
            }
            ControlMessage::KillPty { pty_id } => {
                if let Some(handle) = self.ptys.remove(&pty_id) {
                    // Clean up socket file
                    let _ = std::fs::remove_file(&handle.socket_path);
                }
            }
            // These are outgoing messages, shouldn't receive them
            ControlMessage::Ready
            | ControlMessage::PtyAttached { .. }
            | ControlMessage::PtyDetached { .. } => {}
        }

        Ok(())
    }
}

impl Drop for Chaperone {
    fn drop(&mut self) {
        // Clean up socket
        let socket_path = self.control_socket_path();
        let _ = std::fs::remove_file(&socket_path);
    }
}

/// Read a control message from stream
async fn read_control_message(stream: &mut UnixStream) -> std::io::Result<ControlMessage> {
    // Read length prefix
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;

    // Read payload
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;

    protocol::decode(&payload).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}
