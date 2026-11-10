//! Comment-preserving edits and atomic config replacement, shared with Settings.

use std::{io::Write, path::Path};
use toml_edit::{DocumentMut, Item, Value};

pub fn replace(doc: &mut DocumentMut, key: &str, mut value: Value) {
    if let Some(old) = doc.get(key).and_then(Item::as_value) {
        if let (Some(previous), Some(next)) = (old.as_array(), value.as_array_mut()) {
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
}

pub fn save(path: &Path, doc: &DocumentMut) -> Result<(), String> {
    let parent = path.parent().ok_or("The config path has no folder.")?;
    std::fs::create_dir_all(parent).map_err(|error| format!("Cannot create the config folder: {error}"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    file.write_all(doc.to_string().as_bytes())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|error| format!("Cannot write config.toml: {error}"))?;
    // persist replaces the destination on Windows without a delete-then-rename gap.
    file.persist(path)
        .map_err(|error| format!("Cannot replace config.toml: {}", error.error))?;
    Ok(())
}
