//! Parses hotkey strings like `"Alt+Space"` into `RegisterHotKey` arguments.

use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub struct Hotkey {
    pub modifiers: HOT_KEY_MODIFIERS,
    pub vk: u32,
}

pub fn parse(spec: &str) -> Result<Hotkey, String> {
    let mut modifiers = MOD_NOREPEAT;
    let mut vk = None;
    for part in spec.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "alt" => modifiers |= MOD_ALT,
            "ctrl" | "control" => modifiers |= MOD_CONTROL,
            "shift" => modifiers |= MOD_SHIFT,
            "win" | "super" | "meta" => modifiers |= MOD_WIN,
            key => {
                if vk.is_some() {
                    return Err(format!("hotkey \"{spec}\" has more than one key"));
                }
                vk = Some(key_code(key).ok_or_else(|| format!("unknown key \"{part}\" in hotkey"))?);
            }
        }
    }
    let vk = vk.ok_or_else(|| format!("hotkey \"{spec}\" has no key"))?;
    Ok(Hotkey { modifiers, vk })
}

fn key_code(key: &str) -> Option<u32> {
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase() as u32);
        }
        let vk = match c {
            '`' => VK_OEM_3,
            '-' => VK_OEM_MINUS,
            '=' => VK_OEM_PLUS,
            '[' => VK_OEM_4,
            ']' => VK_OEM_6,
            '\\' => VK_OEM_5,
            ';' => VK_OEM_1,
            '\'' => VK_OEM_7,
            ',' => VK_OEM_COMMA,
            '.' => VK_OEM_PERIOD,
            '/' => VK_OEM_2,
            _ => return None,
        };
        return Some(vk.0 as u32);
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
        return (1..=24).contains(&n).then(|| (VK_F1.0 + n - 1) as u32);
    }
    let vk = match key {
        "space" => VK_SPACE,
        "enter" | "return" => VK_RETURN,
        "tab" => VK_TAB,
        "esc" | "escape" => VK_ESCAPE,
        "backquote" | "tilde" => VK_OEM_3,
        "insert" => VK_INSERT,
        "delete" => VK_DELETE,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" => VK_PRIOR,
        "pagedown" => VK_NEXT,
        _ => return None,
    };
    Some(vk.0 as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_hotkeys() {
        let hk = parse("Alt+Space").unwrap();
        assert_eq!(hk.modifiers, MOD_ALT | MOD_NOREPEAT);
        assert_eq!(hk.vk, VK_SPACE.0 as u32);

        let hk = parse("ctrl + shift + k").unwrap();
        assert_eq!(hk.modifiers, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT);
        assert_eq!(hk.vk, 'K' as u32);

        assert_eq!(parse("Win+F12").unwrap().vk, VK_F12.0 as u32);
        assert!(parse("Alt").is_err());
        assert!(parse("Alt+Foo").is_err());
        assert!(parse("A+B").is_err());
    }
}
