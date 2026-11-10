//! The status matrix and clock at the right end of the dock.

use windows::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::core::{PCWSTR, w};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// In the user's time format, such as "1:45 PM" or "13:45".
    pub time: String,
    /// "Fri 30 Oct".
    pub date: String,
    pub battery: Option<Battery>,
    /// `None` without an output device.
    pub volume: Option<Volume>,
    /// Changes once a minute, so the dock knows when to redraw.
    pub minute: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    /// 0 to 100.
    pub percent: u32,
    pub muted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    /// 0 to 100.
    pub percent: u32,
    pub charging: bool,
}

pub fn read() -> Status {
    let now = unsafe { GetLocalTime() };
    let mut buffer = [0u16; 64];
    let text = |len: i32, buffer: &[u16]| String::from_utf16_lossy(&buffer[..(len.max(1) - 1) as usize]);
    let time = unsafe {
        let len = GetTimeFormatEx(
            PCWSTR::null(),
            TIME_NOSECONDS,
            Some(&now),
            PCWSTR::null(),
            Some(&mut buffer),
        );
        text(len, &buffer)
    };
    let date = unsafe {
        let len = GetDateFormatEx(
            PCWSTR::null(),
            Default::default(),
            Some(&now),
            w!("ddd d MMM"),
            Some(&mut buffer),
            PCWSTR::null(),
        );
        text(len, &buffer)
    };
    Status {
        time,
        date,
        battery: battery(),
        volume: crate::commands::audio::level().map(|(percent, muted)| Volume { percent, muted }),
        minute: now.wMinute,
    }
}

impl Status {
    /// `tray` is the number of inline notification icons, if the dock hosts them.
    pub fn parts(&self, tray: Option<usize>) -> Vec<super::logic::Part> {
        super::logic::parts(tray)
    }
}

fn battery() -> Option<Battery> {
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status).ok()? };
    // 128 means no system battery; 255 means the charge is unknown.
    if status.BatteryFlag & 128 != 0 || status.BatteryLifePercent > 100 {
        return None;
    }
    Some(Battery {
        percent: status.BatteryLifePercent as u32,
        charging: status.ACLineStatus == 1,
    })
}

/// Slow radio readings, owned by the UI after delivery.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Radios {
    /// False until the first reading arrives, so the radios aren't shown as missing.
    pub read: bool,
    pub wifi: Option<crate::commands::wifi::State>,
    pub bluetooth: Option<crate::commands::bluetooth::State>,
}

impl Status {
    pub fn tooltip(&self, radios: &Radios) -> String {
        let volume = match self.volume {
            Some(v) if v.muted => "Muted".into(),
            Some(v) => format!("Volume {}%", v.percent),
            None => "No audio device".into(),
        };
        let battery = match self.battery {
            Some(b) => format!("Battery {}%{}", b.percent, if b.charging { ", charging" } else { "" }),
            None => "No battery".into(),
        };
        let wifi = match &radios.wifi {
            Some(w) if !w.on => "Wi-Fi off".into(),
            Some(w) => w
                .network
                .as_ref()
                .map_or_else(|| "Wi-Fi on, not connected".into(), |n| format!("Wi-Fi: {n}")),
            None if !radios.read => "Wi-Fi: checking".into(),
            None => "No Wi-Fi".into(),
        };
        let bluetooth = match &radios.bluetooth {
            Some(b) if !b.on => "Bluetooth off".into(),
            Some(b) => match b.connected.first() {
                Some(name) if b.connected.len() > 1 => format!("Bluetooth: {name} (+{})", b.connected.len() - 1),
                Some(name) => format!("Bluetooth: {name}"),
                None => "Bluetooth on".into(),
            },
            None if !radios.read => "Bluetooth: checking".into(),
            None => "No Bluetooth".into(),
        };
        format!("{volume}\n{battery}\n{wifi}\n{bluetooth}")
    }
}

/// One cancellable read in flight; no timer or idle thread.
#[derive(Default)]
pub struct RadioWorker {
    active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pending: Option<std::sync::mpsc::Receiver<Option<Radios>>>,
    last: Option<std::time::Instant>,
}

impl RadioWorker {
    pub fn hide(&mut self) {
        self.active.store(false, std::sync::atomic::Ordering::Release);
        self.last = None;
    }

    pub fn refresh(&mut self, hwnd: windows::Win32::Foundation::HWND, message: u32) {
        use std::sync::{atomic::Ordering, mpsc};
        use windows::Win32::{
            Foundation::{HWND, LPARAM, WPARAM},
            System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
            UI::WindowsAndMessaging::PostMessageW,
        };
        if self.pending.is_some()
            || self
                .last
                .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(5))
        {
            return;
        }
        self.last = Some(std::time::Instant::now());
        self.active.store(true, Ordering::Release);
        let active = self.active.clone();
        let (tx, rx) = mpsc::channel();
        let window = hwnd.0 as usize;
        self.pending = Some(rx);
        if std::thread::Builder::new()
            .name("dock-status".into())
            .spawn(move || {
                let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED).is_ok() };
                let mut radios = Radios {
                    read: initialized,
                    ..Radios::default()
                };
                if initialized {
                    if active.load(Ordering::Acquire) {
                        radios.wifi = crate::commands::wifi::state();
                    }
                    if active.load(Ordering::Acquire) {
                        radios.bluetooth = crate::commands::bluetooth::state();
                    }
                    unsafe { RoUninitialize() };
                }
                if tx.send(active.load(Ordering::Acquire).then_some(radios)).is_ok() {
                    unsafe {
                        let _ = PostMessageW(Some(HWND(window as _)), message, WPARAM(0), LPARAM(0));
                    }
                }
            })
            .is_err()
        {
            self.pending = None;
        }
    }

    pub fn take(&mut self) -> Option<Radios> {
        use std::sync::mpsc::TryRecvError;
        match self.pending.as_ref()?.try_recv() {
            Ok(radios) => {
                self.pending = None;
                radios
            }
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                None
            }
            Err(TryRecvError::Empty) => None,
        }
    }
}

impl Drop for RadioWorker {
    fn drop(&mut self) {
        self.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_keeps_four_cells_with_or_without_hardware() {
        let mut status = Status::default();
        let mut radios = Radios::default();
        assert!(
            status
                .tooltip(&radios)
                .ends_with("Wi-Fi: checking\nBluetooth: checking")
        );
        radios.read = true;
        assert_eq!(
            status.tooltip(&radios),
            "No audio device\nNo battery\nNo Wi-Fi\nNo Bluetooth"
        );
        status.volume = Some(Volume {
            percent: 42,
            muted: false,
        });
        status.battery = Some(Battery {
            percent: 80,
            charging: true,
        });
        radios.wifi = Some(crate::commands::wifi::State {
            on: true,
            network: Some("Office".into()),
        });
        radios.bluetooth = Some(crate::commands::bluetooth::State {
            on: true,
            connected: vec!["Headphones".into(), "Mouse".into()],
        });
        assert_eq!(
            status.tooltip(&radios),
            "Volume 42%\nBattery 80%, charging\nWi-Fi: Office\nBluetooth: Headphones (+1)"
        );
        status.volume.as_mut().unwrap().muted = true;
        radios.wifi.as_mut().unwrap().network = None;
        radios.bluetooth.as_mut().unwrap().connected.clear();
        assert_eq!(
            status.tooltip(&radios),
            "Muted\nBattery 80%, charging\nWi-Fi on, not connected\nBluetooth on"
        );
        radios.wifi.as_mut().unwrap().on = false;
        radios.bluetooth.as_mut().unwrap().on = false;
        assert!(status.tooltip(&radios).ends_with("Wi-Fi off\nBluetooth off"));
    }

    #[test]
    fn radio_refresh_is_throttled_and_hide_cancels() {
        use std::sync::{atomic::Ordering, mpsc};
        use windows::Win32::Foundation::HWND;
        let mut worker = RadioWorker {
            active: Default::default(),
            pending: None,
            last: Some(std::time::Instant::now()),
        };
        worker.refresh(HWND::default(), 0);
        assert!(worker.pending.is_none());
        let (tx, rx) = mpsc::channel();
        worker.pending = Some(rx);
        worker.active.store(true, Ordering::Release);
        worker.hide();
        assert!(!worker.active.load(Ordering::Acquire));
        assert!(worker.last.is_none());
        // A pending request prevents another worker even after a reveal.
        worker.refresh(HWND::default(), 0);
        assert!(!worker.active.load(Ordering::Acquire));
        assert!(worker.take().is_none());
        tx.send(None).unwrap();
        assert!(worker.take().is_none());
        assert!(worker.pending.is_none());
    }
}
