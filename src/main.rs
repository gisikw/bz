use std::io::{self, stdout, Read, Write};
use std::time::Duration;

use color_eyre::eyre::Result;
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;
use tui_term::widget::PseudoTerminal;

#[tokio::main]
async fn main() -> Result<()> {
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

    // Get reader and writer for PTY
    let mut pty_reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to clone PTY reader: {}", e))?;

    let mut pty_writer = pair
        .master
        .take_writer()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to get PTY writer: {}", e))?;

    // Async channel to receive PTY output
    let (tx, rx) = mpsc::channel::<Vec<u8>>(256);

    // Spawn blocking task to read from PTY
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break; // Receiver dropped
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Create vt100 parser
    let mut parser = vt100::Parser::new(size.height, size.width, 0);

    // Run the app (pass master for resize handling)
    let result = run(&mut terminal, &mut parser, rx, &mut pty_writer, &pair.master).await;

    // Restore terminal (always, even on error)
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    result
}

/// Convert a crossterm key event to bytes for the PTY
fn key_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    let bytes = match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+letter = ASCII 1-26
                let ctrl = (c.to_ascii_lowercase() as u8).wrapping_sub(b'a').wrapping_add(1);
                vec![ctrl]
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                s.as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![127],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![27],
        KeyCode::Up => vec![27, b'[', b'A'],
        KeyCode::Down => vec![27, b'[', b'B'],
        KeyCode::Right => vec![27, b'[', b'C'],
        KeyCode::Left => vec![27, b'[', b'D'],
        KeyCode::Home => vec![27, b'[', b'H'],
        KeyCode::End => vec![27, b'[', b'F'],
        KeyCode::Delete => vec![27, b'[', b'3', b'~'],
        KeyCode::PageUp => vec![27, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![27, b'[', b'6', b'~'],
        KeyCode::Insert => vec![27, b'[', b'2', b'~'],
        _ => return None,
    };
    Some(bytes)
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    parser: &mut vt100::Parser,
    mut pty_rx: mpsc::Receiver<Vec<u8>>,
    pty_writer: &mut Box<dyn Write + Send>,
    pty_master: &Box<dyn portable_pty::MasterPty + Send>,
) -> Result<()> {
    let mut event_stream = EventStream::new();
    let mut render_interval = tokio::time::interval(Duration::from_millis(16)); // ~60fps

    loop {
        tokio::select! {
            // Handle terminal events
            Some(event_result) = event_stream.next() => {
                match event_result? {
                    Event::Key(key) => {
                        // Ctrl+Q to quit bz
                        if key.code == KeyCode::Char('q')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            break;
                        }

                        // Forward all other keys to PTY
                        if let Some(bytes) = key_to_bytes(key) {
                            pty_writer.write_all(&bytes)?;
                            pty_writer.flush()?;
                        }
                    }
                    Event::Resize(cols, rows) => {
                        // Resize PTY (sends SIGWINCH to child process)
                        pty_master
                            .resize(PtySize {
                                rows,
                                cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            })
                            .map_err(|e| color_eyre::eyre::eyre!("Failed to resize PTY: {}", e))?;

                        // Resize vt100 parser
                        parser.set_size(rows, cols);
                    }
                    _ => {}
                }
            }

            // Handle PTY output
            Some(data) = pty_rx.recv() => {
                parser.process(&data);
            }

            // Render at regular intervals
            _ = render_interval.tick() => {
                terminal.draw(|frame| {
                    let pseudo_term = PseudoTerminal::new(parser.screen());
                    frame.render_widget(pseudo_term, frame.area());
                })?;
            }
        }
    }

    Ok(())
}
