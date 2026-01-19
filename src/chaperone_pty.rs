//! Chaperone PTY connection
//!
//! Connects to a chaperone's PTY socket for terminal I/O.

use std::path::Path;

use color_eyre::eyre::{Result, WrapErr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use crate::protocol::{encode, decode, ClientMessage, DaemonMessage};

/// A connection to a chaperone PTY socket
pub struct ChaperonePtyConnection {
    /// Channel to send input to the PTY
    input_tx: mpsc::Sender<ClientMessage>,
    /// Channel to receive output from the PTY
    output_rx: mpsc::Receiver<DaemonMessage>,
}

impl ChaperonePtyConnection {
    /// Connect to a PTY socket
    pub async fn connect(socket_path: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket_path)
            .await
            .wrap_err_with(|| format!("Failed to connect to PTY socket: {}", socket_path.display()))?;

        let (read_half, write_half) = stream.into_split();

        // Channel for input (from app to PTY)
        let (input_tx, input_rx) = mpsc::channel::<ClientMessage>(256);
        // Channel for output (from PTY to app)
        let (output_tx, output_rx) = mpsc::channel::<DaemonMessage>(256);

        // Spawn task to read from socket
        tokio::spawn(read_task(read_half, output_tx));

        // Spawn task to write to socket
        tokio::spawn(write_task(write_half, input_rx));

        Ok(Self {
            input_tx,
            output_rx,
        })
    }

    /// Send input to the PTY
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        let msg = ClientMessage::Input {
            pty_id: 0,
            data: data.to_vec(),
        };
        self.input_tx
            .send(msg)
            .await
            .wrap_err("Failed to send input to PTY")?;
        Ok(())
    }

    /// Send resize to the PTY
    pub async fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let msg = ClientMessage::Resize {
            pty_id: 0,
            rows,
            cols,
        };
        self.input_tx
            .send(msg)
            .await
            .wrap_err("Failed to send resize to PTY")?;
        Ok(())
    }

    /// Try to receive a message (non-blocking)
    pub fn try_recv(&mut self) -> Option<DaemonMessage> {
        self.output_rx.try_recv().ok()
    }

    /// Receive a message (blocking)
    pub async fn recv(&mut self) -> Option<DaemonMessage> {
        self.output_rx.recv().await
    }

    /// Get a clone of the input sender for synchronous use
    pub fn input_tx(&self) -> mpsc::Sender<ClientMessage> {
        self.input_tx.clone()
    }
}

/// Task that reads from the socket and sends to the output channel
async fn read_task(
    mut read_half: tokio::net::unix::OwnedReadHalf,
    output_tx: mpsc::Sender<DaemonMessage>,
) {
    loop {
        // Read length prefix
        let mut len_buf = [0u8; 4];
        if read_half.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        let len = u32::from_le_bytes(len_buf) as usize;

        // Read payload
        let mut payload = vec![0u8; len];
        if read_half.read_exact(&mut payload).await.is_err() {
            break;
        }

        // Decode and send
        if let Ok(msg) = decode::<DaemonMessage>(&payload) {
            if output_tx.send(msg).await.is_err() {
                break;
            }
        }
    }
}

/// Task that reads from the input channel and writes to the socket
async fn write_task(
    mut write_half: tokio::net::unix::OwnedWriteHalf,
    mut input_rx: mpsc::Receiver<ClientMessage>,
) {
    while let Some(msg) = input_rx.recv().await {
        if let Ok(encoded) = encode(&msg) {
            if write_half.write_all(&encoded).await.is_err() {
                break;
            }
        }
    }
}
