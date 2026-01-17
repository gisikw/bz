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

/// How long to ignore activity after resize (reflow causes output)
/// Set high because heavy apps like Claude Code can take ~1s to reflow
const RESIZE_COOLDOWN: Duration = Duration::from_millis(2000);

/// Number of lines to keep in scrollback buffer
const SCROLLBACK_LINES: usize = 10000;

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

/// PTY process status
#[derive(Debug, Clone, PartialEq, Default)]
pub enum PtyStatus {
    /// Process is running
    #[default]
    Running,
    /// Process has exited (reader channel closed)
    Exited,
}

/// Hash the screen contents for comparison
fn hash_screen(screen: &vt100::Screen) -> u64 {
    let mut hasher = DefaultHasher::new();
    screen.contents().hash(&mut hasher);
    hasher.finish()
}

/// A PTY instance with its terminal emulator state
pub struct Pty {
    /// Unique identifier for this PTY (reserved for future use)
    #[allow(dead_code)]
    pub id: usize,
    /// Terminal emulator state (parses escape sequences, maintains screen)
    pub parser: Parser,
    /// Activity state (unread output, bells)
    pub activity: ActivityState,
    /// Scroll offset from bottom (0 = live/bottom, >0 = scrolled up)
    pub scroll_offset: usize,
    /// Process status (running or exited)
    pub status: PtyStatus,
    /// Cooldown until which activity detection is paused (after resize)
    resize_cooldown_until: Option<Instant>,
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

        let parser = Parser::new(rows, cols, SCROLLBACK_LINES);

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
            scroll_offset: 0,
            status: PtyStatus::Running,
            resize_cooldown_until: None,
            master: pair.master,
            writer,
            output_rx: rx,
        })
    }

    /// Resize the PTY and parser
    ///
    /// Sets a cooldown to ignore activity detection since resize causes screen
    /// reflow which would otherwise trigger false activity.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| color_eyre::eyre::eyre!("Failed to resize PTY: {}", e))?;
        self.parser.screen_mut().set_size(rows, cols);

        // Set cooldown to ignore activity from reflow
        // Reset any pending activity and pause detection briefly
        if let ActivityState::Pending { .. } = self.activity {
            self.activity = ActivityState::Idle;
        }
        self.resize_cooldown_until = Some(Instant::now() + RESIZE_COOLDOWN);

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

        // Check if we're in resize cooldown (skip activity detection)
        let in_cooldown = self
            .resize_cooldown_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false);

        // Capture snapshot BEFORE processing if we're Idle and unfocused
        // (we'll use this to detect if screen actually changed)
        let pre_snapshot = if !is_focused && !in_cooldown && self.activity == ActivityState::Idle {
            Some(hash_screen(self.parser.screen()))
        } else {
            None
        };

        loop {
            match self.output_rx.try_recv() {
                Ok(data) => {
                    processed = true;

                    // Count bells before processing (ASCII BEL = 0x07)
                    // Still count bells even in cooldown - they're intentional
                    if !is_focused && !in_cooldown {
                        total_bells += data.iter().filter(|&&b| b == 0x07).count() as u32;
                    }

                    // Always process through parser
                    self.parser.process(&data);
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    // No more data right now
                    break;
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    // Reader task exited - process has exited
                    self.status = PtyStatus::Exited;
                    break;
                }
            }
        }

        // Update activity state if not focused, not in cooldown, and we processed data
        if !is_focused && !in_cooldown && processed {
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

    /// Prepare screen for rendering with current scroll offset applied
    pub fn apply_scroll_for_render(&mut self) {
        self.parser.screen_mut().set_scrollback(self.scroll_offset);
    }

    /// Reset scroll view after rendering (back to 0 for internal state)
    pub fn reset_scroll_view(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    /// Check if we're in scroll mode (not at bottom)
    pub fn is_scrolled(&self) -> bool {
        self.scroll_offset > 0
    }

    /// Get the total scrollback length (lines available to scroll through)
    /// Note: vt100 doesn't expose this directly, so we use a workaround
    pub fn scrollback_len(&mut self) -> usize {
        // Save current offset
        let current = self.parser.screen().scrollback();
        // Set to max to find the clamped value
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let max_available = self.parser.screen().scrollback();
        // Restore original offset
        self.parser.screen_mut().set_scrollback(current);
        max_available
    }

    /// Scroll up by the given number of lines
    /// Returns true if we entered scroll mode (were at bottom)
    pub fn scroll_up(&mut self, lines: usize) -> bool {
        let was_at_bottom = self.scroll_offset == 0;
        let max_scroll = self.scrollback_len();
        self.scroll_offset = (self.scroll_offset + lines).min(max_scroll);
        was_at_bottom && self.scroll_offset > 0
    }

    /// Scroll down by the given number of lines
    /// Returns true if we exited scroll mode (reached bottom)
    pub fn scroll_down(&mut self, lines: usize) -> bool {
        let was_scrolled = self.scroll_offset > 0;
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        was_scrolled && self.scroll_offset == 0
    }

    /// Scroll to bottom (exit scroll mode)
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }
}
