//! Chaperone-based channel
//!
//! A channel that manages a PTY through the chaperone.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use color_eyre::eyre::Result;
use vt100::Parser;

use crate::chaperone_pty::ChaperonePtyConnection;
use crate::picker::{HasNameActivity, HasPtyStatus};
use crate::protocol::{ClientMessage, DaemonMessage};
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

/// A channel backed by a chaperone PTY
pub struct ChaperoneChannel {
    /// Unique ID for this channel/PTY
    id: String,
    /// Display name
    pub name: String,
    /// Command
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// PTY connection
    connection: ChaperonePtyConnection,
    /// Terminal emulator state
    parser: Parser,
    /// Activity state
    activity: ActivityState,
    /// Scroll offset from bottom
    pub scroll_offset: usize,
    /// Process status
    status: PtyStatus,
    /// Resize cooldown
    resize_cooldown_until: Option<Instant>,
    /// Whether history has been fully received
    history_complete: bool,
}

impl ChaperoneChannel {
    /// Create a new chaperone channel
    pub async fn new(
        name: String,
        command: String,
        cwd: Option<String>,
        socket_path: PathBuf,
        rows: u16,
        cols: u16,
    ) -> Result<Self> {
        let connection = ChaperonePtyConnection::connect(&socket_path).await?;
        let id = uuid::Uuid::new_v4().to_string();

        Ok(Self {
            id,
            name,
            command,
            cwd,
            connection,
            parser: Parser::new(rows, cols, SCROLLBACK_LINES),
            activity: ActivityState::default(),
            scroll_offset: 0,
            status: PtyStatus::Running,
            resize_cooldown_until: None,
            history_complete: false,
        })
    }

    /// Get the unique ID of this channel
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Process any pending messages from the PTY
    pub fn process_pending(&mut self, is_focused: bool) {
        while let Some(msg) = self.connection.try_recv() {
            match msg {
                DaemonMessage::History { data, .. } => {
                    self.parser.process(&data);
                }
                DaemonMessage::HistoryEnd { .. } => {
                    self.history_complete = true;
                }
                DaemonMessage::Output { data, .. } => {
                    self.process_output(&data, is_focused);
                }
                DaemonMessage::PtyExited { .. } => {
                    self.status = PtyStatus::Exited;
                }
                _ => {}
            }
        }
    }

    /// Process output data
    fn process_output(&mut self, data: &[u8], is_focused: bool) {
        if data.is_empty() {
            return;
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
                        snapshot: pre_snapshot
                            .unwrap_or_else(|| hash_screen(self.parser.screen())),
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

    /// Resize the PTY
    pub fn resize(&mut self, rows: u16, cols: u16) {
        // Update parser size
        self.parser.screen_mut().set_size(rows, cols);

        // Set cooldown
        if let ActivityState::Pending { .. } = self.activity {
            self.activity = ActivityState::Idle;
        }
        self.resize_cooldown_until = Some(Instant::now() + RESIZE_COOLDOWN);

        // Send resize to PTY (fire and forget via channel)
        let tx = self.connection.input_tx();
        let msg = ClientMessage::Resize {
            pty_id: 0,
            rows,
            cols,
        };
        let _ = tx.try_send(msg);
    }

    /// Write input to the PTY
    pub fn write(&self, data: &[u8]) {
        let tx = self.connection.input_tx();
        let msg = ClientMessage::Input {
            pty_id: 0,
            data: data.to_vec(),
        };
        let _ = tx.try_send(msg);
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

impl HasNameActivity for ChaperoneChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn activity(&self) -> &ActivityState {
        &self.activity
    }
}

impl HasPtyStatus for ChaperoneChannel {
    fn pty_status(&self) -> &PtyStatus {
        &self.status
    }
}
