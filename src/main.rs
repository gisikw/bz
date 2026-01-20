mod chaperone;
mod chaperone_channel;
mod chaperone_pty;
mod chat_view;
mod config;
mod daemon;
mod env;
mod log;
mod matrix_client;
mod picker;
mod protocol;
mod pty;
mod room_view;
mod sidebar;
mod terminal;
mod user_chaperone;
use std::io::{self, stdout, IsTerminal};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

use log::log;

/// Find the bzd binary (checks target/debug, target/release, then PATH)
fn find_bzd_binary() -> std::path::PathBuf {
    // Check target/debug first (development)
    let debug_path = std::path::PathBuf::from("target/debug/bzd");
    if debug_path.exists() {
        return debug_path;
    }

    // Check target/release
    let release_path = std::path::PathBuf::from("target/release/bzd");
    if release_path.exists() {
        return release_path;
    }

    // Fall back to PATH
    std::path::PathBuf::from("bzd")
}

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

use std::collections::HashMap;

use crate::chaperone_channel::ChaperoneChannel;
use crate::config::Config;
use crate::matrix_client::{AttachedPty, BzMatrixClient};
use crate::picker::{HasPtyStatus, Picker, PickerWidget};
use crate::pty::PtyStatus;
use crate::room_view::RoomView;
use crate::sidebar::SIDEBAR_WIDTH;
use crate::terminal::TerminalWidget;
use crate::user_chaperone::UserChaperone;

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
    /// All rooms (each with chat + PTY screens)
    rooms: Vec<RoomView>,
    /// Index of the currently focused room
    focused_room: usize,
    /// Current input mode
    input_mode: InputMode,
    /// Channel picker (None = closed)
    picker: Option<Picker>,
    /// Whether to show the sidebar
    show_sidebar: bool,
    /// Whether quit confirmation is showing
    quit_confirm: bool,
    /// Matrix client (if connected)
    matrix_client: Option<BzMatrixClient>,
    /// Cached Matrix rooms for sidebar (updated periodically)
    cached_rooms: Vec<(String, String)>,
}

impl App {
    /// Create app with rooms
    fn new(
        rooms: Vec<RoomView>,
        cols: u16,
        matrix_client: Option<BzMatrixClient>,
    ) -> Self {
        let show_sidebar = cols >= MOBILE_WIDTH_THRESHOLD;

        Self {
            rooms,
            focused_room: 0,
            input_mode: InputMode::default(),
            picker: None,
            show_sidebar,
            quit_confirm: false,
            matrix_client,
            cached_rooms: Vec::new(),
        }
    }

    /// Update cached room list from Matrix client
    async fn update_matrix_rooms(&mut self) {
        if let Some(client) = &self.matrix_client {
            self.cached_rooms = client.room_names().await;
        }
    }

    /// Get reference to the focused room
    fn focused_room(&self) -> &RoomView {
        &self.rooms[self.focused_room]
    }

    /// Get mutable reference to the focused room
    fn focused_room_mut(&mut self) -> &mut RoomView {
        &mut self.rooms[self.focused_room]
    }

    /// Process pending PTY output for all rooms
    fn process_pending(&mut self) {
        for (idx, room) in self.rooms.iter_mut().enumerate() {
            let is_focused = idx == self.focused_room;
            room.process_pending(is_focused);
        }
    }

    /// Check pending activities
    fn check_pending_activities(&mut self) {
        for room in &mut self.rooms {
            room.check_pending_activities();
        }
    }

    /// Resize all PTYs in all rooms
    fn resize_all(&mut self, rows: u16, cols: u16) {
        let pty_rows = rows.max(24);
        let pty_cols = if self.show_sidebar {
            cols.saturating_sub(SIDEBAR_WIDTH).max(80)
        } else {
            cols.max(80)
        };
        for room in &mut self.rooms {
            for channel in room.ptys_mut() {
                channel.resize(pty_rows, pty_cols);
            }
        }
    }

    /// Switch to next room (j/k navigation)
    fn next_room(&mut self) {
        self.focused_room = (self.focused_room + 1) % self.rooms.len();
        self.focused_room_mut().clear_current_activity();
    }

    /// Switch to previous room
    fn prev_room(&mut self) {
        if self.focused_room == 0 {
            self.focused_room = self.rooms.len() - 1;
        } else {
            self.focused_room -= 1;
        }
        self.focused_room_mut().clear_current_activity();
    }

    /// Switch to specific room by index
    fn switch_to_room(&mut self, idx: usize) {
        if idx < self.rooms.len() {
            self.focused_room = idx;
            self.focused_room_mut().clear_current_activity();
        }
    }

    /// Navigate to next screen within current room (l key)
    fn next_screen(&mut self) {
        self.focused_room_mut().next_screen();
        self.focused_room_mut().clear_current_activity();
    }

    /// Navigate to previous screen within current room (h key)
    fn prev_screen(&mut self) {
        self.focused_room_mut().prev_screen();
        self.focused_room_mut().clear_current_activity();
    }

    /// Check if current screen is a PTY (for input routing)
    fn on_pty_screen(&self) -> bool {
        self.focused_room().on_pty()
    }

    /// Write to current PTY if on PTY screen
    fn write_to_current_pty(&mut self, data: &[u8]) {
        if let Some(pty) = self.focused_room_mut().current_pty_mut() {
            pty.write(data);
        }
    }

    /// Get current PTY screen reference
    fn current_pty(&self) -> Option<&ChaperoneChannel> {
        self.focused_room().current_pty()
    }

    /// Get current PTY screen mutable reference
    fn current_pty_mut(&mut self) -> Option<&mut ChaperoneChannel> {
        self.focused_room_mut().current_pty_mut()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    // Parse CLI args
    let args: Vec<String> = std::env::args().collect();
    let mut config_path: Option<std::path::PathBuf> = None;

    // Handle args
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                println!("bz - Multi-agent coordination TUI");
                println!();
                println!("Usage: bz [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config <path>  Use specific config file");
                println!("  --help           Show this help");
                return Ok(());
            }
            "--config" => {
                i += 1;
                if i < args.len() {
                    config_path = Some(std::path::PathBuf::from(&args[i]));
                } else {
                    eprintln!("bz: --config requires a path argument");
                    return Ok(());
                }
            }
            arg if arg.starts_with("--config=") => {
                config_path = Some(std::path::PathBuf::from(arg.trim_start_matches("--config=")));
            }
            _ => {}
        }
        i += 1;
    }

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

    // Verify we're connected to a TTY (required for TUI)
    if !stdout().is_terminal() {
        eprintln!("bz: error: not connected to a terminal");
        eprintln!("bz: the TUI requires an interactive terminal to run");
        eprintln!("bz: hint: a headless mode for programmatic control is planned");
        return Ok(());
    }

    // Load configuration
    log("bz startup");
    let config = match &config_path {
        Some(path) => {
            log(&format!("loading config from {:?}", path));
            Config::load_from(path)?
        }
        None => Config::load()?
    };
    log(&format!("config loaded: {} channels", config.channel.len()));

    // Ensure bzd is running (provides Matrix/Conduit sidecar)
    let existing_session = daemon::find_session();
    log(&format!("find_session result: {:?}", existing_session));

    // Verify session is actually alive (not a stale socket)
    let session_alive = existing_session.as_ref().map_or(false, |path| {
        std::os::unix::net::UnixStream::connect(path).is_ok()
    });
    log(&format!("session_alive: {}", session_alive));

    if !session_alive {
        // Clean up stale socket if it exists
        if let Some(ref path) = existing_session {
            log(&format!("removing stale socket: {:?}", path));
            let _ = std::fs::remove_file(path);
            // Also remove stale pid file
            let _ = std::fs::remove_file(path.with_extension("pid"));
        }

        // Ensure Conduit config exists with correct server_name
        log(&format!("ensuring Conduit config with server_name: {}", config.matrix.server_name));
        if let Err(e) = daemon::conduit::ensure_config(&config.matrix.server_name) {
            log(&format!("failed to ensure Conduit config: {}", e));
        }

        log("no existing bzd session, spawning new one");
        // Get terminal size for bzd
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        log(&format!("terminal size: {}x{}", cols, rows));

        // Spawn bzd - it will daemonize and print socket path
        let bzd_path = find_bzd_binary();
        log(&format!("using bzd binary: {:?}", bzd_path));
        let mut bzd_args = vec![
            rows.to_string(),
            cols.to_string(),
            "--server-name".to_string(),
            config.matrix.server_name.clone(),
        ];
        // Pass config path if we're using a custom one
        if let Some(ref path) = config_path {
            bzd_args.push("--config".to_string());
            bzd_args.push(path.display().to_string());
        }
        let output = Command::new(&bzd_path)
            .args(&bzd_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .output();

        match output {
            Ok(out) => {
                let socket_path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                log(&format!("bzd spawned, socket: '{}', exit status: {:?}", socket_path, out.status));
                if !socket_path.is_empty() {
                    eprintln!("bz: started bzd (socket: {})", socket_path);
                }
            }
            Err(e) => {
                log(&format!("bzd spawn failed: {}", e));
                eprintln!("bz: failed to start bzd: {} (continuing without Matrix)", e);
            }
        }

        // Wait for Conduit to become available (up to 5 seconds)
        log(&format!("waiting for Conduit on port {}...", env::conduit_port()));
        let mut conduit_ready = false;
        for i in 0..50 {
            if TcpStream::connect_timeout(&env::conduit_addr(), Duration::from_millis(100)).is_ok()
            {
                log(&format!("Conduit ready after {}ms", i * 100));
                conduit_ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if !conduit_ready {
            log("Conduit not ready after 5s timeout");
        }
    } else {
        log("existing bzd session is alive, skipping spawn");
    }

    // Spawn user chaperone (kept alive for app lifetime, cleaned up on exit)
    log("spawning user chaperone");
    let mut user_chaperone = UserChaperone::spawn()?;
    user_chaperone.wait_ready().await?;
    log("user chaperone ready");

    // Connect to Matrix (Conduit should be running via bzd)
    // TODO: Make homeserver/credentials configurable
    log(&format!("connecting to Matrix at {}", env::conduit_url()));
    let mut channel_room_map: HashMap<String, String> = HashMap::new();
    let mut message_rx: Option<tokio::sync::mpsc::Receiver<crate::matrix_client::MatrixMessage>> = None;
    let matrix_client = match BzMatrixClient::register_or_login(
        &env::conduit_url(),
        "bz-user",
        "bz-password", // TODO: Generate or prompt for password
    )
    .await
    {
        Ok(client) => {
            log(&format!("Matrix login successful: {}", client.user_id()));
            log("starting Matrix sync");
            message_rx = Some(client.start_sync());

            // Wait briefly for initial sync to populate rooms
            log("waiting 500ms for initial sync");
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Create Matrix rooms for channels from config (with timeout)
            log("ensuring rooms for channels");
            let channel_names: Vec<String> = config.channel.iter().map(|c| c.name.clone()).collect();
            match tokio::time::timeout(
                Duration::from_secs(5),
                client.ensure_rooms_for_channels(&channel_names)
            ).await {
                Ok(Ok(mappings)) => {
                    log(&format!("got {} room mappings", mappings.len()));
                    for (name, room_id) in &mappings {
                        channel_room_map.insert(name.clone(), room_id.clone());
                    }

                    // Invite agents to their configured rooms
                    let server_name = &config.matrix.server_name;
                    for agent in &config.agent {
                        let agent_user_id = format!("@{}:{}", agent.name, server_name);
                        for room_name in &agent.rooms {
                            if let Some(room_id) = mappings.iter().find(|(n, _)| n == room_name).map(|(_, id)| id) {
                                log(&format!("inviting {} to room {}", agent_user_id, room_name));
                                if let Err(e) = client.invite_user(room_id, &agent_user_id).await {
                                    // Don't fail if invite fails (agent might already be in room)
                                    log(&format!("invite failed (may already be member): {}", e));
                                }
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    log(&format!("failed to ensure rooms: {}", e));
                    eprintln!("bz: failed to ensure rooms: {}", e);
                }
                Err(_) => {
                    log("ensure_rooms timed out after 5s");
                    eprintln!("bz: room setup timed out (continuing without Matrix rooms)");
                }
            }

            log("rooms ensured, Matrix setup complete");
            Some(client)
        }
        Err(e) => {
            log(&format!("Matrix connection failed: {}", e));
            eprintln!("bz: Matrix connection failed: {} (continuing without Matrix)", e);
            None
        }
    };
    log(&format!("Matrix client initialized: {}", matrix_client.is_some()));

    log("entering TUI setup");
    // Set up terminal first to get size
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

    // Get terminal size (minimum 24x80 to avoid vt100 panics)
    let size = terminal.size()?;
    let pty_height = size.height.saturating_sub(1).max(24);
    let show_sidebar = size.width >= MOBILE_WIDTH_THRESHOLD;
    let pty_cols = if show_sidebar {
        size.width.saturating_sub(SIDEBAR_WIDTH).max(80)
    } else {
        size.width.max(80)
    };

    // Create rooms for each channel in config, each with a PTY
    let mut rooms = Vec::new();
    for ch_config in &config.channel {
        // Get room_id for this channel (or use "default" if no Matrix)
        let room_id = channel_room_map
            .get(&ch_config.name)
            .cloned()
            .unwrap_or_else(|| "default".to_string());

        // Spawn PTY via chaperone
        let socket_path = user_chaperone
            .spawn_pty(&room_id, ch_config.cwd.as_deref(), Some(&ch_config.command), pty_height, pty_cols)
            .await?;

        // Wait briefly for socket to be ready
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect to PTY socket
        let channel = ChaperoneChannel::new(
            ch_config.name.clone(),
            ch_config.command.clone(),
            ch_config.cwd.clone(),
            socket_path.clone(),
            pty_height,
            pty_cols,
        )
        .await?;

        // Register PTY in Matrix room state
        if let Some(ref client) = matrix_client {
            let pty = AttachedPty {
                pty_id: channel.id().to_string(),
                user_id: client.user_id().to_string(),
                socket: socket_path.display().to_string(),
                command: ch_config.command.clone(),
            };
            if let Err(e) = client.attach_pty(&room_id, pty).await {
                log(&format!("failed to register PTY in room state: {}", e));
            }
        }

        // Create room with chat + PTY (starts on PTY screen)
        let room = RoomView::with_pty(room_id, ch_config.name.clone(), channel);
        rooms.push(room);
    }

    // Create app with rooms
    let mut app = App::new(rooms, size.width, matrix_client);

    // Run the app
    let result = run(&mut terminal, &mut app, &mut user_chaperone, &mut message_rx).await;

    // Drain any buffered terminal events to prevent them leaking to underlying terminal
    while crossterm::event::poll(Duration::from_millis(0))? {
        let _ = crossterm::event::read();
    }

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

/// Render sidebar with rooms instead of channels
fn render_room_sidebar(
    frame: &mut ratatui::Frame,
    area: Rect,
    rooms: &[RoomView],
    focused: usize,
    _cached_matrix_rooms: &[(String, String)],
) {
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, List, ListItem};

    let items: Vec<ListItem> = rooms
        .iter()
        .enumerate()
        .map(|(i, room)| {
            let is_focused = i == focused;
            let activity = room.activity();

            // Build room line with focus indicator
            let prefix = if is_focused {
                " \u{25B8} ".to_string() // ▸
            } else {
                "   ".to_string()
            };

            // Screen indicator
            let screen_idx = room.current_screen_index();
            let screen_count = room.screen_count();
            let screen_info = if screen_count > 1 {
                format!(" [{}/{}]", screen_idx + 1, screen_count)
            } else {
                String::new()
            };

            let mut spans = vec![
                Span::styled(
                    prefix,
                    if is_focused {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(
                    "#",
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(&room.name),
                Span::styled(
                    screen_info,
                    Style::default().fg(Color::DarkGray),
                ),
            ];

            // Activity indicator
            use crate::pty::ActivityState;
            match activity {
                ActivityState::Idle | ActivityState::Pending { .. } => {}
                ActivityState::Active(0) => {
                    spans.push(Span::styled(
                        " \u{25CF}".to_string(), // ●
                        Style::default().fg(Color::Yellow),
                    ));
                }
                ActivityState::Active(n) => {
                    spans.push(Span::styled(
                        format!(" \u{25C6} {}", n), // ◆
                        Style::default()
                            .fg(Color::Red)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            }

            let style = if is_focused {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if matches!(activity, ActivityState::Active(_)) {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();

    let version = concat!(" bz v", env!("CARGO_PKG_VERSION"), " ");
    let block = Block::default()
        .title(version)
        .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(Color::DarkGray));

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
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
    user_chaperone: &mut UserChaperone,
    message_rx: &mut Option<tokio::sync::mpsc::Receiver<crate::matrix_client::MatrixMessage>>,
) -> Result<()> {
    let mut event_stream = EventStream::new();
    let mut render_interval = tokio::time::interval(Duration::from_millis(16));
    let mut room_update_interval = tokio::time::interval(Duration::from_secs(5));

    // Drain any buffered terminal events from before startup
    while crossterm::event::poll(Duration::from_millis(0))? {
        let _ = crossterm::event::read();
    }

    // Initial room fetch
    app.update_matrix_rooms().await;

    loop {
        // Process any pending PTY output from all channels
        app.process_pending();

        // Poll for incoming Matrix messages (non-blocking)
        if let Some(ref mut rx) = message_rx {
            while let Ok(msg) = rx.try_recv() {
                // Find the room index and add the message
                let room_idx = app.rooms.iter().position(|r| r.room_id == msg.room_id);
                if let Some(idx) = room_idx {
                    use crate::chat_view::ChatMessage;
                    let sender_name = msg.sender_display_name.unwrap_or_else(|| {
                        // Extract local part of Matrix ID (@user:server -> user)
                        msg.sender.trim_start_matches('@').split(':').next().unwrap_or(&msg.sender).to_string()
                    });
                    let timestamp = chrono::DateTime::from_timestamp(msg.timestamp as i64, 0)
                        .map(|dt| dt.format("%H:%M").to_string())
                        .unwrap_or_else(|| "??:??".to_string());
                    let chat_msg = ChatMessage::new(sender_name, msg.content, timestamp, false);

                    // Check if this room/screen is focused
                    let is_focused_chat = idx == app.focused_room
                        && !app.focused_room().on_pty();

                    if let Some(chat_state) = app.rooms[idx].chat_state_mut() {
                        chat_state.add_message(chat_msg);
                        // Mark as unread if not currently viewing this chat
                        if !is_focused_chat {
                            chat_state.has_unread = true;
                        }
                    }
                }
            }
        }

        tokio::select! {
            // Periodic room list update
            _ = room_update_interval.tick() => {
                app.update_matrix_rooms().await;
            }
            // Handle terminal events
            Some(event_result) = event_stream.next() => {
                match event_result? {
                    Event::Key(key) => {
                        // Handle quit confirmation modal
                        if app.quit_confirm {
                            if key.code == KeyCode::Char('y') || key.code == KeyCode::Char('Y') {
                                // Confirmed quit - kill daemon and all agents
                                if let Some(socket_path) = daemon::find_session() {
                                    let _ = daemon::stop_session(&socket_path);
                                }
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
                                            if let Some(idx) = picker.selected_index() {
                                                app.switch_to_room(idx);
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
                                            picker.pop_char_from_rooms(&app.rooms);
                                        }
                                        KeyCode::Char(c) => {
                                            picker.push_char_from_rooms(c, &app.rooms);
                                        }
                                        _ => {}
                                    }
                                } else if app.current_pty().map(|p| p.is_scrolled()).unwrap_or(false) {
                                    // In scroll mode (PTY screen only)
                                    if let Some(pty) = app.current_pty_mut() {
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
                                    }
                                // TODO: revisit PageUp/PageDown intercept for scroll mode
                                // } else if key.code == KeyCode::PageUp && app.on_pty_screen() {
                                //     if let Some(pty) = app.current_pty_mut() {
                                //         let page = pty.screen().size().0 as usize;
                                //         pty.scroll_up(page.saturating_sub(2));
                                //     }
                                } else if key.code == KeyCode::Char('b')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    app.input_mode = InputMode::Leader;
                                } else if key.code == KeyCode::Char('k')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    let mut picker = Picker::new();
                                    picker.update_filter_from_rooms(&app.rooms);
                                    app.picker = Some(picker);
                                } else if key.code == KeyCode::Char('\\')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    app.show_sidebar = !app.show_sidebar;
                                    let (cols, rows) = crossterm::terminal::size()?;
                                    let pty_height = rows.saturating_sub(1);
                                    app.resize_all(pty_height, cols);
                                } else if app.on_pty_screen() {
                                    // Send key to PTY
                                    if let Some(bytes) = key_to_bytes(key) {
                                        app.write_to_current_pty(&bytes);
                                    }
                                } else {
                                    // Chat screen - handle input
                                    match key.code {
                                        KeyCode::Enter => {
                                            // Send message via Matrix (will appear when sync receives it)
                                            if let Some(chat_state) = app.focused_room_mut().chat_state_mut() {
                                                let message = chat_state.take_input();
                                                if !message.is_empty() {
                                                    let room_id = app.focused_room().room_id.clone();
                                                    if let Some(ref client) = app.matrix_client {
                                                        let client = client.client().clone();
                                                        tokio::spawn(async move {
                                                            if let Ok(room_id) = room_id.parse::<matrix_sdk::ruma::OwnedRoomId>() {
                                                                if let Some(room) = client.get_room(&room_id) {
                                                                    let content = matrix_sdk::ruma::events::room::message::RoomMessageEventContent::text_plain(&message);
                                                                    let _ = room.send(content).await;
                                                                }
                                                            }
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                        KeyCode::Backspace => {
                                            if let Some(chat_state) = app.focused_room_mut().chat_state_mut() {
                                                chat_state.pop_input();
                                            }
                                        }
                                        KeyCode::Char(c) => {
                                            if let Some(chat_state) = app.focused_room_mut().chat_state_mut() {
                                                chat_state.push_input(c);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            InputMode::Leader => {
                                app.input_mode = InputMode::Normal;

                                match key.code {
                                    // Room navigation (j/k)
                                    KeyCode::Char('j') | KeyCode::Down => {
                                        app.next_room();
                                    }
                                    KeyCode::Char('k') | KeyCode::Up => {
                                        app.prev_room();
                                    }
                                    KeyCode::Char('n') => {
                                        app.next_room();
                                    }
                                    KeyCode::Char('p') => {
                                        app.prev_room();
                                    }
                                    // Screen navigation (h/l)
                                    KeyCode::Char('h') | KeyCode::Left => {
                                        app.prev_screen();
                                    }
                                    KeyCode::Char('l') | KeyCode::Right => {
                                        app.next_screen();
                                    }
                                    KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                                        let idx = (c as usize) - ('1' as usize);
                                        app.switch_to_room(idx);
                                    }
                                    KeyCode::Char('/') => {
                                        let mut picker = Picker::new();
                                        picker.update_filter_from_rooms(&app.rooms);
                                        app.picker = Some(picker);
                                    }
                                    KeyCode::Char('[') => {
                                        // Enter scroll mode on PTY
                                        if let Some(pty) = app.current_pty_mut() {
                                            let scrollback = pty.scrollback_len();
                                            if scrollback > 0 {
                                                pty.scroll_up(1);
                                            }
                                        }
                                    }
                                    KeyCode::Char('q') => {
                                        // Quit (chaperone stays alive for session resume)
                                        break;
                                    }
                                    KeyCode::Char('Q') => {
                                        // Show quit confirmation
                                        app.quit_confirm = true;
                                        app.input_mode = InputMode::Normal;
                                    }
                                    KeyCode::Char('b') => {
                                        // Send Ctrl+B to PTY
                                        let bytes = vec![2];
                                        app.write_to_current_pty(&bytes);
                                    }
                                    KeyCode::Char('t') => {
                                        // Spawn new terminal in current room
                                        let (cols, rows) = crossterm::terminal::size()?;
                                        let pty_height = rows.saturating_sub(1).max(24);
                                        let pty_cols = if app.show_sidebar {
                                            cols.saturating_sub(SIDEBAR_WIDTH).max(80)
                                        } else {
                                            cols.max(80)
                                        };

                                        // Get room info from focused room
                                        let room = app.focused_room();
                                        let room_id = room.room_id.clone();
                                        let room_name = room.name.clone();

                                        // Get cwd from current PTY, or create tmpdir
                                        let cwd = app.current_pty()
                                            .and_then(|p| p.cwd.clone())
                                            .or_else(|| {
                                                // Create tmpdir for chat-only rooms
                                                let tmpdir = format!("/tmp/bz-room-{}", room_id.replace(':', "_").replace('!', ""));
                                                if let Err(e) = std::fs::create_dir_all(&tmpdir) {
                                                    log(&format!("failed to create tmpdir: {}", e));
                                                }
                                                Some(tmpdir)
                                            });

                                        // Spawn PTY via chaperone
                                        match user_chaperone.spawn_pty(&room_id, cwd.as_deref(), None, pty_height, pty_cols).await {
                                            Ok(socket_path) => {
                                                // Wait briefly for socket
                                                tokio::time::sleep(Duration::from_millis(100)).await;

                                                // Connect to PTY socket
                                                let pty_count = app.focused_room().screen_count();
                                                match ChaperoneChannel::new(
                                                    format!("{}-{}", room_name, pty_count),
                                                    std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
                                                    cwd,
                                                    socket_path.clone(),
                                                    pty_height,
                                                    pty_cols,
                                                ).await {
                                                    Ok(channel) => {
                                                        // Register in Matrix room state
                                                        if let Some(ref client) = app.matrix_client {
                                                            let pty = AttachedPty {
                                                                pty_id: channel.id().to_string(),
                                                                user_id: client.user_id().to_string(),
                                                                socket: socket_path.display().to_string(),
                                                                command: std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()),
                                                            };
                                                            if let Err(e) = client.attach_pty(&room_id, pty).await {
                                                                log(&format!("failed to register PTY: {}", e));
                                                            }
                                                        }

                                                        // Add PTY to room and switch to it
                                                        app.focused_room_mut().add_pty(channel, true);
                                                    }
                                                    Err(e) => {
                                                        log(&format!("failed to connect to PTY: {}", e));
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                log(&format!("failed to spawn PTY: {}", e));
                                            }
                                        }
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
                                let room_idx = mouse.row.saturating_sub(1) as usize;
                                if room_idx < app.rooms.len() {
                                    app.switch_to_room(room_idx);
                                }
                            }
                        }
                    }
                    Event::Paste(text) => {
                        app.write_to_current_pty(text.as_bytes());
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

                        // Render sidebar with rooms
                        render_room_sidebar(frame, h_chunks[0], &app.rooms, app.focused_room, &app.cached_rooms);

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

                    // Render current screen (chat or PTY)
                    let pty_status = app.current_pty()
                        .map(|p| p.pty_status().clone())
                        .unwrap_or(PtyStatus::Running);

                    if app.on_pty_screen() {
                        // PTY content
                        if let Some(pty) = app.current_pty_mut() {
                            if pty_status == PtyStatus::Exited {
                                pty.apply_scroll_for_render();
                                let term_widget = TerminalWidget::new(pty.screen());
                                frame.render_widget(term_widget, v_chunks[0]);
                                pty.reset_scroll_view();

                                let exit_msg = Paragraph::new("Process exited")
                                    .style(Style::default().fg(Color::Yellow))
                                    .alignment(Alignment::Center);
                                frame.render_widget(exit_msg, v_chunks[0]);
                            } else {
                                pty.apply_scroll_for_render();
                                let term_widget = TerminalWidget::new(pty.screen());
                                frame.render_widget(term_widget, v_chunks[0]);
                                pty.reset_scroll_view();
                            }
                        }
                    } else {
                        // Chat screen
                        use crate::chat_view::ChatViewWidget;
                        let room = app.focused_room();
                        let room_name = room.name.clone();
                        if let Some(chat_state) = app.focused_room().chat_state() {
                            let chat_widget = ChatViewWidget::new(chat_state, &room_name, true);
                            frame.render_widget(chat_widget, v_chunks[0]);
                        }
                    }

                    // Status line
                    let (is_scrolled, scroll_offset, scrollback_len) = app.current_pty_mut()
                        .map(|p| (p.is_scrolled(), p.scroll_offset, p.scrollback_len()))
                        .unwrap_or((false, 0, 0));

                    // Screen position indicator
                    let screen_idx = app.focused_room().current_screen_index();
                    let screen_count = app.focused_room().screen_count();
                    let screen_indicator = format!("[{}/{}]", screen_idx + 1, screen_count);

                    let (status, status_style) = if pty_status == PtyStatus::Exited {
                        (
                            format!(" EXITED {} │ ^K switch room ", screen_indicator),
                            Style::default()
                                .bg(Color::Rgb(100, 50, 50))
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else if is_scrolled {
                        (
                            format!(" SCROLL [{}/{}] {} │ j/k line │ PgUp/PgDn page │ g/G top/bottom │ Esc/q exit ", scroll_offset, scrollback_len, screen_indicator),
                            Style::default()
                                .bg(Color::Rgb(60, 60, 100))
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        match app.input_mode {
                            InputMode::Normal => {
                                let scroll_hint = if scrollback_len > 0 && app.on_pty_screen() {
                                    format!(" │ ^B[ scroll ({} lines)", scrollback_len)
                                } else {
                                    String::new()
                                };
                                (
                                    format!(" {} │ ^K search │ ^B leader{} ", screen_indicator, scroll_hint),
                                    Style::default()
                                        .bg(Color::Rgb(30, 30, 40))
                                        .fg(Color::DarkGray),
                                )
                            }
                            InputMode::Leader => (
                                format!(" LEADER {} │ j/k room │ h/l screen │ 1-9 jump │ / search │ t new term │ q quit ", screen_indicator),
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
                        let picker_widget = PickerWidget::from_rooms(picker, &app.rooms);
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
