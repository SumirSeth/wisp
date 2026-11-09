//! Only the icons used by Settings and its GPUI components.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

// Keep the original gpui-kit-assets SVGs unchanged. In addition to the sidebar,
// Select uses ChevronDown, Check and Inbox, Input uses Close, and the updater's
// loading buttons use Spinner's Loader. Sliders, switches and scrollbars draw
// their own shapes and do not load icons. See assets/icons/LICENSE-LUCIDE.
const ICONS: &[(&str, &[u8])] = &[
    ("icons/info.svg", include_bytes!("../assets/icons/info.svg")),
    ("icons/palette.svg", include_bytes!("../assets/icons/palette.svg")),
    ("icons/search.svg", include_bytes!("../assets/icons/search.svg")),
    ("icons/settings.svg", include_bytes!("../assets/icons/settings.svg")),
    (
        "icons/chevron-down.svg",
        include_bytes!("../assets/icons/chevron-down.svg"),
    ),
    ("icons/check.svg", include_bytes!("../assets/icons/check.svg")),
    ("icons/close.svg", include_bytes!("../assets/icons/close.svg")),
    ("icons/inbox.svg", include_bytes!("../assets/icons/inbox.svg")),
    ("icons/loader.svg", include_bytes!("../assets/icons/loader.svg")),
];

pub struct SettingsAssets;

impl AssetSource for SettingsAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, data)| Cow::Borrowed(*data)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| (*name).into())
            .collect())
    }
}
