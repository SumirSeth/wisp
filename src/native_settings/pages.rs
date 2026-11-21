use super::*;
use windows::Win32::UI::Shell::*;
const PRESETS: [&str; 9] = [
    "#000000FF",
    "#0F172ABF",
    "#1E3A8ABF",
    "#0F766EBF",
    "#166534BF",
    "#6B21A8BF",
    "#9F1239BF",
    "#B45309BF",
    "#475569BF",
];
impl Settings {
    pub(super) fn page_rows(&self, rows: &mut Vec<Control>) {
        let c = &self.config;
        let mut add =
            |id: &str, label: &str, hint: &str, kind: Kind| rows.push(Control::new(id, label, hint, kind, true));
        if let Some(step) = self.welcome {
            if step == 0 {
                add(
                    "intro",
                    "Move the pointer to the bottom edge of the screen to see the dock. Press the shortcut, type a few letters of an app or file, and press Enter.",
                    "",
                    Kind::Heading,
                );
            }
            if step < 2 {
                add("hotkey", "Shortcut", "", Kind::Text(c.hotkey.clone()));
                add(
                    "record",
                    "",
                    "",
                    Kind::Button(
                        if self.recording {
                            "Press a shortcut..."
                        } else {
                            "Record"
                        }
                        .into(),
                    ),
                );
                add(
                    "win_key",
                    "Use Win key",
                    "Tap the Windows key alone to open Wisp instead of Start.",
                    Kind::Toggle(c.win_key),
                );
                if !c.win_key && system::has_shortcut_conflict(&c.hotkey, self.conflict) {
                    add(
                        "warning",
                        "PowerToys Run also uses Alt+Space.",
                        "Turn it off in PowerToys Settings, or choose another shortcut.",
                        Kind::Heading,
                    );
                }
            } else {
                add("autostart", "Start with Windows", "", Kind::Toggle(self.autostart));
                add(
                    "tray-hint",
                    "To change settings later, right-click the Wisp icon in the notification area and select Settings. With the dock, the icon is behind the \u{2304} button.",
                    "",
                    Kind::Heading,
                );
            }
            if step > 0 {
                add("back", "", "", Kind::Button("Back".into()));
            }
            add(
                "next",
                "",
                "",
                Kind::Button(if step == 2 { "Done" } else { "Next" }.into()),
            );
            for row in rows.iter_mut().skip(4) {
                if matches!(row.id.as_str(), "hotkey" | "record") {
                    row.enabled = !c.win_key;
                }
                if row.id == "next" {
                    row.enabled = self.error.is_none();
                }
            }
            return;
        }
        match self.page {
            1 => {
                add("look", "LOOK", "", Kind::Heading);
                for (id, label, hint, values, current) in [
                    (
                        "theme",
                        "Theme",
                        "",
                        vec![("system", "System"), ("dark", "Dark"), ("light", "Light")],
                        &c.theme,
                    ),
                    (
                        "backdrop",
                        "Backdrop",
                        "Acrylic and Mica blur what is behind the launcher and fade in. Solid appears at once.",
                        vec![("acrylic", "Acrylic"), ("mica", "Mica"), ("solid", "Solid")],
                        &c.backdrop,
                    ),
                ] {
                    let selected = values.iter().position(|(v, _)| *v == current).unwrap_or(0);
                    add(
                        id,
                        label,
                        hint,
                        Kind::Select {
                            choices: values.into_iter().map(|(v, l)| (v.into(), l.into())).collect(),
                            selected,
                        },
                    );
                }
                add("advanced", "Advanced", "", Kind::Button(String::new()));
                if self.advanced {
                    add(
                        "dock_margin",
                        "Gap below the dock",
                        "",
                        Kind::Slider {
                            value: c.dock_margin,
                            min: 0,
                            max: 24,
                        },
                    );
                    add(
                        "custom-color",
                        "Custom dock color",
                        "Colors the dock and the launcher.",
                        Kind::Toggle(!c.dock_color.is_empty()),
                    );
                    if !c.dock_color.is_empty() {
                        add(
                            "picker",
                            "Dock color",
                            "With Acrylic, the color's alpha sets how much of the theme's acrylic shows through. Mica and Solid show the color at full strength.",
                            Kind::Button(self.shown_color().into()),
                        );
                        if self.picker {
                            let color = self.shown_color();
                            add(
                                "preset",
                                "Palette",
                                "",
                                Kind::Select {
                                    choices: PRESETS.iter().map(|s| (s.to_string(), s.to_string())).collect(),
                                    selected: PRESETS.iter().position(|s| *s == color).unwrap_or(2),
                                },
                            );
                            add("dock_color_hex", "Hex", "", Kind::Text(color.into()));
                            let rgba = rgba(color);
                            for (i, (id, label)) in [
                                ("red", "Red"),
                                ("green", "Green"),
                                ("blue", "Blue"),
                                ("alpha", "Opacity"),
                            ]
                            .iter()
                            .enumerate()
                            {
                                add(id, label, "", Kind::Text(rgba[i].to_string()));
                            }
                        }
                    }
                }
                add("results", "RESULTS", "", Kind::Heading);
                add(
                    "max_results",
                    "Visible results",
                    "",
                    Kind::Slider {
                        value: c.max_results,
                        min: 1,
                        max: 20,
                    },
                );
            }
            2 => {
                add("apps-web", "APPS AND WEB", "", Kind::Heading);
                add(
                    "show_recent",
                    "Recent apps",
                    "Shown when the search box is empty.",
                    Kind::Toggle(c.show_recent),
                );
                add(
                    "search_url",
                    "Web search URL",
                    "Wisp replaces {query} with your search text.",
                    Kind::Text(c.search_url.clone()),
                );
                add("files", "FILES", "", Kind::Heading);
                add("file_search", "File search", "", Kind::Toggle(c.file_search));
                add(
                    "file_search_everything",
                    "Use Everything",
                    "When Everything is running, search every drive with it.",
                    Kind::Toggle(c.file_search_everything),
                );
                add(
                    "folder",
                    "Skipped folders",
                    "Names that start with a dot are always skipped.",
                    Kind::Text(self.folder.clone()),
                );
                add("add-folder", "", "", Kind::Button("Add".into()));
                for (i, name) in c.file_search_exclude.iter().enumerate() {
                    add(&format!("remove:{i}"), name, "", Kind::Button("Remove".into()));
                }
            }
            3 => {
                add(
                    "version",
                    "Wisp",
                    &format!("Version {}", env!("CARGO_PKG_VERSION")),
                    Kind::Heading,
                );
                let (status, label) = match &self.updates {
                    Updates::Idle => ("".into(), "Check for updates"),
                    Updates::Checking => ("Checking for updates...".into(), "Checking..."),
                    Updates::Latest => (
                        if crate::channel::NIGHTLY {
                            "Nightly builds skip checks."
                        } else {
                            "Up to date."
                        }
                        .into(),
                        "Check again",
                    ),
                    Updates::Available(release) => (
                        format!("Wisp {} is available.", release.version()),
                        "Install and restart",
                    ),
                    Updates::Installing => ("Installing and restarting...".into(), "Installing..."),
                    Updates::Failed(error) => (error.clone(), "Try again"),
                };
                add("update-status", "Updates", &status, Kind::Heading);
                if let Some(manager) = crate::install::current_manager() {
                    add("manager", manager.instruction(), "", Kind::Heading);
                }
                add("update-action", "", "", Kind::Button(label.into()));
                add("release-notes", "", "", Kind::Link("What's new".into()));
                add(
                    "open-config",
                    "Config file",
                    &self.store.path.display().to_string(),
                    Kind::Button("Open".into()),
                );
                add(
                    "source",
                    "Source code",
                    "github.com/SumirSeth/wisp",
                    Kind::Button("Open".into()),
                );
            }
            _ => {}
        }
    }
    fn shown_color(&self) -> &str {
        self.pending_color
            .as_deref()
            .unwrap_or(if self.config.dock_color.is_empty() {
                &self.color
            } else {
                &self.config.dock_color
            })
    }
    pub(super) fn cancel_color(&mut self) {
        self.pending_color = None;
        if !self.hwnd.is_invalid() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), 42);
            }
        }
    }
    pub(super) fn flush_color(&mut self) {
        let value = self.pending_color.take();
        self.cancel_color();
        if let Some(value) = value
            && value != self.config.dock_color
        {
            self.save("dock_color", value.into());
        }
    }
    fn queue_color(&mut self, value: String) {
        self.color = value.clone();
        self.pending_color = Some(value);
        if !self.hwnd.is_invalid() {
            unsafe {
                SetTimer(Some(self.hwnd), 42, 250, None);
            }
        }
        self.rebuild();
    }
    pub(super) fn edit_color(&mut self, text: &str, channel: Option<&str>) {
        let mut color = rgba(self.shown_color());
        if let Some(channel) = channel {
            let Ok(value) = text.parse::<u8>() else {
                self.error = Some("Enter a whole number from 0 to 255.".into());
                self.rebuild();
                return;
            };
            color[match channel {
                "red" => 0,
                "green" => 1,
                "blue" => 2,
                _ => 3,
            }] = value;
        } else {
            if text.is_empty() || crate::settings_config::validate_color(text).is_err() {
                self.error = Some("Use #RRGGBB or #RRGGBBAA.".into());
                self.rebuild();
                return;
            }
            let alpha = color[3];
            color = rgba(text);
            if text.trim().len() == 7 {
                color[3] = alpha;
            }
        }
        self.error = None;
        self.queue_color(format!(
            "#{:02X}{:02X}{:02X}{:02X}",
            color[0], color[1], color[2], color[3]
        ));
    }
    pub(super) fn add_folder(&mut self, text: &str) {
        let name = text.trim();
        if name.is_empty() || name.contains(['/', '\\']) {
            self.error = Some("Enter a folder name, such as node_modules.".into());
        } else if self
            .config
            .file_search_exclude
            .iter()
            .any(|s| s.eq_ignore_ascii_case(name))
        {
            self.error = Some("This folder name is already in the list.".into());
        } else {
            let mut folders = self.config.file_search_exclude.clone();
            folders.push(name.into());
            self.folder.clear();
            if self
                .focus
                .and_then(|i| self.controls.get(i))
                .is_some_and(|c| c.id == "folder")
            {
                self.editor = Editor::default();
            }
            self.save(
                "file_search_exclude",
                toml_edit::Value::Array(folders.iter().map(String::as_str).collect()),
            );
        }
        self.rebuild();
    }
    pub(super) fn page_action(&mut self, c: &Control, delta: i64, popup: bool) -> bool {
        if let Some(index) = c.id.strip_prefix("remove:").and_then(|s| s.parse::<usize>().ok()) {
            let mut folders = self.config.file_search_exclude.clone();
            if index < folders.len() {
                folders.remove(index);
                self.save(
                    "file_search_exclude",
                    toml_edit::Value::Array(folders.iter().map(String::as_str).collect()),
                );
            }
            return true;
        }
        match c.id.as_str() {
            "advanced" => {
                self.advanced = !self.advanced;
            }
            "picker" => {
                self.picker = !self.picker;
            }
            "custom-color" => {
                if !self.config.dock_color.is_empty() {
                    self.color = self.shown_color().into();
                }
                self.cancel_color();
                self.save(
                    "dock_color",
                    if self.config.dock_color.is_empty() {
                        self.color.clone()
                    } else {
                        String::new()
                    }
                    .into(),
                );
            }
            "preset" => {
                if let Kind::Select { choices, selected } = &c.kind
                    && let Some(i) = if popup {
                        self.popup(&c.rect, choices, *selected)
                    } else {
                        Some((*selected as i64 + delta).rem_euclid(choices.len() as i64) as usize)
                    }
                {
                    self.queue_color(choices[i].0.clone());
                }
            }
            "add-folder" => {
                self.add_folder(&self.folder.clone());
            }
            "update-action" => {
                if matches!(self.updates, Updates::Available(_)) {
                    self.install_update();
                } else if !self.preview {
                    self.check_updates();
                }
            }
            "source" | "release-notes" => {
                let url = windows::core::HSTRING::from(if c.id == "source" {
                    "https://github.com/SumirSeth/wisp"
                } else {
                    "https://github.com/SumirSeth/wisp/releases/latest"
                });
                unsafe {
                    ShellExecuteW(
                        Some(self.hwnd),
                        windows::core::w!("open"),
                        PCWSTR(url.as_ptr()),
                        None,
                        None,
                        SW_SHOWNORMAL,
                    );
                }
            }
            "back" | "next" => {
                self.recording = false;
                if c.id == "next" && self.welcome == Some(2) {
                    unsafe {
                        let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                } else {
                    if c.id == "next"
                        && !self.config.win_key
                        && self.store.set("hotkey", self.config.hotkey.clone().into()).is_err()
                    {
                        return true;
                    }
                    self.welcome = self.welcome.map(|s| {
                        if c.id == "back" {
                            s.saturating_sub(1)
                        } else {
                            (s + 1).min(2)
                        }
                    });
                    self.conflict = system::powertoys_run_enabled().unwrap_or(false);
                    self.scroll = 0.;
                    self.focus = None;
                }
            }
            _ => return false,
        }
        self.rebuild();
        true
    }
    pub(super) fn text_changed(&mut self) {
        let id = self
            .focus
            .and_then(|i| self.controls.get(i))
            .map(|c| c.id.clone())
            .unwrap_or_default();
        let text = self.editor.text().to_owned();
        match id.as_str() {
            "hotkey" => self.error = crate::hotkey::parse(&text).err(),
            "search_url" => self.save("search_url", text.into()),
            "dock_color_hex" | "red" | "green" | "blue" | "alpha" if !text.is_empty() => {
                self.edit_color(&text, if id == "dock_color_hex" { None } else { Some(&id) });
            }
            _ => {}
        }
        self.rebuild();
    }
    pub(super) fn hit(&self, lp: LPARAM) -> Option<usize> {
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.;
        let x = (lp.0 as u16 as i16) as f32 / scale;
        let y = ((lp.0 >> 16) as u16 as i16) as f32 / scale;
        self.controls.iter().enumerate().rposition(|(i, c)| {
            let yy = y + if i >= 4 { self.scroll } else { 0. };
            c.focusable()
                && (i < 4 || y >= 68.)
                && x >= c.rect.left
                && x < c.rect.right
                && yy >= c.rect.top
                && yy < c.rect.bottom
        })
    }
}
pub(super) fn rgba(text: &str) -> [u8; 4] {
    let hex = text.trim().trim_start_matches('#');
    let mut result = [0, 0, 0, 255];
    for (i, value) in result.iter_mut().enumerate() {
        if let Some(part) = hex.get(i * 2..i * 2 + 2) {
            *value = u8::from_str_radix(part, 16).unwrap_or(0);
        }
    }
    result
}
