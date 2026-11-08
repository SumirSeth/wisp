//! State comparisons independent of Win32.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

pub const AUTOHIDE: u32 = 1;

pub fn hidden_state(previous: u32) -> u32 {
    previous | AUTOHIDE
}

/// Reclaim only a strip attributable to this taskbar; retain other appbars.
pub fn reclaimed_work(monitor: Rect, work: Rect, bar: Rect) -> Rect {
    let mut target = work;
    if bar.left <= monitor.left && bar.right >= monitor.right {
        if bar.top <= work.bottom && bar.bottom >= monitor.bottom && bar.top > monitor.top {
            target.bottom = monitor.bottom;
        } else if bar.bottom >= work.top && bar.top <= monitor.top && bar.bottom < monitor.bottom {
            target.top = monitor.top;
        }
    } else if bar.top <= monitor.top && bar.bottom >= monitor.bottom {
        if bar.left <= work.right && bar.right >= monitor.right && bar.left > monitor.left {
            target.right = monitor.right;
        } else if bar.right >= work.left && bar.left <= monitor.left && bar.right < monitor.right {
            target.left = monitor.left;
        }
    }
    target
}

pub fn visibility_diff(previous: bool, current: bool) -> Option<bool> {
    (previous != current).then_some(previous)
}

/// Keep a full-window region the size of the taskbar after a display change.
pub fn restored_region(region: Option<Rect>, previous: Rect, current: Rect) -> Option<Rect> {
    let full = |rect: Rect| Rect {
        right: rect.right - rect.left,
        bottom: rect.bottom - rect.top,
        ..Rect::default()
    };
    if region == Some(full(previous)) {
        Some(full(current))
    } else {
        region
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_other_state_bits() {
        for state in 0..8 {
            assert_eq!(hidden_state(state), state | 1);
        }
    }

    #[test]
    fn visibility_is_idempotent() {
        assert_eq!(visibility_diff(true, false), Some(true));
        assert_eq!(visibility_diff(false, true), Some(false));
        assert_eq!(visibility_diff(false, false), None);
        assert_eq!(visibility_diff(true, true), None);
    }

    #[test]
    fn restores_full_window_regions_at_the_current_size() {
        let previous = Rect {
            left: -2560,
            top: 1380,
            right: 0,
            bottom: 1440,
        };
        let region = Rect {
            right: 2560,
            bottom: 60,
            ..Rect::default()
        };
        for (width, height) in [(3840, 90), (1920, 40), (60, 1440), (2560, 60)] {
            let current = Rect {
                left: 100,
                top: -200,
                right: 100 + width,
                bottom: -200 + height,
            };
            assert_eq!(
                restored_region(Some(region), previous, current),
                Some(Rect {
                    right: width,
                    bottom: height,
                    ..Rect::default()
                })
            );
            assert_eq!(restored_region(None, previous, current), None);
            for custom in [
                Rect { right: 2500, ..region },
                Rect { bottom: 50, ..region },
                Rect { left: 1, ..region },
                Rect { top: 1, ..region },
                Rect::default(),
            ] {
                assert_eq!(restored_region(Some(custom), previous, current), Some(custom));
            }
        }
    }

    #[test]
    fn reclaims_only_taskbar_edge() {
        let monitor = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let work = Rect {
            top: 30,
            bottom: 1040,
            ..monitor
        };
        let bar = Rect { top: 1040, ..monitor };
        assert_eq!(reclaimed_work(monitor, work, bar), Rect { bottom: 1080, ..work });
        let unrelated = Rect {
            left: -1000,
            right: -900,
            top: 500,
            bottom: 600,
        };
        assert_eq!(reclaimed_work(monitor, work, unrelated), work);
        let left = Rect {
            right: -1880,
            ..monitor
        };
        let work = Rect { left: -1880, ..monitor };
        assert_eq!(reclaimed_work(monitor, work, left), monitor);
        let top = Rect { bottom: 40, ..monitor };
        assert_eq!(reclaimed_work(monitor, Rect { top: 40, ..monitor }, top), monitor);
        let right = Rect { left: -40, ..monitor };
        assert_eq!(reclaimed_work(monitor, Rect { right: -40, ..monitor }, right), monitor);
    }
}
