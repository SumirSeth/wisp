use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::{HSTRING, w};

/// Creates `%APPDATA%\wisp` when first needed.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("wisp");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// DirectWrite needs UTF-16 offsets; the text editor uses byte offsets.
pub fn utf16_offset(s: &str, byte: usize) -> u32 {
    s[..byte].encode_utf16().count() as u32
}

/// Reserves one UTF-16 unit for the NUL terminator and truncates text to fit.
pub fn copy_wide(dst: &mut [u16], s: &str) {
    let max = dst.len() - 1;
    let mut n = 0;
    for (d, c) in dst.iter_mut().take(max).zip(s.encode_utf16()) {
        *d = c;
        n += 1;
    }
    dst[n] = 0;
}

pub fn error_box(message: &str) {
    unsafe {
        MessageBoxW(None, &HSTRING::from(message), w!("Wisp"), MB_OK | MB_ICONERROR);
    }
}
