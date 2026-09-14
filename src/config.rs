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
    pub preview: PreviewConfig,
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

/// `[preview]` — image preview settings. The terminal matrix behind them is
/// in `docs/image-preview-plan.md`; `ncoxide --probe-terminal` shows what
/// the current terminal reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviewConfig {
    /// `auto` (query the terminal), `off`, `halfblocks`, or a forced
    /// protocol: `sixel`, `kitty`, `iterm2`. `NCOXIDE_IMAGES` overrides it
    /// for one shell.
    pub images: String,
    /// Cell size in pixels `[width, height]` for terminals that do not report
    /// one (typical over SSH or inside a multiplexer); without it a detected
    /// protocol silently degrades to half-blocks.
    pub image_font_size: Option<[u16; 2]>,
    /// Image files above this size show their header facts only.
    pub image_max_bytes: u64,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        PreviewConfig {
            images: "auto".into(),
            image_font_size: None,
            image_max_bytes: 64 * 1024 * 1024,
        }
    }
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
        let Some(path) = config_path() else {
            return Config::default();
        };
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

    /// Save config to the standard XDG path. (Not yet called from the app;
    /// kept as the public API for upcoming bookmark/settings persistence.)
    pub fn save(&self) -> Result<()> {
        let path =
            config_path().ok_or_else(|| NcError::Config("no config directory found".into()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(NcError::Io)?;
        }
        let content = toml::to_string_pretty(self).map_err(|e| NcError::Config(e.to_string()))?;
        std::fs::write(&path, content).map_err(NcError::Io)
    }
}

/// Standard config path: ~/.config/ncoxide/config.toml. `None` when no
/// config directory can be determined (no $HOME) — a literal "~" fallback
/// would never expand.
fn config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("ncoxide").join("config.toml"))
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
    fn test_preview_config_parse_and_defaults() {
        let config: Config =
            toml::from_str("[preview]\nimages = \"sixel\"\nimage_font_size = [9, 18]\n").unwrap();
        assert_eq!(config.preview.images, "sixel");
        assert_eq!(config.preview.image_font_size, Some([9, 18]));
        assert_eq!(config.preview.image_max_bytes, 64 * 1024 * 1024);

        let defaults = Config::default();
        assert_eq!(defaults.preview.images, "auto");
        assert!(defaults.preview.image_font_size.is_none());
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
