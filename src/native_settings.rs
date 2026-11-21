//! Native Settings: no worker, timer or rendering resources survive WM_NCDESTROY.
#[path = "native_settings/controls.rs"]
mod controls;
#[path = "native_settings/pages.rs"]
mod pages;
use crate::{
    editor::Editor,
    settings_config::{Config, Store},
    settings_system as system,
};
use controls::{Control, Kind};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use windows::Win32::Graphics::DirectWrite::{IDWriteFactory, IDWriteTextFormat};
use windows::Win32::{
    Foundation::*,
    Graphics::{Direct2D::Common::*, Direct2D::*, Gdi::*},
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
use windows::core::{PCWSTR, Result};

thread_local! {
    static WINDOW: RefCell<Option<Settings>> = const { RefCell::new(None) };
    static HANDLE: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
    static PENDING_PAGE: Cell<Option<usize>> = const { Cell::new(None) };
}
const PAGES: [&str; 4] = ["General", "Appearance", "Search", "About"];
pub fn page_arg(args: &[String]) -> usize {
    if args.iter().any(|a| a == "--welcome") {
        return 4;
    }
    args.iter()
        .position(|a| a == "--page")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| PAGES.iter().position(|p| p.eq_ignore_ascii_case(s)))
        .unwrap_or(0)
}
struct Settings {
    hwnd: HWND,
    renderer: crate::render::Renderer,
    logo: Option<crate::indexer::IconPixels>,
    store: Store,
    config: Config,
    controls: Vec<Control>,
    focus: Option<usize>,
    scroll: f32,
    extent: f32,
    page: usize,
    editor: Editor,
    recording: bool,
    dragging: Option<String>,
    error: Option<String>,
    autostart: bool,
    conflict: bool,
    preview: bool,
    width: f32,
    height: f32,
    folder: String,
    advanced: bool,
    picker: bool,
    color: String,
    pending_color: Option<String>,
    welcome: Option<usize>,
    hover: Option<usize>,
    pressed: Option<usize>,
    formats: std::cell::OnceCell<Formats>,
}
/// Settings' own type scale, matching the GPUI app it replaces.
struct Formats {
    body: IDWriteTextFormat,
    center: IDWriteTextFormat,
    hint: IDWriteTextFormat,
    section: IDWriteTextFormat,
    title: IDWriteTextFormat,
    brand: IDWriteTextFormat,
    chevron: IDWriteTextFormat,
    trailing: IDWriteTextFormat,
}
impl Formats {
    fn new(dw: &IDWriteFactory) -> Result<Self> {
        use windows::Win32::Graphics::DirectWrite::*;
        let fonts = crate::render::Fonts::installed(dw);
        unsafe {
            let f = |family, size, weight| crate::render::text_format(dw, family, size, weight);
            let center = f(fonts.text, 13.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            center.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            let chevron = f(fonts.icons, 10.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            chevron.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            let trailing = f(fonts.text, 14.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            trailing.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            Ok(Self {
                body: f(fonts.text, 14.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                center,
                hint: f(fonts.text, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                section: f(fonts.text, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                title: f(fonts.display, 20.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                brand: f(fonts.display, 17.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                chevron,
                trailing,
            })
        }
    }
}
/// A registered chord is delivered to the launcher even while Settings records.
pub fn record_hotkey(lp: LPARAM) -> bool {
    WINDOW.with(|w| {
        let Ok(mut w) = w.try_borrow_mut() else { return false };
        let Some(s) = w.as_mut().filter(|s| s.recording) else {
            return false;
        };
        s.key(((lp.0 as u32) >> 16) as u16);
        unsafe {
            let _ = InvalidateRect(Some(s.hwnd), None, false);
        }
        true
    })
}
pub fn open(page: usize) -> Result<()> {
    open_path(page, crate::config::Config::path(), false)
}
fn open_path(page: usize, path: PathBuf, preview: bool) -> Result<()> {
    let existing = HANDLE.with(|h| {
        let hwnd = h.get();
        (!hwnd.is_invalid()).then_some(hwnd)
    });
    if let Some(hwnd) = existing {
        PENDING_PAGE.with(|p| p.set(Some(page)));
        WINDOW.with(|w| {
            if let Ok(mut w) = w.try_borrow_mut()
                && let Some(s) = w.as_mut()
            {
                s.page = page.min(3);
                s.welcome = (page == 4).then_some(0);
                s.scroll = 0.;
                s.rebuild();
            }
        });
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        return Ok(());
    }
    let store = Store::new(path);
    let loaded = store.load();
    let (config, error) = match loaded {
        Ok(c) => (c, None),
        Err(e) => (Config::default(), Some(e)),
    };
    let renderer = crate::render::Renderer::new()?;
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = crate::channel::SETTINGS_CLASS;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: LoadIconW(Some(instance.into()), PCWSTR(1 as _)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let dpi = GetDpiForSystem();
        let scale = dpi as f32 / 96.;
        let mut bounds = RECT {
            left: 0,
            top: 0,
            right: (if page == 4 { 560. } else { 820. } * scale) as i32,
            bottom: (if page == 4 { 460. } else { 580. } * scale) as i32,
        };
        AdjustWindowRectExForDpi(&mut bounds, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE::default(), dpi)?;
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        let title = crate::util::wide(&format!("{} Settings", crate::channel::NAME));
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            (GetSystemMetrics(SM_CXSCREEN) - width) / 2,
            (GetSystemMetrics(SM_CYSCREEN) - height) / 2,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let mut state = Settings {
            hwnd,
            renderer,
            logo: crate::indexer::resource_icon(),
            store,
            config,
            controls: Vec::new(),
            focus: None,
            scroll: 0.,
            extent: 0.,
            page: page.min(3),
            editor: Editor::default(),
            recording: false,
            dragging: None,
            error,
            autostart: system::autostart_enabled(),
            conflict: system::powertoys_run_enabled().unwrap_or(false),
            preview,
            width: 820.,
            height: 580.,
            folder: String::new(),
            advanced: false,
            picker: false,
            color: "#1E3A8ABF".into(),
            pending_color: None,
            welcome: (page == 4).then_some(0),
            hover: None,
            pressed: None,
            formats: std::cell::OnceCell::new(),
        };
        state.renderer.set_dpi(GetDpiForWindow(hwnd));
        state.rebuild();
        state.style();
        HANDLE.with(|h| h.set(hwnd));
        WINDOW.with(|w| *w.borrow_mut() = Some(state));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    Ok(())
}
/// Isolated visual harness: bypasses launcher startup, mutex, hotkeys and taskbar changes.
pub fn preview(args: &[String]) -> Result<()> {
    let path = args
        .iter()
        .position(|s| s == "--config")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .ok_or_else(|| windows::core::Error::new(E_INVALIDARG, "Preview requires --config PATH"))?;
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    open_path(page_arg(args), path, true)?;
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
impl Settings {
    fn style(&self) {
        let dark = match self.config.theme.as_str() {
            "dark" => true,
            "light" => false,
            _ => !crate::system::system_uses_light_theme(),
        };
        crate::theme::apply_backdrop(self.hwnd, dark, crate::config::Backdrop::Mica, false, None);
    }
    fn rebuild(&mut self) {
        let focused_id = self.focus.and_then(|i| self.controls.get(i)).map(|c| c.id.clone());
        let c = &self.config;
        let mut rows = Vec::new();
        for (i, p) in PAGES.iter().enumerate() {
            rows.push(Control::new(
                &format!("nav:{i}"),
                p,
                "",
                Kind::Link(p.to_string()),
                true,
            ));
        }
        if self.welcome.is_some() || self.page != 0 {
            self.page_rows(&mut rows);
        } else {
            let mut add = |id: &str, label: &str, hint: &str, kind: Kind, enabled: bool| {
                rows.push(Control::new(id, label, hint, kind, enabled))
            };
            add("launcher", "LAUNCHER", "", Kind::Heading, true);
            add(
                "hotkey",
                "Shortcut",
                if c.win_key {
                    "Turn off Use Win key to choose a shortcut."
                } else {
                    ""
                },
                Kind::Text(c.hotkey.clone()),
                !c.win_key,
            );
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
                !c.win_key,
            );
            if !c.win_key && system::has_shortcut_conflict(&c.hotkey, self.conflict) {
                add(
                    "warning",
                    "PowerToys Run also uses Alt+Space.",
                    "Turn it off in PowerToys Settings, or choose another shortcut.",
                    Kind::Heading,
                    true,
                );
            }
            add(
                "win_key",
                "Use Win key",
                "Tap the Windows key alone to open Wisp instead of Start.",
                Kind::Toggle(c.win_key),
                true,
            );
            add("dock-group", "DOCK", "", Kind::Heading, true);
            add("dock", "Show the dock", "", Kind::Toggle(c.dock), true);
            add(
                "dock_autohide",
                "Keep the dock visible",
                "While the taskbar is hidden, windows stop above the dock.",
                Kind::Toggle(!c.dock_autohide),
                c.dock,
            );
            add(
                "dock_dodge_windows",
                "Hide only when windows overlap",
                "",
                Kind::Toggle(c.dock_dodge_windows),
                c.dock && c.dock_autohide,
            );
            let mut screens = vec![
                ("pointer".into(), "Follow the pointer".into()),
                ("main".into(), "Main screen".into()),
            ];
            screens.extend(crate::screens::list().into_iter().map(|s| (s.id, s.name)));
            if !c.dock_screen.is_empty() && !screens.iter().any(|(v, _)| v.eq_ignore_ascii_case(&c.dock_screen)) {
                screens.push((c.dock_screen.clone(), "A screen that isn't connected".into()));
            }
            let selected = screens
                .iter()
                .position(|(v, _)| v.eq_ignore_ascii_case(&c.dock_screen))
                .unwrap_or(0);
            add(
                "dock_screen",
                "Screen",
                if !c.dock_autohide && matches!(c.dock_screen.as_str(), "" | "pointer") {
                    "Uses the main screen while the dock stays visible."
                } else {
                    ""
                },
                Kind::Select {
                    choices: screens,
                    selected,
                },
                c.dock,
            );
            add(
                "dock_search_button",
                "Search button",
                "",
                Kind::Toggle(c.dock_search_button),
                c.dock,
            );
            add(
                "dock_icon_size",
                "App icon size",
                &format!("{} DIP", c.dock_icon_size),
                Kind::Slider {
                    value: c.dock_icon_size,
                    min: 24,
                    max: 40,
                },
                true,
            );
            let pins = crate::dock::pins::load(&self.store.path.with_file_name("dock-pins.toml")).len();
            add(
                "unpin",
                "Pinned apps",
                &format!("{pins} pinned"),
                Kind::Button("Unpin all".into()),
                pins > 0,
            );
            let choices = vec![
                ("multiple".into(), "Only for multiple windows".into()),
                ("always".into(), "Always".into()),
                ("off".into(), "Off".into()),
            ];
            let selected = choices.iter().position(|(v, _)| v == &c.window_list).unwrap_or(0);
            add(
                "window_list",
                "App window list",
                "Lists an app's windows when you hover its icon.",
                Kind::Select { choices, selected },
                true,
            );
            add(
                "hide_taskbar",
                "Hide the taskbar",
                "The taskbar comes back when Wisp quits.",
                Kind::Toggle(c.hide_taskbar),
                true,
            );
            add(
                "position",
                "Launcher at the bottom",
                "",
                Kind::Toggle(c.position == "bottom"),
                true,
            );
            add("status", "STATUS", "", Kind::Heading, true);
            add(
                "group-status",
                "Group status icons",
                "",
                Kind::Toggle(!c.dock_status_group.is_empty()),
                !c.dock_status_group.is_empty() || !c.dock_status_icons.is_empty(),
            );
            for (name, title) in [
                ("sound", "Sound"),
                ("battery", "Battery"),
                ("wifi", "Wi-Fi"),
                ("bluetooth", "Bluetooth"),
            ] {
                let selected = if c.dock_status_group.iter().any(|s| s == name) {
                    0
                } else if c.dock_status_icons.iter().any(|s| s == name) {
                    1
                } else {
                    2
                };
                add(
                    &format!("status:{name}"),
                    title,
                    "",
                    Kind::Select {
                        choices: vec![
                            ("group".into(), "In group".into()),
                            ("icon".into(), "Own icon".into()),
                            ("hidden".into(), "Hidden".into()),
                        ],
                        selected,
                    },
                    true,
                );
            }
            add("updates", "UPDATES", "", Kind::Heading, true);
            add(
                "check_for_updates",
                "Check for updates",
                if crate::channel::NIGHTLY {
                    "Nightly builds skip checks."
                } else {
                    "Daily, except on metered connections."
                },
                Kind::Toggle(c.check_for_updates),
                true,
            );
            add("startup", "STARTUP", "", Kind::Heading, true);
            add(
                "autostart",
                "Start with Windows",
                "",
                Kind::Toggle(self.autostart),
                true,
            );
        }
        if let Some(error) = &self.error {
            rows.push(Control::new("error", error, "", Kind::Heading, true));
            rows.push(Control::new(
                "reload",
                "",
                "",
                Kind::Button("Reload config".into()),
                true,
            ));
            rows.push(Control::new(
                "open-config",
                "",
                "",
                Kind::Link("Open config file".into()),
                true,
            ));
        }
        if self.error.is_some() {
            let at = rows.len().saturating_sub(3);
            let error_rows: Vec<_> = rows.drain(at..).collect();
            rows.splice(4..4, error_rows);
        }
        if self.welcome.is_some() {
            for c in &mut rows[..4] {
                c.enabled = false;
            }
        }
        self.extent = controls::layout(&mut rows[4..], self.width);
        if self.welcome.is_some() {
            for c in &mut rows[4..] {
                if c.id != "record" {
                    c.rect.left = 32.;
                }
            }
        }
        for (i, c) in rows[..4].iter_mut().enumerate() {
            c.rect = D2D_RECT_F {
                left: 12.,
                top: 64. + i as f32 * 38.,
                right: 184.,
                bottom: 98. + i as f32 * 38.,
            };
        }
        self.focus = focused_id.and_then(|id| rows.iter().position(|c| c.id == id && c.focusable()));
        self.controls = rows;
        self.scroll = self.scroll.clamp(0., (self.extent - self.height).max(0.));
    }
    fn save(&mut self, key: &str, value: toml_edit::Value) {
        match self.store.set(key, value) {
            Ok(c) => {
                self.config = c;
                self.error = None;
                if !self.preview {
                    system::notify_launcher();
                }
            }
            Err(e) => self.error = Some(e),
        }
        self.style();
        self.rebuild();
    }
    fn commit_text(&mut self) {
        let Some(id) = self.focus.and_then(|i| self.controls.get(i)).map(|c| c.id.clone()) else {
            return;
        };
        let text = self.editor.text().to_owned();
        match id.as_str() {
            "hotkey" | "search_url" => self.save(&id, text.into()),
            "folder" => self.folder = text,
            "dock_color_hex" => self.edit_color(&text, None),
            "red" | "green" | "blue" | "alpha" => {
                if !text.is_empty() {
                    self.edit_color(&text, Some(&id));
                }
                self.rebuild();
                if let Some(Kind::Text(value)) = self.focus.and_then(|i| self.controls.get(i)).map(|c| &c.kind) {
                    self.editor = Editor::default();
                    self.editor.insert(value);
                }
            }
            _ => {}
        }
    }

    fn activate(&mut self, index: usize, delta: i64, popup: bool) {
        let Some(c) = self.controls.get(index).cloned().filter(|c| c.focusable()) else {
            return;
        };
        if let Some(p) = c.id.strip_prefix("nav:").and_then(|s| s.parse::<usize>().ok()) {
            self.autostart = system::autostart_enabled();
            self.page = p;
            self.scroll = 0.;
            self.recording = false;
            self.rebuild();
            return;
        }
        if self.page_action(&c, delta, popup) {
            return;
        }
        match c.id.as_str() {
            "record" => {
                self.recording = true;
                self.rebuild();
            }
            "reload" => {
                self.cancel_color();
                match self.store.load() {
                    Ok(c) => {
                        self.config = c;
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                }
                self.style();
                self.rebuild();
            }
            "open-config" => {
                if let Err(e) = system::open_config(&self.store.path) {
                    self.error = Some(e);
                    self.rebuild();
                }
            }
            "autostart" => {
                if !self.preview {
                    match system::set_autostart(!self.autostart) {
                        Ok(()) => self.autostart = !self.autostart,
                        Err(e) => self.error = Some(e),
                    }
                }
                self.rebuild();
            }
            "unpin" => {
                match crate::dock::pins::save(&self.store.path.with_file_name("dock-pins.toml"), &[]) {
                    Ok(()) => {
                        if !self.preview {
                            system::notify_launcher()
                        }
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
                self.rebuild();
            }
            "group-status" => {
                let mut names = self.config.dock_status_group.clone();
                names.extend(self.config.dock_status_icons.clone());
                self.save(
                    if self.config.dock_status_group.is_empty() {
                        "dock_status_group"
                    } else {
                        "dock_status_icons"
                    },
                    toml_edit::Value::Array(names.iter().map(String::as_str).collect()),
                );
            }
            _ => match c.kind {
                Kind::Toggle(v) => self.save(
                    &c.id,
                    if c.id == "position" {
                        if v { "center" } else { "bottom" }.into()
                    } else {
                        (if c.id == "dock_autohide" { v } else { !v }).into()
                    },
                ),
                Kind::Slider { value, min, max } => self.save(&c.id, (value + delta).clamp(min, max).into()),
                Kind::Select { choices, selected } => {
                    let next = if popup {
                        self.popup(&c.rect, &choices, selected)
                    } else {
                        Some((selected as i64 + delta).rem_euclid(choices.len() as i64) as usize)
                    };
                    if let Some(next) = next
                        && let Some((value, _)) = choices.get(next)
                    {
                        if let Some(name) = c.id.strip_prefix("status:") {
                            let key = if value == "group" {
                                "dock_status_group"
                            } else if value == "icon" {
                                "dock_status_icons"
                            } else if self.config.dock_status_group.iter().any(|s| s == name) {
                                "dock_status_group"
                            } else {
                                "dock_status_icons"
                            };
                            let mut names = if key == "dock_status_group" {
                                self.config.dock_status_group.clone()
                            } else {
                                self.config.dock_status_icons.clone()
                            };
                            names.retain(|s| s != name);
                            if value != "hidden" {
                                names.push(name.into());
                            }
                            self.save(key, toml_edit::Value::Array(names.iter().map(String::as_str).collect()));
                        } else {
                            self.save(&c.id, value.as_str().into());
                        }
                    }
                }
                _ => {}
            },
        }
    }
    fn popup(&self, r: &D2D_RECT_F, choices: &[(String, String)], selected: usize) -> Option<usize> {
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            for (i, (_, label)) in choices.iter().enumerate() {
                let label = crate::util::wide(label);
                let _ = AppendMenuW(
                    menu,
                    MF_STRING | if i == selected { MF_CHECKED } else { MF_UNCHECKED },
                    i + 1,
                    PCWSTR(label.as_ptr()),
                );
            }
            let scale = GetDpiForWindow(self.hwnd) as f32 / 96.;
            let mut pt = POINT {
                x: (r.left * scale) as i32,
                y: ((r.bottom - self.scroll) * scale) as i32,
            };
            let _ = ClientToScreen(self.hwnd, &mut pt);
            let result = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, None, self.hwnd, None).0;
            let _ = DestroyMenu(menu);
            if result > 0 { Some(result as usize - 1) } else { None }
        }
    }
    fn focus_to(&mut self, next: Option<usize>) {
        let next_id = next.and_then(|i| self.controls.get(i)).map(|c| c.id.clone());
        self.commit_text();
        let next = next_id.and_then(|id| self.controls.iter().position(|c| c.id == id && c.focusable()));
        self.recording = false;
        self.focus = next;
        if let Some(c) = next.and_then(|i| self.controls.get(i)) {
            if let Kind::Text(text) = &c.kind {
                self.editor = Editor::default();
                self.editor.insert(text);
            }
            if next.is_some_and(|i| i >= 4) {
                if c.rect.top < self.scroll + 68. {
                    self.scroll = (c.rect.top - 68.).max(0.);
                }
                if c.rect.bottom > self.scroll + self.height {
                    self.scroll = c.rect.bottom - self.height;
                }
            }
        }
    }
    fn key(&mut self, vk: u16) {
        let pressed = |key: VIRTUAL_KEY| unsafe { GetKeyState(key.0 as i32) < 0 };
        let shift = pressed(VK_SHIFT);
        let ctrl = pressed(VK_CONTROL);
        if self.recording {
            if vk == VK_ESCAPE.0 {
                self.recording = false;
                self.rebuild();
                return;
            }
            if [VK_SHIFT.0, VK_CONTROL.0, VK_MENU.0, VK_LWIN.0, VK_RWIN.0].contains(&vk) {
                return;
            }
            let key = match vk {
                0x20 => "Space".into(),
                0x0d => "Enter".into(),
                0x09 => "Tab".into(),
                0x2d => "Insert".into(),
                0x2e => "Delete".into(),
                0x24 => "Home".into(),
                0x23 => "End".into(),
                0x21 => "PageUp".into(),
                0x22 => "PageDown".into(),
                0xc0 => "`".into(),
                0xbd => "-".into(),
                0xbb => "=".into(),
                0xdb => "[".into(),
                0xdd => "]".into(),
                0xdc => "\\".into(),
                0xba => ";".into(),
                0xde => "'".into(),
                0xbc => ",".into(),
                0xbe => ".".into(),
                0xbf => "/".into(),
                0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
                0x30..=0x5a => (vk as u8 as char).to_string(),
                _ => {
                    self.error = Some("Use a letter, number, Space, Enter or F1-F24, or type the shortcut.".into());
                    self.rebuild();
                    return;
                }
            };
            let mut parts = Vec::new();
            for (on, name) in [
                (ctrl, "Ctrl"),
                (pressed(VK_MENU), "Alt"),
                (shift, "Shift"),
                (pressed(VK_LWIN) || pressed(VK_RWIN), "Win"),
            ] {
                if on {
                    parts.push(name.to_string());
                }
            }
            parts.push(key);
            self.recording = false;
            self.save("hotkey", parts.join("+").into());
            return;
        }
        if vk == VK_ESCAPE.0 && self.picker {
            self.picker = false;
            self.rebuild();
            return;
        }
        if vk == VK_TAB.0 {
            let next = controls::next_focus(&self.controls, self.focus, shift);
            self.focus_to(next);
            return;
        }
        if vk == VK_PRIOR.0 {
            self.scroll = (self.scroll - self.height + 48.).max(0.);
            return;
        }
        if vk == VK_NEXT.0 {
            self.scroll = (self.scroll + self.height - 48.).min((self.extent - self.height).max(0.));
            return;
        }
        let Some(i) = self.focus else { return };
        let text = self.controls.get(i).is_some_and(|c| matches!(c.kind, Kind::Text(_)));
        if text {
            match vk {
                0x0d => {
                    self.commit_text();
                    if self.controls.get(i).is_some_and(|c| c.id == "folder") {
                        self.add_folder(&self.folder.clone());
                    }
                }
                0x08 => {
                    self.editor.backspace(ctrl);
                }
                0x2e => {
                    self.editor.delete(ctrl);
                }
                0x25 => self.editor.left(ctrl, shift),
                0x27 => self.editor.right(ctrl, shift),
                0x24 => self.editor.home(shift),
                0x23 => self.editor.end(shift),
                0x41 if ctrl => self.editor.select_all(),
                0x43 | 0x58 if ctrl => {
                    let _ = crate::system::set_clipboard(self.hwnd, self.editor.selected_text());
                    if vk == 0x58 {
                        self.editor.insert("");
                    }
                }
                0x56 if ctrl => {
                    if let Some(text) = crate::system::get_clipboard(self.hwnd) {
                        self.editor.insert(&text);
                    }
                }
                _ => {}
            }
            if matches!(vk, 0x08 | 0x2e | 0x58 | 0x56) {
                self.text_changed();
            }
        } else {
            match vk {
                0x20 | 0x0d => self.activate(i, 1, true),
                0x26 | 0x28 if self.controls.get(i).is_some_and(|c| c.id.starts_with("remove:")) => {
                    let next = if vk == 0x26 { i.checked_sub(1) } else { Some(i + 1) };
                    if let Some(next) =
                        next.filter(|n| self.controls.get(*n).is_some_and(|c| c.id.starts_with("remove:")))
                    {
                        self.focus_to(Some(next));
                    }
                }
                0x25 | 0x26
                    if self
                        .controls
                        .get(i)
                        .is_some_and(|c| matches!(c.kind, Kind::Slider { .. } | Kind::Select { .. })) =>
                {
                    self.activate(i, -1, false)
                }
                0x27 | 0x28
                    if self
                        .controls
                        .get(i)
                        .is_some_and(|c| matches!(c.kind, Kind::Slider { .. } | Kind::Select { .. })) =>
                {
                    self.activate(i, 1, false)
                }
                0x21 => self.scroll = (self.scroll - self.height + 48.).max(0.),
                0x22 => self.scroll = (self.scroll + self.height - 48.).min((self.extent - self.height).max(0.)),
                _ => {}
            }
        }
    }
    fn paint(&mut self) -> Result<()> {
        if let Some(page) = PENDING_PAGE.with(Cell::take) {
            self.page = page.min(3);
            self.welcome = (page == 4).then_some(0);
            self.scroll = 0.;
            self.rebuild();
        }
        let mut rect = RECT::default();
        unsafe {
            GetClientRect(self.hwnd, &mut rect)?;
        }
        self.renderer
            .resize(rect.right.max(1) as u32, rect.bottom.max(1) as u32);
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.;
        let (width, height) = (rect.right as f32 / scale, rect.bottom as f32 / scale);
        if self.width != width || self.height != height {
            self.width = width;
            self.height = height;
            self.rebuild();
        }
        let dark = match self.config.theme.as_str() {
            "dark" => true,
            "light" => false,
            _ => !crate::system::system_uses_light_theme(),
        };
        let p = crate::theme::Palette::settings(dark);
        let scroll = self.scroll;
        let focus = self.focus;
        let page = self.page;
        let edit = self.editor.text();
        let caret = self.editor.caret();
        let selection = self.editor.selection();
        self.renderer.settings_canvas(
            self.hwnd,
            self.logo.as_ref(),
            |rt, dw, _title, _small, _heading, glyph| unsafe {
                if self.formats.get().is_none() {
                    let _ = self.formats.set(Formats::new(dw)?);
                }
                let Some(fm) = self.formats.get() else { return Ok(()) };
                let (title, small) = (&fm.body, &fm.hint);
                rt.Clear(Some(&p.background));
                let brush = rt.CreateSolidColorBrush(&p.text, None)?;
                let text = |s: &str,
                            r: D2D_RECT_F,
                            format: &windows::Win32::Graphics::DirectWrite::IDWriteTextFormat,
                            color: &D2D1_COLOR_F| {
                    brush.SetColor(color);
                    rt.DrawText(
                        &s.encode_utf16().collect::<Vec<_>>(),
                        format,
                        &r,
                        &brush,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL,
                    );
                };
                text(
                    if self.welcome.is_some() { "" } else { "wisp" },
                    D2D_RECT_F {
                        left: 56.,
                        top: 16.,
                        right: 180.,
                        bottom: 46.,
                    },
                    &fm.brand,
                    &p.text,
                );
                text(
                    self.welcome.map_or(PAGES[page], |step| {
                        ["Welcome to Wisp", "Check the shortcut", "Start with Windows"][step]
                    }),
                    D2D_RECT_F {
                        left: if self.welcome.is_some() { 32. } else { 228. },
                        top: 16.,
                        right: self.width - 24.,
                        bottom: 60.,
                    },
                    &fm.title,
                    &p.text,
                );
                if let Some(step) = self.welcome {
                    for n in 0..3 {
                        brush.SetColor(if n <= step { &p.accent } else { &p.divider });
                        rt.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: self.width - 116. + n as f32 * 28.,
                                    top: 56.,
                                    right: self.width - 92. + n as f32 * 28.,
                                    bottom: 59.,
                                },
                                radiusX: 1.5,
                                radiusY: 1.5,
                            },
                            &brush,
                        );
                    }
                }
                // Section cards share a surface; individual rows draw only their divider.
                rt.PushAxisAlignedClip(
                    &D2D_RECT_F {
                        left: if self.welcome.is_some() { 0. } else { 196. },
                        top: 68.,
                        right: self.width,
                        bottom: self.height,
                    },
                    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                );
                let mut group: Option<D2D_RECT_F> = None;
                let card = |r: D2D_RECT_F| {
                    let r = D2D_RECT_F {
                        top: r.top - scroll,
                        bottom: r.bottom - scroll,
                        ..r
                    };
                    brush.SetColor(&p.tile);
                    rt.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: r,
                            radiusX: 8.,
                            radiusY: 8.,
                        },
                        &brush,
                    );
                    brush.SetColor(&p.divider);
                    rt.DrawRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: r,
                            radiusX: 8.,
                            radiusY: 8.,
                        },
                        &brush,
                        1.,
                        None,
                    );
                };
                for c in &self.controls[4..] {
                    if matches!(c.kind, Kind::Heading) {
                        if let Some(r) = group.take() {
                            card(r);
                        }
                    } else if c.id != "record" {
                        group = Some(match group {
                            Some(r) => D2D_RECT_F {
                                bottom: c.rect.bottom,
                                ..r
                            },
                            None => c.rect,
                        });
                    }
                }
                if let Some(r) = group {
                    card(r);
                }
                rt.PopAxisAlignedClip();
                for (i, c) in self.controls.iter().enumerate() {
                    if self.welcome.is_some() && i < 4 {
                        continue;
                    }
                    let mut r = c.rect;
                    if i >= 4 {
                        r.top -= scroll;
                        r.bottom -= scroll;
                        if r.bottom < 68. || r.top > self.height {
                            continue;
                        }
                        rt.PushAxisAlignedClip(
                            &D2D_RECT_F {
                                left: if self.welcome.is_some() { 0. } else { 196. },
                                top: 68.,
                                right: self.width,
                                bottom: self.height,
                            },
                            D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                        );
                    }
                    let fg = if c.enabled { p.text } else { p.placeholder };
                    if (i < 4 && i == page) || c.id == "record" {
                        brush.SetColor(&p.tile);
                        rt.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect: r,
                                radiusX: 6.,
                                radiusY: 6.,
                            },
                            &brush,
                        );
                    }
                    if c.focusable() && (self.hover == Some(i) || self.pressed == Some(i)) {
                        let mut fill = p.divider;
                        fill.a *= if self.pressed == Some(i) { 0.8 } else { 0.4 };
                        brush.SetColor(&fill);
                        rt.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect: r,
                                radiusX: 6.,
                                radiusY: 6.,
                            },
                            &brush,
                        );
                    }
                    if focus == Some(i) {
                        brush.SetColor(&p.accent);
                        rt.DrawRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect: r,
                                radiusX: 6.,
                                radiusY: 6.,
                            },
                            &brush,
                            2.,
                            None,
                        );
                    }
                    // Label and hint sit as one block, centred in the row like the GPUI rows.
                    let block = if c.hint.is_empty() || matches!(c.kind, Kind::Slider { .. }) {
                        20.
                    } else {
                        38.
                    };
                    let top = if matches!(c.kind, Kind::Text(_)) {
                        r.top + 14.
                    } else {
                        r.top + ((r.bottom - r.top) - block) / 2.
                    };
                    let label = D2D_RECT_F {
                        left: r.left + 16.,
                        top: if i < 4 { r.top } else { top },
                        right: r.right - 12.,
                        bottom: if i < 4 { r.bottom } else { top + 20. },
                    };
                    match &c.kind {
                        Kind::Heading => {
                            text(
                                &c.label,
                                D2D_RECT_F {
                                    left: r.left + 4.,
                                    top: if c.label.len() > 100 || !c.hint.is_empty() {
                                        r.top + 12.
                                    } else {
                                        r.bottom - 32.
                                    },
                                    bottom: r.bottom - if c.hint.is_empty() { 6. } else { 28. },
                                    ..label
                                },
                                if c.label.chars().any(|ch| ch.is_lowercase()) {
                                    small
                                } else {
                                    &fm.section
                                },
                                if c.id == "error" { &p.accent } else { &p.subtext },
                            );
                        }
                        Kind::Link(s) => {
                            if i == page {
                                brush.SetColor(&p.accent);
                                rt.FillRoundedRectangle(
                                    &D2D1_ROUNDED_RECT {
                                        rect: D2D_RECT_F {
                                            left: r.left,
                                            top: r.top + 10.,
                                            right: r.left + 3.,
                                            bottom: r.top + 24.,
                                        },
                                        radiusX: 1.5,
                                        radiusY: 1.5,
                                    },
                                    &brush,
                                );
                            }
                            if i < 4 {
                                text(
                                    ["\u{E713}", "\u{E790}", "\u{E721}", "\u{E946}"][i],
                                    D2D_RECT_F {
                                        left: r.left + 12.,
                                        top: r.top + 6.,
                                        right: r.left + 28.,
                                        bottom: r.bottom - 6.,
                                    },
                                    glyph,
                                    &fg,
                                );
                            }
                            text(
                                s,
                                D2D_RECT_F {
                                    left: if i < 4 { r.left + 40. } else { label.left },
                                    ..label
                                },
                                title,
                                &fg,
                            );
                        }
                        Kind::Toggle(v) => {
                            text(
                                &c.label,
                                D2D_RECT_F {
                                    right: r.right - 72.,
                                    ..label
                                },
                                title,
                                &fg,
                            );
                            let mid = (r.top + r.bottom) / 2.;
                            let track = D2D_RECT_F {
                                left: r.right - 56.,
                                top: mid - 10.,
                                right: r.right - 16.,
                                bottom: mid + 10.,
                            };
                            brush.SetColor(if *v { &p.accent } else { &p.divider });
                            rt.FillRoundedRectangle(
                                &D2D1_ROUNDED_RECT {
                                    rect: track,
                                    radiusX: 10.,
                                    radiusY: 10.,
                                },
                                &brush,
                            );
                            brush.SetColor(if *v { &p.background_solid } else { &p.subtext });
                            rt.FillEllipse(
                                &D2D1_ELLIPSE {
                                    point: windows_numerics::Vector2 {
                                        X: if *v { track.right - 10. } else { track.left + 10. },
                                        Y: mid,
                                    },
                                    radiusX: 6.,
                                    radiusY: 6.,
                                },
                                &brush,
                            );
                        }
                        Kind::Button(s) if c.id == "record" => {
                            let mut fill = p.divider;
                            fill.a *= 0.6;
                            brush.SetColor(&fill);
                            rt.FillRoundedRectangle(
                                &D2D1_ROUNDED_RECT {
                                    rect: r,
                                    radiusX: 6.,
                                    radiusY: 6.,
                                },
                                &brush,
                            );
                            brush.SetColor(&p.divider);
                            rt.DrawRoundedRectangle(
                                &D2D1_ROUNDED_RECT {
                                    rect: D2D_RECT_F {
                                        left: r.left + 0.5,
                                        top: r.top + 0.5,
                                        right: r.right - 0.5,
                                        bottom: r.bottom - 0.5,
                                    },
                                    radiusX: 6.,
                                    radiusY: 6.,
                                },
                                &brush,
                                1.,
                                None,
                            );
                            text(s, r, &fm.center, &fg)
                        }
                        Kind::Button(s) => {
                            text(&c.label, label, title, &fg);
                            text(
                                s,
                                D2D_RECT_F {
                                    left: r.right - 160.,
                                    top: r.top,
                                    right: r.right - 16.,
                                    bottom: r.bottom,
                                },
                                &fm.trailing,
                                if c.enabled { &p.accent } else { &p.placeholder },
                            );
                        }
                        Kind::Select { choices, selected } => {
                            text(
                                &c.label,
                                D2D_RECT_F {
                                    right: r.right - 252.,
                                    ..label
                                },
                                title,
                                &fg,
                            );
                            if let Some((_, s)) = choices.get(*selected) {
                                let mid = (r.top + r.bottom) / 2.;
                                let field = D2D_RECT_F {
                                    left: r.right - 240.,
                                    top: mid - 16.,
                                    right: r.right - 16.,
                                    bottom: mid + 16.,
                                };
                                brush.SetColor(&p.divider);
                                rt.DrawRoundedRectangle(
                                    &D2D1_ROUNDED_RECT {
                                        rect: D2D_RECT_F {
                                            left: field.left + 0.5,
                                            top: field.top + 0.5,
                                            right: field.right - 0.5,
                                            bottom: field.bottom - 0.5,
                                        },
                                        radiusX: 6.,
                                        radiusY: 6.,
                                    },
                                    &brush,
                                    1.,
                                    None,
                                );
                                text(
                                    s,
                                    D2D_RECT_F {
                                        left: field.left + 12.,
                                        right: field.right - 32.,
                                        ..field
                                    },
                                    title,
                                    &fg,
                                );
                                text(
                                    "\u{E70D}",
                                    D2D_RECT_F {
                                        right: field.right - 12.,
                                        ..field
                                    },
                                    &fm.chevron,
                                    &p.subtext,
                                );
                            }
                        }
                        Kind::Slider { value, min, max } => {
                            let row = D2D_RECT_F {
                                top: r.top + 12.,
                                bottom: r.top + 34.,
                                ..label
                            };
                            text(&c.label, row, title, &fg);
                            text(
                                &if c.id == "max_results" {
                                    value.to_string()
                                } else if c.id == "dock_margin" && *value == 0 {
                                    "On the edge".into()
                                } else {
                                    format!("{value} DIP")
                                },
                                D2D_RECT_F {
                                    right: r.right - 16.,
                                    ..row
                                },
                                &fm.trailing,
                                &p.subtext,
                            );
                            let y = r.top + 50.;
                            brush.SetColor(&p.divider);
                            rt.DrawLine(
                                windows_numerics::Vector2 { X: r.left + 16., Y: y },
                                windows_numerics::Vector2 { X: r.right - 16., Y: y },
                                &brush,
                                4.,
                                None,
                            );
                            let x =
                                r.left + 16. + (r.right - r.left - 32.) * (*value - *min) as f32 / (*max - *min) as f32;
                            brush.SetColor(&p.accent);
                            rt.DrawLine(
                                windows_numerics::Vector2 { X: r.left + 16., Y: y },
                                windows_numerics::Vector2 { X: x, Y: y },
                                &brush,
                                4.,
                                None,
                            );
                            rt.FillEllipse(
                                &D2D1_ELLIPSE {
                                    point: windows_numerics::Vector2 { X: x, Y: y },
                                    radiusX: 7.,
                                    radiusY: 7.,
                                },
                                &brush,
                            );
                        }
                        Kind::Text(s) => {
                            let label = D2D_RECT_F {
                                right: label.right - if c.id == "hotkey" { 92. } else { 0. },
                                ..label
                            };
                            text(&c.label, label, title, &fg);
                            let field = D2D_RECT_F {
                                left: label.left,
                                top: r.top + 44.,
                                right: label.right,
                                bottom: r.top + 76.,
                            };
                            brush.SetColor(&p.divider);
                            rt.DrawRoundedRectangle(
                                &D2D1_ROUNDED_RECT {
                                    rect: D2D_RECT_F {
                                        left: field.left + 0.5,
                                        top: field.top + 0.5,
                                        right: field.right - 0.5,
                                        bottom: field.bottom - 0.5,
                                    },
                                    radiusX: 6.,
                                    radiusY: 6.,
                                },
                                &brush,
                                1.,
                                None,
                            );
                            let shown = if focus == Some(i) { edit } else { s };
                            // Text and caret share this inset box.
                            let label = D2D_RECT_F {
                                left: field.left + 12.,
                                right: field.right - 12.,
                                ..label
                            };
                            text(
                                shown,
                                D2D_RECT_F {
                                    top: r.top + 44.,
                                    bottom: r.top + 76.,
                                    ..label
                                },
                                title,
                                &fg,
                            );
                            if focus == Some(i) {
                                let units: Vec<_> = edit.encode_utf16().collect();
                                let layout =
                                    dw.CreateTextLayout(&units, title, (label.right - label.left).max(1.), 26.)?;
                                let position = |byte: usize| {
                                    let mut x = 0.;
                                    let mut y = 0.;
                                    let mut metrics =
                                        windows::Win32::Graphics::DirectWrite::DWRITE_HIT_TEST_METRICS::default();
                                    let _ = layout.HitTestTextPosition(
                                        edit[..byte].encode_utf16().count() as u32,
                                        false,
                                        &mut x,
                                        &mut y,
                                        &mut metrics,
                                    );
                                    x
                                };
                                let caret_x = position(caret).min(label.right - label.left);
                                brush.SetColor(&p.accent);
                                rt.DrawLine(
                                    windows_numerics::Vector2 {
                                        X: label.left + caret_x,
                                        Y: r.top + 48.,
                                    },
                                    windows_numerics::Vector2 {
                                        X: label.left + caret_x,
                                        Y: r.top + 72.,
                                    },
                                    &brush,
                                    1.5,
                                    None,
                                );
                                if !selection.is_empty() {
                                    rt.DrawLine(
                                        windows_numerics::Vector2 {
                                            X: label.left + position(selection.start),
                                            Y: r.top + 73.,
                                        },
                                        windows_numerics::Vector2 {
                                            X: label.left + position(selection.end),
                                            Y: r.top + 73.,
                                        },
                                        &brush,
                                        2.,
                                        None,
                                    );
                                }
                            }
                        }
                    }
                    if !c.hint.is_empty() && !matches!(c.kind, Kind::Slider { .. }) {
                        text(
                            &c.hint,
                            if matches!(c.kind, Kind::Text(_) | Kind::Heading) {
                                D2D_RECT_F {
                                    left: r.left + 16.,
                                    top: r.bottom - 24.,
                                    right: r.right - 12.,
                                    bottom: r.bottom - 6.,
                                }
                            } else {
                                D2D_RECT_F {
                                    left: r.left + 16.,
                                    top: label.bottom,
                                    right: r.right - if matches!(c.kind, Kind::Toggle(_)) { 72. } else { 12. },
                                    bottom: label.bottom + 18.,
                                }
                            },
                            small,
                            &p.subtext,
                        );
                    }
                    if i >= 4 {
                        if c.id != "record"
                            && !matches!(c.kind, Kind::Heading)
                            && self
                                .controls
                                .get(i + 1)
                                .is_some_and(|c| !matches!(c.kind, Kind::Heading))
                        {
                            brush.SetColor(&p.divider);
                            rt.DrawLine(
                                windows_numerics::Vector2 { X: r.left, Y: r.bottom },
                                windows_numerics::Vector2 {
                                    X: r.right,
                                    Y: r.bottom,
                                },
                                &brush,
                                1.,
                                None,
                            );
                        }
                        rt.PopAxisAlignedClip();
                    }
                }
                if self.extent > self.height {
                    let h = self.height * self.height / self.extent;
                    let y = scroll / self.extent * self.height;
                    brush.SetColor(&p.scrollbar);
                    rt.FillRoundedRectangle(
                        &D2D1_ROUNDED_RECT {
                            rect: D2D_RECT_F {
                                left: self.width - 6.,
                                top: y + 2.,
                                right: self.width - 3.,
                                bottom: y + h - 2.,
                            },
                            radiusX: 1.5,
                            radiusY: 1.5,
                        },
                        &brush,
                    );
                }
                Ok(())
            },
        )
    }
}
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCDESTROY {
        HANDLE.with(|h| h.set(HWND::default()));
        PENDING_PAGE.with(|p| p.set(None));
        let preview = WINDOW.with(|w| {
            w.try_borrow_mut()
                .ok()
                .and_then(|mut w| w.take())
                .is_some_and(|s| s.preview)
        });
        if preview {
            unsafe {
                PostQuitMessage(0);
            }
        }
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    if msg == WM_GETMINMAXINFO {
        unsafe {
            let dpi = GetDpiForWindow(hwnd) as i32;
            let info = &mut *(lp.0 as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT {
                x: 520 * dpi / 96,
                y: 420 * dpi / 96,
            };
        }
        return LRESULT(0);
    }
    let handled = WINDOW.with(|w| {
        let Ok(mut w) = w.try_borrow_mut() else { return false };
        let Some(s) = w.as_mut() else { return false };
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                unsafe {
                    let _ = BeginPaint(hwnd, &mut ps);
                }
                if let Err(e) = s.paint() {
                    s.error = Some(format!("Cannot draw Settings: {e}"));
                }
                unsafe {
                    let _ = EndPaint(hwnd, &ps);
                }
                return true;
            }
            WM_SIZE => {
                s.renderer
                    .resize((lp.0 as u32) & 0xffff, ((lp.0 as u32) >> 16) & 0xffff);
            }
            WM_DPICHANGED => {
                s.renderer.set_dpi((wp.0 & 0xffff) as u32);
                let r = unsafe { &*(lp.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED => s.style(),
            WM_ACTIVATE if wp.0 & 0xffff != WA_INACTIVE as usize => {
                s.rebuild();
            }
            WM_SYSKEYDOWN if !s.recording => return false,
            WM_KEYDOWN | WM_SYSKEYDOWN => s.key(wp.0 as u16),
            WM_CHAR => {
                if s.focus
                    .and_then(|i| s.controls.get(i))
                    .is_some_and(|c| matches!(c.id.as_str(), "red" | "green" | "blue" | "alpha"))
                    && wp.0 >= 32
                    && !(48..=57).contains(&wp.0)
                {
                    return true;
                }
                if !s.recording
                    && s.focus
                        .is_some_and(|i| s.controls.get(i).is_some_and(|c| matches!(c.kind, Kind::Text(_))))
                    && s.editor.push_utf16(wp.0 as u16)
                {
                    s.text_changed();
                }
            }
            WM_TIMER if wp.0 == 42 => {
                s.flush_color();
            }
            WM_CLOSE => {
                s.flush_color();
                return false;
            }
            WM_LBUTTONUP | WM_CAPTURECHANGED => {
                s.pressed = None;
                s.dragging = None;
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            WM_MOUSEMOVE if s.dragging.is_some() => {
                let id = s.dragging.clone().unwrap_or_default();
                if let Some(Control {
                    kind: Kind::Slider { min, max, value },
                    rect,
                    ..
                }) = s.controls.iter().find(|c| c.id == id)
                {
                    let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.;
                    let x = (lp.0 as u16 as i16) as f32 / scale;
                    let next = (*min as f32
                        + (x - rect.left - 16.) / (rect.right - rect.left - 32.) * (*max - *min) as f32)
                        .round() as i64;
                    let next = next.clamp(*min, *max);
                    if next != *value {
                        s.save(&id, next.into());
                    }
                }
            }
            WM_MOUSEMOVE => {
                s.hover = s.hit(lp);
                unsafe {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        ..Default::default()
                    };
                    let _ = TrackMouseEvent(&mut track);
                }
            }
            windows::Win32::UI::Controls::WM_MOUSELEAVE => {
                s.hover = None;
                s.pressed = None;
            }
            WM_MOUSEWHEEL => {
                s.scroll = (s.scroll - ((wp.0 >> 16) as u16 as i16) as f32 / 120. * 48.)
                    .clamp(0., (s.extent - s.height).max(0.));
            }
            WM_KILLFOCUS => {
                s.recording = false;
                s.commit_text();
            }
            WM_LBUTTONDOWN => {
                let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.;
                let x = (lp.0 as u16 as i16) as f32 / scale;
                let y = ((lp.0 >> 16) as u16 as i16) as f32 / scale;
                let hit = s.controls.iter().enumerate().rposition(|(i, c)| {
                    let yy = y + if i >= 4 { s.scroll } else { 0. };
                    c.focusable()
                        && (i < 4 || y >= 68.)
                        && x >= c.rect.left
                        && x < c.rect.right
                        && yy >= c.rect.top
                        && yy < c.rect.bottom
                });
                s.pressed = hit;
                if let Some(i) = hit {
                    s.focus_to(Some(i));
                    if let Some(Control {
                        id,
                        kind: Kind::Slider { min, max, .. },
                        rect,
                        ..
                    }) = s.controls.get(i)
                    {
                        let value = (*min as f32
                            + (x - rect.left - 16.) / (rect.right - rect.left - 32.) * (*max - *min) as f32)
                            .round() as i64;
                        s.dragging = Some(id.clone());
                        unsafe {
                            SetCapture(hwnd);
                        }
                        s.save(&id.clone(), value.clamp(*min, *max).into());
                    } else if !s.controls.get(i).is_some_and(|c| matches!(c.kind, Kind::Text(_))) {
                        s.activate(i, 1, true);
                    }
                }
            }
            _ => return false,
        }
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        true
    });
    if handled {
        LRESULT(0)
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(path: PathBuf) -> Settings {
        let store = Store::new(path);
        store.ensure_exists().unwrap();
        let config = store.load().unwrap();
        let mut s = Settings {
            hwnd: HWND::default(),
            renderer: crate::render::Renderer::new().unwrap(),
            logo: None,
            store,
            config,
            controls: Vec::new(),
            focus: None,
            scroll: 0.,
            extent: 0.,
            page: 0,
            editor: Editor::default(),
            recording: false,
            dragging: None,
            error: None,
            autostart: false,
            conflict: false,
            preview: true,
            width: 820.,
            height: 580.,
            folder: String::new(),
            advanced: false,
            picker: false,
            color: "#1E3A8ABF".into(),
            pending_color: None,
            welcome: None,
            hover: None,
            pressed: None,
            formats: std::cell::OnceCell::new(),
        };
        s.rebuild();
        s
    }
    fn activate(s: &mut Settings, id: &str, delta: i64) {
        let i = s.controls.iter().position(|c| c.id == id).unwrap();
        s.activate(i, delta, false);
        assert!(s.error.is_none(), "{:?}", s.error);
    }
    #[test]
    fn appearance_search_about_and_welcome_use_shared_store_and_focus_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = fixture(dir.path().join("config.toml"));
        s.page = 1;
        s.rebuild();
        activate(&mut s, "theme", 1);
        activate(&mut s, "backdrop", 1);
        activate(&mut s, "advanced", 1);
        activate(&mut s, "dock_margin", 99);
        activate(&mut s, "custom-color", 1);
        activate(&mut s, "picker", 1);
        s.edit_color("12", Some("red"));
        s.edit_color("#112233", None);
        assert_eq!(s.pending_color.as_deref(), Some("#112233BF"));
        assert_eq!(s.store.load().unwrap().dock_color, "#1E3A8ABF");
        s.flush_color();
        activate(&mut s, "max_results", 99);
        let c = s.store.load().unwrap();
        assert_eq!(c.theme, "dark");
        assert_eq!(c.backdrop, "mica");
        assert_eq!(c.dock_margin, 24);
        assert_eq!(c.max_results, 20);
        assert_eq!(c.dock_color, "#112233BF");
        s.edit_color("256", Some("green"));
        assert!(s.error.is_some());
        s.error = None;
        s.page = 2;
        s.rebuild();
        activate(&mut s, "show_recent", 1);
        activate(&mut s, "file_search", 1);
        activate(&mut s, "file_search_everything", 1);
        s.add_folder(" sample ");
        assert!(s.store.load().unwrap().file_search_exclude.contains(&"sample".into()));
        s.add_folder("SAMPLE");
        assert!(s.error.is_some());
        s.add_folder("a/b");
        assert!(s.error.is_some());
        s.save("search_url", "invalid".into());
        assert!(s.error.is_some());
        s.save("search_url", "https://example.com/?q={query}".into());
        assert_eq!(s.store.load().unwrap().search_url, "https://example.com/?q={query}");
        let index = s.config.file_search_exclude.iter().position(|v| v == "sample").unwrap();
        activate(&mut s, &format!("remove:{index}"), 1);
        assert!(!s.store.load().unwrap().file_search_exclude.contains(&"sample".into()));
        for page in 0..4 {
            s.page = page;
            s.rebuild();
            let expected: Vec<_> = s
                .controls
                .iter()
                .enumerate()
                .filter(|(_, c)| c.focusable())
                .map(|(i, _)| i)
                .collect();
            let mut focus = None;
            for index in &expected {
                focus = controls::next_focus(&s.controls, focus, false);
                assert_eq!(focus, Some(*index));
            }
            assert_eq!(
                controls::next_focus(&s.controls, focus, false),
                expected.first().copied()
            );
        }
        s.page = 1;
        s.rebuild();
        let ids: Vec<_> = s
            .controls
            .iter()
            .filter(|c| c.focusable())
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "nav:0",
                "nav:1",
                "nav:2",
                "nav:3",
                "theme",
                "backdrop",
                "advanced",
                "dock_margin",
                "custom-color",
                "picker",
                "preset",
                "dock_color_hex",
                "red",
                "green",
                "blue",
                "alpha",
                "max_results"
            ]
        );
        let before = std::fs::read_to_string(&s.store.path).unwrap();
        s.page = 3;
        s.rebuild();
        assert!(s.controls.iter().any(|c| c.id == "update-status"));
        assert_eq!(before, std::fs::read_to_string(&s.store.path).unwrap());
        s.welcome = Some(0);
        s.rebuild();
        assert_eq!(
            controls::next_focus(&s.controls, None, false),
            s.controls.iter().position(|c| c.id == "hotkey")
        );
        activate(&mut s, "win_key", 1);
        activate(&mut s, "next", 1);
        assert_eq!(s.welcome, Some(1));
        activate(&mut s, "next", 1);
        assert_eq!(s.welcome, Some(2));
        activate(&mut s, "back", 1);
        assert_eq!(s.welcome, Some(1));
    }
    #[test]
    fn general_actions_persist_correct_keys_and_inverse_values_without_losing_external_edits() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = fixture(dir.path().join("config.toml"));
        let text = std::fs::read_to_string(&s.store.path).unwrap();
        std::fs::write(&s.store.path, format!("# user comment\nunknown=42\n{text}")).unwrap();
        activate(&mut s, "win_key", 1);
        assert!(s.store.load().unwrap().win_key);
        assert!(!s.controls.iter().find(|c| c.id == "hotkey").unwrap().focusable());
        activate(&mut s, "win_key", 1);
        activate(&mut s, "dock_autohide", 1);
        assert!(!s.store.load().unwrap().dock_autohide);
        assert!(
            !s.controls
                .iter()
                .find(|c| c.id == "dock_dodge_windows")
                .unwrap()
                .focusable()
        );
        activate(&mut s, "dock_autohide", 1);
        activate(&mut s, "dock_dodge_windows", 1);
        activate(&mut s, "dock_search_button", 1);
        activate(&mut s, "dock_icon_size", 99);
        activate(&mut s, "window_list", 1);
        activate(&mut s, "hide_taskbar", 1);
        activate(&mut s, "position", 1);
        activate(&mut s, "check_for_updates", 1);
        let c = s.store.load().unwrap();
        assert!(c.dock_dodge_windows);
        assert!(!c.dock_search_button);
        assert_eq!(c.dock_icon_size, 40);
        assert_eq!(c.window_list, "always");
        assert!(!c.hide_taskbar);
        assert_eq!(c.position, "center");
        assert!(!c.check_for_updates);
        activate(&mut s, "dock", 1);
        assert!(!s.store.load().unwrap().dock);
        assert!(!s.controls.iter().find(|c| c.id == "dock_screen").unwrap().focusable());
        let saved = std::fs::read_to_string(&s.store.path).unwrap();
        assert!(saved.contains("# user comment"));
        assert!(saved.contains("unknown=42"));
    }
    #[test]
    fn general_status_transfer_shortcut_validation_and_screen_choice_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = fixture(dir.path().join("config.toml"));
        activate(&mut s, "status:sound", 1);
        let c = s.store.load().unwrap();
        assert!(!c.dock_status_group.contains(&"sound".into()));
        assert!(c.dock_status_icons.contains(&"sound".into()));
        activate(&mut s, "status:sound", 1);
        assert!(!s.store.load().unwrap().dock_status_icons.contains(&"sound".into()));
        activate(&mut s, "dock_screen", 1);
        assert_eq!(s.store.load().unwrap().dock_screen, "main");
        let i = s.controls.iter().position(|c| c.id == "hotkey").unwrap();
        s.focus_to(Some(i));
        s.editor.select_all();
        s.editor.insert("Ctrl");
        s.commit_text();
        assert!(s.error.is_some());
        assert_eq!(s.store.load().unwrap().hotkey, "Alt+Space");
        s.editor.select_all();
        s.editor.insert("Ctrl+Shift+F12");
        s.commit_text();
        assert!(s.error.is_none());
        assert_eq!(s.store.load().unwrap().hotkey, "Ctrl+Shift+F12");
    }
    #[test]
    fn page_request_has_stable_bounded_wire_values() {
        assert_eq!(page_arg(&["--page".into(), "ABOUT".into()]), 3);
        assert_eq!(page_arg(&["--page".into(), "unknown".into()]), 0);
        assert_eq!(page_arg(&["--page".into()]), 0);
    }
}
