//! bz-pty-driver - Interactive PTY driver for testing bz
//!
//! Spawns bz in a PTY and provides a simple command interface for:
//! - Sending keystrokes
//! - Capturing screen state
//! - Resizing the terminal
//!
//! Designed for use by Claude or other automation tools that lack a TTY.
//!
//! ## Commands
//!
//! - `spawn` - Start bz (uses BZ_SESSION_DIR, BZ_DATA_DIR, BZ_CONDUIT_PORT for isolation)
//! - `send <text>` - Send text (supports \x02 for Ctrl+B, \n for newline, etc.)
//! - `screen` - Dump current screen contents
//! - `cursor` - Show cursor position
//! - `resize <rows> <cols>` - Resize the PTY
//! - `wait <ms>` - Wait for specified milliseconds
//! - `quit` - Clean shutdown
//!
//! ## Escape Sequences
//!
//! In `send` command:
//! - `\n` - newline (0x0a)
//! - `\r` - carriage return (0x0d)
//! - `\t` - tab (0x09)
//! - `\x##` - hex byte (e.g., `\x02` for Ctrl+B)
//! - `\\` - literal backslash

use std::io::{self, BufRead, Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

struct PtyDriver {
    parser: vt100::Parser,
    writer: Box<dyn Write + Send>,
    _reader_thread: thread::JoinHandle<()>,
    output_rx: mpsc::Receiver<Vec<u8>>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    rows: u16,
    cols: u16,
}

impl PtyDriver {
    fn spawn(rows: u16, cols: u16) -> io::Result<Self> {
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

        // Pass through isolation env vars if set
        if let Ok(v) = std::env::var("BZ_SESSION_DIR") {
            cmd.env("BZ_SESSION_DIR", v);
        }
        if let Ok(v) = std::env::var("BZ_DATA_DIR") {
            cmd.env("BZ_DATA_DIR", v);
        }
        if let Ok(v) = std::env::var("BZ_CONDUIT_PORT") {
            cmd.env("BZ_CONDUIT_PORT", v);
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
            rows,
            cols,
        })
    }

    fn process_pending_output(&mut self) {
        // Drain all pending output
        while let Ok(data) = self.output_rx.try_recv() {
            self.parser.process(&data);
        }
    }

    fn wait_and_process(&mut self, ms: u64) {
        let deadline = std::time::Instant::now() + Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            match self.output_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(data) => self.parser.process(&data),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn send(&mut self, text: &str) -> io::Result<()> {
        let bytes = parse_escape_sequences(text);
        self.writer.write_all(&bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    fn screen(&mut self) -> String {
        self.process_pending_output();
        self.parser.screen().contents()
    }

    fn screen_with_cursor(&mut self) -> String {
        self.process_pending_output();
        let screen = self.parser.screen();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let contents = screen.contents();
        format!(
            "{}\n--- cursor: row={}, col={} ---",
            contents, cursor_row, cursor_col
        )
    }

    fn resize(&mut self, rows: u16, cols: u16) -> io::Result<()> {
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

    fn is_running(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            _ => false,
        }
    }
}

fn find_bz_binary() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    // Check debug first, then release
    let debug_path = format!("{}/target/debug/bz", manifest_dir);
    if std::path::Path::new(&debug_path).exists() {
        return debug_path;
    }
    let release_path = format!("{}/target/release/bz", manifest_dir);
    if std::path::Path::new(&release_path).exists() {
        return release_path;
    }
    // Fall back to PATH
    "bz".to_string()
}

fn binary_dir() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/target/debug", manifest_dir)
}

fn parse_escape_sequences(s: &str) -> Vec<u8> {
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

fn print_help() {
    println!("bz-pty-driver - Interactive PTY driver for testing bz");
    println!();
    println!("Commands:");
    println!("  spawn [rows] [cols]  - Start bz (default: 24x80)");
    println!("  send <text>          - Send text (supports \\x02, \\n, etc.)");
    println!("  screen               - Dump current screen contents");
    println!("  cursor               - Show screen with cursor position");
    println!("  resize <rows> <cols> - Resize the PTY");
    println!("  wait <ms>            - Wait and collect output");
    println!("  status               - Check if bz is still running");
    println!("  quit                 - Exit driver");
    println!("  help                 - Show this help");
    println!();
    println!("Escape sequences in send:");
    println!("  \\n     - newline");
    println!("  \\r     - carriage return");
    println!("  \\t     - tab");
    println!("  \\x##   - hex byte (e.g., \\x02 for Ctrl+B)");
    println!("  \\\\     - literal backslash");
    println!();
    println!("Environment variables for isolation:");
    println!("  BZ_SESSION_DIR   - Session socket directory");
    println!("  BZ_DATA_DIR      - Data directory");
    println!("  BZ_CONDUIT_PORT  - Matrix server port");
}

fn main() {
    let mut driver: Option<PtyDriver> = None;
    let stdin = io::stdin();

    println!("bz-pty-driver ready. Type 'help' for commands.");

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let parts: Vec<&str> = line.trim().splitn(2, ' ').collect();
        let cmd = parts.first().map(|s| *s).unwrap_or("");
        let arg = parts.get(1).map(|s| *s).unwrap_or("");

        match cmd {
            "help" | "?" => print_help(),

            "spawn" => {
                if driver.is_some() {
                    println!("ERROR: bz already running. Use 'quit' first.");
                    continue;
                }

                let (rows, cols) = if arg.is_empty() {
                    (24, 80)
                } else {
                    let dims: Vec<&str> = arg.split_whitespace().collect();
                    let rows = dims.first().and_then(|s| s.parse().ok()).unwrap_or(24);
                    let cols = dims.get(1).and_then(|s| s.parse().ok()).unwrap_or(80);
                    (rows, cols)
                };

                match PtyDriver::spawn(rows, cols) {
                    Ok(d) => {
                        driver = Some(d);
                        println!("OK: spawned bz ({}x{})", rows, cols);
                    }
                    Err(e) => println!("ERROR: {}", e),
                }
            }

            "send" => {
                if let Some(ref mut d) = driver {
                    match d.send(arg) {
                        Ok(()) => println!("OK: sent {} bytes", parse_escape_sequences(arg).len()),
                        Err(e) => println!("ERROR: {}", e),
                    }
                } else {
                    println!("ERROR: no bz running. Use 'spawn' first.");
                }
            }

            "screen" => {
                if let Some(ref mut d) = driver {
                    let screen = d.screen();
                    println!("--- screen ---");
                    println!("{}", screen);
                    println!("--- end screen ---");
                } else {
                    println!("ERROR: no bz running. Use 'spawn' first.");
                }
            }

            "cursor" => {
                if let Some(ref mut d) = driver {
                    let screen = d.screen_with_cursor();
                    println!("--- screen ---");
                    println!("{}", screen);
                    println!("--- end screen ---");
                } else {
                    println!("ERROR: no bz running. Use 'spawn' first.");
                }
            }

            "resize" => {
                if let Some(ref mut d) = driver {
                    let dims: Vec<&str> = arg.split_whitespace().collect();
                    let rows: u16 = dims.first().and_then(|s| s.parse().ok()).unwrap_or(24);
                    let cols: u16 = dims.get(1).and_then(|s| s.parse().ok()).unwrap_or(80);
                    match d.resize(rows, cols) {
                        Ok(()) => println!("OK: resized to {}x{}", rows, cols),
                        Err(e) => println!("ERROR: {}", e),
                    }
                } else {
                    println!("ERROR: no bz running. Use 'spawn' first.");
                }
            }

            "wait" => {
                if let Some(ref mut d) = driver {
                    let ms: u64 = arg.parse().unwrap_or(100);
                    d.wait_and_process(ms);
                    println!("OK: waited {}ms", ms);
                } else {
                    println!("ERROR: no bz running. Use 'spawn' first.");
                }
            }

            "status" => {
                if let Some(ref mut d) = driver {
                    if d.is_running() {
                        println!("OK: bz is running");
                    } else {
                        println!("OK: bz has exited");
                    }
                } else {
                    println!("OK: no bz instance");
                }
            }

            "quit" | "exit" => {
                println!("OK: goodbye");
                break;
            }

            "" => {}

            _ => println!("ERROR: unknown command '{}'. Type 'help' for commands.", cmd),
        }
    }
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
