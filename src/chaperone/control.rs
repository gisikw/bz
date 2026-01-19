//! Control socket handling for the chaperone
//!
//! Manages the control socket that bzd uses to send commands.

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
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("bz/chaperones")
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
        }

        // Remove stale socket if exists
        let _ = std::fs::remove_file(&socket_path);

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

        // Read and handle messages
        loop {
            // Read length prefix
            let mut len_buf = [0u8; 4];
            match stream.read_exact(&mut len_buf).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
            let len = u32::from_le_bytes(len_buf) as usize;

            // Read payload
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).await?;

            let msg = protocol::decode(&payload)?;
            self.handle_message(msg, &mut stream).await?;
        }

        Ok(())
    }

    /// Handle a control message
    async fn handle_message(
        &mut self,
        msg: ControlMessage,
        stream: &mut UnixStream,
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
                                    },
                                );

                                // Spawn task to run socket server
                                tokio::spawn(async move {
                                    if let Err(e) = pty_socket.run(history, output_rx, input_tx).await {
                                        eprintln!("bzc: PTY socket error: {}", e);
                                    }
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
