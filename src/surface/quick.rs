//! Fixed quick settings, built from the same commands as the list surfaces.
use super::{Input, Part, Widget, nav::Rect};
use crate::{
    commands::{Command, State, glyph, power::Power},
    search::Action,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Hint,
    Clock,
    Battery,
    Radio(u8),
    RadioList(u8),
    Media,
    Small(u8),
    Volume,
    Output,
    Footer(u8),
}

fn tile(slot: Slot, id: &str, mut widget: Widget) -> Widget {
    widget.id = format!("quick:{id}");
    widget.quick = Some(slot);
    widget
}
fn command(label: &str, icon: &'static str, command: Command) -> Widget {
    Widget::new(
        label,
        "",
        icon,
        Input::Link {
            action: Action::Command(command),
        },
    )
}
pub fn widgets(state: &State) -> Vec<Widget> {
    let mut out = Vec::new();
    if let Some(clock) = super::clock(None).pop() {
        out.push(tile(Slot::Clock, "clock", clock));
    }
    if let Some(Some(battery)) = state.battery {
        out.push(tile(
            Slot::Battery,
            "battery",
            Widget::new(
                "Battery",
                "",
                super::BATTERY,
                Input::Info {
                    value: format!(
                        "{}%{}",
                        battery.percent,
                        if battery.plugged_in { " · charging" } else { "" }
                    ),
                },
            ),
        ));
    }
    for (row, part, on) in [
        (
            0,
            Part::Wifi,
            state.wifi.as_ref().and_then(|v| v.as_ref()).map(|s| s.on),
        ),
        (
            1,
            Part::Bluetooth,
            state.bluetooth.as_ref().and_then(|v| v.as_ref()).map(|s| s.on),
        ),
    ] {
        if let Some(mut body) = super::widgets(&part, state).pop() {
            if on == Some(false) {
                body.detail = "Off".into();
            }
            if let Some(on) = on {
                body.input = Input::Toggle {
                    on,
                    command: if row == 0 {
                        Command::WifiPower(!on)
                    } else {
                        Command::BluetoothPower(!on)
                    },
                };
            }
            if body.detail == "No devices connected" {
                body.detail = "Not connected".into();
            }
            out.push(tile(
                Slot::Radio(row),
                if row == 0 { "wifi" } else { "bluetooth" },
                body,
            ));
            out.push(tile(
                Slot::RadioList(row),
                if row == 0 { "networks" } else { "devices" },
                command("More", "\u{E76C}", Command::RadioView(row == 0)),
            ));
        }
    }
    let media = match &state.media {
        Some(Some(playing)) => super::media_widget(playing),
        _ => Widget::new("Nothing playing", "", glyph::PLAY, Input::Info { value: String::new() }),
    };
    out.push(tile(Slot::Media, "media", media));
    let saver = state
        .power_saver
        .or_else(|| state.battery.flatten().map(|v| v.saver))
        .unwrap_or(false);
    for (col, label, icon, on, cmd) in [
        (
            0,
            "Night light",
            "\u{E706}",
            state.night_light.unwrap_or(false),
            Command::NightLight(!state.night_light.unwrap_or(false)),
        ),
        (
            1,
            "Dark mode",
            "\u{E790}",
            state.color_modes.is_some_and(|v| !v[0]),
            Command::LightMode(state.color_modes.is_some_and(|v| !v[0])),
        ),
        (
            2,
            "Mute",
            glyph::MUTED,
            state.audio.as_ref().and_then(|v| v.as_ref()).is_some_and(|v| v.muted),
            Command::Mute(!state.audio.as_ref().and_then(|v| v.as_ref()).is_some_and(|v| v.muted)),
        ),
        (3, "Power saver", glyph::POWER_SAVER, saver, Command::PowerSaver(!saver)),
    ] {
        out.push(tile(
            Slot::Small(col),
            label,
            Widget::new(label, "", icon, Input::Toggle { on, command: cmd }),
        ));
    }
    if let Some(mut volume) = super::widgets(&Part::Volume, state).pop() {
        volume.detail.clear();
        out.push(tile(Slot::Volume, "volume", volume));
    }
    let name = state
        .audio
        .as_ref()
        .and_then(|v| v.as_ref())
        .map_or("No output", |v| v.device.as_str());
    out.push(tile(
        Slot::Output,
        "output",
        Widget::new(
            name,
            "",
            "\u{E76C}",
            Input::Link {
                action: Action::SoundView,
            },
        ),
    ));
    for (col, label, icon, cmd) in [
        (0, "Lock", glyph::LOCK, Command::Power(Power::Lock)),
        (1, "Sleep", "\u{E708}", Command::Power(Power::Sleep)),
        (2, "Restart", "\u{E72C}", Command::Power(Power::Restart)),
        (3, "Shut down", "\u{E7E8}", Command::Power(Power::ShutDown)),
        (4, "Wisp Settings", glyph::SETTINGS, Command::WispSettings),
    ] {
        out.push(tile(Slot::Footer(col), label, command(label, icon, cmd)));
    }
    out.push(tile(
        Slot::Hint,
        "hint",
        Widget::new("", "", "", Input::Info { value: String::new() }),
    ));
    out
}

/// Four columns. Missing radios share their space with media; media never
/// depends on playback state, and the header never depends on a battery.
pub fn layout(widgets: &[Widget], width: f32) -> Vec<Rect> {
    let gap = 10.0;
    let col = (width - 3.0 * gap) / 4.0;
    let half = col * 2.0 + gap;
    let radios: Vec<_> = (0..2)
        .filter(|row| {
            widgets
                .iter()
                .any(|w| matches!(w.quick, Some(Slot::Radio(r) | Slot::RadioList(r)) if r == *row))
        })
        .collect();
    let media_h = 146.0;
    let small_y = 56.0 + media_h + gap;
    widgets
        .iter()
        .map(|w| {
            let (x, y, w, h) = match w.quick {
                Some(Slot::Hint) => (0.0, small_y + 160.0, width - 210.0, 34.0),
                Some(Slot::Clock) => (0.0, 0.0, width - 150.0, 46.0),
                Some(Slot::Battery) => (width - 150.0, 0.0, 150.0, 46.0),
                Some(Slot::Radio(row) | Slot::RadioList(row)) => {
                    let h = if radios.len() == 1 { media_h } else { 68.0 };
                    let y = 56.0 + radios.iter().position(|r| *r == row).unwrap_or(0) as f32 * 78.0;
                    if matches!(w.quick, Some(Slot::RadioList(_))) {
                        (half - 36.0, y, 36.0, h)
                    } else {
                        (0.0, y, half - 36.0, h)
                    }
                }
                Some(Slot::Media) => (
                    if radios.is_empty() { 0.0 } else { half + gap },
                    56.0,
                    if radios.is_empty() { width } else { half },
                    media_h,
                ),
                Some(Slot::Small(col)) => (col as f32 * (col_width(width) + gap), small_y, col_width(width), 82.0),
                Some(Slot::Volume) => (0.0, small_y + 92.0, width - 178.0, 58.0),
                Some(Slot::Output) => (width - 178.0, small_y + 92.0, 178.0, 58.0),
                Some(Slot::Footer(col)) => (width - 5.0 * 40.0 + col as f32 * 40.0, small_y + 160.0, 34.0, 34.0),
                None => (0.0, small_y + 204.0, width, 44.0),
            };
            Rect { x, y, w, h }
        })
        .collect()
}
fn col_width(width: f32) -> f32 {
    (width - 30.0) / 4.0
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        commands::{audio, bluetooth, media, wifi},
        surface::{
            Spec, Target,
            nav::{self, Direction},
        },
    };
    fn state() -> State {
        State {
            wifi: Some(Some(wifi::State {
                on: true,
                network: Some("Home".into()),
                signal: Some(80),
            })),
            bluetooth: Some(Some(bluetooth::State {
                on: false,
                connected: vec![],
            })),
            audio: Some(Some(audio::State {
                volume: 62,
                muted: false,
                device: "Speakers".into(),
            })),
            night_light: Some(true),
            color_modes: Some([false; 2]),
            ..State::default()
        }
    }
    fn index(widgets: &[Widget], slot: Slot) -> usize {
        widgets.iter().position(|w| w.quick == Some(slot)).unwrap()
    }
    #[test]
    fn grid_fits_and_media_stays_put() {
        let mut state = state();
        state.battery = Some(Some(crate::commands::battery::State {
            percent: 82,
            plugged_in: true,
            saver: false,
        }));
        let widgets = widgets(&state);
        let width = crate::render::surface_width();
        let rects = layout(&widgets, width);
        assert!(rects.iter().all(|r| r.x >= 0.0 && r.x + r.w <= width && r.w > 0.0));
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(a.x + a.w <= b.x || b.x + b.w <= a.x || a.bottom() <= b.y || b.bottom() <= a.y);
            }
        }
        let media_rect = rects[index(&widgets, Slot::Media)];
        state.media = Some(Some(media::State {
            title: "Song".into(),
            artist: "Artist".into(),
            app: "Player".into(),
            playing: true,
        }));
        let playing = super::widgets(&state);
        assert_eq!(layout(&playing, width)[index(&playing, Slot::Media)], media_rect);
        state.bluetooth = Some(None);
        let missing = super::widgets(&state);
        assert!(!missing.iter().any(|w| w.quick == Some(Slot::Radio(1))));
        assert_eq!(layout(&missing, width)[index(&missing, Slot::Media)], media_rect);
        state.wifi = Some(None);
        let missing = super::widgets(&state);
        assert_eq!(layout(&missing, width)[index(&missing, Slot::Media)].w, width);
        // Battery absence never shifts the grid.
        assert!(widgets.iter().any(|w| w.quick == Some(Slot::Battery)));
        state.battery = Some(None);
        let missing = super::widgets(&state);
        assert!(!missing.iter().any(|w| w.quick == Some(Slot::Battery)));
        assert_eq!(
            layout(&missing, width)[index(&missing, Slot::Small(0))].y,
            rects[index(&widgets, Slot::Small(0))].y
        );
    }
    #[test]
    fn saver_tile_tracks_ac_and_batteryless_states() {
        for battery in [
            None,
            Some(crate::commands::battery::State {
                percent: 82,
                plugged_in: true,
                saver: true,
            }),
        ] {
            for on in [false, true] {
                let state = State {
                    power_saver: Some(on),
                    battery: Some(battery),
                    ..State::default()
                };
                let widgets = widgets(&state);
                let tile = &widgets[index(&widgets, Slot::Small(3))];
                assert_eq!(tile.label, "Power saver");
                assert!(
                    matches!(tile.input, Input::Toggle { on: shown, command: Command::PowerSaver(target) } if shown == on && target != on)
                );
                assert!(Command::PowerSaver(!on).stays_open());
                let footer: Vec<_> = widgets
                    .iter()
                    .filter(|w| matches!(w.quick, Some(Slot::Footer(_))))
                    .map(|w| w.label.as_str())
                    .collect();
                assert_eq!(footer, ["Lock", "Sleep", "Restart", "Shut down", "Wisp Settings"]);
            }
        }
    }
    #[test]
    fn arrows_reach_segments_and_adjacent_rows() {
        let widgets = widgets(&state());
        let rects = layout(&widgets, crate::render::surface_width());
        let next = |slot, dir| {
            nav::control_neighbour(&widgets, &rects, index(&widgets, slot), dir).map(|i| widgets[i].quick.unwrap())
        };
        assert_eq!(next(Slot::Radio(0), Direction::Right), Some(Slot::RadioList(0)));
        assert_eq!(next(Slot::RadioList(0), Direction::Left), Some(Slot::Radio(0)));
        assert_eq!(next(Slot::Radio(0), Direction::Down), Some(Slot::Radio(1)));
        assert_eq!(next(Slot::RadioList(1), Direction::Right), Some(Slot::Media));
        assert!(matches!(next(Slot::Media, Direction::Down), Some(Slot::Small(_))));
        assert_eq!(next(Slot::Output, Direction::Left), Some(Slot::Volume));
        assert_eq!(next(Slot::Radio(0), Direction::Up), None);
    }
    #[test]
    fn tiles_use_existing_commands_and_custom_specs_stay_lists() {
        let mut state = state();
        state.media = Some(Some(media::State {
            title: "Song".into(),
            artist: String::new(),
            app: String::new(),
            playing: true,
        }));
        let widgets = widgets(&state);
        for (slot, action) in [
            (Slot::Radio(0), Action::Command(Command::WifiPower(false))),
            (Slot::Radio(1), Action::Command(Command::BluetoothPower(true))),
            (Slot::RadioList(0), Action::Command(Command::RadioView(true))),
            (Slot::RadioList(1), Action::Command(Command::RadioView(false))),
            (Slot::Small(0), Action::Command(Command::NightLight(false))),
            (Slot::Small(1), Action::Command(Command::LightMode(true))),
            (Slot::Small(2), Action::Command(Command::Mute(true))),
            (Slot::Small(3), Action::Command(Command::PowerSaver(true))),
            (Slot::Output, Action::SoundView),
            (Slot::Footer(0), Action::Command(Command::Power(Power::Lock))),
            (Slot::Footer(1), Action::Command(Command::Power(Power::Sleep))),
            (Slot::Footer(2), Action::Command(Command::Power(Power::Restart))),
            (Slot::Footer(3), Action::Command(Command::Power(Power::ShutDown))),
            (Slot::Footer(4), Action::Command(Command::WispSettings)),
        ] {
            assert_eq!(widgets[index(&widgets, slot)].activate(0), Some(action));
        }
        assert!(matches!(
            widgets[index(&widgets, Slot::Volume)].input,
            Input::Slider {
                target: Target::Master,
                ..
            }
        ));
        let media = &widgets[index(&widgets, Slot::Media)];
        for (stop, control) in [media::Control::Previous, media::Control::Pause, media::Control::Next]
            .into_iter()
            .enumerate()
        {
            assert_eq!(media.activate(stop), Some(Action::Command(Command::Media(control))));
        }
        let custom: Spec = toml::from_str("title = \"Now\"\n[[part]]\nkind = \"volume\"").unwrap();
        assert!(
            super::super::resolve(&custom, &state, "")
                .iter()
                .all(|w| w.quick.is_none() && w.quiet)
        );
    }
}

#[cfg(test)]
mod slider_tests {
    use super::*;
    #[test]
    fn slider_endpoints_reserve_the_percentage() {
        let r = Rect {
            x: 0.0,
            y: 0.0,
            w: 460.0,
            h: 58.0,
        };
        let (a, b) = super::super::nav::slider_track(&r, true);
        assert_eq!((a, b), (48.0, 404.0));
        assert_eq!(super::super::nav::slider_value(&r, a, true), 0);
        assert_eq!(super::super::nav::slider_value(&r, b, true), 100);
    }
}
