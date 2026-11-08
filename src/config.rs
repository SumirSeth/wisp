//! TOML config in `%APPDATA%\wisp\config.toml`.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use crate::util;

/// wisp-settings includes the same file, so both apps create the same config.
const DEFAULT_CONFIG: &str = include_str!("default-config.toml");
/// Values for missing keys that differ from the default config, shared with wisp-settings.
const MISSING_KEYS: &str = include_str!("config-0.3.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backdrop {
    Acrylic,
    Mica,
    Solid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    #[serde(alias = "top")]
    Center,
    /// The query box sits at the bottom of the screen, where the dock is.
    Bottom,
}

/// When hovering an app shows its window list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum WindowListMode {
    #[default]
    Multiple,
    Always,
    Off,
}

/// Keys missing from the user's file take their value from `config-0.3.toml`,
/// then from the default config, as in wisp-settings, so both apps agree on
/// what an old config means.
#[derive(Debug, Deserialize)]
pub struct Config {
    pub hide_taskbar: bool,
    pub dock: bool,
    pub dock_autohide: bool,
    pub dock_icon_size: usize,
    pub hotkey: String,
    pub max_results: usize,
    pub theme: Theme,
    pub backdrop: Backdrop,
    pub position: Position,
    pub show_recent: bool,
    pub search_url: String,
    pub file_search: bool,
    pub file_search_exclude: Vec<String>,
    pub window_list: WindowListMode,
}

impl Default for Config {
    fn default() -> Self {
        Self::parse(DEFAULT_CONFIG).expect("valid default config")
    }
}

impl Config {
    pub fn path() -> PathBuf {
        util::data_dir().join("config.toml")
    }

    pub fn modified() -> Option<SystemTime> {
        std::fs::metadata(Self::path()).and_then(|m| m.modified()).ok()
    }

    /// Creates the default config on first run. Returns defaults and an error
    /// if the file cannot be read or parsed.
    pub fn load() -> (Config, Option<String>) {
        let path = Self::path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let _ = std::fs::write(&path, DEFAULT_CONFIG);
                DEFAULT_CONFIG.to_string()
            }
            // Preserve unreadable files, including locked files and invalid UTF-8.
            Err(e) => return (Config::default(), Some(format!("config.toml: {e}"))),
        };
        match Self::parse(&text) {
            Ok(mut config) => {
                config.max_results = config.max_results.clamp(1, 20);
                config.dock_icon_size = config.dock_icon_size.clamp(24, 40);
                (config, None)
            }
            Err(e) => (Config::default(), Some(format!("config.toml: {}", e.message()))),
        }
    }

    /// Reads a config, taking each missing key from `config-0.3.toml` or the
    /// default config.
    fn parse(text: &str) -> Result<Config, toml::de::Error> {
        let mut table: toml::Table = toml::from_str(DEFAULT_CONFIG)?;
        table.extend(toml::from_str::<toml::Table>(MISSING_KEYS)?);
        table.extend(toml::from_str::<toml::Table>(text)?);
        table.try_into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_pins_are_ignored() {
        let config = Config::parse("dock = true\ndock_apps = ['old.app']").unwrap();
        assert!(config.dock);
        assert_eq!(config.window_list, super::WindowListMode::Multiple);
    }

    #[test]
    fn window_list_modes_deserialize_and_older_config_defaults_to_multiple() {
        for (value, expected) in [
            ("multiple", super::WindowListMode::Multiple),
            ("always", super::WindowListMode::Always),
            ("off", super::WindowListMode::Off),
        ] {
            let config = Config::parse(&format!("window_list = '{value}'")).unwrap();
            assert_eq!(config.window_list, expected);
        }
    }

    #[test]
    fn missing_keys_keep_a_config_from_before_the_dock() {
        // A Wisp 0.3 config has none of the dock's keys and keeps the taskbar.
        let config = Config::parse("hotkey = \"Ctrl+Space\"").unwrap();
        assert_eq!((config.hotkey.as_str(), config.max_results), ("Ctrl+Space", 8));
        assert!(!config.dock && !config.hide_taskbar);
        assert_eq!(config.position, Position::Center);
        // A new config has them all.
        let default = Config::default();
        assert!(default.dock && default.hide_taskbar);
        assert_eq!(default.dock_icon_size, 32);
        assert_eq!(default.position, Position::Bottom);
        assert!(Config::parse("dock = \"yes\"").is_err());
    }
}
