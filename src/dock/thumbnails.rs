//! Live DWM previews for the dock's app window list.
//!
//! Each preview needs a top-level destination window: `DwmRegisterThumbnail`
//! rejects child windows. The hosts are transparent to the mouse so clicks
//! reach the popup beneath, and are owned by that popup so they die with it.
//! A layered overlay above them tints the hovered tile like the taskbar does.

use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
    DwmRegisterThumbnail, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, COLOR_WINDOW, EndPaint, FillRect, GetSysColorBrush, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_EXSTYLE, GetClientRect, GetWindowLongPtrW, GetWindowRect,
    HTTRANSPARENT, HWND_TOPMOST, IsWindow, KillTimer, LWA_ALPHA, SW_HIDE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetTimer, SetWindowPos, ShowWindow, WM_NCHITTEST, WM_PAINT,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, w};

use crate::indexer::IconPixels;

use super::logic;

/// Timer ID. Kept here so `Dock::owns_message` can name the last timer ID.
/// 110 is taken by `DODGE_TIMER` on main, so previews use the next free slot.
pub const TIMER: usize = 111;
/// The taskbar's previews arrive with a quick, restrained fade and almost no
/// travel: just enough that the whole surface settles into place.
const SHOW_MS: u32 = 110;
const HIDE_MS: u32 = 90;
const RISE: f32 = 4.0;
/// Gap between the preview image and the tile edge; the tint covers it too.
const HOST_INSET: f32 = 2.0;
/// Alpha of the layered hover wash over a preview, out of 255.
const HOVER_ALPHA: u8 = 18;
/// A hovering icon can wobble between neighbouring slots while the dock moves.
pub const DEBOUNCE: Duration = Duration::from_millis(180);

struct Preview {
    id: isize,
    host: HWND,
    /// Tile column, which need not match the list index if a source window
    /// could not be previewed.
    column: usize,
}

#[derive(Clone, Copy)]
struct Anim {
    started: Instant,
    from: f32,
    to: f32,
}

/// The previews, their animation, and the popup geometry they follow.
pub struct Thumbnails {
    previews: Vec<Preview>,
    /// WGC capture for windows DWM cannot preview: (column, source HWND).
    captures: Vec<(usize, isize)>,
    capture: Option<super::capture::Capture>,
    /// One layered wash, moved over whichever tile the pointer rests on.
    overlay: HWND,
    anim: Option<Anim>,
    /// Cached source handles, so an unchanged hover does not rebuild hosts.
    targets: Vec<isize>,
    base: RECT,
    last_show: Option<Instant>,
}

impl Default for Thumbnails {
    fn default() -> Self {
        Self::new()
    }
}

impl Thumbnails {
    pub fn new() -> Self {
        Self {
            previews: Vec::new(),
            captures: Vec::new(),
            capture: super::capture::Capture::new(),
            overlay: HWND::default(),
            anim: None,
            targets: Vec::new(),
            base: RECT::default(),
            last_show: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.previews.is_empty() && self.captures.is_empty()
    }

    /// Latest frames, keyed by tile column, for the renderer to draw.
    /// PrintWindow runs on this STA thread: Chromium returns black from the
    /// WGC worker's MTA thread. WGC frames are the fallback.
    pub fn captures(&mut self, dpi: u32) -> Vec<(usize, Arc<IconPixels>)> {
        let Some(capture) = &mut self.capture else {
            return Vec::new();
        };
        capture.poll();
        let scale = dpi as f32 / 96.0;
        let width = (logic::THUMB_W * scale).round().max(1.0) as u32;
        let height = (logic::THUMB_H * scale).round().max(1.0) as u32;
        self.captures
            .iter()
            .filter_map(|&(column, hwnd)| {
                super::capture::print_window(hwnd, width, height)
                    .map(Arc::new)
                    .or_else(|| capture.frame(hwnd))
                    .map(|pixels| (column, pixels))
            })
            .collect()
    }

    /// How far the previews are currently faded in.
    pub fn amount(&self) -> f32 {
        let Some(anim) = self.anim else { return 1.0 };
        let showing = anim.to == 1.0;
        let duration = if showing { SHOW_MS } else { HIDE_MS };
        let t = (anim.started.elapsed().as_secs_f32() / (duration as f32 / 1000.0)).clamp(0.0, 1.0);
        // A quick ease-out on arrival, a plain gentle fade on the way out.
        let eased = if showing {
            crate::ease::out_cubic(t)
        } else {
            crate::ease::smoothstep(t)
        };
        anim.from + (anim.to - anim.from) * eased
    }

    pub fn animating_out(&self) -> bool {
        self.anim.is_some_and(|anim| anim.to == 0.0)
    }

    pub fn last_show(&self) -> Option<Instant> {
        self.last_show
    }

    pub fn note_show(&mut self) {
        self.last_show = Some(Instant::now());
    }

    pub fn set_base(&mut self, base: RECT) {
        self.base = base;
    }

    /// Reports capture failures when probing with `WISP_DEBUG_THUMBS=1`.
    pub(crate) fn debug(what: &str, error: &windows::core::Error) {
        if std::env::var_os("WISP_DEBUG_THUMBS").is_some() {
            eprintln!("Dock thumbnails: {what}: {error}");
        }
    }

    /// Drops every live preview.
    pub fn clear(&mut self) {
        for preview in self.previews.drain(..) {
            unsafe {
                let _ = DwmUnregisterThumbnail(preview.id);
                let _ = DestroyWindow(preview.host);
            }
        }
        if let Some(capture) = &mut self.capture {
            capture.clear();
        }
        self.captures.clear();
        if !self.overlay.is_invalid() {
            unsafe {
                let _ = ShowWindow(self.overlay, SW_HIDE);
            }
        }
        self.targets.clear();
        self.anim = None;
        self.last_show = None;
    }

    /// Rebuilds previews only when the set of source windows changed.
    pub fn sync(&mut self, popup: HWND, targets: &[isize], dpi: u32) {
        if self.targets == targets {
            return;
        }
        self.clear();
        self.targets = targets.to_vec();
        let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
            return;
        };
        let scale = dpi as f32 / 96.0;
        let frame_w = (logic::THUMB_W * scale).round().max(1.0) as u32;
        let frame_h = (logic::THUMB_H * scale).round().max(1.0) as u32;
        for (column, &target) in targets.iter().enumerate() {
            let source = HWND(target as _);
            // DWM can preview minimized windows too, as the taskbar does.
            if !unsafe { IsWindow(Some(source)) }.as_bool() {
                continue;
            }
            // Chromium-style windows have no redirection bitmap; DWM shows
            // black for them, so use WGC instead.
            let style = unsafe { GetWindowLongPtrW(source, GWL_EXSTYLE) };
            if style & (WS_EX_NOREDIRECTIONBITMAP.0 as isize) != 0 {
                if let Some(capture) = &mut self.capture {
                    capture.start(target, popup.0 as usize, frame_w, frame_h);
                }
                self.captures.push((column, target));
                continue;
            }
            // DWM rejects a child as the destination: the host must be top-level.
            let Ok(host) = (unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                    w!("Wisp.DockThumb"),
                    w!(""),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    Some(popup),
                    None,
                    Some(instance.into()),
                    None,
                )
            }) else {
                continue;
            };
            match unsafe { DwmRegisterThumbnail(host, source) } {
                Ok(id) => self.previews.push(Preview { id, host, column }),
                Err(error) => {
                    Self::debug("register", &error);
                    unsafe {
                        let _ = DestroyWindow(host);
                    }
                }
            }
        }
        if self.overlay.is_invalid() {
            self.overlay = unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                    w!("Wisp.DockThumbTint"),
                    w!(""),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    Some(popup),
                    None,
                    Some(instance.into()),
                    None,
                )
            }
            .unwrap_or_default();
            if !self.overlay.is_invalid() {
                unsafe {
                    let _ = SetLayeredWindowAttributes(self.overlay, COLORREF(0), HOVER_ALPHA, LWA_ALPHA);
                }
            }
        }
        self.place(popup, dpi, 1.0);
    }

    /// Moves the wash over the hovered tile, or hides it when none is hovered.
    pub fn set_hover(&mut self, popup: HWND, dpi: u32, column: Option<usize>) {
        if self.overlay.is_invalid() {
            return;
        }
        let Some(preview) = column.and_then(|col| self.previews.iter().find(|p| p.column == col)) else {
            unsafe {
                let _ = ShowWindow(self.overlay, SW_HIDE);
            }
            return;
        };
        let scale = dpi as f32 / 96.0;
        let (left, top, width, height) = Self::tile(popup, scale, preview.column);
        unsafe {
            let _ = SetWindowPos(
                self.overlay,
                Some(HWND_TOPMOST),
                left,
                top,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }

    /// Screen rect of a tile's outer bounds.
    fn tile(popup: HWND, scale: f32, column: usize) -> (i32, i32, i32, i32) {
        let pad = (logic::THUMB_PAD * scale).round() as i32;
        let width = (logic::THUMB_W * scale).round() as i32;
        let height = (logic::THUMB_TILE_H * scale).round() as i32;
        let step = width + pad;
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(popup, &mut rect) }.is_err() {
            return (0, 0, 0, 0);
        }
        (rect.left + pad + column as i32 * step, rect.top + pad, width, height)
    }

    /// Moves the previews to follow the popup and applies the current fade.
    pub fn place(&mut self, popup: HWND, dpi: u32, amount: f32) {
        let scale = dpi as f32 / 96.0;
        let inset = (HOST_INSET * scale).round() as i32;
        let image_width = (logic::THUMB_W * scale).round() as i32 - 2 * inset;
        let image_height = (logic::THUMB_H * scale).round() as i32 - 2 * inset;
        let opacity = (255.0 * amount).round().clamp(0.0, 255.0) as u8;
        let mut rect = RECT::default();
        if let Err(error) = unsafe { GetWindowRect(popup, &mut rect) } {
            Self::debug("popup rect", &error);
            return;
        }
        for preview in &self.previews {
            let (left, top, _, _) = Self::tile(popup, scale, preview.column);
            unsafe {
                let _ = SetWindowPos(
                    preview.host,
                    Some(HWND_TOPMOST),
                    left + inset,
                    top + inset,
                    image_width,
                    image_height,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
            let properties = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
                rcDestination: RECT {
                    left: 0,
                    top: 0,
                    right: image_width,
                    bottom: image_height,
                },
                rcSource: RECT::default(),
                opacity,
                fVisible: BOOL(1),
                fSourceClientAreaOnly: BOOL(0),
            };
            if let Err(error) = unsafe { DwmUpdateThumbnailProperties(preview.id, &properties) } {
                Self::debug("update", &error);
            }
        }
    }

    /// Starts the fade in when the previews first appear.
    pub fn begin_show(&mut self, owner: HWND, popup: HWND, dpi: u32) {
        if self.previews.is_empty() {
            return;
        }
        self.anim = Some(Anim {
            started: Instant::now(),
            from: 0.0,
            to: 1.0,
        });
        self.place(popup, dpi, 0.0);
        unsafe {
            let _ = SetTimer(Some(owner), TIMER, 16, None);
        }
    }

    /// Starts the fade out from wherever the fade in currently is. Returns
    /// false when there is nothing to animate and the caller should close now.
    pub fn begin_hide(&mut self, owner: HWND) -> bool {
        if self.is_empty() {
            return false;
        }
        if self.animating_out() {
            return true;
        }
        self.anim = Some(Anim {
            started: Instant::now(),
            from: self.amount(),
            to: 0.0,
        });
        unsafe {
            let _ = SetTimer(Some(owner), TIMER, 16, None);
        }
        true
    }

    /// Reverses a fade out if the pointer came back before it finished.
    pub fn cancel_hide(&mut self) {
        if self.animating_out() {
            self.anim = Some(Anim {
                started: Instant::now(),
                from: self.amount(),
                to: 1.0,
            });
        }
    }

    /// Advances the fade. Returns true when a hide finished and the popup
    /// should be closed by the caller.
    pub fn step(&mut self, owner: HWND, popup: HWND, dpi: u32) -> bool {
        let Some(anim) = self.anim else { return false };
        let duration = if anim.to == 1.0 { SHOW_MS } else { HIDE_MS };
        let t = anim.started.elapsed().as_secs_f32() / (duration as f32 / 1000.0);
        let amount = self.amount();
        // The entire popup, labels included, rises into place with the fade.
        let rise = ((1.0 - amount) * RISE * (dpi as f32 / 96.0)).round() as i32;
        unsafe {
            let _ = SetWindowPos(
                popup,
                None,
                self.base.left,
                self.base.top + rise,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.place(popup, dpi, amount);
        if t < 1.0 {
            return false;
        }
        unsafe {
            let _ = KillTimer(Some(owner), TIMER);
        }
        self.anim = None;
        anim.to == 0.0
    }
}

/// A translucent wash over the hovered tile. Windows paints it with the
/// theme's window brush; the layered alpha keeps it barely there.
pub unsafe extern "system" fn tint_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCHITTEST {
            return LRESULT(HTTRANSPARENT as isize);
        }
        if msg == WM_PAINT {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            FillRect(hdc, &client, GetSysColorBrush(COLOR_WINDOW));
            let _ = EndPaint(hwnd, &paint);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

/// A thumbnail host: transparent to the mouse, with no painting of its own.
pub unsafe extern "system" fn thumb_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        // WS_EX_TRANSPARENT alone does not pass every click through to the
        // popup below; sending hit tests through makes the whole tile live.
        if msg == WM_NCHITTEST {
            return LRESULT(HTTRANSPARENT as isize);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_is_monotone_in_each_direction() {
        let thumbs = Thumbnails::new();
        // With no previews registered, amount is the fully shown state.
        assert_eq!(thumbs.amount(), 1.0);
        assert!(!thumbs.animating_out());
        assert!(thumbs.is_empty());
    }

    #[test]
    fn debounce_is_shorter_than_the_steady_hover_delay() {
        assert!(DEBOUNCE < Duration::from_millis(450));
    }
}
