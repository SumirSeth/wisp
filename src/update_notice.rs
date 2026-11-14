//! Small shared update state. TOML keeps JSON and network code out of Wisp.
use std::{io::Write, path::Path};

/// Shared with the settings app, so it's defined here; listed in `messages`.
pub const MESSAGE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 9;

#[derive(Default)]
pub struct State {
    pub version: String,
    pub last_notified: String,
}

impl State {
    pub fn load(path: &Path) -> Self {
        let table = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| toml::from_str::<toml::Table>(&s).ok())
            .unwrap_or_default();
        let value = |key| {
            table
                .get(key)
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Self {
            version: value("version"),
            last_notified: value("last_notified"),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
        write!(
            file,
            "version = {:?}\nlast_notified = {:?}\n",
            self.version, self.last_notified
        )?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }

    pub fn available(&self) -> bool {
        let parts = |s: &str| s.split('.').map(str::parse::<u32>).collect::<Result<Vec<_>, _>>();
        match (parts(&self.version), parts(env!("CARGO_PKG_VERSION"))) {
            (Ok(latest), Ok(current)) => latest.len() == 3 && latest > current,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notice_survives_restarts_and_ignores_old_or_invalid_versions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update.toml");
        assert!(!State::load(&path).available());
        let mut state = State {
            version: "0.9.0".into(),
            ..Default::default()
        };
        assert!(state.available());
        state.last_notified = state.version.clone();
        state.save(&path).unwrap();
        let loaded = State::load(&path);
        assert_eq!(loaded.last_notified, loaded.version);
        assert!(loaded.available());
        for version in ["0.1.0", env!("CARGO_PKG_VERSION"), "oops", "0.99", "0.9.0-beta"] {
            state.version = version.into();
            assert!(!state.available(), "{version}");
        }
        std::fs::write(path.clone(), "broken").unwrap();
        assert!(!State::load(&path).available());
    }
}
