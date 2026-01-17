//! bz session daemon
//!
//! Manages PTY sessions that persist across bz restarts.
//! Spawned by `bz` when no existing session is found.

use std::env;
use std::io::{self, Write};

use color_eyre::eyre::Result;

// Import from bz crate
use bz::config::Config;
use bz::daemon::Daemon;

fn main() -> Result<()> {
    color_eyre::install()?;

    // Parse args: bzd <rows> <cols> [--foreground]
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: bzd <rows> <cols> [--foreground]");
        std::process::exit(1);
    }

    let rows: u16 = args[1].parse().unwrap_or(24);
    let cols: u16 = args[2].parse().unwrap_or(80);
    let foreground = args.get(3).map(|s| s == "--foreground").unwrap_or(false);

    // Load config
    let config = Config::load()?;

    // Create daemon (doesn't spawn PTYs yet - needs tokio runtime)
    let mut daemon = Daemon::new(&config, rows, cols)?;

    // Print socket path for parent process
    println!("{}", daemon.socket_path().display());
    io::stdout().flush()?;

    if !foreground {
        // Daemonize: double fork
        unsafe {
            match libc::fork() {
                -1 => {
                    eprintln!("First fork failed");
                    std::process::exit(1);
                }
                0 => {
                    // Child - create new session
                    libc::setsid();

                    // Second fork
                    match libc::fork() {
                        -1 => {
                            eprintln!("Second fork failed");
                            std::process::exit(1);
                        }
                        0 => {
                            // Grandchild - this is the daemon
                            // Close stdin/stdout/stderr
                            libc::close(0);
                            libc::close(1);
                            libc::close(2);

                            // Redirect to /dev/null
                            let null = std::ffi::CString::new("/dev/null").unwrap();
                            libc::open(null.as_ptr(), libc::O_RDWR);
                            libc::dup(0);
                            libc::dup(0);
                        }
                        _ => {
                            // First child exits
                            std::process::exit(0);
                        }
                    }
                }
                _ => {
                    // Parent exits immediately after printing socket path
                    std::process::exit(0);
                }
            }
        }
    }

    // Run daemon (in foreground or after daemonizing)
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        // Spawn PTYs inside the runtime
        daemon.spawn_ptys(&config)?;
        daemon.run().await
    })?;

    Ok(())
}
