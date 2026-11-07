//! Open windows, for switching to one or closing it.

use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, WPARAM},
        Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
        System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        },
        UI::Input::KeyboardAndMouse::{KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_ESCAPE, keybd_event},
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

pub fn process_path(pid: u32) -> String {
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
        if GetForegroundWindow() == hwnd {
            return Ok(());
        }
        if SetForegroundWindow(hwnd).as_bool() && wait_for_foreground(hwnd) {
            return Ok(());
        }
        // A shell flyout such as the Action Center holds the foreground from
        // an AppContainer, where Windows refuses both a plain activation and
        // thread-input attachment. Escape closes it, as the taskbar's own
        // click does, and then the app can come forward.
        let foreground = GetForegroundWindow();
        if !dismiss_shell_flyout(foreground) {
            return Err("Windows did not switch to the window.".into());
        }
        for _ in 0..20 {
            if GetForegroundWindow() != foreground {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = SetForegroundWindow(hwnd);
        if wait_for_foreground(hwnd) {
            Ok(())
        } else {
            Err("Windows did not switch to the window.".into())
        }
    }
}

/// True when the window is the foreground one, so the dock can toggle it.
pub fn is_foreground(hwnd: isize) -> bool {
    unsafe { GetForegroundWindow() == HWND(hwnd as _) }
}

/// Hides the window the way its minimize button does.
pub fn minimize(hwnd: isize) -> Result<(), String> {
    unsafe {
        let hwnd = HWND(hwnd as _);
        if !IsWindow(Some(hwnd)).as_bool() {
            return Err("The window was closed.".into());
        }
        let _ = ShowWindow(hwnd, SW_MINIMIZE);
        Ok(())
    }
}

/// Waits briefly for a requested activation to land; Windows reports it a
/// moment after the call while the flyout closes.
fn wait_for_foreground(hwnd: HWND) -> bool {
    unsafe {
        for _ in 0..20 {
            if GetForegroundWindow() == hwnd {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        GetForegroundWindow() == hwnd
    }
}

/// Closes the Action Center, Start menu or search, which otherwise keep the
/// foreground and stop the dock from switching apps. Returns whether one was
/// in front.
fn dismiss_shell_flyout(foreground: HWND) -> bool {
    const SHELL: [&str; 4] = [
        r"\shellexperiencehost.exe",
        r"\startmenuexperiencehost.exe",
        r"\searchhost.exe",
        r"\shellhost.exe",
    ];
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(foreground, Some(&mut pid));
        if pid == 0 {
            return false;
        }
        let exe = process_path(pid).to_ascii_lowercase();
        if !SHELL.iter().any(|name| exe.ends_with(name)) {
            return false;
        }
        keybd_event(VK_ESCAPE.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_ESCAPE.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        true
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
