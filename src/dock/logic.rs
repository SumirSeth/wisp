//! Matching and DIP geometry.

use crate::config::WindowListMode;
use crate::indexer::AppEntry;

pub const DEFAULT_APP_ICON_SIZE: f32 = 32.0;
pub const MIN_APP_ICON_SIZE: f32 = 24.0;
pub const MAX_APP_ICON_SIZE: f32 = 40.0;
pub const TILE_TOP: f32 = 5.0;
pub const TILE_BOTTOM: f32 = 51.0;
pub const TILE_RADIUS: f32 = 8.0;
pub const ICON_CENTER_Y: f32 = 26.0;
pub const INDICATOR_TOP: f32 = 46.0;

pub fn slide_value(from: f32, to: f32, progress: f32) -> f32 {
    let t = progress.clamp(0.0, 1.0);
    let ease = t * t * (3.0 - 2.0 * t);
    from + (to - from) * ease
}

pub fn icon_pixels(dpi: u32, size: f32) -> i32 {
    (size * dpi as f32 / 96.0).round() as i32
}

pub const HEIGHT: f32 = 56.0;
pub const MARGIN: f32 = 9.0;
pub const SLOT: f32 = 48.0;
pub const PAD: f32 = 8.0;
pub const SEPARATOR: f32 = 9.0;

pub fn layout_scale(app_icon_size: f32) -> f32 {
    app_icon_size / DEFAULT_APP_ICON_SIZE
}

pub fn height(scale: f32) -> f32 {
    HEIGHT * scale
}
pub fn margin(scale: f32) -> f32 {
    MARGIN * scale
}
pub fn slot(scale: f32) -> f32 {
    SLOT * scale
}
pub fn pad(scale: f32) -> f32 {
    PAD * scale
}
pub fn separator_width(scale: f32) -> f32 {
    SEPARATOR * scale
}

pub fn left(index: usize) -> f32 {
    PAD + index as f32 * SLOT
}

pub fn left_scaled(index: usize, scale: f32) -> f32 {
    left(index) * scale
}

pub fn hit(x: f32, y: f32, running: usize) -> Option<usize> {
    if !(0.0..HEIGHT).contains(&y) {
        return None;
    }
    (0..1 + running).find(|&i| (left(i)..left(i) + SLOT).contains(&x))
}

pub fn hit_scaled(x: f32, y: f32, running: usize, scale: f32) -> Option<usize> {
    hit(x / scale, y / scale, running)
}

/// App slots and the optional rule between visible pinned and running apps.
#[derive(Clone, Copy)]
pub struct AppLayout {
    pub count: usize,
    pub pinned: usize,
}

impl AppLayout {
    pub fn divider(self) -> bool {
        self.pinned > 0 && self.pinned < self.count
    }
    pub fn gap(self) -> f32 {
        if self.divider() { SEPARATOR } else { 0.0 }
    }
    pub fn left(self, index: usize, scale: f32) -> f32 {
        (left(index)
            + if self.divider() && index > self.pinned {
                SEPARATOR
            } else {
                0.0
            })
            * scale
    }
    pub fn hit(self, x: f32, y: f32, scale: f32) -> Option<usize> {
        if !self.divider() {
            return hit_scaled(x, y, self.count, scale);
        }
        if !(0.0..height(scale)).contains(&y) {
            return None;
        }
        (0..=self.count).find(|&i| (self.left(i, scale)..self.left(i, scale) + slot(scale)).contains(&x))
    }
    /// Clamps a drag to an app slot, including the divider and outside edges.
    pub fn slot_at(self, x: f32, scale: f32) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        let x = x.clamp(self.left(1, scale), self.left(self.count, scale) + slot(scale) - 0.01);
        self.hit(x, 0.0, scale).or(Some(
            if x < self.left(self.pinned + 1, scale) - separator_width(scale) / 2.0 {
                self.pinned
            } else {
                self.pinned + 1
            },
        ))
    }

    /// The divider's midpoint separates groups. Empty groups have an edge target.
    pub fn drop_target(self, x: f32, scale: f32, scroll: usize, pins: usize) -> Option<DropTarget> {
        let mut index = self.slot_at(x, scale)? - 1 + scroll;
        let boundary = if pins == 0 {
            self.left(1, scale) + slot(scale) / 2.0
        } else if pins < scroll {
            f32::NEG_INFINITY
        } else if pins >= scroll + self.count {
            self.left(pins - scroll, scale) + slot(scale)
        } else {
            self.left(pins - scroll + 1, scale) - self.gap() * scale / 2.0
        };
        let pinned = x < boundary;
        // The pin side of the gap appends to the pins instead of replacing the last pin's slot.
        if pinned && self.divider() && x >= self.left(self.pinned, scale) + slot(scale) {
            index = pins;
        }
        Some(DropTarget { index, pinned })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct DropTarget {
    pub index: usize,
    pub pinned: bool,
}

/// Moves an app and returns the new pin count, keeping both groups contiguous.
pub fn move_app<T>(items: &mut Vec<T>, from: usize, target: DropTarget, pins: usize) -> usize {
    if from >= items.len() || target.index >= items.len() {
        return pins;
    }
    let remaining_pins = pins - usize::from(from < pins);
    let to = if target.pinned {
        target.index.min(remaining_pins)
    } else {
        target.index.max(remaining_pins)
    };
    move_item(items, from, to);
    remaining_pins + usize::from(target.pinned)
}

/// Moves an item, shifting the ones between.
pub fn move_item<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from != to && from < items.len() && to < items.len() {
        let item = items.remove(from);
        items.insert(to, item);
    }
}

/// Blocks in the dock's saved row order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Launcher,
    Apps,
    Tray,
    Status,
    Clock,
}

pub const DEFAULT_ORDER: [Section; 5] = [
    Section::Launcher,
    Section::Apps,
    Section::Tray,
    Section::Status,
    Section::Clock,
];

impl Section {
    pub fn name(self) -> &'static str {
        match self {
            Self::Launcher => "launcher",
            Self::Apps => "apps",
            Self::Tray => "tray",
            Self::Status => "status",
            Self::Clock => "clock",
        }
    }
}

/// Unknown and duplicate names disappear; omitted blocks follow in default order.
pub fn section_order<'a>(names: impl IntoIterator<Item = &'a str>) -> [Section; 5] {
    let mut order = Vec::with_capacity(5);
    for section in names
        .into_iter()
        .filter_map(|name| DEFAULT_ORDER.into_iter().find(|section| section.name() == name))
        .chain(DEFAULT_ORDER)
    {
        if !order.contains(&section) {
            order.push(section);
        }
    }
    order.try_into().unwrap()
}

pub fn move_section(order: &mut [Section; 5], section: Section, target: Section) {
    let from = order.iter().position(|&item| item == section).unwrap();
    let to = order.iter().position(|&item| item == target).unwrap();
    if from < to {
        order[from..=to].rotate_left(1);
    } else {
        order[to..=from].rotate_right(1);
    }
}

/// One padded row, with one gap between each pair of nonempty blocks.
#[derive(Clone, Copy)]
pub struct RowLayout {
    pub apps: AppLayout,
    pub order: [Section; 5],
    pub tray: Option<usize>,
}

impl RowLayout {
    pub fn sections(self, scale: f32) -> impl Iterator<Item = (Section, f32, f32)> {
        let mut x = pad(scale);
        let mut first = true;
        self.order.into_iter().filter_map(move |section| {
            let width = match section {
                Section::Launcher => SLOT,
                Section::Apps => self.apps.count as f32 * SLOT + self.apps.gap(),
                Section::Tray => self
                    .tray
                    .map_or(0.0, |count| CHEVRON_W + count.min(MAX_INLINE) as f32 * TRAY_W),
                Section::Status => STATUS_W,
                Section::Clock => CLOCK_W,
            } * scale;
            if width == 0.0 {
                return None;
            }
            if !first {
                x += separator_width(scale);
            }
            first = false;
            let left = x;
            x += width;
            Some((section, left, width))
        })
    }

    pub fn section_left(self, section: Section, scale: f32) -> Option<f32> {
        self.sections(scale)
            .find(|&(item, _, _)| item == section)
            .map(|(_, left, _)| left)
    }

    pub fn width(self, scale: f32) -> f32 {
        self.sections(scale)
            .last()
            .map_or(2.0 * pad(scale), |(_, left, width)| left + width + pad(scale))
    }

    pub fn left(self, index: usize, scale: f32) -> f32 {
        if index == 0 {
            return self.section_left(Section::Launcher, scale).unwrap();
        }
        self.section_left(Section::Apps, scale).unwrap_or(pad(scale)) + self.apps.left(index, scale)
            - left_scaled(1, scale)
    }

    pub fn hit(self, x: f32, y: f32, scale: f32) -> Option<usize> {
        if !(0.0..height(scale)).contains(&y) {
            return None;
        }
        (0..=self.apps.count)
            .find(|&index| (self.left(index, scale)..self.left(index, scale) + slot(scale)).contains(&x))
    }

    pub fn part_left(self, part: Part, scale: f32) -> Option<f32> {
        let (section, offset) = match part {
            Part::Chevron => (Section::Tray, 0.0),
            Part::Tray(index) if self.tray.is_some_and(|count| index < count.min(MAX_INLINE)) => {
                (Section::Tray, CHEVRON_W + index as f32 * TRAY_W)
            }
            Part::Tray(_) => return None,
            Part::Status => (Section::Status, 0.0),
            Part::Clock => (Section::Clock, 0.0),
        };
        self.section_left(section, scale).map(|left| left + offset * scale)
    }

    pub fn part_hit(self, x: f32, y: f32, parts: &[Part], scale: f32) -> Option<Part> {
        if !(0.0..height(scale)).contains(&y) {
            return None;
        }
        parts.iter().copied().find(|part| {
            self.part_left(*part, scale)
                .is_some_and(|left| (left..left + part.width_scaled(scale)).contains(&x))
        })
    }

    /// Gaps belong to their nearest block; no target splits the apps or tray.
    pub fn section_at(self, x: f32, scale: f32) -> Section {
        self.sections(scale)
            .min_by(|(_, a, aw), (_, b, bw)| {
                let distance = |left: f32, width: f32| (left - x).max(0.0).max(x - left - width);
                distance(*a, *aw).total_cmp(&distance(*b, *bw))
            })
            .unwrap()
            .0
    }

    /// Cross a neighboring block's center before shifting it past the dragged block.
    pub fn section_target(self, section: Section, x: f32, scale: f32) -> Section {
        let from = self.order.iter().position(|&item| item == section).unwrap();
        let mut target = section;
        for (item, left, width) in self.sections(scale) {
            let at = self.order.iter().position(|&entry| entry == item).unwrap();
            let center = left + width / 2.0;
            if (at < from && x < center && target == section) || (at > from && x >= center) {
                target = item;
            }
        }
        target
    }

    pub fn drop_target(self, x: f32, scale: f32, scroll: usize, pins: usize) -> Option<DropTarget> {
        let local = x - self.section_left(Section::Apps, scale)? + left_scaled(1, scale);
        self.apps.drop_target(local, scale, scroll, pins)
    }
}

/// Pins keep their saved order; running apps keep their first-seen order.
pub fn pinned_first<T>(
    pins: impl IntoIterator<Item = T>,
    mut running: Vec<T>,
    same: impl Fn(&T, &T) -> bool,
) -> Vec<T> {
    let mut ordered = Vec::new();
    for pin in pins {
        if let Some(at) = running.iter().position(|item| same(&pin, item)) {
            ordered.push(running.remove(at));
        } else {
            ordered.push(pin);
        }
    }
    ordered.extend(running);
    ordered
}

pub const CHEVRON_W: f32 = 28.0;
pub const TRAY_W: f32 = 32.0;
pub const STATUS_W: f32 = SLOT;
pub const STATUS_CELL: f32 = 20.0;

/// Top-left of a matrix cell, relative to its slot, in DIPs.
pub fn status_cell(index: usize) -> (f32, f32) {
    (
        4.0 + (index % 2) as f32 * STATUS_CELL,
        8.0 + (index / 2) as f32 * STATUS_CELL,
    )
}
pub const CLOCK_W: f32 = 88.0;
/// The window list above an app icon: its padding, one row per window, and
/// the widest a row may be before it is clipped, in DIPs.
pub const LIST_PAD: (f32, f32) = (7.0, 3.0);
pub const LIST_ROW: f32 = 22.0;
pub const LIST_TEXT_MAX: f32 = 280.0;

/// Whole rows that fit in a physical-pixel height, including both DIP paddings.
pub fn window_list_rows(available: i32, scale: f32, padding: f32) -> usize {
    let available = available.max(0) as f32;
    let rows = ((available / scale - 2.0 * padding).max(0.0) / LIST_ROW).floor() as usize;
    // Match the popup's rounding to whole physical pixels at fractional scales.
    if rows > 0 && ((rows as f32 * LIST_ROW + 2.0 * padding) * scale).ceil() > available {
        rows - 1
    } else {
        rows
    }
}

pub fn show_window_list(mode: WindowListMode, windows: usize) -> bool {
    windows > 0 && (mode == WindowListMode::Always || mode == WindowListMode::Multiple && windows > 1)
}
/// Notification-area icons shown in the dock; the rest are behind the chevron.
pub const MAX_INLINE: usize = 5;
/// Tray icons share the dock's enlarged visual rhythm.
pub const TRAY_ICON: f32 = 18.0;

/// The status parts after the apps, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// Opens the notification-area flyout.
    Chevron,
    /// The nth notification-area icon shown in the dock.
    Tray(usize),
    Status,
    Clock,
}

impl Part {
    pub fn width(self) -> f32 {
        match self {
            Part::Chevron => CHEVRON_W,
            Part::Tray(_) => TRAY_W,
            Part::Status => STATUS_W,
            Part::Clock => CLOCK_W,
        }
    }

    pub fn width_scaled(self, scale: f32) -> f32 {
        self.width() * scale
    }
}

/// The tray, fixed status matrix and clock, in order.
pub fn parts(tray: Option<usize>) -> Vec<Part> {
    let tray = tray.map(|count| std::iter::once(Part::Chevron).chain((0..count.min(MAX_INLINE)).map(Part::Tray)));
    tray.into_iter().flatten().chain([Part::Status, Part::Clock]).collect()
}

/// The notification-area flyout: a grid of cells, as in Windows 11.
pub const FLYOUT_CELL: f32 = 40.0;
pub const FLYOUT_PAD: f32 = 8.0;
pub const FLYOUT_COLUMNS: usize = 5;
/// Space between the flyout and the dock.
pub const FLYOUT_GAP: f32 = 8.0;

/// The flyout's size in DIPs for `count` icons; an empty one keeps one cell.
pub fn flyout_size(count: usize) -> (f32, f32) {
    let columns = count.clamp(1, FLYOUT_COLUMNS);
    let rows = count.max(1).div_ceil(FLYOUT_COLUMNS);
    (
        2.0 * FLYOUT_PAD + columns as f32 * FLYOUT_CELL,
        2.0 * FLYOUT_PAD + rows as f32 * FLYOUT_CELL,
    )
}

/// The cell's top-left corner.
pub fn flyout_cell(index: usize) -> (f32, f32) {
    (
        FLYOUT_PAD + (index % FLYOUT_COLUMNS) as f32 * FLYOUT_CELL,
        FLYOUT_PAD + (index / FLYOUT_COLUMNS) as f32 * FLYOUT_CELL,
    )
}

pub fn flyout_hit(x: f32, y: f32, count: usize) -> Option<usize> {
    if x < FLYOUT_PAD || y < FLYOUT_PAD {
        return None;
    }
    let column = ((x - FLYOUT_PAD) / FLYOUT_CELL) as usize;
    let index = ((y - FLYOUT_PAD) / FLYOUT_CELL) as usize * FLYOUT_COLUMNS + column;
    (column < FLYOUT_COLUMNS && index < count).then_some(index)
}

/// Wheel delta per volume percent, for 2% per 120-delta notch.
pub const WHEEL_PER_PERCENT: i32 = 60;

/// Spends whole percents of the accumulated wheel `delta` on `percent`, and
/// returns the new volume with the leftover delta, for precision touchpads.
pub fn wheel_volume(percent: u32, delta: i32) -> (u32, i32) {
    let steps = delta / WHEEL_PER_PERCENT;
    ((percent as i32 + steps).clamp(0, 100) as u32, delta % WHEEL_PER_PERCENT)
}

/// Wheel travel that switches one desktop: a mouse wheel's notch.
pub const DESKTOP_NOTCH: i32 = 120;

/// Adds wheel `delta` to the `travel` so far, negative toward the right.
/// Returns whether to switch right or left once a notch is complete, and the
/// travel left over. Turning the wheel back starts over.
pub fn wheel_desktop(travel: i32, delta: i32) -> (Option<bool>, i32) {
    let travel = if travel != 0 && (travel < 0) != (delta < 0) {
        delta
    } else {
        travel + delta
    };
    if travel.abs() >= DESKTOP_NOTCH {
        (Some(travel < 0), 0)
    } else {
        (None, travel)
    }
}

fn normalize(path: &str) -> String {
    path.trim()
        .trim_matches('"')
        .replace('/', "\\")
        .trim_start_matches(r"\\?\")
        .to_lowercase()
}

/// Paths must match completely: unrelated programs often share a filename.
pub fn match_app<'a>(apps: &'a [AppEntry], exe: &str, aumid: Option<&str>) -> Option<&'a AppEntry> {
    if let Some(id) = aumid.filter(|s| !s.is_empty())
        && let Some(app) = apps.iter().find(|app| app.id.eq_ignore_ascii_case(id))
    {
        return Some(app);
    }
    if exe.is_empty()
        || exe
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|s| s.eq_ignore_ascii_case("ApplicationFrameHost.exe"))
    {
        return None;
    }
    let exe = normalize(exe);
    apps.iter()
        .find(|app| app.path.as_deref().is_some_and(|path| normalize(path) == exe) || normalize(&app.id) == exe)
}

/// Keeps items in the order they first appeared. The window list arrives in
/// z-order, which changes whenever an app is focused; following it directly
/// made a clicked icon jump to another place in the dock.
pub fn stable_order<T>(previous: Vec<T>, current: Vec<T>, same: impl Fn(&T, &T) -> bool) -> Vec<T> {
    let mut current = current;
    let mut ordered = Vec::with_capacity(current.len());
    for old in previous {
        if let Some(at) = current.iter().position(|new| same(&old, new)) {
            ordered.push(current.remove(at));
        }
    }
    ordered.append(&mut current);
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_names_and_moves_keep_every_block_once() {
        assert_eq!(section_order([]), DEFAULT_ORDER);
        assert_eq!(
            section_order(["clock", "bad", "clock", "apps"]),
            [
                Section::Clock,
                Section::Apps,
                Section::Launcher,
                Section::Tray,
                Section::Status
            ]
        );
        for source in DEFAULT_ORDER {
            for target in DEFAULT_ORDER {
                let mut order = DEFAULT_ORDER;
                move_section(&mut order, source, target);
                assert_eq!(order.iter().filter(|&&item| item == source).count(), 1);
                assert_eq!(
                    order.iter().position(|&item| item == source),
                    DEFAULT_ORDER.iter().position(|&item| item == target)
                );
                assert_eq!(section_order(order.map(Section::name)), order);
            }
        }
    }

    #[test]
    fn row_geometry_hits_and_gaps_follow_every_order() {
        fn permutations(order: &mut [Section; 5], at: usize, check: &impl Fn([Section; 5])) {
            if at == 5 {
                check(*order);
                return;
            }
            for index in at..5 {
                order.swap(at, index);
                permutations(order, at + 1, check);
                order.swap(at, index);
            }
        }
        let mut order = DEFAULT_ORDER;
        permutations(&mut order, 0, &|order| {
            for scale in [0.75, 1.0, 1.25, 2.0] {
                for tray in [None, Some(0), Some(3)] {
                    for (count, pinned) in [(0, 0), (4, 0), (4, 2), (4, 4)] {
                        let row = RowLayout {
                            apps: AppLayout { count, pinned },
                            order,
                            tray,
                        };
                        let parts = parts(tray);
                        let mut end = pad(scale);
                        for (index, (section, left, width)) in row.sections(scale).enumerate() {
                            assert_eq!(left, end + if index == 0 { 0.0 } else { separator_width(scale) });
                            assert_eq!(row.section_at(left + width / 2.0, scale), section);
                            if index > 0 {
                                let gap = left - separator_width(scale) / 2.0;
                                assert_eq!(row.hit(gap, 10.0, scale), None);
                                assert_eq!(row.part_hit(gap, 10.0, &parts, scale), None);
                            }
                            end = left + width;
                        }
                        assert_eq!(row.width(scale), end + pad(scale));
                        for index in 0..=count {
                            let x = row.left(index, scale) + slot(scale) / 2.0;
                            assert_eq!(row.hit(x, 10.0, scale), Some(index));
                            assert_eq!(row.part_hit(x, 10.0, &parts, scale), None);
                        }
                        for part in &parts {
                            let x = row.part_left(*part, scale).unwrap() + part.width_scaled(scale) / 2.0;
                            assert_eq!(row.part_hit(x, 10.0, &parts, scale), Some(*part));
                            assert_eq!(row.hit(x, 10.0, scale), None);
                        }
                        if count > 0 {
                            let x = row.left(1, scale) + slot(scale) / 2.0;
                            assert_eq!(row.drop_target(x, scale, 7, 7 + pinned).unwrap().index, 7);
                        }
                    }
                }
            }
        });
    }

    #[test]
    fn section_drag_crosses_centers_without_bouncing_back() {
        for source in DEFAULT_ORDER {
            for target in DEFAULT_ORDER {
                let mut row = RowLayout {
                    apps: AppLayout { count: 8, pinned: 2 },
                    order: DEFAULT_ORDER,
                    tray: Some(2),
                };
                let (_, left, width) = row.sections(1.0).find(|&(item, _, _)| item == target).unwrap();
                let from = row.order.iter().position(|&item| item == source).unwrap();
                let to = row.order.iter().position(|&item| item == target).unwrap();
                let x = left + width / 2.0 + if from > to { -0.1 } else { 0.1 };
                let selected = row.section_target(source, x, 1.0);
                assert_eq!(selected, target);
                move_section(&mut row.order, source, selected);
                assert_eq!(row.section_target(source, x, 1.0), source);
            }
        }
    }

    #[test]
    fn matrix_cells_fit_and_hit_one_slot() {
        assert_eq!(STATUS_W, SLOT);
        let parts = parts(None);
        for scale in [0.75, 1.0, 1.25, 2.0] {
            let row = RowLayout {
                apps: AppLayout { count: 0, pinned: 0 },
                order: DEFAULT_ORDER,
                tray: None,
            };
            let left = row.part_left(Part::Status, scale).unwrap();
            for (i, expected) in [(4.0, 8.0), (24.0, 8.0), (4.0, 28.0), (24.0, 28.0)]
                .into_iter()
                .enumerate()
            {
                let (x, y) = status_cell(i);
                assert_eq!((x, y), expected);
                assert!(x + STATUS_CELL <= STATUS_W && y + STATUS_CELL <= HEIGHT);
                assert_eq!(
                    row.part_hit(left + (x + 10.0) * scale, (y + 10.0) * scale, &parts, scale),
                    Some(Part::Status)
                );
            }
        }
    }

    #[test]
    fn drag_slots_and_group_edges_scale_and_scroll() {
        for size in [24.0, 32.0, 40.0] {
            for dpi in [96.0, 144.0, 192.0] {
                let scale = layout_scale(size);
                let layout = AppLayout { count: 4, pinned: 2 };
                let target = |x: f32| layout.drop_target(x * dpi / 96.0 / (dpi / 96.0), scale, 3, 5).unwrap();
                assert_eq!(target(-100.0), DropTarget { index: 3, pinned: true });
                for index in 1..=4 {
                    let x = layout.left(index, scale) + slot(scale) / 2.0;
                    assert_eq!(
                        target(x),
                        DropTarget {
                            index: index - 1 + 3,
                            pinned: index <= 2
                        }
                    );
                }
                let boundary = layout.left(3, scale) - separator_width(scale) / 2.0;
                assert_eq!(target(boundary - 0.1), DropTarget { index: 5, pinned: true });
                assert!(!target(boundary).pinned);
                assert_eq!(
                    target(10000.0),
                    DropTarget {
                        index: 6,
                        pinned: false
                    }
                );
                let empty = AppLayout { count: 0, pinned: 0 };
                assert_eq!(empty.slot_at(10.0, scale), None);
                let all = AppLayout { count: 2, pinned: 2 };
                let end = all.left(2, scale) + slot(scale);
                assert!(all.drop_target(end - 0.1, scale, 0, 2).unwrap().pinned);
                assert!(!all.drop_target(end, scale, 0, 2).unwrap().pinned);
                let none = AppLayout { count: 2, pinned: 0 };
                let start = none.left(1, scale) + slot(scale) / 2.0;
                assert!(none.drop_target(start - 0.1, scale, 0, 0).unwrap().pinned);
                assert!(!none.drop_target(start, scale, 0, 0).unwrap().pinned);
                assert!(!none.drop_target(100.0, scale, 4, 2).unwrap().pinned);
            }
        }
    }

    #[test]
    fn drops_pin_unpin_and_keep_group_order() {
        let mut apps = vec!["a", "b", "c", "d"];
        let pins = move_app(&mut apps, 3, DropTarget { index: 2, pinned: true }, 2);
        assert_eq!((apps.clone(), pins), (vec!["a", "b", "d", "c"], 3));
        let pins = move_app(
            &mut apps,
            0,
            DropTarget {
                index: 3,
                pinned: false,
            },
            pins,
        );
        assert_eq!((apps.clone(), pins), (vec!["b", "d", "c", "a"], 2));
        let pins = move_app(&mut apps, 1, DropTarget { index: 0, pinned: true }, pins);
        assert_eq!((apps.clone(), pins), (vec!["d", "b", "c", "a"], 2));
        let pins = move_app(
            &mut apps,
            3,
            DropTarget {
                index: 2,
                pinned: false,
            },
            pins,
        );
        assert_eq!((apps.clone(), pins), (vec!["d", "b", "a", "c"], 2));
        // A z-order refresh retains the dragged running order and saved pins.
        let refreshed = stable_order(apps, vec!["c", "b", "a"], |a, b| a == b);
        assert_eq!(pinned_first(["d", "b"], refreshed, |a, b| a == b), ["d", "b", "a", "c"]);
        let mut closed = vec!["closed", "running"];
        let pins = move_app(
            &mut closed,
            0,
            DropTarget {
                index: 1,
                pinned: false,
            },
            1,
        );
        closed.retain(|app| *app != "closed");
        assert_eq!(pinned_first([], closed, |a, b| a == b), ["running"]);
        assert_eq!(pins, 0);
        let mut apps = vec![1, 2];
        move_item(&mut apps, 5, 0);
        move_item(&mut apps, 0, 5);
        assert_eq!(apps, [1, 2]);
    }

    #[test]
    fn pins_precede_running_apps_without_duplicates() {
        assert_eq!(
            pinned_first(["c", "a", "closed"], vec!["b", "a", "c", "d"], |a, b| a == b),
            ["c", "a", "closed", "b", "d"]
        );
        assert_eq!(pinned_first([], vec!["b", "a"], |a, b| a == b), ["b", "a"]);
    }

    #[test]
    fn pin_divider_geometry_and_hits_agree() {
        for scale in [0.75, 1.0, 1.25, 1.5, 2.0] {
            for (count, pinned) in [(0, 0), (3, 0), (3, 3), (3, 1), (3, 2)] {
                let layout = AppLayout { count, pinned };
                assert_eq!(layout.divider(), pinned > 0 && pinned < count);
                for i in 0..=count {
                    let x = layout.left(i, scale);
                    assert_eq!(layout.hit(x, 0.0, scale), Some(i));
                    assert_eq!(layout.hit(x + slot(scale) - 0.5, height(scale) - 0.5, scale), Some(i));
                    assert_eq!(layout.hit(x, height(scale), scale), None);
                }
                if layout.divider() {
                    let gap = left_scaled(pinned + 1, scale);
                    assert_eq!(layout.hit(gap, 10.0, scale), None);
                    assert_eq!(layout.hit(gap + separator_width(scale) - 0.1, 10.0, scale), None);
                }
            }
        }
    }

    #[test]
    fn window_list_fits_whole_rows_with_padding_at_each_scale() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            for padding in [0.0, LIST_PAD.1, 8.0] {
                assert_eq!(window_list_rows(-10, scale, padding), 0);
                assert_eq!(window_list_rows(0, scale, padding), 0);
                let padding_height = (2.0 * padding * scale).ceil() as i32;
                assert_eq!(window_list_rows(padding_height, scale, padding), 0);
                for rows in [1, 2, 10, 100] {
                    let height = ((rows as f32 * LIST_ROW + 2.0 * padding) * scale).ceil() as i32;
                    assert_eq!(window_list_rows(height, scale, padding), rows);
                    assert_eq!(window_list_rows(height - 1, scale, padding), rows - 1);
                    assert_eq!(window_list_rows(height + 1, scale, padding), rows);
                }
            }
        }
    }

    #[test]
    fn window_list_modes_follow_the_selected_policy() {
        assert!(show_window_list(WindowListMode::Always, 1));
        assert!(show_window_list(WindowListMode::Always, 3));
        assert!(!show_window_list(WindowListMode::Multiple, 1));
        assert!(show_window_list(WindowListMode::Multiple, 2));
        assert!(!show_window_list(WindowListMode::Off, 1));
        assert!(!show_window_list(WindowListMode::Off, 2));
        assert!(!show_window_list(WindowListMode::Always, 0));
    }

    #[test]
    fn app_icon_pixel_size_scales_with_dpi() {
        assert_eq!(icon_pixels(96, 24.0), 24);
        assert_eq!(icon_pixels(144, DEFAULT_APP_ICON_SIZE), 48);
        assert_eq!(icon_pixels(192, MAX_APP_ICON_SIZE), 80);
    }
    #[test]
    fn dock_geometry_scales_with_app_icon_size() {
        let small = layout_scale(MIN_APP_ICON_SIZE);
        let normal = layout_scale(DEFAULT_APP_ICON_SIZE);
        let large = layout_scale(MAX_APP_ICON_SIZE);
        assert_eq!((small, normal, large), (0.75, 1.0, 1.25));
        assert_eq!(height(small), 42.0);
        assert_eq!(slot(small), 36.0);
        assert_eq!(height(large), 70.0);
        assert_eq!(slot(large), 60.0);

        for scale in [small, normal, large] {
            let row = RowLayout {
                apps: AppLayout { count: 2, pinned: 1 },
                order: DEFAULT_ORDER,
                tray: None,
            };
            assert_eq!(row.width(scale), row.width(1.0) * scale);
            let x = left_scaled(1, scale) + slot(scale) / 2.0;
            assert_eq!(hit_scaled(x, height(scale) / 2.0, 2, scale), Some(1));
        }
    }

    #[test]
    fn scaled_slot_edges_match_hit_targets() {
        for size in [24.0, 28.0, 32.0, 36.0, 40.0] {
            let scale = layout_scale(size);
            for index in 0..=8 {
                let left = left_scaled(index, scale);
                let right = left + slot(scale);
                let bottom = height(scale);
                assert_eq!(left, (8.0 + index as f32 * 48.0) * size / 32.0);
                assert_eq!(right, left_scaled(index + 1, scale));
                assert_eq!(bottom, 56.0 * size / 32.0);
                assert_eq!(hit_scaled(left, 0.0, 8, scale), Some(index));
                assert_eq!(hit_scaled(right - 0.5, bottom - 0.5, 8, scale), Some(index));
                assert_eq!(hit_scaled(left, bottom, 8, scale), None);
                assert_ne!(hit_scaled(right, 0.0, 8, scale), Some(index));
            }
        }
    }

    #[test]
    fn slide_endpoints_and_reversal_are_continuous() {
        assert_eq!(slide_value(1.0, 0.0, 0.0), 1.0);
        assert_eq!(slide_value(1.0, 0.0, 1.0), 0.0);
        let midway = slide_value(0.0, 1.0, 0.4);
        assert_eq!(slide_value(midway, 0.0, 0.0), midway);
        assert!(slide_value(midway, 0.0, 0.1) < midway);
        assert_eq!(slide_value(0.0, 1.0, 2.0), 1.0);
    }
    #[test]
    fn stable_order_keeps_first_seen_positions() {
        let previous = vec!["a", "b", "c"];
        // Focusing c makes it first in z-order; it must stay third.
        let current = vec!["c", "a", "d", "b"];
        assert_eq!(stable_order(previous, current, |a, b| a == b), ["a", "b", "c", "d"]);
        // Closed apps drop out and leaving ones keep their relative order.
        assert_eq!(stable_order(vec!["a", "b"], vec!["b", "c"], |a, b| a == b), ["b", "c"]);
    }
    #[test]
    fn matches_full_paths_and_store_ids() {
        let apps = vec![
            AppEntry {
                id: "{folder}\\a.exe".into(),
                path: Some(r"C:\Apps\a.exe".into()),
                name: "A".into(),
            },
            AppEntry {
                id: "Package!App".into(),
                path: None,
                name: "Store".into(),
            },
        ];
        assert_eq!(match_app(&apps, r"\\?\C:\APPS\A.EXE", None).unwrap().name, "A");
        assert!(match_app(&apps, r"D:\Other\a.exe", None).is_none());
        assert_eq!(
            match_app(&apps, r"C:\Windows\ApplicationFrameHost.exe", Some("package!app"))
                .unwrap()
                .name,
            "Store"
        );
        assert!(match_app(&apps, "", None).is_none());
    }
    #[test]
    fn wheel_steps_volume_by_two_percent_within_range() {
        assert_eq!(wheel_volume(50, 120), (52, 0));
        assert_eq!(wheel_volume(50, -360), (44, 0));
        assert_eq!(wheel_volume(99, 120), (100, 0));
        assert_eq!(wheel_volume(1, -120), (0, 0));
        // Precision touchpads send fractions of a notch, which add up.
        assert_eq!(wheel_volume(50, 40), (50, 40));
        assert_eq!(wheel_volume(50, 40 + 40), (51, 20));
        assert_eq!(wheel_volume(50, -70), (49, -10));
    }
    #[test]
    fn wheel_switches_one_desktop_a_notch() {
        // Down and touchpad swipes right go to the desktop on the right.
        assert_eq!(wheel_desktop(0, -120), (Some(true), 0));
        assert_eq!(wheel_desktop(0, 120), (Some(false), 0));
        // A fast flick still switches only once.
        assert_eq!(wheel_desktop(0, -480), (Some(true), 0));
        // Precision touchpads send fractions of a notch, which add up.
        assert_eq!(wheel_desktop(0, -50), (None, -50));
        assert_eq!(wheel_desktop(-100, -50), (Some(true), 0));
        // Turning back starts over.
        assert_eq!(wheel_desktop(-100, 30), (None, 30));
    }
    #[test]
    fn floating_layout_and_hit_targets() {
        assert_eq!((HEIGHT, MARGIN, SLOT), (56.0, 9.0, 48.0));
        for count in [0, 1, 8, 30] {
            for index in 0..=count {
                assert_eq!(hit(left(index) + SLOT / 2.0, HEIGHT / 2.0, count), Some(index));
            }
            assert_eq!(hit(-1.0, 20.0, count), None);
            assert_eq!(hit(PAD, HEIGHT, count), None);
        }
        for dpi in [96, 120, 144] {
            assert_eq!(
                icon_pixels(dpi, DEFAULT_APP_ICON_SIZE) as f32,
                DEFAULT_APP_ICON_SIZE * dpi as f32 / 96.0
            );
        }
        assert_eq!(((HEIGHT + MARGIN) * 120.0 / 96.0).round() as i32, 81);
    }
    #[test]
    fn tray_sits_between_apps_and_status() {
        let tray = parts(Some(2));
        assert_eq!(
            tray,
            [Part::Chevron, Part::Tray(0), Part::Tray(1), Part::Status, Part::Clock]
        );
        let row = RowLayout {
            apps: AppLayout { count: 2, pinned: 0 },
            order: DEFAULT_ORDER,
            tray: Some(2),
        };
        let chevron = row.part_left(Part::Chevron, 1.0).unwrap();
        assert_eq!(row.part_left(Part::Tray(0), 1.0), Some(chevron + CHEVRON_W));
        assert_eq!(row.part_hit(chevron + 1.0, 20.0, &tray, 1.0), Some(Part::Chevron));
        let status = row.part_left(Part::Status, 1.0).unwrap();
        assert_eq!(status, chevron + CHEVRON_W + 2.0 * TRAY_W + SEPARATOR);
        assert_eq!(row.part_hit(status - 1.0, 20.0, &tray, 1.0), None);
        // Inline icons are capped; with none, only the chevron shows.
        assert_eq!(parts(Some(9)).len(), 1 + MAX_INLINE + 2);
        assert_eq!(parts(Some(0)), [Part::Chevron, Part::Status, Part::Clock]);
        // Tray icons fit the dock's height budget.
        const { assert!(TRAY_ICON < HEIGHT && TRAY_ICON < TRAY_W) };
    }
    #[test]
    fn flyout_grid_wraps_at_five() {
        assert_eq!(flyout_size(0), flyout_size(1));
        assert_eq!(
            flyout_size(3),
            (2.0 * FLYOUT_PAD + 3.0 * FLYOUT_CELL, 2.0 * FLYOUT_PAD + FLYOUT_CELL)
        );
        assert_eq!(
            flyout_size(6),
            (
                2.0 * FLYOUT_PAD + 5.0 * FLYOUT_CELL,
                2.0 * FLYOUT_PAD + 2.0 * FLYOUT_CELL
            )
        );
        assert_eq!(flyout_cell(6), (FLYOUT_PAD + FLYOUT_CELL, FLYOUT_PAD + FLYOUT_CELL));
        let (x, y) = flyout_cell(6);
        assert_eq!(flyout_hit(x + 1.0, y + 1.0, 7), Some(6));
        assert_eq!(flyout_hit(x + 1.0, y + 1.0, 6), None);
        assert_eq!(flyout_hit(FLYOUT_PAD - 1.0, 10.0, 3), None);
        assert_eq!(flyout_hit(FLYOUT_PAD + 5.0 * FLYOUT_CELL + 1.0, 10.0, 9), None);
    }
}
