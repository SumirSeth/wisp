//! Lays widgets out in rows and moves the focus between them.

use super::{Input, Widget};

pub const GAP: f32 = 8.0;

/// A widget's place in DIPs, relative to the surface's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    fn center_x(&self) -> f32 {
        self.x + self.w / 2.0
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.bottom()
    }
}

pub fn height(widget: &Widget, width: f32) -> f32 {
    if widget.quiet {
        return if widget.footer && !widget.detail.is_empty() {
            64.0
        } else if widget.editor.is_some() {
            88.0
        } else if matches!(widget.input, Input::Slider { .. } | Input::Buttons { .. }) {
            64.0
        } else {
            44.0
        };
    }
    match &widget.input {
        Input::Slider { .. } => 56.0,
        Input::Choice { options, .. } => {
            let names: Vec<&str> = options.iter().map(|(name, _)| name.as_str()).collect();
            let rect = Rect {
                x: 0.0,
                y: 0.0,
                w: width,
                h: 0.0,
            };
            chip_rects(&rect, &names)
                .last()
                .map_or(76.0, |chip| chip.bottom() + 10.0)
        }
        Input::Buttons { .. } => 64.0,
        _ => 68.0,
    }
}

const CHIP_H: f32 = 24.0;
const CHIP_GAP: f32 = 6.0;

/// Where a choice draws its options, wrapping onto more lines as needed.
/// Widths come from the character count, so layout needs no text measuring.
pub fn chip_rects(rect: &Rect, names: &[&str]) -> Vec<Rect> {
    let (left, right) = (rect.x + 48.0, rect.x + rect.w - 12.0);
    let (mut x, mut y) = (left, rect.y + 42.0);
    names
        .iter()
        .map(|name| {
            let w = (name.chars().count() as f32 * 6.6 + 24.0).clamp(56.0, right - left);
            if x + w > right && x > left {
                x = left;
                y += CHIP_H + CHIP_GAP;
            }
            let chip = Rect { x, y, w, h: CHIP_H };
            x += w + CHIP_GAP;
            chip
        })
        .collect()
}

/// Wide widgets take a row; others fill two columns. A half widget alone in
/// its row stays half width, so the columns line up.
pub fn layout(widgets: &[Widget], width: f32) -> Vec<Rect> {
    let half = (width - GAP) / 2.0;
    let mut rects = Vec::with_capacity(widgets.len());
    let (mut y, mut column) = (0.0, 0);
    for (index, widget) in widgets.iter().enumerate() {
        let h = height(widget, if widget.wide { width } else { half });
        if widget.wide {
            if column == 1 {
                y += GAP + height(&widgets[index - 1], half);
                column = 0;
            }
            rects.push(Rect { x: 0.0, y, w: width, h });
            y += h + GAP;
        } else if column == 0 {
            rects.push(Rect { x: 0.0, y, w: half, h });
            column = 1;
        } else {
            rects.push(Rect {
                x: half + GAP,
                y,
                w: half,
                h,
            });
            y += h.max(height(&widgets[index - 1], half)) + GAP;
            column = 0;
        }
    }
    rects
}

/// A slider's track, from its left to its right end, relative to the surface.
pub fn slider_track(rect: &Rect) -> (f32, f32) {
    (rect.x + 48.0, rect.x + rect.w - 16.0)
}

/// The slider value at surface coordinate `x`.
pub fn slider_value(rect: &Rect, x: f32) -> u32 {
    let (x0, x1) = slider_track(rect);
    (((x - x0) / (x1 - x0)).clamp(0.0, 1.0) * 100.0).round() as u32
}

/// Total height of the laid-out widgets.
pub fn content_height(rects: &[Rect]) -> f32 {
    rects.iter().map(Rect::bottom).fold(0.0, f32::max)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// The widget nearest to `from` in `direction`, or `None` at the edge.
pub fn neighbour(rects: &[Rect], from: usize, direction: Direction) -> Option<usize> {
    let current = rects.get(from)?;
    let candidates = rects.iter().enumerate().filter(|(i, r)| {
        *i != from
            && match direction {
                Direction::Up => r.bottom() <= current.y,
                Direction::Down => r.y >= current.bottom(),
                Direction::Left => r.y == current.y && r.x < current.x,
                Direction::Right => r.y == current.y && r.x > current.x,
            }
    });
    // Prefer the nearest row, then the closest column.
    candidates
        .min_by(|(_, a), (_, b)| {
            let distance = |r: &Rect| {
                let dy = (r.y - current.y).abs();
                let dx = (r.center_x() - current.center_x()).abs();
                (dy, dx)
            };
            distance(a).partial_cmp(&distance(b)).unwrap()
        })
        .map(|(i, _)| i)
}

/// Scrolls the least distance that shows all of `rect` in a view `view_h` tall.
pub fn scroll_to(rect: &Rect, scroll: f32, view_h: f32) -> f32 {
    if rect.y < scroll {
        rect.y
    } else if rect.bottom() > scroll + view_h {
        rect.bottom() - view_h
    } else {
        scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::Target;

    fn tile(label: &str) -> Widget {
        Widget {
            id: label.into(),
            quiet: false,
            footer: false,
            editor: None,
            label: label.into(),
            detail: String::new(),
            glyph: "",
            icon: None,
            wide: false,
            input: Input::Info { value: String::new() },
        }
    }

    fn slider(label: &str) -> Widget {
        Widget {
            wide: true,
            input: Input::Slider {
                value: 0,
                muted: false,
                target: Target::Master,
            },
            ..tile(label)
        }
    }

    #[test]
    fn lays_out_two_columns_and_full_rows() {
        let widgets = [tile("a"), tile("b"), slider("c"), tile("d"), slider("e")];
        let rects = layout(&widgets, 200.0);
        assert_eq!((rects[0].x, rects[0].y, rects[0].w), (0.0, 0.0, 96.0));
        assert_eq!((rects[1].x, rects[1].y), (104.0, 0.0));
        assert_eq!((rects[2].y, rects[2].w), (76.0, 200.0));
        assert_eq!((rects[3].y, rects[3].w), (140.0, 96.0));
        // A half tile alone in its row moves the next wide widget down.
        assert_eq!(rects[4].y, 216.0);
        assert_eq!(content_height(&rects), 272.0);
    }

    #[test]
    fn moves_focus_by_direction() {
        let widgets = [tile("a"), tile("b"), slider("c"), tile("d")];
        let rects = layout(&widgets, 200.0);
        assert_eq!(neighbour(&rects, 0, Direction::Right), Some(1));
        assert_eq!(neighbour(&rects, 1, Direction::Left), Some(0));
        assert_eq!(neighbour(&rects, 1, Direction::Down), Some(2));
        assert_eq!(neighbour(&rects, 2, Direction::Up), Some(0));
        assert_eq!(neighbour(&rects, 2, Direction::Down), Some(3));
        assert_eq!(neighbour(&rects, 3, Direction::Down), None);
        assert_eq!(neighbour(&rects, 0, Direction::Left), None);
    }

    #[test]
    fn wraps_choices_onto_more_lines() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 300.0,
            h: 0.0,
        };
        let chips = chip_rects(&rect, &["Speakers", "Headphones (Bluetooth)", "Monitor"]);
        assert_eq!(chips[0].y, 42.0);
        assert!(chips[1].y > chips[0].y || chips[2].y > chips[0].y);
        assert!(chips.iter().all(|c| c.x + c.w <= 288.0));
    }

    #[test]
    fn scrolls_focus_into_view() {
        let rect = Rect {
            x: 0.0,
            y: 300.0,
            w: 10.0,
            h: 50.0,
        };
        assert_eq!(scroll_to(&rect, 0.0, 200.0), 150.0);
        assert_eq!(scroll_to(&rect, 320.0, 200.0), 300.0);
        assert_eq!(scroll_to(&rect, 200.0, 200.0), 200.0);
    }
}
