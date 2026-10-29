//! Windows Settings pages, opened with `ms-settings:` links.

/// (title, extra search words, link).
pub const PAGES: &[(&str, &str, &str)] = &[
    (
        "Display",
        "screen resolution scale monitor brightness",
        "ms-settings:display",
    ),
    ("Night light", "blue light", "ms-settings:nightlight"),
    ("Sound", "audio speakers microphone output input", "ms-settings:sound"),
    ("Notifications", "alerts", "ms-settings:notifications"),
    ("Focus", "do not disturb dnd", "ms-settings:quiethours"),
    ("Power & battery", "battery sleep energy", "ms-settings:powersleep"),
    ("Storage", "disk space cleanup", "ms-settings:storagesense"),
    ("Multitasking", "snap windows desktops", "ms-settings:multitasking"),
    ("Clipboard", "clipboard history", "ms-settings:clipboard"),
    ("About this PC", "about system specs rename", "ms-settings:about"),
    ("Bluetooth & devices", "bluetooth devices pair", "ms-settings:bluetooth"),
    ("Printers & scanners", "printer scanner", "ms-settings:printers"),
    ("Mouse", "pointer cursor", "ms-settings:mousetouchpad"),
    ("Touchpad", "trackpad gestures", "ms-settings:devices-touchpad"),
    ("Network & internet", "network ethernet internet", "ms-settings:network"),
    ("VPN", "vpn", "ms-settings:network-vpn"),
    (
        "Mobile hotspot",
        "hotspot tethering",
        "ms-settings:network-mobilehotspot",
    ),
    ("Proxy", "proxy", "ms-settings:network-proxy"),
    (
        "Background",
        "wallpaper personalization",
        "ms-settings:personalization-background",
    ),
    ("Colors", "dark mode light mode accent theme", "ms-settings:colors"),
    ("Themes", "theme", "ms-settings:themes"),
    ("Lock screen", "lock screen", "ms-settings:lockscreen"),
    ("Taskbar", "taskbar", "ms-settings:taskbar"),
    ("Fonts", "font", "ms-settings:fonts"),
    (
        "Installed apps",
        "uninstall programs apps features",
        "ms-settings:appsfeatures",
    ),
    ("Default apps", "default browser", "ms-settings:defaultapps"),
    ("Startup apps", "startup login", "ms-settings:startupapps"),
    ("Your info", "account profile", "ms-settings:yourinfo"),
    (
        "Sign-in options",
        "password pin windows hello",
        "ms-settings:signinoptions",
    ),
    ("Date & time", "clock time zone", "ms-settings:dateandtime"),
    (
        "Language & region",
        "language region keyboard layout",
        "ms-settings:regionlanguage",
    ),
    ("Typing", "autocorrect spelling", "ms-settings:typing"),
    ("Accessibility", "ease of access", "ms-settings:easeofaccess"),
    ("Privacy & security", "privacy permissions", "ms-settings:privacy"),
    ("Location", "location gps", "ms-settings:privacy-location"),
    ("Camera", "webcam", "ms-settings:privacy-webcam"),
    ("Microphone", "mic", "ms-settings:privacy-microphone"),
    ("Windows Security", "antivirus defender", "ms-settings:windowsdefender"),
    ("Windows Update", "updates", "ms-settings:windowsupdate"),
    ("Recovery", "reset", "ms-settings:recovery"),
    ("For developers", "developer mode", "ms-settings:developers"),
];

/// Pages where every query word starts a word of the title or its search
/// words, best first. Queries shorter than 3 characters match nothing.
pub fn find(query: &str) -> Vec<(&'static str, &'static str)> {
    let query = query.trim().to_lowercase();
    if query.chars().count() < 3 {
        return Vec::new();
    }
    let mut found: Vec<_> = PAGES
        .iter()
        .filter_map(|&(title, extra, link)| {
            let title_lower = title.to_lowercase();
            let words: Vec<&str> = title_lower
                .split(|c: char| !c.is_alphanumeric())
                .chain(extra.split(' '))
                .filter(|w| !w.is_empty())
                .collect();
            let all = query.split_whitespace().all(|q| words.iter().any(|w| w.starts_with(q)));
            // Titles that start with the query rank first.
            all.then_some((!title_lower.starts_with(&query), title, link))
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
    fn finds_pages_by_title_and_search_words() {
        assert_eq!(find("display")[0].0, "Display");
        assert_eq!(find("wallpaper")[0].0, "Background");
        assert_eq!(find("dark mode")[0].0, "Colors");
        assert_eq!(find("uninstall")[0].0, "Installed apps");
        assert!(find("di").is_empty());
        assert!(find("zzz").is_empty());
    }
}
