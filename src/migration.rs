//! Compatibility for the 0.8 two-executable updater. No desktop input is used.
use std::path::{Path, PathBuf};
use windows::{
    Win32::{
        Foundation::*,
        System::{Diagnostics::ToolHelp::*, Threading::*},
        UI::WindowsAndMessaging::*,
    },
    core::PWSTR,
};
fn process_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut path = vec![0u16; 32768];
        let mut len = path.len() as u32;
        let result = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut len);
        let _ = CloseHandle(handle);
        result.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&path[..len as usize])))
    }
}
fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}
pub fn same_install_window(hwnd: HWND, dir: &Path) -> bool {
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    process_path(pid).is_some_and(|p| same_path(&p, &dir.join("wisp.exe")))
}
/// Fail closed if the process list cannot be read: defer cleanup until next start.
pub fn settings_running(dir: &Path) -> bool {
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return true;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut next = Process32FirstW(snapshot, &mut entry);
        let mut found = false;
        while next.is_ok() {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if ["wisp-settings.exe", "wisp-settings.exe.old"]
                .iter()
                .any(|n| name.eq_ignore_ascii_case(n))
            {
                match process_path(entry.th32ProcessID) {
                    Some(p) if p.parent().is_some_and(|parent| same_path(parent, dir)) => {
                        found = true;
                        break;
                    }
                    None => {
                        found = true;
                        break;
                    }
                    _ => {}
                }
            }
            next = Process32NextW(snapshot, &mut entry);
        }
        let _ = CloseHandle(snapshot);
        found
    }
}
/// The compatibility copy only forwards or starts the adjacent real launcher.
pub fn dispatch(args: &[String]) -> Result<bool, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if !exe
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("wisp-settings.exe"))
    {
        return Ok(false);
    }
    let dir = exe.parent().ok_or("Cannot find the Wisp folder.")?;
    unsafe {
        if let Ok(hwnd) = FindWindowW(crate::channel::LAUNCHER_CLASS, None)
            && same_install_window(hwnd, dir)
        {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let _ = AllowSetForegroundWindow(pid);
            PostMessageW(
                Some(hwnd),
                crate::channel::NATIVE_SETTINGS_MESSAGE,
                WPARAM(crate::native_settings::page_arg(args)),
                LPARAM(0),
            )
            .map_err(|e| e.to_string())?;
            return Ok(true);
        }
    }
    let mut command = std::process::Command::new(dir.join("wisp.exe"));
    command.arg("--settings");
    if args.iter().any(|a| a == "--welcome") {
        command.arg("--welcome");
    } else if let Some(page) = args.iter().position(|a| a == "--page").and_then(|i| args.get(i + 1)) {
        command.args(["--page", page]);
    }
    command.spawn().map_err(|e| format!("Cannot open Wisp Settings: {e}"))?;
    Ok(true)
}
