//! Physical-pixel geometry for the shared dock/launcher surface.
use windows::Win32::Foundation::RECT;

pub const OPEN_MS: f32 = 155.0;
pub const CLOSE_MS: f32 = 135.0;

pub fn fades(expanded: f32) -> (f32, f32) {
    (
        (1.0 - expanded / 0.38).clamp(0.0, 1.0),
        ((expanded - 0.4) / 0.6).clamp(0.0, 1.0),
    )
}

pub fn can_morph(visible: bool, hidden_fraction: f32) -> bool {
    visible && hidden_fraction <= 0.4
}

pub fn radius(expanded: f32) -> f32 {
    8.0 + 4.0 * expanded.clamp(0.0, 1.0)
}

pub fn bounds(from: RECT, to: RECT, progress: f32) -> RECT {
    let t = crate::ease::out_cubic(progress);
    let mix = |a: i32, b: i32| (a as f32 + (b - a) as f32 * t).round() as i32;
    RECT {
        left: mix(from.left, to.left),
        top: mix(from.top, to.top),
        right: mix(from.right, to.right),
        bottom: mix(from.bottom, to.bottom),
    }
}

/// Both rendering and pointer coordinates use this bottom-centered offset.
pub fn content_offset(width: f32, height: f32, content_width: f32, content_height: f32) -> (f32, f32) {
    ((width - content_width) / 2.0, height - content_height)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoints_and_reversal() {
        for scale in [1.0, 1.25] {
            let rect = |l: f32, t: f32, r: f32, b: f32| RECT {
                left: (l * scale) as i32,
                top: (t * scale) as i32,
                right: (r * scale) as i32,
                bottom: (b * scale) as i32,
            };
            let a = rect(200., 900., 1000., 956.);
            let b = rect(260., 400., 940., 956.);
            assert_eq!(bounds(a, b, 0.), a);
            assert_eq!(bounds(a, b, 1.), b);
            for t in [0., 0.25, 0.5, 0.75, 1.] {
                assert_eq!(bounds(a, b, t).bottom, bounds(b, a, 1. - t).bottom);
                let r = bounds(a, b, t);
                assert!((r.left + r.right - a.left - a.right).abs() <= 1);
                assert_eq!(r.bottom, a.bottom);
            }
        }
    }
    #[test]
    fn only_a_mostly_revealed_dock_morphs() {
        assert!(!can_morph(false, 0.0));
        assert!(!can_morph(true, 1.0));
        assert!(!can_morph(true, 0.5));
        assert!(can_morph(true, 0.4));
        assert!(can_morph(true, 0.0));
    }
    #[test]
    fn contents_never_overlap() {
        for i in 0..=100 {
            let (dock, launcher) = fades(i as f32 / 100.0);
            assert!(dock == 0.0 || launcher == 0.0);
        }
    }
    #[test]
    fn drawing_and_pointer_offsets_agree() {
        for dpi in [96., 120.] {
            let scale = dpi / 96.;
            let (x, y) = content_offset(500. * scale, 200. * scale, 680. * scale, 487. * scale);
            assert_eq!((x, y), (-90. * scale, -287. * scale));
        }
    }
}
