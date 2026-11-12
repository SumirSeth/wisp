//! Surfaces: small interfaces that Wisp composes for an intent, instead of a
//! list of results. "Sound" shows the volume, the output devices and each
//! app's volume; "Now" shows the time, battery, radios and what is playing.
//!
//! A surface is a spec: a title, keywords and a list of parts. Each part names
//! something Wisp can read and change, such as `volume` or `wifi`, never code.
//! Wisp resolves the parts against the latest state into widgets, so a surface
//! stays live, and a spec can only use actions that Wisp already has. Specs
//! are TOML; built-in ones ship in the executable, and users can add their own
//! in `%APPDATA%\wisp\surfaces`.

pub mod nav;
pub mod radios;

use std::path::PathBuf;

use serde::Deserialize;

use crate::commands::{Command, State, glyph, media, time};
use crate::search::{Action, Kind, ResultItem};
use crate::util;

const BUILT_IN: [(&str, &str); 4] = [
    ("wifi.toml", include_str!("wifi.toml")),
    ("bluetooth.toml", include_str!("bluetooth.toml")),
    ("now.toml", include_str!("now.toml")),
    ("sound.toml", include_str!("sound.toml")),
];
/// Shortest query that matches a surface's keywords.
const MIN_QUERY: usize = 3;
/// Volume change per Left or Right press on a slider.
pub const STEP: u32 = 5;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(rename = "part", default)]
    pub parts: Vec<Part>,
}

/// Something a surface shows. Each part resolves to one or more widgets.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Part {
    /// The local time, or a city's time with `city`.
    Clock {
        #[serde(default)]
        city: Option<String>,
    },
    Battery,
    Networks,
    Devices,
    Wifi,
    Bluetooth,
    /// The default output device's volume.
    Volume,
    /// Output devices to switch between.
    Outputs,
    /// One volume per app that plays sound.
    Apps,
    Media,
    /// Opens a file, folder, URL or `ms-settings:` page.
    Link {
        title: String,
        target: String,
        #[serde(default)]
        detail: String,
    },
}

/// What a slider changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Master,
    App(u32),
}

impl Target {
    pub fn command(self, percent: u32) -> Command {
        let percent = percent.min(100);
        match self {
            Target::Master => Command::Volume(percent),
            Target::App(pid) => Command::AppVolume { pid, percent },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// Shows a value, such as the time.
    Info {
        value: String,
    },
    Toggle {
        on: bool,
        command: Command,
    },
    /// 0 to 100.
    Slider {
        value: u32,
        muted: bool,
        target: Target,
    },
    /// One of several options; `selected` is the current one.
    Choice {
        options: Vec<(String, Command)>,
        selected: usize,
    },
    Buttons {
        buttons: Vec<(&'static str, Command)>,
    },
    Link {
        action: Action,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    /// Stays the same while the state changes, such as "app:1234" for an
    /// app's volume, so the focus stays on it.
    pub id: String,
    pub label: String,
    pub detail: String,
    pub glyph: &'static str,
    /// Key into the icon map, drawn instead of the glyph.
    pub icon: Option<String>,
    /// Takes the surface's full width instead of half.
    pub wide: bool,
    pub input: Input,
    pub quiet: bool,
    pub footer: bool,
    pub editor: Option<String>,
}

impl Widget {
    fn new(label: impl Into<String>, detail: impl Into<String>, glyph: &'static str, input: Input) -> Self {
        let wide = matches!(
            input,
            Input::Slider { .. } | Input::Choice { .. } | Input::Buttons { .. }
        );
        Widget {
            id: String::new(),
            label: label.into(),
            detail: detail.into(),
            glyph,
            icon: None,
            wide,
            input,
            quiet: false,
            footer: false,
            editor: None,
        }
    }

    /// Positions inside the widget that Left and Right move between.
    pub fn stops(&self) -> usize {
        match &self.input {
            Input::Choice { options, .. } => options.len(),
            Input::Buttons { buttons } => buttons.len(),
            _ => 1,
        }
    }

    /// Where the cursor starts when the widget gets focus.
    pub fn first_stop(&self) -> usize {
        match &self.input {
            Input::Choice { selected, .. } => *selected,
            Input::Buttons { buttons } => buttons.len() / 2,
            _ => 0,
        }
    }

    /// The command of the option or button at `stop`.
    pub fn command_at(&self, stop: usize) -> Option<&Command> {
        match &self.input {
            Input::Choice { options, .. } => options.get(stop).map(|(_, c)| c),
            Input::Buttons { buttons } => buttons.get(stop).map(|(_, c)| c),
            _ => None,
        }
    }

    /// Where the option or button with `command` is now.
    pub fn stop_of(&self, command: &Command) -> Option<usize> {
        (0..self.stops()).find(|&stop| self.command_at(stop) == Some(command))
    }

    /// The command or action Enter runs, with the cursor at `stop`.
    pub fn activate(&self, stop: usize) -> Option<Action> {
        match &self.input {
            Input::Info { .. } => None,
            Input::Toggle { command, .. } => Some(Action::Command(command.clone())),
            Input::Slider { muted, target, .. } => match target {
                Target::Master => Some(Action::Command(Command::Mute(!muted))),
                Target::App(_) => None,
            },
            Input::Choice { options, selected } => options
                .get(stop)
                .filter(|_| stop != *selected)
                .map(|(_, c)| Action::Command(c.clone())),
            Input::Buttons { buttons } => buttons.get(stop).map(|(_, c)| Action::Command(c.clone())),
            Input::Link { action } => Some(action.clone()),
        }
    }

    fn matches(&self, filter: &str) -> bool {
        filter.is_empty() || self.label.to_lowercase().contains(filter) || self.detail.to_lowercase().contains(filter)
    }
}

/// Built-in surfaces and the user's own.
pub struct Library {
    pub specs: Vec<Spec>,
    /// Problems with the user's files, as "name.toml: message".
    pub errors: Vec<String>,
}

impl Library {
    pub fn dir() -> PathBuf {
        util::data_dir().join("surfaces")
    }

    pub fn load() -> Library {
        // Built-ins never change. Parse once, while preserving fresh user-file reads.
        static BUILT_INS: std::sync::OnceLock<Vec<Spec>> = std::sync::OnceLock::new();
        let mut specs = BUILT_INS
            .get_or_init(|| {
                BUILT_IN
                    .iter()
                    .map(|(name, text)| toml::from_str(text).unwrap_or_else(|e| panic!("{name}: {e}")))
                    .collect()
            })
            .clone();
        let mut errors = Vec::new();
        let mut files: Vec<PathBuf> = std::fs::read_dir(Self::dir())
            .map(|dir| dir.filter_map(|entry| Some(entry.ok()?.path())).collect())
            .unwrap_or_default();
        files.retain(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("toml")));
        files.sort();
        for path in files {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| toml::from_str::<Spec>(&text).map_err(|e| e.message().to_string()))
                .and_then(|spec| spec.check().map(|()| spec))
            {
                // A user's surface replaces a built-in one with the same title.
                Ok(spec) => {
                    specs.retain(|s| !s.title.eq_ignore_ascii_case(&spec.title));
                    specs.push(spec);
                }
                Err(error) => errors.push(format!("{name}: {error}")),
            }
        }
        Library { specs, errors }
    }

    /// Rows for surfaces whose title or keywords start with the query's words.
    pub fn search(&self, query: &str) -> Vec<ResultItem> {
        let query = query.trim().to_lowercase();
        if query.chars().count() < MIN_QUERY {
            return Vec::new();
        }
        self.specs
            .iter()
            .enumerate()
            .filter(|(_, spec)| {
                std::iter::once(&spec.title)
                    .chain(&spec.keywords)
                    .any(|keyword| keyword.to_lowercase().starts_with(&query))
            })
            .map(|(index, spec)| {
                // Says it is Wisp's own panel, since a Settings page often has the same title.
                let subtitle = if spec.description.is_empty() {
                    "Wisp panel".to_string()
                } else {
                    format!("Wisp panel · {}", spec.description)
                };
                ResultItem::new(Kind::Command, spec.title.clone(), subtitle, Action::Surface(index))
                    .glyph(GLYPH)
                    .verb("Open")
            })
            .collect()
    }
}

/// Where the focus starts: the first widget that does something, so a
/// clock or battery reading at the top does not hold it.
pub fn first_control(widgets: &[Widget]) -> usize {
    widgets
        .iter()
        .position(|w| !matches!(w.input, Input::Info { .. }))
        .unwrap_or(0)
}

/// Shown on surface rows.
pub const GLYPH: &str = "\u{E80A}";

/// The surface's widgets for the current state, keeping those that match `filter`.
pub fn resolve(spec: &Spec, state: &State, filter: &str) -> Vec<Widget> {
    let filter = filter.trim().to_lowercase();
    spec.parts
        .iter()
        .enumerate()
        .flat_map(|(index, part)| {
            widgets(part, state).into_iter().map(move |mut widget| {
                if spec.title == "Now" {
                    widget.quiet = true;
                    widget.wide = true;
                }
                widget.id = match widget.input {
                    Input::Slider {
                        target: Target::App(pid),
                        ..
                    } => format!("app:{pid}"),
                    // Other parts make at most one widget.
                    _ => format!("part:{index}"),
                };
                widget
            })
        })
        .filter(|widget| widget.matches(&filter))
        .collect()
}

/// Extensions that would run a program. Links open pages, files and folders.
const RUNNABLE: [&str; 12] = [
    "exe", "com", "bat", "cmd", "ps1", "vbs", "vbe", "js", "jse", "wsf", "msi", "scr",
];

const LINK_SCHEMES: [&str; 4] = ["http", "https", "ms-settings", "mailto"];

impl Spec {
    /// Checks what a valid TOML spec could still get wrong.
    fn check(&self) -> Result<(), String> {
        for part in &self.parts {
            if let Part::Link { title, target, .. } = part {
                let lower = target.trim().to_ascii_lowercase();
                // "C:\…" is a path; "https:…" is a scheme.
                let scheme = lower.split_once(':').map(|(s, _)| s).filter(|s| s.len() > 1);
                let allowed = match scheme {
                    Some(scheme) => LINK_SCHEMES.contains(&scheme),
                    None => {
                        let file = lower.rsplit(['\\', '/']).next().unwrap_or(&lower);
                        let extension = file.rsplit_once('.').map(|(_, ext)| ext);
                        !extension.is_some_and(|ext| RUNNABLE.contains(&ext))
                    }
                };
                if !allowed {
                    return Err(format!(
                        "link \"{title}\" can only open a web page, a Settings page, a file or a folder"
                    ));
                }
            }
        }
        Ok(())
    }
}

fn checking(label: &str, glyph: &'static str) -> Widget {
    Widget::new(label, "Checking…", glyph, Input::Info { value: "…".into() })
}

fn widgets(part: &Part, state: &State) -> Vec<Widget> {
    match part {
        Part::Networks | Part::Devices => Vec::new(),
        Part::Clock { city } => clock(city.as_deref()),
        Part::Battery => match state.battery {
            None => vec![checking("Battery", BATTERY)],
            Some(None) => Vec::new(),
            Some(Some(battery)) => {
                let detail = match (battery.plugged_in, battery.saver) {
                    (true, _) => "Plugged in",
                    (false, true) => "Battery saver",
                    (false, false) => "On battery",
                };
                vec![Widget::new(
                    "Battery",
                    detail,
                    BATTERY,
                    Input::Info {
                        value: format!("{}%", battery.percent),
                    },
                )]
            }
        },
        Part::Wifi => match &state.wifi {
            None => vec![checking("Wi-Fi", glyph::WIFI)],
            Some(None) => Vec::new(),
            Some(Some(wifi)) => {
                let detail = match (&wifi.network, wifi.on) {
                    (Some(network), _) => network.clone(),
                    (None, true) => "Not connected".into(),
                    (None, false) => "Off".into(),
                };
                vec![Widget::new(
                    "Wi-Fi",
                    detail,
                    glyph::WIFI,
                    Input::Link {
                        action: Action::Command(Command::RadioView(true)),
                    },
                )]
            }
        },
        Part::Bluetooth => match &state.bluetooth {
            None => vec![checking("Bluetooth", glyph::BLUETOOTH)],
            Some(None) => Vec::new(),
            Some(Some(bluetooth)) => {
                let detail = match bluetooth.connected.as_slice() {
                    _ if !bluetooth.on => "Off".to_string(),
                    [] => "No devices connected".into(),
                    [one] => one.clone(),
                    [first, rest @ ..] => format!("{first} and {} more", rest.len()),
                };
                vec![Widget::new(
                    "Bluetooth",
                    detail,
                    glyph::BLUETOOTH,
                    Input::Link {
                        action: Action::Command(Command::RadioView(false)),
                    },
                )]
            }
        },
        Part::Volume => match &state.audio {
            None => vec![checking("Volume", glyph::VOLUME)],
            Some(None) => Vec::new(),
            Some(Some(audio)) => vec![Widget::new(
                "Volume",
                audio.device.clone(),
                if audio.muted { glyph::MUTED } else { glyph::VOLUME },
                Input::Slider {
                    value: audio.volume,
                    muted: audio.muted,
                    target: Target::Master,
                },
            )],
        },
        Part::Outputs => {
            if state.outputs.len() < 2 {
                return Vec::new();
            }
            let selected = state.outputs.iter().position(|o| o.default).unwrap_or(0);
            vec![Widget::new(
                "Output",
                "",
                glyph::SPEAKER,
                Input::Choice {
                    options: state
                        .outputs
                        .iter()
                        .map(|o| (o.name.clone(), Command::Output(o.id.clone())))
                        .collect(),
                    selected,
                },
            )]
        }
        Part::Apps => state
            .sessions
            .iter()
            .map(|session| {
                let mut widget = Widget::new(
                    &session.name,
                    "",
                    glyph::VOLUME,
                    Input::Slider {
                        value: session.volume,
                        muted: session.muted,
                        target: Target::App(session.pid),
                    },
                );
                widget.icon = session.icon_key();
                widget
            })
            .collect(),
        Part::Media => match &state.media {
            Some(Some(playing)) => vec![media_widget(playing)],
            _ => Vec::new(),
        },
        Part::Link { title, target, detail } => {
            vec![Widget::new(
                title.clone(),
                detail.clone(),
                glyph::OPEN,
                Input::Link {
                    action: Action::Open(target.clone()),
                },
            )]
        }
    }
}

const BATTERY: &str = "\u{E83F}";

fn clock(city: Option<&str>) -> Vec<Widget> {
    let Some(city) = city else {
        let now = time::local();
        return vec![Widget::new(
            "Local time",
            now.short_date(),
            glyph::CLOCK,
            Input::Info { value: now.time() },
        )];
    };
    let Some((zone, name)) = time::find(city).into_iter().next() else {
        return vec![Widget::new(
            city,
            "Unknown city",
            glyph::CLOCK,
            Input::Info { value: "?".into() },
        )];
    };
    let Some(now) = time::in_zone(zone) else {
        return Vec::new();
    };
    vec![Widget::new(
        name,
        format!("{} · {}", now.short_date(), now.difference()),
        glyph::CLOCK,
        Input::Info { value: now.time() },
    )]
}

fn media_widget(playing: &media::State) -> Widget {
    let title = if playing.title.is_empty() {
        playing.app.clone()
    } else {
        playing.title.clone()
    };
    let detail = [playing.artist.as_str(), playing.app.as_str()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let play = if playing.playing { glyph::PAUSE } else { glyph::PLAY };
    Widget::new(
        title,
        detail,
        play,
        Input::Buttons {
            buttons: vec![
                (glyph::PREVIOUS, Command::Media(media::Control::Previous)),
                (play, Command::Media(media::Control::toggle(playing.playing))),
                (glyph::NEXT, Command::Media(media::Control::Next)),
            ],
        },
    )
}

/// Applies a command to `state` before the worker confirms it, so a surface
/// shows the change at once.
pub fn apply(command: &Command, state: &mut State) {
    match command {
        Command::AppVolume { pid, percent } => {
            for session in state.sessions.iter_mut().filter(|s| s.pid == *pid) {
                session.volume = *percent;
                session.muted = false;
            }
        }
        Command::Output(id) => {
            for output in &mut state.outputs {
                output.default = output.id == *id;
            }
            if let (Some(Some(audio)), Some(output)) = (&mut state.audio, state.outputs.iter().find(|o| o.default)) {
                audio.device = output.name.clone();
            }
        }
        Command::Media(control @ (media::Control::Play | media::Control::Pause)) => {
            if let Some(Some(media)) = &mut state.media {
                media.playing = *control == media::Control::Play;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{audio, battery};

    fn state() -> State {
        State {
            audio: Some(Some(audio::State {
                volume: 40,
                muted: false,
                device: "Speakers".into(),
            })),
            battery: Some(Some(battery::State {
                percent: 82,
                plugged_in: true,
                saver: false,
            })),
            sessions: vec![audio::Session {
                pid: 7,
                exe: r"C:\Apps\Spotify.exe".into(),
                name: "Spotify".into(),
                volume: 60,
                muted: false,
            }],
            ..State::default()
        }
    }

    #[test]
    fn built_in_surfaces_parse() {
        let library = Library::load();
        assert!(library.specs.iter().any(|s| s.title == "Sound"));
        assert!(library.specs.iter().any(|s| s.title == "Now"));
    }

    #[test]
    fn finds_surfaces_by_keyword() {
        let library = Library::load();
        let titles = |q: &str| library.search(q).into_iter().map(|r| r.title).collect::<Vec<_>>();
        assert_eq!(titles("mixer"), ["Sound"]);
        assert_eq!(titles("sou"), ["Sound"]);
        assert!(titles("so").is_empty());
    }

    #[test]
    fn resolves_parts_against_state() {
        let spec: Spec = toml::from_str(
            r#"
            title = "Test"
            [[part]]
            kind = "volume"
            [[part]]
            kind = "apps"
            [[part]]
            kind = "battery"
            [[part]]
            kind = "media"
            "#,
        )
        .unwrap();
        let widgets = resolve(&spec, &state(), "");
        let labels: Vec<_> = widgets.iter().map(|w| w.label.as_str()).collect();
        // Nothing plays, so media has no widget.
        assert_eq!(labels, ["Volume", "Spotify", "Battery"]);
        assert_eq!(widgets[1].icon.as_deref(), Some(r"exe:c:\apps\spotify.exe"));
        assert_eq!(
            widgets[1].input,
            Input::Slider {
                value: 60,
                muted: false,
                target: Target::App(7)
            }
        );
        assert_eq!(resolve(&spec, &state(), "spot").len(), 1);
    }

    #[test]
    fn rejects_unknown_parts() {
        assert!(toml::from_str::<Spec>("title = \"X\"\n[[part]]\nkind = \"shell\"\n").is_err());
        assert!(toml::from_str::<Spec>("title = \"X\"\nscript = \"rm\"\n").is_err());
    }

    #[test]
    fn rejects_links_that_run_programs() {
        let spec = |target: &str| -> Spec {
            toml::from_str(&format!(
                "title = \"X\"\n[[part]]\nkind = \"link\"\ntitle = \"L\"\ntarget = '{target}'\n"
            ))
            .unwrap()
        };
        assert!(spec(r"C:\Tools\evil.EXE").check().is_err());
        assert!(spec(r"C:\Tools\setup.cmd").check().is_err());
        assert!(spec("ms-settings:display").check().is_ok());
        assert!(spec("https://example.com").check().is_ok());
        assert!(spec(r"C:\Users\me\Documents").check().is_ok());
        assert!(spec(r"D:\notes\plan.md").check().is_ok());
        assert!(spec("file:///C:/Tools/evil.exe").check().is_err());
        assert!(spec("search-ms:query=x").check().is_err());
    }

    #[test]
    fn keeps_widget_ids_while_state_changes() {
        let spec: Spec =
            toml::from_str("title = \"T\"\n[[part]]\nkind = \"volume\"\n[[part]]\nkind = \"apps\"\n").unwrap();
        let mut changed = state();
        changed.sessions[0].volume = 10;
        let ids = |s: &State| resolve(&spec, s, "").into_iter().map(|w| w.id).collect::<Vec<_>>();
        assert_eq!(ids(&state()), ["part:0", "app:7"]);
        assert_eq!(ids(&state()), ids(&changed));
    }

    #[test]
    fn applies_app_volume_at_once() {
        let mut state = state();
        apply(&Command::AppVolume { pid: 7, percent: 20 }, &mut state);
        assert_eq!(state.sessions[0].volume, 20);
    }
}
