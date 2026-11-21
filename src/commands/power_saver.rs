//! Energy Saver status (including AC on 24H2+) and the battery threshold fallback.
//!
//! There is no documented direct saver setter. Unlike a power-mode overlay,
//! ESBATTTHRESHOLD actually requests saver. On systems exposing that setting,
//! battery requests apply 100% (on) or 0% (off), then restore the stored threshold
//! without reapplying it. A subsequent plan reload resumes the user's policy.
//! Failed requests also reapply the original threshold. We never change brightness,
//! switch plans, elevate, or invent settings missing on newer Windows builds.
use std::sync::{
    OnceLock,
    atomic::{AtomicU32, Ordering},
};
use std::time::Duration;
use windows::{
    Win32::{
        Foundation::{HANDLE, HLOCAL, LPARAM, LocalFree, WIN32_ERROR, WPARAM},
        System::Power::*,
        UI::WindowsAndMessaging::*,
    },
    core::GUID,
};

const ENERGY_SAVER: GUID = GUID::from_u128(0x550e8400_e29b_41d4_a716_446655440000);
const SUBGROUP: GUID = GUID::from_u128(0xde830923_a562_41af_a086_e3a2c6bad2da);
const THRESHOLD: GUID = GUID::from_u128(0xe69653ca_cf7f_4f05_aa73_cb833fa90ad4);
static STATUS: AtomicU32 = AtomicU32::new(u32::MAX);
// Registration lives for the process; callback uses only static storage.
static REGISTERED: OnceLock<bool> = OnceLock::new();

fn saver(value: u32) -> Option<bool> {
    match value {
        0 => Some(false),
        1 | 2 => Some(true),
        _ => None,
    }
}

unsafe extern "system" fn changed(_: *const std::ffi::c_void, _: u32, setting: *const std::ffi::c_void) -> u32 {
    if setting.is_null() {
        return 0;
    }
    let setting = unsafe { &*setting.cast::<POWERBROADCAST_SETTING>() };
    if setting.PowerSetting == ENERGY_SAVER && setting.DataLength == 4 {
        let value = unsafe { setting.Data.as_ptr().cast::<u32>().read_unaligned() };
        let previous = STATUS.swap(value, Ordering::SeqCst);
        if previous != value {
            unsafe {
                if let Ok(hwnd) = FindWindowW(crate::channel::LAUNCHER_CLASS, None) {
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_POWERBROADCAST,
                        WPARAM(PBT_APMPOWERSTATUSCHANGE as usize),
                        LPARAM(0),
                    );
                }
            }
        }
    }
    0
}

pub fn read() -> Option<bool> {
    if crate::util::perf::hook_on("WISP_CAPTURE") {
        return Some(false);
    }
    REGISTERED.get_or_init(|| unsafe {
        let params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(changed),
            Context: std::ptr::null_mut(),
        }));
        let mut handle = std::ptr::null_mut();
        PowerSettingRegisterNotification(
            &ENERGY_SAVER,
            DEVICE_NOTIFY_CALLBACK,
            HANDLE((params as *const DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS).cast_mut().cast()),
            &mut handle,
        )
        .is_ok()
    });
    // The documented registration delivers the current value immediately.
    // Older Windows versions use the battery-saver flag, independent of battery presence.
    saver(STATUS.load(Ordering::SeqCst)).or_else(|| {
        let mut status = SYSTEM_POWER_STATUS::default();
        unsafe {
            GetSystemPowerStatus(&mut status).ok()?;
        }
        match status.SystemStatusFlag {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    })
}

/// Supply status is independent of battery presence (including desktop PCs).
pub fn plugged_in() -> Option<bool> {
    if crate::util::perf::hook_on("WISP_CAPTURE") {
        return Some(true);
    }
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe {
        GetSystemPowerStatus(&mut status).ok()?;
    }
    match status.ACLineStatus {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

pub fn set(on: bool) -> Result<(), String> {
    let Some(current) = read() else {
        return Ok(());
    };
    if current == on {
        return Ok(());
    }
    let unsupported = "Windows does not expose a Power saver toggle on this PC. Use Windows Power & battery settings.";
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe {
        GetSystemPowerStatus(&mut status).map_err(|e| e.message())?;
    }
    // Windows supports AC saver through Settings on 24H2+, but does not
    // document a direct setter. Live non-elevated testing on build 26300
    // ignored AC ESBATTTHRESHOLD=100, even with ESPOLICY=Aggressive.
    // Exposing those settings is not evidence that a toggle is supported.
    if status.ACLineStatus != 0 {
        return Err(unsupported.into());
    }
    unsafe {
        let mut allocated = std::ptr::null_mut();
        PowerGetActiveScheme(None, &mut allocated)
            .ok()
            .map_err(|e| e.message())?;
        let scheme = *allocated;
        let _ = LocalFree(Some(HLOCAL(allocated.cast())));
        let mut original = 0;
        WIN32_ERROR(PowerReadDCValueIndex(
            None,
            Some(&scheme),
            Some(&SUBGROUP),
            Some(&THRESHOLD),
            &mut original,
        ))
        .ok()
        .map_err(|_| unsupported.to_string())?;
        let write = |value| {
            WIN32_ERROR(PowerWriteDCValueIndex(
                None,
                &scheme,
                Some(&SUBGROUP),
                Some(&THRESHOLD),
                value,
            ))
            .ok()
        };
        write(if on { 100 } else { 0 }).map_err(|e| e.message())?;
        let applied = PowerSetActiveScheme(None, Some(&scheme)).ok();
        if applied.is_ok() {
            for _ in 0..20 {
                std::thread::sleep(Duration::from_millis(100));
                if self::read() == Some(on) {
                    // PowerWrite changes storage; PowerSetActiveScheme applies it.
                    // Keep the temporary runtime request, never persist a new threshold.
                    write(original).map_err(|e| format!("Could not restore Power saver threshold: {}", e.message()))?;
                    std::thread::sleep(Duration::from_millis(100));
                    if self::read() == Some(on) {
                        return Ok(());
                    }
                    break;
                }
            }
        }
        // Roll back unsupported/ignored writes, including the applied runtime policy.
        write(original).map_err(|e| format!("Could not restore Power saver threshold: {}", e.message()))?;
        PowerSetActiveScheme(None, Some(&scheme))
            .ok()
            .map_err(|e| format!("Could not restore Power saver policy: {}", e.message()))?;
    }
    Err(unsupported.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "changes the real machine's saver state; run explicitly"]
    fn live_round_trip() {
        struct Restore(bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                println!("restore {:?}: {:?}", self.0, set(self.0));
            }
        }
        fn thresholds() -> (GUID, u32, u32) {
            unsafe {
                let mut pointer = std::ptr::null_mut();
                PowerGetActiveScheme(None, &mut pointer).ok().unwrap();
                let scheme = *pointer;
                let _ = LocalFree(Some(HLOCAL(pointer.cast())));
                let (mut ac, mut dc) = (0, 0);
                PowerReadACValueIndex(None, Some(&scheme), Some(&SUBGROUP), Some(&THRESHOLD), &mut ac)
                    .ok()
                    .unwrap();
                WIN32_ERROR(PowerReadDCValueIndex(
                    None,
                    Some(&scheme),
                    Some(&SUBGROUP),
                    Some(&THRESHOLD),
                    &mut dc,
                ))
                .ok()
                .unwrap();
                (scheme, ac, dc)
            }
        }
        let saved = thresholds();
        println!("saved thresholds: AC={}, DC={}", saved.1, saved.2);
        let initial = read().expect("power status unavailable");
        let _restore = Restore(initial);
        let mut status = SYSTEM_POWER_STATUS::default();
        unsafe {
            GetSystemPowerStatus(&mut status).unwrap();
        }
        println!(
            "initial={initial:?}, AC={}, battery flag={}, SystemStatusFlag={}",
            status.ACLineStatus, status.BatteryFlag, status.SystemStatusFlag
        );
        for on in [true, false] {
            let result = set(on);
            println!("set({on})={result:?}");
            result.expect("Windows did not apply the saver request");
            std::thread::sleep(Duration::from_millis(300));
            unsafe {
                GetSystemPowerStatus(&mut status).unwrap();
            }
            println!(
                "read={:?}, notification={}, SystemStatusFlag={}",
                read(),
                STATUS.load(Ordering::SeqCst),
                status.SystemStatusFlag
            );
            assert_eq!(read(), Some(on));
            assert_eq!(thresholds(), saved);
        }
        assert_eq!(thresholds(), saved, "the saved thresholds must not change");
        assert_eq!(read(), Some(false));
        println!("thresholds unchanged: AC={}, DC={}", saved.1, saved.2);
    }
    #[test]
    fn both_energy_saver_levels_are_on_and_unknown_is_not_off() {
        assert_eq!(saver(0), Some(false));
        assert_eq!(saver(1), Some(true));
        assert_eq!(saver(2), Some(true));
        assert_eq!(saver(3), None);
        assert_eq!(saver(u32::MAX), None);
    }
}
