//! Match the launcher's shortcut grammar without calling Windows.

#[derive(Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

pub fn parse(spec: &str) -> Result<Hotkey, String> {
    let mut modifiers = 0x4000; // MOD_NOREPEAT
    let mut vk = None;
    for part in spec.split('+').map(str::trim).filter(|part| !part.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "alt" => modifiers |= 1,
            "ctrl" | "control" => modifiers |= 2,
            "shift" => modifiers |= 4,
            "win" | "super" | "meta" => modifiers |= 8,
            key => {
                if vk.is_some() {
                    return Err(format!("shortcut \"{spec}\" has more than one non-modifier key"));
                }
                vk = Some(key_code(key).ok_or_else(|| format!("unknown key \"{part}\" in shortcut \"{spec}\""))?);
            }
        }
    }
    let vk = vk.ok_or_else(|| format!("shortcut \"{spec}\" has no non-modifier key"))?;
    Ok(Hotkey { modifiers, vk })
}

fn key_code(key: &str) -> Option<u32> {
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase() as u32);
        }
        return Some(match c {
            '`' => 0xc0,
            '-' => 0xbd,
            '=' => 0xbb,
            '[' => 0xdb,
            ']' => 0xdd,
            '\\' => 0xdc,
            ';' => 0xba,
            '\'' => 0xde,
            ',' => 0xbc,
            '.' => 0xbe,
            '/' => 0xbf,
            _ => return None,
        });
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
        return (1..=24).contains(&n).then(|| u32::from(0x70 + n - 1));
    }
    Some(match key {
        "space" => 0x20,
        "enter" | "return" => 0x0d,
        "tab" => 0x09,
        "esc" | "escape" => 0x1b,
        "backquote" | "tilde" => 0xc0,
        "insert" => 0x2d,
        "delete" => 0x2e,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        _ => return None,
    })
}

/// GPUI folds Shift into punctuation. Restore the physical key and modifier.
pub fn recorded(key: &str, control: bool, alt: bool, shift: bool, win: bool) -> Result<String, String> {
    let (key, shifted) = match key {
        "~" => ("`", true),
        "_" => ("-", true),
        "+" => ("=", true),
        "{" => ("[", true),
        "}" => ("]", true),
        "|" => ("\\", true),
        ":" => (";", true),
        "\"" => ("'", true),
        "<" => (",", true),
        ">" => (".", true),
        "?" => ("/", true),
        " " => ("Space", false),
        key => (key, false),
    };
    let mut parts = Vec::new();
    for (pressed, name) in [
        (control, "Ctrl"),
        (alt, "Alt"),
        (shift || shifted, "Shift"),
        (win, "Win"),
    ] {
        if pressed {
            parts.push(name.to_string());
        }
    }
    let mut key = key.to_ascii_lowercase();
    if let Some(first) = key.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    parts.push(key);
    let spec = parts.join("+");
    parse(&spec)?;
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_launcher_modifiers_and_aliases() {
        assert_eq!(
            parse("Alt+Space").unwrap(),
            Hotkey {
                modifiers: 0x4001,
                vk: 0x20
            }
        );
        assert_eq!(
            parse(" ctrl + shift + k ").unwrap(),
            Hotkey {
                modifiers: 0x4006,
                vk: 75
            }
        );
        for alias in ["Win", "Super", "Meta"] {
            assert_eq!(parse(&format!("{alias}+F12")).unwrap().modifiers, 0x4008);
        }
        assert_eq!(parse("Control+Return"), parse("Ctrl+Enter"));
        assert_eq!(parse("Alt++Alt+Space+"), parse("Alt+Space"));
        assert_eq!(parse("a").unwrap().vk, 65);
    }

    #[test]
    fn supports_all_launcher_keys() {
        for key in [
            "`",
            "-",
            "=",
            "[",
            "]",
            "\\",
            ";",
            "'",
            ",",
            ".",
            "/",
            "0",
            "Z",
            "Space",
            "Enter",
            "Return",
            "Tab",
            "Esc",
            "Escape",
            "Backquote",
            "Tilde",
            "Insert",
            "Delete",
            "Home",
            "End",
            "PageUp",
            "PageDown",
        ] {
            assert!(parse(&format!("Alt+{key}")).is_ok(), "{key}");
        }
        for n in 1..=24 {
            assert_eq!(parse(&format!("F{n}")).unwrap().vk, 0x70 + n - 1);
        }
    }

    #[test]
    fn rejects_missing_multiple_and_unknown_keys() {
        for key in [
            "",
            "+++",
            "Alt",
            "Ctrl+Shift",
            "A+B",
            "Alt+Foo",
            "F0",
            "F25",
            "F65536",
            "Up",
            "é",
        ] {
            assert!(parse(key).is_err(), "{key}");
        }
    }

    #[test]
    fn recorder_outputs_launcher_shortcuts() {
        assert_eq!(recorded("space", true, false, true, false).unwrap(), "Ctrl+Shift+Space");
        assert_eq!(recorded("+", false, true, false, false).unwrap(), "Alt+Shift+=");
        assert_eq!(recorded("?", false, false, false, true).unwrap(), "Shift+Win+/");
        assert!(recorded("up", true, false, false, false).is_err());
    }
}
