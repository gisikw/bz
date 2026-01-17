mod channel;
mod config;
mod picker;
mod pty;
mod sidebar;
mod terminal;

use std::io::{self, stdout};
use std::time::Duration;

use color_eyre::eyre::Result;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::Paragraph,
    Terminal,
};
use crate::channel::Channel;
use crate::config::Config;
use crate::picker::{Picker, PickerWidget};
use crate::pty::PtyStatus;
use crate::sidebar::{Sidebar, SIDEBAR_WIDTH};
use crate::terminal::TerminalWidget;

/// Width threshold below which sidebar auto-hides (mobile breakpoint)
const MOBILE_WIDTH_THRESHOLD: u16 = 100;

/// Input mode for key handling
#[derive(Debug, Clone, PartialEq, Default)]
pub enum InputMode {
    /// Normal mode - keys go to PTY
    #[default]
    Normal,
    /// Leader mode - waiting for command after Ctrl+B
    Leader,
}

/// Application state
struct App {
    /// All channels
    channels: Vec<Channel>,
    /// Index of the currently focused channel
    focused: usize,
    /// Current input mode
    input_mode: InputMode,
    /// Channel picker (None = closed)
    picker: Option<Picker>,
    /// Whether to show the sidebar
    show_sidebar: bool,
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
            channels.push(Channel::new(
                id,
                ch_config.name.clone(),
                pty,
                ch_config.command.clone(),
                ch_config.cwd.clone(),
            ));
        }
        Ok(Self {
            channels,
            focused: 0,
            input_mode: InputMode::default(),
            picker: None,
            show_sidebar: true,
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

    /// Check pending activities and promote to Active if settled
    fn check_pending_activities(&mut self) {
        for channel in &mut self.channels {
            channel.pty.check_pending_activity();
        }
    }

    /// Resize all PTYs
    ///
    /// `cols` should be the full terminal width - sidebar width is subtracted internally.
    fn resize_all(&mut self, rows: u16, cols: u16) -> Result<()> {
        let pty_cols = if self.show_sidebar {
            cols.saturating_sub(SIDEBAR_WIDTH)
        } else {
            cols
        };
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

    /// Switch to specific channel by index
    fn switch_to_channel(&mut self, idx: usize) {
        if idx < self.channels.len() {
            self.focused = idx;
            self.channels[self.focused].clear_activity();
        }
    }

    /// Restart the PTY for the focused channel
    fn restart_focused_pty(&mut self, rows: u16, cols: u16) -> Result<()> {
        use crate::pty::Pty;

        let channel = &mut self.channels[self.focused];
        let pty_cols = if self.show_sidebar {
            cols.saturating_sub(SIDEBAR_WIDTH)
        } else {
            cols
        };

        let new_pty = Pty::spawn(
            channel.id,
            rows,
            pty_cols,
            &channel.command,
            channel.cwd.as_deref(),
        )?;

        channel.pty = new_pty;
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
        let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen);
        original_hook(panic);
    }));

    // Load configuration
    let config = Config::load()?;

    // Set up terminal with mouse and bracketed paste support
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
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
    execute!(terminal.backend_mut(), DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen)?;

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
        KeyCode::Enter => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                // Shift+Enter: CSI 13;2u (kitty keyboard protocol)
                vec![27, b'[', b'1', b'3', b';', b'2', b'u']
            } else {
                vec![b'\r']
            }
        }
        KeyCode::Backspace => vec![127],
        KeyCode::Tab => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                // Shift+Tab: CSI Z (back-tab)
                vec![27, b'[', b'Z']
            } else {
                vec![b'\t']
            }
        }
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
                        match app.input_mode {
                            InputMode::Normal => {
                                // Check if picker is open first
                                if let Some(ref mut picker) = app.picker {
                                    match key.code {
                                        KeyCode::Esc => {
                                            app.picker = None;
                                        }
                                        KeyCode::Enter => {
                                            if let Some(idx) = picker.selected_channel() {
                                                app.switch_to_channel(idx);
                                            }
                                            app.picker = None;
                                        }
                                        KeyCode::Up => {
                                            picker.move_up();
                                        }
                                        KeyCode::Down => {
                                            picker.move_down();
                                        }
                                        KeyCode::Backspace => {
                                            picker.pop_char(&app.channels);
                                        }
                                        KeyCode::Char(c) => {
                                            picker.push_char(c, &app.channels);
                                        }
                                        _ => {}
                                    }
                                } else if app.focused_channel().pty.is_scrolled() {
                                    // In scroll mode - handle scroll navigation
                                    let pty = &mut app.focused_channel().pty;
                                    match key.code {
                                        KeyCode::Esc | KeyCode::Char('q') => {
                                            pty.scroll_to_bottom();
                                        }
                                        KeyCode::PageUp => {
                                            let page = pty.screen().size().0 as usize;
                                            pty.scroll_up(page.saturating_sub(2));
                                        }
                                        KeyCode::PageDown => {
                                            let page = pty.screen().size().0 as usize;
                                            pty.scroll_down(page.saturating_sub(2));
                                        }
                                        KeyCode::Up | KeyCode::Char('k') => {
                                            pty.scroll_up(1);
                                        }
                                        KeyCode::Down | KeyCode::Char('j') => {
                                            pty.scroll_down(1);
                                        }
                                        KeyCode::Char('g') => {
                                            // Go to top of scrollback
                                            let max = pty.scrollback_len();
                                            pty.scroll_up(max);
                                        }
                                        KeyCode::Char('G') => {
                                            // Go to bottom
                                            pty.scroll_to_bottom();
                                        }
                                        _ => {}
                                    }
                                } else if key.code == KeyCode::PageUp {
                                    // PageUp enters scroll mode
                                    let page = app.focused_channel().pty.screen().size().0 as usize;
                                    app.focused_channel().pty.scroll_up(page.saturating_sub(2));
                                } else if key.code == KeyCode::Char('b')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    // Ctrl+B enters leader mode
                                    app.input_mode = InputMode::Leader;
                                } else if key.code == KeyCode::Char('k')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    // Ctrl+K opens channel picker directly
                                    let mut picker = Picker::new();
                                    picker.update_filter(&app.channels);
                                    app.picker = Some(picker);
                                } else if key.code == KeyCode::Char('\\')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    // Ctrl+\ toggles sidebar
                                    app.show_sidebar = !app.show_sidebar;
                                    // Resize PTYs to use new available width
                                    let (cols, rows) = crossterm::terminal::size()?;
                                    let pty_height = rows.saturating_sub(1);
                                    app.resize_all(pty_height, cols)?;
                                } else {
                                    // All other keys go to PTY
                                    if let Some(bytes) = key_to_bytes(key) {
                                        app.focused_channel().pty.write(&bytes)?;
                                    }
                                }
                            }
                            InputMode::Leader => {
                                // Always return to normal after processing
                                app.input_mode = InputMode::Normal;

                                match key.code {
                                    // Navigation
                                    KeyCode::Char('j') | KeyCode::Down => {
                                        app.next_channel();
                                    }
                                    KeyCode::Char('k') | KeyCode::Up => {
                                        app.prev_channel();
                                    }
                                    KeyCode::Char('n') => {
                                        app.next_channel();
                                    }
                                    KeyCode::Char('p') => {
                                        app.prev_channel();
                                    }

                                    // Direct channel access (1-9)
                                    KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                                        let idx = (c as usize) - ('1' as usize);
                                        app.switch_to_channel(idx);
                                    }

                                    // Channel picker (search)
                                    KeyCode::Char('/') => {
                                        let mut picker = Picker::new();
                                        picker.update_filter(&app.channels);
                                        app.picker = Some(picker);
                                    }

                                    // Scroll mode
                                    KeyCode::Char('[') => {
                                        // Enter scroll mode (like tmux copy-mode)
                                        let pty = &mut app.focused_channel().pty;
                                        let scrollback = pty.scrollback_len();
                                        if scrollback > 0 {
                                            pty.scroll_up(1);
                                        }
                                    }

                                    // Restart PTY
                                    KeyCode::Char('r') => {
                                        let (cols, rows) = crossterm::terminal::size()?;
                                        let pty_height = rows.saturating_sub(1);
                                        app.restart_focused_pty(pty_height, cols)?;
                                    }

                                    // Quit
                                    KeyCode::Char('q') => {
                                        break;
                                    }

                                    // Send Ctrl+B to PTY (double-tap)
                                    KeyCode::Char('b') => {
                                        let bytes = vec![2]; // Ctrl+B = ASCII 2
                                        app.focused_channel().pty.write(&bytes)?;
                                    }

                                    // Cancel / unknown - just ignore
                                    KeyCode::Esc | _ => {}
                                }
                            }
                        }
                    }
                    Event::Resize(cols, rows) => {
                        // Auto-toggle sidebar based on width threshold
                        app.show_sidebar = cols >= MOBILE_WIDTH_THRESHOLD;

                        // Resize PTYs (minus 1 for status line)
                        let pty_height = rows.saturating_sub(1);
                        app.resize_all(pty_height, cols)?;
                    }
                    Event::Mouse(mouse) => {
                        // Handle mouse clicks in sidebar
                        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                            // Check if click is in sidebar area
                            if app.show_sidebar && mouse.column < SIDEBAR_WIDTH {
                                // Row 0 is the title, channels start at row 1
                                let channel_idx = mouse.row.saturating_sub(1) as usize;
                                if channel_idx < app.channels.len() {
                                    app.switch_to_channel(channel_idx);
                                }
                            }
                        }
                    }
                    Event::Paste(text) => {
                        // Write entire paste content at once (much faster than char-by-char)
                        app.focused_channel().pty.write(text.as_bytes())?;
                    }
                    _ => {}
                }
            }

            // Render at regular intervals
            _ = render_interval.tick() => {
                // Check if any pending activities have settled
                app.check_pending_activities();

                terminal.draw(|frame| {
                    // Determine main content area based on sidebar visibility
                    let main_area = if app.show_sidebar {
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

                        h_chunks[1]
                    } else {
                        frame.area()
                    };

                    // Vertical split for main content: PTY | status line
                    let v_chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Min(0),
                            Constraint::Length(1),
                        ])
                        .split(main_area);

                    // PTY in main area (with scrollback support)
                    // Check if PTY has exited
                    let pty_status = app.focused_channel().pty.status.clone();
                    if pty_status == PtyStatus::Exited {
                        // Show exit message over the terminal content
                        app.focused_channel().pty.apply_scroll_for_render();
                        let term_widget = TerminalWidget::new(app.focused_channel().pty.screen());
                        frame.render_widget(term_widget, v_chunks[0]);
                        app.focused_channel().pty.reset_scroll_view();

                        // Overlay exit message
                        let exit_msg = Paragraph::new("Process exited\n\nPress Ctrl+B r to restart")
                            .style(Style::default().fg(Color::Yellow))
                            .alignment(Alignment::Center);
                        frame.render_widget(exit_msg, v_chunks[0]);
                    } else {
                        app.focused_channel().pty.apply_scroll_for_render();
                        let term_widget = TerminalWidget::new(app.focused_channel().pty.screen());
                        frame.render_widget(term_widget, v_chunks[0]);
                        app.focused_channel().pty.reset_scroll_view();
                    }

                    // Status line with mode indicator
                    // Extract pty state before borrowing app again
                    let is_scrolled = app.focused_channel().pty.is_scrolled();
                    let scroll_offset = app.focused_channel().pty.scroll_offset;
                    let scrollback_len = app.focused_channel().pty.scrollback_len();

                    let (status, status_style) = if pty_status == PtyStatus::Exited {
                        // Exited status
                        (
                            " EXITED │ ^B r restart │ ^K switch channel ".to_string(),
                            Style::default()
                                .bg(Color::Rgb(100, 50, 50))
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else if is_scrolled {
                        // Scroll mode status
                        (
                            format!(" SCROLL [{}/{}] │ j/k line │ PgUp/PgDn page │ g/G top/bottom │ Esc/q exit ", scroll_offset, scrollback_len),
                            Style::default()
                                .bg(Color::Rgb(60, 60, 100))
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        match app.input_mode {
                            InputMode::Normal => {
                                let scroll_hint = if scrollback_len > 0 {
                                    format!(" │ ^B[ scroll ({} lines)", scrollback_len)
                                } else {
                                    String::new()
                                };
                                (
                                    format!(" ^K search │ ^B leader{} ", scroll_hint),
                                    Style::default()
                                        .bg(Color::Rgb(30, 30, 40))
                                        .fg(Color::DarkGray),
                                )
                            }
                            InputMode::Leader => (
                                " LEADER │ j/k nav │ 1-9 jump │ / search │ r restart │ q quit │ b send ^B ".to_string(),
                                Style::default()
                                    .bg(Color::Rgb(180, 140, 40))
                                    .fg(Color::Black)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        }
                    };
                    let status_widget = Paragraph::new(status).style(status_style);
                    frame.render_widget(status_widget, v_chunks[1]);

                    // Render picker overlay if open
                    if let Some(ref picker) = app.picker {
                        let picker_widget = PickerWidget::new(picker, &app.channels);
                        frame.render_widget(picker_widget, frame.area());
                    }
                })?;
            }
        }
    }

    Ok(())
}
