use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{NcError, Result};

/// Application configuration, loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub general: GeneralConfig,
    pub colors: ColorConfig,
    pub bookmarks: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub show_hidden: bool,
    pub sort_by: String,
    pub sort_ascending: bool,
    pub left_dir: Option<PathBuf>,
    pub right_dir: Option<PathBuf>,
    pub editor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorConfig {
    pub directory: String,
    pub symlink: String,
    pub executable: String,
    pub selected: String,
    pub cursor: String,
    pub active_border: String,
    pub inactive_border: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        GeneralConfig {
            show_hidden: false,
            sort_by: "name".into(),
            sort_ascending: true,
            left_dir: None,
            right_dir: None,
            editor: None,
        }
    }
}

impl Default for ColorConfig {
    fn default() -> Self {
        ColorConfig {
            directory: "blue".into(),
            symlink: "magenta".into(),
            executable: "green".into(),
            selected: "yellow".into(),
            cursor: "darkgray".into(),
            active_border: "cyan".into(),
            inactive_border: "darkgray".into(),
        }
    }
}

impl Config {
    /// Load config from the standard XDG path, or return defaults.
    pub fn load() -> Self {
        let path = config_path();
        if path.exists() {
            match Self::load_from(&path) {
                Ok(config) => config,
                Err(e) => {
                    log::warn!("Failed to load config from {}: {e}", path.display());
                    Config::default()
                }
            }
        } else {
            Config::default()
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(NcError::Io)?;
        toml::from_str(&content).map_err(|e| NcError::Config(e.to_string()))
    }

    /// Save config to the standard XDG path.
    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(NcError::Io)?;
        }
        let content = toml::to_string_pretty(self).map_err(|e| NcError::Config(e.to_string()))?;
        std::fs::write(&path, content).map_err(NcError::Io)
    }
}

/// Standard config path: ~/.config/ncoxide/config.toml
fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("ncoxide")
        .join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(!config.general.show_hidden);
        assert_eq!(config.general.sort_by, "name");
        assert!(config.general.sort_ascending);
        assert!(config.bookmarks.is_empty());
    }

    #[test]
    fn test_config_roundtrip() {
        let config = Config::default();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.general.sort_by, config.general.sort_by);
    }

    #[test]
    fn test_config_parse() {
        let toml_str = r#"
bookmarks = ["/home", "/tmp"]

[general]
show_hidden = true
sort_by = "size"

[colors]
directory = "blue"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.general.show_hidden);
        assert_eq!(config.general.sort_by, "size");
        assert_eq!(config.bookmarks.len(), 2);
    }
}
