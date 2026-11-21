//! Release channel. Nightly builds use their own data folder, window class,
//! mutex and startup entry, so they run beside a stable install.

use windows::core::{PCWSTR, w};

pub const NIGHTLY: bool = cfg!(feature = "nightly");
pub const NAME: &str = if NIGHTLY { "Wisp Nightly" } else { "Wisp" };
/// Folder under `%APPDATA%`.
pub const DATA_DIR: &str = if NIGHTLY { "wisp-nightly" } else { "wisp" };
pub const LAUNCHER_CLASS: PCWSTR = if NIGHTLY {
    w!("Wisp.Nightly.Launcher")
} else {
    w!("Wisp.Launcher")
};
pub const INSTANCE_MUTEX: PCWSTR = if NIGHTLY {
    w!(r"Local\Wisp.Nightly.Launcher.SingleInstance")
} else {
    w!(r"Local\Wisp.Launcher.SingleInstance")
};
/// Value name under the Run key for Start with Windows.
pub const RUN_VALUE: PCWSTR = if NIGHTLY { w!("Wisp Nightly") } else { w!("Wisp") };

/// Hidden native Settings request, with a numeric page in WPARAM.
pub const NATIVE_SETTINGS_MESSAGE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 10;
pub const SETTINGS_CLASS: PCWSTR = if NIGHTLY {
    w!("Wisp.Nightly.Settings.Native")
} else {
    w!("Wisp.Settings.Native")
};
