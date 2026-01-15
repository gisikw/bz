use std::io::{self, stdout, Read};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use color_eyre::eyre::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::{backend::CrosstermBackend, Terminal};
use tui_term::widget::PseudoTerminal;

fn main() -> Result<()> {
    color_eyre::install()?;

    // Set up panic hook to restore terminal on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(panic);
    }));

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Get terminal size for PTY
    let size = terminal.size()?;

    // Spawn PTY with shell
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: size.height,
            cols: size.width,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| color_eyre::eyre::eyre!("Failed to open PTY: {}", e))?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".into());
    let cmd = CommandBuilder::new(shell);
    let _child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| color_eyre::eyre::eyre!("Failed to spawn shell: {}", e))?;

    // Get reader for PTY output
    let mut pty_reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to clone PTY reader: {}", e))?;

    // Channel to receive PTY output in main thread
    let (tx, rx) = mpsc::channel::<Vec<u8>>();

    // Spawn thread to read from PTY
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break; // Receiver dropped
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Create vt100 parser
    let mut parser = vt100::Parser::new(size.height, size.width, 0);

    // Run the app
    let result = run(&mut terminal, &mut parser, &rx);

    // Restore terminal (always, even on error)
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    parser: &mut vt100::Parser,
    pty_rx: &mpsc::Receiver<Vec<u8>>,
) -> Result<()> {
    loop {
        // Process any pending PTY output
        while let Ok(data) = pty_rx.try_recv() {
            parser.process(&data);
        }

        // Render the terminal
        terminal.draw(|frame| {
            let pseudo_term = PseudoTerminal::new(parser.screen());
            frame.render_widget(pseudo_term, frame.area());
        })?;

        // Handle input with short timeout for responsiveness
        if event::poll(Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                // Ctrl+Q to quit (since regular 'q' goes to shell)
                if key.code == KeyCode::Char('q')
                    && key.modifiers.contains(event::KeyModifiers::CONTROL)
                {
                    break;
                }
            }
        }
    }

    Ok(())
}
