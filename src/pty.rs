//! PTY types for bz
//!
//! Shared types used across PTY implementations.

/// Activity state for a screen (PTY or chat)
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ActivityState {
    /// No unread activity
    #[default]
    Idle,
    /// Has unread activity
    Active,
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
