//! PTY management for bz
//!
//! Each PTY represents a shell session with its own terminal emulator state.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use color_eyre::eyre::Result;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;
use vt100::Parser;

/// How long to wait for screen to settle before confirming activity
const ACTIVITY_SETTLE_TIME: Duration = Duration::from_millis(500);

/// Activity state for a PTY
///
/// Uses content-based detection with settling to avoid false positives
/// from transient screen changes (e.g., tmux redraw, echo+clear).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ActivityState {
    /// No unread output
    #[default]
    Idle,
    /// Output received, waiting for screen to settle
    /// Contains: (timestamp, screen hash before output, accumulated bells)
    Pending {
        since: Instant,
        snapshot: u64,
        bells: u32,
    },
    /// Confirmed activity (screen changed and settled), with bell count
    Active(u32),
}

/// Hash the screen contents for comparison
fn hash_screen(screen: &vt100::Screen) -> u64 {
    let mut hasher = DefaultHasher::new();
    screen.contents().hash(&mut hasher);
    hasher.finish()
}

/// A PTY instance with its terminal emulator state
pub struct Pty {
    /// Unique identifier for this PTY
    pub id: usize,
    /// Terminal emulator state (parses escape sequences, maintains screen)
    pub parser: Parser,
    /// Activity state (unread output, bells)
    pub activity: ActivityState,
    /// Master PTY handle (for resize)
    master: Box<dyn MasterPty + Send>,
    /// Writer to send input to the PTY
    writer: Box<dyn Write + Send>,
    /// Receiver for PTY output
    output_rx: mpsc::Receiver<Vec<u8>>,
}

impl Pty {
    /// Spawn a new PTY with the given command
    ///
    /// # Arguments
    /// * `id` - Unique identifier for this PTY
    /// * `rows` - Terminal height
    /// * `cols` - Terminal width
    /// * `command` - Command to run (e.g., "bash", "zsh")
    /// * `cwd` - Optional working directory
    pub fn spawn(
        id: usize,
        rows: u16,
        cols: u16,
        command: &str,
        cwd: Option<&str>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| color_eyre::eyre::eyre!("Failed to open PTY: {}", e))?;

        // Parse command string into binary and arguments
        let parts: Vec<&str> = command.split_whitespace().collect();
        let (bin, args) = parts.split_first().ok_or_else(|| {
            color_eyre::eyre::eyre!("Empty command string")
        })?;

        let mut cmd = CommandBuilder::new(bin);
        cmd.args(args);
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }
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
            activity: ActivityState::default(),
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
    ///
    /// If `is_focused` is false, updates activity state for unread output.
    /// Uses content-based detection: captures screen snapshot before processing,
    /// then waits for settle time before confirming activity.
    ///
    /// Bells (0x07) are accumulated but only shown if activity is confirmed.
    ///
    /// Returns true if any data was processed.
    pub fn process_output(&mut self, is_focused: bool) -> bool {
        let mut processed = false;
        let mut total_bells = 0u32;

        // Capture snapshot BEFORE processing if we're Idle and unfocused
        // (we'll use this to detect if screen actually changed)
        let pre_snapshot = if !is_focused && self.activity == ActivityState::Idle {
            Some(hash_screen(self.parser.screen()))
        } else {
            None
        };

        while let Ok(data) = self.output_rx.try_recv() {
            processed = true;

            // Count bells before processing (ASCII BEL = 0x07)
            if !is_focused {
                total_bells += data.iter().filter(|&&b| b == 0x07).count() as u32;
            }

            // Always process through parser
            self.parser.process(&data);
        }

        // Update activity state if not focused and we processed data
        if !is_focused && processed {
            match &mut self.activity {
                ActivityState::Idle => {
                    // Transition to Pending with the pre-processing snapshot
                    self.activity = ActivityState::Pending {
                        since: Instant::now(),
                        snapshot: pre_snapshot.unwrap_or_else(|| hash_screen(self.parser.screen())),
                        bells: total_bells,
                    };
                }
                ActivityState::Pending { bells, .. } => {
                    // Still pending - accumulate bells
                    // Don't reset timer - settle counts from first output
                    *bells += total_bells;
                }
                ActivityState::Active(count) => {
                    // Already confirmed active - just add bells
                    *count += total_bells;
                }
            }
        }

        processed
    }

    /// Check pending activity and promote to Active if settled and changed
    ///
    /// Call this periodically (e.g., in render loop) to check if pending
    /// activity has settled and should be promoted to confirmed activity.
    pub fn check_pending_activity(&mut self) {
        if let ActivityState::Pending {
            since,
            snapshot,
            bells,
        } = self.activity
        {
            if since.elapsed() >= ACTIVITY_SETTLE_TIME {
                // Settle time elapsed - check if screen actually changed
                let current_hash = hash_screen(self.parser.screen());
                if current_hash != snapshot {
                    // Screen changed - confirm activity
                    self.activity = ActivityState::Active(bells);
                } else {
                    // Screen returned to original - no real activity
                    self.activity = ActivityState::Idle;
                }
            }
        }
    }

    /// Clear activity state (call when PTY becomes focused)
    pub fn clear_activity(&mut self) {
        self.activity = ActivityState::Idle;
    }

    /// Get the terminal screen for rendering
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }
}
