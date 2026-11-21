//! Retained controls and layout in DIPs. Drawing and hit testing share these rectangles.
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

#[derive(Clone)]
pub enum Kind {
    Toggle(bool),
    Select {
        choices: Vec<(String, String)>,
        selected: usize,
    },
    Slider {
        value: i64,
        min: i64,
        max: i64,
    },
    Text(String),
    Button(String),
    Link(String),
    Heading,
}
#[derive(Clone)]
pub struct Control {
    pub id: String,
    pub label: String,
    pub hint: String,
    pub kind: Kind,
    pub enabled: bool,
    pub rect: D2D_RECT_F,
}
impl Control {
    pub fn new(id: &str, label: &str, hint: &str, kind: Kind, enabled: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            hint: hint.into(),
            kind,
            enabled,
            rect: D2D_RECT_F::default(),
        }
    }
    pub fn focusable(&self) -> bool {
        self.enabled && !matches!(self.kind, Kind::Heading)
    }
}
pub fn layout(controls: &mut [Control], width: f32) -> f32 {
    let mut y = 72.;
    let mut previous = D2D_RECT_F::default();
    let mut field_top = 44.;
    for c in controls {
        if inline(&c.id) {
            c.rect = D2D_RECT_F {
                left: previous.right - 92.,
                top: previous.top + field_top,
                right: previous.right - 12.,
                bottom: previous.top + field_top + 32.,
            };
            continue;
        }
        if matches!(c.kind, Kind::Text(_)) {
            field_top = text_field_top(c);
        }
        if let Some(column) = ["red", "green", "blue", "alpha"].iter().position(|id| *id == c.id) {
            let span = ((width - 32.).max(260.) - 228.) / 4.;
            if column == 0 {
                y += 88.;
            }
            c.rect = D2D_RECT_F {
                left: 228. + column as f32 * span,
                right: 228. + (column + 1) as f32 * span,
                top: y - 88.,
                bottom: y,
            };
            continue;
        }
        let height = if c.id == "picker" || (c.id == "open-config" && !c.hint.is_empty()) {
            100.
        } else if c.id == "backdrop" {
            88.
        } else if matches!(c.kind, Kind::Heading) {
            // An empty heading only separates two cards.
            if c.label.is_empty() {
                16.
            } else if c.label.len() > 100 {
                112.
            } else if y == 72. && c.hint.is_empty() {
                28.
            } else {
                56.
            }
        } else if matches!(c.kind, Kind::Text(_)) {
            text_field_top(c) + 44.
        } else if matches!(c.kind, Kind::Slider { .. }) || !c.hint.is_empty() {
            68.
        } else if matches!(c.kind, Kind::Select { .. }) {
            56.
        } else {
            48.
        };
        c.rect = D2D_RECT_F {
            left: 228.,
            top: y,
            right: (width - 32.).max(260.),
            bottom: y + height,
        };
        previous = c.rect;
        y += height;
    }
    y + 24.
}
/// Buttons drawn beside the text field above them, such as Record and Add.
pub fn inline(id: &str) -> bool {
    matches!(id, "record" | "add-folder")
}
/// A text row stacks label, hint and field; the field moves down for a hint.
pub fn text_field_top(c: &Control) -> f32 {
    if c.hint.is_empty() { 44. } else { 62. }
}
pub fn next_focus(controls: &[Control], current: Option<usize>, backwards: bool) -> Option<usize> {
    let order: Vec<_> = controls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.focusable())
        .map(|(i, _)| i)
        .collect();
    if order.is_empty() {
        return None;
    }
    let pos = current.and_then(|c| order.iter().position(|i| *i == c));
    let next = match pos {
        None => {
            if backwards {
                order.len() - 1
            } else {
                0
            }
        }
        Some(p) => {
            if backwards {
                (p + order.len() - 1) % order.len()
            } else {
                (p + 1) % order.len()
            }
        }
    };
    order.get(next).copied()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_and_focus_skip_labels_and_disabled_controls_and_wrap() {
        let mut c = vec![
            Control::new("h", "Group", "", Kind::Heading, true),
            Control::new("a", "A", "", Kind::Toggle(true), true),
            Control::new("b", "B", "", Kind::Button("B".into()), false),
            Control::new("c", "C", "Hint", Kind::Text("".into()), true),
        ];
        let end = layout(&mut c, 520.);
        assert!(end > c[3].rect.bottom);
        assert!(c.windows(2).all(|w| w[0].rect.bottom == w[1].rect.top));
        assert_eq!(c[3].rect.right, 488.);
        assert_eq!(next_focus(&c, None, false), Some(1));
        assert_eq!(next_focus(&c, Some(1), false), Some(3));
        assert_eq!(next_focus(&c, Some(3), false), Some(1));
        assert_eq!(next_focus(&c, Some(1), true), Some(3));
    }
}
