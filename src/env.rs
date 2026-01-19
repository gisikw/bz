//! Environment-based configuration for test isolation
//!
//! All paths and ports can be overridden via environment variables
//! to allow running isolated test instances alongside production.
//!
//! Environment variables:
//! - `BZ_SESSION_DIR`: Override session socket directory
//! - `BZ_DATA_DIR`: Override data directory (Conduit config, database, logs)
//! - `BZ_CONDUIT_PORT`: Override Conduit Matrix server port (default: 6167)

use std::path::PathBuf;

/// Default Conduit port
const DEFAULT_CONDUIT_PORT: u16 = 6167;

/// Get the data directory for bz
///
/// Contains: Conduit config, Matrix database, logs
/// Can be overridden with `BZ_DATA_DIR` env var.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BZ_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("bz")
}

/// Get the session directory for bz
///
/// Contains: daemon sockets, agent configs
/// Can be overridden with `BZ_SESSION_DIR` env var.
pub fn session_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BZ_SESSION_DIR") {
        return PathBuf::from(dir);
    }
    dirs::state_dir()
        .or_else(|| dirs::data_local_dir())
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("bz/sessions")
}

/// Get the Conduit Matrix server port
///
/// Can be overridden with `BZ_CONDUIT_PORT` env var.
pub fn conduit_port() -> u16 {
    std::env::var("BZ_CONDUIT_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_CONDUIT_PORT)
}

/// Get the Conduit Matrix server URL
///
/// Returns `http://127.0.0.1:{port}` using the configured port.
pub fn conduit_url() -> String {
    format!("http://127.0.0.1:{}", conduit_port())
}

/// Get the Conduit socket address for health checks
pub fn conduit_addr() -> std::net::SocketAddr {
    format!("127.0.0.1:{}", conduit_port())
        .parse()
        .expect("valid socket addr")
}
