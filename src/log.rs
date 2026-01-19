//! Logging utilities for bz
//!
//! Writes to ~/.local/share/bz/bz.log for debugging without mangling the TUI.

use std::fs::OpenOptions;
use std::io::Write;

/// Log a message to ~/.local/share/bz/bz.log
pub fn log(msg: &str) {
    let log_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("bz/bz.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
        let _ = writeln!(file, "[{}] {}", timestamp, msg);
    }
}
