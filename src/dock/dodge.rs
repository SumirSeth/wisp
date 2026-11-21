//! Window overlap checks run after window events, never from the pointer poll.
use crate::messages::WM_DOCK_DODGE;
use std::cell::Cell;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Dwm::*,
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        UI::{
            Accessibility::*,
            Shell::{IVirtualDesktopManager, VirtualDesktopManager},
            WindowsAndMessaging::*,
        },
    },
    core::BOOL,
};
thread_local! {
    static TARGET: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
    static PENDING: Cell<bool> = const { Cell::new(false) };
}
pub fn ready() {
    PENDING.set(false);
}
pub struct Watch {
    hooks: Vec<HWINEVENTHOOK>,
    desktop: Option<IVirtualDesktopManager>,
}
impl Watch {
    pub fn new(target: HWND) -> Self {
        TARGET.set(target);
        ready();
        let mut hooks = Vec::new();
        for (first, last) in [
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        ] {
            let hook = unsafe {
                SetWinEventHook(
                    first,
                    last,
                    None,
                    Some(changed),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            if !hook.is_invalid() {
                hooks.push(hook);
            }
        }
        let desktop = unsafe { CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_INPROC_SERVER) }.ok();
        Self { hooks, desktop }
    }
    pub fn overlaps(&self, rect: RECT) -> bool {
        let mut check = Check {
            rect,
            desktop: self.desktop.as_ref(),
            found: false,
        };
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&raw mut check as isize));
        }
        check.found
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        TARGET.set(HWND::default());
        ready();
        for hook in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
    }
}
unsafe extern "system" fn changed(_: HWINEVENTHOOK, event: u32, hwnd: HWND, object: i32, child: i32, _: u32, _: u32) {
    if event >= EVENT_OBJECT_CREATE
        && (object != OBJID_WINDOW.0
            || child != 0
            || event != EVENT_OBJECT_DESTROY && unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd)
    {
        return;
    }
    let target = TARGET.get();
    if !target.is_invalid() && !PENDING.replace(true) {
        unsafe {
            let _ = PostMessageW(Some(target), WM_DOCK_DODGE, WPARAM(0), LPARAM(0));
        }
    }
}
struct Check<'a> {
    rect: RECT,
    desktop: Option<&'a IVirtualDesktopManager>,
    found: bool,
}
unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    unsafe {
        let check = &mut *(data.0 as *mut Check<'_>);
        if !IsWindowVisible(hwnd).as_bool()
            || IsIconic(hwnd).as_bool()
            || GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0
        {
            return true.into();
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == std::process::id() {
            return true.into();
        }
        let mut class = [0u16; 128];
        let len = GetClassNameW(hwnd, &mut class);
        if matches!(
            String::from_utf16_lossy(&class[..len as usize]).as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return true.into();
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut cloaked).cast(), 4);
        if cloaked != 0
            || check.desktop.is_some_and(|desktop| {
                desktop
                    .IsWindowOnCurrentVirtualDesktop(hwnd)
                    .is_ok_and(|current| !current.as_bool())
            })
        {
            return true.into();
        }
        let mut rect = RECT::default();
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut rect).cast(),
            size_of::<RECT>() as u32,
        )
        .is_err()
            && GetWindowRect(hwnd, &mut rect).is_err()
        {
            return true.into();
        }
        check.found = intersects(check.rect, rect);
        (!check.found).into()
    }
}
fn intersects(a: RECT, b: RECT) -> bool {
    a.left < a.right
        && a.top < a.bottom
        && b.left < b.right
        && b.top < b.bottom
        && a.left < b.right
        && b.left < a.right
        && a.top < b.bottom
        && b.top < a.bottom
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlap_requires_area_and_works_on_negative_monitor_coordinates() {
        let dock = RECT {
            left: -900,
            top: 900,
            right: -100,
            bottom: 950,
        };
        assert!(intersects(
            dock,
            RECT {
                left: -400,
                top: 100,
                right: 0,
                bottom: 920
            }
        ));
        assert!(!intersects(
            dock,
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080
            }
        ));
        assert!(!intersects(dock, RECT { bottom: 900, ..dock }));
        assert!(!intersects(
            dock,
            RECT {
                top: 950,
                bottom: 1080,
                ..dock
            }
        ));
    }
}
