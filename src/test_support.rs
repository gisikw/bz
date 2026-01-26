//! Test support utilities for bz
//!
//! Provides `PtyDriver` for integration testing - a high-level interface
//! for spawning bz in a PTY, sending input, and asserting on screen state.
//!
//! ## Usage
//!
//! ```no_run
//! use bz::test_support::PtyDriver;
//!
//! let mut driver = PtyDriver::spawn_isolated(24, 120).unwrap();
//! driver.wait_and_process(1000);
//!
//! assert!(driver.screen().contains("#main"));
//!
//! driver.send(r"\x02n").unwrap();  // Ctrl+B n
//! driver.wait_and_process(200);
//!
//! assert!(driver.screen().contains("#build"));
//! ```

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

/// Counter for unique test session directories
static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

/// PTY driver for integration testing bz
///
/// Spawns bz in a pseudo-terminal and provides methods for:
/// - Sending keystrokes
/// - Capturing screen state
/// - Resizing the terminal
pub struct PtyDriver {
    parser: vt100::Parser,
    writer: Box<dyn Write + Send>,
    _reader_thread: thread::JoinHandle<()>,
    output_rx: mpsc::Receiver<Vec<u8>>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    session_dir: Option<String>,
    rows: u16,
    cols: u16,
}

impl PtyDriver {
    /// Spawn bz with custom environment configuration
    pub fn spawn(rows: u16, cols: u16) -> io::Result<Self> {
        Self::spawn_with_config(rows, cols, None, None)
    }

    /// Spawn bz with isolated session directory (for tests)
    ///
    /// Creates a unique session directory to avoid conflicts with
    /// other test runs or the user's production bz instance.
    pub fn spawn_isolated(rows: u16, cols: u16) -> io::Result<Self> {
        let session_dir = unique_session_dir();
        Self::spawn_with_config(rows, cols, Some(session_dir), None)
    }

    /// Spawn bz with a specific working directory (for fixture-based tests)
    pub fn spawn_in_dir(rows: u16, cols: u16, cwd: &str) -> io::Result<Self> {
        let session_dir = unique_session_dir();
        Self::spawn_with_config(rows, cols, Some(session_dir), Some(cwd.to_string()))
    }

    /// Spawn bz with an existing session directory (for pre-setup tests)
    ///
    /// Use this when you need to create state (e.g., stale sockets) before spawning.
    /// The data_dir will be `{session_dir}/data`.
    pub fn spawn_with_session_dir(rows: u16, cols: u16, session_dir: String) -> io::Result<Self> {
        Self::spawn_with_config(rows, cols, Some(session_dir), None)
    }

    fn spawn_with_config(
        rows: u16,
        cols: u16,
        session_dir: Option<String>,
        cwd: Option<String>,
    ) -> io::Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        // Build command for bz
        let mut cmd = CommandBuilder::new(find_bz_binary());
        cmd.env("TERM", "xterm-256color");

        // Add target/debug to PATH so bz can find bzd
        let path = format!(
            "{}:{}",
            binary_dir(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.env("PATH", path);

        // Set isolated directories if provided
        if let Some(ref dir) = session_dir {
            cmd.env("BZ_SESSION_DIR", dir);
            // Also set BZ_DATA_DIR to isolated location for chaperone sockets
            let data_dir = format!("{}/data", dir);
            cmd.env("BZ_DATA_DIR", &data_dir);
        } else if let Ok(v) = std::env::var("BZ_DATA_DIR") {
            // Pass through BZ_DATA_DIR if set in parent environment
            cmd.env("BZ_DATA_DIR", v);
        }
        if let Ok(v) = std::env::var("BZ_CONDUIT_PORT") {
            cmd.env("BZ_CONDUIT_PORT", v);
        }

        // Skip Matrix connection in tests (much faster and more reliable)
        cmd.env("BZ_SKIP_MATRIX", "1");

        // Set working directory if provided
        if let Some(ref dir) = cwd {
            cmd.cwd(dir);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        // Spawn reader thread
        let (tx, rx) = mpsc::channel();
        let reader_thread = thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            parser: vt100::Parser::new(rows, cols, 0),
            writer,
            _reader_thread: reader_thread,
            output_rx: rx,
            master: pair.master,
            child,
            session_dir,
            rows,
            cols,
        })
    }

    /// Process any pending output from bz
    pub fn process_pending_output(&mut self) {
        while let Ok(data) = self.output_rx.try_recv() {
            self.parser.process(&data);
        }
    }

    /// Wait for specified duration, processing output as it arrives
    pub fn wait_and_process(&mut self, ms: u64) {
        let deadline = std::time::Instant::now() + Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            match self.output_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(data) => self.parser.process(&data),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    /// Wait until screen contains specific text, with timeout
    ///
    /// Returns true if text was found, false if timeout expired.
    /// Use this instead of fixed-duration waits to handle timing variations.
    pub fn wait_for_content(&mut self, needle: &str, timeout_ms: u64) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        while std::time::Instant::now() < deadline {
            match self.output_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(data) => self.parser.process(&data),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if self.parser.screen().contents().contains(needle) {
                return true;
            }
        }
        false
    }

    /// Wait until the TUI is ready (status bar visible)
    ///
    /// The status bar contains "^K search" or "^B leader" which indicates
    /// the TUI has fully initialized and is ready for input.
    pub fn wait_for_tui_ready(&mut self, timeout_ms: u64) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        while std::time::Instant::now() < deadline {
            match self.output_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(data) => self.parser.process(&data),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            let screen = self.parser.screen().contents();
            if screen.contains("^K search") || screen.contains("^B leader") {
                return true;
            }
        }
        false
    }

    /// Send text to bz (supports escape sequences like \x02 for Ctrl+B)
    pub fn send(&mut self, text: &str) -> io::Result<()> {
        let bytes = parse_escape_sequences(text);
        self.writer.write_all(&bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Send raw bytes to bz
    pub fn send_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Get current screen contents as text
    pub fn screen(&mut self) -> String {
        self.process_pending_output();
        self.parser.screen().contents()
    }

    /// Get screen contents with cursor position info
    pub fn screen_with_cursor(&mut self) -> String {
        self.process_pending_output();
        let screen = self.parser.screen();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let contents = screen.contents();
        format!(
            "{}\n--- cursor: row={}, col={} ---",
            contents, cursor_row, cursor_col
        )
    }

    /// Resize the PTY
    pub fn resize(&mut self, rows: u16, cols: u16) -> io::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        // vt100::Parser doesn't support resize, so create a new one
        // This loses scroll history but that's acceptable for testing
        self.parser = vt100::Parser::new(rows, cols, 0);
        self.rows = rows;
        self.cols = cols;
        Ok(())
    }

    /// Check if bz is still running
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Wait for bz to exit, with timeout
    pub fn wait_for_exit(&mut self, timeout_ms: u64) -> io::Result<bool> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(timeout_ms);

        loop {
            if let Some(_status) = self.child.try_wait()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
            {
                return Ok(true);
            }
            if start.elapsed() > timeout {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Graceful quit: send Ctrl+B Q y (leader mode quit with confirmation)
    pub fn quit(&mut self) -> io::Result<()> {
        self.send(r"\x02")?; // Ctrl+B enters leader mode
        std::thread::sleep(Duration::from_millis(100));
        self.send("Q")?; // Q shows quit confirmation (uppercase)
        std::thread::sleep(Duration::from_millis(100));
        self.send("y")?; // y confirms quit
        Ok(())
    }

    /// Get the session directory (if using isolated mode)
    pub fn session_dir(&self) -> Option<&str> {
        self.session_dir.as_deref()
    }

    /// Get current dimensions
    pub fn dimensions(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }
}

impl Drop for PtyDriver {
    fn drop(&mut self) {
        // Try graceful shutdown first
        let _ = self.quit();

        // Give it a moment to exit
        std::thread::sleep(Duration::from_millis(200));

        // Clean up session directory if we created one
        if let Some(ref dir) = self.session_dir {
            // Stop any daemon in this session dir
            let _ = std::process::Command::new(find_bz_binary())
                .arg("stop")
                .env("BZ_SESSION_DIR", dir)
                .env("PATH", format!("{}:{}", binary_dir(), std::env::var("PATH").unwrap_or_default()))
                .output();

            // Remove the directory
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Generate a unique session directory for test isolation
pub fn unique_session_dir() -> String {
    let count = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    format!("/tmp/bz-test-{}-{}", pid, count)
}

/// Find the bz binary (debug build preferred, then release, then exe dir, then PATH)
pub fn find_bz_binary() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    // Try debug build first
    let debug_path = format!("{}/target/debug/bz", manifest_dir);
    if std::path::Path::new(&debug_path).exists() {
        return debug_path;
    }

    // Try release build
    let release_path = format!("{}/target/release/bz", manifest_dir);
    if std::path::Path::new(&release_path).exists() {
        return release_path;
    }

    // Try same directory as test executable (works in Nix sandbox)
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(parent) = exe_path.parent() {
            let sibling_path = parent.join("bz");
            if sibling_path.exists() {
                return sibling_path.display().to_string();
            }
        }
    }

    // Fall back to PATH
    "bz".to_string()
}

/// Get the directory containing bz binaries
pub fn binary_dir() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    // Try debug build first
    let debug_dir = format!("{}/target/debug", manifest_dir);
    if std::path::Path::new(&format!("{}/bz", debug_dir)).exists() {
        return debug_dir;
    }

    // Try release build
    let release_dir = format!("{}/target/release", manifest_dir);
    if std::path::Path::new(&format!("{}/bz", release_dir)).exists() {
        return release_dir;
    }

    // Fall back to current exe's directory (works in Nix sandbox)
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(parent) = exe_path.parent() {
            return parent.display().to_string();
        }
    }

    // Last resort: just return debug dir and hope for the best
    debug_dir
}

/// Parse escape sequences in a string (e.g., \x02 for Ctrl+B, \n for newline)
pub fn parse_escape_sequences(s: &str) -> Vec<u8> {
    let mut result = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push(0x0a),
                Some('r') => result.push(0x0d),
                Some('t') => result.push(0x09),
                Some('\\') => result.push(b'\\'),
                Some('x') => {
                    // Parse \x## hex escape
                    let mut hex = String::new();
                    if let Some(&c1) = chars.peek() {
                        if c1.is_ascii_hexdigit() {
                            hex.push(chars.next().unwrap());
                        }
                    }
                    if let Some(&c2) = chars.peek() {
                        if c2.is_ascii_hexdigit() {
                            hex.push(chars.next().unwrap());
                        }
                    }
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        result.push(byte);
                    }
                }
                Some(other) => {
                    result.push(b'\\');
                    result.extend(other.to_string().as_bytes());
                }
                None => result.push(b'\\'),
            }
        } else {
            result.extend(c.to_string().as_bytes());
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_escape_sequences() {
        assert_eq!(parse_escape_sequences("hello"), b"hello");
        assert_eq!(parse_escape_sequences(r"\n"), vec![0x0a]);
        assert_eq!(parse_escape_sequences(r"\r"), vec![0x0d]);
        assert_eq!(parse_escape_sequences(r"\t"), vec![0x09]);
        assert_eq!(parse_escape_sequences(r"\\"), vec![b'\\']);
        assert_eq!(parse_escape_sequences(r"\x02"), vec![0x02]);
        assert_eq!(parse_escape_sequences(r"\x02n"), vec![0x02, b'n']);
        assert_eq!(
            parse_escape_sequences(r"hello\nworld"),
            b"hello\nworld".to_vec()
        );
    }
}
