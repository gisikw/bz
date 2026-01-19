//! Chaperone - the atomic PTY management unit
//!
//! A chaperone (bzc) manages PTYs for a single principal (user or agent).
//! It communicates with bzd via a control socket and exposes PTY streams
//! via per-PTY sockets.

pub mod config;
pub mod control;
pub mod protocol;
pub mod pty;
pub mod pty_socket;

pub use config::{ChaperoneConfig, ChaperoneMode};
pub use control::Chaperone;
pub use pty::{ManagedPty, PtyInfo, PtySpawnConfig};
pub use pty_socket::{PtySocket, PtyInput};
