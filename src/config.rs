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
        })
    }
}
