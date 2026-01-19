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
//! - `spawn [rows] [cols]` - Start bz (default: 24x80)
//! - `send <text>` - Send text (supports \x02 for Ctrl+B, \n for newline, etc.)
//! - `screen` - Dump current screen contents
//! - `cursor` - Show screen with cursor position
//! - `resize <rows> <cols>` - Resize the PTY
//! - `wait <ms>` - Wait and collect output
//! - `status` - Check if bz is still running
//! - `quit` - Exit driver
//!
//! ## Environment Variables
//!
//! For test isolation:
//! - `BZ_SESSION_DIR` - Session socket directory
//! - `BZ_DATA_DIR` - Data directory
//! - `BZ_CONDUIT_PORT` - Matrix server port

use std::io::{self, BufRead};

use bz::test_support::{parse_escape_sequences, PtyDriver};

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
        let cmd = parts.first().copied().unwrap_or("");
        let arg = parts.get(1).copied().unwrap_or("");

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
