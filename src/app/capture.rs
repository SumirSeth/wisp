//! Scripted runs for screenshots, recordings and performance traces.
//!
//! Driven by environment variables that only `perf-trace` builds read: in
//! release builds `perf::hook_on` is always false, so all of this compiles
//! away. The launcher calls in at a handful of one-line hooks.

use std::cell::Cell;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, IsWindowVisible, KillTimer, SetTimer};

use super::{Launcher, with_launcher};
use crate::util;

/// Steps a performance scenario: typing, sliding, or show and hide.
const PERF_TIMER: usize = 97;
/// Ends a capture scenario after its first moments.
const SCENARIO_TIMER: usize = 98;
/// Quits once the capture's duration is over.
const END_TIMER: usize = 99;
/// Steps the dock-to-launcher reel.
const REEL_TIMER: usize = 96;
const REEL_MS: u32 = 50;

/// Whether this run is scripted.
pub(super) fn active() -> bool {
    util::perf::hook_on("WISP_CAPTURE")
}

/// Sets up a scripted run after the launcher starts.
pub(super) fn start(l: &mut Launcher) {
    if !active() {
        return;
    }
    unsafe {
        if !matches!(
            util::perf::hook("WISP_CAPTURE_SCENARIO").as_deref(),
            Some("hidden" | "reel")
        ) && let Some(dock) = &mut l.dock
        {
            dock.hold(
                None,
                util::perf::hook("WISP_CAPTURE_DOCK_PART").as_deref(),
                util::perf::hook("DOCK_DEMO_DPI").and_then(|s| s.parse().ok()),
            );
        }
        if let Some(query) = util::perf::hook("WISP_CAPTURE_QUERY") {
            l.editor.insert(&query);
        }
        SetTimer(
            Some(l.hwnd),
            END_TIMER,
            util::perf::hook("WISP_CAPTURE_DURATION_MS")
                .and_then(|s| s.parse().ok())
                .unwrap_or(3500),
            None,
        );
        #[cfg(feature = "perf-trace")]
        if let Some(scenario) = util::perf::hook("WISP_PERF_SCENARIO") {
            if scenario.ends_with("hidden")
                && let Some(dock) = &mut l.dock
            {
                dock.hide();
                dock.show();
            }
            if !scenario.starts_with("idle") {
                SetTimer(Some(l.hwnd), PERF_TIMER, 250, None);
            }
        }
        if util::perf::hook("WISP_CAPTURE_SCENARIO").as_deref() == Some("reel") {
            // The app icons load during the first 4 s.
            SetTimer(Some(l.hwnd), REEL_TIMER, 4000, None);
        } else if util::perf::hook_on("WISP_CAPTURE_SCENARIO") {
            SetTimer(Some(l.hwnd), SCENARIO_TIMER, 45, None);
        }
    }
}

pub(super) fn owns_timer(id: usize) -> bool {
    active() && matches!(id, PERF_TIMER | SCENARIO_TIMER | END_TIMER | REEL_TIMER)
}

pub(super) fn on_timer(hwnd: HWND, id: usize) {
    match id {
        PERF_TIMER => perf_step(),
        SCENARIO_TIMER => scenario_end(),
        REEL_TIMER => reel_step(hwnd),
        END_TIMER => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        _ => {}
    }
}

/// A fixed point of the dock-to-launcher animation, for stills.
pub(super) fn fixed_morph() -> Option<f32> {
    if !active() {
        return None;
    }
    let value = util::perf::hook("WISP_CAPTURE_MORPH")?.parse::<f32>().ok()?;
    Some(value.clamp(0.0, 1.0))
}

/// Typing while the launcher opens, to check the query survives the animation.
pub(super) fn type_during_morph(l: &mut Launcher) {
    if active() && util::perf::hook("WISP_CAPTURE_SCENARIO").as_deref() == Some("typing") && l.editor.text() != "typed"
    {
        l.editor.insert("typed");
        l.update_results();
        eprintln!("during animation query={} morph=true", l.editor.text());
    }
}

/// Keeps screenshots to a set number of result rows.
pub(super) fn limit_rows<T>(results: &mut Vec<T>) {
    if active()
        && let Some(rows) = util::perf::hook("WISP_CAPTURE_ROWS")
        && let Ok(rows) = rows.parse::<usize>()
    {
        results.truncate(rows);
    }
}

#[cfg(feature = "perf-trace")]
fn perf_step() {
    with_launcher(|l| {
        thread_local! { static STEP: Cell<usize> = const { Cell::new(0) }; }
        let step = STEP.with(|s| {
            let n = s.get();
            s.set(n + 1);
            n
        });
        let scenario = util::perf::hook("WISP_PERF_SCENARIO").unwrap_or_default();
        if scenario == "typing" {
            if !l.visible {
                l.show();
            }
            util::perf::input("key_to_content");
            l.on_char(if step.is_multiple_of(6) { 8 } else { 101 });
            l.paint();
        } else if scenario == "slide" {
            if let Some(dock) = &mut l.dock {
                dock.capture_slide();
            }
        } else if !scenario.starts_with("idle") {
            if l.visible {
                l.hide();
            } else {
                if scenario == "hidden"
                    && let Some(dock) = &mut l.dock
                {
                    dock.hide();
                }
                util::perf::input("hotkey_to_content");
                l.show();
            }
        }
    });
}

#[cfg(not(feature = "perf-trace"))]
fn perf_step() {}

fn scenario_end() {
    with_launcher(|l| {
        unsafe {
            let _ = KillTimer(Some(l.hwnd), SCENARIO_TIMER);
        }
        let scenario = util::perf::hook("WISP_CAPTURE_SCENARIO").unwrap_or_default();
        eprintln!(
            "scenario={scenario} visible={} morph={} return_to_dock={}",
            l.visible,
            l.morph.is_some(),
            l.morphed
        );
        if scenario == "slide" {
            if let Some(dock) = &mut l.dock {
                dock.capture_slide();
            }
        } else if scenario == "typing" {
            eprintln!("retained query={}", l.editor.text());
        } else {
            l.hide();
            eprintln!(
                "dismiss visible={} morph={} hwnd_visible={}",
                l.visible,
                l.morph.is_some(),
                unsafe { IsWindowVisible(l.hwnd).as_bool() }
            );
        }
    });
}

fn reel_step(hwnd: HWND) {
    with_launcher(|l| {
        thread_local! { static STEP: Cell<u32> = const { Cell::new(0) }; }
        let step = STEP.with(|s| {
            s.set(s.get() + 1);
            s.get()
        });
        if step == 1 {
            let interval = (REEL_MS as f32 * util::perf::slowdown()) as u32;
            unsafe {
                SetTimer(Some(l.hwnd), REEL_TIMER, interval, None);
            }
        }
        // Steps of REEL_MS: the hidden dock slides in, grows into the
        // launcher, takes a query, then shrinks back into the dock.
        match step {
            1 => {
                if let Some(dock) = &mut l.dock {
                    dock.capture_reveal();
                }
            }
            19 => l.show(),
            // The whole query at once: partial queries can list
            // apps by their paths on this PC.
            29 => {
                for unit in b"note" {
                    l.on_char((*unit).into());
                }
                l.paint();
            }
            56 => l.hide(),
            81 => unsafe {
                let _ = DestroyWindow(hwnd);
            },
            _ => {}
        }
    });
}
