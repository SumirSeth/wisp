//! File search through [Everything](https://www.voidtools.com/) when it runs.
//!
//! Everything answers queries over window messages (`WM_COPYDATA`, SDK
//! version 2), which Everything 1.4 and 1.5 both support, so Wisp needs no DLL.
//! The query goes to Everything's window and the results come back as a
//! `WM_COPYDATA` sent to a message-only window on the file search thread.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WAIT_TIMEOUT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use super::Row;

/// Everything's IPC window, for the default instance of 1.4 and of 1.5.
const WINDOW_CLASSES: [PCWSTR; 2] = [
    w!("EVERYTHING_TASKBAR_NOTIFICATION"),
    w!("EVERYTHING_TASKBAR_NOTIFICATION_(1.5a)"),
];
const EVERYTHING_IPC_IS_DB_LOADED: usize = 401;
const EVERYTHING_IPC_COPYDATA_QUERY2W: usize = 18;
const REQUEST_NAME: u32 = 0x1;
const REQUEST_FULL_PATH_AND_NAME: u32 = 0x4;
const SORT_DATE_MODIFIED_DESCENDING: u32 = 14;
/// Item flags in a reply.
const ITEM_FOLDER: u32 = 0x1;
const ITEM_DRIVE: u32 = 0x2;
/// How long to wait for Everything to take a query, and then for its answer.
const SEND_MS: u32 = 500;
const REPLY_WITHIN: Duration = Duration::from_millis(1500);

thread_local! {
    /// The latest reply, with the id it answers.
    static REPLY: RefCell<Option<(usize, Vec<u8>)>> = const { RefCell::new(None) };
}

/// A message-only window that receives Everything's replies. Lives on the
/// file search thread, which must not change while it exists.
pub(super) struct Everything {
    reply: HWND,
    next_id: usize,
}

impl Everything {
    pub(super) fn new() -> Option<Everything> {
        unsafe {
            let instance = GetModuleHandleW(None).ok()?;
            let class = w!("Wisp.EverythingReply");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(reply_proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            // Fails harmlessly if the class is already registered.
            RegisterClassW(&wc);
            let reply = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance.into()),
                None,
            )
            .ok()?;
            // Everything often runs elevated; let its replies through.
            let _ = ChangeWindowMessageFilterEx(reply, WM_COPYDATA, MSGFLT_ALLOW, None);
            Some(Everything { reply, next_id: 1 })
        }
    }

    /// Asks Everything for up to `max` files and folders whose names contain
    /// every word, newest first. `None` if Everything isn't running, hasn't
    /// loaded its database yet, or doesn't answer in time.
    pub(super) fn query(&mut self, words: &[String], exclude: &[String], max: u32) -> Option<Vec<Row>> {
        let target = window()?;
        let mut is_loaded = 0usize;
        let sent = unsafe {
            SendMessageTimeoutW(
                target,
                WM_USER,
                WPARAM(EVERYTHING_IPC_IS_DB_LOADED),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                SEND_MS,
                Some(&mut is_loaded),
            )
        };
        if sent.0 == 0 || is_loaded == 0 {
            return None;
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let request = query2(self.reply, id as u32, &search(words, exclude), max);
        let data = COPYDATASTRUCT {
            dwData: EVERYTHING_IPC_COPYDATA_QUERY2W,
            cbData: request.len() as u32,
            lpData: request.as_ptr() as *mut _,
        };
        REPLY.with(|r| r.borrow_mut().take());
        let mut accepted = 0usize;
        // Not SMTO_BLOCK: Everything may send its reply while this waits.
        let sent = unsafe {
            SendMessageTimeoutW(
                target,
                WM_COPYDATA,
                WPARAM(self.reply.0 as usize),
                LPARAM(&data as *const _ as isize),
                SMTO_NORMAL | SMTO_ABORTIFHUNG,
                SEND_MS,
                Some(&mut accepted),
            )
        };
        if sent.0 == 0 || accepted == 0 {
            return None;
        }
        parse(&self.wait(id)?)
    }

    /// Pumps this thread's messages until the reply to `id` arrives.
    fn wait(&self, id: usize) -> Option<Vec<u8>> {
        let deadline = Instant::now() + REPLY_WITHIN;
        loop {
            // A late reply to an earlier query is dropped here.
            match REPLY.with(|r| r.borrow_mut().take()) {
                Some((got, bytes)) if got == id => return Some(bytes),
                _ => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, Some(self.reply), 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
                if REPLY.with(|r| r.borrow().is_some()) {
                    continue;
                }
                if MsgWaitForMultipleObjects(None, false, left.as_millis() as u32, QS_ALLINPUT) == WAIT_TIMEOUT {
                    return None;
                }
            }
        }
    }
}

impl Drop for Everything {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.reply);
        }
    }
}

fn window() -> Option<HWND> {
    WINDOW_CLASSES
        .iter()
        .find_map(|class| unsafe { FindWindowW(*class, PCWSTR::null()).ok() })
}

unsafe extern "system" fn reply_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA {
        let data = unsafe { (lp.0 as *const COPYDATASTRUCT).as_ref() };
        if let Some(data) = data {
            // Copy now: the pointer is only valid during this message.
            let bytes = if data.lpData.is_null() || data.cbData == 0 {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize).to_vec() }
            };
            REPLY.with(|r| *r.borrow_mut() = Some((data.dwData, bytes)));
            return LRESULT(1);
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Everything's search text: every word somewhere in the name, and no path
/// with an excluded folder or a file or folder whose name starts with a dot.
/// Like the Windows index, it also skips Windows folders at the root of a
/// drive, which `:\Windows\` matches on every drive, and the recycle bins.
/// A term with a backslash matches the full path.
fn search(words: &[String], exclude: &[String]) -> String {
    let clean = |text: &str| text.replace(['"', '*', '?'], "");
    let mut terms: Vec<String> = words
        .iter()
        .map(|word| clean(word))
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{word}\""))
        .collect();
    terms.push("!\"\\.\"".into());
    terms.push("!\":\\Windows\\\"".into());
    terms.push("!\"\\$Recycle.Bin\\\"".into());
    terms.extend(
        exclude
            .iter()
            .map(|folder| clean(folder))
            .filter(|folder| !folder.is_empty())
            .map(|folder| format!("!\"\\{folder}\\\"")),
    );
    terms.join(" ")
}

/// An `EVERYTHING_IPC_QUERY2` followed by the NUL-terminated search text.
fn query2(reply: HWND, id: u32, search: &str, max: u32) -> Vec<u8> {
    let header = [
        reply.0 as usize as u32,
        id,
        0, // search flags: no case, no whole word, no regex
        0, // offset
        max,
        REQUEST_NAME | REQUEST_FULL_PATH_AND_NAME,
        SORT_DATE_MODIFIED_DESCENDING,
    ];
    let mut bytes: Vec<u8> = header.iter().flat_map(|v| v.to_le_bytes()).collect();
    bytes.extend(search.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    bytes
}

/// Reads an `EVERYTHING_IPC_LIST2` with the name and full path of each item.
/// `None` if the reply is malformed or carries other fields.
fn parse(bytes: &[u8]) -> Option<Vec<Row>> {
    let dword =
        |at: usize| -> Option<u32> { Some(u32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?)) };
    let count = dword(4)? as usize;
    if dword(12)? != REQUEST_NAME | REQUEST_FULL_PATH_AND_NAME {
        return None;
    }
    // Reads a length, the UTF-16 text and its NUL; returns the text and the next offset.
    let text = |at: usize| -> Option<(String, usize)> {
        let len = dword(at)? as usize;
        let start = at + 4;
        let end = start.checked_add(len.checked_mul(2)?)?;
        let units: Vec<u16> = bytes
            .get(start..end)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        Some((String::from_utf16_lossy(&units), end + 2))
    };
    let mut rows = Vec::with_capacity(count.min(1000));
    for item in 0..count {
        let at = item.checked_mul(8)?.checked_add(20)?;
        let flags = dword(at)?;
        let (name, next) = text(dword(at + 4)? as usize)?;
        let (path, _) = text(next)?;
        rows.push(Row {
            name,
            path,
            is_folder: flags & (ITEM_FOLDER | ITEM_DRIVE) != 0,
        });
    }
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_search_text() {
        let words = vec!["bug".to_string(), "re\"p*o?rt".into(), "\"".into()];
        let exclude = vec!["node_modules".to_string(), "Code Cache".into()];
        assert_eq!(
            search(&words, &exclude),
            r#""bug" "report" !"\." !":\Windows\" !"\$Recycle.Bin\" !"\node_modules\" !"\Code Cache\""#
        );
    }

    #[test]
    fn builds_a_query2_request() {
        let bytes = query2(HWND(0x1234 as _), 7, "ab", 100);
        let dwords: Vec<u32> = bytes[..28]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        assert_eq!(dwords, [0x1234, 7, 0, 0, 100, 0x5, 14]);
        assert_eq!(&bytes[28..], &[b'a', 0, b'b', 0, 0, 0]);
    }

    /// Builds a reply the way Everything lays it out.
    fn list2(items: &[(u32, &str, &str)]) -> Vec<u8> {
        let mut head: Vec<u32> = vec![items.len() as u32, items.len() as u32, 0, 0x5, 14];
        let mut data = Vec::new();
        let base = 20 + items.len() * 8;
        let mut entries = Vec::new();
        for (flags, name, path) in items {
            entries.push((*flags, (base + data.len()) as u32));
            for text in [name, path] {
                let units: Vec<u16> = text.encode_utf16().collect();
                data.extend((units.len() as u32).to_le_bytes());
                data.extend(units.iter().chain([&0]).flat_map(|u| u.to_le_bytes()));
            }
        }
        for (flags, offset) in entries {
            head.extend([flags, offset]);
        }
        let mut bytes: Vec<u8> = head.iter().flat_map(|v| v.to_le_bytes()).collect();
        bytes.extend(data);
        bytes
    }

    #[test]
    fn parses_a_list2_reply() {
        let bytes = list2(&[
            (0, "bug-reporting.txt", r"D:\notes\bug-reporting.txt"),
            (ITEM_FOLDER, "Berichte", r"E:\Berichte"),
        ]);
        let rows = parse(&bytes).unwrap();
        let rows: Vec<(&str, &str, bool)> = rows
            .iter()
            .map(|r| (r.name.as_str(), r.path.as_str(), r.is_folder))
            .collect();
        assert_eq!(
            rows,
            [
                ("bug-reporting.txt", r"D:\notes\bug-reporting.txt", false),
                ("Berichte", r"E:\Berichte", true),
            ]
        );
    }

    #[test]
    fn rejects_malformed_replies() {
        let bytes = list2(&[(0, "a.txt", r"C:\a.txt")]);
        assert!(parse(&bytes[..bytes.len() - 6]).is_none());
        assert!(parse(&bytes[..10]).is_none());
        let mut other_fields = bytes.clone();
        other_fields[12] = 0x7;
        assert!(parse(&other_fields).is_none());
        assert_eq!(parse(&list2(&[])).unwrap().len(), 0);
    }
}
