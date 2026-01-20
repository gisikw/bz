//! PTY types for bz
//!
//! Shared types used across PTY implementations.

use std::time::Instant;

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
