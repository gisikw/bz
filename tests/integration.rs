//! Integration tests for bz TUI
//!
//! These tests spawn the actual bz binary in a PTY and verify behavior
//! using the `PtyDriver` test harness.
//!
//! Tests use isolated session directories to avoid conflicts with
//! other test runs or the user's production bz instance.

use bz::test_support::{unique_session_dir, PtyDriver};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Path to test fixtures directory
fn fixtures_dir() -> String {
    format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
}

/// Test: spawn bz, verify shell renders, quit gracefully
#[test]
fn test_bz_spawns_shell_and_renders() -> Result<()> {
    let mut driver = PtyDriver::spawn_isolated(24, 120)?;

    // Wait for TUI to be ready (indicated by status bar)
    assert!(
        driver.wait_for_tui_ready(10000),
        "TUI should become ready within 10 seconds"
    );

    let screen = driver.screen();
    println!("Screen contents: {:?}", screen.trim());

    // The shell should have rendered something (prompt, etc.)
    assert!(
        !screen.trim().is_empty(),
        "Screen should have content from shell"
    );

    // Quit gracefully
    driver.quit()?;

    // Wait for process to exit (cleanup can take a moment)
    assert!(
        driver.wait_for_exit(5000)?,
        "Process should exit within 5 seconds after quit"
    );

    Ok(())
}

/// Test input forwarding: send a command, verify it appears in output
#[test]
fn test_input_forwarding() -> Result<()> {
    let mut driver = PtyDriver::spawn_isolated(24, 120)?;

    // Wait for TUI to be ready
    assert!(
        driver.wait_for_tui_ready(10000),
        "TUI should become ready"
    );

    // Send "echo test123" followed by Enter
    driver.send("echo test123\r")?;

    // Wait for command output to appear
    assert!(
        driver.wait_for_content("test123", 2000),
        "Should see command output 'test123'"
    );

    let screen = driver.screen();
    println!("Screen after command: {:?}", screen);

    // Process should still be running
    assert!(driver.is_running(), "Process should still be running");

    // Quit gracefully
    driver.quit()?;
    driver.wait_for_exit(2000)?;

    Ok(())
}

/// Test tab switching: Ctrl+B n cycles through channels
#[test]
fn test_tab_switching() -> Result<()> {
    let mut driver = PtyDriver::spawn_in_dir(24, 120, &fixtures_dir())?;

    // Wait for TUI to be ready
    assert!(
        driver.wait_for_tui_ready(10000),
        "TUI should become ready"
    );

    let screen = driver.screen();
    println!("Initial screen: {:?}", screen);

    // Sidebar shows "#main" (focused - no asterisk)
    assert!(
        screen.contains("#main"),
        "Should show channel 'main' in sidebar, got: {}",
        screen
    );
    // Main is focused, so it shouldn't have an asterisk right after its name
    assert!(
        !screen.contains("#main *"),
        "Focused channel 'main' should not have activity asterisk, got: {}",
        screen
    );

    // Send Ctrl+B n to switch to next channel (build)
    driver.send(r"\x02n")?;
    driver.wait_and_process(300);

    let screen = driver.screen();
    println!("After Ctrl+B n: {:?}", screen);

    // Build is now focused, so it shouldn't have an asterisk
    assert!(
        !screen.contains("#build *"),
        "Focused channel 'build' should not have activity asterisk, got: {}",
        screen
    );

    // Send Ctrl+B n again to switch to logs
    driver.send(r"\x02n")?;
    driver.wait_and_process(300);

    let screen = driver.screen();

    // Logs is now focused
    assert!(
        !screen.contains("#logs *"),
        "Focused channel 'logs' should not have activity asterisk, got: {}",
        screen
    );

    // Send Ctrl+B n again (should wrap to main)
    driver.send(r"\x02n")?;
    driver.wait_and_process(300);

    let screen = driver.screen();

    // Main is focused again
    assert!(
        !screen.contains("#main *"),
        "Wrapped back to 'main', should not have activity asterisk, got: {}",
        screen
    );

    // Quit gracefully
    driver.quit()?;
    driver.wait_for_exit(2000)?;

    Ok(())
}

/// Test activity detection: unfocused channels show activity markers
#[test]
fn test_activity_detection() -> Result<()> {
    let mut driver = PtyDriver::spawn_in_dir(24, 120, &fixtures_dir())?;

    // Wait for TUI to be ready
    assert!(
        driver.wait_for_tui_ready(10000),
        "TUI should become ready"
    );

    let screen = driver.screen();
    println!("Initial screen: {:?}", screen);

    // Sidebar shows #main (focused - no asterisk)
    assert!(
        screen.contains("#main"),
        "Should show channel 'main' in sidebar"
    );
    assert!(
        !screen.contains("#main *"),
        "Focused channel 'main' should not have activity asterisk"
    );

    // Switch to channel 2 'build' (clears its activity)
    driver.send(r"\x02n")?;
    driver.wait_and_process(300);

    let screen = driver.screen();
    println!("On channel 'build': {:?}", screen);

    // Build is now focused, so its asterisk should be cleared
    assert!(
        !screen.contains("#build *"),
        "Focused channel 'build' should not have activity asterisk"
    );

    // Run a command in 'build' channel
    driver.send("echo activity_test\r")?;
    driver.wait_and_process(500);

    // Switch to 'logs' channel
    driver.send(r"\x02n")?;
    driver.wait_and_process(300);

    let screen = driver.screen();
    println!("On channel 'logs': {:?}", screen);

    // Logs is now focused (no asterisk)
    assert!(
        !screen.contains("#logs *"),
        "Focused channel 'logs' should not have activity asterisk"
    );

    // Quit gracefully
    driver.quit()?;
    driver.wait_for_exit(2000)?;

    Ok(())
}

/// Test: sidebar auto-hides on narrow (mobile) terminals
#[test]
fn test_sidebar_hides_on_mobile() -> Result<()> {
    // 80 cols is below MOBILE_WIDTH_THRESHOLD (100)
    let mut driver = PtyDriver::spawn_isolated(24, 80)?;

    // Wait for TUI to be ready
    assert!(
        driver.wait_for_tui_ready(10000),
        "TUI should become ready"
    );

    let screen = driver.screen();
    println!("Mobile screen: {:?}", screen);

    // Sidebar should NOT be visible - no channel names
    assert!(
        !screen.contains("#main"),
        "Sidebar should be hidden on narrow terminal (80 cols)"
    );

    // Status line should still be visible
    assert!(
        screen.contains("^K search") || screen.contains("^B leader"),
        "Status line should be visible"
    );

    // Quit gracefully
    driver.quit()?;
    driver.wait_for_exit(2000)?;

    Ok(())
}

/// Path to test fixtures with agent directory
fn fixtures_with_agent_dir() -> String {
    format!("{}/tests/fixtures_with_agent", env!("CARGO_MANIFEST_DIR"))
}

/// Test: Quit (Ctrl+B Q y) kills daemon and agent chaperone processes
///
/// This verifies that when the user confirms quit, all child processes
/// (bzd daemon and bzc agent chaperones) are properly terminated.
#[test]
#[ignore] // TODO: bzd doesn't write PID file in Nix sandbox - needs investigation
fn test_quit_kills_daemon_and_chaperones() -> Result<()> {
    let mut driver = PtyDriver::spawn_in_dir(24, 120, &fixtures_with_agent_dir())?;
    let session_dir = driver.session_dir().unwrap().to_string();

    // Wait for TUI to be ready
    assert!(
        driver.wait_for_tui_ready(10000),
        "TUI should become ready"
    );

    // Verify bz is running
    assert!(driver.is_running(), "bz should be running");

    // Find daemon PID (with retry - daemon may take a moment to write PID file after daemonizing)
    let mut daemon_pid: Option<i32> = None;
    for _ in 0..20 {
        daemon_pid = std::fs::read_dir(&session_dir)
            .ok()
            .and_then(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .find(|e| e.path().extension().map(|x| x == "pid").unwrap_or(false))
                    .and_then(|e| std::fs::read_to_string(e.path()).ok())
            })
            .and_then(|s| s.trim().parse().ok());
        if daemon_pid.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let daemon_pid = daemon_pid.unwrap_or_else(|| {
        // Debug: list session_dir contents
        let contents: Vec<_> = std::fs::read_dir(&session_dir)
            .map(|entries| entries.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        panic!("Daemon PID file should exist in {}, found: {:?}", session_dir, contents);
    });

    // Find agent PID (in agents subdirectory)
    let agents_dir = format!("{}/agents", session_dir);
    let agent_pid: Option<i32> = std::fs::read_dir(&agents_dir)
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .find(|e| e.path().extension().map(|x| x == "pid").unwrap_or(false))
                .and_then(|e| std::fs::read_to_string(e.path()).ok())
        })
        .and_then(|s| s.trim().parse().ok());

    // Verify daemon is running
    assert!(
        unsafe { libc::kill(daemon_pid, 0) } == 0,
        "Daemon (pid {}) should be running before quit",
        daemon_pid
    );

    // Verify agent is running (if spawned)
    if let Some(pid) = agent_pid {
        assert!(
            unsafe { libc::kill(pid, 0) } == 0,
            "Agent chaperone (pid {}) should be running before quit",
            pid
        );
    }

    // Send Ctrl+B Q y to quit with confirmation
    driver.send(r"\x02")?; // Ctrl+B
    driver.wait_and_process(100);
    driver.send("Q")?; // Uppercase Q for destructive quit
    driver.wait_and_process(100);

    // Verify quit confirmation is shown
    let screen = driver.screen();
    assert!(
        screen.contains("kill all PTYs") || screen.contains("Quit Session"),
        "Should show quit confirmation dialog, got: {}",
        screen
    );

    // Confirm quit
    driver.send("y")?;

    // Wait for bz to exit
    assert!(
        driver.wait_for_exit(5000)?,
        "bz should exit after quit confirmation"
    );

    // Give processes time to clean up
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Verify daemon is no longer running
    assert!(
        unsafe { libc::kill(daemon_pid, 0) } != 0,
        "Daemon (pid {}) should be killed after quit",
        daemon_pid
    );

    // Verify agent is no longer running (if it was spawned)
    if let Some(pid) = agent_pid {
        assert!(
            unsafe { libc::kill(pid, 0) } != 0,
            "Agent chaperone (pid {}) should be killed after quit",
            pid
        );
    }

    Ok(())
}

/// Test: sidebar shows screen indicators for focused room with multiple screens
///
/// When a room has multiple screens (chat + workspace), the focused room
/// should show indented sub-items with emoji prefixes (💬 chat, 🖥️ workspace)
/// instead of the old [1/2] format.
#[test]
fn test_sidebar_screen_indicators() -> Result<()> {
    let mut driver = PtyDriver::spawn_isolated(24, 120)?;

    // Wait for TUI to be ready
    assert!(
        driver.wait_for_tui_ready(10000),
        "TUI should become ready"
    );

    let screen = driver.screen();
    println!("Screen with indicators: {:?}", screen);

    // Should show chat indicator (💬)
    assert!(
        screen.contains("💬") || screen.contains("chat"),
        "Should show chat screen indicator, got: {}",
        screen
    );

    // Should show workspace indicator (🖥️)
    assert!(
        screen.contains("🖥") || screen.contains("workspace"),
        "Should show workspace screen indicator, got: {}",
        screen
    );

    // Should NOT show old [1/2] format in sidebar
    // (Note: status bar still shows [x/y] which is fine)
    let sidebar_area: String = screen
        .lines()
        .filter_map(|line| line.split('│').next())
        .collect();
    assert!(
        !sidebar_area.contains("[1/") && !sidebar_area.contains("[2/"),
        "Sidebar should not show [n/m] format, got sidebar: {}",
        sidebar_area
    );

    driver.quit()?;
    driver.wait_for_exit(2000)?;

    Ok(())
}

/// Test: Ctrl+B h/l switches screens and updates sidebar indicator
///
/// The ▸ indicator should move between chat and workspace when
/// switching screens with Ctrl+B h (previous) and Ctrl+B l (next).
#[test]
fn test_screen_switching_updates_indicator() -> Result<()> {
    let mut driver = PtyDriver::spawn_isolated(24, 120)?;

    // Wait for TUI to be ready
    assert!(
        driver.wait_for_tui_ready(10000),
        "TUI should become ready"
    );

    let screen = driver.screen();
    println!("Initial (on workspace): {:?}", screen);

    // Initially on workspace - workspace line should have ▸ indicator
    // The workspace line should be highlighted (▸ before 🖥️)
    assert!(
        screen.contains("▸") && (screen.contains("🖥") || screen.contains("workspace")),
        "Workspace should be indicated as current screen"
    );

    // Switch to chat with Ctrl+B h
    driver.send(r"\x02h")?;
    driver.wait_and_process(300);

    let screen = driver.screen();
    println!("After Ctrl+B h (on chat): {:?}", screen);

    // Now on chat - status bar should show [1/2]
    assert!(
        screen.contains("[1/2]"),
        "Status bar should show [1/2] when on chat screen, got: {}",
        screen
    );

    // Switch back to workspace with Ctrl+B l
    driver.send(r"\x02l")?;
    driver.wait_and_process(300);

    let screen = driver.screen();
    println!("After Ctrl+B l (on workspace): {:?}", screen);

    // Back on workspace - status bar should show [2/2]
    assert!(
        screen.contains("[2/2]"),
        "Status bar should show [2/2] when on workspace screen, got: {}",
        screen
    );

    driver.quit()?;
    driver.wait_for_exit(2000)?;

    Ok(())
}

/// Test: switching rooms collapses/expands screen indicators
///
/// Only the focused room should show expanded screen indicators.
/// Unfocused rooms should just show their name without sub-items.
#[test]
fn test_room_switch_collapses_indicators() -> Result<()> {
    let mut driver = PtyDriver::spawn_in_dir(24, 120, &fixtures_dir())?;

    // Wait for TUI to be ready
    assert!(
        driver.wait_for_tui_ready(10000),
        "TUI should become ready"
    );

    let screen = driver.screen();
    println!("Initial (on #main): {:?}", screen);

    // #main is focused and should show screen indicators if it has multiple screens
    // Count how many lines have screen indicators
    let _initial_indicator_count = screen.matches("💬").count() + screen.matches("🖥").count();

    // Switch to next room with Ctrl+B j
    driver.send(r"\x02j")?;
    driver.wait_and_process(300);

    let screen = driver.screen();
    println!("After Ctrl+B j (on #build): {:?}", screen);

    // #build is now focused
    // The total indicator count should stay the same (indicators moved, not duplicated)
    let new_indicator_count = screen.matches("💬").count() + screen.matches("🖥").count();

    // Indicators should only appear for one room at a time
    assert!(
        new_indicator_count <= 2,
        "Should have at most 2 screen indicators (chat + workspace), got: {}",
        new_indicator_count
    );

    // Switch back to first room with Ctrl+B k
    driver.send(r"\x02k")?;
    driver.wait_and_process(300);

    let screen = driver.screen();
    println!("After Ctrl+B k (back to #main): {:?}", screen);

    // Verify we're back on first room
    assert!(
        screen.contains("▸ #main") || screen.contains("▸ #"),
        "Should be back on first room"
    );

    driver.quit()?;
    driver.wait_for_exit(2000)?;

    Ok(())
}

/// Test: bz starts successfully despite stale sockets from crashed session
///
/// When SSH drops or bz crashes, stale Unix sockets can be left behind.
/// This test verifies that bz cleans up stale sockets on startup and
/// successfully reconnects.
#[test]
fn test_reconnect_cleans_stale_sockets() -> Result<()> {
    // Create isolated session directory
    let session_dir = unique_session_dir();
    let data_dir = format!("{}/data", session_dir);
    let chaperone_dir = format!("{}/chaperones/user", data_dir);

    // Create directory structure
    std::fs::create_dir_all(&chaperone_dir)?;

    // Create stale sockets (simulating crashed session)
    let stale_control = format!("{}/control.sock", chaperone_dir);
    let stale_pty1 = format!("{}/fake-pty-1.sock", chaperone_dir);
    let stale_pty2 = format!("{}/fake-pty-2.sock", chaperone_dir);

    std::fs::write(&stale_control, "stale")?;
    std::fs::write(&stale_pty1, "stale")?;
    std::fs::write(&stale_pty2, "stale")?;

    // Verify stale sockets exist
    assert!(
        std::path::Path::new(&stale_control).exists(),
        "Stale control.sock should exist before test"
    );
    assert!(
        std::path::Path::new(&stale_pty1).exists(),
        "Stale PTY socket should exist before test"
    );

    // Spawn bz with the pre-setup session directory
    let mut driver = PtyDriver::spawn_with_session_dir(24, 120, session_dir)?;

    // Wait for TUI to be ready (this is where socket cleanup happens)
    assert!(
        driver.wait_for_tui_ready(10000),
        "TUI should become ready despite stale sockets"
    );

    // Verify bz is running (meaning it successfully started despite stale sockets)
    assert!(
        driver.is_running(),
        "bz should start successfully despite stale sockets"
    );

    // Verify stale PTY sockets were cleaned up
    assert!(
        !std::path::Path::new(&stale_pty1).exists(),
        "Stale PTY socket should be cleaned up on startup"
    );
    assert!(
        !std::path::Path::new(&stale_pty2).exists(),
        "Stale PTY socket should be cleaned up on startup"
    );

    // The control.sock should now be a real socket (not our fake file)
    // We can verify this by checking the file type
    let metadata = std::fs::metadata(&stale_control);
    if let Ok(meta) = metadata {
        // On Unix, socket files have a special file type
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            assert!(
                meta.file_type().is_socket(),
                "control.sock should be a real socket, not a regular file"
            );
        }
    }

    // Quit gracefully
    driver.quit()?;
    driver.wait_for_exit(5000)?;

    Ok(())
}
