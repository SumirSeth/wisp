//! The settings window: a grid of modules, one per area of the launcher.
//! `--welcome` shows the one-time onboarding instead.

use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    switch::Switch,
};
use gpui_kit::{component::*, prelude::*, *};
use toml_edit::{Array, Value};

use crate::{
    config::{self, Config, Store},
    hotkey,
    palette::{self, Palette},
    system,
};

const ICON_SVG: &[u8] = include_bytes!("../../assets/icon.svg");

const THEMES: [Choice; 3] = [
    Choice::new("system", "System", "Like Windows"),
    Choice::new("dark", "Dark", "Night blue"),
    Choice::new("light", "Light", "Paper white"),
];
const BACKDROPS: [Choice; 3] = [
    Choice::new("acrylic", "Acrylic", "Blurred, fades in"),
    Choice::new("mica", "Mica", "Tinted, fades in"),
    Choice::new("solid", "Solid", "Instant"),
];
const SEARCH_ENGINES: [(&str, &str); 5] = [
    ("Google", "https://www.google.com/search?q={query}"),
    ("DuckDuckGo", "https://duckduckgo.com/?q={query}"),
    ("Bing", "https://www.bing.com/search?q={query}"),
    ("Brave", "https://search.brave.com/search?q={query}"),
    ("Kagi", "https://kagi.com/search?q={query}"),
];
const ONBOARDING_STEPS: usize = 3;

#[derive(Clone, Copy)]
struct Choice {
    value: &'static str,
    label: &'static str,
    note: &'static str,
}

impl Choice {
    const fn new(value: &'static str, label: &'static str, note: &'static str) -> Self {
        Self { value, label, note }
    }
}

pub struct Settings {
    store: Option<Store>,
    config: Config,
    error: Option<String>,
    shortcut_error: Option<String>,
    url_error: Option<String>,
    folder_error: Option<String>,
    /// `Some(step)` while the one-time onboarding is showing.
    onboarding: Option<usize>,
    shortcut: Entity<InputState>,
    url: Entity<InputState>,
    folder: Entity<InputState>,
    autostart: bool,
    powertoys: Result<bool, String>,
    launcher_running: bool,
    recording: bool,
    typing_shortcut: bool,
    focus: FocusHandle,
    icon: Arc<Image>,
}

impl Settings {
    pub fn new(welcome: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (store, config, error) = match config::path() {
            Ok(path) => {
                let store = Store::new(path);
                let existed = store.path.exists();
                let loaded = store.ensure_exists().and_then(|()| store.load());
                if !existed && loaded.is_ok() {
                    system::notify_launcher();
                }
                match loaded {
                    Ok(config) => (Some(store), config, None),
                    Err(error) => (Some(store), Config::default(), Some(error)),
                }
            }
            Err(error) => (None, Config::default(), Some(error)),
        };
        let shortcut = cx.new(|cx| InputState::new(window, cx).default_value(config.hotkey.clone()));
        let url = cx.new(|cx| InputState::new(window, cx).default_value(config.search_url.clone()));
        let folder = cx.new(|cx| InputState::new(window, cx).placeholder("Add a folder name, such as node_modules"));
        let focus = cx.focus_handle();

        cx.subscribe_in(&shortcut, window, |this, input, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().to_string();
                this.shortcut_error = hotkey::parse(&value).err();
                cx.notify();
            } else if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. })
                && this.commit_shortcut(window, cx)
            {
                this.typing_shortcut = false;
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&url, window, |this, input, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().to_string();
                this.url_error = config::validate_url(&value).err();
                if this.url_error.is_none() && value != this.config.search_url {
                    this.save("search_url", value.into(), window, cx);
                }
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&folder, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.add_folder(window, cx);
            }
        })
        .detach();
        cx.observe_window_appearance(window, |this, window, cx| this.apply_theme(window, cx))
            .detach();
        cx.on_blur(&focus, window, |this, _, cx| {
            this.recording = false;
            cx.notify();
        })
        .detach();

        let this = Self {
            store,
            shortcut_error: hotkey::parse(&config.hotkey).err(),
            url_error: config::validate_url(&config.search_url).err(),
            config,
            error,
            folder_error: None,
            onboarding: welcome.then_some(0),
            shortcut,
            url,
            folder,
            autostart: system::autostart_enabled(),
            powertoys: system::powertoys_run_enabled(),
            launcher_running: system::launcher_running(),
            recording: false,
            typing_shortcut: false,
            focus,
            icon: Arc::new(Image::from_bytes(ImageFormat::Svg, ICON_SVG.to_vec())),
        };
        this.apply_theme(window, cx);
        this
    }

    fn apply_theme(&self, window: &mut Window, cx: &mut App) {
        match self.config.theme.as_str() {
            "dark" => Theme::change(ThemeMode::Dark, Some(window), cx),
            "light" => Theme::change(ThemeMode::Light, Some(window), cx),
            _ => Theme::sync_system_appearance(Some(window), cx),
        }
        palette::apply(cx);
    }

    fn save(&mut self, key: &str, value: Value, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let result = self
            .store
            .as_ref()
            .ok_or_else(|| "Cannot find the config folder.".into())
            .and_then(|store| store.set(key, value));
        match result {
            Ok(config) => {
                self.config = config;
                self.error = None;
                system::notify_launcher();
                self.apply_theme(window, cx);
                cx.notify();
                true
            }
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                false
            }
        }
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = self
            .store
            .as_ref()
            .ok_or_else(|| "Cannot find the config folder.".into())
            .and_then(Store::load);
        match result {
            Ok(config) => {
                self.config = config;
                self.error = None;
                self.shortcut_error = hotkey::parse(&self.config.hotkey).err();
                self.url_error = config::validate_url(&self.config.search_url).err();
                let (hotkey, url) = (self.config.hotkey.clone(), self.config.search_url.clone());
                self.shortcut
                    .update(cx, |input, cx| input.set_value(hotkey, window, cx));
                self.url.update(cx, |input, cx| input.set_value(url, window, cx));
                self.apply_theme(window, cx);
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn record(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording || !self.focus.is_focused(window) {
            return;
        }
        cx.stop_propagation();
        window.prevent_default();
        let key = &event.keystroke;
        if key.key == "escape" && !key.modifiers.modified() {
            self.recording = false;
            self.shortcut_error = hotkey::parse(&self.config.hotkey).err();
            cx.notify();
            return;
        }
        let m = key.modifiers;
        match hotkey::recorded(&key.key, m.control, m.alt, m.shift, m.platform) {
            Ok(value) => {
                self.recording = false;
                self.shortcut_error = None;
                self.shortcut
                    .update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                if value != self.config.hotkey {
                    self.save("hotkey", value.into(), window, cx);
                }
            }
            Err(error) => self.shortcut_error = Some(error),
        }
        cx.notify();
    }

    fn start_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = true;
        self.typing_shortcut = false;
        self.shortcut_error = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn commit_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let value = self.shortcut.read(cx).value().to_string();
        self.shortcut_error = hotkey::parse(&value).err();
        if self.shortcut_error.is_some() {
            cx.notify();
            return false;
        }
        value == self.config.hotkey || self.save("hotkey", value.into(), window, cx)
    }

    fn set_autostart(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match system::set_autostart(enabled) {
            Ok(()) => self.autostart = enabled,
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn open_config(&mut self, cx: &mut Context<Self>) {
        let result = self
            .store
            .as_ref()
            .ok_or_else(|| "Cannot find the config folder.".into())
            .and_then(|store| {
                let existed = store.path.exists();
                store.ensure_exists()?;
                if !existed {
                    system::notify_launcher();
                }
                system::open_config(&store.path)
            });
        if let Err(error) = result {
            self.error = Some(error);
            cx.notify();
        }
    }

    fn add_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.folder.read(cx).value().trim().to_owned();
        if name.is_empty() || name.contains(['/', '\\']) {
            self.folder_error = Some("Enter a folder name, such as node_modules.".into());
        } else if self
            .config
            .file_search_exclude
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&name))
        {
            self.folder_error = Some("This folder name is already in the list.".into());
        } else {
            let mut folders = self.config.file_search_exclude.clone();
            folders.push(name);
            if self.save_folders(folders, window, cx) {
                self.folder_error = None;
                self.folder.update(cx, |input, cx| input.set_value("", window, cx));
            }
        }
        cx.notify();
    }

    fn save_folders(&mut self, folders: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let array: Array = folders.into_iter().collect();
        self.save("file_search_exclude", Value::Array(array), window, cx)
    }

    fn set_search_url(&mut self, url: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if self.save("search_url", url.into(), window, cx) {
            self.url_error = None;
            self.url.update(cx, |input, cx| input.set_value(url, window, cx));
        }
    }

    // Modules.

    fn shortcut_module(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let keys = h_flex()
            .id("keycaps")
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(if self.recording { p.accent } else { p.border })
            .bg(if self.recording { p.accent_soft } else { p.raised })
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.start_recording(window, cx)))
            .when(self.recording, |row| {
                row.child(div().text_color(p.accent).child("Press the new shortcut…"))
            })
            .when(!self.recording, |row| {
                row.children(self.config.hotkey.split('+').map(|key| keycap(key.trim(), p)))
            });
        module(Lucide::Keyboard, "Shortcut", p)
            .child(hint(
                "Shows or hides the launcher. Click the keys to record a new shortcut.",
                p,
            ))
            .child(
                h_flex().gap_3().items_center().child(keys).child(div().flex_1()).child(
                    Button::new("type-shortcut")
                        .ghost()
                        .small()
                        .label(if self.typing_shortcut {
                            "Cancel"
                        } else {
                            "Type it instead"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.typing_shortcut = !this.typing_shortcut;
                            this.recording = false;
                            cx.notify();
                        })),
                ),
            )
            .when(self.typing_shortcut, |m| {
                m.child(Input::new(&self.shortcut).aria_label("Shortcut"))
                    .child(hint("For example Ctrl+Shift+Space. Press Enter to save.", p))
            })
            .children(self.shortcut_error.as_ref().map(|e| error_text(e, p)))
            .children(self.conflict_warning(p))
    }

    fn conflict_warning(&self, p: Palette) -> Option<Div> {
        let enabled = self.powertoys.as_ref().copied().unwrap_or(false);
        system::has_shortcut_conflict(&self.config.hotkey, enabled).then(|| {
            notice(
                Lucide::TriangleAlert,
                "PowerToys Run is on and also uses Alt+Space. Turn it off in PowerToys Settings, or pick another shortcut.",
                p.warn,
                p,
            )
        })
    }

    fn theme_module(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        module(Lucide::SunMoon, "Theme", p).child(self.tiles("theme", &THEMES, &self.config.theme, p, cx))
    }

    fn backdrop_module(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        module(Lucide::Layers, "Backdrop", p).child(self.tiles("backdrop", &BACKDROPS, &self.config.backdrop, p, cx))
    }

    fn tiles(&self, key: &'static str, choices: &[Choice], current: &str, p: Palette, cx: &mut Context<Self>) -> Div {
        h_flex().gap_2().items_stretch().children(choices.iter().map(|choice| {
            let selected = choice.value == current;
            let value = choice.value;
            v_flex()
                .id(SharedString::from(format!("{key}-{value}")))
                .flex_1()
                .min_w_0()
                .gap_1()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(if selected { p.accent } else { p.border })
                .bg(if selected { p.accent_soft } else { p.raised })
                .cursor_pointer()
                .hover(|s| s.border_color(p.accent))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.config_value(key) != value {
                        this.save(key, value.into(), window, cx);
                    }
                }))
                .child(swatch(key, value, p))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if selected { p.accent } else { p.fg })
                        .child(choice.label),
                )
                .child(div().text_xs().text_color(p.muted).child(choice.note))
        }))
    }

    fn config_value(&self, key: &str) -> &str {
        match key {
            "theme" => &self.config.theme,
            "backdrop" => &self.config.backdrop,
            _ => "",
        }
    }

    fn results_module(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let n = self.config.max_results;
        let step = |id: &'static str, icon: Lucide, delta: i64, cx: &mut Context<Self>| {
            Button::new(id)
                .small()
                .icon(Icon::new(icon))
                .disabled(!(1..=20).contains(&(n + delta)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    let value = (this.config.max_results + delta).clamp(1, 20);
                    this.save("max_results", value.into(), window, cx);
                }))
        };
        // A small picture of the launcher with one bar per visible result.
        let preview = v_flex()
            .gap(px(3.))
            .p_2()
            .w(px(96.))
            .rounded_md()
            .border_1()
            .border_color(p.border)
            .bg(p.raised)
            .child(div().h(px(8.)).rounded_sm().bg(p.border))
            .children((0..n).map(|i| {
                div()
                    .h(px(3.))
                    .rounded_sm()
                    .bg(if i == 0 { p.accent } else { p.muted.opacity(0.35) })
            }));
        module(Lucide::Rows3, "Results", p).child(
            h_flex()
                .gap_4()
                .items_center()
                .child(
                    v_flex()
                        .gap_2()
                        .flex_1()
                        .child(
                            h_flex()
                                .gap_3()
                                .items_center()
                                .child(step("fewer", Lucide::Minus, -1, cx))
                                .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child(n.to_string()))
                                .child(step("more", Lucide::Plus, 1, cx)),
                        )
                        .child(hint("Visible at once. Scroll to see more.", p)),
                )
                .child(preview),
        )
    }

    fn startup_module(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        module(Lucide::Power, "Startup", p)
            .child(toggle_row(
                "Start with Windows",
                "Open Wisp when you sign in.",
                Switch::new("autostart")
                    .checked(self.autostart)
                    .accessibility_label("Start with Windows")
                    .on_change(cx.listener(|this, enabled: &bool, _, cx| this.set_autostart(*enabled, cx))),
                p,
            ))
            .child(toggle_row(
                "Recent apps",
                "Show them when the search box is empty.",
                Switch::new("recent")
                    .checked(self.config.show_recent)
                    .accessibility_label("Show recent apps")
                    .on_change(cx.listener(|this, enabled: &bool, window, cx| {
                        this.save("show_recent", (*enabled).into(), window, cx);
                    })),
                p,
            ))
    }

    fn files_module(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        module(Lucide::FolderSearch, "File search", p)
            .child(toggle_row(
                "Search files",
                "Find file names in the Windows Search index. Start a query with ? for files only.",
                Switch::new("file-search")
                    .checked(self.config.file_search)
                    .accessibility_label("File search")
                    .on_change(cx.listener(|this, enabled: &bool, window, cx| {
                        this.save("file_search", (*enabled).into(), window, cx);
                    })),
                p,
            ))
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Skipped folders"))
            .child(hint(
                "Wisp also skips every file and folder whose name starts with a dot.",
                p,
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(self.config.file_search_exclude.iter().enumerate().map(|(index, name)| {
                        h_flex()
                            .id(("folder", index))
                            .gap_1()
                            .pl_2()
                            .pr_1()
                            .py(px(2.))
                            .rounded_full()
                            .border_1()
                            .border_color(p.border)
                            .bg(p.raised)
                            .text_sm()
                            .child(name.clone())
                            .child(
                                div()
                                    .id(("remove-folder", index))
                                    .p(px(2.))
                                    .rounded_full()
                                    .cursor_pointer()
                                    .text_color(p.muted)
                                    .hover(|s| s.text_color(p.danger))
                                    .child(Icon::new(Lucide::X).size_3())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let mut folders = this.config.file_search_exclude.clone();
                                        if index < folders.len() {
                                            folders.remove(index);
                                            this.save_folders(folders, window, cx);
                                        }
                                    })),
                            )
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(Input::new(&self.folder).aria_label("Folder name to skip").flex_1())
                    .child(
                        Button::new("add-folder")
                            .label("Add")
                            .on_click(cx.listener(|this, _, window, cx| this.add_folder(window, cx))),
                    ),
            )
            .children(self.folder_error.as_ref().map(|e| error_text(e, p)))
    }

    fn web_module(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        module(Lucide::Globe, "Web search", p)
            .child(hint("Used by the last result, \"Search the web\".", p))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(SEARCH_ENGINES.iter().map(|&(name, url)| {
                        let selected = self.config.search_url == url;
                        div()
                            .id(SharedString::from(format!("engine-{name}")))
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .border_1()
                            .border_color(if selected { p.accent } else { p.border })
                            .bg(if selected { p.accent_soft } else { p.raised })
                            .text_color(if selected { p.accent } else { p.fg })
                            .text_sm()
                            .cursor_pointer()
                            .hover(|s| s.border_color(p.accent))
                            .child(name)
                            .on_click(cx.listener(move |this, _, window, cx| this.set_search_url(url, window, cx)))
                    })),
            )
            .child(Input::new(&self.url).aria_label("Web search URL"))
            .child(hint("Custom URL: Wisp replaces {query} with your search text.", p))
            .children(self.url_error.as_ref().map(|e| error_text(e, p)))
    }

    fn header(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let (dot, status) = if self.launcher_running {
            (p.accent, format!("Running · {}", self.config.hotkey))
        } else {
            (p.muted, "Not running".to_string())
        };
        h_flex()
            .gap_3()
            .items_center()
            .px_6()
            .py_4()
            .child(img(self.icon.clone()).size(px(36.)))
            .child(
                v_flex()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Wisp"))
                    .child(hint(format!("Settings · v{}", env!("CARGO_PKG_VERSION")), p)),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .border_1()
                    .border_color(p.border)
                    .bg(p.surface)
                    .text_sm()
                    .child(div().size(px(8.)).rounded_full().bg(dot))
                    .child(status),
            )
            .child(
                Button::new("open-config")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::FileCog))
                    .label("Config file")
                    .on_click(cx.listener(|this, _, _, cx| this.open_config(cx))),
            )
            .child(
                Button::new("github")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::Github))
                    .on_click(|_, _, cx| cx.open_url("https://github.com/SumirSeth/wisp")),
            )
    }

    fn settings_view(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let error = self.error.as_ref().map(|error| {
            notice(Lucide::CircleAlert, error, p.danger, p).child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("fix-config")
                            .small()
                            .label("Open config file")
                            .on_click(cx.listener(|this, _, _, cx| this.open_config(cx))),
                    )
                    .child(
                        Button::new("reload")
                            .small()
                            .label("Reload")
                            .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
                    ),
            )
        });
        v_flex()
            .size_full()
            .child(self.header(p, cx))
            .child(
                v_flex()
                    .id("modules")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_6()
                    .pb_6()
                    .gap_4()
                    .children(error)
                    .child(self.shortcut_module(p, cx))
                    .child(pair(self.theme_module(p, cx), self.backdrop_module(p, cx)))
                    .child(pair(self.results_module(p, cx), self.startup_module(p, cx)))
                    .child(self.files_module(p, cx))
                    .child(self.web_module(p, cx)),
            )
            .into_any_element()
    }

    // Onboarding.

    fn onboarding_view(&self, step: usize, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let body = match step {
            0 => v_flex()
                .gap_4()
                .items_center()
                .child(img(self.icon.clone()).size(px(88.)))
                .child(div().text_3xl().font_weight(FontWeight::SEMIBOLD).child("Welcome to Wisp"))
                .child(
                    div()
                        .text_color(p.muted)
                        .text_center()
                        .child("Press one shortcut, type a few letters, press Enter. Apps, files, sums and the web."),
                )
                .child(self.shortcut_module(p, cx).w_full()),
            1 => v_flex()
                .gap_4()
                .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("Check for conflicts"))
                .child(match self.conflict_warning(p) {
                    Some(warning) => warning,
                    None => notice(
                        Lucide::CircleCheck,
                        "No conflict with PowerToys Run. Another app can still claim the same shortcut; Wisp tells you if that happens.",
                        p.accent,
                        p,
                    ),
                })
                .child(self.shortcut_module(p, cx)),
            _ => v_flex()
                .gap_4()
                .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("Almost done"))
                .child(self.startup_module(p, cx))
                .child(hint(
                    "You can change everything later: right-click the Wisp icon in the tray and choose Settings.",
                    p,
                )),
        };
        let dots = h_flex().gap_2().children((0..ONBOARDING_STEPS).map(|i| {
            div()
                .h(px(6.))
                .w(if i == step { px(22.) } else { px(6.) })
                .rounded_full()
                .bg(if i <= step { p.accent } else { p.border })
        }));
        let last = step + 1 == ONBOARDING_STEPS;
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .p_8()
            .child(
                v_flex().w_full().max_w(px(560.)).gap_6().child(body).child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(dots)
                        .child(div().flex_1())
                        .when(step > 0, |row| {
                            row.child(Button::new("back").ghost().label("Back").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.recording = false;
                                    this.onboarding = this.onboarding.map(|s| s.saturating_sub(1));
                                    cx.notify();
                                },
                            )))
                        })
                        .child(
                            Button::new("next")
                                .primary()
                                .label(if last { "Start using Wisp" } else { "Continue" })
                                .disabled(self.shortcut_error.is_some())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.recording = false;
                                    if last {
                                        window.remove_window();
                                        return;
                                    }
                                    this.onboarding = Some(step + 1);
                                    if step + 1 == 1 {
                                        this.powertoys = system::powertoys_run_enabled();
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
            )
            .into_any_element()
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::current(cx);
        let content = match self.onboarding {
            Some(step) => self.onboarding_view(step, p, cx),
            None => self.settings_view(p, cx),
        };
        let title = if self.onboarding.is_some() {
            "Welcome to Wisp"
        } else {
            "Wisp Settings"
        };
        v_flex()
            .size_full()
            .bg(p.bg)
            .text_color(p.fg)
            .font_family(cx.theme().font_family.clone())
            .text_sm()
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::record))
            .child(TitleBar::new().child(div().text_xs().text_color(p.muted).child(title)))
            .child(div().flex_1().min_h_0().child(content))
    }
}

// Building blocks.

/// A card with an icon badge and a title.
fn module(icon: Lucide, title: &'static str, p: Palette) -> Div {
    v_flex()
        .gap_3()
        .p_4()
        .rounded_xl()
        .border_1()
        .border_color(p.border)
        .bg(p.surface)
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(26.))
                        .rounded_md()
                        .bg(p.accent_soft)
                        .text_color(p.accent)
                        .child(Icon::new(icon).size_4()),
                )
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title)),
        )
}

/// Two modules side by side with equal widths.
fn pair(left: Div, right: Div) -> Div {
    h_flex()
        .gap_4()
        .items_stretch()
        .child(left.flex_1().min_w_0())
        .child(right.flex_1().min_w_0())
}

fn keycap(key: &str, p: Palette) -> Div {
    div()
        .px_3()
        .py_1()
        .min_w(px(36.))
        .flex()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_b_2()
        .border_color(p.border)
        .bg(p.surface)
        .text_base()
        .font_weight(FontWeight::SEMIBOLD)
        .child(key.to_owned())
}

/// A small picture of the choice: a two-tone square for themes, a texture for backdrops.
fn swatch(key: &str, value: &str, p: Palette) -> Div {
    let tile = div().h(px(28.)).w_full().rounded_md().border_1().border_color(p.border);
    match (key, value) {
        ("theme", "dark") => tile.bg(rgb(0x0f1526)),
        ("theme", "light") => tile.bg(rgb(0xf3f6fb)),
        ("theme", _) => h_flex()
            .h(px(28.))
            .w_full()
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(p.border)
            .child(div().flex_1().h_full().bg(rgb(0x0f1526)))
            .child(div().flex_1().h_full().bg(rgb(0xf3f6fb))),
        (_, "acrylic") => tile.bg(p.accent.opacity(0.18)),
        (_, "mica") => tile.bg(p.accent.opacity(0.08)),
        _ => tile.bg(p.raised),
    }
}

fn toggle_row(title: &'static str, description: &'static str, control: impl IntoElement, p: Palette) -> Div {
    h_flex()
        .gap_4()
        .items_center()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(hint(description, p)),
        )
        .child(div().flex_none().child(control))
}

fn notice(icon: Lucide, text: impl Into<SharedString>, color: Hsla, p: Palette) -> Div {
    v_flex()
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(color.opacity(0.5))
        .bg(color.opacity(0.08))
        .child(
            h_flex()
                .gap_2()
                .items_start()
                .child(div().text_color(color).child(Icon::new(icon).size_4()))
                .child(div().flex_1().min_w_0().text_color(p.fg).child(text.into())),
        )
}

fn hint(text: impl Into<SharedString>, p: Palette) -> Div {
    div().text_sm().text_color(p.muted).child(text.into())
}

fn error_text(text: &str, p: Palette) -> Div {
    div().text_sm().text_color(p.danger).child(text.to_owned())
}
