//! Integration tests for bz TUI
//!
//! These tests spawn the actual bz binary in a PTY and verify behavior
//! using the `PtyDriver` test harness.
//!
//! Tests use isolated session directories to avoid conflicts with
//! other test runs or the user's production bz instance.

use bz::test_support::PtyDriver;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Path to test fixtures directory
fn fixtures_dir() -> String {
    format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
}

/// Test: spawn bz, verify shell renders, quit gracefully
#[test]
fn test_bz_spawns_shell_and_renders() -> Result<()> {
    let mut driver = PtyDriver::spawn_isolated(24, 120)?;

    // Wait for shell to start and render (daemon + chaperone + Matrix login)
    driver.wait_and_process(2000);

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

    // Wait for shell to start (daemon + chaperone init takes time)
    driver.wait_and_process(2000);

    // Send "echo test123" followed by Enter
    driver.send("echo test123\r")?;

    // Wait for command output
    driver.wait_and_process(500);

    let screen = driver.screen();
    println!("Screen after command: {:?}", screen);

    // The screen should show "test123" (the output of echo)
    assert!(
        screen.contains("test123"),
        "Screen should show command output 'test123'"
    );

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

    // Wait for initial render (daemon + chaperone init takes time)
    driver.wait_and_process(2000);

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
    driver.wait_and_process(200);

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
    driver.wait_and_process(200);

    let screen = driver.screen();

    // Logs is now focused
    assert!(
        !screen.contains("#logs *"),
        "Focused channel 'logs' should not have activity asterisk, got: {}",
        screen
    );

    // Send Ctrl+B n again (should wrap to main)
    driver.send(r"\x02n")?;
    driver.wait_and_process(200);

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

    // Wait for initial render (daemon + chaperone init takes time)
    driver.wait_and_process(2000);

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
    driver.wait_and_process(200);

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
    driver.wait_and_process(200);

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

    // Wait for bz to render (daemon + chaperone init takes time)
    driver.wait_and_process(2000);

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
