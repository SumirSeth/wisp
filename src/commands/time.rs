//! The time and date, here or in another city.
//!
//! Cities come from both the localized Windows time zone list and a built-in
//! English list. A few large cities that Windows omits are added too.

mod english_cities;

use std::sync::OnceLock;

use windows::Win32::{
    Foundation::SYSTEMTIME,
    System::{
        Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW},
        SystemInformation::{GetLocalTime, GetSystemTime},
        Time::{DYNAMIC_TIME_ZONE_INFORMATION, EnumDynamicTimeZoneInformation, SystemTimeToTzSpecificLocalTimeEx},
    },
};
use windows::core::{HSTRING, PCWSTR, w};

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub struct Zone {
    pub info: DYNAMIC_TIME_ZONE_INFORMATION,
    /// Localized and English names, such as ["Osaka", "Sapporo", "Tokio", "Tokyo"].
    pub cities: Vec<String>,
}

/// A time somewhere: hours and minutes, the day, and the offset from here.
#[derive(Debug, Clone, PartialEq)]
pub struct Clock {
    pub hour: u16,
    pub minute: u16,
    pub weekday: u16,
    pub day: u16,
    pub month: u16,
    pub year: u16,
    /// Minutes ahead of local time; negative if behind.
    pub ahead: i64,
}

impl Clock {
    pub fn time(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    /// "Thursday 29 Oct".
    pub fn short_date(&self) -> String {
        format!(
            "{} {} {}",
            WEEKDAYS[self.weekday as usize % 7],
            self.day,
            &MONTHS[(self.month as usize).clamp(1, 12) - 1][..3]
        )
    }

    /// "Thursday, 29 October 2026".
    pub fn long_date(&self) -> String {
        format!(
            "{}, {} {} {}",
            WEEKDAYS[self.weekday as usize % 7],
            self.day,
            MONTHS[(self.month as usize).clamp(1, 12) - 1],
            self.year
        )
    }

    /// "3 h 30 min ahead of you", "1 h behind you", "Same time as you".
    pub fn difference(&self) -> String {
        if self.ahead == 0 {
            return "Same time as you".into();
        }
        let minutes = self.ahead.unsigned_abs();
        let (h, m) = (minutes / 60, minutes % 60);
        let amount = match (h, m) {
            (0, m) => format!("{m} min"),
            (h, 0) => format!("{h} h"),
            (h, m) => format!("{h} h {m} min"),
        };
        if self.ahead > 0 {
            format!("{amount} ahead of you")
        } else {
            format!("{amount} behind you")
        }
    }
}

fn clock(time: SYSTEMTIME, local: &SYSTEMTIME) -> Clock {
    Clock {
        hour: time.wHour,
        minute: time.wMinute,
        weekday: time.wDayOfWeek,
        day: time.wDay,
        month: time.wMonth,
        year: time.wYear,
        ahead: minutes(&time) - minutes(local),
    }
}

/// Minutes since an arbitrary epoch, to compare two clock readings.
fn minutes(t: &SYSTEMTIME) -> i64 {
    // Days from civil date, Howard Hinnant's algorithm.
    let (y, m, d) = (t.wYear as i64, t.wMonth as i64, t.wDay as i64);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe;
    days * 1440 + t.wHour as i64 * 60 + t.wMinute as i64
}

pub fn local() -> Clock {
    let now = unsafe { GetLocalTime() };
    clock(now, &now)
}

pub fn in_zone(zone: &Zone) -> Option<Clock> {
    unsafe {
        let utc = GetSystemTime();
        let local = GetLocalTime();
        let mut there = SYSTEMTIME::default();
        SystemTimeToTzSpecificLocalTimeEx(Some(&zone.info), &utc, &mut there).ok()?;
        Some(clock(there, &local))
    }
}

pub fn zones() -> &'static [Zone] {
    static ZONES: OnceLock<Vec<Zone>> = OnceLock::new();
    ZONES.get_or_init(|| {
        let mut zones = Vec::new();
        for index in 0.. {
            let mut info = DYNAMIC_TIME_ZONE_INFORMATION::default();
            if unsafe { EnumDynamicTimeZoneInformation(index, &mut info) } != 0 {
                break;
            }
            let key = utf16(&info.TimeZoneKeyName);
            let display = display_name(&key).unwrap_or_default();
            let cities = zone_cities(&key, &display);
            zones.push(Zone { info, cities });
        }
        zones
    })
}

/// Zones with a city that starts with `query`, and that city's name.
pub fn find(query: &str) -> Vec<(&'static Zone, &'static str)> {
    find_in(zones(), query)
}

fn find_in<'a>(zones: &'a [Zone], query: &str) -> Vec<(&'a Zone, &'a str)> {
    let query = query.trim().to_lowercase();
    if query.chars().count() < 2 {
        return Vec::new();
    }
    let mut found = Vec::new();
    for zone in zones {
        if let Some(city) = zone.cities.iter().find(|city| city.to_lowercase().starts_with(&query)) {
            found.push((zone, city.as_str()));
        }
    }
    found.sort_by_key(|(_, city)| city.len());
    found.truncate(3);
    found
}

/// Keep localized spellings first when a prefix matches both languages.
fn zone_cities(key: &str, display: &str) -> Vec<String> {
    let mut names = cities(display);
    if let Some((_, english)) = english_cities::CITIES.iter().find(|(k, _)| *k == key) {
        names.extend(cities(english));
    }
    let mut seen = std::collections::HashSet::new();
    names.retain(|name| seen.insert(name.to_lowercase()));
    names
}

fn display_name(key: &str) -> Option<String> {
    let path = HSTRING::from(format!(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Time Zones\{key}"
    ));
    let mut buffer = [0u16; 256];
    let mut size = (buffer.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            w!("Display"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    Some(utf16(&buffer))
}

/// "(UTC+09:00) Osaka, Sapporo, Tokyo" → ["Osaka", "Sapporo", "Tokyo"].
/// Region names in parentheses, as in "Eastern Time (US & Canada)", are kept whole.
fn cities(display: &str) -> Vec<String> {
    let rest = display.split_once(") ").map(|(_, rest)| rest).unwrap_or(display);
    rest.split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn utf16(units: &[u16]) -> String {
    let len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..len])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(year: u16, month: u16, day: u16, hour: u16, minute: u16) -> SYSTEMTIME {
        SYSTEMTIME {
            wYear: year,
            wMonth: month,
            wDay: day,
            wHour: hour,
            wMinute: minute,
            wDayOfWeek: 4,
            ..Default::default()
        }
    }

    #[test]
    fn describes_the_difference_across_midnight() {
        let local = at(2026, 10, 28, 22, 44);
        let tokyo = clock(at(2026, 10, 29, 2, 14), &local);
        assert_eq!(tokyo.difference(), "3 h 30 min ahead of you");
        assert_eq!(tokyo.time(), "02:14");
        let london = clock(at(2026, 10, 28, 17, 14), &local);
        assert_eq!(london.difference(), "5 h 30 min behind you");
        assert_eq!(clock(local, &local).difference(), "Same time as you");
        let year_end = clock(at(2027, 1, 1, 0, 0), &at(2026, 12, 31, 23, 0));
        assert_eq!(year_end.difference(), "1 h ahead of you");
    }

    #[test]
    fn reads_cities_from_zone_names() {
        assert_eq!(
            cities("(UTC+09:00) Osaka, Sapporo, Tokyo"),
            ["Osaka", "Sapporo", "Tokyo"]
        );
        assert_eq!(
            cities("(UTC-05:00) Eastern Time (US & Canada)"),
            ["Eastern Time (US & Canada)"]
        );
    }

    #[test]
    fn finds_english_and_localized_names_in_german_zones() {
        let zones = [
            ("Tokyo Standard Time", "(UTC+09:00) Osaka, Sapporo, Tokio"),
            (
                "W. Europe Standard Time",
                "(UTC+01:00) Amsterdam, Berlin, Bern, Rom, Stockholm, Wien",
            ),
            ("Eastern Standard Time", "(UTC-05:00) Eastern Zeit (USA & Kanada)"),
        ]
        .map(|(key, display)| Zone {
            info: DYNAMIC_TIME_ZONE_INFORMATION::default(),
            cities: zone_cities(key, display),
        });

        for (query, expected, index) in [
            (" TOKYO ", "Tokyo", 0),
            ("tokio", "Tokio", 0),
            ("rome", "Rome", 1),
            ("vienna", "Vienna", 1),
            ("wien", "Wien", 1),
            ("new york", "New York", 2),
            ("eastern zeit", "Eastern Zeit (USA & Kanada)", 2),
        ] {
            let found = find_in(&zones, query);
            assert_eq!(found.len(), 1, "{query}");
            assert_eq!(found[0].1, expected);
            assert!(std::ptr::eq(found[0].0, &zones[index]));
        }
        // Shared names and prefixes still produce just one row per zone.
        assert_eq!(find_in(&zones, "osaka").len(), 1);
        assert_eq!(find_in(&zones, "tok").len(), 1);
        assert_eq!(zones[0].cities, ["Osaka", "Sapporo", "Tokio", "Tokyo"]);
        assert!(find_in(&zones, "t").is_empty());
    }

    #[test]
    fn merges_names_without_case_duplicates_and_keeps_unknown_zones() {
        assert_eq!(
            zone_cities("Tokyo Standard Time", "(UTC+09:00) OSAKA, Sapporo, Tokyo, Tokyo"),
            ["OSAKA", "Sapporo", "Tokyo"]
        );
        assert_eq!(zone_cities("Future Zone", "(UTC+01:00) Local City"), ["Local City"]);
        assert_eq!(zone_cities("Tokyo Standard Time", ""), ["Osaka", "Sapporo", "Tokyo"]);
    }

    #[test]
    fn english_table_has_unique_keys_and_names() {
        let mut keys = std::collections::HashSet::new();
        for (key, names) in english_cities::CITIES {
            assert!(keys.insert(key), "duplicate zone: {key}");
            let names = cities(names);
            assert!(!names.is_empty(), "{key}");
            let unique: std::collections::HashSet<_> = names.iter().map(|name| name.to_lowercase()).collect();
            assert_eq!(names.len(), unique.len(), "duplicate city in {key}");
        }
    }

    #[test]
    fn finds_cities_in_the_windows_zone_list() {
        // These English names come from our table on every Windows display language.
        let found = find("tokyo");
        assert_eq!(found.first().map(|(_, city)| *city), Some("Tokyo"));
        assert_eq!(find("vienna").first().map(|(_, city)| *city), Some("Vienna"));
        assert_eq!(find("new york").first().map(|(_, city)| *city), Some("New York"));
        assert!(find("t").is_empty());
    }
}
