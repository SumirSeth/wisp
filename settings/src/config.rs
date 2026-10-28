//! Read the latest config before each edit, then replace it atomically.

use std::{io::Write, path::PathBuf};

use toml_edit::{DocumentMut, Item, Value};

/// The launcher creates its config from the same file.
pub const DEFAULT_CONFIG: &str = include_str!("../../src/default-config.toml");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub hotkey: String,
    pub max_results: i64,
    pub theme: String,
    pub backdrop: String,
    pub show_recent: bool,
    pub search_url: String,
    pub file_search: bool,
    pub file_search_exclude: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self::from_document(&DEFAULT_CONFIG.parse().expect("valid default config")).expect("valid default values")
    }
}

impl Config {
    fn from_document(doc: &DocumentMut) -> Result<Self, String> {
        let defaults: DocumentMut = DEFAULT_CONFIG.parse().expect("valid default config");
        let get = |key: &str| doc.get(key).unwrap_or(&defaults[key]);
        let string = |key: &str| {
            get(key)
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{key} must be a string."))
        };
        let boolean = |key: &str| {
            get(key)
                .as_bool()
                .ok_or_else(|| format!("{key} must be true or false."))
        };
        let theme = string("theme")?;
        choice(&theme, &["system", "dark", "light"], "theme")?;
        let backdrop = string("backdrop")?;
        choice(&backdrop, &["acrylic", "mica", "solid"], "backdrop")?;
        let max_results = get("max_results")
            .as_integer()
            .filter(|value| *value >= 0)
            .ok_or("max_results must be a positive whole number.")?
            .clamp(1, 20);
        let file_search_exclude = get("file_search_exclude")
            .as_array()
            .ok_or("file_search_exclude must be a list of folder names.")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "Each excluded folder name must be a string.".into())
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            hotkey: string("hotkey")?,
            max_results,
            theme,
            backdrop,
            show_recent: boolean("show_recent")?,
            search_url: string("search_url")?,
            file_search: boolean("file_search")?,
            file_search_exclude,
        })
    }
}

fn choice(value: &str, choices: &[&str], key: &str) -> Result<(), String> {
    if choices.contains(&value) {
        Ok(())
    } else {
        Err(format!("{key} must be {}.", choices.join(", ")))
    }
}

pub fn validate_url(value: &str) -> Result<(), String> {
    if value.contains("{query}") {
        Ok(())
    } else {
        Err("Add {query} where the search text should go.".into())
    }
}

pub fn path() -> Result<PathBuf, String> {
    std::env::var_os("APPDATA")
        .map(|root| PathBuf::from(root).join("wisp/config.toml"))
        .ok_or_else(|| "Windows did not provide the AppData folder.".into())
}

pub struct Store {
    pub path: PathBuf,
}

impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn document(&self) -> Result<DocumentMut, String> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => DEFAULT_CONFIG.to_owned(),
            Err(error) => return Err(format!("Cannot read config.toml: {error}")),
        };
        text.parse()
            .map_err(|error| format!("Cannot parse config.toml: {error}"))
    }

    pub fn load(&self) -> Result<Config, String> {
        Config::from_document(&self.document()?)
    }

    /// Create the default file only when no config exists.
    pub fn ensure_exists(&self) -> Result<(), String> {
        if self.path.exists() {
            return Ok(());
        }
        let parent = self.path.parent().ok_or("The config path has no folder.")?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
        file.write_all(DEFAULT_CONFIG.as_bytes())
            .and_then(|()| file.as_file().sync_all())
            .map_err(|error| error.to_string())?;
        match file.persist_noclobber(&self.path) {
            Ok(_) => Ok(()),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(format!("Cannot create config.toml: {}", error.error)),
        }
    }

    pub fn set(&self, key: &str, mut value: Value) -> Result<Config, String> {
        validate_edit(key, &value)?;
        let mut doc = self.document()?;
        // Refuse to replace a config whose existing fields have invalid types.
        Config::from_document(&doc)?;
        if let Some(old) = doc.get(key).and_then(Item::as_value) {
            if let (Some(previous), Some(next)) = (old.as_array(), value.as_array_mut()) {
                // Keep comments and quoting on folder names that remain in the list.
                for entry in next.iter_mut() {
                    if let Some(existing) = previous.iter().find(|existing| existing.as_str() == entry.as_str()) {
                        *entry = existing.clone();
                    }
                }
                next.set_trailing(previous.trailing().clone());
                next.set_trailing_comma(previous.trailing_comma());
            }
            *value.decor_mut() = old.decor().clone();
        }
        doc[key] = Item::Value(value);
        let config = Config::from_document(&doc)?;
        let parent = self.path.parent().ok_or("The config path has no folder.")?;
        std::fs::create_dir_all(parent).map_err(|error| format!("Cannot create the config folder: {error}"))?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
        file.write_all(doc.to_string().as_bytes())
            .and_then(|()| file.as_file().sync_all())
            .map_err(|error| format!("Cannot write config.toml: {error}"))?;
        // persist replaces the destination on Windows without a delete-then-rename gap.
        file.persist(&self.path)
            .map_err(|error| format!("Cannot replace config.toml: {}", error.error))?;
        Ok(config)
    }
}

fn validate_edit(key: &str, value: &Value) -> Result<(), String> {
    match key {
        "hotkey" => {
            crate::hotkey::parse(value.as_str().ok_or("Shortcut must be text.")?)?;
        }
        "search_url" => validate_url(value.as_str().ok_or("Search URL must be text.")?)?,
        "max_results" if !value.as_integer().is_some_and(|value| (1..=20).contains(&value)) => {
            return Err("Choose between 1 and 20 results.".into());
        }
        "theme" | "backdrop" | "max_results" | "show_recent" | "file_search" | "file_search_exclude" => {}
        _ => return Err(format!("Unknown setting: {key}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_launcher() {
        let config = Config::default();
        assert_eq!(config.hotkey, "Alt+Space");
        assert_eq!(config.max_results, 8);
        assert_eq!(config.theme, "system");
        assert_eq!(config.backdrop, "acrylic");
        assert!(config.show_recent && config.file_search);
        assert_eq!(config.file_search_exclude.len(), 14);
        assert_eq!(Config::from_document(&DocumentMut::new()).unwrap(), config);
    }

    #[test]
    fn preserves_comments_unknown_keys_and_latest_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("config.toml"));
        std::fs::write(
            &store.path,
            "# User note\nhotkey = 'Alt+Space' # Keep this\ncustom = 42\n",
        )
        .unwrap();
        store.set("hotkey", "Ctrl+F12".into()).unwrap();
        let saved = std::fs::read_to_string(&store.path).unwrap();
        assert!(saved.contains("# User note"));
        assert!(saved.contains("# Keep this"));
        assert!(saved.contains("custom = 42"));
        std::fs::write(&store.path, saved.replace("custom = 42", "custom = 99")).unwrap();
        store.set("show_recent", false.into()).unwrap();
        assert!(std::fs::read_to_string(&store.path).unwrap().contains("custom = 99"));
        assert!(!store.load().unwrap().show_recent);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn never_overwrites_broken_or_unreadable_config() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("config.toml"));
        for text in [
            "theme = [",
            "show_recent = 'yes'",
            "theme = 'blue'",
            "file_search_exclude = [42]",
        ] {
            std::fs::write(&store.path, text).unwrap();
            assert!(store.load().is_err());
            assert!(store.set("hotkey", "Ctrl+Space".into()).is_err());
            assert_eq!(std::fs::read_to_string(&store.path).unwrap(), text);
        }
        std::fs::write(&store.path, [0xff, 0xfe]).unwrap();
        assert!(store.set("hotkey", "Ctrl+Space".into()).is_err());
        assert_eq!(std::fs::read(&store.path).unwrap(), [0xff, 0xfe]);
    }

    #[test]
    fn keeps_comments_inside_folder_arrays() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("config.toml"));
        std::fs::write(
            &store.path,
            "file_search_exclude = [\n  # Keep generated files out\n  'target',\n  # Keep this note too\n]\n",
        )
        .unwrap();
        let array: toml_edit::Array = ["target", "build"].into_iter().collect();
        store.set("file_search_exclude", Value::Array(array)).unwrap();
        let saved = std::fs::read_to_string(&store.path).unwrap();
        assert!(saved.contains("# Keep generated files out"));
        assert!(saved.contains("'target'"));
        assert!(saved.contains("# Keep this note too"));
        assert_eq!(store.load().unwrap().file_search_exclude, ["target", "build"]);
    }

    #[test]
    fn creates_defaults_without_replacing_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("nested/config.toml"));
        store.ensure_exists().unwrap();
        assert_eq!(std::fs::read_to_string(&store.path).unwrap(), DEFAULT_CONFIG);
        std::fs::write(&store.path, "# Keep my file").unwrap();
        store.ensure_exists().unwrap();
        assert_eq!(std::fs::read_to_string(&store.path).unwrap(), "# Keep my file");
    }

    #[test]
    fn validates_edits_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("config.toml"));
        for (key, value) in [
            ("hotkey", Value::from("Ctrl")),
            ("search_url", "https://example.com".into()),
            ("max_results", 0.into()),
            ("max_results", 21.into()),
            ("theme", "blue".into()),
            ("unknown", true.into()),
        ] {
            assert!(store.set(key, value).is_err(), "{key}");
        }
        assert!(!store.path.exists());
        assert!(validate_url("https://example.com/?q={query}").is_ok());
        assert!(validate_url("https://example.com/?q={QUERY}").is_err());
        assert_eq!(store.set("max_results", 20.into()).unwrap().max_results, 20);
    }

    #[test]
    fn matches_launcher_result_clamping_and_rejects_negative_numbers() {
        for (value, expected) in [(0, 1), (30, 20), (5, 5)] {
            let doc = format!("max_results = {value}").parse().unwrap();
            assert_eq!(Config::from_document(&doc).unwrap().max_results, expected);
        }
        assert!(Config::from_document(&"max_results = -1".parse().unwrap()).is_err());
    }
}
