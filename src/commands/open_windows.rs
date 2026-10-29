//! Open windows, for switching to one or closing it.

use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, WPARAM},
        Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
        System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        },
        UI::WindowsAndMessaging::*,
    },
    core::{BOOL, PWSTR},
};

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub hwnd: isize,
    pub title: String,
    /// The program's path, for its icon. Empty if Windows does not tell.
    pub exe: String,
}

impl Window {
    /// Key into the launcher's icon map.
    pub fn icon_key(&self) -> Option<String> {
        (!self.exe.is_empty()).then(|| format!("exe:{}", self.exe.to_lowercase()))
    }
}

/// Windows that Alt+Tab would show, in z-order (most recently used first).
/// Skips this process's windows.
pub fn list() -> Vec<Window> {
    let mut windows: Vec<Window> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&raw mut windows as isize));
    }
    windows
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let windows = &mut *(lparam.0 as *mut Vec<Window>);
        if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
            return true.into();
        }
        let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if style & WS_EX_TOOLWINDOW.0 != 0 {
            return true.into();
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut cloaked).cast(), 4);
        if cloaked != 0 {
            return true.into();
        }
        let mut title = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut title);
        if len <= 0 {
            return true.into();
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == std::process::id() {
            return true.into();
        }
        windows.push(Window {
            hwnd: hwnd.0 as isize,
            title: String::from_utf16_lossy(&title[..len as usize]),
            exe: process_path(pid),
        });
        true.into()
    }
}

fn process_path(pid: u32) -> String {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        if ok {
            String::from_utf16_lossy(&buffer[..len as usize])
        } else {
            String::new()
        }
    }
}

/// Brings the window to the front, restoring it if minimized. Works while the
/// launcher is the foreground window.
pub fn focus(hwnd: isize) -> Result<(), String> {
    let hwnd = HWND(hwnd as _);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return Err("The window was closed.".into());
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd).as_bool() {
            Ok(())
        } else {
            Err("Windows did not switch to the window.".into())
        }
    }
}

/// Asks the window to close, as its close button does.
pub fn close(hwnd: isize) -> Result<(), String> {
    unsafe { PostMessageW(Some(HWND(hwnd as _)), WM_CLOSE, WPARAM(0), LPARAM(0)).map_err(|error| error.message()) }
}

/// The program's name from its path: `C:\…\Code.exe` → "Code".
pub fn program_name(exe: &str) -> &str {
    let file = exe.rsplit('\\').next().unwrap_or(exe);
    file.strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file)
}
