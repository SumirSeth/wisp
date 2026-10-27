//! Launcher lifecycle, input and message loop.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result, w};

use crate::config::{Backdrop, Config, Theme};
use crate::files::{FileResults, FileSearch};
use crate::history::History;
use crate::indexer::{AppEntry, IconPixels, IndexEvent, Indexer};
use crate::render::{Frame, Renderer, layout};
use crate::search::{self, FileScope, Kind, ResultItem, Searcher};
use crate::theme::Palette;
use crate::{hotkey, launch, system, tray, util};

const WM_APP_INDEX: u32 = WM_APP + 2;
const WM_APP_FILES: u32 = WM_APP + 5;
const WM_APP_SHOW: u32 = WM_APP + 3;
const WM_APP_QUIT: u32 = WM_APP + 4;
const CLASS_NAME: PCWSTR = w!("Wisp.Launcher");
const HOTKEY_ID: i32 = 1;
const CARET_TIMER: usize = 1;
/// File results shown after apps, and for a query that starts with `?`.
const MIXED_FILE_RESULTS: usize = 5;
const ONLY_FILE_RESULTS: usize = 30;
const RESCAN_AFTER: Duration = Duration::from_secs(120);
const PLACEHOLDER: &str = "Search apps, calculate, or run a command with >";

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

// Preserve index updates and hide requests during reentrant message handling.
const DEFER_INDEX: u8 = 1;
const DEFER_FILES: u8 = 4;
const DEFER_HIDE: u8 = 2;

thread_local! {
    static LAUNCHER: RefCell<Option<Launcher>> = const { RefCell::new(None) };
    static DEFERRED: Cell<u8> = const { Cell::new(0) };
}

/// Returns `None` if the launcher is already borrowed. Windows can send
/// reentrant messages during `ShowWindow` or a modal `ShellExecuteEx` call.
/// Runs deferred work after the outer handler releases the borrow.
fn with_launcher<R>(f: impl FnOnce(&mut Launcher) -> R) -> Option<R> {
    let result = LAUNCHER.with(|cell| cell.try_borrow_mut().ok().and_then(|mut l| l.as_mut().map(f)));
    if result.is_some() {
        run_deferred();
    }
    result
}

/// Defers `work` if a reentrant call cannot borrow the launcher.
fn with_launcher_or_defer(work: u8, f: impl FnOnce(&mut Launcher)) {
    if with_launcher(f).is_none() {
        DEFERRED.with(|d| d.set(d.get() | work));
    }
}

fn run_deferred() {
    loop {
        let work = DEFERRED.with(|d| d.replace(0));
        if work == 0 {
            return;
        }
        let ran = LAUNCHER.with(|cell| {
            let Ok(mut launcher) = cell.try_borrow_mut() else {
                return false;
            };
            if let Some(l) = launcher.as_mut() {
                if work & DEFER_INDEX != 0 {
                    l.drain_index_events();
                }
                if work & DEFER_FILES != 0 {
                    l.drain_file_results();
                }
                if work & DEFER_HIDE != 0 {
                    l.hide();
                }
            }
            true
        });
        if !ran {
            DEFERRED.with(|d| d.set(d.get() | work));
            return;
        }
    }
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;

        // The mutex limits Wisp to one running instance.
        let _mutex = CreateMutexW(None, true, w!("Local\\Wisp.Launcher.SingleInstance"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Wait up to 3 s for a starting instance to create its window.
            for _ in 0..30 {
                if let Ok(existing) = FindWindowW(CLASS_NAME, PCWSTR::null()) {
                    let mut pid = 0;
                    GetWindowThreadProcessId(existing, Some(&mut pid));
                    let _ = AllowSetForegroundWindow(pid);
                    let msg = if flag("--quit") { WM_APP_QUIT } else { WM_APP_SHOW };
                    let _ = PostMessageW(Some(existing), msg, WPARAM(0), LPARAM(0));
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            return Err(windows::core::Error::new(
                E_FAIL,
                "Wisp is running, but its launcher window was not found within 3 s.",
            ));
        }
        if flag("--quit") {
            return Ok(());
        }

        let renderer = Renderer::new()?;
        let system_dpi = GetDpiForSystem();
        let instance = GetModuleHandleW(None)?;
        // build.rs embeds the icon as resource ID 1.
        let load_icon = |metric| -> Result<HICON> {
            let size = GetSystemMetricsForDpi(metric, system_dpi);
            let icon = LoadImageW(
                Some(instance.into()),
                PCWSTR(1 as _),
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR,
            )?;
            Ok(HICON(icon.0))
        };
        let small_icon = load_icon(SM_CXSMICON)?;
        let large_icon = load_icon(SM_CXICON)?;

        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: large_icon,
            hIconSm: small_icon,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS_NAME,
            w!("Wisp"),
            WS_POPUP | WS_CAPTION,
            0,
            0,
            100,
            100,
            None,
            None,
            Some(instance.into()),
            None,
        )?;

        // Apply WM_NCCALCSIZE before the first draw to remove the visible frame.
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )?;

        let (config, config_error) = Config::load();
        let (events_tx, events_rx) = channel();
        let (files_tx, files_rx) = channel();
        // Icons are drawn at 32 DIPs; extract them at the system DPI's pixel size.
        let icon_size = (32 * system_dpi as i32 + 95) / 96;
        let launcher = Launcher {
            hwnd,
            config,
            config_mtime: Config::modified(),
            registered_hotkey: None,
            tray_icon: small_icon,
            palette: Palette::new(true, true),
            renderer,
            dpi: system_dpi,
            position: (0, 0),
            rows_that_fit: usize::MAX,
            visible: false,
            editor: crate::editor::Editor::default(),
            searcher: Searcher::new(),
            results: Vec::new(),
            base_results: Vec::new(),
            file_results: Vec::new(),
            file_query: None,
            selected: 0,
            scroll: 0,
            caret_visible: true,
            last_mouse: None,
            apps: Vec::new(),
            icons: HashMap::new(),
            history: History::load(),
            indexer: Indexer::spawn(hwnd, WM_APP_INDEX, events_tx, icon_size),
            index_events: events_rx,
            file_search: FileSearch::spawn(hwnd, WM_APP_FILES, files_tx, icon_size),
            file_events: files_rx,
            last_scan: Instant::now(),
        };
        LAUNCHER.with(|cell| *cell.borrow_mut() = Some(launcher));
        with_launcher(|l| {
            l.start();
            if let Some(error) = config_error {
                tray::notify(l.hwnd, "Wisp config", &error, true);
            }
            if flag("--show") {
                l.show();
            }
        });

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        LAUNCHER.with(|cell| cell.borrow_mut().take());
    }
    Ok(())
}

struct Launcher {
    hwnd: HWND,
    config: Config,
    config_mtime: Option<SystemTime>,
    /// Registered shortcut; may differ from `config.hotkey` if registration fails.
    registered_hotkey: Option<String>,
    tray_icon: HICON,
    palette: Palette,
    renderer: Renderer,
    dpi: u32,
    /// Screen coordinates of the launcher's top-left corner, in physical pixels.
    position: (i32, i32),
    /// Number of results that fit in the current monitor's work area.
    rows_that_fit: usize,
    visible: bool,

    editor: crate::editor::Editor,
    searcher: Searcher,
    /// What the launcher shows: `base_results` with `file_results` inserted
    /// before the web search result.
    results: Vec<ResultItem>,
    /// Results computed on the UI thread: apps, calculator, URL, command, web search.
    base_results: Vec<ResultItem>,
    /// File search results, which arrive later from the file search worker.
    file_results: Vec<ResultItem>,
    /// The file query sent to the worker, if the current query searches files.
    file_query: Option<String>,
    selected: usize,
    scroll: usize,
    caret_visible: bool,
    last_mouse: Option<(i32, i32)>,

    apps: Vec<AppEntry>,
    icons: HashMap<String, Arc<IconPixels>>,
    history: History,
    indexer: Indexer,
    index_events: Receiver<IndexEvent>,
    file_search: FileSearch,
    file_events: Receiver<FileResults>,
    last_scan: Instant,
}

impl Launcher {
    fn start(&mut self) {
        self.apply_theme();
        // Windows needs the tray icon before it can show notifications.
        tray::add(self.hwnd, self.tray_icon, &self.tooltip());
        self.register_hotkey(true);
        self.indexer.rescan();
        self.last_scan = Instant::now();
    }

    fn tooltip(&self) -> String {
        match &self.registered_hotkey {
            Some(hotkey) => format!("Wisp ({hotkey})"),
            None => "Wisp (no shortcut)".into(),
        }
    }

    /// Tries to restore the previous shortcut if the new shortcut cannot be
    /// registered, so an invalid config does not disable access to the launcher.
    fn register_hotkey(&mut self, report_errors: bool) {
        if self.registered_hotkey.as_deref() == Some(self.config.hotkey.as_str()) {
            return;
        }
        let wanted = match hotkey::parse(&self.config.hotkey) {
            Ok(hotkey) => hotkey,
            Err(error) => {
                if report_errors {
                    tray::notify(self.hwnd, "Wisp config", &error, true);
                }
                return;
            }
        };
        let hwnd = self.hwnd;
        let register =
            |hk: &hotkey::Hotkey| unsafe { RegisterHotKey(Some(hwnd), HOTKEY_ID, hk.modifiers, hk.vk).is_ok() };

        let previous = self.registered_hotkey.take();
        if previous.is_some() {
            unsafe {
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
            }
        }
        if register(&wanted) {
            self.registered_hotkey = Some(self.config.hotkey.clone());
        } else {
            if let Some(previous) = previous
                && hotkey::parse(&previous).is_ok_and(|hk| register(&hk))
            {
                self.registered_hotkey = Some(previous);
            }
            if report_errors {
                let message = format!(
                    "Windows could not register {}. Another app may use it. Change \"hotkey\" in the config.",
                    self.config.hotkey
                );
                tray::notify(hwnd, "Could not register shortcut", &message, true);
            }
        }
        tray::set_tip(hwnd, &self.tooltip());
    }

    fn reload_config_if_changed(&mut self) {
        let mtime = Config::modified();
        if mtime == self.config_mtime {
            return;
        }
        self.config_mtime = mtime;
        let (config, error) = Config::load();
        if let Some(error) = error {
            // Keep the last valid config so a read or parse error does not change it.
            tray::notify(self.hwnd, "Wisp config", &error, true);
            return;
        }
        self.config = config;
        self.apply_theme();
        self.register_hotkey(true);
    }

    fn apply_theme(&mut self) {
        let dark = match self.config.theme {
            Theme::Dark => true,
            Theme::Light => false,
            Theme::System => !system::system_uses_light_theme(),
        };
        let backdrop = match self.config.backdrop {
            Backdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
            Backdrop::Mica => DWMSBT_MAINWINDOW,
            Backdrop::Solid => DWMSBT_NONE,
        };
        self.palette = Palette::new(dark, backdrop != DWMSBT_NONE);
        unsafe {
            let set = |attr: DWMWINDOWATTRIBUTE, value: i32| {
                let _ = DwmSetWindowAttribute(self.hwnd, attr, (&value as *const i32).cast(), size_of::<i32>() as u32);
            };
            set(DWMWA_USE_IMMERSIVE_DARK_MODE, dark as i32);
            set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND.0);
            set(DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE as i32);
            set(DWMWA_SYSTEMBACKDROP_TYPE, backdrop.0);
            let margins = MARGINS {
                cxLeftWidth: -1,
                cxRightWidth: -1,
                cyTopHeight: -1,
                cyBottomHeight: -1,
            };
            let _ = DwmExtendFrameIntoClientArea(self.hwnd, &margins);
        }
        self.invalidate();
    }

    fn toggle(&mut self) {
        if self.visible { self.hide() } else { self.show() }
    }

    fn show(&mut self) {
        self.reload_config_if_changed();
        self.register_hotkey(false);
        if self.last_scan.elapsed() > RESCAN_AFTER {
            self.indexer.rescan();
            self.last_scan = Instant::now();
        }
        // Selecting the previous query lets the user replace it by typing.
        self.editor.select_all();
        self.update_results();
        self.place_on_active_monitor();
        self.last_mouse = None;
        self.visible = true;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(Some(self.hwnd));
            // DWM acrylic uses non-client activation as well as foreground state.
            if GetForegroundWindow() == self.hwnd {
                SendMessageW(self.hwnd, WM_NCACTIVATE, Some(WPARAM(1)), Some(LPARAM(-1)));
            }
        }
        self.restart_caret();
        self.paint();
    }

    fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), CARET_TIMER);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Uses the monitor under the cursor. Reduces the result count and moves
    /// the launcher up if needed to keep it within the work area.
    fn place_on_active_monitor(&mut self) {
        let work = unsafe {
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut info);
            // Use the target monitor's DPI before moving the launcher there.
            let (mut dpi_x, mut dpi_y) = (96, 96);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            self.set_dpi(dpi_x);
            info.rcWork
        };

        let margin = self.px(16.0);
        let width = self.px(layout::WIDTH);
        let full_height = self.px(layout::height(self.config.max_results));
        let top = (work.top + (work.bottom - work.top) / 4)
            .min(work.bottom - margin - full_height)
            .max(work.top + margin);
        let available = (work.bottom - margin - top) as f32 * 96.0 / self.dpi as f32;
        self.rows_that_fit = (((available - layout::height(1)) / layout::ROW_H).max(0.0) as usize + 1).max(1);
        self.position = (work.left + (work.right - work.left - width) / 2, top);
        self.apply_size();
    }

    fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi;
        self.renderer.set_dpi(dpi);
    }

    fn on_dpi_changed(&mut self, dpi: u32) {
        if dpi != self.dpi {
            self.set_dpi(dpi);
            if self.visible {
                self.place_on_active_monitor();
                self.invalidate();
            }
        }
    }

    fn px(&self, dips: f32) -> i32 {
        (dips * self.dpi as f32 / 96.0).round() as i32
    }

    fn list_rows(&self) -> usize {
        self.config.max_results.min(self.rows_that_fit)
    }

    fn visible_rows(&self) -> usize {
        self.results.len().min(self.list_rows())
    }

    /// Keep the height fixed while results are present. Resizing an acrylic or
    /// mica window makes DWM recalculate the blur, which takes about 250 ms to
    /// settle. A fixed height needs one repaint per keystroke.
    fn window_rows(&self) -> usize {
        if self.results.is_empty() { 0 } else { self.list_rows() }
    }

    fn apply_size(&mut self) {
        let width = self.px(layout::WIDTH);
        let height = self.px(layout::height(self.window_rows()));
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                self.position.0,
                self.position.1,
                width,
                height,
                SWP_NOACTIVATE,
            );
        }
        self.renderer.resize(width as u32, height as u32);
    }

    fn update_results(&mut self) {
        let rows_before = self.window_rows();
        self.base_results = self
            .searcher
            .search(self.editor.text(), &self.apps, &self.history, &self.config);

        let scope = if self.config.file_search {
            FileScope::of(self.editor.text(), &self.base_results)
        } else {
            FileScope::None
        };
        let (file_query, limit) = match scope {
            FileScope::None => (None, 0),
            FileScope::Mixed(query) => (Some(query), MIXED_FILE_RESULTS),
            FileScope::Only(query) => (Some(query), ONLY_FILE_RESULTS),
        };
        match file_query {
            None => {
                self.file_query = None;
                self.file_results.clear();
            }
            // Keep the previous file results until the new ones arrive, so the
            // results below the apps don't flicker on each keystroke.
            Some(query) if self.file_query.as_deref() != Some(query) => {
                let exclude = Arc::from(self.config.file_search_exclude.as_slice());
                self.file_search.search(query, limit, exclude);
                self.file_query = Some(query.to_string());
            }
            Some(_) => {}
        }

        self.merge_results();
        self.selected = 0;
        self.scroll = 0;
        if self.window_rows() != rows_before {
            self.apply_size();
        }
        self.invalidate();
    }

    /// Rebuilds `results` from `base_results` and `file_results`.
    fn merge_results(&mut self) {
        let base = std::mem::take(&mut self.base_results);
        let web = base.iter().position(|r| r.kind == Kind::Web).unwrap_or(base.len());
        self.results = Vec::with_capacity(base.len() + self.file_results.len());
        self.results.extend(base[..web].iter().cloned());
        self.results.extend(self.file_results.iter().cloned());
        self.results.extend(base[web..].iter().cloned());
        self.base_results = base;
    }

    fn drain_file_results(&mut self) {
        let mut latest = None;
        while let Ok(results) = self.file_events.try_recv() {
            // Icons are cached even for results that are already outdated.
            self.icons.extend(results.icons);
            if let Some(hits) = results.hits
                && self.file_query.as_deref() == Some(results.query.as_str())
            {
                latest = Some(hits);
            }
        }
        let Some(hits) = latest else {
            self.invalidate();
            return;
        };

        let rows_before = self.window_rows();
        // Keep the web search result selected only if the user selected it.
        // When it is the first result, the top result stays selected instead.
        let web_selected = self.selected > 0 && self.results.get(self.selected).is_some_and(|r| r.kind == Kind::Web);
        self.file_results = hits.into_iter().map(search::file_result).collect();
        self.merge_results();
        if web_selected {
            self.selected = self.results.iter().position(|r| r.kind == Kind::Web).unwrap_or(0);
        }
        self.selected = self.selected.min(self.results.len().saturating_sub(1));
        self.move_selection(0, false);
        if self.window_rows() != rows_before {
            self.apply_size();
        }
        self.invalidate();
    }

    fn move_selection(&mut self, delta: isize, wrap: bool) {
        let len = self.results.len() as isize;
        if len == 0 {
            return;
        }
        let target = self.selected as isize + delta;
        self.selected = if wrap {
            target.rem_euclid(len)
        } else {
            target.clamp(0, len - 1)
        } as usize;
        let rows = self.visible_rows();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        self.invalidate();
    }

    fn activate(&mut self, admin: bool) {
        let Some(result) = self.results.get(self.selected) else {
            return;
        };
        let outcome = match &result.action {
            search::Action::Launch { id, path } => {
                let result = launch::app(id, path.as_deref(), admin);
                if result.is_ok() {
                    self.history.record(id);
                }
                result
            }
            search::Action::Copy(text) => system::set_clipboard(self.hwnd, text),
            search::Action::Open(target) => launch::open(target),
            search::Action::Shell(command) if command.is_empty() => return,
            search::Action::Shell(command) => launch::shell(command, admin),
        };
        match outcome {
            Ok(()) => self.hide(),
            Err(e) if launch::is_cancelled(&e) => {}
            Err(e) => tray::notify(
                self.hwnd,
                &format!("Could not {} {}", result.kind.verb().to_lowercase(), result.title),
                &e.message(),
                true,
            ),
        }
    }

    fn reveal_selected(&mut self) {
        if let Some(path) = self.results.get(self.selected).and_then(ResultItem::path)
            && launch::reveal(path).is_ok()
        {
            self.hide();
        }
    }

    fn copy_selected_path(&mut self) {
        if let Some(path) = self.results.get(self.selected).and_then(ResultItem::path) {
            let _ = system::set_clipboard(self.hwnd, path);
        }
    }

    fn drain_index_events(&mut self) {
        let mut apps_changed = false;
        let mut icons_changed = false;
        while let Ok(event) = self.index_events.try_recv() {
            match event {
                IndexEvent::Apps(apps) => {
                    self.apps = apps;
                    apps_changed = true;
                }
                IndexEvent::Icons(icons) => {
                    self.icons.extend(icons);
                    icons_changed = true;
                }
            }
        }
        if apps_changed && self.visible {
            let selected = self.selected;
            self.update_results();
            self.move_selection(selected as isize, false);
        } else if icons_changed {
            self.invalidate();
        }
    }

    fn on_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let down = |key: VIRTUAL_KEY| unsafe { GetKeyState(key.0 as i32) < 0 };
        let (ctrl, shift) = (down(VK_CONTROL), down(VK_SHIFT));
        let page = self.list_rows() as isize;
        let mut edited = false;
        match vk {
            VK_ESCAPE => {
                self.hide();
                return true;
            }
            VK_RETURN => {
                self.activate(ctrl && shift);
                return true;
            }
            VK_UP => self.move_selection(-1, true),
            VK_DOWN => self.move_selection(1, true),
            VK_TAB => self.move_selection(if shift { -1 } else { 1 }, true),
            VK_PRIOR => self.move_selection(-page, false),
            VK_NEXT => self.move_selection(page, false),
            VK_LEFT => self.editor.left(ctrl, shift),
            VK_RIGHT => self.editor.right(ctrl, shift),
            VK_HOME => self.editor.home(shift),
            VK_END => self.editor.end(shift),
            VK_BACK => edited = self.editor.backspace(ctrl),
            VK_DELETE => edited = self.editor.delete(ctrl),
            VK_A if ctrl => self.editor.select_all(),
            VK_C if ctrl && shift => self.copy_selected_path(),
            VK_C if ctrl => self.copy(),
            VK_X if ctrl && self.editor.has_selection() => {
                self.copy();
                self.editor.insert("");
                edited = true;
            }
            VK_V if ctrl => {
                if let Some(text) = system::get_clipboard(self.hwnd) {
                    self.editor.insert(&text.replace(['\r', '\n'], " "));
                    edited = true;
                }
            }
            VK_E if ctrl && shift => {
                self.reveal_selected();
                return true;
            }
            _ => return false,
        }
        if edited {
            self.update_results();
        }
        self.restart_caret();
        self.invalidate();
        true
    }

    fn copy(&mut self) {
        let text = if self.editor.has_selection() {
            Some(self.editor.selected_text())
        } else {
            match self.results.get(self.selected) {
                Some(ResultItem {
                    kind: Kind::Calc,
                    title,
                    ..
                }) => Some(title.as_str()),
                _ => None,
            }
        };
        if let Some(text) = text {
            let _ = system::set_clipboard(self.hwnd, text);
        }
    }

    fn on_char(&mut self, unit: u16) {
        if self.editor.push_utf16(unit) {
            self.update_results();
            self.restart_caret();
        }
    }

    fn row_at(&self, y: i32) -> Option<usize> {
        let y = y as f32 * 96.0 / self.dpi as f32 - layout::LIST_TOP;
        if y < 0.0 {
            return None;
        }
        let row = (y / layout::ROW_H) as usize;
        (row < self.visible_rows()).then_some(self.scroll + row)
    }

    fn on_mouse_move(&mut self, x: i32, y: i32) {
        // Windows sends a mouse move when the launcher appears under the cursor.
        // Ignore it to keep the initial result selected.
        if self.last_mouse.replace((x, y)).is_none_or(|last| last == (x, y)) {
            return;
        }
        if let Some(row) = self.row_at(y)
            && row != self.selected
        {
            self.selected = row;
            self.invalidate();
        }
    }

    fn on_click(&mut self, y: i32) {
        if let Some(row) = self.row_at(y) {
            self.selected = row;
            self.activate(false);
        }
    }

    fn restart_caret(&mut self) {
        self.caret_visible = true;
        unsafe {
            SetTimer(Some(self.hwnd), CARET_TIMER, GetCaretBlinkTime(), None);
        }
        self.invalidate();
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn paint(&mut self) {
        let text = self.editor.text();
        let frame = Frame {
            palette: &self.palette,
            text,
            caret: util::utf16_offset(text, self.editor.caret()),
            anchor: util::utf16_offset(text, self.editor.anchor()),
            caret_visible: self.caret_visible,
            placeholder: PLACEHOLDER,
            results: &self.results,
            selected: self.selected,
            scroll: self.scroll,
            rows: self.visible_rows(),
            icons: &self.icons,
        };
        if let Err(e) = self.renderer.draw(self.hwnd, &frame) {
            eprintln!("could not draw launcher: {e}");
        }
    }

    fn on_tray_command(&mut self, cmd: u32) {
        match cmd {
            tray::CMD_OPEN => self.show(),
            tray::CMD_RESCAN => {
                self.indexer.rescan();
                self.last_scan = Instant::now();
            }
            tray::CMD_SETTINGS => {
                let path = Config::path();
                let path = path.to_string_lossy();
                // Notepad can edit the config if Windows cannot open .toml files.
                if launch::open(&path).is_err() {
                    let _ = launch::notepad(&path);
                }
            }
            tray::CMD_AUTOSTART => {
                if let Err(e) = system::set_autostart(!system::autostart_enabled()) {
                    tray::notify(self.hwnd, "Could not change Start with Windows", &e.message(), true);
                }
            }
            // Defer destruction until this handler releases the launcher borrow.
            // WM_DESTROY needs to borrow the launcher for shutdown.
            tray::CMD_QUIT => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_APP_QUIT, WPARAM(0), LPARAM(0));
            },
            _ => {}
        }
    }

    fn shutdown(&mut self) {
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_ID);
        }
        tray::remove(self.hwnd);
    }
}

fn loword(v: usize) -> u32 {
    (v & 0xFFFF) as u32
}

fn mouse_pos(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xFFFF) as i16 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    )
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // DWM needs the frame styles for acrylic. Make the full window client area.
        WM_NCCALCSIZE if wparam.0 != 0 => return LRESULT(0),
        WM_NCHITTEST => return LRESULT(HTCLIENT as isize),
        WM_HOTKEY => {
            with_launcher(Launcher::toggle);
        }
        WM_APP_SHOW => {
            with_launcher(Launcher::show);
        }
        WM_APP_QUIT => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        WM_APP_INDEX => with_launcher_or_defer(DEFER_INDEX, Launcher::drain_index_events),
        WM_APP_FILES => with_launcher_or_defer(DEFER_FILES, Launcher::drain_file_results),
        WM_ACTIVATE if loword(wparam.0) == WA_INACTIVE => with_launcher_or_defer(DEFER_HIDE, Launcher::hide),
        WM_DPICHANGED => {
            with_launcher(|l| l.on_dpi_changed(loword(wparam.0)));
            return LRESULT(0);
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if with_launcher(|l| l.on_key(VIRTUAL_KEY(wparam.0 as u16))) == Some(true) {
                return LRESULT(0);
            }
        }
        WM_CHAR => {
            with_launcher(|l| l.on_char(wparam.0 as u16));
            return LRESULT(0);
        }
        // Suppress the Windows beep for Alt shortcuts handled by the launcher.
        WM_SYSCHAR => return LRESULT(0),
        WM_MOUSEMOVE => {
            let (x, y) = mouse_pos(lparam);
            with_launcher(|l| l.on_mouse_move(x, y));
        }
        WM_LBUTTONUP => {
            let (_, y) = mouse_pos(lparam);
            with_launcher(|l| l.on_click(y));
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as u16 as i16;
            with_launcher(|l| l.move_selection(if delta > 0 { -1 } else { 1 }, false));
        }
        WM_TIMER if wparam.0 == CARET_TIMER => {
            with_launcher(|l| {
                l.caret_visible = !l.caret_visible;
                l.invalidate();
            });
        }
        WM_PAINT => unsafe {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            with_launcher(Launcher::paint);
            let _ = EndPaint(hwnd, &ps);
            return LRESULT(0);
        },
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETTINGCHANGE => {
            with_launcher(Launcher::apply_theme);
        }
        tray::WM_TRAY => match loword(lparam.0 as usize) {
            WM_LBUTTONUP => {
                with_launcher(Launcher::show);
            }
            WM_RBUTTONUP | WM_CONTEXTMENU => {
                // Release the launcher borrow before the menu's modal loop handles messages.
                let Some(hotkey) = with_launcher(|l| l.registered_hotkey.clone().unwrap_or_default()) else {
                    return LRESULT(0);
                };
                let cmd = tray::show_menu(hwnd, &hotkey, system::autostart_enabled());
                with_launcher(|l| l.on_tray_command(cmd));
            }
            _ => {}
        },
        // Keep Wisp running after Alt+F4 so the shortcut can reopen the launcher.
        WM_CLOSE => {
            with_launcher(Launcher::hide);
            return LRESULT(0);
        }
        WM_DESTROY => unsafe {
            with_launcher(Launcher::shutdown);
            PostQuitMessage(0);
        },
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            // Explorer removes tray icons when it restarts.
            with_launcher(|l| tray::add(l.hwnd, l.tray_icon, &l.tooltip()));
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
