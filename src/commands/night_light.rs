//! Night Light's CloudStore state, on demand.
//!
//! Windows does not document this value. A layout this module does not know
//! is never written over and never read as a state: the launcher then offers
//! the Windows Settings page instead of a toggle.
//!
//! Format and timestamp research: kvnxiao/win-nightlight-cli,
//! docs/nightlight-registry-format.md (Windows 11 builds 26100 and 26300).
//! This deliberately accepts only the known schema rather than guessing offsets.

use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows::Win32::System::Registry::*;
use windows::core::{PCWSTR, w};

const KEY: PCWSTR = w!(
    "Software\\Microsoft\\Windows\\CurrentVersion\\CloudStore\\Store\\DefaultAccount\\Current\\default$windows.data.bluelightreduction.bluelightreductionstate\\windows.data.bluelightreduction.bluelightreductionstate"
);
const HEADER: &[u8] = &[0x43, 0x42, 1, 0];
const PREFIX: &[u8] = &[0x43, 0x42, 1, 0, 0x0a, 2, 1, 0, 0x2a, 6];
const UNSUPPORTED: &str = "Wisp can't change Night Light on this PC. Use Settings > System > Display > Night light.";

#[derive(Debug, PartialEq)]
struct State {
    on: bool,
    modified: u64,
    usable: bool,
}

struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take(&mut self, expected: &[u8]) -> Option<()> {
        self.0 = self.0.strip_prefix(expected)?;
        Some(())
    }
    fn number(&mut self) -> Option<u64> {
        let mut result = 0;
        for shift in (0..70).step_by(7) {
            let (&byte, rest) = self.0.split_first()?;
            self.0 = rest;
            if shift == 63 && byte > 1 {
                return None;
            }
            result |= u64::from(byte & 127) << shift;
            if byte < 128 {
                // Reject noncanonical encodings as well as overflowing ones.
                return (shift == 0 || byte != 0).then_some(result);
            }
        }
        None
    }
}

fn number(mut value: u64, bytes: &mut Vec<u8>) {
    while value >= 128 {
        bytes.push(value as u8 | 128);
        value >>= 7;
    }
    bytes.push(value as u8);
}

fn parse(bytes: &[u8]) -> Option<State> {
    let mut outer = Reader(bytes);
    outer.take(PREFIX)?;
    let modified = outer.number()?;
    outer.take(&[0x2a, 0x2b, 0x0e])?;
    let len = usize::try_from(outer.number()?).ok()?;
    let (payload, end) = outer.0.split_at_checked(len)?;
    if end != [0, 0, 0] {
        return None;
    }
    let mut inner = Reader(payload);
    inner.take(HEADER)?;
    let on = inner.0.starts_with(&[0x10]);
    if on {
        inner.take(&[0x10, 0])?;
    }
    if inner.0.starts_with(&[0xd0, 0x0a]) {
        inner.take(&[0xd0, 0x0a])?;
        if !matches!(inner.number()?, 0 | 2) {
            return None;
        }
    }
    if inner.0.starts_with(&[0xc6, 0x14]) {
        inner.take(&[0xc6, 0x14])?;
        inner.number()?;
    }
    let mut usable = true;
    if inner.0.starts_with(&[0xc2, 0x1e]) {
        inner.take(&[0xc2, 0x1e])?;
        let (&value, rest) = inner.0.split_first()?;
        if value > 1 {
            return None;
        }
        usable = value != 0;
        inner.0 = rest;
    }
    (inner.0 == [0]).then_some(State { on, modified, usable })
}

fn encode(on: bool, modified: u64, filetime: u64) -> Vec<u8> {
    let mut payload = HEADER.to_vec();
    if on {
        payload.extend([0x10, 0]);
    }
    payload.extend([0xd0, 0x0a, 2, 0xc6, 0x14]); // Manual transition.
    number(filetime, &mut payload);
    payload.push(0);
    let mut bytes = PREFIX.to_vec();
    number(modified, &mut bytes);
    bytes.extend([0x2a, 0x2b, 0x0e]);
    number(payload.len() as u64, &mut bytes);
    bytes.extend(payload);
    bytes.extend([0, 0, 0]);
    bytes
}

fn next_stamp(previous: u64, highest: u64, now: u64) -> Option<u64> {
    Some(previous.max(highest).checked_add(2)?.max(now))
}

pub fn read() -> Option<bool> {
    let state = parse(&read_bytes().ok()?)?;
    state.usable.then_some(state.on)
}

fn read_bytes() -> Result<Vec<u8>, String> {
    let mut bytes = vec![0u8; 256];
    let mut size = bytes.len() as u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            KEY,
            w!("Data"),
            RRF_RT_REG_BINARY,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .map_err(|e| e.message())?;
    }
    bytes.truncate(size as usize);
    Ok(bytes)
}

/// The value that turns Night Light `on`, with its stamp; `None` if it is on
/// or off already. `now` is the time since the Unix epoch.
///
/// Fails for a layout `parse` does not know, and for a value that would not
/// read back as asked, so nothing guessed is ever written.
fn rewrite(original: &[u8], on: bool, highest: u64, now: Duration) -> Result<Option<(Vec<u8>, u64)>, &'static str> {
    let state = parse(original).filter(|s| s.usable).ok_or(UNSUPPORTED)?;
    if state.on == on {
        return Ok(None);
    }
    let modified = next_stamp(state.modified, highest, now.as_secs()).ok_or(UNSUPPORTED)?;
    let filetime = u64::try_from(now.as_nanos() / 100 + 116_444_736_000_000_000).map_err(|_| UNSUPPORTED)?;
    let bytes = encode(on, modified, filetime);
    let expected = State {
        on,
        modified,
        usable: true,
    };
    if parse(&bytes) != Some(expected) {
        return Err(UNSUPPORTED);
    }
    Ok(Some((bytes, modified)))
}

/// Called only by the commands worker, which retains its highest write stamp.
pub fn set(on: bool, highest: &mut u64) -> Result<(), String> {
    // A missing or oversized value is as unknown as an unknown layout.
    let original = read_bytes().map_err(|_| UNSUPPORTED)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    let Some((bytes, modified)) = rewrite(&original, on, *highest, now)? else {
        return Ok(());
    };
    // Do not overwrite a change made while this command was preparing its write.
    if read_bytes()? != original {
        return Err("Night Light changed. Try again.".into());
    }
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, KEY, None, KEY_SET_VALUE, &mut key)
            .ok()
            .map_err(|e| e.message())?;
        let result = RegSetValueExW(key, w!("Data"), None, REG_BINARY, Some(&bytes)).ok();
        let _ = RegCloseKey(key);
        result.map_err(|e| e.message())?;
    }
    *highest = modified;
    if read() != Some(on) {
        return Err("Windows did not keep the Night Light change.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stamps_above_stored_and_previous_writes_even_after_a_rewind() {
        assert_eq!(next_stamp(100, 90, 110), Some(110));
        assert_eq!(next_stamp(110, 90, 100), Some(112));
        assert_eq!(next_stamp(90, 110, 100), Some(112));
        assert_eq!(next_stamp(u64::MAX, 0, 100), None);
    }
    #[test]
    fn toggles_and_stamps_known_schema() {
        for on in [false, true] {
            let expected: &[u8] = if on {
                &[
                    0x43, 0x42, 1, 0, 0x0a, 2, 1, 0, 0x2a, 6, 100, 0x2a, 0x2b, 0x0e, 14, 0x43, 0x42, 1, 0, 0x10, 0,
                    0xd0, 0x0a, 2, 0xc6, 0x14, 0xc8, 1, 0, 0, 0, 0,
                ]
            } else {
                &[
                    0x43, 0x42, 1, 0, 0x0a, 2, 1, 0, 0x2a, 6, 100, 0x2a, 0x2b, 0x0e, 12, 0x43, 0x42, 1, 0, 0xd0, 0x0a,
                    2, 0xc6, 0x14, 0xc8, 1, 0, 0, 0, 0,
                ]
            };
            let bytes = expected.to_vec();
            assert_eq!(encode(on, 100, 200), expected);
            assert_eq!(
                parse(&bytes),
                Some(State {
                    on,
                    modified: 100,
                    usable: true
                })
            );
            for len in 0..bytes.len() {
                assert!(parse(&bytes[..len]).is_none());
            }
            let mut unknown = bytes.clone();
            unknown.insert(unknown.len() - 4, 0x42);
            unknown[14] += 1; // Keep the payload length valid, so the unknown field is rejected.
            assert!(parse(&unknown).is_none());
        }
    }
    #[test]
    fn never_rewrites_an_unknown_or_unusable_value() {
        let now = Duration::from_secs(1_800_000_100);
        let known = encode(false, 1_800_000_000, 134_000_000_000_000_000);
        // A known value turns on with a newer stamp, and is left alone when already so.
        let (bytes, stamp) = rewrite(&known, true, 0, now).unwrap().unwrap();
        assert_eq!(stamp, 1_800_000_100);
        assert_eq!(
            parse(&bytes),
            Some(State {
                on: true,
                modified: stamp,
                usable: true
            })
        );
        assert_eq!(rewrite(&known, false, 0, now), Ok(None));

        // An unknown field, with the payload length kept valid.
        let mut field = known.clone();
        field.insert(field.len() - 4, 0x42);
        field[18] += 1;
        // An unknown transition value.
        let mut transition = known.clone();
        assert_eq!(transition[25], 2);
        transition[25] = 3;
        // A newer header version.
        let mut version = known.clone();
        version[2] = 2;
        // Bytes after the known end.
        let mut longer = known.clone();
        longer.push(0);
        // Windows says Night Light is unavailable.
        let mut unavailable = known.clone();
        let index = unavailable.len() - 4;
        unavailable.splice(index..index, [0xc2, 0x1e, 0]);
        unavailable[18] += 3;
        assert!(parse(&unavailable).is_some_and(|s| !s.usable));

        let unknown: [&[u8]; 9] = [
            &[],
            &[0],
            &[0xff; 64],
            &known[..known.len() - 1],
            &field,
            &transition,
            &version,
            &longer,
            &unavailable,
        ];
        for bytes in unknown {
            for on in [false, true] {
                assert_eq!(rewrite(bytes, on, 0, now), Err(UNSUPPORTED), "{bytes:x?}");
            }
        }
        // A stamp that cannot rise is refused as well.
        let last = encode(false, u64::MAX, 0);
        assert_eq!(rewrite(&last, true, 0, now), Err(UNSUPPORTED));
    }
    #[test]
    fn reads_captured_26300_state_and_rejects_unknown_layouts() {
        let bytes = [
            0x43, 0x42, 1, 0, 0x0a, 2, 1, 0, 0x2a, 6, 0xb3, 0x83, 0xdc, 0xd6, 6, 0x2a, 0x2b, 0x0e, 0x13, 0x43, 0x42, 1,
            0, 0xd0, 0x0a, 2, 0xc6, 0x14, 0xef, 0x8a, 0x83, 0xf5, 0xd5, 0xe3, 0xf1, 0xed, 1, 0, 0, 0, 0,
        ];
        assert!(!parse(&bytes).unwrap().on);
        let mut broken = bytes;
        broken[18] += 1;
        assert!(parse(&broken).is_none());
        assert!(Reader(&[0xff; 10]).number().is_none());
        assert!(Reader(&[0x80, 0]).number().is_none());
        let mut unavailable = encode(false, 0, 0);
        let index = unavailable.len() - 4;
        unavailable.splice(index..index, [0xc2, 0x1e, 0]);
        unavailable[14] += 3;
        assert!(!parse(&unavailable).unwrap().usable);
    }
}
