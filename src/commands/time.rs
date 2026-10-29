//! The time and date, here or in another city.
//!
//! Cities come from the Windows time zone list, whose names list cities, as
//! in "(UTC+09:00) Osaka, Sapporo, Tokyo". A few large cities that the list
//! omits are added.

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

/// Cities that the Windows zone names leave out, and the zone they use.
const MORE_CITIES: &[(&str, &str)] = &[
    ("New York", "Eastern Standard Time"),
    ("Boston", "Eastern Standard Time"),
    ("Toronto", "Eastern Standard Time"),
    ("Miami", "Eastern Standard Time"),
    ("Washington", "Eastern Standard Time"),
    ("Chicago", "Central Standard Time"),
    ("Dallas", "Central Standard Time"),
    ("Houston", "Central Standard Time"),
    ("Denver", "Mountain Standard Time"),
    ("Los Angeles", "Pacific Standard Time"),
    ("San Francisco", "Pacific Standard Time"),
    ("Seattle", "Pacific Standard Time"),
    ("Vancouver", "Pacific Standard Time"),
    ("Bengaluru", "India Standard Time"),
    ("Bangalore", "India Standard Time"),
    ("Delhi", "India Standard Time"),
    ("Hyderabad", "India Standard Time"),
    ("Pune", "India Standard Time"),
    ("Dubai", "Arabian Standard Time"),
    ("Shanghai", "China Standard Time"),
    ("San Jose", "Pacific Standard Time"),
    ("UTC", "UTC"),
    ("GMT", "UTC"),
];

pub struct Zone {
    pub info: DYNAMIC_TIME_ZONE_INFORMATION,
    /// Cities in the zone's name, such as ["Osaka", "Sapporo", "Tokyo"].
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
            let mut cities = cities(&display);
            cities.extend(
                MORE_CITIES
                    .iter()
                    .filter(|(_, k)| *k == key)
                    .map(|(city, _)| city.to_string()),
            );
            zones.push(Zone { info, cities });
        }
        zones
    })
}

/// Zones with a city that starts with `query`, and that city's name.
pub fn find(query: &str) -> Vec<(&'static Zone, &'static str)> {
    let query = query.trim().to_lowercase();
    if query.chars().count() < 2 {
        return Vec::new();
    }
    let mut found = Vec::new();
    for zone in zones() {
        if let Some(city) = zone.cities.iter().find(|city| city.to_lowercase().starts_with(&query)) {
            found.push((zone, city.as_str()));
        }
    }
    found.sort_by_key(|(_, city)| city.len());
    found.truncate(3);
    found
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
    fn finds_cities_in_the_windows_zone_list() {
        let found = find("tokyo");
        assert_eq!(found.first().map(|(_, city)| *city), Some("Tokyo"));
        assert_eq!(find("new york").first().map(|(_, city)| *city), Some("New York"));
        assert!(find("t").is_empty());
    }
}
