//! PTY management for the chaperone
//!
//! Manages PTYs with output buffering for session persistence.

use std::collections::VecDeque;
use std::io::{Read, Write};

use color_eyre::eyre::{eyre, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;

/// Default output buffer size per PTY (1MB)
const DEFAULT_BUFFER_SIZE: usize = 1024 * 1024;

/// PTY spawn configuration
#[derive(Debug, Clone)]
pub struct PtySpawnConfig {
    /// Command to run
    pub command: String,
    /// Working directory (optional)
    pub cwd: Option<String>,
    /// Initial terminal rows
    pub rows: u16,
    /// Initial terminal columns
    pub cols: u16,
}

/// PTY info for protocol messages
#[derive(Debug, Clone)]
pub struct PtyInfo {
    /// PTY identifier (UUID string)
    pub id: String,
    /// Command being run
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// Whether the process is still running
    pub running: bool,
}

/// A managed PTY with output buffering
pub struct ManagedPty {
    /// PTY identifier (UUID string)
    id: String,
    /// Command being run
    command: String,
    /// Working directory
    cwd: Option<String>,
    /// Whether the process is still running
    running: bool,
    /// Master PTY handle (for resize)
    master: Box<dyn MasterPty + Send>,
    /// Writer to send input to the PTY
    writer: Box<dyn Write + Send>,
    /// Receiver for PTY output (Option to allow taking ownership)
    output_rx: Option<mpsc::Receiver<Vec<u8>>>,
    /// Output history buffer (ring buffer)
    history: VecDeque<u8>,
    /// Max buffer size
    buffer_size: usize,
}

impl ManagedPty {
    /// Spawn a new PTY
    pub fn spawn(config: PtySpawnConfig) -> Result<Self> {
        let id = uuid::Uuid::new_v4().to_string();

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: config.rows,
                cols: config.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| eyre!("Failed to open PTY: {}", e))?;

        // Parse command string
        let parts: Vec<&str> = config.command.split_whitespace().collect();
        let (bin, args) = parts
            .split_first()
            .ok_or_else(|| eyre!("Empty command string"))?;

        let mut cmd = CommandBuilder::new(bin);
        cmd.args(args);
        if let Some(dir) = &config.cwd {
            cmd.cwd(dir);
        }

        let _child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| eyre!("Failed to spawn command: {}", e))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| eyre!("Failed to clone PTY reader: {}", e))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| eyre!("Failed to get PTY writer: {}", e))?;

        // Spawn blocking task to read from PTY
        let (tx, rx) = mpsc::channel(256);
        tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            id,
            command: config.command,
            cwd: config.cwd,
            running: true,
            master: pair.master,
            writer,
            output_rx: Some(rx),
            history: VecDeque::with_capacity(DEFAULT_BUFFER_SIZE),
            buffer_size: DEFAULT_BUFFER_SIZE,
        })
    }

    /// Get the PTY ID
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Check if the PTY is still running
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Get PTY info for protocol messages
    pub fn info(&self) -> PtyInfo {
        PtyInfo {
            id: self.id.clone(),
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            running: self.running,
        }
    }

    /// Write input to the PTY
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        if !self.running {
            return Ok(());
        }
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Resize the PTY
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| eyre!("Failed to resize PTY: {}", e))?;
        Ok(())
    }

    /// Poll for output, buffering it and returning any new data
    ///
    /// Returns None if no data available or if output_rx was taken.
    /// Updates running state if PTY exits.
    pub fn poll_output(&mut self) -> Option<Vec<u8>> {
        let output_rx = self.output_rx.as_mut()?;
        let mut collected = Vec::new();

        loop {
            match output_rx.try_recv() {
                Ok(data) => {
                    // Add to history buffer
                    for &byte in &data {
                        if self.history.len() >= self.buffer_size {
                            self.history.pop_front();
                        }
                        self.history.push_back(byte);
                    }
                    collected.extend(data);
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.running = false;
                    break;
                }
            }
        }

        if collected.is_empty() {
            None
        } else {
            Some(collected)
        }
    }

    /// Take ownership of the output receiver
    ///
    /// Used when handing off to PtySocket for client I/O.
    /// After calling this, poll_output() will return None.
    pub fn take_output_rx(&mut self) -> Option<mpsc::Receiver<Vec<u8>>> {
        self.output_rx.take()
    }

    /// Get the buffered history
    pub fn get_history(&self) -> Vec<u8> {
        self.history.iter().copied().collect()
    }
}
