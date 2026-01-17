//! Channel abstraction for bz
//!
//! A channel is a named workspace containing one or more PTYs.
//! Channels are the user-facing concept; PTYs are the implementation detail.

use crate::pty::{ActivityState, Pty};

/// Unique identifier for a channel
pub type ChannelId = usize;

/// A channel is a named workspace with one or more PTYs
pub struct Channel {
    /// Unique identifier
    pub id: ChannelId,
    /// Display name
    pub name: String,
    /// The PTY for this channel (currently one per channel)
    pub pty: Pty,
}

impl Channel {
    /// Create a new channel with a single PTY
    pub fn new(id: ChannelId, name: String, pty: Pty) -> Self {
        Self { id, name, pty }
    }

    /// Get the channel's activity state
    pub fn activity(&self) -> &ActivityState {
        &self.pty.activity
    }

    /// Clear activity (when channel becomes focused)
    pub fn clear_activity(&mut self) {
        self.pty.clear_activity();
    }

    /// Process PTY output
    pub fn process_output(&mut self, is_focused: bool) {
        self.pty.process_output(is_focused);
    }
}
