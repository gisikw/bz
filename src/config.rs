//! Configuration management for bz
//!
//! Loads channel configuration from TOML files.
//! Searches: ./bz.toml, then ~/.config/bz/config.toml

use color_eyre::eyre::Result;
use serde::Deserialize;

/// Top-level configuration
#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub channel: Vec<ChannelConfig>,
    #[serde(default)]
    pub agent: Vec<AgentConfig>,
    #[serde(default)]
    pub matrix: MatrixConfig,
}

/// Matrix/Conduit configuration
#[derive(Debug, Clone, Deserialize)]
pub struct MatrixConfig {
    /// Server name for Matrix federation (e.g., "localhost" or "bz.example.com")
    /// This affects the domain part of user IDs like @user:server_name
    #[serde(default = "default_server_name")]
    pub server_name: String,
}

impl Default for MatrixConfig {
    fn default() -> Self {
        Self {
            server_name: default_server_name(),
        }
    }
}

fn default_server_name() -> String {
    "localhost".to_string()
}

/// Configuration for an AI agent
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    /// Agent name (used for Matrix username)
    pub name: String,
    /// Working directory (where wicket.yaml lives)
    pub cwd: Option<String>,
    /// Chaperone mode: "matrix" (default) or "pty-only"
    #[serde(default = "default_agent_mode")]
    pub mode: String,
    /// Persona description (optional, for future use)
    #[serde(default)]
    pub persona: Option<String>,
    /// Default rooms to join (optional)
    #[serde(default)]
    pub rooms: Vec<String>,
}

fn default_agent_mode() -> String {
    "matrix".to_string()
}

/// Configuration for a single channel
#[derive(Debug, Deserialize)]
pub struct ChannelConfig {
    /// Display name for the channel
    pub name: String,
    /// Working directory (optional, defaults to current dir)
    pub cwd: Option<String>,
    /// Command to run (optional, defaults to $SHELL)
    #[serde(default = "default_command")]
    pub command: String,
}

fn default_command() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "bash".into())
}

impl Config {
    /// Load configuration from a specific path
    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut config: Config = toml::from_str(&content)?;

        // Ensure at least one channel exists
        if config.channel.is_empty() {
            config.channel.push(ChannelConfig {
                name: "default".into(),
                cwd: None,
                command: default_command(),
            });
        }

        Ok(config)
    }

    /// Load configuration from file
    ///
    /// Searches in order:
    /// 1. ./bz.toml (project-local config)
    /// 2. ~/.config/bz/config.toml (user config)
    ///
    /// If no config found, returns default with single channel
    pub fn load() -> Result<Self> {
        let paths = [
            Some(std::path::PathBuf::from("bz.toml")),
            dirs::config_dir().map(|p| p.join("bz/config.toml")),
        ];

        for path in paths.into_iter().flatten() {
            if path.exists() {
                let content = std::fs::read_to_string(&path)?;
                let mut config: Config = toml::from_str(&content)?;

                // Ensure at least one channel exists
                if config.channel.is_empty() {
                    config.channel.push(ChannelConfig {
                        name: "default".into(),
                        cwd: None,
                        command: default_command(),
                    });
                }

                return Ok(config);
            }
        }

        // No config found - default to single channel
        Ok(Config {
            channel: vec![ChannelConfig {
                name: "default".into(),
                cwd: None,
                command: default_command(),
            }],
            agent: vec![],
            matrix: MatrixConfig::default(),
        })
    }
}
