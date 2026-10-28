//! TOML config in `%APPDATA%\wisp\config.toml`.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use crate::util;

/// wisp-settings includes the same file, so both apps create the same config.
const DEFAULT_CONFIG: &str = include_str!("default-config.toml");

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

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkey: String,
    pub max_results: usize,
    pub theme: Theme,
    pub backdrop: Backdrop,
    pub show_recent: bool,
    pub search_url: String,
    pub file_search: bool,
    pub file_search_exclude: Vec<String>,
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
            file_search: true,
            file_search_exclude: [
                "node_modules",
                "AppData",
                "site-packages",
                "__pycache__",
                "venv",
                "target",
                "dist",
                "build",
                "Cache",
                "Code Cache",
                "GPUCache",
                "Service Worker",
                "IndexedDB",
                "Extensions",
            ]
            .map(String::from)
            .to_vec(),
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
        match toml::from_str::<Config>(&text) {
            Ok(mut config) => {
                config.max_results = config.max_results.clamp(1, 20);
                (config, None)
            }
            Err(e) => (Config::default(), Some(format!("config.toml: {}", e.message()))),
        }
    }
}
