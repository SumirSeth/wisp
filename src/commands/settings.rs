//! Windows Settings pages, opened with `ms-settings:` links.

use std::sync::OnceLock;

use windows::Win32::UI::Shell::SHLoadIndirectString;
use windows::core::HSTRING;

/// (English title, extra search words, link, Windows resource path).
/// These are resource identifiers, not translations. Older page IDs follow
/// newer ones after `|`. Use the Settings UI's DisplayName, rather than the
/// search catalog's longer Description.
const PAGES: &[(&str, &str, &str, &str)] = &[
    (
        "Display",
        "screen resolution scale monitor brightness",
        "ms-settings:display",
        "SettingsPagePCSystemDisplay/DisplayName",
    ),
    (
        "Night light",
        "blue light",
        "ms-settings:nightlight",
        "SystemSettings_Display_BlueLight_PageTitle",
    ),
    (
        "Sound",
        "audio speakers microphone output input",
        "ms-settings:sound",
        "SettingsPageAudio/DisplayName",
    ),
    (
        "Notifications",
        "alerts",
        "ms-settings:notifications",
        "SettingsPageAppsNotifications2/DisplayName|SettingsPageAppsNotifications/DisplayName",
    ),
    (
        "Focus",
        "do not disturb dnd",
        "ms-settings:quiethours",
        "SettingsPageFocus/DisplayName",
    ),
    (
        "Power & battery",
        "battery sleep energy",
        "ms-settings:powersleep",
        "SettingsPagePowerAndBattery/DisplayName|SettingsPageScreenPowerAndSleep/DisplayName",
    ),
    (
        "Storage",
        "disk space cleanup",
        "ms-settings:storagesense",
        "SettingsPageStorageSenseStorageOverview/DisplayName",
    ),
    (
        "Multitasking",
        "snap windows desktops",
        "ms-settings:multitasking",
        "SettingsPageMultiTasking/DisplayName",
    ),
    (
        "Clipboard",
        "clipboard history",
        "ms-settings:clipboard",
        "SettingsPageClipboard/DisplayName",
    ),
    (
        "About this PC",
        "about system specs rename",
        "ms-settings:about",
        "SettingsPageAbout/DisplayName",
    ),
    (
        "Bluetooth & devices",
        "bluetooth devices pair",
        "ms-settings:bluetooth",
        "SettingsPageGroupDevices/DisplayName",
    ),
    (
        "Printers & scanners",
        "printer scanner",
        "ms-settings:printers",
        "SettingsPageDevicesPrinters/DisplayName",
    ),
    (
        "Mouse",
        "pointer cursor",
        "ms-settings:mousetouchpad",
        "SettingsPagePCSystemDeviceSettings/DisplayName",
    ),
    (
        "Touchpad",
        "trackpad gestures",
        "ms-settings:devices-touchpad",
        "SettingsPageDevicesTouchpad/DisplayName",
    ),
    (
        "Network & internet",
        "network ethernet internet",
        "ms-settings:network",
        "SettingsPageGroupNetwork/DisplayName",
    ),
    (
        "VPN",
        "vpn",
        "ms-settings:network-vpn",
        "SettingsPageNetworkVPN/DisplayName",
    ),
    (
        "Mobile hotspot",
        "hotspot tethering",
        "ms-settings:network-mobilehotspot",
        "SettingsPageNetworkMobileHotspot/DisplayName",
    ),
    (
        "Proxy",
        "proxy",
        "ms-settings:network-proxy",
        "SettingsPageNetworkProxy/DisplayName",
    ),
    (
        "Background",
        "wallpaper personalization",
        "ms-settings:personalization-background",
        "SettingsPageBackground/DisplayName",
    ),
    (
        "Colors",
        "dark mode light mode accent theme",
        "ms-settings:colors",
        "SettingsPageColors/DisplayName",
    ),
    (
        "Themes",
        "theme",
        "ms-settings:themes",
        "SettingsPageThemes/DisplayName",
    ),
    (
        "Lock screen",
        "lock screen",
        "ms-settings:lockscreen",
        "SettingsPageLockScreen/DisplayName",
    ),
    (
        "Taskbar",
        "taskbar",
        "ms-settings:taskbar",
        "SettingsPageTaskbar/DisplayName",
    ),
    ("Fonts", "font", "ms-settings:fonts", "SettingsPageFonts/DisplayName"),
    (
        "Installed apps",
        "uninstall programs apps features",
        "ms-settings:appsfeatures",
        "SettingsPageInstalledApps/DisplayName|SettingsPageAppsSizes/DisplayName",
    ),
    (
        "Default apps",
        "default browser",
        "ms-settings:defaultapps",
        "SettingsPageAppsDefaults/DisplayName",
    ),
    (
        "Startup apps",
        "startup login",
        "ms-settings:startupapps",
        "SettingsPageStartup/DisplayName",
    ),
    (
        "Your info",
        "account profile",
        "ms-settings:yourinfo",
        "SettingsPageAccountsPicture/DisplayName",
    ),
    (
        "Sign-in options",
        "password pin windows hello",
        "ms-settings:signinoptions",
        "SettingsPageSignInOptions/DisplayName",
    ),
    (
        "Date & time",
        "clock time zone",
        "ms-settings:dateandtime",
        "SettingsPageTimeRegionDateTime/DisplayName",
    ),
    (
        "Language & region",
        "language region keyboard layout",
        "ms-settings:regionlanguage",
        "SettingsPageTimeRegionLanguage/DisplayName",
    ),
    (
        "Typing",
        "autocorrect spelling",
        "ms-settings:typing",
        "SettingsPageTimeRegionSpelling/DisplayName",
    ),
    (
        "Accessibility",
        "ease of access",
        "ms-settings:easeofaccess",
        "SettingsPageGroupEaseOfAccess/DisplayName",
    ),
    (
        "Privacy & security",
        "privacy permissions",
        "ms-settings:privacy",
        "SettingsPageGroupPrivacy/DisplayName",
    ),
    (
        "Location",
        "location gps",
        "ms-settings:privacy-location",
        "SettingsPagePrivacyLocation/DisplayName",
    ),
    (
        "Camera",
        "webcam",
        "ms-settings:privacy-webcam",
        "SettingsPagePrivacyWebcam/DisplayName",
    ),
    (
        "Microphone",
        "mic",
        "ms-settings:privacy-microphone",
        "SettingsPagePrivacyMicrophone/DisplayName",
    ),
    (
        "Windows Security",
        "antivirus defender",
        "ms-settings:windowsdefender",
        "SettingsPageWindowsDefender/DisplayName",
    ),
    (
        "Windows Update",
        "updates",
        "ms-settings:windowsupdate",
        "SettingsPageRestoreMusUpdate/DisplayName",
    ),
    (
        "Recovery",
        "reset",
        "ms-settings:recovery",
        "SettingsPageRestoreRestore/DisplayName",
    ),
    (
        "For developers",
        "developer mode",
        "ms-settings:developers",
        "SettingsPageRestoreDeveloperOptions/DisplayName",
    ),
];

struct Page {
    title: String,
    english: &'static str,
    extra: &'static str,
    link: &'static str,
}

static LOCALIZED: OnceLock<Vec<Page>> = OnceLock::new();

/// Called once on the command worker, away from startup and typing on the UI.
pub fn preload() {
    LOCALIZED.get_or_init(|| load_pages(localized_name));
}

fn pages() -> &'static [Page] {
    static ENGLISH: OnceLock<Vec<Page>> = OnceLock::new();
    LOCALIZED
        .get()
        .unwrap_or_else(|| ENGLISH.get_or_init(|| load_pages(|_| None)))
}

fn load_pages(mut resolve: impl FnMut(&str) -> Option<String>) -> Vec<Page> {
    PAGES
        .iter()
        .map(|&(english, extra, link, resource)| Page {
            title: resource
                .split('|')
                .find_map(&mut resolve)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| english.into()),
            english,
            extra,
            link,
        })
        .collect()
}

/// The Settings PRI moved on newer Windows 11. The XML search catalog still
/// uses the `windows` resource host on both versions; SHLoadIndirectString
/// resolves it using the Shell's language context. No XML scan is needed.
fn localized_name(resource: &str) -> Option<String> {
    let source = HSTRING::from(format!(
        "@{{windows?ms-resource://Windows.UI.SettingsAppThreshold/SystemSettings/Resources/{resource}}}"
    ));
    let mut buffer = [0u16; 1024];
    unsafe {
        SHLoadIndirectString(&source, &mut buffer, None).ok()?;
    }
    resource_text(&buffer)
}

fn resource_text(buffer: &[u16]) -> Option<String> {
    let end = buffer.iter().position(|&c| c == 0)?;
    let text = String::from_utf16(&buffer[..end]).ok()?;
    (!text.trim().is_empty() && !text.starts_with('@') && !text.starts_with("ms-resource:")).then_some(text)
}

/// Pages matching every query word in either language, best first.
/// Queries shorter than 3 characters match nothing.
pub fn find(query: &str) -> Vec<(&'static str, &'static str)> {
    if query.trim().chars().count() < 3 {
        return Vec::new();
    }
    find_in(pages(), query)
}

fn find_in<'a>(pages: &'a [Page], query: &str) -> Vec<(&'a str, &'static str)> {
    let query = query.trim().to_lowercase();
    if query.chars().count() < 3 {
        return Vec::new();
    }
    let mut found: Vec<_> = pages
        .iter()
        .filter_map(|page| {
            let title = page.title.to_lowercase();
            let english = page.english.to_lowercase();
            let words: Vec<_> = title
                .split(|c: char| !c.is_alphanumeric())
                .chain(english.split(|c: char| !c.is_alphanumeric()))
                .chain(page.extra.split(' '))
                .filter(|w| !w.is_empty())
                .collect();
            query
                .split_whitespace()
                .all(|q| words.iter().any(|w| w.starts_with(q)))
                .then_some((
                    !(title.starts_with(&query) || english.starts_with(&query)),
                    page.title.as_str(),
                    page.link,
                ))
        })
        .collect();
    found.sort();
    found
        .into_iter()
        .take(3)
        .map(|(_, title, link)| (title, link))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_localized_and_english_names_and_keywords() {
        let pages = load_pages(|key| match key {
            "SettingsPagePCSystemDisplay/DisplayName" => Some("Bildschirm".into()),
            "SystemSettings_Display_BlueLight_PageTitle" => Some("Nachtmodus".into()),
            "SettingsPageColors/DisplayName" => Some("Farben".into()),
            _ => None,
        });
        for (query, expected) in [
            (" BILDSCHIRM ", ("Bildschirm", "ms-settings:display")),
            ("display", ("Bildschirm", "ms-settings:display")),
            ("resolution", ("Bildschirm", "ms-settings:display")),
            ("bild resolution", ("Bildschirm", "ms-settings:display")),
            ("nacht", ("Nachtmodus", "ms-settings:nightlight")),
            ("blue light", ("Nachtmodus", "ms-settings:nightlight")),
            ("farben", ("Farben", "ms-settings:colors")),
            ("dark mode", ("Farben", "ms-settings:colors")),
            ("wallpaper", ("Background", "ms-settings:personalization-background")),
            ("uninstall", ("Installed apps", "ms-settings:appsfeatures")),
        ] {
            assert_eq!(find_in(&pages, query), [expected], "{query}");
        }
        assert!(find_in(&pages, "bi").is_empty());
        assert!(find_in(&pages, "zzz").is_empty());
        assert!(find_in(&pages, "screen wallpaper").is_empty());
        assert_eq!(find_in(&pages, "win").len(), 3);
    }

    #[test]
    fn empty_names_fall_back_to_english() {
        let pages = load_pages(|_| Some("  ".into()));
        assert_eq!(find_in(&pages, "display"), [("Display", "ms-settings:display")]);
    }

    #[test]
    fn uses_older_page_resources_when_newer_ones_are_missing() {
        let pages = load_pages(|key| match key {
            "SettingsPageScreenPowerAndSleep/DisplayName" => Some("Netzbetrieb und Energiesparen".into()),
            "SettingsPageAppsSizes/DisplayName" => Some("Apps und Features".into()),
            "SettingsPageAppsNotifications/DisplayName" => Some("Benachrichtigungen".into()),
            _ => None,
        });
        assert_eq!(
            find_in(&pages, "energie"),
            [("Netzbetrieb und Energiesparen", "ms-settings:powersleep")]
        );
        assert_eq!(
            find_in(&pages, "uninstall"),
            [("Apps und Features", "ms-settings:appsfeatures")]
        );
        assert_eq!(
            find_in(&pages, "alerts"),
            [("Benachrichtigungen", "ms-settings:notifications")]
        );
    }

    #[test]
    fn reads_only_complete_resolved_utf16_strings() {
        assert_eq!(
            resource_text(&[70, 228, 114, 98, 101, 110, 0, 88]),
            Some("F\u{e4}rben".into())
        );
        assert_eq!(resource_text(&[0]), None);
        assert_eq!(resource_text(&[32, 0]), None);
        assert_eq!(resource_text(&[64, 123, 0]), None);
        assert_eq!(resource_text(&[0xd800, 0]), None);
        assert_eq!(resource_text(&[65, 66]), None);
    }

    #[test]
    #[ignore = "reads installed Windows Settings resources; run manually"]
    fn reads_windows_settings_names() {
        let started = std::time::Instant::now();
        preload();
        let pages = pages();
        for (page, &(_, _, _, resource)) in pages.iter().zip(PAGES) {
            let name = resource.split('|').find_map(localized_name);
            println!("{} -> {:?}", page.english, name);
            assert!(name.is_some(), "{resource}");
        }
        println!("Settings names loaded and checked in {:?}", started.elapsed());
    }
}
