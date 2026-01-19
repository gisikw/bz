//! Session daemon for bz
//!
//! Manages PTY sessions that persist across bz restarts.

pub mod conduit;
pub mod pty_manager;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::eyre::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, Mutex};

use crate::config::{AgentConfig, Config};
use crate::env;
use crate::protocol::{
    encode, decode, ClientMessage, DaemonMessage, PtyConfig, SessionInfo,
};

use pty_manager::PtyManager;
use std::process::{Child, Command};

/// Hash the config for change detection
fn hash_config(config: &Config) -> u64 {
    let mut hasher = DefaultHasher::new();
    for ch in &config.channel {
        ch.name.hash(&mut hasher);
        ch.command.hash(&mut hasher);
        ch.cwd.hash(&mut hasher);
    }
    hasher.finish()
}

/// Session daemon
pub struct Daemon {
    /// Session UUID
    session_id: String,
    /// Config hash
    config_hash: u64,
    /// PTY manager
    pty_manager: PtyManager,
    /// Current client connection (if any)
    client: Option<ClientConnection>,
    /// Socket path
    socket_path: PathBuf,
    /// Last focused channel index
    focused: usize,
    /// Agent chaperone processes
    agent_processes: Vec<AgentProcess>,
}

/// Tracked agent chaperone process
struct AgentProcess {
    /// Agent name
    name: String,
    /// Child process handle
    child: Child,
    /// Config file path (for cleanup)
    config_path: PathBuf,
}

struct ClientConnection {
    tx: mpsc::Sender<DaemonMessage>,
}

impl Daemon {
    /// Create a new daemon for the given config (doesn't spawn PTYs yet)
    pub fn new(config: &Config, rows: u16, cols: u16) -> Result<Self> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let config_hash = hash_config(config);

        let pty_manager = PtyManager::new(rows, cols);

        // Create session directory
        let session_dir = env::session_dir();
        std::fs::create_dir_all(&session_dir)?;

        let socket_path = session_dir.join(format!("{}.sock", session_id));

        Ok(Self {
            session_id,
            config_hash,
            pty_manager,
            client: None,
            socket_path,
            focused: 0,
            agent_processes: Vec::new(),
        })
    }

    /// Spawn PTYs for all channels (must be called inside tokio runtime)
    pub fn spawn_ptys(&mut self, config: &Config) -> Result<()> {
        for ch in &config.channel {
            let pty_config = PtyConfig {
                name: ch.name.clone(),
                command: ch.command.clone(),
                cwd: ch.cwd.clone(),
            };
            self.pty_manager.spawn(&pty_config)?;
        }
        Ok(())
    }

    /// Spawn agent chaperone processes
    pub fn spawn_agents(&mut self, config: &Config) -> Result<()> {
        let agents_dir = env::session_dir().join("agents");
        std::fs::create_dir_all(&agents_dir)?;

        for agent in &config.agent {
            if let Err(e) = self.spawn_agent(&agents_dir, agent) {
                eprintln!("bzd: failed to spawn agent '{}': {}", agent.name, e);
            }
        }
        Ok(())
    }

    /// Spawn a single agent chaperone
    fn spawn_agent(&mut self, agents_dir: &PathBuf, agent: &AgentConfig) -> Result<()> {
        // Generate chaperone config file
        let config_path = agents_dir.join(format!("{}.toml", agent.name));
        let cwd_line = agent.cwd.as_ref()
            .map(|c| format!("cwd = \"{}\"\n", c))
            .unwrap_or_default();
        let config_content = format!(
            r#"name = "{}"
mode = "matrix"
{}"#,
            agent.name,
            cwd_line
        );
        std::fs::write(&config_path, &config_content)?;

        // Find bzc binary (same directory as bzd)
        let bzc_path = std::env::current_exe()?
            .parent()
            .map(|p| p.join("bzc"))
            .unwrap_or_else(|| PathBuf::from("bzc"));

        // Spawn bzc process
        let child = Command::new(&bzc_path)
            .arg("--config")
            .arg(&config_path)
            .spawn()
            .map_err(|e| color_eyre::eyre::eyre!("Failed to spawn bzc: {}", e))?;

        eprintln!("bzd: spawned agent chaperone '{}'", agent.name);

        self.agent_processes.push(AgentProcess {
            name: agent.name.clone(),
            child,
            config_path,
        });

        Ok(())
    }

    /// Terminate all agent chaperone processes
    pub fn terminate_agents(&mut self) {
        for mut agent in self.agent_processes.drain(..) {
            let _ = agent.child.kill();
            let _ = agent.child.wait();
            let _ = std::fs::remove_file(&agent.config_path);
        }
    }

    /// Get the socket path
    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    /// Get session info for protocol
    fn session_info(&self) -> SessionInfo {
        SessionInfo {
            session_id: self.session_id.clone(),
            config_hash: self.config_hash,
            ptys: self.pty_manager.infos(),
            focused: self.focused,
        }
    }

    /// Run the daemon
    pub async fn run(self) -> Result<()> {
        // Wrap in Arc<Mutex> for shared access
        let daemon = Arc::new(Mutex::new(self));

        // Get socket path before moving into async block
        let socket_path = {
            let d = daemon.lock().await;
            d.socket_path.clone()
        };

        // Remove stale socket if exists
        let _ = std::fs::remove_file(&socket_path);

        let listener = UnixListener::bind(&socket_path)?;

        // Write PID file
        let pid_path = socket_path.with_extension("pid");
        std::fs::write(&pid_path, std::process::id().to_string())?;

        // PTY output polling task
        let daemon_poll = Arc::clone(&daemon);
        let poll_task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

                let mut d = daemon_poll.lock().await;
                let outputs = d.pty_manager.poll_all();

                // Send output to client if connected
                if let Some(client) = &d.client {
                    for (pty_id, data) in outputs {
                        let msg = DaemonMessage::Output { pty_id, data };
                        let _ = client.tx.send(msg).await;
                    }
                }

                // Check if all PTYs exited and no client - daemon should exit
                if d.pty_manager.all_exited() && d.client.is_none() {
                    break;
                }
            }
        });

        // Accept connections
        loop {
            let (stream, _) = listener.accept().await?;
            let daemon_conn = Arc::clone(&daemon);

            tokio::spawn(async move {
                if let Err(e) = handle_connection(daemon_conn, stream).await {
                    eprintln!("Connection error: {}", e);
                }
            });

            // Check if we should exit (all PTYs dead, no client)
            let d = daemon.lock().await;
            if d.pty_manager.all_exited() && d.client.is_none() {
                break;
            }
        }

        poll_task.abort();

        // Cleanup
        let d = daemon.lock().await;
        let _ = std::fs::remove_file(&d.socket_path);
        let _ = std::fs::remove_file(d.socket_path.with_extension("pid"));

        Ok(())
    }
}

async fn handle_connection(daemon: Arc<Mutex<Daemon>>, mut stream: UnixStream) -> Result<()> {
    // Read first message (should be Attach or AttachTakeover)
    let msg = read_message(&mut stream).await?;

    let (tx, mut rx) = mpsc::channel::<DaemonMessage>(256);

    match msg {
        ClientMessage::Attach => {
            let mut d = daemon.lock().await;
            if d.client.is_some() {
                // Already have a client, reject
                let reject = DaemonMessage::Rejected {
                    reason: "Another client is connected. Use --takeover to take over.".to_string(),
                };
                write_message(&mut stream, &reject).await?;
                return Ok(());
            }
            d.client = Some(ClientConnection { tx: tx.clone() });
        }
        ClientMessage::AttachTakeover => {
            let mut d = daemon.lock().await;
            if let Some(old_client) = d.client.take() {
                // Kick existing client
                let _ = old_client.tx.send(DaemonMessage::Kicked).await;
            }
            d.client = Some(ClientConnection { tx: tx.clone() });
        }
        _ => {
            // Invalid first message
            return Ok(());
        }
    }

    // Send welcome
    {
        let d = daemon.lock().await;
        let welcome = DaemonMessage::Welcome(d.session_info());
        write_message(&mut stream, &welcome).await?;

        // Send history for each PTY
        for pty_info in d.pty_manager.infos() {
            if let Some(history) = d.pty_manager.get_history(pty_info.id) {
                if !history.is_empty() {
                    let hist_msg = DaemonMessage::History {
                        pty_id: pty_info.id,
                        data: history,
                    };
                    write_message(&mut stream, &hist_msg).await?;
                }
            }
            let end_msg = DaemonMessage::HistoryEnd { pty_id: pty_info.id };
            write_message(&mut stream, &end_msg).await?;
        }
    }

    // Split stream for concurrent read/write
    let (mut read_half, mut write_half) = stream.into_split();

    // Task to forward daemon messages to client
    let write_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write_message_owned(&mut write_half, &msg).await.is_err() {
                break;
            }
            // Check for terminal messages
            if matches!(msg, DaemonMessage::Kicked | DaemonMessage::Shutdown) {
                break;
            }
        }
    });

    // Read client messages
    loop {
        match read_message_owned(&mut read_half).await {
            Ok(msg) => {
                match msg {
                    ClientMessage::Input { pty_id, data } => {
                        let mut d = daemon.lock().await;
                        if let Some(pty) = d.pty_manager.get_mut(pty_id) {
                            let _ = pty.write(&data);
                        }
                    }
                    ClientMessage::Resize { pty_id, rows, cols } => {
                        let mut d = daemon.lock().await;
                        if let Some(pty) = d.pty_manager.get_mut(pty_id) {
                            let _ = pty.resize(rows, cols);
                        }
                    }
                    ClientMessage::SetFocus { channel_idx } => {
                        let mut d = daemon.lock().await;
                        d.focused = channel_idx;
                    }
                    ClientMessage::Spawn(config) => {
                        let mut d = daemon.lock().await;
                        match d.pty_manager.spawn(&config) {
                            Ok(info) => {
                                let _ = tx.send(DaemonMessage::PtySpawned(info)).await;
                            }
                            Err(e) => {
                                eprintln!("Failed to spawn PTY: {}", e);
                            }
                        }
                    }
                    ClientMessage::Kill { pty_id: _ } => {
                        // TODO: implement PTY killing
                    }
                    ClientMessage::Detach => {
                        // Clean disconnect - daemon stays alive
                        let mut d = daemon.lock().await;
                        d.client = None;
                        break;
                    }
                    ClientMessage::Quit => {
                        // Kill daemon and all agent chaperones
                        let mut d = daemon.lock().await;
                        d.client = None;
                        d.terminate_agents();
                        // Exit process (PTYs and Conduit will be cleaned up by drop)
                        std::process::exit(0);
                    }
                    _ => {}
                }
            }
            Err(_) => {
                // Connection lost - treat as detach
                let mut d = daemon.lock().await;
                d.client = None;
                break;
            }
        }
    }

    write_task.abort();
    Ok(())
}

async fn read_message(stream: &mut UnixStream) -> io::Result<ClientMessage> {
    // Read length prefix
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;

    // Read payload
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;

    decode(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

async fn read_message_owned(
    stream: &mut tokio::net::unix::OwnedReadHalf,
) -> io::Result<ClientMessage> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;

    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;

    decode(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

async fn write_message(stream: &mut UnixStream, msg: &DaemonMessage) -> io::Result<()> {
    let encoded = encode(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    stream.write_all(&encoded).await
}

async fn write_message_owned(
    stream: &mut tokio::net::unix::OwnedWriteHalf,
    msg: &DaemonMessage,
) -> io::Result<()> {
    let encoded = encode(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    stream.write_all(&encoded).await
}

/// Find existing session socket
pub fn find_session() -> Option<PathBuf> {
    let session_dir = env::session_dir();
    if !session_dir.exists() {
        return None;
    }

    // Find any .sock file
    std::fs::read_dir(&session_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().map(|x| x == "sock").unwrap_or(false))
        .map(|e| e.path())
}

/// Stop a running daemon by sending QUIT via socket, or killing via PID
pub fn stop_session(socket_path: &PathBuf) -> Result<()> {
    // Try to connect and send QUIT
    let stream = std::os::unix::net::UnixStream::connect(socket_path);
    if let Ok(mut stream) = stream {
        use std::io::Write;
        let msg = encode(&ClientMessage::Quit)?;
        let _ = stream.write_all(&msg);
        return Ok(());
    }

    // Fallback: read PID file and kill
    let pid_path = socket_path.with_extension("pid");
    if pid_path.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
        }
        let _ = std::fs::remove_file(&pid_path);
    }
    let _ = std::fs::remove_file(socket_path);

    Ok(())
}
