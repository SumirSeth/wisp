//! Apps pinned to the dock, kept in dock order in the channel's data folder.

use std::{io::Write, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Pin {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    pin: Vec<Pin>,
}

fn parse(text: &str) -> Vec<Pin> {
    let Ok(file) = toml::from_str::<File>(text) else {
        return Vec::new();
    };
    let mut pins: Vec<Pin> = Vec::with_capacity(file.pin.len());
    for pin in file.pin {
        if !pin.id.trim().is_empty() && !pins.iter().any(|old| old.id.eq_ignore_ascii_case(&pin.id)) {
            pins.push(pin);
        }
    }
    pins
}

fn render<'a>(apps: impl IntoIterator<Item = &'a Pin>) -> Option<String> {
    let file = File {
        pin: apps
            .into_iter()
            .map(|app| Pin {
                id: app.id.clone(),
                name: app.name.clone(),
                path: app.path.clone(),
            })
            .collect(),
    };
    toml::to_string(&file).ok()
}

/// The pinned apps, in dock order. A missing or unreadable file pins nothing.
pub fn load(path: &Path) -> Vec<Pin> {
    std::fs::read_to_string(path)
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Saves the pins. The file is written aside first so a crash keeps the old one.
pub fn save<'a>(path: &Path, apps: impl IntoIterator<Item = &'a Pin>) -> std::io::Result<()> {
    let text = render(apps).ok_or_else(|| std::io::Error::other("Cannot save dock pins."))?;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Cannot find the pins folder."))?;
    std::fs::create_dir_all(parent)?;
    let tmp = path.with_extension("toml.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_keep_their_order_through_the_file() {
        let apps = [
            Pin {
                name: "Notepad".into(),
                id: r"C:\Windows\notepad.exe".into(),
                path: Some(r"C:\Windows\notepad.exe".into()),
            },
            Pin {
                name: "Store app".into(),
                id: "Package!App".into(),
                path: None,
            },
        ];
        let loaded = parse(&render(&apps).unwrap());
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            (loaded[0].id.as_str(), loaded[0].path.as_deref()),
            (apps[0].id.as_str(), apps[0].path.as_deref())
        );
        assert_eq!(
            (loaded[1].name.as_str(), loaded[1].path.as_deref()),
            ("Store app", None)
        );
    }

    #[test]
    fn saves_and_replaces_pins_in_a_temporary_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dock-pins.toml");
        assert!(load(&path).is_empty());
        let pins = [Pin {
            id: "Package!App".into(),
            name: "Store".into(),
            path: None,
        }];
        save(&path, &pins).unwrap();
        assert_eq!(load(&path)[0].id, pins[0].id);
        save(&path, &[]).unwrap();
        assert!(load(&path).is_empty());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        std::fs::write(&path, "invalid = [").unwrap();
        assert!(load(&path).is_empty());
    }

    #[test]
    fn a_broken_file_pins_nothing() {
        assert!(parse("pin = 3").is_empty());
        assert!(parse("").is_empty());
        assert_eq!(
            parse("[[pin]]\nid = 'app'\nname = 'A'\n[[pin]]\nid = 'APP'\nname = 'B'\n").len(),
            1
        );
    }
}
