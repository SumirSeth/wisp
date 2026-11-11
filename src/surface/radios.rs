//! Open-view radio state, pure selection/password logic, and a bounded worker.
use super::{Input, Widget};
use crate::{
    commands::{Command, bluetooth, bluetooth_audio, glyph, wifi},
    search::Action,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    UI::WindowsAndMessaging::PostMessageW,
};

#[derive(Default, PartialEq, Debug)]
pub enum Phase {
    #[default]
    Idle,
    Password(String),
    Pending(String),
    Failed(String, String),
}
impl Phase {
    pub fn cancel(&mut self) {
        if matches!(self, Self::Password(_)) {
            *self = Self::Idle;
        }
    }
    pub fn submit(&mut self, has_password: bool) -> Option<String> {
        if !has_password {
            return None;
        }
        let Self::Password(id) = self else { return None };
        let id = id.clone();
        *self = Self::Pending(id.clone());
        Some(id)
    }
    pub fn finish(&mut self, error: Option<&str>) {
        if let Self::Pending(id) = self {
            *self = match error {
                Some(error) => Self::Failed(id.clone(), error.into()),
                None => Self::Idle,
            };
        }
    }
    pub fn select(&mut self, network: &wifi::Network) -> bool {
        if matches!(self, Self::Pending(_)) {
            return false;
        }
        if !network.connected
            && network.secured
            && (network.profile.is_none() || matches!(self, Self::Failed(id, _) if id == &network.name))
        {
            *self = Self::Password(network.name.clone());
            false
        } else {
            *self = Self::Pending(network.name.clone());
            true
        }
    }
}

#[derive(Default)]
pub struct Panel {
    pub wifi: Option<bool>,
    pub networks: Option<wifi::Networks>,
    pub devices: Vec<bluetooth::Device>,
    pub on: bool,
    pub phase: Phase,
    // Kept apart from the search editor, history, clipboard and debug output.
    pub password: Vec<u16>,
    pending: Option<Receiver<Reading>>,
    active: Arc<AtomicBool>,
    last: Option<Instant>,
    pub error: Option<String>,
    power: Option<bool>,
    queued_power: Option<bool>,
    last_scan: Option<Instant>,
    confirmed_on: bool,
    read: bool,
    access: Option<crate::commands::radio::Access>,
}
struct Reading {
    on: bool,
    networks: Option<wifi::Networks>,
    devices: Vec<bluetooth::Device>,
    result: Result<(), String>,
}
pub enum Job {
    Scan,
    Read,
    Power(bool, crate::commands::radio::Access),
    Network(wifi::Network, Option<String>),
    Audio(String, bool),
}

impl Panel {
    pub fn close(&mut self) {
        self.active.store(false, Ordering::Release);
        self.pending = None;
        self.wifi = None;
        self.password.fill(0);
        self.password.clear();
        self.phase = Phase::Idle;
        self.last = None;
        self.power = None;
        self.access = None;
        self.queued_power = None;
        self.last_scan = None;
    }
    pub fn open(&mut self, wifi: bool, hwnd: HWND, message: u32) {
        self.close();
        self.wifi = Some(wifi);
        self.on = false;
        self.confirmed_on = false;
        self.read = false;
        self.networks = None;
        self.devices.clear();
        self.error = None;
        self.start(Job::Scan, hwnd, message);
    }
    pub fn request_power(&mut self, on: bool, hwnd: HWND, message: u32) {
        // Queue behind an in-flight read; its stale snapshot must not undo the target.
        if self.power.is_some() {
            return;
        }
        // Initiate consent on the foreground UI thread; only the worker waits.
        let access = match crate::commands::radio::request_access() {
            Ok(access) => access,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        self.access = Some(access);
        self.power = Some(on);
        self.queued_power = Some(on);
        self.on = on;
        self.error = None;
        self.password.fill(0);
        self.password.clear();
        self.phase = Phase::Idle;
        if !on {
            self.networks = None;
            self.devices.clear();
        }
        self.refresh(hwnd, message);
    }
    pub fn refresh(&mut self, hwnd: HWND, message: u32) {
        if self.pending.is_some() {
            return;
        }
        if let Some(on) = self.queued_power.take() {
            if let Some(access) = self.access.take() {
                self.start(Job::Power(on, access), hwnd, message);
            }
        } else if self.wifi.is_some() && self.last.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
            let scan = self.last_scan.is_none_or(|t| t.elapsed() >= Duration::from_secs(10));
            self.start(if scan { Job::Scan } else { Job::Read }, hwnd, message);
        }
    }
    pub fn start(&mut self, job: Job, hwnd: HWND, message: u32) {
        if self.pending.is_some() {
            return;
        }
        let Some(wifi_view) = self.wifi else { return };
        if matches!(job, Job::Scan | Job::Power(true, _)) {
            self.last_scan = Some(Instant::now());
        }
        let (tx, rx) = mpsc::channel();
        self.active = Arc::new(AtomicBool::new(true));
        let active = self.active.clone();
        let window = hwnd.0 as usize;
        let was_on = self.confirmed_on;
        self.pending = Some(rx);
        self.last = Some(Instant::now());
        let spawn = std::thread::Builder::new().name("now-radios".into()).spawn(move || {
            if !active.load(Ordering::Acquire) {
                return;
            }
            let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED).is_ok() };
            let mut reading = Reading {
                on: false,
                networks: None,
                devices: Vec::new(),
                result: Err("Windows could not read the radio.".into()),
            };
            if initialized {
                let scan = matches!(&job, Job::Scan | Job::Power(true, _)) || !was_on;
                reading.result = match job {
                    Job::Scan | Job::Read => Ok(()),
                    Job::Power(on, access) => {
                        use windows::Devices::Radios::RadioKind;
                        crate::commands::radio::set_with_access(
                            if wifi_view {
                                RadioKind::WiFi
                            } else {
                                RadioKind::Bluetooth
                            },
                            if wifi_view { "Wi-Fi" } else { "Bluetooth" },
                            on,
                            access,
                        )
                    }
                    Job::Network(network, mut password) => {
                        let result = if network.connected {
                            wifi::disconnect()
                        } else {
                            wifi::join(&network, password.as_deref())
                        };
                        // Release credentials before enumerating devices or posting a result.
                        password.take();
                        result
                    }
                    Job::Audio(id, on) => bluetooth_audio::set(&id, on),
                };
                if active.load(Ordering::Acquire) {
                    if wifi_view {
                        reading.on = wifi::state().is_some_and(|s| s.on);
                        if reading.on && active.load(Ordering::Acquire) {
                            match if scan {
                                wifi::networks()
                            } else {
                                wifi::cached_networks()
                            } {
                                Ok(list) => reading.networks = Some(list),
                                Err(error) => reading.result = Err(error),
                            }
                        }
                    } else {
                        reading.on = crate::commands::radio_on_bluetooth();
                        if reading.on && active.load(Ordering::Acquire) {
                            reading.devices = bluetooth::devices();
                            if active.load(Ordering::Acquire) {
                                bluetooth_audio::identify(&mut reading.devices);
                                reading.devices = bluetooth::model(std::mem::take(&mut reading.devices));
                            }
                        }
                    }
                }
                unsafe { RoUninitialize() };
            }
            if active.load(Ordering::Acquire) && tx.send(reading).is_ok() {
                unsafe {
                    let _ = PostMessageW(Some(HWND(window as _)), message, WPARAM(0), LPARAM(0));
                }
            }
        });
        if spawn.is_err() {
            self.pending = None;
            self.error = Some("Could not start the radio worker.".into());
            self.phase = Phase::Idle;
            self.fail_power();
        }
    }
    fn fail_power(&mut self) {
        self.power = None;
        self.access = None;
        self.queued_power = None;
        self.on = self.confirmed_on;
    }
    pub fn take(&mut self) {
        let Some(rx) = &self.pending else { return };
        let reading = match rx.try_recv() {
            Ok(reading) => reading,
            Err(mpsc::TryRecvError::Empty) => {
                if self.last.is_some_and(|t| t.elapsed() > Duration::from_secs(30)) {
                    self.error = Some("Windows did not respond. Reopen this view to retry.".into());
                    self.phase.finish(Some("Windows did not respond in time."));
                    if self.power.is_some() {
                        self.on = self.confirmed_on;
                    }
                }
                return;
            }
            Err(_) => {
                self.pending = None;
                self.phase = Phase::Idle;
                self.error = Some("Radio worker stopped. Try again.".into());
                self.fail_power();
                return;
            }
        };
        self.pending = None;
        self.last = Some(Instant::now());
        self.confirmed_on = reading.on;
        self.read = true;
        if self.queued_power.is_some() {
            return;
        }
        self.power = None;
        self.on = reading.on;
        if !self.on {
            self.phase = Phase::Idle;
            self.password.fill(0);
            self.password.clear();
        }
        self.networks = reading.networks;
        self.devices = reading.devices;
        match reading.result {
            Ok(()) => {
                self.error = None;
                self.phase.finish(None);
            }
            Err(error) => {
                self.phase.finish(Some(&error));
                self.error = Some(error);
            }
        }
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn widgets(&self, filter: &str) -> Vec<Widget> {
        let wifi_view = self.wifi == Some(true);
        let title = if wifi_view { "Wi-Fi" } else { "Bluetooth" };
        let glyph = if wifi_view { glyph::WIFI } else { glyph::BLUETOOTH };
        let mut widgets = vec![Widget::new(
            title,
            if self.power.is_some() {
                "Changing\u{2026}"
            } else if !self.read {
                "Checking…"
            } else if self.on {
                "On"
            } else {
                "Off"
            },
            glyph,
            Input::Toggle {
                on: self.on,
                command: if wifi_view {
                    Command::WifiPower(!self.on)
                } else {
                    Command::BluetoothPower(!self.on)
                },
            },
        )];
        if self.on && wifi_view {
            if let Some(networks) = &self.networks {
                for n in &networks.list {
                    let bars = match n.signal {
                        Some(0..=24) => "▂",
                        Some(25..=49) => "▂▄",
                        Some(50..=74) => "▂▄▆",
                        Some(_) => "▂▄▆█",
                        None => "",
                    };
                    let state = match &self.phase {
                        Phase::Password(id) if id == &n.name => String::new(),
                        Phase::Pending(id) if id == &n.name => {
                            if n.connected {
                                "Disconnecting…".into()
                            } else {
                                "Connecting…".into()
                            }
                        }
                        Phase::Failed(id, _) if id == &n.name => "Failed · Select to try again".into(),
                        _ if n.connected => "Connected".into(),
                        _ if n.profile.is_some() => "Saved".into(),
                        _ => String::new(),
                    };
                    widgets.push(row(
                        &n.name,
                        format!("{bars} {} {state}", if n.secured { "🔒" } else { "" }),
                        glyph::WIFI,
                        Command::RadioSelect(n.name.clone()),
                    ));
                }
                if networks.location_off {
                    widgets.push(link(
                        "Location is off \u{b7} Turn on",
                        "",
                        "ms-settings:privacy-location",
                    ));
                } else if networks.list.is_empty() {
                    widgets.push(row(
                        "No networks found",
                        "Try again in a moment".into(),
                        glyph,
                        Command::RadioSelect("refresh".into()),
                    ));
                }
            }
        } else if self.on {
            for d in &self.devices {
                let state = match &self.phase {
                    Phase::Pending(id) if id == &d.id => {
                        if d.connected {
                            "Disconnecting…"
                        } else {
                            "Connecting…"
                        }
                    }
                    Phase::Failed(id, _) if id == &d.id => "Failed · Use Bluetooth settings",
                    _ if d.audio_id.is_none() => {
                        if d.connected {
                            "Connected · Opens Bluetooth settings"
                        } else {
                            "Connect in Bluetooth settings"
                        }
                    }
                    _ if d.connected => "Connected",
                    _ => "Paired",
                };
                let battery = d.battery.map(|b| format!(" · {b}%")).unwrap_or_default();
                widgets.push(row(
                    &d.name,
                    format!("{state}{battery}"),
                    if d.audio_id.is_some() {
                        glyph::SPEAKER
                    } else {
                        glyph::BLUETOOTH
                    },
                    Command::RadioSelect(d.id.clone()),
                ));
            }
            if self.devices.is_empty() && !self.busy() {
                widgets.push(Widget::new(
                    "No paired devices",
                    "Add a device below",
                    glyph,
                    Input::Info { value: String::new() },
                ));
            }
        }
        if let Some(error) = &self.error {
            widgets.push(link(
                "Open settings",
                error,
                if wifi_view {
                    "ms-settings:network-wifi"
                } else {
                    "ms-settings:bluetooth"
                },
            ));
        }
        if !wifi_view {
            widgets.push(link("Add device", "", "ms-settings:bluetooth"));
        }
        for (i, widget) in widgets.iter_mut().enumerate() {
            widget.wide = true;
            widget.quiet = true;
            if matches!(&self.phase, Phase::Password(name) if name == &widget.label) {
                widget.editor = Some(format!(
                    "Password  {}  \u{b7}  Enter to connect \u{b7} Esc to cancel",
                    "\u{2022}".repeat(self.password.len())
                ));
            }
            widget.id = if i == 0 {
                "radio:power".into()
            } else {
                format!("radio:{}", widget.label)
            };
        }
        let filter = filter.trim().to_lowercase();
        widgets.retain(|w| filter.is_empty() || w.label.to_lowercase().contains(&filter) || w.id == "radio:power");
        widgets
    }
}
impl Drop for Panel {
    fn drop(&mut self) {
        self.close();
    }
}
fn row(label: &str, detail: String, glyph: &'static str, command: Command) -> Widget {
    Widget::new(
        label,
        detail,
        glyph,
        Input::Link {
            action: Action::Command(command),
        },
    )
}
fn link(label: &str, detail: &str, target: &str) -> Widget {
    let mut widget = Widget::new(
        label,
        detail,
        glyph::OPEN,
        Input::Link {
            action: Action::Open(target.into()),
        },
    );
    widget.footer = true;
    widget
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]
    use super::*;
    fn network() -> wifi::Network {
        wifi::Network {
            name: "Home".into(),
            profile: None,
            connected: false,
            secured: true,
            signal: Some(80),
        }
    }
    fn deliver(panel: &mut Panel, on: bool, result: Result<(), String>) {
        let (tx, rx) = mpsc::channel();
        panel.pending = Some(rx);
        tx.send(Reading {
            on,
            networks: None,
            devices: Vec::new(),
            result,
        })
        .unwrap();
        panel.take();
    }
    #[test]
    fn both_headers_are_first_focus_target_and_keep_target_while_pending() {
        for wifi in [true, false] {
            let mut panel = Panel::default();
            panel.wifi = Some(wifi);
            panel.on = true;
            panel.power = Some(true);
            let widgets = panel.widgets("unmatched filter");
            assert_eq!(super::super::first_control(&widgets), 0);
            assert_eq!(widgets[0].id, "radio:power");
            assert!(widgets[0].quiet && widgets[0].wide);
            assert_eq!(widgets[0].detail, "Changing\u{2026}");
            assert!(matches!(widgets[0].input, Input::Toggle { on: true, .. }));
            assert_eq!(super::super::nav::layout(&widgets, 664.0)[0].h, 44.0);
        }
    }
    #[test]
    fn stale_read_keeps_target_until_power_finishes() {
        let mut panel = Panel::default();
        panel.on = true;
        panel.power = Some(true);
        panel.queued_power = Some(true);
        deliver(&mut panel, false, Ok(()));
        assert!(panel.on);
        assert_eq!(panel.power, Some(true));
        panel.queued_power = None;
        deliver(&mut panel, true, Ok(()));
        assert!(panel.on);
        assert_eq!(panel.power, None);
    }
    #[test]
    fn failed_power_reconciles_and_external_off_clears_credentials() {
        let mut panel = Panel::default();
        panel.on = true;
        panel.power = Some(true);
        deliver(&mut panel, false, Err("Denied".into()));
        assert!(!panel.on);
        assert_eq!(panel.error.as_deref(), Some("Denied"));
        panel.phase = Phase::Password("Home".into());
        panel.password = vec![65];
        deliver(&mut panel, false, Ok(()));
        assert!(panel.password.is_empty());
        assert_eq!(panel.phase, Phase::Idle);
    }
    #[test]
    fn new_secured_network_requires_password_and_pending_blocks_selection() {
        let mut phase = Phase::Idle;
        assert!(!phase.select(&network()));
        assert_eq!(phase, Phase::Password("Home".into()));
        let mut n = network();
        n.profile = Some("Home".into());
        assert!(phase.select(&n));
        assert_eq!(phase, Phase::Pending("Home".into()));
        assert!(!phase.select(&n));
    }
    #[test]
    fn open_and_connected_networks_need_no_password() {
        let mut phase = Phase::Idle;
        let mut n = network();
        n.secured = false;
        assert!(phase.select(&n));
        phase = Phase::Idle;
        n.secured = true;
        n.connected = true;
        assert!(phase.select(&n));
    }
    #[test]
    fn password_submit_cancel_and_failed_retry() {
        let mut phase = Phase::Idle;
        phase.select(&network());
        assert_eq!(phase.submit(false), None);
        phase.cancel();
        assert_eq!(phase, Phase::Idle);
        phase.select(&network());
        assert_eq!(phase.submit(true), Some("Home".into()));
        phase.finish(Some("Wrong password"));
        assert!(matches!(phase, Phase::Failed(_, _)));
        let mut saved = network();
        saved.profile = Some("Home".into());
        assert!(!phase.select(&saved));
        phase.submit(true);
        phase.finish(None);
        assert_eq!(phase, Phase::Idle);
    }
    #[test]
    fn password_is_masked_and_cleared_on_close() {
        let mut panel = Panel {
            wifi: Some(true),
            on: true,
            networks: Some(wifi::Networks {
                list: vec![network()],
                location_off: false,
            }),
            phase: Phase::Password("Home".into()),
            password: "secret".encode_utf16().collect(),
            devices: Vec::new(),
            pending: None,
            active: Arc::default(),
            last: None,
            error: None,
            power: None,
            queued_power: None,
            last_scan: None,
            confirmed_on: false,
            read: false,
            access: None,
        };
        let rows = panel.widgets("");
        assert!(rows[1].editor.as_ref().unwrap().contains("••••••"));
        assert!(!rows[1].editor.as_ref().unwrap().contains("secret"));
        panel.close();
        assert!(panel.password.is_empty());
        assert_eq!(panel.phase, Phase::Idle);
    }
}
