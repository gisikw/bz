//! Wire protocol for bz daemon communication
//!
//! All messages are length-prefixed: 4-byte little-endian length, then bincode payload.

use serde::{Deserialize, Serialize};

/// Session metadata sent on connect
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Session UUID
    pub session_id: String,
    /// Config hash (to detect config changes)
    pub config_hash: u64,
    /// PTY configurations
    pub ptys: Vec<PtyInfo>,
    /// Last focused channel index
    pub focused: usize,
}

/// Information about a single PTY in the session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyInfo {
    /// PTY identifier (index in session)
    pub id: usize,
    /// Channel name
    pub name: String,
    /// Command being run
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
    /// Whether the process is still running
    pub running: bool,
}

/// PTY spawn configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyConfig {
    /// Channel name
    pub name: String,
    /// Command to run
    pub command: String,
    /// Working directory
    pub cwd: Option<String>,
}

/// Messages from client to daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Initial connection (fails if another client connected)
    Attach,
    /// Take over from existing client
    AttachTakeover,
    /// Keyboard input to specific PTY
    Input { pty_id: usize, data: Vec<u8> },
    /// Resize a PTY
    Resize { pty_id: usize, rows: u16, cols: u16 },
    /// Update focused channel (for persistence across detach)
    SetFocus { channel_idx: usize },
    /// Create new PTY
    Spawn(PtyConfig),
    /// Kill a PTY
    Kill { pty_id: usize },
    /// Clean disconnect (daemon stays alive)
    Detach,
    /// Kill daemon and all PTYs
    Quit,
}

/// Messages from daemon to client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonMessage {
    /// Connection accepted, here's session info
    Welcome(SessionInfo),
    /// Connection rejected (another client is connected)
    Rejected { reason: String },
    /// Buffered output for a PTY (sent after Welcome)
    History { pty_id: usize, data: Vec<u8> },
    /// End of history for a PTY
    HistoryEnd { pty_id: usize },
    /// Live PTY output
    Output { pty_id: usize, data: Vec<u8> },
    /// PTY process exited
    PtyExited { pty_id: usize, exit_code: Option<i32> },
    /// New PTY spawned (response to Spawn)
    PtySpawned(PtyInfo),
    /// Another client took over, you're being kicked
    Kicked,
    /// Daemon is shutting down
    Shutdown,
}

/// Encode a message with length prefix
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, bincode::Error> {
    let payload = bincode::serialize(msg)?;
    let len = payload.len() as u32;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend(payload);
    Ok(buf)
}

/// Decode length prefix, returns (length, bytes consumed)
#[allow(dead_code)]
pub fn decode_length(buf: &[u8]) -> Option<(usize, usize)> {
    if buf.len() < 4 {
        return None;
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    Some((len, 4))
}

/// Decode a message from bytes (after length prefix removed)
pub fn decode<T: for<'de> Deserialize<'de>>(buf: &[u8]) -> Result<T, bincode::Error> {
    bincode::deserialize(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_client_message() {
        let msg = ClientMessage::Input {
            pty_id: 0,
            data: b"hello".to_vec(),
        };
        let encoded = encode(&msg).unwrap();

        // Check length prefix
        let (len, consumed) = decode_length(&encoded).unwrap();
        assert_eq!(consumed, 4);

        // Decode payload
        let decoded: ClientMessage = decode(&encoded[4..4 + len]).unwrap();
        match decoded {
            ClientMessage::Input { pty_id, data } => {
                assert_eq!(pty_id, 0);
                assert_eq!(data, b"hello".to_vec());
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_encode_decode_daemon_message() {
        let msg = DaemonMessage::Welcome(SessionInfo {
            session_id: "test-uuid".to_string(),
            config_hash: 12345,
            ptys: vec![PtyInfo {
                id: 0,
                name: "shell".to_string(),
                command: "bash".to_string(),
                cwd: Some("/home".to_string()),
                running: true,
            }],
            focused: 0,
        });
        let encoded = encode(&msg).unwrap();
        let (len, _) = decode_length(&encoded).unwrap();
        let decoded: DaemonMessage = decode(&encoded[4..4 + len]).unwrap();

        match decoded {
            DaemonMessage::Welcome(info) => {
                assert_eq!(info.session_id, "test-uuid");
                assert_eq!(info.ptys.len(), 1);
                assert_eq!(info.ptys[0].name, "shell");
            }
            _ => panic!("Wrong message type"),
        }
    }
}
