//! Status placements shared with Settings; canonical order is the matrix's home cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusItem {
    Sound,
    Battery,
    Wifi,
    Bluetooth,
}
pub const STATUS_ITEMS: [StatusItem; 4] = [
    StatusItem::Sound,
    StatusItem::Battery,
    StatusItem::Wifi,
    StatusItem::Bluetooth,
];
impl StatusItem {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sound => "sound",
            Self::Battery => "battery",
            Self::Wifi => "wifi",
            Self::Bluetooth => "bluetooth",
        }
    }
    pub fn settings(self) -> &'static str {
        match self {
            Self::Sound => "ms-settings:sound",
            Self::Battery => "ms-settings:batterysaver",
            Self::Wifi => "ms-settings:network-wifi",
            Self::Bluetooth => "ms-settings:bluetooth",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusPlacement {
    pub group: [bool; 4],
    pub icons: [bool; 4],
}
impl Default for StatusPlacement {
    fn default() -> Self {
        Self {
            group: [true; 4],
            icons: [false; 4],
        }
    }
}
impl StatusPlacement {
    pub fn new(group: &[String], icons: &[String]) -> Self {
        let group = STATUS_ITEMS.map(|item| group.iter().any(|name| name == item.name()));
        let icons = std::array::from_fn(|i| !group[i] && icons.iter().any(|name| name == STATUS_ITEMS[i].name()));
        Self { group, icons }
    }
    pub fn visible(self, item: StatusItem) -> bool {
        self.group[item as usize] || self.icons[item as usize]
    }
    pub fn names(self, group: bool) -> Vec<String> {
        STATUS_ITEMS
            .into_iter()
            .filter(|item| {
                if group {
                    self.group[*item as usize]
                } else {
                    self.icons[*item as usize]
                }
            })
            .map(|item| item.name().into())
            .collect()
    }
}
