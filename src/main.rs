mod channel;
mod config;
mod pty;
mod sidebar;

use std::io::{self, stdout};
use std::time::Duration;

use color_eyre::eyre::Result;
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::Paragraph,
    Terminal,
};
use tui_term::widget::PseudoTerminal;

use crate::config::Config;
use crate::sidebar::{Sidebar, SIDEBAR_WIDTH};

/// Application state
struct App {
    /// All channels
    channels: Vec<Channel>,
    /// Index of the currently focused channel
    focused: usize,
}

impl App {
    /// Create app from configuration
    ///
    /// `rows` and `cols` are the full terminal dimensions. PTY width is
    /// reduced by SIDEBAR_WIDTH.
    fn from_config(config: &Config, rows: u16, cols: u16) -> Result<Self> {
        use crate::pty::Pty;

        let pty_cols = cols.saturating_sub(SIDEBAR_WIDTH);
        let mut channels = Vec::with_capacity(config.channel.len());
        for (id, ch_config) in config.channel.iter().enumerate() {
            let pty = Pty::spawn(
                id,
                rows,
                pty_cols,
                &ch_config.command,
                ch_config.cwd.as_deref(),
            )?;
            channels.push(Channel::new(id, ch_config.name.clone(), pty));
        }
        Ok(Self {
            channels,
            focused: 0,
        })
    }

    /// Get mutable reference to the focused channel
    fn focused_channel(&mut self) -> &mut Channel {
        &mut self.channels[self.focused]
    }

    /// Process output from all channels
    fn process_all_output(&mut self) {
        let focused = self.focused;
        for (i, channel) in self.channels.iter_mut().enumerate() {
            channel.process_output(i == focused);
        }
    }

    /// Resize all PTYs
    ///
    /// `cols` should be the full terminal width - sidebar width is subtracted internally.
    fn resize_all(&mut self, rows: u16, cols: u16) -> Result<()> {
        let pty_cols = cols.saturating_sub(SIDEBAR_WIDTH);
        for channel in &mut self.channels {
            channel.pty.resize(rows, pty_cols)?;
        }
        Ok(())
    }

    /// Switch to next channel
    fn next_channel(&mut self) {
        self.focused = (self.focused + 1) % self.channels.len();
        self.channels[self.focused].clear_activity();
    }

    /// Switch to previous channel
    fn prev_channel(&mut self) {
        if self.focused == 0 {
            self.focused = self.channels.len() - 1;
        } else {
            self.focused -= 1;
        }
        self.channels[self.focused].clear_activity();
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

    // Load configuration
    let config = Config::load()?;

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Get terminal size (minus 1 row for status line)
    let size = terminal.size()?;
    let pty_height = size.height.saturating_sub(1);

    // Create app from config
    let mut app = App::from_config(&config, pty_height, size.width)?;

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
        // Process output from all channels
        app.process_all_output();

        tokio::select! {
            // Handle terminal events
            Some(event_result) = event_stream.next() => {
                match event_result? {
                    Event::Key(key) => {
                        // Handle Ctrl+ keybinds
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            match key.code {
                                KeyCode::Char('q') => break, // Quit
                                KeyCode::Char('n') => app.next_channel(), // Next channel
                                KeyCode::Char('p') => app.prev_channel(), // Previous channel
                                _ => {
                                    // Forward other Ctrl+ keys to PTY
                                    if let Some(bytes) = key_to_bytes(key) {
                                        app.focused_channel().pty.write(&bytes)?;
                                    }
                                }
                            }
                        } else {
                            // Forward regular keys to focused PTY
                            if let Some(bytes) = key_to_bytes(key) {
                                app.focused_channel().pty.write(&bytes)?;
                            }
                        }
                    }
                    Event::Resize(cols, rows) => {
                        // Resize PTYs (minus 1 for status line)
                        let pty_height = rows.saturating_sub(1);
                        app.resize_all(pty_height, cols)?;
                    }
                    _ => {}
                }
            }

            // Render at regular intervals
            _ = render_interval.tick() => {
                terminal.draw(|frame| {
                    // Horizontal split: sidebar | main content
                    let h_chunks = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([
                            Constraint::Length(SIDEBAR_WIDTH),
                            Constraint::Min(0),
                        ])
                        .split(frame.area());

                    // Render sidebar
                    let sidebar = Sidebar::new(&app.channels, app.focused);
                    frame.render_widget(sidebar, h_chunks[0]);

                    // Vertical split for main content: PTY | status line
                    let v_chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Min(0),
                            Constraint::Length(1),
                        ])
                        .split(h_chunks[1]);

                    // PTY in main area
                    let pseudo_term = PseudoTerminal::new(app.focused_channel().pty.screen());
                    frame.render_widget(pseudo_term, v_chunks[0]);

                    // Status line
                    let status = " Ctrl+N/P: switch | Ctrl+Q: quit ";
                    let status_widget = Paragraph::new(status)
                        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
                    frame.render_widget(status_widget, v_chunks[1]);
                })?;
            }
        }
    }

    Ok(())
}
