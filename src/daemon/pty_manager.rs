//! PTY management for the daemon
//!
//! Manages multiple PTYs with output buffering for session persistence.
//!
//! Used by bzd binary.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{Read, Write};

use color_eyre::eyre::Result;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;

use crate::protocol::{PtyConfig, PtyInfo};

/// Default output buffer size per PTY (1MB)
const DEFAULT_BUFFER_SIZE: usize = 1024 * 1024;

/// A managed PTY with output buffering
pub struct ManagedPty {
    /// PTY identifier
    pub id: usize,
    /// Channel name
    pub name: String,
    /// Command being run
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// Whether the process is still running
    pub running: bool,
    /// Master PTY handle (for resize)
    master: Box<dyn MasterPty + Send>,
    /// Writer to send input to the PTY
    writer: Box<dyn Write + Send>,
    /// Receiver for PTY output
    output_rx: mpsc::Receiver<Vec<u8>>,
    /// Output history buffer (ring buffer)
    history: VecDeque<u8>,
    /// Max buffer size
    buffer_size: usize,
}

impl ManagedPty {
    /// Spawn a new PTY
    pub fn spawn(id: usize, config: &PtyConfig, rows: u16, cols: u16) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| color_eyre::eyre::eyre!("Failed to open PTY: {}", e))?;

        // Parse command string
        let parts: Vec<&str> = config.command.split_whitespace().collect();
        let (bin, args) = parts
            .split_first()
            .ok_or_else(|| color_eyre::eyre::eyre!("Empty command string"))?;

        let mut cmd = CommandBuilder::new(bin);
        cmd.args(args);
        if let Some(dir) = &config.cwd {
            cmd.cwd(dir);
        }

        let _child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to spawn command: {}", e))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| color_eyre::eyre::eyre!("Failed to clone PTY reader: {}", e))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| color_eyre::eyre::eyre!("Failed to get PTY writer: {}", e))?;

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
            name: config.name.clone(),
            command: config.command.clone(),
            cwd: config.cwd.clone(),
            running: true,
            master: pair.master,
            writer,
            output_rx: rx,
            history: VecDeque::with_capacity(DEFAULT_BUFFER_SIZE),
            buffer_size: DEFAULT_BUFFER_SIZE,
        })
    }

    /// Get PTY info for protocol
    pub fn info(&self) -> PtyInfo {
        PtyInfo {
            id: self.id,
            name: self.name.clone(),
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
            .map_err(|e| color_eyre::eyre::eyre!("Failed to resize PTY: {}", e))?;
        Ok(())
    }

    /// Poll for output, buffering it and returning any new data
    ///
    /// Returns None if no data available, Some(data) if new data received.
    /// Updates running state if PTY exits.
    pub fn poll_output(&mut self) -> Option<Vec<u8>> {
        let mut collected = Vec::new();

        loop {
            match self.output_rx.try_recv() {
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

    /// Get the buffered history
    pub fn get_history(&self) -> Vec<u8> {
        self.history.iter().copied().collect()
    }
}

/// Manages all PTYs in a session
pub struct PtyManager {
    ptys: Vec<ManagedPty>,
    next_id: usize,
    default_rows: u16,
    default_cols: u16,
}

impl PtyManager {
    /// Create a new PTY manager
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            ptys: Vec::new(),
            next_id: 0,
            default_rows: rows,
            default_cols: cols,
        }
    }

    /// Spawn a PTY from config
    pub fn spawn(&mut self, config: &PtyConfig) -> Result<PtyInfo> {
        let id = self.next_id;
        self.next_id += 1;

        let pty = ManagedPty::spawn(id, config, self.default_rows, self.default_cols)?;
        let info = pty.info();
        self.ptys.push(pty);
        Ok(info)
    }

    /// Get PTY by ID
    pub fn get(&self, id: usize) -> Option<&ManagedPty> {
        self.ptys.iter().find(|p| p.id == id)
    }

    /// Get mutable PTY by ID
    pub fn get_mut(&mut self, id: usize) -> Option<&mut ManagedPty> {
        self.ptys.iter_mut().find(|p| p.id == id)
    }

    /// Get all PTY infos
    pub fn infos(&self) -> Vec<PtyInfo> {
        self.ptys.iter().map(|p| p.info()).collect()
    }

    /// Poll all PTYs for output
    ///
    /// Returns vec of (pty_id, data) for PTYs with new output
    pub fn poll_all(&mut self) -> Vec<(usize, Vec<u8>)> {
        let mut outputs = Vec::new();
        for pty in &mut self.ptys {
            if let Some(data) = pty.poll_output() {
                outputs.push((pty.id, data));
            }
        }
        outputs
    }

    /// Check if all PTYs have exited
    pub fn all_exited(&self) -> bool {
        self.ptys.iter().all(|p| !p.running)
    }

    /// Get history for a PTY
    pub fn get_history(&self, id: usize) -> Option<Vec<u8>> {
        self.get(id).map(|p| p.get_history())
    }

    /// Update default terminal size
    pub fn set_default_size(&mut self, rows: u16, cols: u16) {
        self.default_rows = rows;
        self.default_cols = cols;
    }
}
