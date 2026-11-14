//! Every WM_APP message Wisp uses, in one place so no two share a number.
//! 3, 4 and 6 are also sent by other processes (a second Wisp, the settings
//! app), so they must keep their values. 9 is `update_notice::MESSAGE`, which
//! the settings app shares, so it is defined there.

use windows::Win32::UI::WindowsAndMessaging::WM_APP;

/// The tray icon's callback.
pub const WM_TRAY: u32 = WM_APP + 1;
/// The app index has news.
pub const WM_APP_INDEX: u32 = WM_APP + 2;
/// Show the launcher; also sent by another Wisp started twice.
pub const WM_APP_SHOW: u32 = WM_APP + 3;
/// Quit; also sent by the settings app before an update.
pub const WM_APP_QUIT: u32 = WM_APP + 4;
/// File search results arrived.
pub const WM_APP_FILES: u32 = WM_APP + 5;
/// Reload the config; also sent by the settings app.
pub const WM_APP_RELOAD: u32 = WM_APP + 6;
/// Command state changed.
pub const WM_APP_COMMANDS: u32 = WM_APP + 7;
/// The next animation frame.
pub const WM_APP_FRAME: u32 = WM_APP + 8;
pub const WM_SHOW_LAUNCHER: u32 = WM_APP + 20;
pub const WM_REFRESH: u32 = WM_APP + 21;
pub const WM_TRAY_CHANGED: u32 = WM_APP + 22;
pub const WM_DOCK_SHOW_TASKBAR: u32 = WM_APP + 23;
pub const WM_CLOSE_FLYOUT: u32 = WM_APP + 24;
pub const WM_SHOW_NOW: u32 = WM_APP + 25;
pub const WM_RADIOS: u32 = WM_APP + 26;
pub const WM_DOCK_DRAIN: u32 = WM_APP + 27;
pub const WM_DOCK_WAKE: u32 = WM_APP + 28;
pub const WM_ARTWORK: u32 = WM_APP + 29;
pub const WM_TASKBAR_SHOW: u32 = WM_APP + 40;
pub const WM_RECOVERY_DONE: u32 = WM_APP + 41;
/// The dock space's app bar callback.
pub const WM_APPBAR: u32 = WM_APP + 50;
pub const WM_CHECK: u32 = WM_APP + 51;
pub const WM_SET_HEIGHT: u32 = WM_APP + 52;
pub const WM_OUTRANK: u32 = WM_APP + 60;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_numbers_are_unique() {
        let all = [
            WM_TRAY,
            WM_APP_INDEX,
            WM_APP_SHOW,
            WM_APP_QUIT,
            WM_APP_FILES,
            WM_APP_RELOAD,
            WM_APP_COMMANDS,
            WM_APP_FRAME,
            crate::update_notice::MESSAGE,
            WM_SHOW_LAUNCHER,
            WM_REFRESH,
            WM_TRAY_CHANGED,
            WM_DOCK_SHOW_TASKBAR,
            WM_CLOSE_FLYOUT,
            WM_SHOW_NOW,
            WM_RADIOS,
            WM_DOCK_DRAIN,
            WM_DOCK_WAKE,
            WM_ARTWORK,
            WM_TASKBAR_SHOW,
            WM_RECOVERY_DONE,
            WM_APPBAR,
            WM_CHECK,
            WM_SET_HEIGHT,
            WM_OUTRANK,
        ];
        let mut sorted = all.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len());
    }
}
