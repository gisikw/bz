//! Session-backed PTY
//!
//! A PTY implementation that communicates through the session daemon
//! instead of directly managing a PTY.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use vt100::Parser;

use crate::protocol::ClientMessage;
use crate::pty::{ActivityState, PtyStatus};

/// How long to wait for screen to settle before confirming activity
const ACTIVITY_SETTLE_TIME: Duration = Duration::from_millis(500);

/// How long to ignore activity after resize
const RESIZE_COOLDOWN: Duration = Duration::from_millis(2000);

/// Number of lines to keep in scrollback buffer
const SCROLLBACK_LINES: usize = 10000;

/// Hash the screen contents for comparison
fn hash_screen(screen: &vt100::Screen) -> u64 {
    let mut hasher = DefaultHasher::new();
    screen.contents().hash(&mut hasher);
    hasher.finish()
}

/// A PTY backed by the session daemon
pub struct SessionPty {
    /// PTY ID in the session
    pub id: usize,
    /// Terminal emulator state
    pub parser: Parser,
    /// Activity state
    pub activity: ActivityState,
    /// Scroll offset from bottom
    pub scroll_offset: usize,
    /// Process status
    pub status: PtyStatus,
    /// Resize cooldown
    resize_cooldown_until: Option<Instant>,
    /// Channel to send messages to session
    session_tx: mpsc::Sender<ClientMessage>,
    /// Whether history has been fully received
    history_complete: bool,
}

impl SessionPty {
    /// Create a new session-backed PTY
    pub fn new(id: usize, rows: u16, cols: u16, session_tx: mpsc::Sender<ClientMessage>) -> Self {
        Self {
            id,
            parser: Parser::new(rows, cols, SCROLLBACK_LINES),
            activity: ActivityState::default(),
            scroll_offset: 0,
            status: PtyStatus::Running,
            resize_cooldown_until: None,
            session_tx,
            history_complete: false,
        }
    }

    /// Process output from daemon
    ///
    /// Returns true if any data was processed.
    pub fn process_daemon_output(&mut self, data: &[u8], is_focused: bool) -> bool {
        if data.is_empty() {
            return false;
        }

        // Check if we're in resize cooldown
        let in_cooldown = self
            .resize_cooldown_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false);

        // Capture snapshot before processing if Idle and unfocused
        let pre_snapshot = if !is_focused && !in_cooldown && self.activity == ActivityState::Idle {
            Some(hash_screen(self.parser.screen()))
        } else {
            None
        };

        // Count bells
        let bells = if !is_focused && !in_cooldown {
            data.iter().filter(|&&b| b == 0x07).count() as u32
        } else {
            0
        };

        // Process through parser
        self.parser.process(data);

        // Update activity state
        if !is_focused && !in_cooldown {
            match &mut self.activity {
                ActivityState::Idle => {
                    self.activity = ActivityState::Pending {
                        since: Instant::now(),
                        snapshot: pre_snapshot.unwrap_or_else(|| hash_screen(self.parser.screen())),
                        bells,
                    };
                }
                ActivityState::Pending { bells: b, .. } => {
                    *b += bells;
                }
                ActivityState::Active(count) => {
                    *count += bells;
                }
            }
        }

        true
    }

    /// Process history data from daemon
    pub fn process_history(&mut self, data: &[u8]) {
        self.parser.process(data);
    }

    /// Mark history as complete
    pub fn mark_history_complete(&mut self) {
        self.history_complete = true;
    }

    /// Check if history is complete
    pub fn is_history_complete(&self) -> bool {
        self.history_complete
    }

    /// Resize the PTY
    pub fn resize(&mut self, rows: u16, cols: u16) {
        // Update parser size
        self.parser.screen_mut().set_size(rows, cols);

        // Set cooldown
        if let ActivityState::Pending { .. } = self.activity {
            self.activity = ActivityState::Idle;
        }
        self.resize_cooldown_until = Some(Instant::now() + RESIZE_COOLDOWN);

        // Send resize to daemon
        let msg = ClientMessage::Resize {
            pty_id: self.id,
            rows,
            cols,
        };
        let _ = self.session_tx.try_send(msg);
    }

    /// Write input to the PTY (via daemon)
    pub fn write(&self, data: &[u8]) {
        let msg = ClientMessage::Input {
            pty_id: self.id,
            data: data.to_vec(),
        };
        let _ = self.session_tx.try_send(msg);
    }

    /// Check pending activity and promote to Active if settled
    pub fn check_pending_activity(&mut self) {
        if let ActivityState::Pending {
            since,
            snapshot,
            bells,
        } = self.activity
        {
            if since.elapsed() >= ACTIVITY_SETTLE_TIME {
                let current_hash = hash_screen(self.parser.screen());
                if current_hash != snapshot {
                    self.activity = ActivityState::Active(bells);
                } else {
                    self.activity = ActivityState::Idle;
                }
            }
        }
    }

    /// Clear activity state
    pub fn clear_activity(&mut self) {
        self.activity = ActivityState::Idle;
    }

    /// Mark PTY as exited
    pub fn mark_exited(&mut self) {
        self.status = PtyStatus::Exited;
    }

    /// Get the terminal screen
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Apply scroll offset for rendering
    pub fn apply_scroll_for_render(&mut self) {
        self.parser.screen_mut().set_scrollback(self.scroll_offset);
    }

    /// Reset scroll view after rendering
    pub fn reset_scroll_view(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    /// Check if scrolled
    pub fn is_scrolled(&self) -> bool {
        self.scroll_offset > 0
    }

    /// Get scrollback length
    pub fn scrollback_len(&mut self) -> usize {
        let current = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let max_available = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(current);
        max_available
    }

    /// Scroll up
    pub fn scroll_up(&mut self, lines: usize) -> bool {
        let was_at_bottom = self.scroll_offset == 0;
        let max_scroll = self.scrollback_len();
        self.scroll_offset = (self.scroll_offset + lines).min(max_scroll);
        was_at_bottom && self.scroll_offset > 0
    }

    /// Scroll down
    pub fn scroll_down(&mut self, lines: usize) -> bool {
        let was_scrolled = self.scroll_offset > 0;
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        was_scrolled && self.scroll_offset == 0
    }

    /// Scroll to bottom
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }
}
