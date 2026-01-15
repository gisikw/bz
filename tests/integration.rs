//! Integration tests for bz TUI
//!
//! These tests spawn the actual bz binary in a PTY and verify behavior.
//!
//! **Note**: These tests require a real TTY environment to run.
//! They may not work in CI or sandboxed environments.
//!
//! Run with: cargo test --test integration
//!
//! To skip in CI: cargo test --test integration -- --ignored

use std::time::Duration;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Get the path to the bz binary
fn bz_binary() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    format!("{}/target/debug/bz", manifest_dir)
}

/// Test: spawn bz, verify shell renders, send Ctrl+Q to quit
#[test]
fn test_bz_spawns_shell_and_renders() -> Result<()> {
    use portable_pty::{native_pty_system, PtySize};
    use std::io::{Read, Write};
    use std::sync::mpsc;

    println!("Creating PTY...");
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    println!("Spawning bz...");
    let mut cmd = portable_pty::CommandBuilder::new(bz_binary());
    cmd.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(cmd)?;

    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let mut parser = vt100::Parser::new(24, 80, 0);

    // Read output in a thread, collecting all data for a period
    // Give more time for shell to initialize
    println!("Reading output...");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut all_data = Vec::new();
        let mut buf = [0u8; 4096];
        let start = std::time::Instant::now();

        // Read for up to 1s, collecting all output (shell needs time to start)
        while start.elapsed() < Duration::from_millis(1000) {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    all_data.extend_from_slice(&buf[..n]);
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(all_data);
    });

    match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(data) => {
            println!("Read {} bytes total", data.len());
            parser.process(&data);
            let screen = parser.screen().contents();
            println!("Screen contents: {:?}", screen.trim());

            // The shell should have rendered something (prompt, etc.)
            // We just verify we got non-empty output
            assert!(
                !screen.trim().is_empty(),
                "Screen should have content from shell"
            );
        }
        Err(_) => {
            panic!("Timeout reading from PTY - no data received!");
        }
    }

    // Send Ctrl+Q to quit (0x11 is Ctrl+Q)
    writer.write_all(&[0x11])?;
    writer.flush()?;

    // Wait for process to exit (with timeout)
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            println!("Process exited with status: {:?}", status);
            assert!(status.success(), "Process should exit successfully");
            break;
        }
        if start.elapsed() > Duration::from_secs(2) {
            panic!("Process did not exit within 2 seconds after Ctrl+Q was sent");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    Ok(())
}

/// Test that regular keys go to shell, only Ctrl+Q quits
#[test]
fn test_keys_go_to_shell_ctrl_q_quits() -> Result<()> {
    use portable_pty::{native_pty_system, PtySize};
    use std::io::{Read, Write};
    use std::sync::mpsc;

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = portable_pty::CommandBuilder::new(bz_binary());
    cmd.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(cmd)?;

    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;

    // Wait for shell to start
    let (tx, rx) = mpsc::channel();
    let reader_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_millis(500) {
            let _ = reader.read(&mut buf);
        }
        tx.send(reader).unwrap();
    });

    // Send regular keys (including 'q' which should NOT quit since it goes to shell)
    writer.write_all(b"echo hello")?;
    writer.flush()?;

    // Get the reader back (wait for drain thread to finish)
    let _reader = rx.recv_timeout(Duration::from_secs(2))?;
    reader_thread.join().unwrap();

    // Give time for keys to be processed
    std::thread::sleep(Duration::from_millis(200));

    // Process should still be running (keys went to shell, not as quit command)
    assert!(
        child.try_wait()?.is_none(),
        "Process should still be running - regular keys go to shell"
    );

    // Now send Ctrl+Q to quit (0x11)
    writer.write_all(&[0x11])?;
    writer.flush()?;

    // Should exit now
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            assert!(status.success(), "Process should exit successfully");
            break;
        }
        if start.elapsed() > Duration::from_secs(2) {
            panic!("Process did not exit after Ctrl+Q");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    Ok(())
}
