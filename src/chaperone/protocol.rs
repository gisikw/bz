//! Control protocol for chaperone-to-bzd communication
//!
//! Messages are length-prefixed bincode, matching the main protocol.

use serde::{Deserialize, Serialize};

/// Messages sent over the control socket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMessage {
    // From bzd to chaperone
    /// Spawn a new PTY
    SpawnPty {
        /// Room ID for Matrix association (future use)
        room_id: String,
        /// Working directory (optional)
        cwd: Option<String>,
        /// Command to run (optional, defaults to shell)
        command: Option<String>,
        /// Terminal rows
        rows: u16,
        /// Terminal columns
        cols: u16,
    },
    /// Kill a PTY
    KillPty {
        /// PTY ID to kill
        pty_id: String,
    },

    // From chaperone to bzd
    /// Chaperone is ready to accept commands
    Ready,
    /// A PTY has been attached
    PtyAttached {
        /// PTY ID
        pty_id: String,
        /// Room ID (for Matrix association)
        room_id: String,
        /// Socket path for PTY I/O
        socket: String,
    },
    /// A PTY has been detached (exited)
    PtyDetached {
        /// PTY ID
        pty_id: String,
        /// Room ID
        room_id: String,
    },
}

/// Encode a control message with length prefix
pub fn encode(msg: &ControlMessage) -> Result<Vec<u8>, bincode::Error> {
    let payload = bincode::serialize(msg)?;
    let len = (payload.len() as u32).to_le_bytes();
    let mut result = Vec::with_capacity(4 + payload.len());
    result.extend_from_slice(&len);
    result.extend(payload);
    Ok(result)
}

/// Decode a control message from bytes (without length prefix)
pub fn decode(data: &[u8]) -> Result<ControlMessage, bincode::Error> {
    bincode::deserialize(data)
}
