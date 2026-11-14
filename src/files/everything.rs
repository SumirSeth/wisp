//! File search through [Everything](https://www.voidtools.com/) when it runs.
//!
//! Everything answers queries over window messages (`WM_COPYDATA`, SDK
//! version 2), which Everything 1.4 and 1.5 both support, so Wisp needs no DLL.
//! The query goes to Everything's window and the results come back as a
//! `WM_COPYDATA` sent to a message-only window on a thread of its own.
//!
//! Everything sends each reply from its window thread and waits up to about
//! 3 s for it to be taken, answering nothing else meanwhile. So the reply
//! window always pumps messages, even after Wisp stopped waiting, and drops
//! replies it doesn't expect.

use std::cell::RefCell;
use std::hash::{BuildHasher, RandomState};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
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
/// How long a query may take, from asking to the reply. Longer means Everything
/// is busy, such as during a rescan.
const ANSWER_WITHIN: Duration = Duration::from_millis(400);
/// How long to use the index instead after Everything didn't answer in time.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Longest name or path read from a reply, in UTF-16 units.
const MAX_TEXT_UNITS: usize = 32767;
/// Longest wait before asking again after repeated failures.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);
/// Largest reply accepted. 500 rows with long paths take well under 1 MB.
const MAX_REPLY_BYTES: usize = 4 << 20;

/// What the reply window's thread needs to pass replies on.
struct Replies {
    /// The id of the query being waited for, or 0.
    pending: Arc<AtomicU32>,
    deliver: Box<dyn Fn(u32, Vec<u8>)>,
}

thread_local! {
    /// Set on the reply window's thread only.
    static REPLIES: RefCell<Option<Replies>> = const { RefCell::new(None) };
}

/// Asks Everything and keeps the window its replies arrive at. Used from the
/// file search thread; the reply window runs on a thread of its own.
pub(super) struct Everything {
    reply: HWND,
    pending: Arc<AtomicU32>,
    next_id: u32,
    /// Rows asked for in the pending query.
    max: u32,
    /// Everything's window, once it said its database is loaded.
    loaded: Option<isize>,
    failed_at: Option<Instant>,
    /// How long to use the index after a failure; doubles while failures repeat.
    retry_after: Duration,
}

impl Everything {
    /// Starts the reply window. `deliver` gets each expected reply's id and
    /// bytes, on the reply window's thread; pass them to [`Everything::answer`].
    pub(super) fn spawn(deliver: impl Fn(u32, Vec<u8>) + Send + 'static) -> Option<Everything> {
        let pending = Arc::new(AtomicU32::new(0));
        let shared = pending.clone();
        let (created, window) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("everything replies".into())
            .spawn(move || {
                REPLIES.with(|r| {
                    *r.borrow_mut() = Some(Replies {
                        pending: shared,
                        deliver: Box::new(deliver),
                    })
                });
                let reply = unsafe { reply_window() };
                let _ = created.send(reply.map(|hwnd| hwnd.0 as isize));
                if reply.is_some() {
                    unsafe {
                        let mut msg = MSG::default();
                        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                            DispatchMessageW(&msg);
                        }
                    }
                }
            })
            .ok()?;
        let reply = HWND(window.recv().ok()?? as _);
        Some(Everything {
            reply,
            pending,
            // Not guessable, so another program can't easily answer in Everything's place.
            next_id: (RandomState::new().hash_one(Instant::now()) as u32).max(1),
            max: 0,
            loaded: None,
            failed_at: None,
            retry_after: RETRY_AFTER,
        })
    }

    /// Asks Everything for up to `max` files and folders whose names contain
    /// every word, newest first. Returns the id its reply will carry and when
    /// to stop waiting for it. `None` if Everything isn't running, hasn't loaded
    /// its database yet, or recently didn't answer in time.
    pub(super) fn ask(&mut self, words: &[String], exclude: &[String], max: u32) -> Option<(u32, Instant)> {
        if self.failed_at.is_some_and(|t| t.elapsed() < self.retry_after) {
            return None;
        }
        let target = window()?;
        let deadline = Instant::now() + ANSWER_WITHIN;
        if self.loaded != Some(target.0 as isize) {
            let mut is_loaded = 0usize;
            let asked = send(
                target,
                WM_USER,
                WPARAM(EVERYTHING_IPC_IS_DB_LOADED),
                LPARAM(0),
                deadline,
                &mut is_loaded,
            );
            if !asked {
                self.failed();
                return None;
            }
            if is_loaded == 0 {
                return None;
            }
            self.loaded = Some(target.0 as isize);
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let request = query2(self.reply, id, &search(words, exclude), max);
        let data = COPYDATASTRUCT {
            dwData: EVERYTHING_IPC_COPYDATA_QUERY2W,
            cbData: request.len() as u32,
            lpData: request.as_ptr() as *mut _,
        };
        self.max = max;
        self.pending.store(id, Ordering::Release);
        let mut accepted = 0usize;
        let asked = send(
            target,
            WM_COPYDATA,
            WPARAM(self.reply.0 as usize),
            LPARAM(&data as *const _ as isize),
            deadline,
            &mut accepted,
        );
        if !asked || accepted == 0 {
            self.failed();
            return None;
        }
        Some((id, deadline))
    }

    /// Reads the reply to the pending query. `None` if it's malformed.
    pub(super) fn answer(&mut self, bytes: &[u8]) -> Option<Vec<Row>> {
        self.cancel();
        let rows = parse(bytes, self.max)?;
        self.failed_at = None;
        self.retry_after = RETRY_AFTER;
        Some(rows.into_iter().filter(is_local).collect())
    }

    /// Stops waiting for the pending query; its reply is dropped.
    pub(super) fn cancel(&mut self) {
        self.pending.store(0, Ordering::Release);
    }

    /// The pending query got no reply in time: use the index for a while.
    pub(super) fn timed_out(&mut self) {
        self.cancel();
        self.failed();
    }

    fn failed(&mut self) {
        self.pending.store(0, Ordering::Release);
        self.loaded = None;
        if self.failed_at.is_some() {
            self.retry_after = (self.retry_after * 2).min(MAX_RETRY_AFTER);
        }
        self.failed_at = Some(Instant::now());
    }
}

impl Drop for Everything {
    fn drop(&mut self) {
        // The window belongs to its own thread; closing it ends that thread.
        unsafe {
            let _ = PostMessageW(Some(self.reply), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

fn window() -> Option<HWND> {
    WINDOW_CLASSES
        .iter()
        .find_map(|class| unsafe { FindWindowW(*class, PCWSTR::null()).ok() })
}

/// Sends `msg` to Everything, giving up at `deadline`. False if it didn't
/// take the message in time.
fn send(target: HWND, msg: u32, wp: WPARAM, lp: LPARAM, deadline: Instant, result: &mut usize) -> bool {
    let ms = deadline.saturating_duration_since(Instant::now()).as_millis().max(1) as u32;
    let sent = unsafe { SendMessageTimeoutW(target, msg, wp, lp, SMTO_NORMAL | SMTO_ABORTIFHUNG, ms, Some(result)) };
    sent.0 != 0
}

/// A message-only window for Everything's replies, on the calling thread.
unsafe fn reply_window() -> Option<HWND> {
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
        Some(reply)
    }
}

unsafe extern "system" fn reply_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            let data = unsafe { (lp.0 as *const COPYDATASTRUCT).as_ref() };
            if let Some(data) = data {
                REPLIES.with(|replies| {
                    let replies = replies.borrow();
                    let Some(replies) = replies.as_ref() else {
                        return;
                    };
                    // Only the reply being waited for, and only if its size is plausible.
                    let id = replies.pending.load(Ordering::Acquire);
                    if id == 0 || data.dwData != id as usize || data.lpData.is_null() {
                        return;
                    }
                    if data.cbData as usize > MAX_REPLY_BYTES {
                        return;
                    }
                    // Copy now: the pointer is only valid during this message.
                    let bytes =
                        unsafe { std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize).to_vec() };
                    (replies.deliver)(id, bytes);
                });
                return LRESULT(1);
            }
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
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
/// `None` if the reply is malformed, carries other fields, or has more than
/// `max` items.
fn parse(bytes: &[u8], max: u32) -> Option<Vec<Row>> {
    let dword =
        |at: usize| -> Option<u32> { Some(u32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?)) };
    let count = dword(4)?;
    if count > max || dword(12)? != REQUEST_NAME | REQUEST_FULL_PATH_AND_NAME {
        return None;
    }
    // Reads a length, the UTF-16 text and its NUL; returns the text and the next offset.
    let text = |at: usize| -> Option<(String, usize)> {
        let len = dword(at)? as usize;
        // No Windows path or name is longer; a longer one is a forged reply.
        if len > MAX_TEXT_UNITS {
            return None;
        }
        let start = at.checked_add(4)?;
        let end = start.checked_add(len.checked_mul(2)?)?;
        let units: Vec<u16> = bytes
            .get(start..end)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        Some((String::from_utf16_lossy(&units), end.checked_add(2)?))
    };
    let mut rows = Vec::with_capacity(count as usize);
    for item in 0..count as usize {
        let at = item.checked_mul(8)?.checked_add(20)?;
        let flags = dword(at)?;
        let (name, next) = text(dword(at.checked_add(4)?)? as usize)?;
        let (path, _) = text(next)?;
        rows.push(Row {
            name,
            path,
            is_folder: flags & (ITEM_FOLDER | ITEM_DRIVE) != 0,
        });
    }
    Some(rows)
}

/// True for a path on a local drive, `X:\…`, that ends with the row's name.
/// The paths are opened and their icons loaded, so a reply from another
/// program must not point anywhere else.
fn is_local(row: &Row) -> bool {
    let path = row.path.as_bytes();
    let drive = path.len() > 3 && path[0].is_ascii_alphabetic() && path[1] == b':' && path[2] == b'\\';
    let name = !row.name.is_empty() && !row.name.contains(['\\', '/']) && row.name != "..";
    drive
        && name
        && !row.path.contains('/')
        && !row.path.split('\\').any(|part| part == "." || part == "..")
        && row
            .path
            .strip_suffix(&row.name)
            .is_some_and(|rest| rest.ends_with('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_search_text() {
        let words = vec!["bug".to_string(), "re\"p*o?rt".into(), "\"".into(), "it's".into()];
        let exclude = vec!["node_modules".to_string(), "Code Cache".into()];
        assert_eq!(
            search(&words, &exclude),
            r#""bug" "report" "it's" !"\." !":\Windows\" !"\$Recycle.Bin\" !"\node_modules\" !"\Code Cache\""#
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
        let rows = parse(&bytes, 100).unwrap();
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
        assert!(parse(&bytes[..bytes.len() - 6], 100).is_none());
        assert!(parse(&bytes[..10], 100).is_none());
        let mut other_fields = bytes.clone();
        other_fields[12] = 0x7;
        assert!(parse(&other_fields, 100).is_none());
        assert_eq!(parse(&list2(&[]), 100).unwrap().len(), 0);
        // More items than asked for.
        assert!(parse(&bytes, 0).is_none());
    }

    #[test]
    fn keeps_only_local_paths_that_end_with_the_name() {
        let row = |name: &str, path: &str| Row {
            name: name.into(),
            path: path.into(),
            is_folder: false,
        };
        assert!(is_local(&row("a.txt", r"C:\a.txt")));
        assert!(is_local(&row("Berichte", r"E:\Docs\Berichte")));
        assert!(!is_local(&row("a.txt", r"\\server\share\a.txt")));
        assert!(!is_local(&row("a.txt", r"C:\b.txt")));
        assert!(!is_local(&row("a.txt", r"C:\xa.txt")));
        assert!(!is_local(&row("a.txt", r"C:\x\..\..\a.txt")));
        assert!(!is_local(&row("a.txt", "C:/x/a.txt")));
        assert!(!is_local(&row("C:", "C:")));
        assert!(!is_local(&row("", r"C:\")));
    }

    /// The reply window passes on only the reply to the pending query.
    #[test]
    fn delivers_only_the_pending_reply() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut everything = Everything::spawn(move |id, bytes| {
            let _ = tx.send((id, bytes));
        })
        .unwrap();
        let reply = everything.reply;
        let send = |id: usize, bytes: &[u8]| {
            let data = COPYDATASTRUCT {
                dwData: id,
                cbData: bytes.len() as u32,
                lpData: bytes.as_ptr() as *mut _,
            };
            unsafe { SendMessageW(reply, WM_COPYDATA, None, Some(LPARAM(&data as *const _ as isize))) }
        };
        let wait = || rx.recv_timeout(Duration::from_millis(200)).ok();

        // Nothing pending: dropped.
        send(5, b"x");
        assert_eq!(wait(), None);
        everything.pending.store(5, Ordering::Release);
        send(6, b"x");
        assert_eq!(wait(), None);
        send(5, b"ok");
        assert_eq!(wait(), Some((5, b"ok".to_vec())));
        everything.cancel();
        send(5, b"late");
        assert_eq!(wait(), None);
    }
}
