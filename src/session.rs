//! Client-side session management
//!
//! Handles connection to the session daemon and provides
//! a PTY-like interface backed by the daemon.

use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use color_eyre::eyre::{eyre, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::daemon::{find_session, stop_session};
use crate::protocol::{
    decode, encode, ClientMessage, DaemonMessage, PtyInfo, SessionInfo,
};

/// A connection to the session daemon
pub struct Session {
    /// Session info from daemon
    info: SessionInfo,
    /// Sender for messages to write task
    write_tx: mpsc::Sender<ClientMessage>,
    /// Receiver for messages from read task
    read_rx: mpsc::Receiver<DaemonMessage>,
}

impl Session {
    /// Get a clone of the write channel for use by SessionPty
    pub fn write_tx(&self) -> mpsc::Sender<ClientMessage> {
        self.write_tx.clone()
    }
}

/// Options for connecting to a session
#[derive(Default)]
pub struct ConnectOptions {
    /// Take over from existing client if connected
    pub takeover: bool,
}

impl Session {
    /// Connect to an existing session or spawn a new one
    pub async fn connect_or_spawn(config: &Config, opts: ConnectOptions) -> Result<Self> {
        // Check for existing session
        if let Some(socket_path) = find_session() {
            return Self::connect(&socket_path, opts).await;
        }

        // Spawn new daemon
        Self::spawn_and_connect(config).await
    }

    /// Connect to an existing session
    async fn connect(socket_path: &PathBuf, opts: ConnectOptions) -> Result<Self> {
        let mut stream = UnixStream::connect(socket_path).await?;

        // Send attach message
        let attach_msg = if opts.takeover {
            ClientMessage::AttachTakeover
        } else {
            ClientMessage::Attach
        };
        write_message(&mut stream, &attach_msg).await?;

        // Read response
        let response = read_message(&mut stream).await?;

        match response {
            DaemonMessage::Welcome(info) => {
                let (write_tx, read_rx) = Self::setup_io_tasks(stream);
                Ok(Self {
                    info,
                    write_tx,
                    read_rx,
                })
            }
            DaemonMessage::Rejected { reason } => Err(eyre!("{}", reason)),
            _ => Err(eyre!("Unexpected response from daemon")),
        }
    }

    /// Spawn a new daemon and connect to it
    async fn spawn_and_connect(_config: &Config) -> Result<Self> {
        // Get terminal size
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));

        // Spawn daemon process
        let output = Command::new("bzd")
            .args([rows.to_string(), cols.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .output()?;

        // Read socket path from stdout
        let socket_path = String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_string();

        if socket_path.is_empty() {
            return Err(eyre!("Failed to get socket path from daemon"));
        }

        let socket_path = PathBuf::from(&socket_path);

        // Wait a bit for daemon to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Connect
        Self::connect(&socket_path, ConnectOptions::default()).await
    }

    /// Set up I/O tasks for the connection
    fn setup_io_tasks(
        stream: UnixStream,
    ) -> (mpsc::Sender<ClientMessage>, mpsc::Receiver<DaemonMessage>) {
        let (read_half, mut write_half) = stream.into_split();

        let (write_tx, mut write_rx) = mpsc::channel::<ClientMessage>(256);
        let (read_tx, read_rx) = mpsc::channel::<DaemonMessage>(256);

        // Write task
        tokio::spawn(async move {
            while let Some(msg) = write_rx.recv().await {
                let encoded =
                    encode(&msg).expect("Failed to encode message");
                if write_half.write_all(&encoded).await.is_err() {
                    break;
                }
            }
        });

        // Read task
        let mut read_half = read_half;
        tokio::spawn(async move {
            loop {
                match read_message_owned(&mut read_half).await {
                    Ok(msg) => {
                        if read_tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        (write_tx, read_rx)
    }

    /// Get session info
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    /// Get PTY infos
    pub fn ptys(&self) -> &[PtyInfo] {
        &self.info.ptys
    }

    /// Send input to a PTY
    pub async fn write(&self, pty_id: usize, data: &[u8]) -> Result<()> {
        let msg = ClientMessage::Input {
            pty_id,
            data: data.to_vec(),
        };
        self.write_tx
            .send(msg)
            .await
            .map_err(|_| eyre!("Failed to send to daemon"))
    }

    /// Resize a PTY
    pub async fn resize(&self, pty_id: usize, rows: u16, cols: u16) -> Result<()> {
        let msg = ClientMessage::Resize { pty_id, rows, cols };
        self.write_tx
            .send(msg)
            .await
            .map_err(|_| eyre!("Failed to send to daemon"))
    }

    /// Receive a message from the daemon
    pub async fn recv(&mut self) -> Option<DaemonMessage> {
        self.read_rx.recv().await
    }

    /// Try to receive a message without blocking
    pub fn try_recv(&mut self) -> Option<DaemonMessage> {
        self.read_rx.try_recv().ok()
    }

    /// Detach from session (daemon stays alive)
    pub async fn detach(&self) -> Result<()> {
        let msg = ClientMessage::Detach;
        let _ = self.write_tx.send(msg).await;
        Ok(())
    }

    /// Quit session (kills daemon)
    pub async fn quit(&self) -> Result<()> {
        let msg = ClientMessage::Quit;
        let _ = self.write_tx.send(msg).await;
        Ok(())
    }
}

async fn read_message(stream: &mut UnixStream) -> io::Result<DaemonMessage> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;

    decode(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

async fn read_message_owned(
    stream: &mut tokio::net::unix::OwnedReadHalf,
) -> io::Result<DaemonMessage> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;

    decode(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

async fn write_message(stream: &mut UnixStream, msg: &ClientMessage) -> io::Result<()> {
    let encoded = encode(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    stream.write_all(&encoded).await
}

/// Stop any running session
pub fn stop() -> Result<()> {
    if let Some(socket_path) = find_session() {
        stop_session(&socket_path)?;
    }
    Ok(())
}
