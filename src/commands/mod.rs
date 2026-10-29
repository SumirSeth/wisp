//! Built-in commands that read or change Windows state: Wi-Fi, Bluetooth,
//! volume, media, open windows, power, the time and Settings pages.
//!
//! Windows calls run on a worker thread, so typing never waits for them. The
//! launcher keeps the latest state the worker reported and builds results
//! from it; the worker posts a message when new state arrives.

pub mod audio;
pub mod bluetooth;
pub mod media;
pub mod open_windows;
pub mod power;
mod radio;
pub mod settings;
pub mod time;
pub mod wifi;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::HSTRING;

use crate::indexer::{self, IconPixels};
use crate::search::{Action, Drill, Kind, ResultItem};
use media::Control;
use power::Power;

const WIFI_SETTINGS: &str = "ms-settings:network-wifi";
const BLUETOOTH_SETTINGS: &str = "ms-settings:bluetooth";
const SOUND_SETTINGS: &str = "ms-settings:sound";
/// Opens the Windows network flyout, which asks for a new network's password.
const NETWORK_FLYOUT: &str = "ms-availablenetworks:";
const LOCATION_SETTINGS: &str = "ms-settings:privacy-location";
const WIFI_KEYWORDS: [&str; 4] = ["wifi", "wi-fi", "wireless", "wlan"];
/// A query that starts with this lists open windows.
const WINDOW_PREFIX: &str = "w ";
const MAX_WINDOWS: usize = 30;

pub mod glyph {
    pub const WIFI: &str = "\u{E701}";
    pub const BLUETOOTH: &str = "\u{E702}";
    pub const VOLUME: &str = "\u{E767}";
    pub const MUTED: &str = "\u{E74F}";
    pub const SPEAKER: &str = "\u{E7F5}";
    pub const PLAY: &str = "\u{E768}";
    pub const PAUSE: &str = "\u{E769}";
    pub const NEXT: &str = "\u{E893}";
    pub const PREVIOUS: &str = "\u{E892}";
    pub const CLOCK: &str = "\u{E823}";
    pub const CALENDAR: &str = "\u{E787}";
    pub const WINDOW: &str = "\u{E737}";
    pub const CLOSE: &str = "\u{E711}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const CHECK: &str = "\u{E73E}";
    pub const LOCK: &str = "\u{E72E}";
    pub const OPEN: &str = "\u{E785}";
    pub const LOCATION: &str = "\u{E81D}";
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    WifiPower(bool),
    WifiConnect(String),
    BluetoothPower(bool),
    Volume(u32),
    Mute(bool),
    /// Makes the output device with this ID the default.
    Output(String),
    Media(Control),
    FocusWindow(isize),
    CloseWindow(isize),
    Power(Power),
}

impl Command {
    /// Toggles and adjustments keep the launcher open and show the new state.
    pub fn stays_open(&self) -> bool {
        !matches!(
            self,
            Command::WifiConnect(_) | Command::FocusWindow(_) | Command::Power(_)
        )
    }
}

/// The latest state the worker reported. `None` until the first check, and
/// `Some(None)` if the PC lacks the device (or, for media, nothing plays).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub wifi: Option<Option<wifi::State>>,
    pub bluetooth: Option<Option<bluetooth::State>>,
    pub audio: Option<Option<audio::State>>,
    pub media: Option<Option<media::State>>,
    pub windows: Vec<open_windows::Window>,
}

pub enum Event {
    State(Box<State>, Vec<(String, Arc<IconPixels>)>),
    /// The items of a list that Tab opened.
    List(Drill, Vec<ResultItem>),
    Failed(String),
}

enum Request {
    Refresh,
    List(Drill),
    Run(Command),
}

pub struct Commands {
    requests: Sender<Request>,
}

impl Commands {
    pub fn spawn(hwnd: HWND, notify_msg: u32, icon_size: i32) -> (Commands, Receiver<Event>) {
        let (requests, rx) = channel();
        let (events, events_rx) = channel();
        let hwnd = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("commands".into())
            .spawn(move || {
                worker(rx, events, icon_size, move || unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as _)), notify_msg, WPARAM(0), LPARAM(0));
                })
            })
            .expect("could not start commands thread");
        (Commands { requests }, events_rx)
    }

    /// Reads the state again. The launcher calls this each time it opens.
    pub fn refresh(&self) {
        let _ = self.requests.send(Request::Refresh);
    }

    pub fn list(&self, drill: Drill) {
        let _ = self.requests.send(Request::List(drill));
    }

    /// Runs a command on the worker thread, then reads the state again.
    pub fn run(&self, command: Command) {
        let _ = self.requests.send(Request::Run(command));
    }
}

fn worker(requests: Receiver<Request>, events: Sender<Event>, icon_size: i32, notify: impl Fn()) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let send = |event| {
        if events.send(event).is_ok() {
            notify();
        }
    };
    let mut icons_sent = HashSet::new();
    let mut refresh = || {
        let state = read_state();
        let icons = state
            .windows
            .iter()
            .filter_map(|window| Some((window.icon_key()?, window.exe.clone())))
            .filter(|(key, _)| icons_sent.insert(key.clone()))
            .filter_map(|(key, exe)| Some((key, Arc::new(exe_icon(&exe, icon_size)?))))
            .collect();
        send(Event::State(Box::new(state), icons));
    };
    while let Ok(request) = requests.recv() {
        match request {
            Request::Refresh => refresh(),
            Request::List(drill) => send(Event::List(drill, list_items(drill))),
            Request::Run(command) => {
                let result = match &command {
                    Command::WifiPower(on) => wifi::set_power(*on),
                    Command::WifiConnect(profile) => wifi::connect(profile),
                    Command::BluetoothPower(on) => bluetooth::set_power(*on),
                    Command::Volume(percent) => audio::set_volume(*percent),
                    Command::Mute(muted) => audio::set_mute(*muted),
                    Command::Output(id) => audio::set_output(id),
                    Command::Media(control) => media::control(*control),
                    Command::FocusWindow(hwnd) => open_windows::focus(*hwnd),
                    Command::CloseWindow(hwnd) => open_windows::close(*hwnd),
                    Command::Power(power) => power::run(*power),
                };
                if let Err(error) = result {
                    send(Event::Failed(error));
                }
                // Apps update their media state and close windows a moment later.
                if matches!(command, Command::Media(_) | Command::CloseWindow(_)) {
                    std::thread::sleep(Duration::from_millis(300));
                }
                refresh();
            }
        }
    }
}

fn read_state() -> State {
    State {
        wifi: Some(wifi::state()),
        bluetooth: Some(bluetooth::state()),
        audio: Some(audio::state()),
        media: Some(media::state()),
        windows: open_windows::list(),
    }
}

fn exe_icon(exe: &str, size: i32) -> Option<IconPixels> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(exe), None).ok()?;
        indexer::load_icon(&item, size)
    }
}

fn row(title: impl Into<String>, subtitle: impl Into<String>, action: Action) -> ResultItem {
    ResultItem::new(Kind::Command, title, subtitle, action)
}

fn open(title: &str, link: &str) -> ResultItem {
    row(title, "Windows Settings", Action::Open(link.into()))
        .glyph(glyph::SETTINGS)
        .verb("Open")
}

/// Whether the query lists open windows instead of searching.
pub fn is_window_query(query: &str) -> bool {
    query.trim_start().to_lowercase().starts_with(WINDOW_PREFIX)
}

/// Command results for `query`, shown above apps.
pub fn search(query: &str, state: &State) -> Vec<ResultItem> {
    // "w" alone is the start of many queries; only "w " lists windows.
    if let Some(filter) = query.trim_start().to_lowercase().strip_prefix(WINDOW_PREFIX) {
        return window_items(filter.trim(), &state.windows);
    }
    let query = query.trim().to_lowercase();
    if query.is_empty() || query.starts_with(['>', '?']) {
        return Vec::new();
    }
    let words: Vec<&str> = query.split_whitespace().collect();
    let mut items = Vec::new();
    items.extend(wifi_items(&words, state.wifi.as_ref()));
    items.extend(bluetooth_items(&words, state.bluetooth.as_ref()));
    items.extend(volume_items(&words, state.audio.as_ref()));
    items.extend(media_items(&words, state.media.as_ref()));
    items.extend(power_items(&query));
    items.extend(time_items(&words));
    for (title, link) in settings::find(&query) {
        let duplicate = items
            .iter()
            .any(|item| matches!(&item.action, Action::Open(l) if l == link));
        if !duplicate {
            items.push(open(title, link));
        }
    }
    items
}

/// Splits `[name, on|off]`. `None` if the query has other words.
fn toggle_target<'a>(words: &[&'a str]) -> Option<(&'a str, Option<bool>)> {
    match words {
        [name] => Some((name, None)),
        [name, "on"] => Some((name, Some(true))),
        [name, "off"] => Some((name, Some(false))),
        _ => None,
    }
}

/// A row for a radio such as Wi-Fi: its state, toggled by Enter.
/// `on` and `detail` come from the state; `None` while it is unknown.
fn radio_item(
    name: &str,
    state: Option<Option<(bool, String)>>,
    target: Option<bool>,
    power: fn(bool) -> Command,
    drill: Drill,
    settings: &str,
) -> ResultItem {
    match state {
        None => row(name, "Checking…", Action::Open(settings.into())).verb("Open"),
        Some(None) => row(
            name,
            format!("This PC has no {name} adapter"),
            Action::Open(settings.into()),
        )
        .verb("Open"),
        Some(Some((on, detail))) => match target {
            Some(target) => {
                let title = format!("Turn {name} {}", if target { "on" } else { "off" });
                let subtitle = if on == target {
                    format!("{name} is already {}", if on { "on" } else { "off" })
                } else if detail.is_empty() {
                    format!("{name} is off")
                } else {
                    detail
                };
                row(title, subtitle, Action::Command(power(target))).verb(if target { "Turn on" } else { "Turn off" })
            }
            None => {
                let word = if on { "On" } else { "Off" };
                let mut item = row(
                    format!("{name} · {word}"),
                    if detail.is_empty() {
                        format!("Tab for {}", drill.title().to_lowercase())
                    } else {
                        format!("{detail} · Tab for {}", drill.title().to_lowercase())
                    },
                    Action::Command(power(!on)),
                )
                .verb(if on { "Turn off" } else { "Turn on" });
                if on {
                    let start = format!("{name} · ").encode_utf16().count() as u32;
                    item.highlights = vec![(start, 2)];
                }
                item.drill = Some(drill);
                item
            }
        },
    }
}

fn wifi_items(words: &[&str], state: Option<&Option<wifi::State>>) -> Vec<ResultItem> {
    let Some((name, target)) = toggle_target(words) else {
        return Vec::new();
    };
    if name.chars().count() < 2 || !WIFI_KEYWORDS.iter().any(|keyword| keyword.starts_with(name)) {
        return Vec::new();
    }
    let state = state.map(|s| {
        s.as_ref().map(|s| {
            let detail = match &s.network {
                Some(network) => format!("Connected to {network}"),
                None if s.on => "Not connected".into(),
                None => String::new(),
            };
            (s.on, detail)
        })
    });
    vec![
        radio_item(
            "Wi-Fi",
            state,
            target,
            Command::WifiPower,
            Drill::WifiNetworks,
            WIFI_SETTINGS,
        )
        .glyph(glyph::WIFI),
        open("Wi-Fi settings", WIFI_SETTINGS),
    ]
}

fn bluetooth_items(words: &[&str], state: Option<&Option<bluetooth::State>>) -> Vec<ResultItem> {
    let Some((name, target)) = toggle_target(words) else {
        return Vec::new();
    };
    if name != "bt" && !(name.len() >= 3 && "bluetooth".starts_with(name)) {
        return Vec::new();
    }
    let state = state.map(|s| {
        s.as_ref().map(|s| {
            let detail = match s.connected.as_slice() {
                _ if !s.on => String::new(),
                [] => "No devices connected".into(),
                [one] => format!("Connected to {one}"),
                [first, rest @ ..] => format!("Connected to {first} and {} more", rest.len()),
            };
            (s.on, detail)
        })
    });
    vec![
        radio_item(
            "Bluetooth",
            state,
            target,
            Command::BluetoothPower,
            Drill::BluetoothDevices,
            BLUETOOTH_SETTINGS,
        )
        .glyph(glyph::BLUETOOTH),
        open("Bluetooth settings", BLUETOOTH_SETTINGS),
    ]
}

fn volume_items(words: &[&str], state: Option<&Option<audio::State>>) -> Vec<ResultItem> {
    let (name, argument) = match words {
        [name] => (*name, None),
        [name, argument] => (*name, Some(*argument)),
        _ => return Vec::new(),
    };
    let Some(Some(audio)) = state else {
        return Vec::new();
    };
    let now = if audio.muted {
        format!("Muted · {}", audio.device)
    } else {
        format!("Now {}% · {}", audio.volume, audio.device)
    };
    let set = |percent: u32| {
        let percent = percent.min(100);
        row(
            format!("Set volume to {percent}%"),
            now.clone(),
            Action::Command(Command::Volume(percent)),
        )
        .glyph(glyph::VOLUME)
        .verb("Set")
    };
    let mute = |muted: bool| {
        let (title, verb, glyph) = if muted {
            ("Mute", "Mute", glyph::MUTED)
        } else {
            ("Unmute", "Unmute", glyph::VOLUME)
        };
        row(title, now.clone(), Action::Command(Command::Mute(muted)))
            .glyph(glyph)
            .verb(verb)
    };
    match (name, argument) {
        ("mute", None) => return vec![mute(true)],
        ("unmute", None) => return vec![mute(false)],
        _ => {}
    }
    if !(name == "vol" || (name.len() >= 3 && "volume".starts_with(name))) {
        return Vec::new();
    }
    match argument {
        None => {
            let title = if audio.muted {
                "Volume · Muted".to_string()
            } else {
                format!("Volume · {}%", audio.volume)
            };
            let mut item = row(
                title,
                format!("{} · Tab for output devices", audio.device),
                Action::Command(Command::Mute(!audio.muted)),
            )
            .glyph(if audio.muted { glyph::MUTED } else { glyph::VOLUME })
            .verb(if audio.muted { "Unmute" } else { "Mute" });
            item.drill = Some(Drill::AudioOutputs);
            vec![item, open("Sound settings", SOUND_SETTINGS)]
        }
        Some("up") => vec![set(audio.volume + 10)],
        Some("down") => vec![set(audio.volume.saturating_sub(10))],
        Some("mute") => vec![mute(true)],
        Some("unmute") => vec![mute(false)],
        Some(value) => {
            let value = value.trim_end_matches('%');
            if let Some(step) = value.strip_prefix('+').and_then(|v| v.parse::<u32>().ok()) {
                vec![set(audio.volume + step)]
            } else if let Some(step) = value.strip_prefix('-').and_then(|v| v.parse::<u32>().ok()) {
                vec![set(audio.volume.saturating_sub(step))]
            } else if let Ok(percent) = value.parse::<u32>() {
                vec![set(percent)]
            } else {
                Vec::new()
            }
        }
    }
}

fn media_items(words: &[&str], state: Option<&Option<media::State>>) -> Vec<ResultItem> {
    let [name] = words else {
        return Vec::new();
    };
    let Some(Some(media)) = state else {
        // Nothing to control. Only "media" and "music" say so.
        return if matches!(*name, "media" | "music") {
            vec![row("Nothing is playing", "", Action::None).glyph(glyph::PLAY).verb("")]
        } else {
            Vec::new()
        };
    };
    let track = [media.title.as_str(), media.artist.as_str(), media.app.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let control = |title: &str, control: Control, glyph: &'static str, verb: &'static str| {
        row(title, track.clone(), Action::Command(Command::Media(control)))
            .glyph(glyph)
            .verb(verb)
    };
    match *name {
        "play" | "resume" => vec![control("Play", Control::Play, glyph::PLAY, "Play")],
        "pause" => vec![control("Pause", Control::Pause, glyph::PAUSE, "Pause")],
        "next" | "skip" => vec![control("Next track", Control::Next, glyph::NEXT, "Next")],
        "previous" | "prev" => vec![control(
            "Previous track",
            Control::Previous,
            glyph::PREVIOUS,
            "Previous",
        )],
        "media" | "music" => {
            let status = if media.playing { "Playing" } else { "Paused" };
            vec![
                row(
                    if media.title.is_empty() {
                        status
                    } else {
                        media.title.as_str()
                    },
                    [media.artist.as_str(), media.app.as_str(), status]
                        .into_iter()
                        .filter(|p| !p.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · "),
                    Action::Command(Command::Media(Control::PlayPause)),
                )
                .glyph(if media.playing { glyph::PAUSE } else { glyph::PLAY })
                .verb(if media.playing { "Pause" } else { "Play" }),
                control("Next track", Control::Next, glyph::NEXT, "Next"),
                control("Previous track", Control::Previous, glyph::PREVIOUS, "Previous"),
            ]
        }
        _ => Vec::new(),
    }
}

fn power_items(query: &str) -> Vec<ResultItem> {
    if query.chars().count() < 3 {
        return Vec::new();
    }
    Power::ALL
        .into_iter()
        .filter(|p| p.keywords().iter().any(|keyword| keyword.starts_with(query)))
        .map(|p| {
            let subtitle = if p.needs_confirmation() {
                format!("{} · Press Enter twice", p.description())
            } else {
                p.description().to_string()
            };
            row(p.title(), subtitle, Action::Command(Command::Power(p)))
                .glyph(p.glyph())
                .verb(p.title())
        })
        .collect()
}

/// `time`, `date`, `time tokyo`, `time in tokyo` and `tokyo time`.
fn time_items(words: &[&str]) -> Vec<ResultItem> {
    let place = match words {
        ["time"] | ["now"] => {
            let now = time::local();
            return vec![
                row(
                    format!("{} · {}", now.time(), now.short_date()),
                    "Local time · Enter to copy",
                    Action::Copy(now.time()),
                )
                .glyph(glyph::CLOCK)
                .verb("Copy"),
            ];
        }
        ["date"] | ["today"] => {
            let date = time::local().long_date();
            return vec![
                row(date.clone(), "Today · Enter to copy", Action::Copy(date))
                    .glyph(glyph::CALENDAR)
                    .verb("Copy"),
            ];
        }
        ["time", "in", place @ ..] | ["time", place @ ..] => place.join(" "),
        [place @ .., "time"] if !place.is_empty() => place.join(" "),
        _ => return Vec::new(),
    };
    time::find(&place)
        .into_iter()
        .filter_map(|(zone, city)| {
            let clock = time::in_zone(zone)?;
            Some(
                row(
                    format!("{} · {}", clock.time(), clock.short_date()),
                    format!("{city} · {}", clock.difference()),
                    Action::Copy(clock.time()),
                )
                .glyph(glyph::CLOCK)
                .verb("Copy"),
            )
        })
        .collect()
}

fn window_items(filter: &str, windows: &[open_windows::Window]) -> Vec<ResultItem> {
    let words: Vec<&str> = filter.split_whitespace().collect();
    windows
        .iter()
        .filter(|w| {
            let text = format!("{} {}", w.title, open_windows::program_name(&w.exe)).to_lowercase();
            words.iter().all(|word| text.contains(word))
        })
        .take(MAX_WINDOWS)
        .map(|w| {
            let mut item = row(
                w.title.clone(),
                open_windows::program_name(&w.exe),
                Action::Command(Command::FocusWindow(w.hwnd)),
            )
            .glyph(glyph::WINDOW)
            .verb("Switch");
            item.icon = w.icon_key();
            item
        })
        .collect()
}

/// The row shown while a list loads.
pub fn loading(drill: Drill) -> ResultItem {
    let title = match drill {
        Drill::WifiNetworks => "Looking for networks…",
        Drill::BluetoothDevices => "Looking for devices…",
        Drill::AudioOutputs => "Looking for output devices…",
    };
    row(title, "", Action::None).glyph(drill.glyph()).verb("")
}

/// Builds a list that Tab opened. Runs on the worker thread.
fn list_items(drill: Drill) -> Vec<ResultItem> {
    match drill {
        Drill::WifiNetworks => network_items(Some(&wifi::state()), Some(&wifi::networks())),
        Drill::BluetoothDevices => device_items(bluetooth::state()),
        Drill::AudioOutputs => output_items(audio::outputs()),
    }
}

/// Rows for the Wi-Fi network list. `networks` is `None` while loading.
pub fn network_items(
    state: Option<&Option<wifi::State>>,
    networks: Option<&Result<wifi::Networks, String>>,
) -> Vec<ResultItem> {
    if let Some(Some(state)) = state
        && !state.on
    {
        return vec![
            row(
                "Wi-Fi is off",
                "Turn it on to see networks",
                Action::Command(Command::WifiPower(true)),
            )
            .glyph(glyph::WIFI)
            .verb("Turn on"),
        ];
    }
    let networks = match networks {
        None => return vec![loading(Drill::WifiNetworks)],
        Some(Err(error)) => {
            return vec![
                row("Cannot list networks", error, Action::Open(WIFI_SETTINGS.into()))
                    .glyph(glyph::WIFI)
                    .verb("Open"),
            ];
        }
        Some(Ok(networks)) => networks,
    };
    let mut items: Vec<ResultItem> = networks
        .list
        .iter()
        .map(|network| {
            let mut details = vec![if network.connected {
                "Connected"
            } else if network.profile.is_some() {
                "Saved"
            } else {
                "New"
            }];
            // Without location access, saved networks have no security details.
            if !networks.location_off {
                details.push(if network.secured { "secured" } else { "open" });
            }
            let mut subtitle = details.join(" · ");
            if let Some(signal) = network.signal {
                subtitle.push_str(&format!(" · {signal}% signal"));
            }
            let (action, verb) = match &network.profile {
                _ if network.connected => (Action::Open(WIFI_SETTINGS.into()), "Settings"),
                Some(profile) => (Action::Command(Command::WifiConnect(profile.clone())), "Connect"),
                None => (Action::Open(NETWORK_FLYOUT.into()), "Connect"),
            };
            let glyph = if network.connected {
                glyph::CHECK
            } else if network.secured {
                glyph::LOCK
            } else {
                glyph::OPEN
            };
            row(network.name.clone(), subtitle, action).glyph(glyph).verb(verb)
        })
        .collect();
    if networks.location_off {
        items.push(
            row(
                "Show networks nearby",
                "Windows needs location access to list nearby Wi-Fi networks",
                Action::Open(LOCATION_SETTINGS.into()),
            )
            .glyph(glyph::LOCATION)
            .verb("Open"),
        );
    }
    items
}

/// Paired Bluetooth devices. Windows has no API to connect a paired device,
/// so Enter opens Bluetooth settings.
fn device_items(state: Option<bluetooth::State>) -> Vec<ResultItem> {
    match state {
        None => vec![open("This PC has no Bluetooth adapter", BLUETOOTH_SETTINGS)],
        Some(state) if !state.on => vec![
            row(
                "Bluetooth is off",
                "Turn it on to see devices",
                Action::Command(Command::BluetoothPower(true)),
            )
            .glyph(glyph::BLUETOOTH)
            .verb("Turn on"),
        ],
        Some(_) => {
            let devices = bluetooth::devices();
            if devices.is_empty() {
                return vec![open("No paired devices", BLUETOOTH_SETTINGS)];
            }
            devices
                .into_iter()
                .map(|device| {
                    let (subtitle, glyph) = if device.connected {
                        ("Connected · Enter for Bluetooth settings", glyph::CHECK)
                    } else {
                        ("Paired · Enter for Bluetooth settings", glyph::BLUETOOTH)
                    };
                    row(device.name, subtitle, Action::Open(BLUETOOTH_SETTINGS.into()))
                        .glyph(glyph)
                        .verb("Settings")
                })
                .collect()
        }
    }
}

fn output_items(outputs: Result<Vec<audio::Output>, String>) -> Vec<ResultItem> {
    match outputs {
        Err(error) => vec![open(&format!("Cannot list output devices: {error}"), SOUND_SETTINGS)],
        Ok(outputs) if outputs.is_empty() => vec![open("No output devices", SOUND_SETTINGS)],
        Ok(outputs) => outputs
            .into_iter()
            .map(|output| {
                if output.default {
                    row(output.name, "In use", Action::Open(SOUND_SETTINGS.into()))
                        .glyph(glyph::CHECK)
                        .verb("Settings")
                } else {
                    row(
                        output.name,
                        "Output device",
                        Action::Command(Command::Output(output.id)),
                    )
                    .glyph(glyph::SPEAKER)
                    .verb("Use")
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> State {
        State {
            wifi: Some(Some(wifi::State {
                on: true,
                network: Some("Home".into()),
            })),
            bluetooth: Some(Some(bluetooth::State {
                on: false,
                connected: Vec::new(),
            })),
            audio: Some(Some(audio::State {
                volume: 43,
                muted: false,
                device: "Speakers".into(),
            })),
            media: Some(Some(media::State {
                title: "Song".into(),
                artist: "Band".into(),
                app: "Spotify".into(),
                playing: true,
            })),
            windows: vec![open_windows::Window {
                hwnd: 1,
                title: "README.md - runner - Visual Studio Code".into(),
                exe: r"C:\Apps\Code.exe".into(),
            }],
        }
    }

    fn titles(query: &str, state: &State) -> Vec<String> {
        search(query, state).into_iter().map(|item| item.title).collect()
    }

    #[test]
    fn matches_wifi_keywords_and_on_off() {
        let state = ready();
        assert!(search("w", &state).is_empty());
        assert!(titles("note", &state).is_empty());
        assert!(titles("wifi please", &state).is_empty());
        for query in ["wi", "wifi", "Wi-Fi", "wireless", "wlan"] {
            assert_eq!(titles(query, &state)[0], "Wi-Fi · On", "{query}");
        }
        let off = &search("wifi off", &state)[0];
        assert_eq!(off.title, "Turn Wi-Fi off");
        assert!(matches!(off.action, Action::Command(Command::WifiPower(false))));
    }

    #[test]
    fn radio_rows_toggle_and_drill_into_lists() {
        let on = &search("wifi", &ready())[0];
        assert_eq!(on.subtitle, "Connected to Home · Tab for networks");
        assert_eq!(on.enter_label(), "Turn off");
        assert_eq!(on.drill, Some(Drill::WifiNetworks));
        assert_eq!(on.highlights, vec![(8, 2)]);
        assert_eq!(search("wifi", &State::default())[0].subtitle, "Checking…");
        let bt = &search("bt", &ready())[0];
        assert_eq!(bt.title, "Bluetooth · Off");
        assert!(matches!(bt.action, Action::Command(Command::BluetoothPower(true))));
        assert_eq!(titles("blu", &ready())[0], "Bluetooth · Off");
        assert!(titles("bl", &ready()).is_empty());
    }

    #[test]
    fn sets_volume_and_mutes() {
        let state = ready();
        assert_eq!(titles("vol", &state)[0], "Volume · 43%");
        assert_eq!(titles("vol 40", &state), ["Set volume to 40%"]);
        assert_eq!(titles("volume 140", &state), ["Set volume to 100%"]);
        assert_eq!(titles("vol up", &state), ["Set volume to 53%"]);
        assert_eq!(titles("vol -50", &state), ["Set volume to 0%"]);
        assert_eq!(titles("mute", &state), ["Mute"]);
        assert!(titles("vol loud", &state).is_empty());
    }

    #[test]
    fn controls_media_only_while_something_plays() {
        let state = ready();
        assert_eq!(titles("pause", &state), ["Pause"]);
        assert_eq!(search("next", &state)[0].subtitle, "Song · Band · Spotify");
        assert_eq!(titles("music", &state)[0], "Song");
        let idle = State {
            media: Some(None),
            ..ready()
        };
        assert!(titles("next", &idle).is_empty());
        assert_eq!(titles("music", &idle), ["Nothing is playing"]);
    }

    #[test]
    fn finds_power_commands_time_and_settings() {
        let state = ready();
        assert_eq!(titles("restart", &state), ["Restart"]);
        assert_eq!(titles("shut", &state), ["Shut down"]);
        assert!(titles("re", &state).is_empty());
        assert_eq!(search("lock", &state)[0].subtitle, "Locks the PC");
        assert_eq!(
            search("restart", &state)[0].subtitle,
            "Restarts the PC · Press Enter twice"
        );
        assert!(search("time", &state)[0].title.contains(" · "));
        assert_eq!(search("time tokyo", &state)[0].glyph, Some(glyph::CLOCK));
        assert_eq!(titles("display", &state), ["Display"]);
        // The Bluetooth command already links to its settings page, so the
        // matching Settings page is not listed again.
        assert_eq!(titles("bluetooth", &state), ["Bluetooth · Off", "Bluetooth settings"]);
    }

    #[test]
    fn lists_windows_after_w() {
        let state = ready();
        assert!(is_window_query("w code"));
        assert!(!is_window_query("wifi"));
        let items = search("w code", &state);
        assert_eq!(items[0].subtitle, "Code");
        assert_eq!(items[0].icon.as_deref(), Some(r"exe:c:\apps\code.exe"));
        assert!(search("w zzz", &state).is_empty());
        assert_eq!(search("w ", &state).len(), 1);
    }

    #[test]
    fn network_rows_connect_saved_networks() {
        let state = ready();
        let networks = Ok(wifi::Networks {
            list: vec![
                wifi::Network {
                    name: "Home".into(),
                    profile: Some("Home".into()),
                    connected: true,
                    secured: true,
                    signal: Some(80),
                },
                wifi::Network {
                    name: "Office".into(),
                    profile: Some("Office".into()),
                    connected: false,
                    secured: true,
                    signal: None,
                },
            ],
            location_off: true,
        });
        let items = network_items(state.wifi.as_ref(), Some(&networks));
        assert_eq!(items[0].subtitle, "Connected · 80% signal");
        assert_eq!(items[1].subtitle, "Saved");
        assert!(matches!(&items[1].action, Action::Command(Command::WifiConnect(p)) if p == "Office"));
        assert_eq!(items.last().unwrap().title, "Show networks nearby");
        assert_eq!(
            network_items(state.wifi.as_ref(), None)[0].title,
            "Looking for networks…"
        );
    }
}
