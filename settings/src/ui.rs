use gpui_kit::component::{
    alert::Alert,
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    select::{SearchableVec, Select, SelectEvent, SelectState},
    sidebar::{Sidebar, SidebarMenu, SidebarMenuItem},
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
};
use gpui_kit::{component::*, prelude::*, *};
use toml_edit::{Array, Value};

use crate::{
    config::{self, Config, Store},
    hotkey, system,
};

const THEMES: [&str; 3] = ["system", "dark", "light"];
const BACKDROPS: [&str; 3] = ["acrylic", "mica", "solid"];
type Choices = SelectState<SearchableVec<&'static str>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Welcome,
    General,
    Appearance,
    Search,
    About,
}

impl Page {
    const ALL: [Self; 5] = [
        Self::Welcome,
        Self::General,
        Self::Appearance,
        Self::Search,
        Self::About,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Search => "Search",
            Self::About => "About",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Welcome => IconName::Star,
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
    step: usize,
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
        let folder = cx.new(|cx| InputState::new(window, cx).placeholder("Folder name"));
        let theme = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(THEMES.to_vec()),
                THEMES
                    .iter()
                    .position(|value| *value == config.theme)
                    .map(IndexPath::new),
                window,
                cx,
            )
        });
        let backdrop = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(BACKDROPS.to_vec()),
                BACKDROPS
                    .iter()
                    .position(|value| *value == config.backdrop)
                    .map(IndexPath::new),
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
            if let SelectEvent::Confirm(Some(value)) = event {
                this.save("theme", (*value).into(), window, cx);
            }
        })
        .detach();
        cx.subscribe_in(&backdrop, window, |this, _, event, window, cx| {
            if let SelectEvent::Confirm(Some(value)) = event {
                this.save("backdrop", (*value).into(), window, cx);
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
            page: if welcome { Page::Welcome } else { Page::General },
            step: 0,
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
                self.shortcut
                    .update(cx, |input, cx| input.set_value(self.config.hotkey.clone(), window, cx));
                self.url.update(cx, |input, cx| {
                    input.set_value(self.config.search_url.clone(), window, cx)
                });
                self.theme.update(cx, |state, cx| {
                    state.set_selected_index(
                        THEMES
                            .iter()
                            .position(|value| *value == self.config.theme)
                            .map(IndexPath::new),
                        window,
                        cx,
                    )
                });
                self.backdrop.update(cx, |state, cx| {
                    state.set_selected_index(
                        BACKDROPS
                            .iter()
                            .position(|value| *value == self.config.backdrop)
                            .map(IndexPath::new),
                        window,
                        cx,
                    )
                });
                self.results.update(cx, |state, cx| {
                    state.set_value(self.config.max_results as f32, window, cx)
                });
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
        let modifiers = key.modifiers;
        match hotkey::recorded(
            &key.key,
            modifiers.control,
            modifiers.alt,
            modifiers.shift,
            modifiers.platform,
        ) {
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

    fn shortcut_control(&self, cx: &mut Context<Self>) -> Div {
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
                                "Record shortcut"
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.start_recording(window, cx))),
                    ),
            )
            .child(hint(
                "Record a shortcut, or type one and press Enter. Press Esc to cancel recording.",
                cx,
            ))
            .children(self.shortcut_error.as_ref().map(|error| field_error(error, cx)))
    }

    fn autostart_control(&self, cx: &mut Context<Self>) -> Div {
        row(
            "Start with Windows",
            "Open Wisp when you sign in.",
            Switch::new("autostart")
                .checked(self.autostart)
                .accessibility_label("Start with Windows")
                .on_change(cx.listener(|this, enabled, _, cx| this.set_autostart(*enabled, cx))),
            cx,
        )
    }

    fn general(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_5()
            .child(section(
                "Shortcut",
                "Show or hide the launcher.",
                self.shortcut_control(cx),
                cx,
            ))
            .child(self.autostart_control(cx))
            .into_any_element()
    }

    fn appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_5()
            .child(row(
                "Theme",
                "Choose the app's colors.",
                Select::new(&self.theme).accessibility_label("Theme").w(px(170.)),
                cx,
            ))
            .child(row(
                "Backdrop",
                "Acrylic and mica fade in. Solid is fastest.",
                Select::new(&self.backdrop).accessibility_label("Backdrop").w(px(170.)),
                cx,
            ))
            .child(section(
                "Maximum results",
                "Choose how many results appear at once.",
                v_flex()
                    .gap_2()
                    .child(Slider::new(&self.results).accessibility_label("Maximum results"))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(hint("1", cx))
                            .child(format!("{} results", self.config.max_results))
                            .child(hint("20", cx)),
                    ),
                cx,
            ))
            .into_any_element()
    }

    fn search(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_5()
            .child(row(
                "Recent apps",
                "Show recent apps when the search box is empty.",
                Switch::new("recent")
                    .checked(self.config.show_recent)
                    .accessibility_label("Show recent apps")
                    .on_change(cx.listener(|this, enabled: &bool, window, cx| {
                        this.save("show_recent", (*enabled).into(), window, cx);
                    })),
                cx,
            ))
            .child(row(
                "File search",
                "Search file names in the Windows Search index.",
                Switch::new("file-search")
                    .checked(self.config.file_search)
                    .accessibility_label("File search")
                    .on_change(cx.listener(|this, enabled: &bool, window, cx| {
                        this.save("file_search", (*enabled).into(), window, cx);
                    })),
                cx,
            ))
            .child(section(
                "Web search URL",
                "Wisp replaces {query} with your search text.",
                v_flex()
                    .gap_2()
                    .child(Input::new(&self.url).aria_label("Web search URL"))
                    .children(self.url_error.as_ref().map(|error| field_error(error, cx))),
                cx,
            ))
            .child(section(
                "Excluded folders",
                "Skip these folder names. Wisp also skips names that start with a dot.",
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Input::new(&self.folder).aria_label("Excluded folder name").flex_1())
                            .child(
                                Button::new("add-folder")
                                    .label("Add")
                                    .on_click(cx.listener(|this, _, window, cx| this.add_folder(window, cx))),
                            ),
                    )
                    .children(self.folder_error.as_ref().map(|error| field_error(error, cx)))
                    .children(self.config.file_search_exclude.iter().enumerate().map(|(index, name)| {
                        h_flex().justify_between().gap_3().child(name.clone()).child(
                            Button::new(("remove-folder", index))
                                .small()
                                .ghost()
                                .label(format!("Remove {name}"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let mut folders = this.config.file_search_exclude.clone();
                                    if index < folders.len() {
                                        folders.remove(index);
                                        this.save_folders(folders, window, cx);
                                    }
                                })),
                        )
                    })),
                cx,
            ))
            .into_any_element()
    }

    fn welcome(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut content = v_flex().gap_4().child(hint(format!("Step {} of 3", self.step + 1), cx));
        content = match self.step {
            0 => content
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Your apps, one shortcut away."),
                )
                .child("Wisp finds apps, files, and web results. Press your shortcut and start typing.")
                .child(section(
                    "Your shortcut",
                    format!("Current shortcut: {}", self.config.hotkey),
                    self.shortcut_control(cx),
                    cx,
                )),
            1 => {
                let enabled = self.powertoys.as_ref().copied().unwrap_or(false);
                let conflict = system::has_shortcut_conflict(&self.config.hotkey, enabled);
                content.child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("Check your shortcut"))
                    .child(if conflict {
                        Alert::warning("conflict", "PowerToys Run is enabled or running. It uses Alt+Space by default. Pick another Wisp shortcut.").into_any_element()
                    } else if self.powertoys.is_err() {
                        Alert::info("conflict", "Choose a shortcut that other launchers do not use.").into_any_element()
                    } else {
                        Alert::info("conflict", "No PowerToys Run conflict found for this shortcut. Other apps may still use it.").into_any_element()
                    })
                    .children(self.powertoys.as_ref().err().map(|error| Alert::warning("powertoys-error", format!("Could not finish the PowerToys check: {error}"))))
                    .child(self.shortcut_control(cx))
            }
            _ => content
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Ready when you are"),
                )
                .child("Choose whether Wisp starts when you sign in. You can change this later.")
                .child(self.autostart_control(cx)),
        };
        content
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .when(self.step == 1, |this| {
                        this.child(Button::new("check-again").label("Check again").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.powertoys = system::powertoys_run_enabled();
                                cx.notify();
                            },
                        )))
                    })
                    .when(self.step > 0, |this| {
                        this.child(
                            Button::new("back")
                                .label("Back")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.recording = false;
                                    this.step = this.step.saturating_sub(1);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("next")
                            .primary()
                            .label(if self.step == 2 { "Done" } else { "Continue" })
                            .disabled(self.step < 2 && self.shortcut_error.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.recording = false;
                                if this.step < 2 && !this.commit_shortcut(window, cx) {
                                    return;
                                }
                                if this.step == 2 {
                                    window.remove_window();
                                } else {
                                    this.step += 1;
                                    if this.step == 1 {
                                        this.powertoys = system::powertoys_run_enabled();
                                    }
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .into_any_element()
    }

    fn about(&self, cx: &mut Context<Self>) -> AnyElement {
        let path = self
            .store
            .as_ref()
            .map(|store| store.path.display().to_string())
            .unwrap_or_else(|| "Config folder unavailable".into());
        v_flex()
            .gap_5()
            .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("Wisp"))
            .child(hint(format!("Version {}", env!("CARGO_PKG_VERSION")), cx))
            .child("An app launcher for Windows.")
            .child(
                Button::new("github")
                    .label("View Wisp on GitHub")
                    .on_click(|_, _, cx| cx.open_url("https://github.com/SumirSeth/wisp")),
            )
            .child(section(
                "Config file",
                path,
                Button::new("open-config")
                    .label("Open config file")
                    .on_click(cx.listener(|this, _, _, cx| this.open_config(cx))),
                cx,
            ))
            .into_any_element()
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = match self.page {
            Page::Welcome => self.welcome(cx),
            Page::General => self.general(cx),
            Page::Appearance => self.appearance(cx),
            Page::Search => self.search(cx),
            Page::About => self.about(cx),
        };
        h_flex()
            .size_full()
            .items_stretch()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .font_family(cx.theme().font_family.clone())
            .text_sm()
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::record))
            .child(
                Sidebar::new("navigation")
                    .w(px(172.))
                    .h_full()
                    .collapsible(false)
                    .header(
                        v_flex()
                            .gap_1()
                            .p_3()
                            .child(div().text_xl().font_weight(FontWeight::BOLD).child("Wisp"))
                            .child(hint("Settings", cx)),
                    )
                    .child(SidebarMenu::new().children(Page::ALL.into_iter().map(|page| {
                        SidebarMenuItem::new(page.title())
                            .icon(page.icon())
                            .active(self.page == page)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.page = page;
                                this.recording = false;
                                this.autostart = system::autostart_enabled();
                                cx.notify();
                            }))
                    })))
                    .footer(div().p_3().child(hint(format!("v{}", env!("CARGO_PKG_VERSION")), cx))),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        v_flex()
                            .p_5()
                            .gap_2()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.page.title()),
                            )
                            .child(hint("Changes apply immediately.", cx)),
                    )
                    .child(
                        v_flex()
                            .id("page-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_5()
                            .gap_4()
                            .children(self.error.as_ref().map(|error| {
                                v_flex()
                                    .gap_2()
                                    .child(Alert::error("config-error", error.clone()))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                Button::new("fix-config")
                                                    .small()
                                                    .label("Open config file")
                                                    .on_click(cx.listener(|this, _, _, cx| this.open_config(cx))),
                                            )
                                            .child(
                                                Button::new("reload").small().label("Reload config").on_click(
                                                    cx.listener(|this, _, window, cx| this.reload(window, cx)),
                                                ),
                                            ),
                                    )
                            }))
                            .child(page),
                    ),
            )
    }
}

fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn field_error(text: &str, cx: &App) -> Div {
    div().text_sm().text_color(cx.theme().danger).child(text.to_owned())
}

fn section(title: &str, description: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    v_flex()
        .gap_3()
        .p_4()
        .border_1()
        .border_color(cx.theme().border)
        .rounded_lg()
        .child(
            v_flex()
                .gap_1()
                .child(div().font_weight(FontWeight::MEDIUM).child(title.to_owned()))
                .child(hint(description, cx)),
        )
        .child(control)
}

fn row(title: &str, description: &str, control: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .gap_4()
        .justify_between()
        .p_4()
        .border_1()
        .border_color(cx.theme().border)
        .rounded_lg()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().font_weight(FontWeight::MEDIUM).child(title.to_owned()))
                .child(hint(description.to_owned(), cx)),
        )
        .child(control)
}
