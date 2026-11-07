//! Draws with Direct2D and DirectWrite in device-independent pixels (DIPs).
//! The render target's DPI controls scaling.

use std::collections::HashMap;
use std::sync::Arc;

use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::{Interface, PCWSTR, Result, w};
use windows_numerics::{Matrix3x2, Vector2};

use crate::indexer::IconPixels;
use crate::search::{Kind, ResultItem};
use crate::surface::nav::{self, Rect};
use crate::surface::{Input, Widget};
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

    /// Where the query box starts in a window `window_h` DIPs tall. At the
    /// bottom of the screen the box is the window's last row.
    pub fn input_top(window_h: f32, bottom: bool) -> f32 {
        if bottom { window_h - INPUT_H } else { 0.0 }
    }

    /// The top of visible row `row`. At the bottom of the screen, results
    /// stack upward so the best match sits next to the query box.
    pub fn row_top(row: usize, window_h: f32, bottom: bool) -> f32 {
        if bottom {
            window_h - LIST_TOP - (row + 1) as f32 * ROW_H
        } else {
            LIST_TOP + row as f32 * ROW_H
        }
    }

    /// The visible row at window DIP `y`, if any of `rows` rows is there.
    pub fn row_at(y: f32, rows: usize, window_h: f32, bottom: bool) -> Option<usize> {
        let offset = if bottom { window_h - LIST_TOP - y } else { y - LIST_TOP };
        if offset < 0.0 {
            return None;
        }
        let row = (offset / ROW_H) as usize;
        (row < rows).then_some(row)
    }

    /// Where a surface's widgets start. At the bottom of the screen the
    /// surface fills the window above the query box.
    pub fn surface_top(bottom: bool) -> f32 {
        if bottom { LIST_PAD } else { LIST_TOP }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn bottom_rows_stack_upward_from_the_query_box() {
            let h = height(3);
            assert_eq!(input_top(h, true), h - INPUT_H);
            // The first result sits directly above the divider and padding.
            assert_eq!(row_top(0, h, true) + ROW_H, h - LIST_TOP);
            assert_eq!(row_top(2, h, true), LIST_PAD);
            assert_eq!(row_top(1, h, false), LIST_TOP + ROW_H);
        }

        #[test]
        fn row_hit_testing_matches_drawing() {
            for bottom in [false, true] {
                let h = height(4);
                for row in 0..4 {
                    let mid = row_top(row, h, bottom) + ROW_H / 2.0;
                    assert_eq!(row_at(mid, 4, h, bottom), Some(row));
                }
                assert_eq!(row_at(input_top(h, bottom) + 1.0, 4, h, bottom), None);
                // Fewer results than rows leave empty space away from the box.
                assert_eq!(row_at(row_top(3, h, bottom) + 1.0, 2, h, bottom), None);
            }
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
    /// The open list's name, shown before the query, as in "Wi-Fi ›".
    pub chip: Option<&'a str>,
    pub results: &'a [ResultItem],
    pub selected: usize,
    pub scroll: usize,
    pub rows: usize,
    pub icons: &'a HashMap<String, Arc<IconPixels>>,
    /// Drawn instead of the results while a surface is open.
    pub surface: Option<SurfaceFrame<'a>>,
    /// The query box is at the bottom and results stack upward.
    pub bottom: bool,
    pub opacity: f32,
    pub dock_opacity: f32,
    pub shared: bool,
    pub radius: f32,
    pub content_size: Option<(f32, f32)>,
}

pub struct SurfaceFrame<'a> {
    pub widgets: &'a [Widget],
    /// From `nav::layout`, relative to the surface's top-left corner.
    pub rects: &'a [Rect],
    pub focus: usize,
    /// The cursor inside the focused widget, such as an output device.
    pub stop: usize,
    pub scroll: f32,
    pub rows: usize,
}

/// Where the surface's widgets start, in window DIPs.
pub fn surface_origin(bottom: bool) -> (f32, f32) {
    (PAD, surface_top(bottom))
}

pub fn surface_width() -> f32 {
    WIDTH - 2.0 * PAD
}

struct Formats {
    query: IDWriteTextFormat,
    title: IDWriteTextFormat,
    subtitle: IDWriteTextFormat,
    hint: IDWriteTextFormat,
    glyph: IDWriteTextFormat,
    search_glyph: IDWriteTextFormat,
    letter: IDWriteTextFormat,
    value: IDWriteTextFormat,
    small: IDWriteTextFormat,
}

// Layouts survive animation geometry changes; keys include every shaping input.
struct CachedLayout {
    text: String,
    width: f32,
    layout: IDWriteTextLayout,
}

struct CachedTitle {
    text: String,
    width: f32,
    highlights: Vec<(u32, u32)>,
    layout: IDWriteTextLayout,
}

fn cached_layout(
    cached: &mut Option<CachedLayout>,
    dw: &IDWriteFactory,
    format: &IDWriteTextFormat,
    text: &str,
    width: f32,
    height: f32,
) -> Result<IDWriteTextLayout> {
    if cached.as_ref().is_none_or(|c| c.text != text || c.width != width) {
        let layout = unsafe { dw.CreateTextLayout(&wide(text), format, width, height)? };
        *cached = Some(CachedLayout {
            text: text.into(),
            width,
            layout,
        });
    }
    Ok(cached.as_ref().unwrap().layout.clone())
}

struct Target {
    rt: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    bitmaps: HashMap<String, ID2D1Bitmap>,
    query_layout: Option<CachedLayout>,
    chip_layout: Option<CachedLayout>,
    titles: Vec<Option<CachedTitle>>,
}

pub struct Renderer {
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    formats: Formats,
    target: Option<Target>,
    dpi: f32,
    input_scroll: f32,
    /// Where the last frame drew each widget's stops, for mouse clicks.
    stops: Vec<(usize, usize, Rect)>,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fonts = Fonts::installed(&dwrite);
            let (ui, icons) = (fonts.text, fonts.icons);
            let formats = Formats {
                query: text_format(&dwrite, fonts.display, 20.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                title: ellipsized(&dwrite, text_format(&dwrite, ui, 14.5, DWRITE_FONT_WEIGHT_NORMAL)?)?,
                subtitle: ellipsized(&dwrite, text_format(&dwrite, ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?)?,
                hint: text_format(&dwrite, ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                glyph: text_format(&dwrite, icons, 16.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                search_glyph: text_format(&dwrite, icons, 17.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                letter: text_format(&dwrite, ui, 15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                value: text_format(&dwrite, fonts.display, 22.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                small: ellipsized(&dwrite, text_format(&dwrite, ui, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?)?,
            };
            formats.small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
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
                stops: Vec::new(),
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
                let size = t.rt.GetPixelSize();
                if size.width != width || size.height != height {
                    let _ = t.rt.Resize(&D2D_SIZE_U { width, height });
                }
            }
        }
    }

    pub fn draw(
        &mut self,
        hwnd: HWND,
        frame: &Frame,
        dock: impl FnOnce(&ID2D1HwndRenderTarget) -> Result<()>,
    ) -> Result<()> {
        if self.target.is_none() {
            self.target = Some(self.create_target(hwnd)?);
        }
        let target = self.target.as_mut().unwrap();
        // Direct2D requires EndDraw after BeginDraw even if drawing fails.
        let rt = target.rt.clone();
        let result = unsafe {
            rt.BeginDraw();
            self.stops.clear();
            if frame.shared {
                crate::theme::paint_background(&rt, &target.brush, frame.palette, frame.radius);
            } else {
                rt.Clear(Some(&frame.palette.background));
            }
            let size = rt.GetSize();
            let (width, height) = frame.content_size.unwrap_or((size.width, size.height));
            let (x, y) = crate::morph::content_offset(size.width, size.height, width, height);
            let layer = |opacity| D2D1_LAYER_PARAMETERS {
                contentBounds: rect(-10000., -10000., 10000., 10000.),
                opacity,
                maskTransform: Matrix3x2::identity(),
                ..Default::default()
            };
            if frame.dock_opacity > 0.0 && frame.dock_opacity < 1.0 {
                rt.PushLayer(&layer(frame.dock_opacity), None);
            }
            let dock_result = if frame.dock_opacity > 0.0 { dock(&rt) } else { Ok(()) };
            if frame.dock_opacity > 0.0 && frame.dock_opacity < 1.0 {
                rt.PopLayer();
            }
            if frame.opacity > 0.0 && frame.opacity < 1.0 {
                rt.PushLayer(&layer(frame.opacity), None);
            }
            rt.SetTransform(&Matrix3x2::translation(x, y));
            let drawn = if frame.opacity > 0.0 {
                draw_frame(&self.dwrite, &self.formats, target, &mut self.input_scroll, frame).and_then(|()| {
                    match &frame.surface {
                        Some(surface) => {
                            draw_surface(&self.dwrite, &self.formats, target, frame, surface, &mut self.stops)
                        }
                        None => Ok(()),
                    }
                })
            } else {
                Ok(())
            };
            rt.SetTransform(&Matrix3x2::identity());
            if frame.opacity > 0.0 && frame.opacity < 1.0 {
                rt.PopLayer();
            }
            let ended = {
                let _span = crate::util::perf::Span::new("present");
                rt.EndDraw(None, None)
            };
            if ended.is_ok() && drawn.is_ok() && frame.opacity > 0.0 {
                crate::util::perf::present();
            }
            dock_result.and(drawn).and(ended)
        };
        if let Err(e) = &result
            && e.code() == D2DERR_RECREATE_TARGET
        {
            self.target = None;
            return Ok(());
        }
        result
    }

    /// The widget and stop drawn at window DIP `(x, y)` in the last frame.
    pub fn stop_at(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        self.stops
            .iter()
            .find(|(_, _, r)| r.contains(x, y))
            .map(|&(widget, stop, _)| (widget, stop))
    }

    fn create_target(&self, hwnd: HWND) -> Result<Target> {
        unsafe {
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                // At this window size, software rendering takes about 1 ms per frame.
                // Avoiding the GPU driver saves about 30 MB of private memory.
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
            // Use grayscale text antialiasing because ClearType needs an opaque background.
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
                query_layout: None,
                chip_layout: None,
                titles: Vec::new(),
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

/// The font families Wisp draws with. Windows 10 lacks the Windows 11 fonts,
/// and without an icon font every glyph draws as a box, so each falls back to
/// its Windows 10 counterpart.
#[derive(Clone, Copy)]
pub struct Fonts {
    pub text: PCWSTR,
    pub display: PCWSTR,
    pub icons: PCWSTR,
}

impl Fonts {
    pub fn installed(dw: &IDWriteFactory) -> Fonts {
        let mut collection = None;
        let collection = unsafe { dw.GetSystemFontCollection(&mut collection, false) }
            .ok()
            .and(collection);
        let pick = |preferred: PCWSTR, fallback: PCWSTR| {
            let Some(collection) = &collection else {
                return preferred;
            };
            let (mut index, mut exists) = (0, false.into());
            match unsafe { collection.FindFamilyName(preferred, &mut index, &mut exists) } {
                Ok(()) if !exists.as_bool() => fallback,
                _ => preferred,
            }
        };
        Fonts {
            text: pick(w!("Segoe UI Variable Text"), w!("Segoe UI")),
            display: pick(w!("Segoe UI Variable Display"), w!("Segoe UI")),
            icons: pick(w!("Segoe Fluent Icons"), w!("Segoe MDL2 Assets")),
        }
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
        let rt: &ID2D1RenderTarget = &t.rt;
        let p = f.palette;
        let width = f.content_size.map_or(rt.GetSize().width, |s| s.0);
        let brush = &t.brush;
        let accent = &t.accent;
        accent.SetColor(&p.accent);

        // The query box is drawn from y = 0 and moved to where it sits.
        let window_h = f.content_size.map_or(rt.GetSize().height, |s| s.1);
        let (ox, oy) = crate::morph::content_offset(rt.GetSize().width, rt.GetSize().height, width, window_h);
        let input_y = input_top(window_h, f.bottom);
        rt.SetTransform(&Matrix3x2::translation(ox, oy + input_y));
        let mid = INPUT_H / 2.0;
        brush.SetColor(&p.placeholder);
        draw_text(rt, "\u{E721}", &fm.search_glyph, rect(16.0, 0.0, 44.0, INPUT_H), brush);

        let mut text_x = INPUT_TEXT_X;
        if let Some(chip) = f.chip {
            let label = format!("{chip} \u{203A}");
            let layout = cached_layout(&mut t.chip_layout, dw, &fm.hint, &label, 240.0, INPUT_H)?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics)?;
            let chip_w = metrics.width + 16.0;
            brush.SetColor(&p.tile);
            rt.FillRoundedRectangle(
                &rounded(rect(text_x, mid - 12.0, text_x + chip_w, mid + 12.0), 5.0),
                brush,
            );
            brush.SetColor(&p.text);
            draw_text(
                rt,
                &label,
                &fm.hint,
                rect(text_x + 8.0, mid - 12.0, text_x + chip_w - 8.0, mid + 12.0),
                brush,
            );
            text_x += chip_w + 10.0;
        }
        let text_right = width - 20.0;
        let box_w = text_right - text_x;
        let mut caret_at = text_x;
        if f.text.is_empty() {
            *input_scroll = 0.0;
            draw_text(
                rt,
                f.placeholder,
                &fm.query,
                rect(text_x, 0.0, text_right, INPUT_H),
                brush,
            );
        } else {
            let layout = cached_layout(&mut t.query_layout, dw, &fm.query, f.text, 100_000.0, INPUT_H)?;
            let caret = caret_x(&layout, f.caret);
            // Keep the caret in view when the text is wider than the box.
            if caret - *input_scroll > box_w - 2.0 {
                *input_scroll = caret - box_w + 2.0;
            } else if caret < *input_scroll {
                *input_scroll = caret;
            }
            let origin = text_x - *input_scroll;
            rt.PushAxisAlignedClip(
                &rect(text_x - 2.0, 0.0, text_right + 2.0, INPUT_H),
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
        rt.SetTransform(&Matrix3x2::translation(ox, oy));

        let divider = if f.bottom { input_y - 1.0 } else { INPUT_H };
        if f.surface.is_some() {
            brush.SetColor(&p.divider);
            rt.FillRectangle(&rect(0.0, divider, width, divider + 1.0), brush);
        } else if f.rows > 0 {
            brush.SetColor(&p.divider);
            rt.FillRectangle(&rect(0.0, divider, width, divider + 1.0), brush);

            for (row, (index, item)) in f.results.iter().enumerate().skip(f.scroll).take(f.rows).enumerate() {
                let top = row_top(row, window_h, f.bottom);
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
                let icon = item
                    .icon
                    .as_ref()
                    .and_then(|key| f.icons.get(key).and_then(|px| bitmap(&mut t.bitmaps, rt, key, px)));
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
                        Kind::App if item.glyph.is_none() => {
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
                            let glyph = item.glyph.unwrap_or(match kind {
                                Kind::File => "\u{E8A5}",
                                Kind::Folder => "\u{E8B7}",
                                Kind::Calc => "\u{E8EF}",
                                Kind::Url => "\u{E71B}",
                                Kind::Shell => "\u{E756}",
                                _ => "\u{E774}",
                            });
                            draw_text(rt, glyph, &fm.glyph, icon_rect, accent);
                        }
                    }
                }

                let text_right = width - PAD - HINT_W;
                if t.titles.len() < f.rows {
                    t.titles.resize_with(f.rows, || None);
                }
                let title_width = text_right - TEXT_X;
                if t.titles[row]
                    .as_ref()
                    .is_none_or(|c| c.text != item.title || c.width != title_width || c.highlights != item.highlights)
                {
                    let title = dw.CreateTextLayout(&wide(&item.title), &fm.title, title_width, 20.0)?;
                    for &(start, length) in &item.highlights {
                        let range = DWRITE_TEXT_RANGE {
                            startPosition: start,
                            length,
                        };
                        title.SetDrawingEffect(&accent.cast::<windows::core::IUnknown>()?, range)?;
                        title.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, range)?;
                    }
                    t.titles[row] = Some(CachedTitle {
                        text: item.title.clone(),
                        width: title_width,
                        highlights: item.highlights.clone(),
                        layout: title,
                    });
                }
                let title = &t.titles[row].as_ref().unwrap().layout;
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
                    title,
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

                if selected && !item.enter_label().is_empty() {
                    brush.SetColor(&p.subtext);
                    let hint = format!("{} \u{21B5}", item.enter_label());
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
                // Rows run upward at the bottom, so the track does too.
                let track_top = row_top(if f.bottom { f.rows - 1 } else { 0 }, window_h, f.bottom);
                let track_h = f.rows as f32 * ROW_H;
                let thumb_h = (track_h * f.rows as f32 / f.results.len() as f32).max(24.0);
                let max_scroll = (f.results.len() - f.rows) as f32;
                let progress = f.scroll as f32 / max_scroll;
                let progress = if f.bottom { 1.0 - progress } else { progress };
                let thumb_top = track_top + (track_h - thumb_h) * progress;
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

fn d2d(r: &Rect, dx: f32, dy: f32) -> D2D_RECT_F {
    rect(r.x + dx, r.y + dy, r.x + r.w + dx, r.bottom() + dy)
}

/// Draws a surface's widgets as tiles below the query box.
unsafe fn draw_surface(
    _dw: &IDWriteFactory,
    fm: &Formats,
    t: &mut Target,
    f: &Frame,
    s: &SurfaceFrame,
    stops: &mut Vec<(usize, usize, Rect)>,
) -> Result<()> {
    unsafe {
        let rt: &ID2D1RenderTarget = &t.rt;
        let p = f.palette;
        let brush = &t.brush;
        let accent = &t.accent;
        let (ox, oy) = surface_origin(f.bottom);
        let view_h = s.rows as f32 * ROW_H;
        let dy = oy - s.scroll;
        rt.PushAxisAlignedClip(
            &rect(0.0, oy - 2.0, WIDTH, oy + view_h + 2.0),
            D2D1_ANTIALIAS_MODE_ALIASED,
        );
        if s.widgets.is_empty() {
            brush.SetColor(&p.subtext);
            draw_text(
                rt,
                "Nothing matches",
                &fm.small,
                rect(ox, oy, ox + surface_width(), oy + 48.0),
                brush,
            );
        }
        for (index, (widget, r)) in s.widgets.iter().zip(s.rects).enumerate() {
            if r.bottom() + dy < oy || r.y + dy > oy + view_h {
                continue;
            }
            let focused = index == s.focus;
            let tile = d2d(r, ox, dy);
            let (left, top, right) = (tile.left, tile.top, tile.right);
            brush.SetColor(if focused { &p.row_selected } else { &p.tile });
            rt.FillRoundedRectangle(&rounded(tile, 8.0), brush);
            if focused {
                rt.DrawRoundedRectangle(
                    &rounded(rect(left + 0.75, top + 0.75, right - 0.75, tile.bottom - 0.75), 7.5),
                    accent,
                    1.5,
                    None,
                );
            }

            // The icon or glyph, then the label and detail.
            let icon_rect = rect(left + 12.0, top + 12.0, left + 36.0, top + 36.0);
            let icon = widget
                .icon
                .as_ref()
                .and_then(|key| f.icons.get(key).and_then(|px| bitmap(&mut t.bitmaps, rt, key, px)));
            let active = match &widget.input {
                Input::Toggle { on, .. } => *on,
                Input::Slider { muted, .. } => !muted,
                _ => true,
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
                brush.SetColor(&p.subtext);
                draw_text(
                    rt,
                    widget.glyph,
                    &fm.glyph,
                    icon_rect,
                    if active { accent } else { brush },
                );
            }
            let text_x = left + 48.0;

            match &widget.input {
                Input::Info { value } => {
                    brush.SetColor(&p.subtext);
                    let label = if widget.detail.is_empty() {
                        widget.label.clone()
                    } else {
                        format!("{} · {}", widget.label, widget.detail)
                    };
                    draw_text(
                        rt,
                        &label,
                        &fm.subtitle,
                        rect(text_x, top + 10.0, right - 12.0, top + 28.0),
                        brush,
                    );
                    brush.SetColor(&p.text);
                    draw_text(
                        rt,
                        value,
                        &fm.value,
                        rect(text_x, top + 28.0, right - 12.0, top + 60.0),
                        brush,
                    );
                }
                Input::Toggle { on, .. } => {
                    let switch_right = right - 14.0;
                    brush.SetColor(&p.text);
                    draw_text(
                        rt,
                        &widget.label,
                        &fm.title,
                        rect(text_x, top + 13.0, switch_right - 46.0, top + 33.0),
                        brush,
                    );
                    brush.SetColor(&p.subtext);
                    draw_text(
                        rt,
                        &widget.detail,
                        &fm.subtitle,
                        rect(text_x, top + 35.0, switch_right - 46.0, top + 53.0),
                        brush,
                    );
                    let mid = top + r.h / 2.0;
                    let pill = rect(switch_right - 36.0, mid - 9.0, switch_right, mid + 9.0);
                    if *on {
                        rt.FillRoundedRectangle(&rounded(pill, 9.0), accent);
                    } else {
                        brush.SetColor(&p.scrollbar);
                        rt.FillRoundedRectangle(&rounded(pill, 9.0), brush);
                    }
                    let knob_x = if *on { pill.right - 9.0 } else { pill.left + 9.0 };
                    brush.SetColor(if *on { &p.background_solid } else { &p.text });
                    rt.FillEllipse(
                        &D2D1_ELLIPSE {
                            point: Vector2 { X: knob_x, Y: mid },
                            radiusX: 6.0,
                            radiusY: 6.0,
                        },
                        brush,
                    );
                }
                Input::Slider { value, muted, .. } => {
                    let label = if *muted {
                        format!("{} · Muted", widget.label)
                    } else if widget.detail.is_empty() {
                        widget.label.clone()
                    } else {
                        format!("{} · {}", widget.label, widget.detail)
                    };
                    brush.SetColor(&p.text);
                    draw_text(
                        rt,
                        &label,
                        &fm.title,
                        rect(text_x, top + 8.0, right - 64.0, top + 28.0),
                        brush,
                    );
                    brush.SetColor(&p.subtext);
                    draw_text(
                        rt,
                        &format!("{value}%"),
                        &fm.hint,
                        rect(right - 64.0, top + 8.0, right - 16.0, top + 28.0),
                        brush,
                    );
                    let (x0, x1) = nav::slider_track(r);
                    let (x0, x1) = (x0 + ox, x1 + ox);
                    let y = top + 40.0;
                    brush.SetColor(&p.scrollbar);
                    rt.FillRoundedRectangle(&rounded(rect(x0, y - 2.0, x1, y + 2.0), 2.0), brush);
                    let at = x0 + (x1 - x0) * (*value).min(100) as f32 / 100.0;
                    if *muted {
                        brush.SetColor(&p.subtext);
                        rt.FillRoundedRectangle(&rounded(rect(x0, y - 2.0, at, y + 2.0), 2.0), brush);
                    } else {
                        rt.FillRoundedRectangle(&rounded(rect(x0, y - 2.0, at, y + 2.0), 2.0), accent);
                    }
                    brush.SetColor(&p.text);
                    rt.FillEllipse(
                        &D2D1_ELLIPSE {
                            point: Vector2 { X: at, Y: y },
                            radiusX: if focused { 7.0 } else { 6.0 },
                            radiusY: if focused { 7.0 } else { 6.0 },
                        },
                        brush,
                    );
                }
                Input::Choice { options, selected } => {
                    brush.SetColor(&p.text);
                    draw_text(
                        rt,
                        &widget.label,
                        &fm.title,
                        rect(text_x, top + 14.0, right - 12.0, top + 34.0),
                        brush,
                    );
                    let names: Vec<&str> = options.iter().map(|(name, _)| name.as_str()).collect();
                    for (stop, (chip, name)) in nav::chip_rects(r, &names).iter().zip(&names).enumerate() {
                        let c = d2d(chip, ox, dy);
                        if focused && stop == s.stop {
                            brush.SetColor(&p.scrollbar);
                            rt.FillRoundedRectangle(&rounded(c, 6.0), brush);
                        }
                        let border = if stop == *selected { accent } else { brush };
                        brush.SetColor(&p.divider);
                        rt.DrawRoundedRectangle(
                            &rounded(rect(c.left + 0.5, c.top + 0.5, c.right - 0.5, c.bottom - 0.5), 6.0),
                            border,
                            1.0,
                            None,
                        );
                        let text = rect(c.left + 8.0, c.top, c.right - 8.0, c.bottom);
                        if stop == *selected {
                            draw_text(rt, name, &fm.small, text, accent);
                        } else {
                            brush.SetColor(&p.text);
                            draw_text(rt, name, &fm.small, text, brush);
                        }
                        stops.push((
                            index,
                            stop,
                            Rect {
                                x: c.left,
                                y: c.top,
                                w: chip.w,
                                h: chip.h,
                            },
                        ));
                    }
                }
                Input::Buttons { buttons } => {
                    let buttons_w = buttons.len() as f32 * 40.0;
                    let text_right = right - 16.0 - buttons_w;
                    brush.SetColor(&p.text);
                    draw_text(
                        rt,
                        &widget.label,
                        &fm.title,
                        rect(text_x, top + 12.0, text_right, top + 32.0),
                        brush,
                    );
                    brush.SetColor(&p.subtext);
                    draw_text(
                        rt,
                        &widget.detail,
                        &fm.subtitle,
                        rect(text_x, top + 33.0, text_right, top + 51.0),
                        brush,
                    );
                    let mid = top + r.h / 2.0;
                    for (stop, (glyph, _)) in buttons.iter().enumerate() {
                        let x = text_right + 8.0 + stop as f32 * 40.0;
                        let button = rect(x, mid - 16.0, x + 32.0, mid + 16.0);
                        if focused && stop == s.stop {
                            brush.SetColor(&p.scrollbar);
                            rt.FillEllipse(
                                &D2D1_ELLIPSE {
                                    point: Vector2 { X: x + 16.0, Y: mid },
                                    radiusX: 16.0,
                                    radiusY: 16.0,
                                },
                                brush,
                            );
                        }
                        brush.SetColor(&p.text);
                        draw_text(rt, glyph, &fm.glyph, button, brush);
                        stops.push((
                            index,
                            stop,
                            Rect {
                                x,
                                y: mid - 16.0,
                                w: 32.0,
                                h: 32.0,
                            },
                        ));
                    }
                }
                Input::Link { .. } => {
                    brush.SetColor(&p.text);
                    draw_text(
                        rt,
                        &widget.label,
                        &fm.title,
                        rect(text_x, top + 13.0, right - 12.0, top + 33.0),
                        brush,
                    );
                    brush.SetColor(&p.subtext);
                    draw_text(
                        rt,
                        &widget.detail,
                        &fm.subtitle,
                        rect(text_x, top + 35.0, right - 12.0, top + 53.0),
                        brush,
                    );
                }
            }
        }
        rt.PopAxisAlignedClip();

        let content = nav::content_height(s.rects);
        if content > view_h {
            let thumb_h = (view_h * view_h / content).max(24.0);
            let thumb_top = oy + (view_h - thumb_h) * (s.scroll / (content - view_h));
            brush.SetColor(&p.scrollbar);
            let x = WIDTH - 5.0;
            rt.FillRoundedRectangle(&rounded(rect(x, thumb_top, x + 2.5, thumb_top + thumb_h), 1.25), brush);
        }
        Ok(())
    }
}

/// Caches Direct2D bitmaps per render target; a new target needs new bitmaps.
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
