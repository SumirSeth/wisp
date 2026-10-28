//! Pure black and paper white. Wisp's cyan marks only focus, selection and
//! switches that are on.

use gpui_kit::component::theme::Theme;
use gpui_kit::{App, Hsla, rgb, rgba};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Hsla,
    /// Groups of settings.
    pub surface: Hsla,
    /// Hovered and selected rows.
    pub raised: Hsla,
    pub border: Hsla,
    pub fg: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub accent_fg: Hsla,
    pub warn: Hsla,
    pub danger: Hsla,
}

fn dark() -> Palette {
    Palette {
        bg: rgb(0x000000).into(),
        surface: rgb(0x0e0e0e).into(),
        raised: rgb(0x1a1a1a).into(),
        border: rgb(0x232323).into(),
        fg: rgb(0xf5f5f5).into(),
        muted: rgb(0x8f8f8f).into(),
        accent: rgb(0x22d3ee).into(),
        accent_soft: rgba(0x22d3ee26).into(),
        accent_fg: rgb(0x00242c).into(),
        warn: rgb(0xf2c94c).into(),
        danger: rgb(0xff6b6b).into(),
    }
}

fn light() -> Palette {
    Palette {
        bg: rgb(0xfaf9f6).into(),
        surface: rgb(0xffffff).into(),
        raised: rgb(0xf0efeb).into(),
        border: rgb(0xe6e4de).into(),
        fg: rgb(0x111111).into(),
        muted: rgb(0x6b6a66).into(),
        accent: rgb(0x0891b2).into(),
        accent_soft: rgba(0x0891b21f).into(),
        accent_fg: rgb(0xffffff).into(),
        warn: rgb(0x9a6700).into(),
        danger: rgb(0xc42b1c).into(),
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

/// Recolors the component library so inputs, buttons and switches match.
pub fn apply(cx: &mut App) {
    let p = Palette::current(cx);
    Theme::update(cx, |t| {
        t.background = p.bg;
        t.foreground = p.fg;
        t.border = p.border;
        t.input = p.border;
        t.ring = p.accent;
        t.caret = p.fg;
        t.selection = p.accent_soft;
        t.muted = p.raised;
        t.muted_foreground = p.muted;
        t.accent = p.raised;
        t.accent_foreground = p.fg;
        t.popover = p.surface;
        t.popover_foreground = p.fg;
        t.list_hover = p.raised;
        t.list_active = p.raised;
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
        t.slider_bar = p.accent;
        t.slider_thumb = p.fg;
        t.sidebar = p.bg;
        t.sidebar_foreground = p.fg;
        t.sidebar_border = p.bg;
        t.sidebar_accent = p.surface;
        t.sidebar_accent_foreground = p.fg;
        t.sidebar_primary = p.accent;
        t.sidebar_primary_foreground = p.accent_fg;
        t.danger = p.danger;
        t.warning = p.warn;
    });
}
