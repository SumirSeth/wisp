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
}

impl Palette {
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
            }
        }
    }
}
