//! User configuration, stored as TOML in `%APPDATA%\wisp\config.toml`.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use crate::util;

const DEFAULT_CONFIG: &str = r#"# Wisp configuration. Changes are picked up the next time the launcher opens.

# Global shortcut that toggles the launcher.
# Examples: "Alt+Space", "Ctrl+Shift+Space", "Win+Alt+R", "Ctrl+F12"
hotkey = "Alt+Space"

# How many results are visible at once (the list scrolls beyond that).
max_results = 8

# "system", "dark" or "light"
theme = "system"

# Window background: "acrylic", "mica" or "solid"
backdrop = "acrylic"

# Show recently launched apps when the search box is empty.
show_recent = true

# Used by the "Search the web" result. {query} is replaced with the search text.
search_url = "https://www.google.com/search?q={query}"
"#;

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

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkey: String,
    pub max_results: usize,
    pub theme: Theme,
    pub backdrop: Backdrop,
    pub show_recent: bool,
    pub search_url: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Alt+Space".into(),
            max_results: 8,
            theme: Theme::System,
            backdrop: Backdrop::Acrylic,
            show_recent: true,
            search_url: "https://www.google.com/search?q={query}".into(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        util::data_dir().join("config.toml")
    }

    pub fn modified() -> Option<SystemTime> {
        std::fs::metadata(Self::path()).and_then(|m| m.modified()).ok()
    }

    /// Loads the config, writing the documented default file on first run.
    /// A broken file falls back to defaults and returns the parse error.
    pub fn load() -> (Config, Option<String>) {
        let path = Self::path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let _ = std::fs::write(&path, DEFAULT_CONFIG);
                DEFAULT_CONFIG.to_string()
            }
            // Never overwrite a file we couldn't read (locked, not UTF-8, ...).
            Err(e) => return (Config::default(), Some(format!("config.toml: {e}"))),
        };
        match toml::from_str::<Config>(&text) {
            Ok(mut config) => {
                config.max_results = config.max_results.clamp(1, 20);
                (config, None)
            }
            Err(e) => (Config::default(), Some(format!("config.toml: {}", e.message()))),
        }
    }
}
