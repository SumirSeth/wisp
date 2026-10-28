//! Wisp's colors, taken from the app icon: a cyan glow on night blue.

use gpui_kit::component::theme::Theme;
use gpui_kit::{App, Hsla, rgb, rgba};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Hsla,
    /// Module cards.
    pub surface: Hsla,
    /// Controls inside a module, such as tiles and keycaps.
    pub raised: Hsla,
    pub border: Hsla,
    pub fg: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    /// Accent at low opacity, for selected tiles and icon badges.
    pub accent_soft: Hsla,
    pub accent_fg: Hsla,
    pub warn: Hsla,
    pub danger: Hsla,
}

fn dark() -> Palette {
    Palette {
        bg: rgb(0x080c17).into(),
        surface: rgb(0x0f1526).into(),
        raised: rgb(0x161e33).into(),
        border: rgb(0x1f2a44).into(),
        fg: rgb(0xe6ecf7).into(),
        muted: rgb(0x8994ad).into(),
        accent: rgb(0x5ce1f0).into(),
        accent_soft: rgba(0x5ce1f01f).into(),
        accent_fg: rgb(0x04121a).into(),
        warn: rgb(0xf5c46b).into(),
        danger: rgb(0xff7a85).into(),
    }
}

fn light() -> Palette {
    Palette {
        bg: rgb(0xeef2f8).into(),
        surface: rgb(0xffffff).into(),
        raised: rgb(0xf3f6fb).into(),
        border: rgb(0xdbe2ed).into(),
        fg: rgb(0x0e1628).into(),
        muted: rgb(0x5b6880).into(),
        accent: rgb(0x0b93a8).into(),
        accent_soft: rgba(0x0b93a81a).into(),
        accent_fg: rgb(0xffffff).into(),
        warn: rgb(0xa86a00).into(),
        danger: rgb(0xc8323f).into(),
    }
}

impl Palette {
    pub fn current(cx: &App) -> Self {
        if Theme::global(cx).mode.is_dark() {
            dark()
        } else {
            light()
        }
    }
}

/// Recolors the component library so inputs, buttons and switches match Wisp.
pub fn apply(cx: &mut App) {
    let p = Palette::current(cx);
    Theme::update(cx, |t| {
        t.background = p.bg;
        t.foreground = p.fg;
        t.border = p.border;
        t.input = p.border;
        t.ring = p.accent;
        t.caret = p.accent;
        t.selection = p.accent_soft;
        t.muted = p.raised;
        t.muted_foreground = p.muted;
        t.accent = p.raised;
        t.accent_foreground = p.fg;
        t.popover = p.surface;
        t.popover_foreground = p.fg;
        t.list_hover = p.raised;
        t.list_active = p.accent_soft;
        t.primary = p.accent;
        t.primary_hover = p.accent;
        t.primary_active = p.accent;
        t.primary_foreground = p.accent_fg;
        t.secondary = p.raised;
        t.secondary_hover = p.border;
        t.secondary_active = p.border;
        t.secondary_foreground = p.fg;
        t.button = p.raised;
        t.button_hover = p.border;
        t.button_active = p.border;
        t.button_foreground = p.fg;
        t.button_primary = p.accent;
        t.button_primary_hover = p.accent;
        t.button_primary_active = p.accent;
        t.button_primary_foreground = p.accent_fg;
        t.switch = p.border;
        t.switch_thumb = p.fg;
        t.title_bar = p.bg;
        t.title_bar_border = p.border;
        t.window_border = p.border;
        t.danger = p.danger;
        t.warning = p.warn;
    });
}
