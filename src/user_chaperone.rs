//! User chaperone management
//!
//! Spawns and manages the user's chaperone process (bzc).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

use color_eyre::eyre::{eyre, Result, WrapErr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::chaperone::protocol::{self, ControlMessage};

/// User chaperone process manager
pub struct UserChaperone {
    /// Child process handle
    child: Child,
    /// Config file (kept alive while chaperone runs)
    _config_file: tempfile::NamedTempFile,
    /// Control socket path
    control_socket_path: PathBuf,
    /// Control socket connection
    control_stream: Option<UnixStream>,
}

impl UserChaperone {
    /// Spawn the user chaperone process
    pub fn spawn() -> Result<Self> {
        let config_file = generate_user_config()?;

        // Find bzc binary - check target/debug first for development
        let bzc_path = find_bzc_binary()?;

        let child = Command::new(&bzc_path)
            .arg("--config")
            .arg(config_file.path())
            .spawn()
            .wrap_err_with(|| format!("Failed to spawn bzc from {}", bzc_path.display()))?;

        let control_socket_path = control_socket_path();

        Ok(Self {
            child,
            _config_file: config_file,
            control_socket_path,
            control_stream: None,
        })
    }

    /// Wait for the chaperone to be ready
    ///
    /// Polls for the control socket, connects, and waits for the Ready message.
    pub async fn wait_ready(&mut self) -> Result<()> {
        // Poll for socket existence (up to 5 seconds)
        for _ in 0..50 {
            if self.control_socket_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        if !self.control_socket_path.exists() {
            return Err(eyre!(
                "Chaperone control socket not created at {}",
                self.control_socket_path.display()
            ));
        }

        let mut stream = UnixStream::connect(&self.control_socket_path)
            .await
            .wrap_err("Failed to connect to chaperone control socket")?;

        // Read Ready message
        let msg = read_control_message(&mut stream).await?;
        match msg {
            ControlMessage::Ready => {
                self.control_stream = Some(stream);
                Ok(())
            }
            other => Err(eyre!("Expected Ready message, got {:?}", other)),
        }
    }

    /// Send a SpawnPty command to the chaperone
    ///
    /// Returns the socket path for the new PTY.
    pub async fn spawn_pty(
        &mut self,
        room_id: &str,
        cwd: Option<&str>,
        command: Option<&str>,
    ) -> Result<PathBuf> {
        let stream = self
            .control_stream
            .as_mut()
            .ok_or_else(|| eyre!("Not connected to chaperone"))?;

        let msg = ControlMessage::SpawnPty {
            room_id: room_id.to_string(),
            cwd: cwd.map(String::from),
            command: command.map(String::from),
        };

        let encoded = protocol::encode(&msg)?;
        stream.write_all(&encoded).await?;

        // Wait for PtyAttached response
        let response = read_control_message(stream).await?;
        match response {
            ControlMessage::PtyAttached {
                pty_id, socket, ..
            } => {
                eprintln!("PTY spawned: {} at {}", pty_id, socket);
                Ok(PathBuf::from(socket))
            }
            other => Err(eyre!("Expected PtyAttached, got {:?}", other)),
        }
    }

    /// Gracefully shutdown the chaperone
    pub fn shutdown(&mut self) -> Result<()> {
        // Send SIGTERM
        #[cfg(unix)]
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }

        // Wait briefly for clean exit
        std::thread::sleep(Duration::from_millis(500));

        // Check if exited
        if let Ok(Some(_)) = self.child.try_wait() {
            return Ok(());
        }

        // Force kill if still running
        let _ = self.child.kill();
        let _ = self.child.wait();

        Ok(())
    }
}

impl Drop for UserChaperone {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Generate the user chaperone config file
fn generate_user_config() -> Result<tempfile::NamedTempFile> {
    let config = r#"name = "user"
mode = "pty-only"
"#;

    let mut file = tempfile::NamedTempFile::new().wrap_err("Failed to create temp config file")?;

    file.write_all(config.as_bytes())
        .wrap_err("Failed to write config")?;

    Ok(file)
}

/// Find the bzc binary
fn find_bzc_binary() -> Result<PathBuf> {
    // Check target/debug first (development)
    let debug_path = PathBuf::from("target/debug/bzc");
    if debug_path.exists() {
        return Ok(debug_path);
    }

    // Check target/release
    let release_path = PathBuf::from("target/release/bzc");
    if release_path.exists() {
        return Ok(release_path);
    }

    // Fall back to PATH
    Ok(PathBuf::from("bzc"))
}

/// Control socket path for the user chaperone
fn control_socket_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("bz/chaperones/user/control.sock")
}

/// Read a control message from a stream
async fn read_control_message(stream: &mut UnixStream) -> Result<ControlMessage> {
    // Read length prefix
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .wrap_err("Failed to read message length")?;
    let len = u32::from_le_bytes(len_buf) as usize;

    // Read payload
    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .await
        .wrap_err("Failed to read message payload")?;

    protocol::decode(&payload).wrap_err("Failed to decode control message")
}
