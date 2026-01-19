//! bz - A multi-agent coordination TUI
//!
//! This library provides the core functionality for bz,
//! including PTY management, session persistence, and UI components.

pub mod channel;
pub mod chaperone;
pub mod chaperone_channel;
pub mod chaperone_pty;
pub mod chat_view;
pub mod config;
pub mod daemon;
pub mod matrix_client;
pub mod picker;
pub mod protocol;
pub mod pty;
pub mod room_view;
pub mod session;
pub mod session_pty;
pub mod sidebar;
pub mod terminal;
pub mod user_chaperone;
