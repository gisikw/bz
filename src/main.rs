mod channel;
mod config;
mod daemon;
mod picker;
mod protocol;
mod pty;
mod session;
mod session_pty;
mod sidebar;
mod terminal;

use std::env;
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
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::picker::{HasNameActivity, HasPtyStatus, Picker, PickerWidget};
use crate::protocol::{ClientMessage, DaemonMessage};
use crate::pty::{ActivityState, PtyStatus};
use crate::session::{ConnectOptions, Session};
use crate::session_pty::SessionPty;
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

/// A session-backed channel
struct SessionChannel {
    /// Display name
    pub name: String,
    /// The session-backed PTY
    pub pty: SessionPty,
    /// Command used to spawn the PTY (for display)
    pub command: String,
    /// Working directory (for display)
    pub cwd: Option<String>,
}

impl SessionChannel {
    fn new(name: String, pty: SessionPty, command: String, cwd: Option<String>) -> Self {
        Self {
            name,
            pty,
            command,
            cwd,
        }
    }

    fn clear_activity(&mut self) {
        self.pty.clear_activity();
    }
}

impl HasNameActivity for SessionChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn activity(&self) -> &ActivityState {
        &self.pty.activity
    }
}

impl HasPtyStatus for SessionChannel {
    fn pty_status(&self) -> &PtyStatus {
        &self.pty.status
    }
}

/// Application state
struct App {
    /// All channels
    channels: Vec<SessionChannel>,
    /// Index of the currently focused channel
    focused: usize,
    /// Current input mode
    input_mode: InputMode,
    /// Channel picker (None = closed)
    picker: Option<Picker>,
    /// Whether to show the sidebar
    show_sidebar: bool,
    /// Channel to send messages to session
    session_tx: mpsc::Sender<ClientMessage>,
    /// Whether quit confirmation is showing
    quit_confirm: bool,
}

impl App {
    /// Create app from session
    fn from_session(
        session: &Session,
        rows: u16,
        cols: u16,
        session_tx: mpsc::Sender<ClientMessage>,
    ) -> Self {
        let show_sidebar = cols >= MOBILE_WIDTH_THRESHOLD;
        let pty_cols = if show_sidebar {
            cols.saturating_sub(SIDEBAR_WIDTH)
        } else {
            cols
        };

        let mut channels = Vec::with_capacity(session.ptys().len());
        for pty_info in session.ptys() {
            let session_pty = SessionPty::new(pty_info.id, rows, pty_cols, session_tx.clone());
            channels.push(SessionChannel::new(
                pty_info.name.clone(),
                session_pty,
                pty_info.command.clone(),
                pty_info.cwd.clone(),
            ));
        }

        // Restore focused channel from session
        let focused = session.info().focused.min(channels.len().saturating_sub(1));

        Self {
            channels,
            focused,
            input_mode: InputMode::default(),
            picker: None,
            show_sidebar,
            session_tx,
            quit_confirm: false,
        }
    }

    /// Get mutable reference to the focused channel
    fn focused_channel(&mut self) -> &mut SessionChannel {
        &mut self.channels[self.focused]
    }

    /// Check pending activities
    fn check_pending_activities(&mut self) {
        for channel in &mut self.channels {
            channel.pty.check_pending_activity();
        }
    }

    /// Resize all PTYs
    fn resize_all(&mut self, rows: u16, cols: u16) {
        let pty_cols = if self.show_sidebar {
            cols.saturating_sub(SIDEBAR_WIDTH)
        } else {
            cols
        };
        for channel in &mut self.channels {
            channel.pty.resize(rows, pty_cols);
        }
    }

    /// Switch to next channel
    fn next_channel(&mut self) {
        self.focused = (self.focused + 1) % self.channels.len();
        self.channels[self.focused].clear_activity();
        self.send_focus_update();
    }

    /// Switch to previous channel
    fn prev_channel(&mut self) {
        if self.focused == 0 {
            self.focused = self.channels.len() - 1;
        } else {
            self.focused -= 1;
        }
        self.channels[self.focused].clear_activity();
        self.send_focus_update();
    }

    /// Switch to specific channel by index
    fn switch_to_channel(&mut self, idx: usize) {
        if idx < self.channels.len() {
            self.focused = idx;
            self.channels[self.focused].clear_activity();
            self.send_focus_update();
        }
    }

    /// Send focus update to daemon
    fn send_focus_update(&self) {
        let msg = ClientMessage::SetFocus { channel_idx: self.focused };
        let _ = self.session_tx.try_send(msg);
    }

    /// Get channel by PTY ID
    fn get_channel_by_pty_id(&mut self, pty_id: usize) -> Option<&mut SessionChannel> {
        self.channels.iter_mut().find(|c| c.pty.id == pty_id)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    // Parse CLI args
    let args: Vec<String> = env::args().collect();

    // Handle subcommands
    if args.len() > 1 {
        match args[1].as_str() {
            "stop" => {
                session::stop()?;
                println!("Session stopped.");
                return Ok(());
            }
            "sessions" => {
                if let Some(socket) = daemon::find_session() {
                    println!("Active session: {}", socket.display());
                } else {
                    println!("No active sessions.");
                }
                return Ok(());
            }
            "--help" | "-h" => {
                println!("bz - Multi-agent coordination TUI");
                println!();
                println!("Usage: bz [OPTIONS] [COMMAND]");
                println!();
                println!("Commands:");
                println!("  stop      Kill the session daemon");
                println!("  sessions  List active sessions");
                println!();
                println!("Options:");
                println!("  --takeover  Take over from existing client");
                println!("  --help      Show this help");
                return Ok(());
            }
            _ => {}
        }
    }

    let takeover = args.iter().any(|a| a == "--takeover");

    // Set up panic hook to restore terminal on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        original_hook(panic);
    }));

    // Load configuration
    let config = Config::load()?;

    // Connect to or spawn session
    let opts = ConnectOptions { takeover };
    let mut session = Session::connect_or_spawn(&config, opts).await?;

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Get terminal size
    let size = terminal.size()?;
    let pty_height = size.height.saturating_sub(1);

    // Get session's write channel for PTY communication
    let session_tx = session.write_tx();

    // Create app from session
    let mut app = App::from_session(&session, pty_height, size.width, session_tx);

    // Run the app
    let result = run(&mut terminal, &mut app, &mut session).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;

    result
}

/// Convert a crossterm key event to bytes for the PTY
fn key_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    let bytes = match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let ctrl = (c.to_ascii_lowercase() as u8)
                    .wrapping_sub(b'a')
                    .wrapping_add(1);
                vec![ctrl]
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                s.as_bytes().to_vec()
            }
        }
        KeyCode::Enter => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                vec![27, b'[', b'1', b'3', b';', b'2', b'u']
            } else {
                vec![b'\r']
            }
        }
        KeyCode::Backspace => vec![127],
        KeyCode::Tab => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
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

/// Render quit confirmation modal
fn render_quit_confirm(frame: &mut ratatui::Frame) {
    let area = frame.area();

    // Center the modal
    let modal_width = 50;
    let modal_height = 7;
    let x = (area.width.saturating_sub(modal_width)) / 2;
    let y = (area.height.saturating_sub(modal_height)) / 2;
    let modal_area = Rect::new(x, y, modal_width, modal_height);

    // Clear the modal area
    frame.render_widget(Clear, modal_area);

    // Render modal
    let block = Block::default()
        .title(" Quit Session ")
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Rgb(50, 50, 60)));

    let text = Paragraph::new(
        "This will kill all PTYs in this session.\n\n\
         Press 'y' to confirm, any other key to cancel.",
    )
    .alignment(Alignment::Center)
    .block(block);

    frame.render_widget(text, modal_area);
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    session: &mut Session,
) -> Result<()> {
    let mut event_stream = EventStream::new();
    let mut render_interval = tokio::time::interval(Duration::from_millis(16));

    loop {
        // Process any pending session messages
        while let Some(msg) = session.try_recv() {
            match msg {
                DaemonMessage::History { pty_id, data } => {
                    if let Some(channel) = app.get_channel_by_pty_id(pty_id) {
                        channel.pty.process_history(&data);
                    }
                }
                DaemonMessage::HistoryEnd { pty_id } => {
                    if let Some(channel) = app.get_channel_by_pty_id(pty_id) {
                        channel.pty.mark_history_complete();
                    }
                }
                DaemonMessage::Output { pty_id, data } => {
                    let is_focused = app
                        .channels
                        .get(app.focused)
                        .map(|c| c.pty.id == pty_id)
                        .unwrap_or(false);
                    if let Some(channel) = app.get_channel_by_pty_id(pty_id) {
                        channel.pty.process_daemon_output(&data, is_focused);
                    }
                }
                DaemonMessage::PtyExited { pty_id, .. } => {
                    if let Some(channel) = app.get_channel_by_pty_id(pty_id) {
                        channel.pty.mark_exited();
                    }
                }
                DaemonMessage::Kicked => {
                    // Another client took over
                    return Ok(());
                }
                DaemonMessage::Shutdown => {
                    return Ok(());
                }
                _ => {}
            }
        }

        tokio::select! {
            // Handle terminal events
            Some(event_result) = event_stream.next() => {
                match event_result? {
                    Event::Key(key) => {
                        // Handle quit confirmation modal
                        if app.quit_confirm {
                            if key.code == KeyCode::Char('y') || key.code == KeyCode::Char('Y') {
                                // Confirmed quit
                                session.quit().await?;
                                break;
                            } else {
                                // Cancel
                                app.quit_confirm = false;
                            }
                            continue;
                        }

                        match app.input_mode {
                            InputMode::Normal => {
                                // Check if picker is open
                                if let Some(ref mut picker) = app.picker {
                                    match key.code {
                                        KeyCode::Esc => {
                                            app.picker = None;
                                        }
                                        KeyCode::Enter => {
                                            if let Some(idx) = picker.selected_channel_from_channels(&app.channels) {
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
                                            picker.pop_char_from_channels(&app.channels);
                                        }
                                        KeyCode::Char(c) => {
                                            picker.push_char_from_channels(c, &app.channels);
                                        }
                                        _ => {}
                                    }
                                } else if app.focused_channel().pty.is_scrolled() {
                                    // In scroll mode
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
                                            let max = pty.scrollback_len();
                                            pty.scroll_up(max);
                                        }
                                        KeyCode::Char('G') => {
                                            pty.scroll_to_bottom();
                                        }
                                        _ => {}
                                    }
                                } else if key.code == KeyCode::PageUp {
                                    let page = app.focused_channel().pty.screen().size().0 as usize;
                                    app.focused_channel().pty.scroll_up(page.saturating_sub(2));
                                } else if key.code == KeyCode::Char('b')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    app.input_mode = InputMode::Leader;
                                } else if key.code == KeyCode::Char('k')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    let mut picker = Picker::new();
                                    picker.update_filter_from_channels(&app.channels);
                                    app.picker = Some(picker);
                                } else if key.code == KeyCode::Char('\\')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    app.show_sidebar = !app.show_sidebar;
                                    let (cols, rows) = crossterm::terminal::size()?;
                                    let pty_height = rows.saturating_sub(1);
                                    app.resize_all(pty_height, cols);
                                } else {
                                    if let Some(bytes) = key_to_bytes(key) {
                                        app.focused_channel().pty.write(&bytes);
                                    }
                                }
                            }
                            InputMode::Leader => {
                                app.input_mode = InputMode::Normal;

                                match key.code {
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
                                    KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                                        let idx = (c as usize) - ('1' as usize);
                                        app.switch_to_channel(idx);
                                    }
                                    KeyCode::Char('/') => {
                                        let mut picker = Picker::new();
                                        picker.update_filter_from_channels(&app.channels);
                                        app.picker = Some(picker);
                                    }
                                    KeyCode::Char('[') => {
                                        let pty = &mut app.focused_channel().pty;
                                        let scrollback = pty.scrollback_len();
                                        if scrollback > 0 {
                                            pty.scroll_up(1);
                                        }
                                    }
                                    KeyCode::Char('q') => {
                                        // Detach (daemon stays alive)
                                        session.detach().await?;
                                        break;
                                    }
                                    KeyCode::Char('Q') => {
                                        // Show quit confirmation
                                        app.quit_confirm = true;
                                        app.input_mode = InputMode::Normal;
                                    }
                                    KeyCode::Char('b') => {
                                        let bytes = vec![2];
                                        app.focused_channel().pty.write(&bytes);
                                    }
                                    KeyCode::Esc | _ => {}
                                }
                            }
                        }
                    }
                    Event::Resize(cols, rows) => {
                        app.show_sidebar = cols >= MOBILE_WIDTH_THRESHOLD;
                        let pty_height = rows.saturating_sub(1);
                        app.resize_all(pty_height, cols);
                    }
                    Event::Mouse(mouse) => {
                        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                            if app.show_sidebar && mouse.column < SIDEBAR_WIDTH {
                                let channel_idx = mouse.row.saturating_sub(1) as usize;
                                if channel_idx < app.channels.len() {
                                    app.switch_to_channel(channel_idx);
                                }
                            }
                        }
                    }
                    Event::Paste(text) => {
                        app.focused_channel().pty.write(text.as_bytes());
                    }
                    _ => {}
                }
            }

            // Render at regular intervals
            _ = render_interval.tick() => {
                app.check_pending_activities();

                terminal.draw(|frame| {
                    let main_area = if app.show_sidebar {
                        let h_chunks = Layout::default()
                            .direction(Direction::Horizontal)
                            .constraints([
                                Constraint::Length(SIDEBAR_WIDTH),
                                Constraint::Min(0),
                            ])
                            .split(frame.area());

                        let sidebar = Sidebar::from_session_channels(&app.channels, app.focused);
                        frame.render_widget(sidebar, h_chunks[0]);

                        h_chunks[1]
                    } else {
                        frame.area()
                    };

                    let v_chunks = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Min(0),
                            Constraint::Length(1),
                        ])
                        .split(main_area);

                    // PTY content
                    let pty_status = app.focused_channel().pty.status.clone();
                    if pty_status == PtyStatus::Exited {
                        app.focused_channel().pty.apply_scroll_for_render();
                        let term_widget = TerminalWidget::new(app.focused_channel().pty.screen());
                        frame.render_widget(term_widget, v_chunks[0]);
                        app.focused_channel().pty.reset_scroll_view();

                        let exit_msg = Paragraph::new("Process exited")
                            .style(Style::default().fg(Color::Yellow))
                            .alignment(Alignment::Center);
                        frame.render_widget(exit_msg, v_chunks[0]);
                    } else {
                        app.focused_channel().pty.apply_scroll_for_render();
                        let term_widget = TerminalWidget::new(app.focused_channel().pty.screen());
                        frame.render_widget(term_widget, v_chunks[0]);
                        app.focused_channel().pty.reset_scroll_view();
                    }

                    // Status line
                    let is_scrolled = app.focused_channel().pty.is_scrolled();
                    let scroll_offset = app.focused_channel().pty.scroll_offset;
                    let scrollback_len = app.focused_channel().pty.scrollback_len();

                    let (status, status_style) = if pty_status == PtyStatus::Exited {
                        (
                            " EXITED │ ^K switch channel ".to_string(),
                            Style::default()
                                .bg(Color::Rgb(100, 50, 50))
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else if is_scrolled {
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
                                    format!(" ^K search │ ^B leader{} │ SESSION ", scroll_hint),
                                    Style::default()
                                        .bg(Color::Rgb(30, 30, 40))
                                        .fg(Color::DarkGray),
                                )
                            }
                            InputMode::Leader => (
                                " LEADER │ j/k nav │ 1-9 jump │ / search │ q detach │ Q quit │ b send ^B ".to_string(),
                                Style::default()
                                    .bg(Color::Rgb(180, 140, 40))
                                    .fg(Color::Black)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        }
                    };
                    let status_widget = Paragraph::new(status).style(status_style);
                    frame.render_widget(status_widget, v_chunks[1]);

                    // Picker overlay
                    if let Some(ref picker) = app.picker {
                        let picker_widget = PickerWidget::from_session_channels(picker, &app.channels);
                        frame.render_widget(picker_widget, frame.area());
                    }

                    // Quit confirmation overlay
                    if app.quit_confirm {
                        render_quit_confirm(frame);
                    }
                })?;
            }
        }
    }

    Ok(())
}
