//! Independent, nonactivating app dock on the message-loop thread.

#[path = "dock/artwork.rs"]
mod artwork;
#[path = "dock/desktops.rs"]
mod desktops;
#[path = "dock/jump_lists.rs"]
mod jump_lists;
#[path = "dock/logic.rs"]
pub(crate) mod logic;
#[path = "dock/pins.rs"]
mod pins;
#[path = "dock/render.rs"]
mod render;
#[path = "dock/space.rs"]
pub mod space;

#[path = "dock/status.rs"]
mod status;
#[path = "dock/tray.rs"]
mod tray;

/// Reserved height in DIPs at the default app icon size.
pub const DEFAULT_RESERVED_HEIGHT: f32 = logic::HEIGHT + 2.0 * logic::MARGIN;

use crate::deferred::{Deferred, Queue};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{
    CoTaskMemFree,
    StructuredStorage::{PropVariantClear, PropVariantToStringAlloc},
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetCapture, GetDoubleClickTime, GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE,
    TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE, VK_LBUTTON, VK_RBUTTON, VK_SHIFT,
};
use windows::Win32::UI::Input::{RAWINPUTDEVICE, RIDEV_INPUTSINK, RIDEV_REMOVE, RegisterRawInputDevices};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, GUID, HSTRING, Result, w};

use crate::commands::open_windows::{self, Window};
use crate::config::{Backdrop, WindowListMode};
use crate::indexer::{AppEntry, IconPixels};
use crate::theme::{Palette, Tint};
use crate::tray_host::{self, Action, TrayHost};

use crate::messages::WM_ARTWORK;
use crate::messages::WM_CLOSE_FLYOUT;
pub(crate) use crate::messages::WM_DOCK_DRAIN;
/// Asks the owner to show the Windows taskbar for a moment, for icons the
/// dock lacks.
pub use crate::messages::WM_DOCK_SHOW_TASKBAR;
use crate::messages::WM_DOCK_STATUS;
pub(crate) use crate::messages::WM_DOCK_WAKE;
use crate::messages::WM_RADIOS;
use crate::messages::WM_REFRESH;
pub use crate::messages::WM_SHOW_LAUNCHER;
pub use crate::messages::WM_SHOW_NOW;
/// The tray host's icons changed.
use crate::messages::WM_TRAY_CHANGED;
/// Shows a notification icon's tooltip after a short hover.
const TIP_TIMER: usize = 104;
const TIP_MS: u32 = 450;
/// App menu commands. Pin and Unpin share one, as they did before.
const MENU_PIN: usize = 1;
const MENU_CLOSE: usize = 2;
/// The app's name, which starts a new window, and its second menu.
const MENU_NEW_WINDOW: usize = 10;
const MENU_ADMIN: usize = 12;
const MENU_LOCATION: usize = 13;
const MENU_PROPERTIES: usize = 14;
const MENU_END_TASK: usize = 15;
/// Tooltips wrap past this width, in DIPs.
const TIP_MAX: f32 = 320.0;
/// Lists an app's windows after the same hover, and hides it once the pointer
/// has left both the list and the dock.
const WINDOWS_TIMER: usize = 107;
const WINDOWS_HIDE_TIMER: usize = 108;
const WINDOWS_HIDE_MS: u32 = 220;
const REFRESH_TIMER: usize = 101;
/// WM_REFRESH's wParam when only other apps' windows changed.
const REFRESH_WINDOWS: usize = 1;
/// Watches the pointer for the bottom edge while the dock is hidden, and
/// for leaving while it shows.
const POLL_TIMER: usize = 102;
const POLL_MS: u32 = 100;
/// Polls faster while the pointer moves in the lower part of a screen and the
/// dock is hidden, so the dock starts to slide in soon after the pointer
/// reaches the edge.
const NEAR_POLL_MS: u32 = 15;
/// Fast polls without pointer movement before the dock goes back to the slow
/// poll. Windows then wakes it on the next movement (`WM_INPUT`), so a
/// resting pointer costs what it does far from the edge.
const STILL_POLLS: u8 = 4;
/// Polls the pointer may spend away from the dock before it hides.
const HIDE_AFTER: u8 = 5;
/// Steps the slide-in animation.
const SLIDE_TIMER: usize = 103;
const SLIDE_MS: f32 = 180.0;
const HOVER_TIMER: usize = 105;
const HOVER_MS: f32 = 110.0;
/// Hides the desktop pill after a scroll.
const DESKTOP_TIMER: usize = 106;
/// Fires as each minute starts, for the clock.
const CLOCK_TIMER: usize = 109;
/// A dock that stays visible and isn't in use checks for fullscreen apps at
/// this rate; its clock, volume and battery update from events.
const QUIET_POLL_MS: u32 = 500;
const DESKTOP_PILL_MS: u32 = 1100;
/// Wheel input right after a switch is the same flick, not another request.
const DESKTOP_PAUSE: Duration = Duration::from_millis(300);
thread_local! { static EVENT_TARGET: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) }; }

struct Item {
    app: AppEntry,
    windows: Vec<Window>,
}

struct WindowId {
    exe: String,
    aumid: Option<String>,
    read: Instant,
}

/// How long a window's app ID is trusted. Some apps set it after their
/// window appears.
const WINDOW_ID_TTL: Duration = Duration::from_secs(10);

/// Fires [`CLOCK_TIMER`] just after the next minute starts.
fn arm_clock(hwnd: HWND) {
    unsafe {
        let now = windows::Win32::System::SystemInformation::GetLocalTime();
        let into_minute = u32::from(now.wSecond) * 1000 + u32::from(now.wMilliseconds);
        SetTimer(Some(hwnd), CLOCK_TIMER, 60_000 - into_minute.min(59_999) + 20, None);
    }
}

/// What the dock shows for its items: their apps and windows.
fn items_key(items: &[Item]) -> Vec<(String, Vec<isize>)> {
    items
        .iter()
        .map(|item| (item.app.id.clone(), item.windows.iter().map(|w| w.hwnd).collect()))
        .collect()
}

/// Identifies the pins, so app matches made for other pins are dropped.
fn pins_key(pins: &[AppEntry]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for pin in pins {
        pin.id.hash(&mut hasher);
        pin.path.hash(&mut hasher);
    }
    hasher.finish()
}

enum PressTarget {
    App(String),
    Section(logic::Section),
}

struct Press {
    target: PressTarget,
    sections: [logic::Section; 9],
    x: i32,
    dragging: bool,
    order: Vec<String>,
    pins: Vec<AppEntry>,
}

struct State {
    hwnd: HWND,
    owner: HWND,
    apps: Vec<AppEntry>,
    icons: HashMap<String, Arc<IconPixels>>,
    artwork: artwork::Artwork,
    items: Vec<Item>,
    pinned: Vec<AppEntry>,
    order: [logic::Section; 9],
    press: Option<Press>,
    suppress_click: bool,
    palette: Palette,
    backdrop: Backdrop,
    /// The user's dock color, kept so system theme changes reapply it.
    tint: Option<Tint>,
    dark: bool,
    material: Option<crate::theme::MaterialKey>,
    window_list_mode: WindowListMode,
    app_icon_size: f32,
    renderer: render::Renderer,
    dpi: u32,
    hover: Option<usize>,
    visible: bool,
    fullscreen: bool,
    scheduled: bool,
    scroll: usize,
    capacity: usize,
    status: status::Status,
    status_placement: logic::StatusPlacement,
    radios: status::Radios,
    percent_width: f32,
    radio_worker: status::RadioWorker,
    hover_part: Option<logic::Part>,
    /// The pointer touched the bottom edge, so the dock slid in.
    revealed: bool,
    away: u8,
    /// The poll timer's current interval.
    poll_ms: u32,
    /// The pointer at the last poll, and how many fast polls in a row found it
    /// there.
    last_cursor: POINT,
    still_polls: u8,
    /// Windows reports the next pointer movement, while the pointer rests in
    /// the lower part of a screen and the dock is hidden.
    listening: bool,
    /// Stays revealed regardless of the pointer, for the demo's screenshots,
    /// optionally at another DPI than the monitor's.
    held: bool,
    /// Whether the dock hides until the pointer touches a bottom edge. When
    /// false it stays on the primary screen.
    autohide: bool,
    search_button: bool,
    suspended: bool,
    frame_paced: bool,
    held_dpi: Option<u32>,
    /// Where the window rests, in physical pixels, and when it began sliding there.
    top: i32,
    slide: Option<Instant>,
    slide_from: f32,
    slide_to: f32,
    hover_levels: Vec<f32>,
    hover_tick: Option<Instant>,
    /// Wheel delta over the volume not yet spent on a whole percent.
    wheel: i32,
    /// Wheel travel toward the next desktop switch, and when the last one happened.
    desktop_travel: i32,
    desktop_switched: Option<Instant>,
    /// The desktops the pill above the dock shows, while it shows.
    desktop_pill: Option<desktops::Desktops>,
    /// The dock's left edge on screen, in physical pixels.
    left: i32,
    width: i32,
    surface_rect: Option<RECT>,
    /// Hosts the notification area while the taskbar is hidden.
    tray_host: Option<TrayHost>,
    tray: Vec<tray::Shown>,
    /// Indices into `tray` of the icons shown in the dock itself.
    inline: Vec<usize>,
    /// Lists every notification icon above the chevron.
    flyout: HWND,
    flyout_open: bool,
    flyout_hover: Option<usize>,
    flyout_renderer: render::Renderer,
    tip: HWND,
    tip_renderer: render::Renderer,
    /// What the pointer rests on, and the text the tooltip shows for it.
    tip_target: Option<tray::Tip>,
    tip_text: String,
    /// The window list above the hovered app icon: which slot it belongs to,
    /// its rows and their windows, and the row under the pointer.
    windows: HWND,
    windows_renderer: render::Renderer,
    name_hook: HWINEVENTHOOK,
    windows_slot: Option<usize>,
    windows_rows: Vec<String>,
    windows_targets: Vec<isize>,
    /// Each window's program and app ID, read once and checked again after
    /// [`WINDOW_ID_TTL`]: reading an app ID asks the shell.
    window_ids: HashMap<isize, WindowId>,
    /// Apps matched to a program and app ID. Cleared when apps or pins change.
    app_matches: HashMap<(String, Option<String>), Option<AppEntry>>,
    /// The pins `app_matches` was built for.
    matched_pins: u64,
    /// A scheduled refresh must lay the dock out again, not only rescan windows.
    relayout: bool,
    /// Set while a window-change refresh runs: it may skip layout and redraw
    /// when no app, window or status changed.
    layout_unchanged_ok: bool,
    windows_hover: Option<usize>,
    /// The last click on a notification icon, to tell double clicks.
    tray_click: Option<(tray_host::Key, Instant)>,
    /// The demo's flyout and its hovered icon, applied as icons arrive.
    held_flyout: Option<Option<usize>>,
}

type DockMessage = (HWND, u32, WPARAM, LPARAM);
type PendingMessages = Queue<(isize, u32, usize), DockMessage>;

struct StateCell {
    state: Deferred<State>,
    messages: RefCell<PendingMessages>,
}
impl std::ops::Deref for StateCell {
    type Target = Deferred<State>;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
impl StateCell {
    fn new(state: State) -> Self {
        Self {
            state: Deferred::new(state),
            messages: RefCell::new(Queue::new()),
        }
    }
    fn defer_message(&self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) {
        let msg = if msg == WM_INPUT { WM_DOCK_WAKE } else { msg };
        if retain_message(msg) {
            let lp = if matches!(msg, WM_DPICHANGED | WM_SETTINGCHANGE | WM_POWERBROADCAST | WM_DOCK_WAKE) {
                LPARAM(0)
            } else {
                lp
            };
            self.messages
                .borrow_mut()
                .push((hwnd.0 as isize, msg, wp.0), (hwnd, msg, wp, lp));
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_DOCK_DRAIN, WPARAM(0), LPARAM(0));
            }
        }
    }
    fn drain(&self) {
        self.state.drain();
        if self.try_borrow_mut().is_err() {
            return;
        }
        loop {
            let next = self.messages.borrow_mut().pop();
            let Some((hwnd, msg, wp, lp)) = next else {
                break;
            };
            unsafe {
                let _ = PostMessageW(Some(hwnd), msg, wp, lp);
            }
        }
    }
}

/// Only retain scalar payloads: DPI/setting pointers expire with SendMessage.
/// Dock handlers recompute placement and material without using those pointers.
pub(crate) fn retain_message(msg: u32) -> bool {
    matches!(
        msg,
        WM_TIMER
            | WM_CLOSE
            | WM_MOUSELEAVE
            | WM_REFRESH
            | WM_ARTWORK
            | WM_DOCK_STATUS
            | WM_TRAY_CHANGED
            | WM_RADIOS
            | WM_CLOSE_FLYOUT
            | WM_DOCK_WAKE
            | WM_PAINT
            | WM_DISPLAYCHANGE
            | WM_DPICHANGED
            | WM_SETTINGCHANGE
            | WM_THEMECHANGED
            | WM_POWERBROADCAST
            | WM_CAPTURECHANGED
            | WM_ACTIVATE
    )
}

/// Dock settings prepared before entering the state mutation.
#[derive(Clone, Copy)]
pub struct DockConfig {
    placement: logic::StatusPlacement,
    order: [logic::Section; 9],
    size: f32,
    mode: WindowListMode,
    search: bool,
    autohide: bool,
}
impl From<&crate::config::Config> for DockConfig {
    fn from(config: &crate::config::Config) -> Self {
        let placement = logic::StatusPlacement::new(&config.dock_status_group, &config.dock_status_icons);
        let order = logic::section_order(config.dock_order.iter().map(String::as_str));
        let size = (config.dock_icon_size as f32).clamp(logic::MIN_APP_ICON_SIZE, logic::MAX_APP_ICON_SIZE);
        let mode = config.window_list;
        let search = config.dock_search_button;
        let autohide = config.dock_autohide;
        Self {
            placement,
            order,
            size,
            mode,
            search,
            autohide,
        }
    }
}

/// Own on the launcher's message-loop thread. Never borrows Launcher from its wndproc.
pub struct Dock {
    hwnd: HWND,
    shared: bool,
    popups: Vec<HWND>,
    state: Box<StateCell>,
    hooks: Vec<HWINEVENTHOOK>,
    autohide: Cell<bool>,
    app_icon_size: Cell<f32>,
}

impl Dock {
    pub fn new(owner: HWND) -> Result<Self> {
        Self::create(owner, false)
    }

    pub fn on_surface(owner: HWND) -> Result<Self> {
        Self::create(owner, true)
    }

    fn create(owner: HWND, shared: bool) -> Result<Self> {
        if EVENT_TARGET.with(|target| !target.get().is_invalid()) {
            return Err(Error::new(E_FAIL, "A dock already exists on this thread"));
        }
        let mut state = Box::new(StateCell::new(State {
            hwnd: HWND::default(),
            owner,
            apps: Vec::new(),
            icons: HashMap::new(),
            artwork: artwork::Artwork::default(),
            items: Vec::new(),
            pinned: load_pins(),
            order: logic::DEFAULT_ORDER,
            press: None,
            suppress_click: false,
            palette: Palette::new(true, true),
            backdrop: Backdrop::Acrylic,
            tint: None,
            dark: true,
            material: None,
            window_list_mode: WindowListMode::default(),
            app_icon_size: logic::DEFAULT_APP_ICON_SIZE,
            renderer: render::Renderer::new()?,
            dpi: 96,
            hover: None,
            visible: false,
            fullscreen: false,
            scheduled: false,
            scroll: 0,
            capacity: usize::MAX,
            status: status::read(logic::StatusPlacement {
                group: [false; 4],
                icons: [false; 4],
            }),
            status_placement: logic::StatusPlacement {
                group: [false; 4],
                icons: [false; 4],
            },
            radios: status::Radios::default(),
            percent_width: 28.0,
            radio_worker: status::RadioWorker::default(),
            hover_part: None,
            revealed: false,
            away: 0,
            poll_ms: POLL_MS,
            last_cursor: POINT::default(),
            still_polls: 0,
            listening: false,
            held: false,
            autohide: true,
            search_button: true,
            suspended: false,
            frame_paced: false,
            held_dpi: None,
            top: 0,
            slide: None,
            slide_from: 0.0,
            slide_to: 0.0,
            hover_levels: Vec::new(),
            hover_tick: None,
            wheel: 0,
            desktop_travel: 0,
            desktop_switched: None,
            desktop_pill: None,
            left: 0,
            width: 0,
            surface_rect: None,
            tray_host: None,
            tray: Vec::new(),
            inline: Vec::new(),
            flyout: HWND::default(),
            flyout_open: false,
            flyout_hover: None,
            flyout_renderer: render::Renderer::new()?,
            tip: HWND::default(),
            tip_renderer: render::Renderer::new()?,
            tip_target: None,
            tip_text: String::new(),
            windows: HWND::default(),
            windows_renderer: render::Renderer::new()?,
            name_hook: HWINEVENTHOOK::default(),
            windows_slot: None,
            windows_rows: Vec::new(),
            windows_targets: Vec::new(),
            window_ids: HashMap::new(),
            app_matches: HashMap::new(),
            matched_pins: 0,
            relayout: false,
            layout_unchanged_ok: false,
            windows_hover: None,
            tray_click: None,
            held_flyout: None,
        }));
        let instance = unsafe { GetModuleHandleW(None)? };
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("Wisp.Dock"),
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&class) } == 0 && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
            return Err(Error::from_thread());
        }
        let hwnd = if shared {
            unsafe {
                SetWindowLongPtrW(owner, GWLP_USERDATA, (&raw mut *state) as isize);
            }
            owner
        } else {
            unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                    class.lpszClassName,
                    w!("Wisp Dock"),
                    WS_POPUP | WS_CAPTION,
                    0,
                    0,
                    64,
                    44,
                    None,
                    None,
                    Some(instance.into()),
                    Some((&raw mut *state).cast()),
                )?
            }
        };
        state.borrow_mut().hwnd = hwnd;
        let mut dock = Self {
            hwnd,
            shared,
            popups: Vec::new(),
            state,
            hooks: Vec::new(),
            autohide: Cell::new(true),
            app_icon_size: Cell::new(logic::DEFAULT_APP_ICON_SIZE),
        };
        // The flyout and tooltips are owned by the dock, so they stay above it.
        let popup = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(popup_wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("Wisp.DockPopup"),
            hCursor: class.hCursor,
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&popup) } == 0 && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
            return Err(Error::from_thread());
        }
        for (style, title) in [
            // The flyout takes focus, for Esc and closing on a click elsewhere.
            (WS_EX_TOOLWINDOW | WS_EX_TOPMOST, w!("Notification area")),
            (
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                w!("Wisp Dock tooltip"),
            ),
            // The window list takes clicks, but not the focus.
            (
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                w!("Wisp Dock windows"),
            ),
        ] {
            let window = unsafe {
                CreateWindowExW(
                    style,
                    popup.lpszClassName,
                    title,
                    WS_POPUP | WS_CAPTION,
                    0,
                    0,
                    1,
                    1,
                    Some(hwnd),
                    None,
                    Some(instance.into()),
                    Some((&raw mut *dock.state).cast()),
                )?
            };
            dock.popups.push(window);
            let mut s = dock.state.borrow_mut();
            if s.flyout.is_invalid() {
                s.flyout = window;
            } else if s.tip.is_invalid() {
                s.tip = window;
            } else {
                s.windows = window;
            }
        }
        EVENT_TARGET.with(|target| target.set(hwnd));
        crate::commands::audio::notify_level_changes(hwnd, WM_DOCK_STATUS);
        arm_clock(hwnd);
        for (first, last) in [
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            // Windows on other desktops are cloaked, so switching desktops changes the apps.
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        ] {
            let hook = unsafe {
                SetWinEventHook(
                    first,
                    last,
                    None,
                    Some(window_event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if hook.is_invalid() {
                return Err(Error::from_thread());
            }
            dock.hooks.push(hook);
        }
        dock.apply_theme(true, Backdrop::Acrylic, None);
        dock.refresh();
        Ok(dock)
    }

    fn native_rect(&self) -> RECT {
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rect);
        }
        rect
    }
    fn update<R>(&self, key: Option<&'static str>, op: impl FnOnce(&mut State) -> R + 'static) -> Option<R> {
        let result = self.state.run(key, op);
        if result.is_none() {
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_DOCK_DRAIN, WPARAM(0), LPARAM(0));
            }
        }
        self.state.drain();
        result
    }

    /// Keep ownership changes out of a standalone dock's modal stack as well.
    /// Returns false when idle so the caller can handle the message normally.
    pub fn defer_owner_message(&self, key: &'static str, msg: u32, wp: WPARAM, lp: LPARAM) -> bool {
        if self.state.try_borrow_mut().is_ok() {
            return false;
        }
        self.update(Some(key), move |s| unsafe {
            let _ = PostMessageW(Some(s.owner), msg, wp, lp);
        });
        true
    }

    pub fn draw_on(&mut self, rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget) -> Result<()> {
        // A render target belongs to this frame: skip it while busy and invalidate for a later paint.
        let result = if let Ok(mut s) = self.state.try_borrow_mut() {
            s.paint(Some(rt))
        } else {
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
            Ok(())
        };
        self.state.drain();
        result
    }

    pub fn shared(&self) -> bool {
        self.shared
    }

    pub fn dispatch(&self, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe { wndproc(self.hwnd, msg, wp, lp) }
    }

    pub fn owns_message(msg: u32, wp: WPARAM) -> bool {
        msg == WM_TIMER && (REFRESH_TIMER..=CLOCK_TIMER).contains(&wp.0)
            || matches!(
                msg,
                WM_REFRESH
                    | WM_ARTWORK
                    | WM_DOCK_STATUS
                    | WM_TRAY_CHANGED
                    | WM_RADIOS
                    | WM_INPUT
                    | WM_DOCK_DRAIN
                    | WM_DOCK_WAKE
                    | WM_CLOSE_FLYOUT
            )
    }

    pub fn mostly_visible(&self) -> bool {
        // While busy, use native visibility without consulting the animation.
        let Ok(s) = self.state.try_borrow() else {
            return unsafe { IsWindowVisible(self.hwnd).as_bool() };
        };
        unsafe { crate::morph::can_morph(IsWindowVisible(self.hwnd).as_bool(), s.slide_value()) }
    }

    pub fn pace_frames(&mut self) {
        self.update(Some("pace_frames"), move |s| {
            s.frame_paced = true;
        });
    }

    /// Slides the dock in as the pointer would, and keeps it in, for captures.
    pub fn capture_reveal(&mut self) {
        self.update(Some("capture_reveal"), move |s| {
            s.held = true;
            s.reveal();
        });
    }

    pub fn capture_slide(&mut self) {
        self.update(None, move |s| {
            let to = if s.slide_to > 0.5 { 0.0 } else { 1.0 };
            s.revealed = to < 0.5;
            if !s.revealed {
                s.radio_worker.hide();
            }
            unsafe {
                let _ = ShowWindow(s.hwnd, SW_SHOWNOACTIVATE);
            }
            s.start_slide(to);
        });
    }

    pub fn animating(&self) -> bool {
        // Keep frames requested while a menu may be holding an animation.
        self.state.try_borrow().map(|s| s.slide.is_some()).unwrap_or(true)
    }

    pub fn frame(&mut self) {
        self.update(Some("frame"), move |s| {
            if s.slide.is_some() {
                s.step_slide();
            }
        });
    }

    /// Pauses placement without hiding the shared surface.
    pub fn suspend(&mut self) -> RECT {
        self.update(Some("suspend"), move |s| {
            s.finish_press(false);
            s.close_flyout();
            s.hide_tip();
            s.hide_windows();
            let mut rect = RECT::default();
            unsafe {
                let _ = GetWindowRect(s.hwnd, &mut rect);
                if !IsWindowVisible(s.hwnd).as_bool() {
                    let height = rect.bottom - rect.top;
                    let layout_scale = logic::layout_scale(s.app_icon_size);
                    rect.top = s.top + height + (logic::margin(layout_scale) * s.dpi as f32 / 96.0).round() as i32;
                    rect.bottom = rect.top + height;
                }
                let _ = KillTimer(Some(s.hwnd), POLL_TIMER);
            }
            s.listen(false);
            s.stop_slide();
            s.suspended = true;
            s.radio_worker.hide();
            rect
        })
        // While busy, return the current native rectangle; placement catches up after the menu.
        .unwrap_or_else(|| {
            let mut rect = RECT::default();
            unsafe {
                let _ = GetWindowRect(self.hwnd, &mut rect);
            }
            rect
        })
    }

    pub fn resume(&mut self) {
        self.update(Some("resume"), move |s| {
            s.suspended = false;
            s.surface_rect = None;
            s.revealed = true;
            s.visible = true;
            s.away = 0;
            s.stop_slide();
            s.refresh();
            s.start_poll(POLL_MS);
        });
    }

    pub fn surface_rect(&mut self, rect: RECT) {
        self.update(Some("surface_rect"), move |s| {
            s.surface_rect = Some(rect);
            s.publish_rects();
        });
    }

    pub fn resting_rect(&self) -> RECT {
        // Current window geometry is the best available rectangle while busy.
        let Ok(s) = self.state.try_borrow() else {
            return self.native_rect();
        };
        RECT {
            left: s.left,
            top: s.top,
            right: s.left + s.width,
            bottom: s.top + (logic::height(logic::layout_scale(s.app_icon_size)) * s.dpi as f32 / 96.0).round() as i32,
        }
    }

    /// Call after index or icon batches.
    pub fn set_apps(&mut self, apps: &[AppEntry], icons: &HashMap<String, Arc<IconPixels>>) {
        // Usually the state is free: update it from the borrowed lists, and
        // copy them only when the change has to wait for a menu to close.
        self.state.drain();
        if let Ok(mut s) = self.state.try_borrow_mut() {
            s.apply_apps(apps, icons);
            return;
        }
        let apps = apps.to_vec();
        let icons = icons.clone();
        self.update(Some("set_apps"), move |s| s.apply_apps(&apps, &icons));
    }

    pub fn configure(&mut self, config: &DockConfig) {
        let DockConfig {
            placement,
            order,
            size,
            mode,
            search,
            autohide,
        } = *config;
        self.app_icon_size.set(size);
        self.autohide.set(autohide);
        self.update(Some("configure"), move |s| {
            if s.status_placement == placement
                && s.order == order
                && s.app_icon_size == size
                && s.window_list_mode == mode
                && s.search_button == search
                && s.autohide == autohide
            {
                return;
            }
            s.finish_press(false);
            s.hide_tip();
            s.hide_windows();
            s.hover = None;
            s.hover_part = None;
            if s.status_placement != placement {
                s.radio_worker.hide();
                s.radios = status::Radios::default();
            }
            s.status_placement = placement;
            s.order = order;
            s.app_icon_size = size;
            s.window_list_mode = mode;
            s.search_button = search;
            s.autohide = autohide;
            s.away = 0;
            if !autohide && s.visible && !s.suspended {
                s.revealed = true;
            }
            s.refresh();
        });
    }

    pub fn reload_pins(&mut self) {
        self.update(Some("reload_pins"), move |s| {
            s.finish_press(false);
            let pins = load_pins();
            if pins == s.pinned {
                return;
            }
            s.pinned = pins;
            s.migrate_pins();
            s.update_pins();
        });
    }

    pub fn refresh(&mut self) {
        self.update(Some("refresh"), move |s| {
            s.refresh();
        });
    }

    pub fn apply_theme(&mut self, dark: bool, backdrop: Backdrop, tint: Option<Tint>) {
        self.update(Some("apply_theme"), move |s| {
            s.finish_press(false);
            s.apply_material(dark, backdrop, tint);
        });
    }

    /// Hosts the notification area in the dock while `enabled`, which
    /// should follow the hidden taskbar. Failure leaves icons to Explorer.
    pub fn set_tray(&mut self, enabled: bool) {
        self.update(Some("set_tray"), move |s| {
            s.finish_press(false);
            if enabled == s.tray_host.is_some() {
                return;
            }
            if enabled {
                match TrayHost::start(s.hwnd, WM_TRAY_CHANGED) {
                    Ok(host) => s.tray_host = Some(host),
                    Err(error) => {
                        eprintln!("Dock notification area: {error}");
                        return;
                    }
                }
            } else {
                s.close_flyout();
                s.hide_tip();
                s.tray_host = None;
            }
            s.sync_tray();
        });
    }

    /// Keeps notification icons reaching Wisp after Explorer shows its taskbar.
    pub fn outrank_tray(&self) {
        self.update(Some("outrank_tray"), move |s| {
            if let Some(host) = &s.tray_host {
                host.outrank();
            }
        });
    }

    /// Lets the dock auto-hide: it appears when the pointer reaches the
    /// bottom edge of a screen.
    pub fn show(&mut self) {
        self.update(Some("show"), move |s| {
            if s.visible && !s.suspended {
                return;
            }
            s.suspended = false;
            s.visible = true;
            // A dock that stays visible comes back in place, without sliding.
            s.revealed = !s.autohide;
            if !s.revealed {
                s.radio_worker.hide();
            }
            s.surface_rect = None;
            s.stop_slide();
            s.start_poll(POLL_MS);
            s.refresh();
        });
    }
    /// Reveals the dock for good, with a fixed hover, so screenshots can show it
    /// without moving the pointer.
    pub fn hold(&mut self, hover: Option<usize>, hover_part: Option<&str>, dpi: Option<u32>) {
        let hover_part = hover_part.map(str::to_owned);
        self.update(Some("hold"), move |s| {
            let hover_part = hover_part.as_deref();
            s.held = true;
            s.held_dpi = dpi;
            s.revealed = true;
            s.hover = hover;
            s.animate_hover();
            s.hover_part = match hover_part {
                Some("volume" | "battery" | "status") => Some(logic::Part::Status),
                Some("clock") => Some(logic::Part::Clock),
                Some("chevron") => Some(logic::Part::Chevron),
                Some(part) => part
                    .strip_prefix("tray")
                    .and_then(|i| i.parse().ok())
                    .map(logic::Part::Tray),
                _ => None,
            };
            // "flyout" opens the flyout, "flyout2" also hovers its third icon.
            s.held_flyout = hover_part
                .and_then(|part| part.strip_prefix("flyout"))
                .map(|rest| rest.parse().ok());
            s.reveal();
            s.stop_slide();
            s.place();
            if hover_part == Some("desktops") {
                s.show_desktops(desktops::read());
            }
            s.sync_tray();
        });
    }
    /// Whether the dock stays on screen instead of auto-hiding.
    pub fn stays_visible(&self) -> bool {
        !self.autohide.get()
    }

    /// The room a dock that stays visible keeps at the bottom of the primary
    /// screen, in DIPs: its height and the margins above and below it.
    pub fn reserved_height(&self) -> f32 {
        let scale = logic::layout_scale(self.app_icon_size.get());
        logic::height(scale) + 2.0 * logic::margin(scale)
    }

    /// Hides the dock until `show`, such as while the launcher is open.
    pub fn hide(&mut self) {
        self.update(Some("hide"), move |s| {
            s.finish_press(false);
            s.close_flyout();
            s.hide_tip();
            s.hide_windows();
            s.visible = false;
            s.stop_slide();
            s.revealed = false;
            s.radio_worker.hide();
            s.listen(false);
            unsafe {
                let _ = KillTimer(Some(s.hwnd), POLL_TIMER);
                let _ = ShowWindow(s.hwnd, SW_HIDE);
            }
        });
    }
}

impl Drop for Dock {
    fn drop(&mut self) {
        EVENT_TARGET.with(|target| target.set(HWND::default()));
        if let Ok(mut s) = self.state.try_borrow_mut() {
            s.finish_press(false);
            s.listen(false);
            s.hide_windows();
        }
        unsafe {
            for hook in self.hooks.drain(..) {
                let _ = UnhookWinEvent(hook);
            }
            let _ = KillTimer(Some(self.hwnd), REFRESH_TIMER);
            let _ = KillTimer(Some(self.hwnd), POLL_TIMER);
            let _ = KillTimer(Some(self.hwnd), SLIDE_TIMER);
            let _ = KillTimer(Some(self.hwnd), TIP_TIMER);
            let _ = KillTimer(Some(self.hwnd), HOVER_TIMER);
            let _ = KillTimer(Some(self.hwnd), DESKTOP_TIMER);
            let _ = KillTimer(Some(self.hwnd), WINDOWS_TIMER);
            let _ = KillTimer(Some(self.hwnd), WINDOWS_HIDE_TIMER);
            let _ = KillTimer(Some(self.hwnd), CLOCK_TIMER);
            // Remove the pointer before synchronous destruction can reenter the wndproc.
            for hwnd in self.popups.iter().copied().chain(std::iter::once(self.hwnd)) {
                if !hwnd.is_invalid() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
            }
            // Destroys the owned flyout, tooltip and window list too.
            if self.shared {
                for hwnd in &self.popups {
                    let _ = DestroyWindow(*hwnd);
                }
            } else {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

impl State {
    /// Paint directly: UpdateWindow would reenter a wndproc whose state is borrowed.
    fn paint_now(&mut self) {
        match self.paint(None) {
            Ok(()) => unsafe {
                let _ = ValidateRect(Some(self.hwnd), None);
            },
            Err(error) => {
                eprintln!("Dock redraw: {error}");
                unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
            }
        }
    }

    fn paint(&mut self, rt: Option<&windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget>) -> Result<()> {
        let hwnd = self.hwnd;
        let parts = self.parts();
        let layout = self.row_layout();
        let State {
            renderer,
            items,
            icons,
            palette,
            dpi,
            hover,
            hover_levels,
            app_icon_size,
            capacity,
            scroll,
            status,
            radios,
            hover_part,
            tray,
            inline,
            flyout_open,
            ..
        } = self;
        let frame = &render::Frame {
            layout,
            dpi: *dpi,
            palette,
            items,
            icons,
            hover: *hover,
            hover_levels,
            app_icon_size: *app_icon_size,
            layout_scale: logic::layout_scale(*app_icon_size),
            capacity: *capacity,
            scroll: *scroll,
            status,
            radios,
            hover_part: *hover_part,
            parts: &parts,
            tray,
            inline,
            flyout_open: *flyout_open,
        };
        match rt {
            Some(rt) => renderer.draw_on(rt, frame),
            None => renderer.draw(hwnd, frame),
        }
    }

    fn apply_material(&mut self, dark: bool, backdrop: Backdrop, tint: Option<Tint>) {
        let key = crate::theme::MaterialKey::new(dark, backdrop, tint);
        if self.material == Some(key) {
            return;
        }
        self.material = Some(key);
        self.backdrop = backdrop;
        self.tint = tint;
        self.dark = dark;
        self.palette = crate::theme::apply_backdrop(self.hwnd, dark, backdrop, true, tint);
        // Popups over the dock (flyouts, tooltips, window lists) wear the same color.
        for hwnd in [self.flyout, self.tip, self.windows] {
            crate::theme::apply_backdrop(hwnd, dark, backdrop, true, tint);
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }

    fn refresh(&mut self) {
        if self.dragging() {
            return;
        }
        if self.suspended || !self.visible {
            return;
        }
        // Other apps' windows change constantly. A hidden dock skips the window
        // scan and catches up in `reveal`.
        if self.visible && !self.revealed && self.slide.is_none() && !self.held {
            unsafe {
                if IsWindowVisible(self.hwnd).as_bool() {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
            }
            return;
        }
        let before = self.layout_unchanged_ok.then(|| items_key(&self.items));
        let windows = open_windows::list();
        let pins = pins_key(&self.pinned);
        if pins != self.matched_pins {
            self.matched_pins = pins;
            self.app_matches.clear();
        }
        self.window_ids
            .retain(|hwnd, _| windows.iter().any(|w| w.hwnd == *hwnd));
        let now = Instant::now();
        let mut items: Vec<Item> = Vec::new();
        for window in windows {
            let aumid = match self.window_ids.get(&window.hwnd) {
                Some(id) if id.exe == window.exe && now.duration_since(id.read) < WINDOW_ID_TTL => id.aumid.clone(),
                _ => {
                    // A window that names no app may still belong to a packaged one,
                    // whose process does.
                    let aumid = window_aumid(HWND(window.hwnd as _)).or_else(|| {
                        logic::package_family(&window.exe).and_then(|_| open_windows::window_process_aumid(window.hwnd))
                    });
                    self.window_ids.insert(
                        window.hwnd,
                        WindowId {
                            exe: window.exe.clone(),
                            aumid: aumid.clone(),
                            read: now,
                        },
                    );
                    aumid
                }
            };
            let (pinned, apps) = (&self.pinned, &self.apps);
            let app = self
                .app_matches
                .entry((window.exe.clone(), aumid.clone()))
                .or_insert_with(|| {
                    logic::match_app(pinned, &window.exe, aumid.as_deref())
                        .or_else(|| logic::match_app(apps, &window.exe, aumid.as_deref()))
                        .cloned()
                })
                .clone()
                .unwrap_or_else(|| {
                    let id = aumid.filter(|s| !s.is_empty()).unwrap_or_else(|| window.exe.clone());
                    AppEntry {
                        aliases: Vec::new(),
                        name: open_windows::program_name(&window.exe).into(),
                        id,
                        path: (!window.exe.is_empty()).then(|| window.exe.clone()),
                    }
                });
            if app.id.is_empty() {
                continue;
            }
            if let Some(item) = items.iter_mut().find(|item| item.app.id.eq_ignore_ascii_case(&app.id)) {
                item.windows.push(window);
            } else {
                items.push(Item {
                    app,
                    windows: vec![window],
                });
            }
        }
        // A slot may now belong to another app, or contain different windows.
        let previous = self
            .windows_slot
            .and_then(|slot| self.item_at(slot))
            .map(|item| (item.app.id.clone(), item.app.name.clone(), item.windows.clone()));
        self.items = logic::stable_order(std::mem::take(&mut self.items), items, |old, new| {
            old.app.id.eq_ignore_ascii_case(&new.app.id)
        });
        self.order_pins();
        if let Some(slot) = self.windows_slot
            && self.item_at(slot).is_none_or(|item| {
                !logic::show_window_list(self.window_list_mode, item.windows.len())
                    || previous.as_ref().is_none_or(|(id, name, windows)| {
                        !id.eq_ignore_ascii_case(&item.app.id)
                            || *name != item.app.name
                            || windows.len() != item.windows.len()
                            || windows
                                .iter()
                                .any(|old| !item.windows.iter().any(|new| old.hwnd == new.hwnd))
                    })
            })
        {
            self.hide_windows();
        }
        let status = status::read(self.status_placement);
        let unchanged = before.is_some_and(|before| before == items_key(&self.items)) && status == self.status;
        self.status = status;
        if !unchanged {
            self.place();
        }
        if self.suspended {
            return;
        }
        self.fullscreen = fullscreen_foreground(self.hwnd);
        if !self.fullscreen {
            self.refresh_window_titles();
        }
        if self.fullscreen {
            self.finish_press(false);
            self.hide_windows();
            self.stop_slide();
            self.revealed = false;
            self.radio_worker.hide();
        }
        unsafe {
            let was_shown = IsWindowVisible(self.hwnd).as_bool();
            let shown = self.visible && (self.revealed || self.slide.is_some()) && !self.fullscreen;
            let _ = ShowWindow(self.hwnd, if shown { SW_SHOWNOACTIVATE } else { SW_HIDE });
            if unchanged && shown && was_shown {
                // Nothing to draw: the same apps, windows and status.
            } else if shown && !was_shown {
                // Layout no longer reorders the dock; come back above other
                // topmost windows only when it appears.
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
                self.paint_now();
            } else {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    fn apply_apps(&mut self, apps: &[AppEntry], icons: &HashMap<String, Arc<IconPixels>>) {
        let mut changed = self.apps != apps;
        if changed {
            self.apps = apps.to_vec();
            self.app_matches.clear();
            self.migrate_pins();
        }
        for (id, pixels) in icons {
            if !self.icons.contains_key(id) {
                self.icons.insert(id.clone(), pixels.clone());
                changed = true;
            }
        }
        if changed {
            self.refresh();
        }
    }

    fn order_pins(&mut self) {
        if self.pinned.is_empty() {
            self.items.retain(|item| !item.windows.is_empty());
            return;
        }
        let running = std::mem::take(&mut self.items)
            .into_iter()
            .filter(|item| !item.windows.is_empty())
            .collect();
        self.items = logic::pinned_first(
            self.pinned.iter().map(|pin| Item {
                app: AppEntry {
                    aliases: Vec::new(),
                    id: pin.id.clone(),
                    name: pin.name.clone(),
                    path: pin.path.clone(),
                },
                windows: Vec::new(),
            }),
            running,
            |a, b| a.app.id.eq_ignore_ascii_case(&b.app.id),
        );
    }

    /// Moves pins made from a Store app's exe to the app's ID, once the index knows it.
    fn migrate_pins(&mut self) {
        if logic::migrate_packaged_pins(&mut self.pinned, &self.apps) {
            self.save_pins();
        }
    }

    fn update_pins(&mut self) {
        self.hide_windows();
        self.hide_tip();
        self.hover = None;
        self.order_pins();
        self.place();
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn row_layout(&self) -> logic::RowLayout {
        logic::RowLayout {
            launcher: self.search_button,
            apps: logic::AppLayout {
                count: self.count(),
                pinned: self.pinned.len().saturating_sub(self.scroll).min(self.count()),
            },
            order: self.order,
            status: self.status_placement,
            percent_width: self.percent_width,
            tray: self.tray_host.as_ref().map(|_| self.inline.len()),
        }
    }

    fn count(&self) -> usize {
        self.items.len().saturating_sub(self.scroll).min(self.capacity)
    }

    /// Slides the dock in when the pointer reaches a bottom edge, and out once
    /// it has been away for a moment.
    fn poll(&mut self) {
        let _span = crate::util::perf::Span::new("dock_poll");
        let mut cursor = POINT::default();
        if unsafe { GetCursorPos(&mut cursor) }.is_err() {
            return;
        }
        let hidden = !self.revealed && self.slide.is_none();
        if !hidden && fullscreen_foreground(self.hwnd) {
            self.finish_press(false);
            self.stop_slide();
            self.revealed = false;
            self.radio_worker.hide();
            self.close_flyout();
            self.hide_tip();
            self.hide_windows();
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            return;
        }
        if self.press.is_some() && unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } >= 0 {
            self.finish_press(true);
        }
        if !self.revealed {
            if !self.autohide {
                // Back after a fullscreen app, or after the launcher closed.
                if !fullscreen_foreground(self.hwnd) {
                    self.reveal();
                }
                return;
            }
            // Dragging along the bottom edge, such as a scroll bar, is not a request for the dock.
            let dragging =
                unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0 };
            let moved = cursor != self.last_cursor;
            self.last_cursor = cursor;
            let monitor = cursor_monitor(cursor);
            let at_edge = monitor.is_some_and(|m| cursor.y >= m.bottom - 2);
            let near_edge = monitor.is_some_and(|m| cursor.y >= m.bottom - (m.bottom - m.top) / 3);
            if !dragging && at_edge && !fullscreen_foreground(self.hwnd) {
                self.listen(false);
                self.reveal();
                self.start_poll(POLL_MS);
            } else if !near_edge {
                // Away from the edge, the slow poll is enough.
                self.listen(false);
                self.set_poll(POLL_MS);
            } else if moved {
                // Over a full-screen app, the slow poll is enough. The check
                // runs only when the pointer starts to move, so a resting
                // pointer costs no more than before.
                self.still_polls = 0;
                self.listen(false);
                let fast = self.poll_ms == NEAR_POLL_MS || !fullscreen_foreground(self.hwnd);
                self.set_poll(if fast { NEAR_POLL_MS } else { POLL_MS });
            } else if self.still_polls < STILL_POLLS {
                // A resting pointer can't reach the edge. Poll slowly until it
                // moves again, and let Windows report that movement unless a
                // full-screen app is in front. Checked once per rest.
                self.still_polls += 1;
                if self.still_polls == STILL_POLLS {
                    if !fullscreen_foreground(self.hwnd) {
                        self.listen(true);
                    }
                    self.set_poll(POLL_MS);
                }
            }
            return;
        }
        self.listen(false);
        // Once the dock hides, a resting pointer counts its rest afresh.
        self.still_polls = 0;
        if self.autohide && self.poll_ms != POLL_MS {
            // HIDE_AFTER counts slow polls.
            self.start_poll(POLL_MS);
        }
        let mut rect = RECT::default();
        let _ = unsafe { GetWindowRect(self.hwnd, &mut rect) };
        if self.flyout_open && !self.held {
            // Activation closes the flyout on a click elsewhere; this covers
            // clicks while it could not take the foreground.
            let pressed =
                unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0 };
            let mut flyout = RECT::default();
            let _ = unsafe { GetWindowRect(self.flyout, &mut flyout) };
            let inside =
                |r: &RECT| cursor.x >= r.left && cursor.x < r.right && cursor.y >= r.top && cursor.y < r.bottom;
            if pressed && !inside(&flyout) && !inside(&rect) {
                self.close_flyout();
            }
        }
        let slack = (12 * self.dpi / 96) as i32;
        // The gap below the dock counts as on it, so the pointer can rest on the edge.
        // The dock stays while its flyout is open.
        let mut windows_rect = RECT::default();
        let windows_open = unsafe { IsWindowVisible(self.windows) }.as_bool()
            && unsafe { GetWindowRect(self.windows, &mut windows_rect) }.is_ok();
        let over_windows = windows_open
            && cursor.x >= windows_rect.left
            && cursor.x < windows_rect.right
            && cursor.y >= windows_rect.top
            && cursor.y < windows_rect.bottom;
        let near = !self.autohide
            || self.held
            || self.press.is_some()
            || self.flyout_open
            || over_windows
            || cursor.x >= rect.left - slack && cursor.x < rect.right + slack && cursor.y >= rect.top - slack;
        if near {
            self.away = 0;
        } else {
            self.away += 1;
            if self.away >= HIDE_AFTER {
                self.hide_tip();
                self.hide_windows();
                self.revealed = false;
                self.radio_worker.hide();
                self.start_slide(1.0);
                return;
            }
        }
        if !self.autohide {
            // Nothing to count: only fullscreen apps and clicks away from the
            // flyout matter, and the flyout and window list need a quick answer.
            let busy = self.press.is_some() || self.flyout_open || windows_open || self.held;
            self.set_poll(if busy { POLL_MS } else { QUIET_POLL_MS });
        }
    }

    /// Starts or restarts the poll timer at `ms`.
    fn start_poll(&mut self, ms: u32) {
        self.poll_ms = ms;
        unsafe {
            SetTimer(Some(self.hwnd), POLL_TIMER, ms, None);
        }
    }

    /// Changes the poll timer to `ms` unless it already runs at that interval.
    fn set_poll(&mut self, ms: u32) {
        if ms != self.poll_ms {
            self.start_poll(ms);
        }
    }

    /// Asks Windows to report pointer movement (`WM_INPUT`), or stops it. Only
    /// a resting pointer listens; a moving one is followed by the fast poll.
    fn listen(&mut self, on: bool) {
        if self.listening == on {
            return;
        }
        let device = RAWINPUTDEVICE {
            // The generic desktop page's mouse.
            usUsagePage: 0x01,
            usUsage: 0x02,
            dwFlags: if on { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
            hwndTarget: if on { self.hwnd } else { HWND::default() },
        };
        let result = unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) };
        if let Err(error) = &result {
            eprintln!("Dock pointer listener: {error}");
        }
        // Without the listener, a resting pointer keeps the slow poll as before.
        self.listening = on && result.is_ok();
    }

    /// The pointer moved while resting near the edge: follow it with the fast poll.
    fn wake(&mut self) {
        if self.listening {
            self.listen(false);
            self.still_polls = 0;
            self.start_poll(NEAR_POLL_MS);
        }
    }

    /// Shows the dock, or reverses an outgoing slide at its current position.
    fn reveal(&mut self) {
        let hidden = !self.revealed && self.slide.is_none();
        self.revealed = true;
        self.update_status();
        self.away = 0;
        if hidden {
            self.slide_to = 1.0;
        }
        self.start_slide(0.0);
        self.refresh();
    }

    fn slide_value(&self) -> f32 {
        let Some(start) = self.slide else { return self.slide_to };
        logic::slide_value(
            self.slide_from,
            self.slide_to,
            start.elapsed().as_secs_f32() * 1000.0 / crate::util::perf::slowdown() / SLIDE_MS,
        )
    }

    fn start_slide(&mut self, to: f32) {
        if to > 0.5 {
            self.hide_windows();
        }
        crate::util::perf::event("slide_begin");
        self.slide_from = self.slide_value();
        self.slide_to = to;
        self.slide = Some(Instant::now());
        if !self.frame_paced {
            unsafe {
                SetTimer(Some(self.hwnd), SLIDE_TIMER, 10, None);
            }
        }
    }

    fn stop_slide(&mut self) {
        self.slide = None;
        self.slide_from = 0.0;
        self.slide_to = 0.0;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), SLIDE_TIMER);
        }
    }

    fn slide_offset(&self, height: i32) -> i32 {
        let layout_scale = logic::layout_scale(self.app_icon_size);
        ((height + (logic::margin(layout_scale) * self.dpi as f32 / 96.0).round() as i32) as f32 * self.slide_value())
            .round() as i32
    }

    fn step_slide(&mut self) {
        let _span = crate::util::perf::Span::new("slide");
        crate::util::perf::event("slide_frame");
        let tick = Instant::now();
        let elapsed = self.slide.map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0);
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rect);
            let offset = self.slide_offset(rect.bottom - rect.top);
            let _ = SetWindowPos(
                self.hwnd,
                None,
                rect.left,
                self.top + offset,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            self.publish_rects();
            if self
                .slide
                .is_some_and(|start| start.elapsed().as_secs_f32() * 1000.0 / crate::util::perf::slowdown() >= SLIDE_MS)
            {
                self.slide = None;
                let _ = KillTimer(Some(self.hwnd), SLIDE_TIMER);
                if !self.revealed {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
            }
        }
        if crate::util::perf::hook_on("WISP_FRAME_TIMING") {
            eprintln!(
                "slide frame timestamp_ms={elapsed:.3} work_ms={:.3}",
                tick.elapsed().as_secs_f64() * 1000.0
            );
        }
    }

    fn animate_hover(&mut self) {
        if self.hover_tick.is_none() {
            self.hover_tick = Some(Instant::now());
        }
        unsafe {
            SetTimer(Some(self.hwnd), HOVER_TIMER, 10, None);
        }
    }

    fn step_hover(&mut self) {
        let now = Instant::now();
        let step = self.hover_tick.replace(now).map_or(1.0, |last| {
            now.duration_since(last).as_secs_f32() * 1000.0 / crate::util::perf::slowdown() / HOVER_MS
        });
        self.hover_levels.resize(self.items.len() + 1, 0.0);
        let mut moving = false;
        for (index, level) in self.hover_levels.iter_mut().enumerate() {
            let goal = if self.hover == Some(index) { 1.0 } else { 0.0 };
            *level += (goal - *level).clamp(-step, step);
            moving |= *level != goal;
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            if !moving {
                self.hover_tick = None;
                let _ = KillTimer(Some(self.hwnd), HOVER_TIMER);
            }
        }
    }

    fn place(&mut self) {
        let previous = (self.dpi, self.left, self.top, self.width, self.scroll, self.capacity);
        if self.suspended {
            return;
        }
        match self.renderer.percent_width(logic::layout_scale(self.app_icon_size)) {
            Ok(width) => self.percent_width = width,
            Err(error) => eprintln!("Dock percentage width: {error}"),
        }
        unsafe {
            // The dock appears on the screen whose bottom edge the pointer
            // touched, or stays on the primary screen.
            let mut cursor = POINT::default();
            if self.autohide {
                let _ = GetCursorPos(&mut cursor);
            }
            let monitor = if self.dragging() {
                MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTOPRIMARY)
            } else {
                MonitorFromPoint(cursor, MONITOR_DEFAULTTOPRIMARY)
            };
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return;
            }
            // Move first, then query the window's new monitor DPI. Skipped on the
            // same monitor, where the jump would show.
            if MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONULL) != monitor {
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    info.rcMonitor.left,
                    info.rcMonitor.bottom - 60,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOREDRAW,
                );
            }
            self.dpi = self.held_dpi.unwrap_or_else(|| GetDpiForWindow(self.hwnd)).max(96);
            let scale = self.dpi as f32 / 96.0;
            // Notification icons are rendered for the DPI they are shown at.
            let size = tray::icon_size(self.dpi);
            if self
                .tray
                .iter()
                .any(|shown| shown.pixels.as_ref().is_some_and(|p| p.width != size as u32))
            {
                let icons = std::mem::take(&mut self.tray)
                    .into_iter()
                    .map(|shown| shown.icon)
                    .collect();
                (self.tray, self.inline) = tray::update(Vec::new(), icons, size);
            }
            let size = logic::icon_pixels(self.dpi, self.app_icon_size);
            self.artwork.drain();
            for item in &self.items {
                {
                    let path = item
                        .app
                        .path
                        .clone()
                        .unwrap_or_else(|| format!("shell:AppsFolder\\{}", item.app.id));
                    if let Some(pixels) = self.artwork.request(self.hwnd, &item.app.id, &path, size) {
                        self.icons.insert(item.app.id.clone(), pixels);
                    }
                }
            }
            let layout_scale = logic::layout_scale(self.app_icon_size);
            let mut fixed = self.row_layout();
            fixed.apps = logic::AppLayout { count: 1, pinned: 0 };
            let available = (info.rcMonitor.right - info.rcMonitor.left) as f32 / scale
                - 8.0 * layout_scale
                - fixed.width(layout_scale)
                + logic::slot(layout_scale)
                - if !self.pinned.is_empty() && self.items.len() > self.pinned.len() {
                    logic::separator_width(layout_scale)
                } else {
                    0.0
                };
            self.capacity = ((available / logic::slot(layout_scale)) as usize).max(1);
            self.scroll = self.scroll.min(self.items.len().saturating_sub(self.capacity));
            let width = (self.row_layout().width(layout_scale) * scale).ceil() as i32;
            let height = (logic::height(layout_scale) * scale).round() as i32;
            let x = info.rcMonitor.left + (info.rcMonitor.right - info.rcMonitor.left - width) / 2;
            self.top = info.rcMonitor.bottom - height - (logic::margin(layout_scale) * scale).round() as i32;
            self.left = x;
            self.width = width;
            let y = self.top + self.slide_offset(height);
            let mut old = RECT::default();
            let known = GetWindowRect(self.hwnd, &mut old).is_ok();
            let resized = !known || old.right - old.left != width || old.bottom - old.top != height;
            let flags = SWP_NOACTIVATE | SWP_NOZORDER | if resized { SWP_NOCOPYBITS } else { SWP_NOSIZE };
            if resized || old.left != x || old.top != y {
                let _ = SetWindowPos(self.hwnd, None, x, y, width, height, flags);
            }
            if resized {
                self.paint_now();
            }
        }
        if previous != (self.dpi, self.left, self.top, self.width, self.scroll, self.capacity) {
            self.hide_windows();
        }
        // The chevron and icons move whenever the dock's width changes.
        if self.flyout_open {
            self.layout_flyout();
        }
        if unsafe { IsWindowVisible(self.tip) }.as_bool() {
            self.show_tip();
        }
        self.publish_rects();
    }

    /// The status parts, with the notification area while the dock hosts it.
    fn parts(&self) -> Vec<logic::Part> {
        self.status.parts(
            self.tray_host.as_ref().map(|_| self.inline.len()),
            self.status_placement,
        )
    }

    /// A part's rectangle on screen where the dock rests, in physical pixels.
    fn part_rect(&self, part: logic::Part) -> Option<RECT> {
        let scale = self.dpi as f32 / 96.0;
        let left = self
            .row_layout()
            .part_left(part, logic::layout_scale(self.app_icon_size))?;
        let px = |dip: f32| (dip * scale).round() as i32;
        let rect = self.surface_rect;
        let x = |dip| match rect {
            Some(rect) => {
                rect.left + (px(dip) as f32 * (rect.right - rect.left) as f32 / self.width.max(1) as f32).round() as i32
            }
            None => self.left + px(dip),
        };
        let layout_scale = logic::layout_scale(self.app_icon_size);
        let height = logic::height(layout_scale);
        let top = rect.map_or(self.top + self.slide_offset(px(height)), |r| r.bottom - px(height));
        Some(RECT {
            left: x(left),
            top,
            right: x(left + self.row_layout().part_width(part, layout_scale)),
            bottom: top + px(height),
        })
    }

    /// An app slot's rectangle on screen where the dock rests, in physical pixels.
    fn item_rect(&self, slot: usize) -> RECT {
        let scale = self.dpi as f32 / 96.0;
        let px = |dip: f32| (dip * scale).round() as i32;
        let rect = self.surface_rect;
        let x = |dip| match rect {
            Some(rect) => {
                rect.left + (px(dip) as f32 * (rect.right - rect.left) as f32 / self.width.max(1) as f32).round() as i32
            }
            None => self.left + px(dip),
        };
        let layout_scale = logic::layout_scale(self.app_icon_size);
        let height = logic::height(layout_scale);
        let left = self.row_layout().left(slot, layout_scale);
        let top = rect.map_or(self.top + self.slide_offset(px(height)), |r| r.bottom - px(height));
        RECT {
            left: x(left),
            top,
            right: x(left + logic::slot(layout_scale)),
            bottom: top + px(height),
        }
    }

    /// A flyout cell's rectangle on screen, in physical pixels.
    fn cell_rect(&self, index: usize) -> RECT {
        let scale = self.dpi as f32 / 96.0;
        let mut flyout = RECT::default();
        let _ = unsafe { GetWindowRect(self.flyout, &mut flyout) };
        let (left, top) = logic::flyout_cell(index);
        let px = |dip: f32| (dip * scale).round() as i32;
        RECT {
            left: flyout.left + px(left),
            top: flyout.top + px(top),
            right: flyout.left + px(left + logic::FLYOUT_CELL),
            bottom: flyout.top + px(top + logic::FLYOUT_CELL),
        }
    }

    /// Tells the tray host where icons are, for apps that place their
    /// windows by Shell_NotifyIconGetRect. Hidden icons point at the chevron.
    fn publish_rects(&self) {
        let Some(host) = &self.tray_host else { return };
        let mut rects = tray_host::Rects {
            fallback: self.part_rect(logic::Part::Chevron),
            ..Default::default()
        };
        for (position, &index) in self.inline.iter().enumerate() {
            if let Some(rect) = self.part_rect(logic::Part::Tray(position)) {
                rects.icons.insert(self.tray[index].icon.key(), rect);
            }
        }
        if self.flyout_open {
            for (index, shown) in self.tray.iter().enumerate() {
                rects.icons.insert(shown.icon.key(), self.cell_rect(index));
            }
        }
        host.set_rects(rects);
    }

    /// Rereads the tray host's icons.
    fn sync_tray(&mut self) {
        let icons = self.tray_host.as_ref().map(TrayHost::icons).unwrap_or_default();
        let size = tray::icon_size(self.dpi);
        (self.tray, self.inline) = tray::update(std::mem::take(&mut self.tray), icons, size);
        if self.flyout_hover.is_some_and(|i| i >= self.tray.len()) {
            self.flyout_hover = None;
        }
        let stale = match self.tip_target {
            Some(tray::Tip::Inline(i)) => i >= self.inline.len(),
            Some(tray::Tip::Flyout(i)) => i >= self.tray.len(),
            Some(tray::Tip::Chevron) => self.tray_host.is_none(),
            Some(tray::Tip::Status(_)) => false,
            // The desktop pill has nothing to do with the tray.
            None => false,
        };
        if stale {
            self.hide_tip();
        }
        if self.tray_host.is_none() {
            self.close_flyout();
        }
        self.place();
        if self.held {
            self.apply_hold();
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// Shows the demo's held flyout or tooltip once the icons are known.
    fn apply_hold(&mut self) {
        if self.tray_host.is_none() && !matches!(self.hover_part, Some(logic::Part::Status | logic::Part::Icon(_))) {
            return;
        }
        if let Some(hover) = self.held_flyout {
            if !self.flyout_open {
                self.open_flyout();
            }
            self.flyout_hover = hover.filter(|&i| i < self.tray.len());
            self.tip_target = self.flyout_hover.map(tray::Tip::Flyout);
        } else {
            self.tip_target = match self.hover_part {
                Some(part @ (logic::Part::Status | logic::Part::Icon(_))) => Some(tray::Tip::Status(part)),
                Some(logic::Part::Chevron) => Some(tray::Tip::Chevron),
                Some(logic::Part::Tray(i)) if i < self.inline.len() => Some(tray::Tip::Inline(i)),
                _ => None,
            };
        }
        self.show_tip();
        unsafe {
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }
    }

    fn toggle_flyout(&mut self) {
        if self.flyout_open {
            self.close_flyout();
        } else {
            self.open_flyout();
        }
    }

    fn open_flyout(&mut self) {
        self.hide_tip();
        self.hide_windows();
        self.flyout_open = true;
        self.flyout_hover = None;
        self.layout_flyout();
        unsafe {
            let _ = ShowWindow(self.flyout, SW_SHOWNA);
            // The click on the chevron lets Wisp take the foreground, which
            // the flyout needs for Esc and to close on a click elsewhere.
            if !self.held {
                let _ = SetForegroundWindow(self.flyout);
            }
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
        self.publish_rects();
    }

    /// Sizes the flyout to its icons and centers it above the chevron.
    fn layout_flyout(&mut self) {
        let scale = self.dpi as f32 / 96.0;
        let (width, height) = logic::flyout_size(self.tray.len());
        let (width, height) = ((width * scale).round() as i32, (height * scale).round() as i32);
        let Some(chevron) = self.part_rect(logic::Part::Chevron) else {
            return;
        };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            let _ = GetMonitorInfoW(MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
        }
        let margin = (logic::FLYOUT_GAP * scale).round() as i32;
        let monitor = info.rcMonitor;
        let x = ((chevron.left + chevron.right - width) / 2)
            .min(monitor.right - margin - width)
            .max(monitor.left + margin);
        let y = self.top - margin - height;
        unsafe {
            let _ = SetWindowPos(self.flyout, Some(HWND_TOPMOST), x, y, width, height, SWP_NOACTIVATE);
            let _ = InvalidateRect(Some(self.flyout), None, false);
        }
    }

    fn close_flyout(&mut self) {
        if !self.flyout_open {
            return;
        }
        self.flyout_open = false;
        self.flyout_hover = None;
        if matches!(self.tip_target, Some(tray::Tip::Flyout(_))) {
            self.hide_tip();
        }
        unsafe {
            let _ = ShowWindow(self.flyout, SW_HIDE);
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
        self.publish_rects();
    }

    /// Sends a gesture on `tray[index]` to its app, as Explorer would.
    /// `center` is the icon's center on screen.
    fn click_tray(&mut self, index: usize, action: Action, center: POINT) {
        let Some(shown) = self.tray.get(index) else { return };
        let key = shown.icon.key();
        let double = Duration::from_millis(unsafe { GetDoubleClickTime() }.into());
        let action = match action {
            Action::Click
                if self
                    .tray_click
                    .is_some_and(|(last, at)| last == key && at.elapsed() < double) =>
            {
                self.tray_click = None;
                Action::DoubleClick
            }
            Action::Click => {
                self.tray_click = Some((key, Instant::now()));
                Action::Click
            }
            other => other,
        };
        self.hide_tip();
        if let Err(error) = tray_host::invoke(&self.tray[index].icon, action, center.x, center.y) {
            eprintln!("Dock notification icon: {error}");
        }
    }

    /// An inline icon's gesture; its anchor is the icon's center.
    fn click_inline(&mut self, position: usize, action: Action) {
        let (Some(&index), Some(rect)) = (self.inline.get(position), self.part_rect(logic::Part::Tray(position)))
        else {
            return;
        };
        let center = POINT {
            x: (rect.left + rect.right) / 2,
            y: (rect.top + rect.bottom) / 2,
        };
        self.click_tray(index, action, center);
    }

    /// The pointer rests on `target`; its tooltip follows after a moment.
    fn hover_tip(&mut self, target: Option<tray::Tip>) {
        // The open flyout already shows what the chevron's tooltip offers.
        let target = target.filter(|&t| !(t == tray::Tip::Chevron && self.flyout_open));
        if self.tip_target == target || self.held {
            return;
        }
        self.hide_tip();
        self.tip_target = target;
        if target.is_some() {
            self.hide_windows();
            unsafe {
                SetTimer(Some(self.hwnd), TIP_TIMER, TIP_MS, None);
            }
        }
    }

    /// Lists the windows of the app under the pointer, or of none.
    fn hover_windows(&mut self, slot: Option<usize>) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), WINDOWS_HIDE_TIMER);
        }
        let slot = slot.filter(|&slot| {
            self.window_list_mode != WindowListMode::Off
                && self
                    .item_at(slot)
                    .is_some_and(|item| logic::show_window_list(self.window_list_mode, item.windows.len()))
        });
        if self.windows_slot == slot {
            return;
        }
        let shown = self.windows_slot.is_some();
        self.hide_windows();
        self.windows_slot = slot;
        if slot.is_some() {
            self.hide_tip();
            unsafe {
                let _ = SetTimer(Some(self.hwnd), WINDOWS_TIMER, if shown { 0 } else { TIP_MS }, None);
            }
        }
    }

    /// The item in a visible slot.
    fn item_at(&self, slot: usize) -> Option<&Item> {
        slot.checked_sub(1).and_then(|i| self.items.get(i + self.scroll))
    }

    /// Hides the window list and forgets its windows.
    fn hide_windows(&mut self) {
        if !self.name_hook.is_invalid() {
            unsafe {
                let _ = UnhookWinEvent(self.name_hook);
            }
            self.name_hook = HWINEVENTHOOK::default();
        }
        self.windows_slot = None;
        self.windows_hover = None;
        self.windows_rows.clear();
        self.windows_targets.clear();
        unsafe {
            let _ = KillTimer(Some(self.hwnd), WINDOWS_TIMER);
            let _ = KillTimer(Some(self.hwnd), WINDOWS_HIDE_TIMER);
            let _ = ShowWindow(self.windows, SW_HIDE);
        }
    }

    /// Keeps the list a moment while the pointer crosses to it, then hides it.
    fn leave_windows(&mut self) {
        if self.windows_slot.is_none() {
            return;
        }
        let shown = unsafe { IsWindowVisible(self.windows) }.as_bool();
        if !shown {
            // It never appeared, so a pending hover is now stale.
            self.hide_windows();
            return;
        }
        unsafe {
            let _ = SetTimer(Some(self.hwnd), WINDOWS_HIDE_TIMER, WINDOWS_HIDE_MS, None);
        }
    }

    /// Draws the hovered app's windows above its icon.
    fn show_windows(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), WINDOWS_TIMER);
        }
        if !self.visible || self.suspended || self.fullscreen || !self.revealed || self.slide_to > 0.5 {
            self.hide_windows();
            return;
        }
        let Some(slot) = self.windows_slot else { return };
        let Some(item) = self.item_at(slot) else {
            self.hide_windows();
            return;
        };
        if !logic::show_window_list(self.window_list_mode, item.windows.len()) {
            self.hide_windows();
            return;
        }
        let rows: Vec<String> = item
            .windows
            .iter()
            .map(|window| match window.title.trim() {
                "" => item.app.name.clone(),
                title => title.to_string(),
            })
            .collect();
        let targets: Vec<isize> = item.windows.iter().map(|window| window.hwnd).collect();
        if rows.is_empty() {
            self.hide_windows();
            return;
        }
        self.windows_hover = None;
        if self.name_hook.is_invalid() {
            self.name_hook = unsafe {
                SetWinEventHook(
                    EVENT_OBJECT_NAMECHANGE,
                    EVENT_OBJECT_NAMECHANGE,
                    None,
                    Some(window_event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
        }
        self.layout_windows(slot, rows, targets);
    }

    /// Updates titles by handle, preserving the displayed order and click targets
    /// even when a foreground change has reordered the app's windows.
    fn refresh_window_titles(&mut self) {
        let Some(slot) = self.windows_slot.filter(|_| !self.windows_rows.is_empty()) else {
            return;
        };
        let Some(item) = self.item_at(slot) else { return };
        let mut rows = self.windows_rows.clone();
        for (row, target) in rows.iter_mut().zip(&self.windows_targets) {
            let Some(window) = item.windows.iter().find(|window| window.hwnd == *target) else {
                self.hide_windows();
                return;
            };
            *row = match window.title.trim() {
                "" => item.app.name.clone(),
                title => title.to_string(),
            };
        }
        if rows != self.windows_rows {
            self.layout_windows(slot, rows, self.windows_targets.clone());
        }
    }

    /// Measures and places the list after its row text changes.
    fn layout_windows(&mut self, slot: usize, mut rows: Vec<String>, mut targets: Vec<isize>) {
        let scale = self.dpi as f32 / 96.0;
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST), &mut info) }.as_bool() {
            self.hide_windows();
            return;
        }
        let anchor = self.item_rect(slot);
        let gap = (6.0 * scale).round() as i32;
        let available = anchor.top - gap - (info.rcWork.top + gap);
        let capacity = logic::window_list_rows(available, scale, logic::LIST_PAD.1);
        if capacity == 0 {
            self.hide_windows();
            return;
        }
        if rows.len() > capacity {
            // Keep the first (most recently used) windows. The final row only
            // reports the remainder and has no target or hover highlight.
            let visible = capacity - 1;
            let total = self.item_at(slot).map_or(targets.len(), |item| item.windows.len());
            rows.truncate(visible);
            targets.truncate(visible);
            rows.push(format!("+{} more", total - visible));
        }
        let Ok((width, height)) = self.windows_renderer.measure_windows(&rows) else {
            self.hide_windows();
            return;
        };
        let (width, height) = ((width * scale).ceil() as i32, (height * scale).ceil() as i32);
        let x = ((anchor.left + anchor.right - width) / 2)
            .min(info.rcWork.right - gap - width)
            .max(info.rcWork.left + gap);
        self.hide_tip();
        self.windows_hover = self.windows_hover.filter(|&row| row < targets.len());
        self.windows_rows = rows;
        self.windows_targets = targets;
        unsafe {
            let _ = SetWindowPos(
                self.windows,
                Some(HWND_TOPMOST),
                x,
                anchor.top - gap - height,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(Some(self.windows), None, false);
        }
    }

    /// The row of the window list under a popup pointer position.
    fn windows_hit(&self, lp: LPARAM) -> Option<usize> {
        let scale = self.dpi as f32 / 96.0;
        let y = ((lp.0 >> 16) as i16) as f32 / scale;
        let row = (y - logic::LIST_PAD.1) / logic::LIST_ROW;
        (row >= 0.0)
            .then_some(row as usize)
            .filter(|&row| row < self.windows_targets.len())
    }

    fn hide_tip(&mut self) {
        self.tip_target = None;
        self.desktop_pill = None;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), DESKTOP_TIMER);
            let _ = KillTimer(Some(self.hwnd), TIP_TIMER);
            let _ = ShowWindow(self.tip, SW_HIDE);
        }
    }

    /// Shows the tooltip above what the pointer rests on.
    fn show_tip(&mut self) {
        if matches!(self.tip_target, Some(tray::Tip::Status(_)))
            && self.visible
            && self.revealed
            && !self.suspended
            && !self.fullscreen
        {
            // Hovering wants fresh details, but not a read on every redraw.
            self.refresh_radios(status::RADIO_HOVER);
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIP_TIMER);
        }
        let (text, anchor) = match self.tip_target {
            Some(tray::Tip::Status(part)) => (
                self.status.tooltip_for(&self.radios, &self.status_items(part)),
                self.part_rect(part),
            ),
            Some(tray::Tip::Chevron) => ("Show hidden icons".to_string(), self.part_rect(logic::Part::Chevron)),
            Some(tray::Tip::Inline(i)) => (
                self.inline
                    .get(i)
                    .map(|&i| self.tray[i].icon.tip.clone())
                    .unwrap_or_default(),
                self.part_rect(logic::Part::Tray(i)),
            ),
            Some(tray::Tip::Flyout(i)) => (
                self.tray.get(i).map(|shown| shown.icon.tip.clone()).unwrap_or_default(),
                Some(self.cell_rect(i)),
            ),
            None => return,
        };
        let text = text.trim().to_string();
        let Some(anchor) = anchor.filter(|_| !text.is_empty()) else {
            return;
        };
        let scale = self.dpi as f32 / 96.0;
        let Ok((width, height)) = self.tip_renderer.measure_tip(&text, TIP_MAX) else {
            return;
        };
        let (width, height) = ((width * scale).ceil() as i32, (height * scale).ceil() as i32);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            let _ = GetMonitorInfoW(MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
        }
        let gap = (6.0 * scale).round() as i32;
        let x = ((anchor.left + anchor.right - width) / 2)
            .min(info.rcMonitor.right - gap - width)
            .max(info.rcMonitor.left + gap);
        self.tip_text = text;
        unsafe {
            let _ = SetWindowPos(
                self.tip,
                Some(HWND_TOPMOST),
                x,
                anchor.top - gap - height,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(Some(self.tip), None, false);
        }
    }

    /// Offers the taskbar for icons the dock lacks, and Windows' settings
    /// for which icons show in the dock.
    fn chevron_menu(&mut self) {
        self.close_flyout();
        self.hide_tip();
        self.hide_windows();
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let _ = AppendMenuW(menu, MF_STRING, 1, w!("Show taskbar"));
            let _ = AppendMenuW(menu, MF_STRING, 2, w!("Taskbar settings"));
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let _ = SetForegroundWindow(self.hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                None,
                self.hwnd,
                None,
            )
            .0;
            let _ = DestroyMenu(menu);
            match command {
                1 => {
                    let _ = PostMessageW(Some(self.owner), WM_DOCK_SHOW_TASKBAR, WPARAM(0), LPARAM(0));
                }
                2 => {
                    if let Err(error) = crate::launch::open("ms-settings:taskbar") {
                        eprintln!("Dock taskbar settings: {error}");
                    }
                }
                _ => {}
            }
        }
    }

    fn hit(&self, lp: LPARAM) -> Option<usize> {
        let scale = self.dpi as f32 / 96.0;
        self.row_layout().hit(
            (lp.0 as i16) as f32 / scale,
            ((lp.0 >> 16) as i16) as f32 / scale,
            logic::layout_scale(self.app_icon_size),
        )
    }

    fn hit_part(&self, lp: LPARAM) -> Option<logic::Part> {
        let scale = self.dpi as f32 / 96.0;
        self.row_layout().part_hit(
            (lp.0 as i16) as f32 / scale,
            ((lp.0 >> 16) as i16) as f32 / scale,
            &self.parts(),
            logic::layout_scale(self.app_icon_size),
        )
    }

    /// Opens Now, the calendar, or the tray target.
    fn click_part(&mut self, part: logic::Part) {
        let target = match part {
            logic::Part::Chevron => {
                self.toggle_flyout();
                return;
            }
            logic::Part::Tray(position) => {
                self.click_inline(position, Action::Click);
                return;
            }
            logic::Part::Status | logic::Part::Icon(_) => {
                unsafe {
                    let _ = PostMessageW(Some(self.owner), WM_SHOW_NOW, WPARAM(0), LPARAM(0));
                }
                return;
            }
            logic::Part::Clock => "ms-actioncenter:",
        };
        if let Err(error) = crate::launch::open(target) {
            eprintln!("Dock status: {error}");
        }
    }

    /// Wheel `delta` over the dock switches desktops a notch at a time,
    /// negative toward the right.
    fn wheel_desktops(&mut self, delta: i32) {
        if self.desktop_switched.is_some_and(|at| at.elapsed() < DESKTOP_PAUSE) {
            self.desktop_travel = 0;
            return;
        }
        let (step, travel) = logic::wheel_desktop(self.desktop_travel, delta);
        self.desktop_travel = travel;
        let Some(right) = step else { return };
        // Explorer updates its list after the switch, so a showing pill knows better.
        let mut desktops = self.desktop_pill.take().unwrap_or_else(desktops::read);
        let target = if right {
            desktops.current + 1
        } else {
            desktops.current.wrapping_sub(1)
        };
        // Like Windows, the ends don't wrap around. The pill still shows where you are.
        if target < desktops.names.len() {
            desktops::switch(right);
            desktops.current = target;
            self.desktop_switched = Some(Instant::now());
        }
        self.show_desktops(desktops);
    }

    /// Shows the desktops in a pill above the dock for a moment.
    fn show_desktops(&mut self, desktops: desktops::Desktops) {
        self.hide_windows();
        self.tip_target = None;
        let name = &desktops.names[desktops.current];
        let Ok((width, height)) = self.tip_renderer.measure_desktops(desktops.names.len(), name) else {
            return;
        };
        let scale = self.dpi as f32 / 96.0;
        let (width, height) = ((width * scale).ceil() as i32, (height * scale).ceil() as i32);
        let gap = (6.0 * scale).round() as i32;
        self.desktop_pill = Some(desktops);
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIP_TIMER);
            let mut dock = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut dock);
            let _ = SetWindowPos(
                self.tip,
                Some(HWND_TOPMOST),
                (dock.left + dock.right - width) / 2,
                dock.top - gap - height,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(Some(self.tip), None, false);
            if !self.held {
                SetTimer(Some(self.hwnd), DESKTOP_TIMER, DESKTOP_PILL_MS, None);
            }
        }
    }

    /// Wheel `delta` over sound's placement changes volume by 2% a notch.
    fn wheel_volume(&mut self, delta: i32) {
        let Some(volume) = self.status.volume else {
            return;
        };
        let (percent, rest) = logic::wheel_volume(volume.percent, self.wheel + delta);
        self.wheel = rest;
        if percent != volume.percent
            && let Err(error) = crate::commands::audio::set_volume(percent)
        {
            eprintln!("Dock volume: {error}");
        }
        self.update_status();
    }

    fn status_items(&self, part: logic::Part) -> Vec<logic::StatusItem> {
        match part {
            logic::Part::Icon(item) => vec![item],
            logic::Part::Status => logic::STATUS_ITEMS
                .into_iter()
                .filter(|item| self.status_placement.group[*item as usize])
                .collect(),
            _ => Vec::new(),
        }
    }

    fn refresh_radios(&mut self, age: std::time::Duration) {
        let wifi = self.status_placement.visible(logic::StatusItem::Wifi);
        let bluetooth = self.status_placement.visible(logic::StatusItem::Bluetooth);
        if wifi || bluetooth {
            self.radio_worker.refresh(self.hwnd, WM_RADIOS, age, wifi, bluetooth);
        }
    }

    fn refresh_status_tip(&mut self) {
        if matches!(self.tip_target, Some(tray::Tip::Status(_))) && unsafe { IsWindowVisible(self.tip) }.as_bool() {
            self.show_tip();
        }
    }

    /// Rereads the status, redrawing if it changed.
    fn update_status(&mut self) {
        let _span = crate::util::perf::Span::new("status");
        if self.visible && self.revealed && !self.suspended && !self.fullscreen {
            self.refresh_radios(status::RADIO_POLL);
        }
        let status = status::read(self.status_placement);
        if status != self.status {
            self.status = status;
            self.refresh_status_tip();
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    fn click(&mut self, index: usize, new_instance: bool) {
        if index == 0 {
            unsafe {
                let _ = PostMessageW(Some(self.owner), WM_SHOW_LAUNCHER, WPARAM(0), LPARAM(0));
            }
        } else if let Some(item) = self.items.get(index - 1 + self.scroll) {
            if !new_instance && let Some(window) = item.windows.first() {
                // Clicking the app already in front minimizes it, as its
                // taskbar button does; any other click brings it forward.
                if item.windows.iter().any(|w| open_windows::is_foreground(w.hwnd)) {
                    for window in &item.windows {
                        if let Err(error) = open_windows::minimize(window.hwnd) {
                            eprintln!("Dock minimize: {error}");
                        }
                    }
                } else if let Err(error) = open_windows::focus(window.hwnd) {
                    eprintln!("Dock activation: {error}");
                }
            } else {
                self.launch(&item.app, false);
            }
        }
    }

    /// Starts `app`, or another instance of it, as clicking it in Start would.
    fn launch(&self, app: &AppEntry, admin: bool) {
        let result = if self.apps.iter().any(|known| known.id == app.id) {
            crate::launch::app(&app.id, app.path.as_deref(), admin)
        } else if let Some(path) = &app.path {
            if admin {
                crate::launch::app(&app.id, Some(path), true)
            } else {
                crate::launch::open(path)
            }
        } else {
            crate::launch::app(&app.id, None, admin)
        };
        if let Err(error) = result
            && !crate::launch::is_cancelled(&error)
        {
            eprintln!("Dock launch: {error}");
            if admin {
                // Some apps, such as most Store apps, cannot run elevated.
                crate::tray::notify(
                    self.owner,
                    &format!("Could not run {} as administrator", app.name),
                    &error.message(),
                    true,
                );
            }
        }
    }

    /// The app at a dock slot: its ID, its file and its windows.
    fn app_at(&self, index: usize) -> Option<(String, Option<String>, Vec<isize>)> {
        let item = index.checked_sub(1).and_then(|i| self.items.get(i + self.scroll))?;
        let id = item.app.id.clone();
        // A pin made before the index knew the app may lack the file the index has.
        let path = item.app.path.clone().or_else(|| {
            self.apps
                .iter()
                .find(|known| known.id.eq_ignore_ascii_case(&id))
                .and_then(|known| known.path.clone())
        });
        Some((id, path, item.windows.iter().map(|window| window.hwnd).collect()))
    }

    /// Starts reading the hovered app's Jump List, so its menu opens ready.
    fn prefetch_jump_list(&self) {
        if let Some((id, path, windows)) = self.hover.and_then(|index| self.app_at(index)) {
            jump_lists::prefetch(&id, path.as_deref(), &windows);
        }
    }

    fn menu(&mut self, index: usize) {
        let Some(item) = index.checked_sub(1).and_then(|i| self.items.get(i + self.scroll)) else {
            return;
        };
        let Some((id, path, windows)) = self.app_at(index) else {
            return;
        };
        let app = item.app.clone();
        let pinned = self.pinned.iter().any(|pin| pin.id.eq_ignore_ascii_case(&id));
        // A Store app starts by its ID and runs from its package: it has no file
        // to show, and its windows belong to hosts such as ApplicationFrameHost.
        let packaged = path.is_none() && app.id.contains('!');
        // The file to reveal: the app's own path, else the running program's.
        let location = path
            .clone()
            .filter(|path| path.contains('\\'))
            .or_else(|| item.windows.iter().map(|w| w.exe.clone()).find(|exe| !exe.is_empty()))
            .filter(|path| {
                let lower = path.to_ascii_lowercase();
                !packaged && !lower.contains(r"\windowsapps\") && !lower.ends_with(r"\applicationframehost.exe")
            });
        let hosted = item
            .windows
            .iter()
            .any(|w| w.exe.to_ascii_lowercase().ends_with(r"\applicationframehost.exe"));
        let owners: Vec<_> = item
            .windows
            .iter()
            .map(|w| open_windows::owner(w.hwnd, window_aumid(HWND(w.hwnd as _)).as_deref()))
            .collect();
        let shell = owners.contains(&open_windows::Owner::Shell);
        let mut pids: Vec<u32> = owners
            .iter()
            .filter_map(|owner| match owner {
                open_windows::Owner::Process(pid) => Some(*pid),
                _ => None,
            })
            .collect();
        pids.sort_unstable();
        pids.dedup();
        // Held from now until the menu closes, so End task ends these
        // processes and no other that might take one of their IDs. Only
        // offered where Windows offers it on the taskbar.
        let ending = (!shell && end_task_enabled())
            .then(|| open_windows::EndTask::open(&pids))
            .filter(open_windows::EndTask::has_target);
        // Only file-backed programs can run elevated; packaged apps and shell hosts cannot.
        let elevatable = !shell
            && !hosted
            && !packaged
            && location.as_deref().is_some_and(|path| {
                let lower = path.to_ascii_lowercase();
                lower.ends_with(".exe") || lower.ends_with(".lnk")
            });
        let jumps = jump_lists::read(&id, path.as_deref(), &windows);
        'menus: {
            unsafe {
                let (menu, submenu) = match (CreatePopupMenu(), CreatePopupMenu()) {
                    (Ok(menu), Ok(submenu)) => (menu, submenu),
                    (menu, submenu) => {
                        for menu in [menu, submenu].into_iter().flatten() {
                            let _ = DestroyMenu(menu);
                        }
                        break 'menus;
                    }
                };
                let mut jump_items = Vec::new();
                for section in jumps {
                    let header = HSTRING::from(section.title);
                    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, &header);
                    for item in section.items {
                        let title = HSTRING::from(item.title.replace('&', "&&").replace(['\t', '\r', '\n'], " "));
                        let command = jump_lists::MENU_FIRST + jump_items.len();
                        if AppendMenuW(menu, MF_STRING, command, &title).is_ok() {
                            jump_items.push(item);
                        }
                    }
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                }
                // The taskbar's order, with items that do not apply left out:
                // the app's name, pinning, then closing.
                let name = HSTRING::from(app.name.as_str());
                // The parent owns the submenu only after a successful attach.
                if AppendMenuW(menu, MF_POPUP, submenu.0 as usize, &name).is_err() {
                    let _ = DestroyMenu(submenu);
                    let _ = DestroyMenu(menu);
                    break 'menus;
                }
                let pin = if pinned {
                    w!("Unpin from dock")
                } else {
                    w!("Pin to dock")
                };
                let _ = AppendMenuW(menu, MF_STRING, MENU_PIN, pin);
                if !windows.is_empty() {
                    let close = if windows.len() > 1 {
                        w!("Close all windows")
                    } else {
                        w!("Close window")
                    };
                    let _ = AppendMenuW(menu, MF_STRING, MENU_CLOSE, close);
                }
                if ending.is_some() {
                    let _ = AppendMenuW(menu, MF_STRING, MENU_END_TASK, w!("End task"));
                }
                let _ = AppendMenuW(submenu, MF_STRING, MENU_NEW_WINDOW, w!("New window"));
                if elevatable {
                    let _ = AppendMenuW(submenu, MF_STRING, MENU_ADMIN, w!("Run as administrator"));
                }
                if location.is_some() {
                    let _ = AppendMenuW(submenu, MF_STRING, MENU_LOCATION, w!("Open file location"));
                }
                if location.is_some() {
                    let _ = AppendMenuW(submenu, MF_STRING, MENU_PROPERTIES, w!("Properties"));
                }
                let mut point = POINT::default();
                let _ = GetCursorPos(&mut point);
                let _ = SetForegroundWindow(self.hwnd);
                let command = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_NONOTIFY,
                    point.x,
                    point.y,
                    None,
                    self.hwnd,
                    None,
                )
                .0 as usize;
                // DestroyMenu recursively destroys the attached submenu.
                let _ = DestroyMenu(menu);
                match command {
                    MENU_PIN => self.toggle_pin(&id),
                    MENU_CLOSE => {
                        for hwnd in windows {
                            let _ = open_windows::close(hwnd);
                        }
                    }
                    MENU_NEW_WINDOW => self.launch(&app, false),
                    MENU_ADMIN => self.launch(&app, true),
                    MENU_LOCATION => {
                        if let Some(path) = &location
                            && let Err(error) = crate::launch::reveal(path)
                        {
                            eprintln!("Dock open file location: {error}");
                        }
                    }
                    MENU_PROPERTIES => {
                        if let Some(path) = &location
                            && let Err(error) = crate::launch::properties(path)
                        {
                            eprintln!("Dock properties: {error}");
                        }
                    }
                    MENU_END_TASK => {
                        if let Some(ending) = ending
                            && let Err(error) = ending.end()
                        {
                            let hint = if error.denied {
                                " It may be running as another user or protected by Windows. Task Manager can end it."
                            } else {
                                ""
                            };
                            crate::tray::notify(
                                self.owner,
                                &format!("Could not end {}", app.name),
                                &format!("{}{hint}", error.message),
                                true,
                            );
                        }
                    }
                    command => {
                        if let Some(item) = command
                            .checked_sub(jump_lists::MENU_FIRST)
                            .and_then(|i| jump_items.get(i))
                            && let Err(error) = item.invoke()
                        {
                            eprintln!("Dock Jump List: {error}");
                        }
                    }
                }
            }
        }
        self.refresh();
    }

    fn dragging(&self) -> bool {
        self.press.as_ref().is_some_and(|press| press.dragging)
    }

    /// Captures an app or section press until it becomes a click or a drag.
    fn press(&mut self, lp: LPARAM) {
        self.finish_press(false);
        self.suppress_click = false;
        let scale = logic::layout_scale(self.app_icon_size);
        let x = (lp.0 as i16) as f32 * 96.0 / self.dpi as f32;
        let target = if let Some(item) = self.hit(lp).and_then(|slot| self.item_at(slot)) {
            PressTarget::App(item.app.id.clone())
        } else {
            // Notification icons keep their app-owned click behavior.
            if matches!(self.hit_part(lp), Some(logic::Part::Tray(_))) {
                return;
            }
            PressTarget::Section(self.row_layout().section_at(x, scale))
        };
        self.press = Some(Press {
            target,
            sections: self.order,
            x: (lp.0 as i16).into(),
            dragging: false,
            order: self.items.iter().map(|item| item.app.id.clone()).collect(),
            pins: self.pinned.clone(),
        });
        unsafe {
            SetCapture(self.hwnd);
        }
    }

    /// Follows the pointer after the system's horizontal drag distance.
    fn drag_to(&mut self, lp: LPARAM, released: bool) -> bool {
        if self.press.is_none() {
            return false;
        }
        if !released && unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } >= 0 {
            return self.finish_press(true);
        }
        let press = self.press.as_mut().unwrap();
        let x = i32::from(lp.0 as i16);
        if !press.dragging {
            let threshold = unsafe { GetSystemMetricsForDpi(SM_CXDRAG, self.dpi) }.max(1);
            if (x - press.x).abs() < threshold {
                return false;
            }
            press.dragging = true;
            self.hide_tip();
            self.hide_windows();
            self.close_flyout();
            self.hover = None;
            self.hover_part = None;
            self.animate_hover();
            self.revealed = true;
            self.start_slide(0.0);
        }
        let scale = logic::layout_scale(self.app_icon_size);
        let x = x as f32 * 96.0 / self.dpi as f32;
        let id = match &self.press.as_ref().unwrap().target {
            PressTarget::Section(section) => {
                let section = *section;
                let target = self.row_layout().section_target(section, x, scale);
                if target != section {
                    logic::move_section(&mut self.order, section, target);
                    self.place();
                    unsafe {
                        let _ = InvalidateRect(Some(self.hwnd), None, false);
                    }
                }
                return true;
            }
            PressTarget::App(id) => id,
        };
        let Some(from) = self.items.iter().position(|item| item.app.id.eq_ignore_ascii_case(id)) else {
            self.finish_press(false);
            return true;
        };
        if let Some(target) = self.row_layout().drop_target(x, scale, self.scroll, self.pinned.len()) {
            let old_pins = self.pinned.len();
            if target.index == from && target.pinned == (from < old_pins) {
                return true;
            }
            let pins = logic::move_app(&mut self.items, from, target, old_pins);
            if from < old_pins || pins != old_pins {
                self.pinned = self.items[..pins].iter().map(|item| item.app.clone()).collect();
            }
            if pins != old_pins {
                self.place();
            } else {
                self.publish_rects();
            }
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
        true
    }

    /// Drops or cancels a drag. A drag never also activates the app.
    fn finish_press(&mut self, drop: bool) -> bool {
        let Some(press) = self.press.take() else {
            return false;
        };
        unsafe {
            if GetCapture() == self.hwnd {
                let _ = ReleaseCapture();
            }
        }
        self.suppress_click |= press.dragging || !drop;
        if !press.dragging {
            return false;
        }
        if matches!(press.target, PressTarget::Section(_)) {
            if !drop {
                self.order = press.sections;
            } else if self.order != press.sections {
                let names: Vec<_> = self.order.iter().map(|section| section.name()).collect();
                match crate::config::Config::save_order(&names) {
                    Ok(()) => unsafe {
                        let _ = PostMessageW(Some(self.owner), crate::messages::WM_APP_RELOAD, WPARAM(0), LPARAM(0));
                    },
                    Err(error) => {
                        eprintln!("Dock order: {error}");
                        self.order = press.sections;
                    }
                }
            }
            self.update_pins();
        } else if !drop {
            self.pinned = press.pins;
            self.items
                .sort_by_key(|item| press.order.iter().position(|id| id.eq_ignore_ascii_case(&item.app.id)));
            self.update_pins();
        } else if self
            .pinned
            .iter()
            .map(|app| &app.id)
            .ne(press.pins.iter().map(|app| &app.id))
        {
            self.save_pins();
        } else {
            self.update_pins();
        }
        self.hover_part = None;
        self.animate_hover();
        self.refresh();
        true
    }

    fn toggle_pin(&mut self, id: &str) {
        if let Some(at) = self.pinned.iter().position(|pin| pin.id.eq_ignore_ascii_case(id)) {
            self.pinned.remove(at);
        } else if let Some(item) = self.items.iter().find(|item| item.app.id.eq_ignore_ascii_case(id)) {
            self.pinned.push(AppEntry {
                aliases: Vec::new(),
                id: item.app.id.clone(),
                name: item.app.name.clone(),
                path: item.app.path.clone(),
            });
        }
        self.save_pins();
    }

    fn save_pins(&mut self) {
        let pins: Vec<_> = self
            .pinned
            .iter()
            .map(|app| pins::Pin {
                id: app.id.clone(),
                name: app.name.clone(),
                path: app.path.clone(),
            })
            .collect();
        if let Err(error) = pins::save(&crate::util::data_dir().join("dock-pins.toml"), &pins) {
            eprintln!("Dock pins: {error}");
            self.pinned = load_pins();
        }
        self.update_pins();
    }
}

fn load_pins() -> Vec<AppEntry> {
    pins::load(&crate::util::data_dir().join("dock-pins.toml"))
        .into_iter()
        .map(|pin| AppEntry {
            aliases: Vec::new(),
            id: pin.id,
            name: pin.name,
            path: pin.path,
        })
        .collect()
}

/// Whether Windows offers End task on the taskbar: Settings > System > For
/// developers > End task. Off for most people, so the dock hides it too.
fn end_task_enabled() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let mut value = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced\\TaskbarDeveloperSettings"),
            w!("TaskbarEndTask"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
        .is_ok()
            && value == 1
    }
}

fn window_aumid(hwnd: HWND) -> Option<String> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
        // PKEY_AppUserModel_ID, defined here to avoid an unrelated storage feature.
        let key = PROPERTYKEY {
            fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
            pid: 5,
        };
        let mut value = store.GetValue(&key).ok()?;
        let result = PropVariantToStringAlloc(&value);
        let _ = PropVariantClear(&mut value);
        let ptr = result.ok()?;
        let text = ptr.to_string().ok();
        CoTaskMemFree(Some(ptr.0.cast()));
        text.filter(|text| !text.is_empty())
    }
}

/// The screen under the pointer, if the pointer is within its width.
fn cursor_monitor(cursor: POINT) -> Option<RECT> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let found = unsafe { GetMonitorInfoW(MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST), &mut info) }.as_bool();
    let monitor = info.rcMonitor;
    (found && (monitor.left..monitor.right).contains(&cursor.x)).then_some(monitor)
}

fn fullscreen_foreground(dock: HWND) -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid()
            || hwnd == dock
            || IsIconic(hwnd).as_bool()
            || MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) != MonitorFromWindow(dock, MONITOR_DEFAULTTONEAREST)
        {
            return false;
        }
        let mut class = [0u16; 128];
        let len = GetClassNameW(hwnd, &mut class);
        if matches!(
            String::from_utf16_lossy(&class[..len as usize]).as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return false;
        }
        let mut rect = RECT::default();
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetWindowRect(hwnd, &mut rect).is_err()
            || !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool()
        {
            return false;
        }
        let covers = rect.left <= info.rcMonitor.left
            && rect.top <= info.rcMonitor.top
            && rect.right >= info.rcMonitor.right
            && rect.bottom >= info.rcMonitor.bottom;
        // Maximized windows have resize borders outside the monitor; don't
        // treat those as fullscreen. Some apps go fullscreen from maximized,
        // though: that shows when the window fills the monitor exactly while
        // the work area, and so a plain maximize, stops short of it.
        covers && (!IsZoomed(hwnd).as_bool() || (rect == info.rcMonitor && info.rcWork != info.rcMonitor))
    }
}

unsafe extern "system" fn window_event(
    _: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _: u32,
    _: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND {
        if !matches!(
            event,
            EVENT_OBJECT_DESTROY
                | EVENT_OBJECT_SHOW
                | EVENT_OBJECT_HIDE
                | EVENT_OBJECT_NAMECHANGE
                | EVENT_OBJECT_CLOAKED
                | EVENT_OBJECT_UNCLOAKED
        ) || object != OBJID_WINDOW.0
            || child != 0
            || hwnd.is_invalid()
        {
            return;
        }
        if event != EVENT_OBJECT_DESTROY && unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd {
            return;
        }
    }
    EVENT_TARGET.with(|target| unsafe {
        let hwnd = target.get();
        if !hwnd.is_invalid() {
            let _ = PostMessageW(Some(hwnd), WM_REFRESH, WPARAM(REFRESH_WINDOWS), LPARAM(0));
        }
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let Some(filtered) = crate::theme::filter_broadcast(msg, wp, lp) else {
            return DefWindowProcW(hwnd, msg, wp, lp);
        };
        if filtered != msg {
            return wndproc(hwnd, filtered, wp, LPARAM(0));
        }
        if msg == WM_NCACTIVATE {
            return DefWindowProcW(hwnd, msg, WPARAM(1), LPARAM(-1));
        }
        if msg == WM_MOUSEACTIVATE {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        if msg == WM_NCCALCSIZE && wp.0 != 0 {
            return LRESULT(0);
        }
        if msg == WM_NCHITTEST {
            return LRESULT(HTCLIENT as isize);
        }
        if msg == WM_ERASEBKGND {
            return LRESULT(1);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const StateCell;
        if let Some(cell) = pointer.as_ref() {
            if cell.try_borrow_mut().is_err() {
                cell.defer_message(hwnd, msg, wp, lp);
                // DefWindowProc would destroy the window instead of our usual hide.
                if msg == WM_CLOSE {
                    return LRESULT(0);
                }
                // Validate paint / free raw input through DefWindowProc; discard stale input.
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            if msg == WM_DOCK_DRAIN {
                cell.drain();
                return LRESULT(0);
            }
        }
        let result = wndproc_inner(hwnd, msg, wp, lp);
        if let Some(cell) = pointer.as_ref() {
            cell.drain();
        }
        result
    }
}

unsafe fn wndproc_inner(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let create = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const StateCell;
        if !pointer.is_null()
            && let Ok(mut s) = (*pointer).try_borrow_mut()
        {
            match msg {
                WM_POWERBROADCAST | WM_THEMECHANGED => {
                    let dark = s.dark;
                    let (backdrop, tint) = (s.backdrop, s.tint);
                    s.apply_material(dark, backdrop, tint);
                    // Battery level and charging arrive here too.
                    if msg == WM_POWERBROADCAST {
                        s.update_status();
                    }
                }
                WM_DOCK_STATUS => {
                    s.update_status();
                    return LRESULT(0);
                }
                WM_TIMER if wp.0 == CLOCK_TIMER => {
                    s.update_status();
                    arm_clock(hwnd);
                    return LRESULT(0);
                }
                WM_ARTWORK => {
                    s.artwork.drain();
                    s.place();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return LRESULT(0);
                }
                WM_REFRESH => {
                    // Anything but another app's windows changing lays the dock out again.
                    if wp.0 != REFRESH_WINDOWS {
                        s.relayout = true;
                    }
                    if !s.scheduled {
                        s.scheduled = SetTimer(Some(hwnd), REFRESH_TIMER, 100, None) != 0;
                    }
                    return LRESULT(0);
                }
                WM_TIMER if wp.0 == POLL_TIMER => {
                    s.poll();
                    return LRESULT(0);
                }
                WM_DOCK_WAKE => {
                    s.wake();
                    return LRESULT(0);
                }
                WM_INPUT => {
                    s.wake();
                    // Windows frees the input data in DefWindowProc.
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                WM_TIMER if wp.0 == SLIDE_TIMER => {
                    s.step_slide();
                    return LRESULT(0);
                }
                WM_TIMER if wp.0 == HOVER_TIMER => {
                    s.step_hover();
                    return LRESULT(0);
                }
                WM_TIMER if wp.0 == TIP_TIMER => {
                    s.show_tip();
                    return LRESULT(0);
                }
                WM_TIMER if wp.0 == DESKTOP_TIMER => {
                    let _ = KillTimer(Some(hwnd), DESKTOP_TIMER);
                    if s.desktop_pill.take().is_some() {
                        let _ = ShowWindow(s.tip, SW_HIDE);
                    }
                    return LRESULT(0);
                }
                WM_TIMER if wp.0 == WINDOWS_TIMER => {
                    s.prefetch_jump_list();
                    s.show_windows();
                    return LRESULT(0);
                }
                WM_TIMER if wp.0 == WINDOWS_HIDE_TIMER => {
                    let _ = KillTimer(Some(hwnd), WINDOWS_HIDE_TIMER);
                    let mut cursor = POINT::default();
                    let _ = GetCursorPos(&mut cursor);
                    let over = |window: HWND| {
                        let mut rect = RECT::default();
                        GetWindowRect(window, &mut rect).is_ok()
                            && (rect.left..rect.right).contains(&cursor.x)
                            && (rect.top..rect.bottom).contains(&cursor.y)
                    };
                    if !over(s.hwnd) && !over(s.windows) {
                        s.hide_windows();
                    }
                    return LRESULT(0);
                }
                WM_RADIOS => {
                    if let Some(radios) = s.radio_worker.take()
                        && s.visible
                        && s.revealed
                        && !s.suspended
                        && !s.fullscreen
                        && radios != s.radios
                    {
                        s.radios = radios;
                        s.refresh_status_tip();
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    return LRESULT(0);
                }
                WM_TRAY_CHANGED => {
                    s.sync_tray();
                    return LRESULT(0);
                }
                WM_TIMER if wp.0 == REFRESH_TIMER => {
                    let _ = KillTimer(Some(hwnd), REFRESH_TIMER);
                    s.scheduled = false;
                    let relayout = std::mem::take(&mut s.relayout);
                    s.layout_unchanged_ok = !relayout;
                    s.refresh();
                    s.layout_unchanged_ok = false;
                    return LRESULT(0);
                }
                WM_DPICHANGED | WM_DISPLAYCHANGE => {
                    s.finish_press(false);
                    let _ = PostMessageW(Some(hwnd), WM_REFRESH, WPARAM(0), LPARAM(0));
                    return LRESULT(0);
                }
                WM_CAPTURECHANGED => {
                    s.finish_press(false);
                    return LRESULT(0);
                }
                WM_LBUTTONDOWN => {
                    s.press(lp);
                    return LRESULT(0);
                }
                WM_MOUSEMOVE => {
                    if s.drag_to(lp, false) {
                        return LRESULT(0);
                    }
                    let hover = s.hit(lp);
                    let hover_part = s.hit_part(lp);
                    if s.hover != hover || s.hover_part != hover_part {
                        s.hover = hover;
                        s.animate_hover();
                        s.hover_part = hover_part;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    s.hover_tip(match hover_part {
                        Some(part @ (logic::Part::Status | logic::Part::Icon(_))) => Some(tray::Tip::Status(part)),
                        Some(logic::Part::Chevron) => Some(tray::Tip::Chevron),
                        Some(logic::Part::Tray(i)) => Some(tray::Tip::Inline(i)),
                        _ => None,
                    });
                    s.hover_windows(if hover_part.is_none() { hover } else { None });
                    let _ = TrackMouseEvent(&mut TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    });
                    return LRESULT(0);
                }
                WM_MOUSELEAVE => {
                    s.hover = None;
                    s.animate_hover();
                    s.hover_part = None;
                    s.hover_tip(None);
                    s.leave_windows();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return LRESULT(0);
                }
                WM_LBUTTONUP | WM_MBUTTONUP => {
                    if msg == WM_LBUTTONUP {
                        if s.dragging() {
                            s.drag_to(lp, true);
                        }
                        s.finish_press(true);
                        if std::mem::take(&mut s.suppress_click) {
                            return LRESULT(0);
                        }
                    }
                    // The dock never takes the focus, so the flyout can't see this click.
                    if s.hit_part(lp) != Some(logic::Part::Chevron) {
                        s.close_flyout();
                    }
                    s.hide_windows();
                    if let Some(index) = s.hit(lp) {
                        s.click(index, msg == WM_MBUTTONUP);
                    } else if let Some(part) = s.hit_part(lp)
                        && msg == WM_LBUTTONUP
                    {
                        s.click_part(part);
                    }
                    return LRESULT(0);
                }
                WM_RBUTTONUP => {
                    s.finish_press(false);
                    s.close_flyout();
                    s.hide_windows();
                    if let Some(index) = s.hit(lp) {
                        s.menu(index);
                    } else {
                        match s.hit_part(lp) {
                            Some(part @ (logic::Part::Status | logic::Part::Icon(_))) => {
                                if let Err(error) = crate::launch::open(s.status_items(part)[0].settings()) {
                                    eprintln!("Dock sound settings: {error}");
                                }
                            }
                            Some(logic::Part::Chevron) => s.chevron_menu(),
                            Some(logic::Part::Tray(position)) => s.click_inline(position, Action::Menu),
                            _ => {}
                        }
                    }
                    return LRESULT(0);
                }
                WM_MOUSEWHEEL => {
                    s.hide_windows();
                    let delta = (wp.0 >> 16) as i16;
                    // Wheel messages carry screen coordinates.
                    let mut point = POINT {
                        x: (lp.0 as i16).into(),
                        y: ((lp.0 >> 16) as i16).into(),
                    };
                    let _ = ScreenToClient(hwnd, &mut point);
                    let client = LPARAM(((point.y as u16 as isize) << 16) | point.x as u16 as isize);
                    if logic::wheel_changes_volume(s.hit_part(client), s.status_placement) {
                        s.wheel_volume(delta.into());
                        return LRESULT(0);
                    }
                    // Shift scrolls the apps when more run than fit.
                    if GetKeyState(VK_SHIFT.0 as i32) >= 0 {
                        s.wheel_desktops(delta.into());
                        return LRESULT(0);
                    }
                    s.scroll = if delta < 0 {
                        (s.scroll + 1).min(s.items.len().saturating_sub(s.capacity))
                    } else {
                        s.scroll.saturating_sub(1)
                    };
                    if s.dragging() {
                        s.drag_to(client, false);
                    } else {
                        s.hover = s.hit(client);
                        s.animate_hover();
                    }
                    s.publish_rects();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return LRESULT(0);
                }
                WM_MOUSEHWHEEL => {
                    s.hide_windows();
                    // Positive is a swipe toward the right.
                    s.wheel_desktops(-i32::from((wp.0 >> 16) as i16));
                    return LRESULT(0);
                }
                WM_PAINT => {
                    let mut paint = PAINTSTRUCT::default();
                    let _ = BeginPaint(hwnd, &mut paint);
                    if let Err(error) = s.paint(None) {
                        eprintln!("Dock paint: {error}");
                    }
                    let _ = EndPaint(hwnd, &paint);
                    return LRESULT(0);
                }
                WM_CLOSE => {
                    s.finish_press(false);
                    s.hide_windows();
                    s.stop_slide();
                    s.visible = false;
                    s.radio_worker.hide();
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    return LRESULT(0);
                }
                WM_NCDESTROY => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

/// The flyout and the tooltip, which share the dock's state.
unsafe extern "system" fn popup_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const StateCell;
        if let Some(cell) = pointer.as_ref() {
            if cell.try_borrow_mut().is_err() {
                cell.defer_message(hwnd, msg, wp, lp);
                // DefWindowProc would destroy the window instead of our usual hide.
                if msg == WM_CLOSE {
                    return LRESULT(0);
                }
                // Validate paint / free raw input through DefWindowProc; discard stale input.
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            if msg == WM_DOCK_DRAIN {
                cell.drain();
                return LRESULT(0);
            }
        }
        let result = popup_wndproc_inner(hwnd, msg, wp, lp);
        if let Some(cell) = pointer.as_ref() {
            cell.drain();
        }
        result
    }
}

unsafe fn popup_wndproc_inner(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let create = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        if msg == WM_NCCALCSIZE && wp.0 != 0 {
            return LRESULT(0);
        }
        if msg == WM_ERASEBKGND {
            return LRESULT(1);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const StateCell;
        let Some(cell) = pointer.as_ref() else {
            return DefWindowProcW(hwnd, msg, wp, lp);
        };
        let tip = cell.try_borrow().is_ok_and(|s| s.tip == hwnd);
        if tip {
            // The tooltip never takes the pointer or the focus.
            match msg {
                WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
                WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
                _ => {}
            }
        } else if msg == WM_NCHITTEST {
            return LRESULT(HTCLIENT as isize);
        }
        let windows = cell.try_borrow().is_ok_and(|s| s.windows == hwnd);
        if windows {
            match msg {
                // Clicks land here, but the list never takes the focus.
                WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
                WM_MOUSEMOVE => {
                    let Ok(mut s) = cell.try_borrow_mut() else {
                        return DefWindowProcW(hwnd, msg, wp, lp);
                    };
                    s.away = 0;
                    let _ = KillTimer(Some(s.hwnd), WINDOWS_HIDE_TIMER);
                    let hover = s.windows_hit(lp);
                    if s.windows_hover != hover {
                        s.windows_hover = hover;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    let _ = TrackMouseEvent(&mut TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    });
                    return LRESULT(0);
                }
                WM_MOUSELEAVE => {
                    let Ok(mut s) = cell.try_borrow_mut() else {
                        return DefWindowProcW(hwnd, msg, wp, lp);
                    };
                    s.leave_windows();
                    return LRESULT(0);
                }
                WM_LBUTTONUP | WM_MBUTTONUP => {
                    let Ok(mut s) = cell.try_borrow_mut() else {
                        return DefWindowProcW(hwnd, msg, wp, lp);
                    };
                    if let Some(row) = s.windows_hit(lp)
                        && let Some(&target) = s.windows_targets.get(row)
                    {
                        s.hide_windows();
                        let result = if msg == WM_MBUTTONUP {
                            open_windows::close(target)
                        } else {
                            open_windows::focus(target)
                        };
                        if let Err(error) = result {
                            eprintln!("Dock window: {error}");
                        }
                    }
                    return LRESULT(0);
                }
                WM_PAINT => {
                    let Ok(mut s) = cell.try_borrow_mut() else {
                        return DefWindowProcW(hwnd, msg, wp, lp);
                    };
                    let mut paint = PAINTSTRUCT::default();
                    let _ = BeginPaint(hwnd, &mut paint);
                    let State {
                        windows_renderer,
                        palette,
                        dpi,
                        windows_rows,
                        windows_hover,
                        ..
                    } = &mut *s;
                    if let Err(error) = windows_renderer.draw_windows(hwnd, *dpi, palette, windows_rows, *windows_hover)
                    {
                        eprintln!("Dock window list: {error}");
                    }
                    let _ = EndPaint(hwnd, &paint);
                    return LRESULT(0);
                }
                _ => return DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
        let Ok(mut s) = cell.try_borrow_mut() else {
            return DefWindowProcW(hwnd, msg, wp, lp);
        };
        let scale = s.dpi as f32 / 96.0;
        let cell_at = |s: &State, lp: LPARAM| {
            logic::flyout_hit(
                (lp.0 as i16) as f32 / scale,
                ((lp.0 >> 16) as i16) as f32 / scale,
                s.tray.len(),
            )
        };
        match msg {
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let _ = BeginPaint(hwnd, &mut paint);
                let State {
                    tray,
                    palette,
                    dpi,
                    flyout_hover,
                    flyout_renderer,
                    tip_renderer,
                    tip_text,
                    desktop_pill,
                    ..
                } = &mut *s;
                let result = if let Some(pill) = desktop_pill.as_ref().filter(|_| tip) {
                    tip_renderer.draw_desktops(
                        hwnd,
                        *dpi,
                        palette,
                        (pill.current, pill.names.len()),
                        &pill.names[pill.current],
                    )
                } else if tip {
                    tip_renderer.draw_tip(hwnd, *dpi, palette, tip_text)
                } else {
                    flyout_renderer.draw_flyout(
                        hwnd,
                        &render::FlyoutFrame {
                            dpi: *dpi,
                            palette,
                            tray,
                            hover: *flyout_hover,
                        },
                    )
                };
                if let Err(error) = result {
                    eprintln!("Dock popup paint: {error}");
                }
                let _ = EndPaint(hwnd, &paint);
                return LRESULT(0);
            }
            _ if tip => {}
            WM_MOUSEMOVE => {
                let hover = cell_at(&s, lp);
                if s.flyout_hover != hover {
                    s.flyout_hover = hover;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                s.hover_tip(hover.map(tray::Tip::Flyout));
                let _ = TrackMouseEvent(&mut TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                });
                return LRESULT(0);
            }
            WM_MOUSELEAVE => {
                s.flyout_hover = None;
                s.hover_tip(None);
                let _ = InvalidateRect(Some(hwnd), None, false);
                return LRESULT(0);
            }
            // Like the taskbar's overflow, the flyout closes once an icon is used.
            WM_LBUTTONUP | WM_RBUTTONUP => {
                if let Some(index) = cell_at(&s, lp) {
                    let rect = s.cell_rect(index);
                    let center = POINT {
                        x: (rect.left + rect.right) / 2,
                        y: (rect.top + rect.bottom) / 2,
                    };
                    s.close_flyout();
                    let action = if msg == WM_LBUTTONUP {
                        Action::Click
                    } else {
                        Action::Menu
                    };
                    s.click_tray(index, action, center);
                }
                return LRESULT(0);
            }
            WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
                s.close_flyout();
                return LRESULT(0);
            }
            // Closing within activation changes can reenter; close afterwards.
            WM_ACTIVATE if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE => {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE_FLYOUT, WPARAM(0), LPARAM(0));
            }
            WM_CLOSE_FLYOUT => {
                // A held demo flyout stays for its screenshot.
                if !s.held && GetForegroundWindow() != hwnd {
                    s.close_flyout();
                }
                return LRESULT(0);
            }
            WM_CLOSE => {
                s.close_flyout();
                return LRESULT(0);
            }
            _ => {}
        }
        drop(s);
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if !self.name_hook.is_invalid() {
            unsafe {
                let _ = UnhookWinEvent(self.name_hook);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_surface_routes_radio_results_to_the_dock() {
        assert!(Dock::owns_message(WM_RADIOS, WPARAM(0)));
        assert!(!Dock::owns_message(WM_SHOW_NOW, WPARAM(0)));
    }
}
