use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

/// Brand color from `assets/icon.svg`.
const BRAND: u32 = 0x0891B2;

fn rgba(hex: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a,
    }
}

/// A user-chosen surface color from `dock_color`: `#RRGGBB`, or `#RRGGBBAA`
/// where `AA` sets how strongly the color covers the theme's acrylic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tint {
    pub rgb: u32,
    pub alpha: Option<f32>,
}

impl Tint {
    /// How much of the theme's acrylic the color covers when `AA` is absent.
    const DEFAULT_ALPHA: f32 = 0.75;
    /// Below this strength the surface looks mostly like the theme's own
    /// acrylic, so the text follows the theme instead of the color.
    const TEXT_ALPHA: f32 = 0.4;

    /// Reads `#RRGGBB` or `#RRGGBBAA`. An empty string means no tint.
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.trim().strip_prefix('#')?;
        if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(hex, 16).ok()?;
        Some(if hex.len() == 6 {
            Self {
                rgb: value,
                alpha: None,
            }
        } else {
            Self {
                rgb: value >> 8,
                alpha: Some((value & 0xFF) as f32 / 255.0),
            }
        })
    }

    /// Only acrylic shows the blur through the color. Mica and solid surfaces
    /// take it at full strength, whatever `AA` says.
    fn on(self, backdrop: crate::config::Backdrop) -> Self {
        if backdrop == crate::config::Backdrop::Acrylic {
            self
        } else {
            Self {
                alpha: Some(1.0),
                ..self
            }
        }
    }

    /// Whether light text reads better on this color, by WCAG relative luminance.
    pub fn is_dark(self) -> bool {
        let channel = |shift: u32| {
            let c = ((self.rgb >> shift) & 0xFF) as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let luminance = 0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0);
        // Above ~0.18, black text has more contrast than white.
        luminance < 0.18
    }
}

/// How strongly composition acrylic tints its blur without a dock color.
const ACCENT_ALPHA: f32 = 0x70 as f32 / 255.0;

/// `rgb` at opacity `alpha` laid over `below`, as one color.
fn over(below: D2D1_COLOR_F, rgb: u32, alpha: f32) -> D2D1_COLOR_F {
    let top = rgba(rgb, 1.0);
    let out = alpha + below.a * (1.0 - alpha);
    if out <= 0.0 {
        return rgba(rgb, 0.0);
    }
    let mix = |t: f32, b: f32| (t * alpha + b * below.a * (1.0 - alpha)) / out;
    D2D1_COLOR_F {
        r: mix(top.r, below.r),
        g: mix(top.g, below.g),
        b: mix(top.b, below.b),
        a: out,
    }
}

/// What composition draws behind a surface.
#[derive(Clone, Copy, PartialEq)]
enum Accent {
    Off,
    /// The tint alone. Cheap enough to redraw on every frame of a resize.
    Tint,
    /// The tint over a blur of what is behind the surface.
    Blur,
}

pub struct Palette {
    pub background: D2D1_COLOR_F,
    pub text: D2D1_COLOR_F,
    pub subtext: D2D1_COLOR_F,
    pub placeholder: D2D1_COLOR_F,
    pub row_selected: D2D1_COLOR_F,
    pub divider: D2D1_COLOR_F,
    pub accent: D2D1_COLOR_F,
    pub text_selection: D2D1_COLOR_F,
    pub tile: D2D1_COLOR_F,
    pub scrollbar: D2D1_COLOR_F,
    /// An opaque background color, for marks on the accent color.
    pub background_solid: D2D1_COLOR_F,
    /// The color composition acrylic lays over its blur, behind the surface.
    pub acrylic_tint: D2D1_COLOR_F,
}

impl Palette {
    /// The palette for `dark` and `translucent`, recolored by `tint`. A
    /// translucent surface keeps the theme's own acrylic and lays the color
    /// over it at the tint's strength: at none it looks like the theme, at
    /// full strength it is the color alone. Text is light or dark for the
    /// color's brightness, or the theme's while the color is too faint to set
    /// the surface's look.
    pub fn with_tint(dark: bool, translucent: bool, tint: Option<Tint>) -> Self {
        let Some(tint) = tint else {
            return Self::new(dark, translucent);
        };
        let alpha = tint.alpha.unwrap_or(Tint::DEFAULT_ALPHA);
        let text_dark = if alpha < Tint::TEXT_ALPHA { dark } else { tint.is_dark() };
        let theme = Self::new(dark, translucent);
        let mut palette = Self::new(text_dark, translucent);
        if translucent {
            palette.background = over(theme.background, tint.rgb, alpha);
            palette.acrylic_tint = theme.acrylic_tint;
        } else {
            palette.background = over(theme.background, tint.rgb, alpha);
        }
        palette
    }

    /// A translucent fill lets the acrylic or mica backdrop show through.
    pub fn new(dark: bool, translucent: bool) -> Self {
        if dark {
            Self {
                background: if translucent {
                    rgba(0x1C1C1C, 0.3)
                } else {
                    rgba(0x202020, 1.0)
                },
                text: rgba(0xF5F5F5, 1.0),
                subtext: rgba(0xFFFFFF, 0.58),
                placeholder: rgba(0xFFFFFF, 0.42),
                row_selected: rgba(0xFFFFFF, 0.08),
                divider: rgba(0xFFFFFF, 0.08),
                accent: rgba(0x22D3EE, 1.0),
                text_selection: rgba(BRAND, 0.6),
                tile: rgba(0xFFFFFF, 0.07),
                scrollbar: rgba(0xFFFFFF, 0.22),
                background_solid: rgba(0x1C1C1C, 1.0),
                acrylic_tint: rgba(0x1C1C1C, ACCENT_ALPHA),
            }
        } else {
            Self {
                background: if translucent {
                    rgba(0xF3F3F3, 0.4)
                } else {
                    rgba(0xF9F9F9, 1.0)
                },
                text: rgba(0x1B1B1B, 1.0),
                subtext: rgba(0x000000, 0.58),
                placeholder: rgba(0x000000, 0.42),
                row_selected: rgba(0x000000, 0.055),
                divider: rgba(0x000000, 0.08),
                accent: rgba(BRAND, 1.0),
                text_selection: rgba(BRAND, 0.3),
                tile: rgba(0x000000, 0.05),
                scrollbar: rgba(0x000000, 0.25),
                background_solid: rgba(0xFFFFFF, 1.0),
                acrylic_tint: rgba(0xFFFFFF, ACCENT_ALPHA),
            }
        }
    }
}

pub fn transparency_available() -> bool {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    use windows::Win32::System::Registry::*;
    use windows::core::w;
    let mut enabled = 1u32;
    let mut size = 4u32;
    let mut power = SYSTEM_POWER_STATUS::default();
    unsafe {
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("EnableTransparency"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut enabled as *mut u32).cast()),
            Some(&mut size),
        );
        let saver = GetSystemPowerStatus(&mut power).is_ok() && power.SystemStatusFlag != 0;
        enabled != 0 && !saver
    }
}

/// Nonactivating surfaces need the composition acrylic policy as well as DWM's backdrop.
fn composition_accent(hwnd: windows::Win32::Foundation::HWND, accent: Accent, palette: &Palette) -> bool {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::core::{BOOL, s, w};
    #[repr(C)]
    struct Policy {
        state: u32,
        flags: u32,
        tint: u32,
        animation: u32,
    }
    #[repr(C)]
    struct Attribute {
        attribute: u32,
        data: *mut Policy,
        size: usize,
    }
    unsafe {
        let Ok(module) = GetModuleHandleW(w!("user32.dll")) else {
            return false;
        };
        let Some(proc) = GetProcAddress(module, s!("SetWindowCompositionAttribute")) else {
            return false;
        };
        let set: unsafe extern "system" fn(windows::Win32::Foundation::HWND, *mut Attribute) -> BOOL =
            std::mem::transmute(proc);
        let alpha = (palette.acrylic_tint.a * 255.0).round() as u32;
        let mut accent = Policy {
            state: match accent {
                Accent::Off => 0,
                Accent::Tint => 2,
                Accent::Blur => 4,
            },
            flags: 2,
            tint: alpha << 24
                | ((palette.acrylic_tint.b * 255.0) as u32) << 16
                | ((palette.acrylic_tint.g * 255.0) as u32) << 8
                | (palette.acrylic_tint.r * 255.0) as u32,
            animation: 0,
        };
        let mut attribute = Attribute {
            attribute: 19,
            data: &mut accent,
            size: size_of::<Policy>(),
        };
        set(hwnd, &mut attribute).as_bool()
    }
}

/// One material policy for launcher, dock and its owned popups. Persistent
/// acrylic uses composition blur because DWM's system material falls back
/// to solid when the nonactivating dock is inactive.
pub fn apply_backdrop(
    hwnd: windows::Win32::Foundation::HWND,
    dark: bool,
    backdrop: crate::config::Backdrop,
    persistent: bool,
    tint: Option<Tint>,
) -> Palette {
    use crate::config::Backdrop;
    use windows::Win32::Graphics::Dwm::*;
    use windows::Win32::UI::Controls::MARGINS;
    let tint = tint.map(|tint| tint.on(backdrop));
    let translucent = backdrop != Backdrop::Solid && transparency_available();
    let mut palette = Palette::with_tint(dark, translucent, tint);
    // DWM's caption and dark-mode flags follow the text the tint chose.
    let caption_dark = palette.text.r > 0.5;
    unsafe {
        let set = |attr, value: i32| DwmSetWindowAttribute(hwnd, attr, (&value as *const i32).cast(), 4);
        let _ = set(DWMWA_USE_IMMERSIVE_DARK_MODE, i32::from(caption_dark));
        let _ = set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND.0);
        let _ = set(DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE as i32);
        let accent = persistent && backdrop == Backdrop::Acrylic && translucent;
        let material = if !translucent || accent {
            DWMSBT_NONE
        } else {
            match backdrop {
                Backdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
                Backdrop::Mica => DWMSBT_MAINWINDOW,
                Backdrop::Solid => DWMSBT_NONE,
            }
        };
        {
            use windows::Win32::UI::WindowsAndMessaging::*;
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            SetWindowLongPtrW(
                hwnd,
                GWL_STYLE,
                if persistent {
                    style & !(WS_CAPTION.0 as isize)
                } else {
                    style | WS_CAPTION.0 as isize
                },
            );
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
        let system = set(DWMWA_SYSTEMBACKDROP_TYPE, material.0).is_ok();
        let _ = DwmExtendFrameIntoClientArea(
            hwnd,
            &MARGINS {
                cxLeftWidth: if persistent { 0 } else { -1 },
                cxRightWidth: if persistent { 0 } else { -1 },
                cyTopHeight: if persistent { 0 } else { -1 },
                cyBottomHeight: if persistent { 0 } else { -1 },
            },
        );
        if !persistent {
            let _ = windows::Win32::Graphics::Gdi::SetWindowRgn(hwnd, None, false);
        }
        let composition = composition_accent(hwnd, if accent { Accent::Blur } else { Accent::Off }, &palette);
        if !system || accent && !composition {
            palette = Palette::with_tint(dark, false, tint);
        }
    }
    palette
}

/// The material for a persistent surface while it changes size. Composition
/// acrylic blurs again on every resize, so motion uses a flat color. A dock
/// color keeps its strength through the motion, without the blur, so the
/// surface does not turn solid and back.
pub fn apply_moving_backdrop(
    hwnd: windows::Win32::Foundation::HWND,
    dark: bool,
    backdrop: crate::config::Backdrop,
    tint: Option<Tint>,
) -> Palette {
    use crate::config::Backdrop;
    let solid = apply_backdrop(hwnd, dark, Backdrop::Solid, true, tint);
    let Some(tint) = tint.filter(|tint| {
        backdrop == Backdrop::Acrylic && tint.alpha.unwrap_or(Tint::DEFAULT_ALPHA) < 1.0 && transparency_available()
    }) else {
        return solid;
    };
    let palette = Palette::with_tint(dark, true, Some(tint));
    if composition_accent(hwnd, Accent::Tint, &palette) {
        palette
    } else {
        solid
    }
}

/// The shared tint and hairline, in device-independent coordinates.
pub fn paint_background(
    rt: &windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget,
    brush: &windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush,
    palette: &Palette,
    _radius: f32,
) {
    let size = unsafe { rt.GetSize() };
    let (width, height) = (size.width, size.height);
    use windows::Win32::Graphics::Direct2D::Common::*;
    use windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT;
    unsafe {
        // DWM owns the antialiased outer silhouette. Fill through that edge:
        // a second rounded alpha mask exposes the theme acrylic between the
        // independently rasterized curves, especially with an opaque tint.
        rt.Clear(Some(&palette.background));
        let (mut dpi, mut dpi_y) = (96.0, 96.0);
        rt.GetDpi(&mut dpi, &mut dpi_y);
        let pixel = 96.0 / dpi;
        // DWMWCP_ROUND uses the native 8-DIP corner for every surface.
        let radius = 8.0;
        brush.SetColor(&D2D1_COLOR_F {
            a: if palette.text.r > 0.5 { 0.14 } else { 0.12 },
            ..palette.text
        });
        rt.DrawRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: pixel / 2.0,
                    top: pixel / 2.0,
                    right: width - pixel / 2.0,
                    bottom: height - pixel / 2.0,
                },
                radiusX: (radius - pixel / 2.0).max(0.0),
                radiusY: (radius - pixel / 2.0).max(0.0),
            },
            brush,
            pixel,
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tints_parse_with_and_without_alpha() {
        assert_eq!(
            Tint::parse("#1E3A8A"),
            Some(Tint {
                rgb: 0x1E3A8A,
                alpha: None
            })
        );
        let tint = Tint::parse(" #ff000080 ").unwrap();
        assert_eq!(tint.rgb, 0xFF0000);
        assert!((tint.alpha.unwrap() - 128.0 / 255.0).abs() < 1e-6);
        for bad in ["", "1E3A8A", "#12345", "#1234567", "#GGGGGG", "#123456789"] {
            assert_eq!(Tint::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn tints_choose_readable_text() {
        assert!(Tint::parse("#000000").unwrap().is_dark());
        assert!(Tint::parse("#1E3A8A").unwrap().is_dark());
        assert!(!Tint::parse("#FFFFFF").unwrap().is_dark());
        assert!(!Tint::parse("#FDE047").unwrap().is_dark());
        // Light text on a dark tint, whatever the theme says.
        let palette = Palette::with_tint(false, true, Tint::parse("#101010"));
        assert!(palette.text.r > 0.5);
        // The default strength over the light theme's 40% fill.
        assert!((palette.background.a - (0.75 + 0.4 * 0.25)).abs() < 1e-6);
        // A solid surface is always opaque.
        assert_eq!(
            Palette::with_tint(true, false, Tint::parse("#10101040")).background.a,
            1.0
        );
    }

    #[test]
    fn a_faint_tint_keeps_the_themes_text() {
        // Black at 6% over the light theme's acrylic: dark text, as the theme has.
        let faint = Tint::parse("#00000010");
        assert!(Palette::with_tint(false, true, faint).text.r < 0.5);
        assert!(Palette::with_tint(true, true, faint).text.r > 0.5);
        // At full strength the color decides, even if transparency is unavailable.
        assert!(Palette::with_tint(false, true, Tint::parse("#000000FF")).text.r > 0.5);
        assert!(Palette::with_tint(false, false, faint).text.r < 0.5);
    }

    #[test]
    fn only_acrylic_shows_through_a_tint() {
        use crate::config::Backdrop;
        let tint = Tint::parse("#1E3A8A1A").unwrap();
        assert_eq!(tint.on(Backdrop::Acrylic), tint);
        for backdrop in [Backdrop::Mica, Backdrop::Solid] {
            let palette = Palette::with_tint(true, true, Some(tint.on(backdrop)));
            assert_eq!(palette.background.a, 1.0);
        }
    }

    #[test]
    fn the_color_lies_over_the_themes_acrylic_at_its_strength() {
        let close = |a: D2D1_COLOR_F, b: D2D1_COLOR_F| {
            (a.r - b.r).abs() < 1e-5 && (a.g - b.g).abs() < 1e-5 && (a.b - b.b).abs() < 1e-5 && (a.a - b.a).abs() < 1e-5
        };
        for dark in [true, false] {
            let theme = Palette::new(dark, true);
            // No strength: the theme's own acrylic, accent and fill.
            let none = Palette::with_tint(dark, true, Tint::parse("#1E3A8A00"));
            assert!(close(none.background, theme.background) && close(none.acrylic_tint, theme.acrylic_tint));
            // Full strength: the color alone.
            let full = Palette::with_tint(dark, true, Tint::parse("#1E3A8AFF"));
            assert!(close(full.background, rgba(0x1E3A8A, 1.0)));
            // In between, more strength covers more of the theme.
            let low = Palette::with_tint(dark, true, Tint::parse("#1E3A8A33")).background;
            let high = Palette::with_tint(dark, true, Tint::parse("#1E3A8ACC")).background;
            assert!(theme.background.a < low.a && low.a < high.a && high.a < 1.0);
        }
        // The accent keeps the theme's usual strength.
        assert_eq!((Palette::new(true, true).acrylic_tint.a * 255.0).round(), 112.0);
    }
    #[test]
    fn opaque_fallback_preserves_tint_strength_and_theme() {
        for dark in [false, true] {
            for value in ["#00000010", "#FFFFFF10", "#00000000", "#FFFFFFFF"] {
                let tint = Tint::parse(value).unwrap();
                let expected = over(Palette::new(dark, false).background, tint.rgb, tint.alpha.unwrap());
                let actual = Palette::with_tint(dark, false, Some(tint)).background;
                assert_eq!(
                    (actual.r, actual.g, actual.b, actual.a),
                    (expected.r, expected.g, expected.b, expected.a)
                );
            }
            // A contrasting tint changes text, but subsequent applications still use the real theme.
            let contrast = Tint::parse(if dark { "#FFFFFF80" } else { "#00000080" });
            assert_ne!(Palette::with_tint(dark, true, contrast).text.r > 0.5, dark);
            let fallback = Palette::with_tint(dark, false, contrast);
            let expected = over(
                Palette::new(dark, false).background,
                contrast.unwrap().rgb,
                128.0 / 255.0,
            );
            assert_eq!(
                (fallback.background.r, fallback.background.g, fallback.background.b),
                (expected.r, expected.g, expected.b)
            );
            let restored = Palette::with_tint(dark, true, None);
            assert_eq!(restored.text.r > 0.5, dark);
        }
    }
}

/// The material inputs whose changes require DWM calls.
#[derive(Clone, Copy, PartialEq)]
pub struct MaterialKey(pub bool, pub crate::config::Backdrop, pub Option<Tint>, pub bool);

impl MaterialKey {
    pub fn new(dark: bool, backdrop: crate::config::Backdrop, tint: Option<Tint>) -> Self {
        Self(dark, backdrop, tint, transparency_available())
    }
}

/// Classify the borrowed broadcast payload before any modal-loop deferral.
pub unsafe fn filter_broadcast(
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::*;
    if msg == WM_SETTINGCHANGE {
        let immersive = if lp.0 == 0 {
            false
        } else {
            unsafe {
                windows::core::PCWSTR(lp.0 as _)
                    .to_string()
                    .is_ok_and(|name| name == "ImmersiveColorSet")
            }
        };
        setting_message(wp.0, immersive)
    } else if msg == WM_POWERBROADCAST && wp.0 != 10 {
        // PBT_APMPOWERSTATUSCHANGE includes battery-saver transitions.
        None
    } else {
        Some(msg)
    }
}

fn setting_message(action: usize, immersive: bool) -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::*;
    if immersive {
        Some(WM_THEMECHANGED)
    } else if action == SPI_SETWORKAREA.0 as usize {
        Some(WM_SETTINGCHANGE)
    } else {
        None
    }
}

#[cfg(test)]
mod broadcast_tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    #[test]
    fn only_theme_and_work_area_settings_trigger_work() {
        assert_eq!(setting_message(0, false), None);
        assert_eq!(
            setting_message(SPI_SETWORKAREA.0 as usize, false),
            Some(WM_SETTINGCHANGE)
        );
        assert_eq!(setting_message(0, true), Some(WM_THEMECHANGED));
    }
}
