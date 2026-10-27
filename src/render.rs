//! Direct2D/DirectWrite drawing. Everything is laid out in DIPs; the render
//! target's DPI handles scaling.

use std::collections::HashMap;
use std::sync::Arc;

use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::{Interface, PCWSTR, Result, w};
use windows_numerics::Vector2;

use crate::indexer::IconPixels;
use crate::search::{Kind, ResultItem};
use crate::theme::Palette;
use crate::util::wide;

pub mod layout {
    pub const WIDTH: f32 = 680.0;
    pub const INPUT_H: f32 = 58.0;
    pub const ROW_H: f32 = 52.0;
    pub const PAD: f32 = 8.0;
    pub const LIST_PAD: f32 = 6.0;
    pub const LIST_TOP: f32 = INPUT_H + 1.0 + LIST_PAD;

    pub fn height(rows: usize) -> f32 {
        if rows == 0 {
            INPUT_H
        } else {
            LIST_TOP + rows as f32 * ROW_H + LIST_PAD
        }
    }
}
use layout::*;

const ICON: f32 = 32.0;
const ICON_X: f32 = PAD + 12.0;
const TEXT_X: f32 = ICON_X + ICON + 14.0;
const HINT_W: f32 = 96.0;
const INPUT_TEXT_X: f32 = 52.0;

pub struct Frame<'a> {
    pub palette: &'a Palette,
    pub text: &'a str,
    /// UTF-16 offsets into `text`.
    pub caret: u32,
    pub anchor: u32,
    pub caret_visible: bool,
    pub placeholder: &'a str,
    pub results: &'a [ResultItem],
    pub selected: usize,
    pub scroll: usize,
    pub rows: usize,
    pub icons: &'a HashMap<String, Arc<IconPixels>>,
}

struct Formats {
    query: IDWriteTextFormat,
    title: IDWriteTextFormat,
    subtitle: IDWriteTextFormat,
    hint: IDWriteTextFormat,
    glyph: IDWriteTextFormat,
    search_glyph: IDWriteTextFormat,
    letter: IDWriteTextFormat,
}

struct Target {
    rt: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    bitmaps: HashMap<String, ID2D1Bitmap>,
}

pub struct Renderer {
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    formats: Formats,
    target: Option<Target>,
    dpi: f32,
    input_scroll: f32,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let ui = w!("Segoe UI Variable Text");
            let icons = w!("Segoe Fluent Icons");
            let formats = Formats {
                query: text_format(
                    &dwrite,
                    w!("Segoe UI Variable Display"),
                    20.0,
                    DWRITE_FONT_WEIGHT_NORMAL,
                )?,
                title: ellipsized(&dwrite, text_format(&dwrite, ui, 14.5, DWRITE_FONT_WEIGHT_NORMAL)?)?,
                subtitle: ellipsized(&dwrite, text_format(&dwrite, ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?)?,
                hint: text_format(&dwrite, ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                glyph: text_format(&dwrite, icons, 16.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                search_glyph: text_format(&dwrite, icons, 17.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                letter: text_format(&dwrite, ui, 15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
            };
            formats.hint.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            for format in [&formats.glyph, &formats.search_glyph, &formats.letter] {
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            }
            Ok(Self {
                d2d,
                dwrite,
                formats,
                target: None,
                dpi: 96.0,
                input_scroll: 0.0,
            })
        }
    }

    pub fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi as f32;
        if let Some(t) = &self.target {
            unsafe { t.rt.SetDpi(self.dpi, self.dpi) };
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if let Some(t) = &self.target {
            unsafe {
                let _ = t.rt.Resize(&D2D_SIZE_U { width, height });
            }
        }
    }

    pub fn draw(&mut self, hwnd: HWND, frame: &Frame) -> Result<()> {
        if self.target.is_none() {
            self.target = Some(self.create_target(hwnd)?);
        }
        let target = self.target.as_mut().unwrap();
        // EndDraw must follow BeginDraw even if drawing fails partway.
        let rt = target.rt.clone();
        let result = unsafe {
            rt.BeginDraw();
            let drawn = draw_frame(&self.dwrite, &self.formats, target, &mut self.input_scroll, frame);
            let ended = rt.EndDraw(None, None);
            drawn.and(ended)
        };
        if let Err(e) = &result
            && e.code() == D2DERR_RECREATE_TARGET
        {
            self.target = None;
            return Ok(());
        }
        result
    }

    fn create_target(&self, hwnd: HWND) -> Result<Target> {
        unsafe {
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                // The window is small enough that CPU rasterization takes ~1ms,
                // and skipping the GPU driver saves ~30 MB of private memory.
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                ..Default::default()
            };
            let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U {
                    width: rc.right as u32,
                    height: rc.bottom as u32,
                },
                presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
            };
            let rt = self.d2d.CreateHwndRenderTarget(&props, &hwnd_props)?;
            rt.SetDpi(self.dpi, self.dpi);
            // ClearType needs an opaque background; we draw over a translucent one.
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let white = D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            };
            Ok(Target {
                brush: rt.CreateSolidColorBrush(&white, None)?,
                accent: rt.CreateSolidColorBrush(&white, None)?,
                bitmaps: HashMap::new(),
                rt,
            })
        }
    }
}

unsafe fn text_format(
    dw: &IDWriteFactory,
    family: PCWSTR,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = dw.CreateTextFormat(
            family,
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        Ok(format)
    }
}

unsafe fn ellipsized(dw: &IDWriteFactory, format: IDWriteTextFormat) -> Result<IDWriteTextFormat> {
    unsafe {
        let sign = dw.CreateEllipsisTrimmingSign(&format)?;
        let trimming = DWRITE_TRIMMING {
            granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
            delimiter: 0,
            delimiterCount: 0,
        };
        format.SetTrimming(&trimming, &sign)?;
        Ok(format)
    }
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left,
        top,
        right,
        bottom,
    }
}

fn rounded(r: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: r,
        radiusX: radius,
        radiusY: radius,
    }
}

unsafe fn draw_text(
    rt: &ID2D1RenderTarget,
    text: &str,
    format: &IDWriteTextFormat,
    r: D2D_RECT_F,
    brush: &ID2D1SolidColorBrush,
) {
    unsafe {
        rt.DrawText(
            &wide(text),
            format,
            &r,
            brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }
}

unsafe fn caret_x(layout: &IDWriteTextLayout, pos: u32) -> f32 {
    let (mut x, mut y) = (0.0, 0.0);
    let mut metrics = DWRITE_HIT_TEST_METRICS::default();
    unsafe {
        let _ = layout.HitTestTextPosition(pos, false, &mut x, &mut y, &mut metrics);
    }
    x
}

unsafe fn draw_frame(
    dw: &IDWriteFactory,
    fm: &Formats,
    t: &mut Target,
    input_scroll: &mut f32,
    f: &Frame,
) -> Result<()> {
    unsafe {
        let target = t.rt.clone();
        let rt: &ID2D1RenderTarget = &target;
        let p = f.palette;
        let width = rt.GetSize().width;
        let brush = &t.brush.clone();
        let accent = &t.accent.clone();
        accent.SetColor(&p.accent);

        rt.Clear(Some(&p.background));

        // Search box.
        let mid = INPUT_H / 2.0;
        brush.SetColor(&p.placeholder);
        draw_text(rt, "\u{E721}", &fm.search_glyph, rect(16.0, 0.0, 44.0, INPUT_H), brush);

        let text_right = width - 20.0;
        let box_w = text_right - INPUT_TEXT_X;
        let mut caret_at = INPUT_TEXT_X;
        if f.text.is_empty() {
            *input_scroll = 0.0;
            draw_text(
                rt,
                f.placeholder,
                &fm.query,
                rect(INPUT_TEXT_X, 0.0, text_right, INPUT_H),
                brush,
            );
        } else {
            let layout = dw.CreateTextLayout(&wide(f.text), &fm.query, 100_000.0, INPUT_H)?;
            let caret = caret_x(&layout, f.caret);
            // Keep the caret in view when the text is wider than the box.
            if caret - *input_scroll > box_w - 2.0 {
                *input_scroll = caret - box_w + 2.0;
            } else if caret < *input_scroll {
                *input_scroll = caret;
            }
            let origin = INPUT_TEXT_X - *input_scroll;
            rt.PushAxisAlignedClip(
                &rect(INPUT_TEXT_X - 2.0, 0.0, text_right + 2.0, INPUT_H),
                D2D1_ANTIALIAS_MODE_ALIASED,
            );
            if f.anchor != f.caret {
                let anchor = caret_x(&layout, f.anchor);
                brush.SetColor(&p.text_selection);
                rt.FillRectangle(
                    &rect(
                        origin + caret.min(anchor),
                        mid - 13.0,
                        origin + caret.max(anchor),
                        mid + 13.0,
                    ),
                    brush,
                );
            }
            brush.SetColor(&p.text);
            rt.DrawTextLayout(
                Vector2 { X: origin, Y: 0.0 },
                &layout,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
            rt.PopAxisAlignedClip();
            caret_at = origin + caret;
        }
        if f.caret_visible {
            rt.FillRectangle(&rect(caret_at, mid - 12.0, caret_at + 1.5, mid + 12.0), accent);
        }

        // Results.
        if f.rows > 0 {
            brush.SetColor(&p.divider);
            rt.FillRectangle(&rect(0.0, INPUT_H, width, INPUT_H + 1.0), brush);

            for (row, (index, item)) in f.results.iter().enumerate().skip(f.scroll).take(f.rows).enumerate() {
                let top = LIST_TOP + row as f32 * ROW_H;
                let mid = top + ROW_H / 2.0;
                let selected = index == f.selected;

                if selected {
                    brush.SetColor(&p.row_selected);
                    rt.FillRoundedRectangle(
                        &rounded(rect(PAD, top + 1.0, width - PAD, top + ROW_H - 1.0), 6.0),
                        brush,
                    );
                    rt.FillRoundedRectangle(&rounded(rect(PAD, mid - 9.0, PAD + 3.0, mid + 9.0), 1.5), accent);
                }

                let icon_rect = rect(ICON_X, mid - ICON / 2.0, ICON_X + ICON, mid + ICON / 2.0);
                let icon = match (&item.kind, &item.action) {
                    (Kind::App, crate::search::Action::Launch { id, .. }) => {
                        f.icons.get(id).and_then(|px| bitmap(&mut t.bitmaps, rt, id, px))
                    }
                    _ => None,
                };
                if let Some(icon) = icon {
                    rt.DrawBitmap(
                        &icon,
                        Some(&icon_rect),
                        1.0,
                        D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                        None,
                    );
                } else {
                    brush.SetColor(&p.tile);
                    rt.FillRoundedRectangle(&rounded(icon_rect, 7.0), brush);
                    match item.kind {
                        Kind::App => {
                            let letter = item
                                .title
                                .chars()
                                .next()
                                .map(|c| c.to_uppercase().to_string())
                                .unwrap_or_default();
                            brush.SetColor(&p.subtext);
                            draw_text(rt, &letter, &fm.letter, icon_rect, brush);
                        }
                        kind => {
                            let glyph = match kind {
                                Kind::Calc => "\u{E8EF}",
                                Kind::Url => "\u{E71B}",
                                Kind::Shell => "\u{E756}",
                                _ => "\u{E774}",
                            };
                            draw_text(rt, glyph, &fm.glyph, icon_rect, accent);
                        }
                    }
                }

                let text_right = width - PAD - HINT_W;
                let title = dw.CreateTextLayout(&wide(&item.title), &fm.title, text_right - TEXT_X, 20.0)?;
                for &(start, length) in &item.highlights {
                    let range = DWRITE_TEXT_RANGE {
                        startPosition: start,
                        length,
                    };
                    title.SetDrawingEffect(&accent.cast::<windows::core::IUnknown>()?, range)?;
                    title.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, range)?;
                }
                let title_top = if item.subtitle.is_empty() {
                    mid - 10.0
                } else {
                    mid - 19.0
                };
                brush.SetColor(&p.text);
                rt.DrawTextLayout(
                    Vector2 {
                        X: TEXT_X,
                        Y: title_top,
                    },
                    &title,
                    brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
                if !item.subtitle.is_empty() {
                    brush.SetColor(&p.subtext);
                    draw_text(
                        rt,
                        &item.subtitle,
                        &fm.subtitle,
                        rect(TEXT_X, mid + 1.0, text_right, mid + 18.0),
                        brush,
                    );
                }

                if selected {
                    brush.SetColor(&p.subtext);
                    let hint = format!("{} \u{21B5}", item.kind.verb());
                    draw_text(
                        rt,
                        &hint,
                        &fm.hint,
                        rect(width - PAD - HINT_W, top, width - PAD - 14.0, top + ROW_H),
                        brush,
                    );
                }
            }

            if f.results.len() > f.rows {
                let track_top = LIST_TOP;
                let track_h = f.rows as f32 * ROW_H;
                let thumb_h = (track_h * f.rows as f32 / f.results.len() as f32).max(24.0);
                let max_scroll = (f.results.len() - f.rows) as f32;
                let thumb_top = track_top + (track_h - thumb_h) * (f.scroll as f32 / max_scroll);
                brush.SetColor(&p.scrollbar);
                rt.FillRoundedRectangle(
                    &rounded(rect(width - 5.0, thumb_top, width - 2.5, thumb_top + thumb_h), 1.25),
                    brush,
                );
            }
        }

        Ok(())
    }
}

/// Uploads an icon to the GPU once and caches it for this render target.
unsafe fn bitmap(
    cache: &mut HashMap<String, ID2D1Bitmap>,
    rt: &ID2D1RenderTarget,
    key: &str,
    px: &IconPixels,
) -> Option<ID2D1Bitmap> {
    if let Some(bitmap) = cache.get(key) {
        return Some(bitmap.clone());
    }
    let props = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    let bitmap = unsafe {
        rt.CreateBitmap(
            D2D_SIZE_U {
                width: px.width,
                height: px.height,
            },
            Some(px.bgra.as_ptr().cast()),
            px.width * 4,
            &props,
        )
        .ok()?
    };
    cache.insert(key.to_string(), bitmap.clone());
    Some(bitmap)
}
