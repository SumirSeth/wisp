//! System tray icon, menu and notifications.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR};

use crate::util::copy_wide;

pub const WM_TRAY: u32 = WM_APP + 1;
const ICON_ID: u32 = 1;

pub const CMD_OPEN: u32 = 1;
pub const CMD_RESCAN: u32 = 2;
pub const CMD_SETTINGS: u32 = 3;
pub const CMD_AUTOSTART: u32 = 4;
pub const CMD_QUIT: u32 = 5;
pub const CMD_CONFIG: u32 = 6;

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        ..Default::default()
    }
}

pub fn add(hwnd: HWND, icon: HICON, tip: &str) {
    let mut d = data(hwnd);
    d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    d.uCallbackMessage = WM_TRAY;
    d.hIcon = icon;
    copy_wide(&mut d.szTip, tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &d);
    }
}

pub fn set_tip(hwnd: HWND, tip: &str) {
    let mut d = data(hwnd);
    d.uFlags = NIF_TIP;
    copy_wide(&mut d.szTip, tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
    }
}

pub fn remove(hwnd: HWND) {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data(hwnd));
    }
}

pub fn notify(hwnd: HWND, title: &str, message: &str, warning: bool) {
    let mut d = data(hwnd);
    d.uFlags = NIF_INFO;
    d.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
    copy_wide(&mut d.szInfoTitle, title);
    copy_wide(&mut d.szInfo, message);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
    }
}

/// Returns the selected command, or 0 if the user dismisses the tray menu.
pub fn show_menu(hwnd: HWND, hotkey: &str, autostart: bool) -> u32 {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let item = |flags: MENU_ITEM_FLAGS, id: u32, text: &str| {
            let _ = AppendMenuW(menu, flags, id as usize, &HSTRING::from(text));
        };
        item(MF_STRING, CMD_OPEN, &format!("Open Wisp\t{hotkey}"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        item(MF_STRING, CMD_RESCAN, "Rescan apps");
        item(MF_STRING, CMD_SETTINGS, "Settings");
        item(MF_STRING, CMD_CONFIG, "Edit config file");
        item(
            if autostart { MF_STRING | MF_CHECKED } else { MF_STRING },
            CMD_AUTOSTART,
            "Start with Windows",
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        item(MF_STRING, CMD_QUIT, "Quit");
        let _ = SetMenuDefaultItem(menu, CMD_OPEN, 0);

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Windows needs foreground ownership to close the menu on an outside click.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd.0 as u32
    }
}
