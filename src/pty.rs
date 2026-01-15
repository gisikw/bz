//! PTY management for bz
//!
//! Each PTY represents a shell session with its own terminal emulator state.

use std::io::{Read, Write};

use color_eyre::eyre::Result;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;
use vt100::Parser;

/// A PTY instance with its terminal emulator state
pub struct Pty {
    /// Unique identifier for this PTY
    pub id: usize,
    /// Terminal emulator state (parses escape sequences, maintains screen)
    pub parser: Parser,
    /// Master PTY handle (for resize)
    master: Box<dyn MasterPty + Send>,
    /// Writer to send input to the PTY
    writer: Box<dyn Write + Send>,
    /// Receiver for PTY output
    output_rx: mpsc::Receiver<Vec<u8>>,
}

impl Pty {
    /// Spawn a new PTY with the user's default shell
    pub fn spawn(id: usize, rows: u16, cols: u16) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| color_eyre::eyre::eyre!("Failed to open PTY: {}", e))?;

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".into());
        let cmd = CommandBuilder::new(shell);
        let _child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| color_eyre::eyre::eyre!("Failed to spawn shell: {}", e))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| color_eyre::eyre::eyre!("Failed to clone PTY reader: {}", e))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| color_eyre::eyre::eyre!("Failed to get PTY writer: {}", e))?;

        let parser = Parser::new(rows, cols, 0);

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
            parser,
            master: pair.master,
            writer,
            output_rx: rx,
        })
    }

    /// Resize the PTY and parser
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| color_eyre::eyre::eyre!("Failed to resize PTY: {}", e))?;
        self.parser.set_size(rows, cols);
        Ok(())
    }

    /// Write input to the PTY
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Process any pending output from the PTY
    /// Returns true if any data was processed
    pub fn process_output(&mut self) -> bool {
        let mut processed = false;
        while let Ok(data) = self.output_rx.try_recv() {
            self.parser.process(&data);
            processed = true;
        }
        processed
    }

    /// Get the terminal screen for rendering
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }
}
