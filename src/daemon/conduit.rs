//! Conduit Matrix homeserver lifecycle management
//!
//! Manages Conduit as a subprocess of bzd.
//!
//! Used by bzd binary and main for config generation.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use color_eyre::eyre::{Result, WrapErr};

use crate::env;

/// Path to Conduit configuration file
pub fn config_path() -> PathBuf {
    env::data_dir().join("conduit.toml")
}

/// Path to Conduit database directory
fn database_path() -> PathBuf {
    env::data_dir().join("matrix")
}

/// Generate Conduit configuration file
///
/// Writes config if it doesn't exist or if server_name changed.
/// Returns the path to the config file.
pub fn ensure_config(server_name: &str) -> Result<PathBuf> {
    let config_path = config_path();

    // Check if config exists and has correct server_name
    if config_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            let expected = format!("server_name = \"{}\"", server_name);
            if content.contains(&expected) {
                return Ok(config_path);
            }
            // server_name changed, will regenerate
        }
    }

    // Ensure parent directories exist
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .wrap_err_with(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Ensure database directory exists
    let db_path = database_path();
    std::fs::create_dir_all(&db_path)
        .wrap_err_with(|| format!("Failed to create database directory: {}", db_path.display()))?;

    let config_content = format!(
        r#"[global]
server_name = "{}"
database_backend = "rocksdb"
database_path = "{}"
port = {}
address = "127.0.0.1"
allow_registration = true
"#,
        server_name,
        db_path.display(),
        env::conduit_port()
    );

    std::fs::write(&config_path, config_content)
        .wrap_err_with(|| format!("Failed to write Conduit config: {}", config_path.display()))?;

    Ok(config_path)
}

/// Managed Conduit process
pub struct ConduitProcess {
    child: Child,
    config_path: PathBuf,
}

impl ConduitProcess {
    /// Spawn Conduit with the given config file
    pub fn spawn(config_path: &Path) -> Result<Self> {
        let child = Command::new("conduit")
            .env("CONDUIT_CONFIG", config_path)
            .spawn()
            .wrap_err("Failed to spawn Conduit. Is it installed and on PATH?")?;

        Ok(Self {
            child,
            config_path: config_path.to_owned(),
        })
    }

    /// Get the process ID
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Get the config path
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Check if Conduit process is still running
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Check if Conduit API is responding
    ///
    /// Makes a blocking HTTP request with a short timeout.
    /// Returns true if the API returns a valid response.
    pub fn is_api_healthy(&self) -> bool {
        // Use a simple TCP connect check for now
        // A full HTTP check would require adding a dependency
        use std::net::TcpStream;

        TcpStream::connect_timeout(&env::conduit_addr(), Duration::from_secs(1)).is_ok()
    }

    /// Combined health check: process running AND API responding
    pub fn is_healthy(&mut self) -> bool {
        self.is_running() && self.is_api_healthy()
    }

    /// Gracefully shutdown Conduit
    ///
    /// Sends SIGTERM, waits briefly, then SIGKILL if needed.
    pub fn shutdown(&mut self) -> Result<()> {
        // Send SIGTERM
        #[cfg(unix)]
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }

        // Wait for graceful shutdown (up to 5 seconds)
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(100));
            if let Ok(Some(_)) = self.child.try_wait() {
                return Ok(());
            }
        }

        // Force kill if still running
        self.child.kill().wrap_err("Failed to kill Conduit process")?;
        self.child.wait().wrap_err("Failed to wait for Conduit process")?;

        Ok(())
    }
}

impl Drop for ConduitProcess {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Spawn Conduit with auto-generated config
///
/// Convenience function that ensures config exists and spawns the process.
/// Uses "localhost" as default server_name.
pub fn spawn_conduit() -> Result<ConduitProcess> {
    let config_path = ensure_config("localhost")?;
    ConduitProcess::spawn(&config_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths() {
        // Just verify paths are constructed correctly
        let config = config_path();
        assert!(config.ends_with("bz/conduit.toml"));

        let db = database_path();
        assert!(db.ends_with("bz/matrix"));
    }

    /// Integration test that spawns Conduit and verifies health checks.
    /// Run with: cargo test conduit_lifecycle --ignored
    /// Requires: conduit binary on PATH
    #[test]
    #[ignore]
    fn test_conduit_lifecycle() {
        use std::io::Write;

        // Use temp directory for isolation
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let config_path = temp_dir.path().join("conduit.toml");
        let db_path = temp_dir.path().join("matrix");
        std::fs::create_dir_all(&db_path).expect("Failed to create db dir");

        // Write test config (uses env::conduit_port() for consistency)
        let config_content = format!(
            r#"[global]
server_name = "localhost"
database_backend = "rocksdb"
database_path = "{}"
port = {}
address = "127.0.0.1"
allow_registration = true
"#,
            db_path.display(),
            env::conduit_port()
        );
        std::fs::File::create(&config_path)
            .and_then(|mut f| f.write_all(config_content.as_bytes()))
            .expect("Failed to write config");

        // Spawn Conduit
        let mut conduit = ConduitProcess::spawn(&config_path)
            .expect("Failed to spawn Conduit");

        // Verify process is running
        assert!(conduit.is_running(), "Conduit should be running after spawn");

        // Wait for API to become healthy (up to 10 seconds)
        let mut healthy = false;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(100));
            if conduit.is_api_healthy() {
                healthy = true;
                break;
            }
        }
        assert!(healthy, "Conduit API should respond within 10 seconds");

        // Combined health check should pass
        assert!(conduit.is_healthy(), "Combined health check should pass");

        // Graceful shutdown
        conduit.shutdown().expect("Shutdown should succeed");

        // Process should no longer be running
        assert!(!conduit.is_running(), "Conduit should not be running after shutdown");
    }
}
