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

// Re-exports for bzc binary (not used within lib, but part of public API)
#[allow(unused_imports)]
pub use config::{ChaperoneConfig, ChaperoneMode};
#[allow(unused_imports)]
pub use control::Chaperone;
#[allow(unused_imports)]
pub use pty::{ManagedPty, PtyInfo, PtySpawnConfig};
#[allow(unused_imports)]
pub use pty_socket::{PtyInput, PtySocket};
