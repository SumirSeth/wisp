//! Opt-in Windows-key taps. Hooks run on the launcher's message-loop thread.
//!
//! Elevated foreground apps can block the hook or SendInput through UIPI;
//! Start then opens as usual. Never swallow a physical key, including Win-down.

use std::cell::Cell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::Result;

/// All physical keys participate, including modifiers held before Win-down.
/// Two overlapping Win keys cancel the tap until both have been released.
#[derive(Clone, Copy, Default)]
struct Tap {
    held: [u64; 4],
    candidate: Option<u32>,
}

impl Tap {
    fn key(&mut self, vk: u32, down: bool, injected: bool) -> bool {
        if injected || vk >= 256 {
            return false;
        }
        let slot = vk as usize / 64;
        let bit = 1 << (vk % 64);
        let was_down = self.held[slot] & bit != 0;
        let win = vk == VK_LWIN.0 as u32 || vk == VK_RWIN.0 as u32;
        if down {
            if !was_down {
                self.candidate = if win && self.held == [0; 4] { Some(vk) } else { None };
                self.held[slot] |= bit;
            }
            false
        } else {
            self.held[slot] &= !bit;
            let tap = was_down && self.candidate == Some(vk);
            if win {
                self.candidate = None;
            }
            tap
        }
    }

    fn click(&mut self) {
        self.candidate = None;
    }
}

thread_local! {
    static TAP: Cell<Tap> = Cell::new(Tap::default());
    static TARGET: Cell<(HWND, u32)> = const { Cell::new((HWND(std::ptr::null_mut()), 0)) };
}

/// Owns both hooks; dropping it on reload removes all input monitoring.
pub struct Hook {
    keyboard: HHOOK,
    mouse: HHOOK,
}

impl Hook {
    pub fn install(hwnd: HWND, message: u32) -> Result<Self> {
        unsafe {
            let module = GetModuleHandleW(None)?;
            let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(module.into()), 0)?;
            let mouse = match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(module.into()), 0) {
                Ok(mouse) => mouse,
                Err(error) => {
                    let _ = UnhookWindowsHookEx(keyboard);
                    return Err(error);
                }
            };
            // Seed held keys when enabling midway through a chord. No hooks or
            // Windows input are involved in the pure state-machine tests.
            let mut tap = Tap::default();
            for vk in 8..256 {
                // Low-level events identify the left/right modifier, not its
                // generic alias. Mouse buttons are excluded from this range.
                if !matches!(vk, 0x10..=0x12) && GetAsyncKeyState(vk) < 0 {
                    tap.key(vk as u32, true, false);
                }
            }
            tap.candidate = None;
            TAP.with(|state| state.set(tap));
            TARGET.with(|target| target.set((hwnd, message)));
            Ok(Self { keyboard, mouse })
        }
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.keyboard);
            let _ = UnhookWindowsHookEx(self.mouse);
        }
        TARGET.with(|target| target.set((HWND(std::ptr::null_mut()), 0)));
        TAP.with(|state| state.set(Tap::default()));
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let message = wparam.0 as u32;
        if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP) {
            let tap = TAP.with(|state| {
                let mut tap = state.get();
                let matched = tap.key(
                    event.vkCode,
                    matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN),
                    event.flags.contains(LLKHF_INJECTED),
                );
                state.set(tap);
                matched
            });
            if tap {
                // The only synchronous work besides the state machine: send a
                // fixed pair before forwarding Win-up, so Explorer sees a chord.
                // 0xE8 is unassigned in Microsoft's virtual-key table and is
                // AutoHotkey's recommended menu mask. No allocation or logging:
                // Windows removes hooks that exceed LowLevelHooksTimeout.
                let key = |flags| INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(0xE8),
                            dwFlags: flags,
                            ..Default::default()
                        },
                    },
                };
                let inputs = [key(Default::default()), key(KEYEVENTF_KEYUP)];
                if unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) } == 2 {
                    TARGET.with(|target| {
                        let (hwnd, message) = target.get();
                        let _ = unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) };
                    });
                }
            }
        }
    }
    // Injected desktop-switch chords (dock/desktops.rs) pass through unchanged
    // and never change Tap, including their Win-down and Win-up events.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32
        && matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL
        )
    {
        let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if event.flags & LLMHF_INJECTED == 0 {
            TAP.with(|state| {
                let mut tap = state.get();
                tap.click();
                state.set(tap);
            });
        }
    }
    // Preserve Win+click and Win+wheel. Pointer movement alone allows a tap.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: u32 = VK_LWIN.0 as u32;
    const R: u32 = VK_RWIN.0 as u32;
    const E: u32 = b'E' as u32;

    fn sequence(events: &[(u32, bool, bool)], expected: &[usize]) {
        let mut tap = Tap::default();
        let taps: Vec<_> = events
            .iter()
            .enumerate()
            .filter_map(|(i, &(vk, down, injected))| tap.key(vk, down, injected).then_some(i))
            .collect();
        assert_eq!(taps, expected);
    }

    #[test]
    fn lone_and_repeated_taps_on_either_side() {
        sequence(
            &[
                (L, true, false),
                (L, false, false),
                (R, true, false),
                (R, false, false),
                (L, true, false),
                (L, false, false),
            ],
            &[1, 3, 5],
        );
    }

    #[test]
    fn chords_cancel_even_after_other_key_released() {
        sequence(
            &[(L, true, false), (E, true, false), (E, false, false), (L, false, false)],
            &[],
        );
        sequence(
            &[(L, true, false), (E, true, false), (L, false, false), (E, false, false)],
            &[],
        );
        sequence(
            &[(E, true, false), (L, true, false), (E, false, false), (L, false, false)],
            &[],
        );
    }

    #[test]
    fn overlapping_win_keys_cancel_both_taps() {
        for (a, b) in [(L, R), (R, L)] {
            sequence(
                &[
                    (a, true, false),
                    (b, true, false),
                    (a, false, false),
                    (b, false, false),
                    (a, true, false),
                    (a, false, false),
                ],
                &[5],
            );
        }
    }

    #[test]
    fn win_auto_repeat_toggles_once() {
        sequence(
            &[
                (L, true, false),
                (L, true, false),
                (L, true, false),
                (L, false, false),
                (L, false, false),
            ],
            &[3],
        );
    }

    #[test]
    fn injected_events_do_not_start_cancel_or_complete_taps() {
        sequence(
            &[(L, true, true), (E, true, true), (E, false, true), (L, false, true)],
            &[],
        );
        sequence(
            &[
                (L, true, false),
                (E, true, true),
                (E, false, true),
                (L, false, true),
                (L, false, false),
            ],
            &[4],
        );
        // The dock injects Ctrl+Win+Arrow, all of which must be ignored.
        sequence(
            &[
                (0xA2, true, true),
                (L, true, true),
                (0x27, true, true),
                (0x27, false, true),
                (L, false, true),
                (0xA2, false, true),
            ],
            &[],
        );
    }

    #[test]
    fn mouse_click_cancels_until_next_tap() {
        let mut tap = Tap::default();
        assert!(!tap.key(L, true, false));
        tap.click();
        assert!(!tap.key(L, false, false));
        assert!(!tap.key(L, true, false));
        assert!(tap.key(L, false, false));
    }
}
