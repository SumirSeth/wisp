//! Windows integration. These functions run only after the app starts.

use std::path::{Path, PathBuf};

use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, LPARAM, WPARAM},
        System::{Diagnostics::ToolHelp::*, Registry::*},
        UI::{Shell::ShellExecuteW, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Wisp");

fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

pub fn autostart_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, None).is_ok() }
}

pub fn set_autostart(enabled: bool) -> Result<(), String> {
    unsafe {
        if enabled {
            let settings_exe = std::env::current_exe().map_err(|error| error.to_string())?;
            let launcher = launcher_path(&settings_exe)?;
            if !launcher.is_file() {
                return Err("Place wisp.exe next to wisp-settings.exe, then try again.".into());
            }
            let command = wide(format!("\"{}\"", launcher.display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(command.as_ptr().cast()),
                (command.len() * 2) as u32,
            )
            .ok()
            .map_err(|error| format!("Cannot start Wisp with Windows: {error}"))
        } else {
            let status = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(());
            }
            status
                .ok()
                .map_err(|error| format!("Cannot turn off Start with Windows: {error}"))
        }
    }
}

fn launcher_path(settings_exe: &Path) -> Result<PathBuf, String> {
    settings_exe
        .parent()
        .map(|parent| parent.join("wisp.exe"))
        .ok_or_else(|| "Cannot find the Wisp folder.".into())
}

pub fn notify_launcher() {
    unsafe {
        if let Ok(hwnd) = FindWindowW(w!("Wisp.Launcher"), None) {
            let _ = PostMessageW(Some(hwnd), WM_APP + 6, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn powertoys_run_enabled() -> Result<bool, String> {
    let settings =
        std::env::var_os("LOCALAPPDATA").map(|root| PathBuf::from(root).join("Microsoft/PowerToys/settings.json"));
    let setting = match settings.map(std::fs::read_to_string) {
        Some(Ok(text)) => Some(powerlauncher_setting(&text)),
        Some(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => Some(Err(error.to_string())),
        _ => None,
    };
    // A running launcher also signals a conflict if its settings cannot be read.
    let running = powerlauncher_process_running()?;
    if running {
        return Ok(true);
    }
    setting.unwrap_or(Ok(false))
}

fn powerlauncher_setting(text: &str) -> Result<bool, String> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("Cannot read PowerToys settings: {error}"))?;
    Ok(json
        .get("enabled")
        .and_then(|enabled| enabled.get("PowerToys Run"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

fn powerlauncher_process_running() -> Result<bool, String> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|error| error.to_string())?;
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut has_entry = Process32FirstW(snapshot, &mut entry).is_ok();
        let mut running = false;
        while has_entry {
            let len = entry
                .szExeFile
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..len]).eq_ignore_ascii_case("PowerToys.PowerLauncher.exe") {
                running = true;
                break;
            }
            has_entry = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        Ok(running)
    }
}

pub fn has_shortcut_conflict(shortcut: &str, powertoys_enabled: bool) -> bool {
    powertoys_enabled && crate::hotkey::parse(shortcut).ok() == crate::hotkey::parse("Alt+Space").ok()
}

pub fn open_config(path: &Path) -> Result<(), String> {
    let path_wide = wide(path);
    let result = unsafe { ShellExecuteW(None, w!("open"), PCWSTR(path_wide.as_ptr()), None, None, SW_SHOWNORMAL) };
    if result.0 as isize > 32 {
        return Ok(());
    }
    std::process::Command::new("notepad.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Cannot open config.toml: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_the_powertoys_run_flag() {
        assert!(powerlauncher_setting(r#"{"enabled":{"PowerToys Run":true}}"#).unwrap());
        for text in [
            r#"{"enabled":{"PowerToys Run":false}}"#,
            r#"{"PowerToys Run":true}"#,
            "{}",
        ] {
            assert!(!powerlauncher_setting(text).unwrap());
        }
        assert!(powerlauncher_setting("{").is_err());
    }

    #[test]
    fn conflict_uses_parsed_keys_and_modifiers() {
        assert!(has_shortcut_conflict("alt + space", true));
        assert!(!has_shortcut_conflict("Ctrl+Space", true));
        assert!(!has_shortcut_conflict("Alt+Shift+Space", true));
        assert!(!has_shortcut_conflict("Alt+Space", false));
        assert!(!has_shortcut_conflict("invalid", true));
    }

    #[test]
    fn autostart_targets_launcher_beside_settings() {
        assert_eq!(
            launcher_path(Path::new(r"C:\Program Files\Wisp\wisp-settings.exe")).unwrap(),
            PathBuf::from(r"C:\Program Files\Wisp\wisp.exe")
        );
    }
}
