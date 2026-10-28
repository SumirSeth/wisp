//! The settings window: a sidebar of pages. `--welcome` shows the one-time
//! onboarding instead.

use std::sync::Arc;

use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    select::{SearchableVec, Select, SelectEvent, SelectState},
    slider::{Slider, SliderEvent, SliderState},
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

/// Config values and the labels the dropdowns show for them.
const THEMES: [(&str, &str); 3] = [("system", "System"), ("dark", "Dark"), ("light", "Light")];
const BACKDROPS: [(&str, &str); 3] = [("acrylic", "Acrylic"), ("mica", "Mica"), ("solid", "Solid")];
const ONBOARDING_STEPS: usize = 3;
type Choices = SelectState<SearchableVec<&'static str>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Appearance,
    Search,
    About,
}

impl Page {
    const ALL: [Self; 4] = [Self::General, Self::Appearance, Self::Search, Self::About];

    fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Search => "Search",
            Self::About => "About",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::General => IconName::Settings,
            Self::Appearance => IconName::Palette,
            Self::Search => IconName::Search,
            Self::About => IconName::Info,
        }
    }
}

pub struct Settings {
    store: Option<Store>,
    config: Config,
    error: Option<String>,
    shortcut_error: Option<String>,
    url_error: Option<String>,
    folder_error: Option<String>,
    page: Page,
    /// `Some(step)` while the one-time onboarding is showing.
    onboarding: Option<usize>,
    shortcut: Entity<InputState>,
    url: Entity<InputState>,
    folder: Entity<InputState>,
    theme: Entity<Choices>,
    backdrop: Entity<Choices>,
    results: Entity<SliderState>,
    autostart: bool,
    powertoys: Result<bool, String>,
    recording: bool,
    focus: FocusHandle,
    icon: Arc<Image>,
}

fn labels(choices: &[(&'static str, &'static str)]) -> Vec<&'static str> {
    choices.iter().map(|(_, label)| *label).collect()
}

fn index_of(choices: &[(&str, &str)], value: &str) -> Option<IndexPath> {
    choices.iter().position(|(v, _)| *v == value).map(IndexPath::new)
}

fn value_of(choices: &[(&'static str, &'static str)], label: &str) -> Option<&'static str> {
    choices.iter().find(|(_, l)| *l == label).map(|(v, _)| *v)
}

impl Settings {
    pub fn new(welcome: bool, page: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
        let folder = cx.new(|cx| InputState::new(window, cx).placeholder("Folder name"));
        let theme = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(labels(&THEMES)),
                index_of(&THEMES, &config.theme),
                window,
                cx,
            )
        });
        let backdrop = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(labels(&BACKDROPS)),
                index_of(&BACKDROPS, &config.backdrop),
                window,
                cx,
            )
        });
        let results = cx.new(|_| {
            SliderState::new()
                .min(1.)
                .max(20.)
                .step(1.)
                .default_value(config.max_results as f32)
        });
        let focus = cx.focus_handle();

        cx.subscribe_in(&shortcut, window, |this, input, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().to_string();
                this.shortcut_error = hotkey::parse(&value).err();
                cx.notify();
            } else if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                this.commit_shortcut(window, cx);
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
        cx.subscribe_in(&theme, window, |this, _, event, window, cx| {
            if let SelectEvent::Confirm(Some(label)) = event
                && let Some(value) = value_of(&THEMES, label)
            {
                this.save("theme", value.into(), window, cx);
            }
        })
        .detach();
        cx.subscribe_in(&backdrop, window, |this, _, event, window, cx| {
            if let SelectEvent::Confirm(Some(label)) = event
                && let Some(value) = value_of(&BACKDROPS, label)
            {
                this.save("backdrop", value.into(), window, cx);
            }
        })
        .detach();
        cx.subscribe_in(&results, window, |this, _, event, window, cx| {
            if let SliderEvent::Change(value) = event {
                let value = (value.start().round() as i64).clamp(1, 20);
                if value != this.config.max_results {
                    this.save("max_results", value.into(), window, cx);
                }
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
            page: Page::ALL
                .into_iter()
                .find(|p| page.is_some_and(|name| p.title().eq_ignore_ascii_case(name)))
                .unwrap_or(Page::General),
            onboarding: welcome.then_some(0),
            shortcut,
            url,
            folder,
            theme,
            backdrop,
            results,
            autostart: system::autostart_enabled(),
            powertoys: system::powertoys_run_enabled(),
            recording: false,
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
        let dark = Theme::global(cx).mode.is_dark();
        let (background, text) = if dark {
            (0x000000, 0xf5f5f5)
        } else {
            (0xfaf9f6, 0x111111)
        };
        let title = if self.onboarding.is_some() {
            "Welcome to Wisp"
        } else {
            "Wisp Settings"
        };
        system::style_title_bar(title, dark, background, text);
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
                let theme = index_of(&THEMES, &self.config.theme);
                self.theme
                    .update(cx, |state, cx| state.set_selected_index(theme, window, cx));
                let backdrop = index_of(&BACKDROPS, &self.config.backdrop);
                self.backdrop
                    .update(cx, |state, cx| state.set_selected_index(backdrop, window, cx));
                let results = self.config.max_results as f32;
                self.results
                    .update(cx, |state, cx| state.set_value(results, window, cx));
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
            self.shortcut_error = hotkey::parse(&self.shortcut.read(cx).value()).err();
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

    fn shortcut_control(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(Input::new(&self.shortcut).aria_label("Shortcut").flex_1())
                    .child(
                        Button::new("record")
                            .label(if self.recording {
                                "Press a shortcut…"
                            } else {
                                "Record"
                            })
                            .when(self.recording, |b| b.primary())
                            .on_click(cx.listener(|this, _, window, cx| this.start_recording(window, cx))),
                    ),
            )
            .child(hint(
                "Type a shortcut and press Enter, or select Record and press it. Esc cancels.",
                p,
            ))
            .children(self.shortcut_error.as_ref().map(|e| error_text(e, p)))
    }

    fn conflict_warning(&self, p: Palette) -> Option<Div> {
        let enabled = self.powertoys.as_ref().copied().unwrap_or(false);
        system::has_shortcut_conflict(&self.config.hotkey, enabled).then(|| {
            note(
                "PowerToys Run is on and also uses Alt+Space. Turn it off in PowerToys Settings, or choose another shortcut.",
                p.warn,
                p,
            )
        })
    }

    fn autostart_switch(&self, cx: &mut Context<Self>) -> Switch {
        Switch::new("autostart")
            .checked(self.autostart)
            .accessibility_label("Start with Windows")
            .on_change(cx.listener(|this, enabled: &bool, _, cx| this.set_autostart(*enabled, cx)))
    }

    fn general(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_2()
            .child(label("Launcher", p))
            .child(
                group(p).child(
                    item("Shortcut", "Shows or hides the launcher.", p)
                        .child(self.shortcut_control(p, cx))
                        .children(self.conflict_warning(p)),
                ),
            )
            .child(div().h_3())
            .child(label("Startup", p))
            .child(group(p).child(row(
                "Start with Windows",
                "Open Wisp when you sign in.",
                self.autostart_switch(cx),
                p,
            )))
            .into_any_element()
    }

    fn appearance(&self, p: Palette, _cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_2()
            .child(label("Look", p))
            .child(
                group(p)
                    .child(row(
                        "Theme",
                        "Light, dark, or the same as Windows.",
                        Select::new(&self.theme).accessibility_label("Theme").w(px(160.)),
                        p,
                    ))
                    .child(divider(p))
                    .child(row(
                        "Backdrop",
                        "Acrylic and mica blur what is behind the launcher and fade in. Solid appears at once.",
                        Select::new(&self.backdrop).accessibility_label("Backdrop").w(px(160.)),
                        p,
                    )),
            )
            .child(div().h_3())
            .child(label("Results", p))
            .child(
                group(p).child(
                    item(
                        "Visible results",
                        format!(
                            "{} results visible at once. Scroll to see more.",
                            self.config.max_results
                        ),
                        p,
                    )
                    .child(Slider::new(&self.results).accessibility_label("Results visible at once")),
                ),
            )
            .into_any_element()
    }

    fn search(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let folders = v_flex().rounded_md().border_1().border_color(p.border).children(
            self.config.file_search_exclude.iter().enumerate().map(|(index, name)| {
                h_flex()
                    .justify_between()
                    .px_3()
                    .py_1()
                    .when(index > 0, |r| r.border_t_1().border_color(p.border))
                    .child(name.clone())
                    .child(
                        Button::new(("remove-folder", index))
                            .small()
                            .ghost()
                            .label("Remove")
                            .accessibility_label(format!("Remove {name}"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let mut folders = this.config.file_search_exclude.clone();
                                if index < folders.len() {
                                    folders.remove(index);
                                    this.save_folders(folders, window, cx);
                                }
                            })),
                    )
            }),
        );
        v_flex()
            .gap_2()
            .child(label("Apps and web", p))
            .child(
                group(p)
                    .child(row(
                        "Recent apps",
                        "Show recently opened apps when the search box is empty.",
                        Switch::new("recent")
                            .checked(self.config.show_recent)
                            .accessibility_label("Show recent apps")
                            .on_change(cx.listener(|this, enabled: &bool, window, cx| {
                                this.save("show_recent", (*enabled).into(), window, cx);
                            })),
                        p,
                    ))
                    .child(divider(p))
                    .child(
                        item("Web search URL", "Wisp replaces {query} with your search text.", p)
                            .child(Input::new(&self.url).aria_label("Web search URL"))
                            .children(self.url_error.as_ref().map(|e| error_text(e, p))),
                    ),
            )
            .child(div().h_3())
            .child(label("Files", p))
            .child(
                group(p)
                    .child(row(
                        "File search",
                        "Search file names in the Windows Search index. Start a query with ? to search only files.",
                        Switch::new("file-search")
                            .checked(self.config.file_search)
                            .accessibility_label("File search")
                            .on_change(cx.listener(|this, enabled: &bool, window, cx| {
                                this.save("file_search", (*enabled).into(), window, cx);
                            })),
                        p,
                    ))
                    .child(divider(p))
                    .child(
                        item(
                            "Skipped folders",
                            "File search skips folders with these names, and every name that starts with a dot.",
                            p,
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
                        .child(folders),
                    ),
            )
            .into_any_element()
    }

    fn about(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let path = self
            .store
            .as_ref()
            .map(|store| store.path.display().to_string())
            .unwrap_or_else(|| "Config folder unavailable".into());
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(img(self.icon.clone()).size(px(40.)))
                    .child(
                        v_flex()
                            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child("Wisp"))
                            .child(hint(format!("Version {}", env!("CARGO_PKG_VERSION")), p)),
                    ),
            )
            .child(
                group(p)
                    .child(row(
                        "Config file",
                        path,
                        Button::new("open-config")
                            .label("Open")
                            .on_click(cx.listener(|this, _, _, cx| this.open_config(cx))),
                        p,
                    ))
                    .child(divider(p))
                    .child(row(
                        "Source code",
                        "github.com/SumirSeth/wisp",
                        Button::new("github")
                            .label("Open")
                            .on_click(|_, _, cx| cx.open_url("https://github.com/SumirSeth/wisp")),
                        p,
                    )),
            )
            .into_any_element()
    }

    fn settings_view(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let page = match self.page {
            Page::General => self.general(p, cx),
            Page::Appearance => self.appearance(p, cx),
            Page::Search => self.search(p, cx),
            Page::About => self.about(p, cx),
        };
        let error = self.error.as_ref().map(|error| {
            note(error.clone(), p.danger, p).child(
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
        h_flex()
            .size_full()
            .items_stretch()
            .child(
                v_flex()
                    .w(px(196.))
                    .h_full()
                    .flex_none()
                    .px_3()
                    .py_4()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .px_2()
                            .pb_4()
                            .child(img(self.icon.clone()).size(px(22.)))
                            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child("wisp")),
                    )
                    .children(
                        Page::ALL
                            .into_iter()
                            .map(|page| nav_item(page, self.page == page, p, cx)),
                    ),
            )
            .child(
                v_flex()
                    .id("page")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px_8()
                    .py_6()
                    .gap_4()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.page.title()),
                    )
                    .children(error)
                    .child(page),
            )
            .into_any_element()
    }

    fn onboarding_view(&self, step: usize, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let shortcut =
            group(p).child(item("Shortcut", "Shows or hides the launcher.", p).child(self.shortcut_control(p, cx)));
        let (title, body): (&str, Div) = match step {
            0 => (
                "Welcome to Wisp",
                v_flex()
                    .gap_4()
                    .child("Press the shortcut, type a few letters of an app or file, and press Enter.")
                    .child(shortcut),
            ),
            1 => (
                "Check the shortcut",
                v_flex()
                    .gap_4()
                    .child(match self.conflict_warning(p) {
                        Some(warning) => warning,
                        None => hint(
                            "PowerToys Run does not use this shortcut. If another app takes it, Wisp shows a notification.",
                            p,
                        ),
                    })
                    .child(shortcut),
            ),
            _ => (
                "Start with Windows",
                v_flex()
                    .gap_4()
                    .child(group(p).child(row(
                        "Start with Windows",
                        "Open Wisp when you sign in.",
                        self.autostart_switch(cx),
                        p,
                    )))
                    .child(hint(
                        "To change settings later, right-click the Wisp icon in the notification area and select Settings.",
                        p,
                    )),
            ),
        };
        let last = step + 1 == ONBOARDING_STEPS;
        v_flex()
            .size_full()
            .px_8()
            .py_6()
            .gap_5()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(img(self.icon.clone()).size(px(28.)))
                    .child(div().flex_1())
                    .child(h_flex().gap_1().children((0..ONBOARDING_STEPS).map(|i| {
                        div()
                            .h(px(3.))
                            .w(px(24.))
                            .rounded_full()
                            .bg(if i <= step { p.accent } else { p.border })
                    }))),
            )
            .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().flex_1().child(body))
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .when(step > 0, |row| {
                        row.child(
                            Button::new("back")
                                .label("Back")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.recording = false;
                                    this.onboarding = this.onboarding.map(|s| s.saturating_sub(1));
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("next")
                            .primary()
                            .label(if last { "Done" } else { "Next" })
                            .disabled(self.shortcut_error.is_some())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.recording = false;
                                if !last && !this.commit_shortcut(window, cx) {
                                    return;
                                }
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
        div()
            .size_full()
            .bg(p.bg)
            .text_color(p.fg)
            .font_family(cx.theme().font_family.clone())
            .text_sm()
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::record))
            .child(content)
    }
}

/// A sidebar entry. The active page gets the launcher's selection marker: a
/// short cyan bar at the left edge.
fn nav_item(page: Page, active: bool, p: Palette, cx: &mut Context<Settings>) -> Stateful<Div> {
    h_flex()
        .id(page.title())
        .relative()
        .gap_3()
        .items_center()
        .h(px(34.))
        .px_3()
        .rounded_md()
        .cursor_pointer()
        .text_color(if active { p.fg } else { p.muted })
        .when(active, |item| item.bg(p.surface))
        .when(!active, |item| item.hover(|s| s.bg(p.surface).text_color(p.fg)))
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(10.))
                .h(px(14.))
                .w(px(3.))
                .rounded_full()
                .when(active, |bar| bar.bg(p.accent)),
        )
        .child(Icon::new(page.icon()).size_4())
        .child(page.title())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.page = page;
            this.recording = false;
            this.autostart = system::autostart_enabled();
            cx.notify();
        }))
}

/// A small label above a group of settings.
fn label(text: &'static str, p: Palette) -> Div {
    div()
        .px_1()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.muted)
        .child(text.to_uppercase())
}

/// A card that holds related settings, separated by dividers.
fn group(p: Palette) -> Div {
    v_flex().rounded_lg().border_1().border_color(p.border).bg(p.surface)
}

fn divider(p: Palette) -> Div {
    div().h(px(1.)).bg(p.border)
}

/// A setting whose control sits below its title.
fn item(title: &'static str, description: impl Into<SharedString>, p: Palette) -> Div {
    v_flex()
        .gap_2()
        .px_4()
        .py_3()
        .child(v_flex().child(div().child(title)).child(hint(description, p)))
}

/// A setting whose control sits at the right. The control keeps its own width.
fn row(title: &'static str, description: impl Into<SharedString>, control: impl IntoElement, p: Palette) -> Div {
    h_flex()
        .gap_4()
        .px_4()
        .py_3()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().child(title))
                .child(hint(description, p)),
        )
        .child(div().flex_none().child(control))
}

fn note(text: impl Into<SharedString>, color: Hsla, p: Palette) -> Div {
    v_flex()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .border_l_2()
        .border_color(color)
        .bg(p.surface)
        .child(text.into())
}

fn hint(text: impl Into<SharedString>, p: Palette) -> Div {
    div().text_xs().text_color(p.muted).child(text.into())
}

fn error_text(text: &str, p: Palette) -> Div {
    div().text_xs().text_color(p.danger).child(text.to_owned())
}
