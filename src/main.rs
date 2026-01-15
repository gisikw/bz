mod pty;

use std::io::{self, stdout};
use std::time::Duration;

use color_eyre::eyre::Result;
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};
use tui_term::widget::PseudoTerminal;

use crate::pty::Pty;

/// Application state
struct App {
    /// All PTY instances
    ptys: Vec<Pty>,
    /// Index of the currently focused PTY
    focused: usize,
}

impl App {
    /// Create a new app with the given number of PTYs
    fn new(count: usize, rows: u16, cols: u16) -> Result<Self> {
        let mut ptys = Vec::with_capacity(count);
        for id in 0..count {
            ptys.push(Pty::spawn(id, rows, cols)?);
        }
        Ok(Self { ptys, focused: 0 })
    }

    /// Get mutable reference to the focused PTY
    fn focused_pty(&mut self) -> &mut Pty {
        &mut self.ptys[self.focused]
    }

    /// Process output from all PTYs
    fn process_all_output(&mut self) {
        for pty in &mut self.ptys {
            pty.process_output();
        }
    }

    /// Resize all PTYs
    fn resize_all(&mut self, rows: u16, cols: u16) -> Result<()> {
        for pty in &mut self.ptys {
            pty.resize(rows, cols)?;
        }
        Ok(())
    }
}

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

    // Get terminal size
    let size = terminal.size()?;

    // Create app with 3 PTYs
    let mut app = App::new(3, size.height, size.width)?;

    // Run the app
    let result = run(&mut terminal, &mut app).await;

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
    app: &mut App,
) -> Result<()> {
    let mut event_stream = EventStream::new();
    let mut render_interval = tokio::time::interval(Duration::from_millis(16)); // ~60fps

    loop {
        // Process output from all PTYs (not just focused)
        app.process_all_output();

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

                        // Forward all other keys to focused PTY
                        if let Some(bytes) = key_to_bytes(key) {
                            app.focused_pty().write(&bytes)?;
                        }
                    }
                    Event::Resize(cols, rows) => {
                        app.resize_all(rows, cols)?;
                    }
                    _ => {}
                }
            }

            // Render at regular intervals
            _ = render_interval.tick() => {
                terminal.draw(|frame| {
                    let pseudo_term = PseudoTerminal::new(app.focused_pty().screen());
                    frame.render_widget(pseudo_term, frame.area());
                })?;
            }
        }
    }

    Ok(())
}
