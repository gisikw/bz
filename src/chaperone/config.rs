//! Chaperone configuration
//!
//! Parses the TOML config file for a chaperone instance.
//!
//! Used by bzc binary.

#![allow(dead_code)]

use std::path::Path;

use color_eyre::eyre::{Result, WrapErr};
use serde::Deserialize;

/// Chaperone operating mode
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ChaperoneMode {
    /// PTY-only mode (user chaperone)
    PtyOnly,
    /// Matrix mode (agent chaperone)
    Matrix,
}

/// Chaperone configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ChaperoneConfig {
    /// Chaperone name (e.g., "user" or agent name)
    pub name: String,
    /// Operating mode
    pub mode: ChaperoneMode,
    /// Working directory for agent (where wicket.yaml lives)
    #[serde(default)]
    pub cwd: Option<String>,
}

impl ChaperoneConfig {
    /// Load configuration from a TOML file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .wrap_err_with(|| format!("Failed to read config: {}", path.display()))?;

        toml::from_str(&content)
            .wrap_err_with(|| format!("Failed to parse config: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pty_only() {
        let config: ChaperoneConfig = toml::from_str(r#"
            name = "user"
            mode = "pty-only"
        "#).unwrap();

        assert_eq!(config.name, "user");
        assert_eq!(config.mode, ChaperoneMode::PtyOnly);
    }

    #[test]
    fn test_parse_matrix() {
        let config: ChaperoneConfig = toml::from_str(r#"
            name = "claude"
            mode = "matrix"
        "#).unwrap();

        assert_eq!(config.name, "claude");
        assert_eq!(config.mode, ChaperoneMode::Matrix);
    }
}
