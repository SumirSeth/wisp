//! Drawing for the fixed quick-settings grid. Text layouts live with the target.
use super::*;
use crate::surface::quick::Slot;
type TextCache = HashMap<(String, u8, u32, u32), IDWriteTextLayout>;

#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn draw(
    dw: &IDWriteFactory,
    fm: &Formats,
    rt: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    accent: &ID2D1SolidColorBrush,
    cache: &mut TextCache,
    bitmaps: &mut HashMap<String, ID2D1Bitmap>,
    f: &Frame,
    s: &SurfaceFrame,
    index: usize,
    widget: &Widget,
    r: &Rect,
    origin: (f32, f32),
    stops: &mut Vec<(usize, usize, Rect)>,
) -> Result<()> {
    unsafe {
        let slot = widget.quick.unwrap();
        let p = f.palette;
        let tile = d2d(r, origin.0, origin.1);
        let (l, top, right, bottom) = (tile.left, tile.top, tile.right, tile.bottom);
        let focused = index == s.focus;
        let header = matches!(slot, Slot::Clock | Slot::Battery | Slot::Hint);
        if !header {
            brush.SetColor(if focused { &p.row_selected } else { &p.tile });
            rt.FillRoundedRectangle(&rounded(tile, 12.0), brush);
        }
        if focused && !header {
            rt.DrawRoundedRectangle(
                &rounded(rect(l + 0.75, top + 0.75, right - 0.75, bottom - 0.75), 11.0),
                accent,
                1.5,
                None,
            );
        }
        if cache.len() > 256 {
            cache.clear();
        }
        let mut text = |value: &str, font: u8, bounds: D2D_RECT_F, color: &D2D1_COLOR_F| -> Result<()> {
            let key = (
                value.to_string(),
                font,
                (bounds.right - bounds.left).to_bits(),
                (bounds.bottom - bounds.top).to_bits(),
            );
            if !cache.contains_key(&key) {
                let format = match font {
                    1 => &fm.subtitle,
                    2 => &fm.glyph,
                    3 => &fm.value,
                    4 => &fm.small,
                    5 => &fm.hint,
                    _ => &fm.title,
                };
                cache.insert(
                    key.clone(),
                    dw.CreateTextLayout(
                        &wide(value),
                        format,
                        bounds.right - bounds.left,
                        bounds.bottom - bounds.top,
                    )?,
                );
            }
            brush.SetColor(color);
            rt.DrawTextLayout(
                Vector2 {
                    X: bounds.left,
                    Y: bounds.top,
                },
                &cache[&key],
                brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
            Ok(())
        };
        match slot {
            Slot::Hint => {
                let label = s
                    .widgets
                    .get(s.focus)
                    .filter(|w| matches!(w.quick, Some(Slot::Footer(_))))
                    .map_or(widget.label.as_str(), |w| {
                        if widget.detail.is_empty() {
                            w.label.as_str()
                        } else {
                            widget.label.as_str()
                        }
                    });
                text(label, 1, tile, &p.subtext)?;
            }
            Slot::Clock => {
                if let Input::Info { value } = &widget.input {
                    text(value, 3, rect(l + 4.0, top, l + 100.0, bottom), &p.text)?;
                    text(&widget.detail, 1, rect(l + 108.0, top + 4.0, right, bottom), &p.subtext)?;
                }
            }
            Slot::Battery => {
                if let Input::Info { value } = &widget.input {
                    text(value, 5, rect(l, top, right, bottom), &p.subtext)?;
                }
            }
            Slot::RadioList(_) | Slot::Footer(_) => {
                text(widget.glyph, 2, rect(l + 8.0, top, right - 8.0, bottom), &p.subtext)?;
            }
            Slot::Output => {
                text(&widget.label, 1, rect(l + 10.0, top, right - 28.0, bottom), &p.subtext)?;
                text(
                    widget.glyph,
                    2,
                    rect(right - 24.0, top, right - 8.0, bottom),
                    &p.subtext,
                )?;
            }
            Slot::Volume => {
                text(widget.glyph, 2, rect(l + 12.0, top, l + 36.0, bottom), &p.subtext)?;
                if let Input::Slider { value, .. } = widget.input {
                    text(
                        &format!("{value}%"),
                        1,
                        rect(right - 48.0, top, right - 8.0, bottom),
                        &p.text,
                    )?;
                    let (x0, x1) = nav::slider_track(r, true);
                    let (x0, x1) = (x0 + origin.0, x1 + origin.0);
                    let mid = (top + bottom) / 2.0;
                    let pos = x0 + (x1 - x0) * value as f32 / 100.0;
                    brush.SetColor(&p.scrollbar);
                    rt.FillRoundedRectangle(&rounded(rect(x0, mid - 3.0, x1, mid + 3.0), 3.0), brush);
                    rt.FillRoundedRectangle(&rounded(rect(x0, mid - 3.0, pos, mid + 3.0), 3.0), accent);
                    brush.SetColor(&p.text);
                    rt.FillEllipse(
                        &D2D1_ELLIPSE {
                            point: Vector2 { X: pos, Y: mid },
                            radiusX: 7.0,
                            radiusY: 7.0,
                        },
                        brush,
                    );
                }
            }
            Slot::Media => {
                if let Some(icon) = widget
                    .icon
                    .as_ref()
                    .and_then(|key| f.icons.get(key).and_then(|px| bitmap(bitmaps, rt, key, px)))
                {
                    rt.DrawBitmap(
                        &icon,
                        Some(&rect(l + 12.0, top + 10.0, l + 50.0, top + 48.0)),
                        1.0,
                        D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                        None,
                    );
                } else {
                    text(
                        widget.glyph,
                        2,
                        rect(l + 14.0, top + 12.0, l + 48.0, top + 48.0),
                        &p.subtext,
                    )?;
                }
                text(
                    &widget.label,
                    0,
                    rect(l + 58.0, top + 10.0, right - 12.0, top + 32.0),
                    &p.text,
                )?;
                text(
                    &widget.detail,
                    1,
                    rect(l + 58.0, top + 32.0, right - 12.0, top + 52.0),
                    &p.subtext,
                )?;
                if let Input::Buttons { buttons } = &widget.input {
                    for (stop, (glyph, _)) in buttons.iter().enumerate() {
                        let x = l + (r.w - 120.0) / 2.0 + stop as f32 * 40.0;
                        let y = bottom - 36.0;
                        if focused && stop == s.stop {
                            brush.SetColor(&p.row_selected);
                            rt.FillRoundedRectangle(&rounded(rect(x, y, x + 32.0, y + 30.0), 6.0), brush);
                            rt.DrawRoundedRectangle(&rounded(rect(x, y, x + 32.0, y + 30.0), 6.0), accent, 1.0, None);
                        }
                        text(glyph, 2, rect(x + 8.0, y, x + 24.0, y + 30.0), &p.text)?;
                        stops.push((index, stop, Rect { x, y, w: 32.0, h: 30.0 }));
                    }
                }
            }
            Slot::Radio(_) | Slot::Small(_) => {
                let small = matches!(slot, Slot::Small(_));
                let on = matches!(widget.input, Input::Toggle { on: true, .. });
                let (cx, cy) = if small {
                    ((l + right) / 2.0, top + 26.0)
                } else {
                    (l + 28.0, top + r.h / 2.0)
                };
                brush.SetColor(&p.scrollbar);
                rt.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: Vector2 { X: cx, Y: cy },
                        radiusX: 17.0,
                        radiusY: 17.0,
                    },
                    if on { accent } else { brush },
                );
                text(
                    widget.glyph,
                    2,
                    rect(cx - 12.0, cy - 12.0, cx + 12.0, cy + 12.0),
                    if on { &p.background_solid } else { &p.subtext },
                )?;
                if small {
                    text(
                        &widget.label,
                        4,
                        rect(l + 8.0, top + 48.0, right - 8.0, bottom - 4.0),
                        &p.text,
                    )?;
                } else {
                    text(
                        &widget.label,
                        0,
                        rect(l + 54.0, cy - 24.0, right - 6.0, cy - 2.0),
                        &p.text,
                    )?;
                    text(
                        &widget.detail,
                        1,
                        rect(l + 54.0, cy - 2.0, right - 6.0, cy + 22.0),
                        &p.subtext,
                    )?;
                }
            }
        }
        Ok(())
    }
}
