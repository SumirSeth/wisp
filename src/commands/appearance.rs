//! Windows app and system color modes. No polling or theme-file replacement.
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    System::Registry::*,
    UI::WindowsAndMessaging::*,
};
use windows::core::{PCWSTR, w};
const KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
const VALUES: [PCWSTR; 2] = [w!("AppsUseLightTheme"), w!("SystemUsesLightTheme")];

pub fn read() -> Option<[bool; 2]> {
    let mut modes = [false; 2];
    for (index, name) in VALUES.into_iter().enumerate() {
        let mut value = 0u32;
        let mut size = 4;
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                KEY,
                name,
                RRF_RT_REG_DWORD,
                None,
                Some((&raw mut value).cast()),
                Some(&mut size),
            )
            .ok()
            .ok()?;
        }
        if value > 1 {
            return None;
        }
        modes[index] = value != 0;
    }
    Some(modes)
}

pub fn set(light: bool) -> Result<(), String> {
    let previous = read().ok_or("Windows did not provide its color modes.")?;
    // Every change makes all open windows repaint, so skip one that changes nothing.
    if previous == [light; 2] {
        return Ok(());
    }
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, KEY, None, KEY_SET_VALUE, &mut key)
            .ok()
            .map_err(|e| e.message())?;
        let write =
            |name, light| RegSetValueExW(key, name, None, REG_DWORD, Some(&u32::from(light).to_le_bytes())).ok();
        let result = write(VALUES[0], light).and_then(|()| write(VALUES[1], light));
        if result.is_err() {
            // Keep a mixed app/system theme intact if either write fails.
            for (name, mode) in VALUES.into_iter().zip(previous) {
                let _ = write(name, mode);
            }
        }
        let _ = RegCloseKey(key);
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("ImmersiveColorSet").0 as isize),
            SMTO_ABORTIFHUNG,
            200,
            None,
        );
        result.map_err(|e| e.message())
    }
}
