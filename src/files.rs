//! File search on a worker thread, through Everything when it runs and the
//! Windows Search index (`SystemIndex`) otherwise.
//!
//! The index runs two queries: words that start a word in the name, which
//! takes 10–30 ms and is shown at once, then words inside names, which takes
//! 70–200 ms more and is skipped when the first one already fills the list.
//! When the user types faster than queries finish, the worker skips to the
//! latest query.

#[path = "files/everything.rs"]
mod everything;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use windows::Win32::Foundation::{E_FAIL, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::*;
use windows::Win32::System::Search::*;
use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{GUID, HSTRING, IUnknown, Interface, Result, w};

use crate::indexer::{self, IconPixels};
use crate::search::NameMatcher;
use everything::Everything;

/// The index has no constant for this OLE DB dialect GUID in the `windows` crate.
const DBGUID_DEFAULT: GUID = GUID::from_u128(0xc8b521fb_5cf3_11ce_ade5_00aa0044773d);
/// Rows fetched per index query before filtering and ranking.
const CANDIDATES: usize = 100;
/// Rows asked from Everything. It covers every drive, so the newest 100 are
/// mostly program and temp files, and older files in the user's folders would
/// never be ranked.
const EVERYTHING_CANDIDATES: u32 = 500;
/// Longest path or name read from the index, in UTF-16 units.
const MAX_CHARS: usize = 1024;
/// How long typing must pause before the slow index pass starts.
const PAUSE_BEFORE_SLOW_PASS: Duration = Duration::from_millis(80);
/// How long to wait before trying to reopen an index that failed to open.
const REOPEN_AFTER: Duration = Duration::from_secs(30);
/// Folders under the user profile that hold the user's own files.
const USER_FOLDERS: &[&str] = &["desktop", "documents", "downloads", "pictures", "videos", "music"];
/// File types whose icon differs per file, so the icon is cached per path.
const PER_FILE_ICONS: &[&str] = &["exe", "lnk", "ico", "url", "appref-ms"];

pub struct FileHit {
    pub name: String,
    pub path: String,
    pub is_folder: bool,
    /// UTF-16 `(start, len)` ranges in `name` that match the query.
    pub highlights: Vec<(u32, u32)>,
    pub icon_key: String,
}

/// The worker sends each query's results, then a message with any icons it
/// had to load. Icon loading takes longer than the query. An index query can
/// send its results twice: the hits of its first pass, then all hits.
pub struct FileResults {
    pub query: String,
    /// `None` in the message that carries only icons.
    pub hits: Option<Vec<FileHit>>,
    /// Icons the UI has not received before, by icon key.
    pub icons: Vec<(String, Arc<IconPixels>)>,
}

struct Request {
    query: String,
    limit: usize,
    exclude: Arc<[String]>,
    /// Ask Everything first, if it runs.
    everything: bool,
}

/// What the worker waits for: the next query, or Everything's reply.
enum Message {
    Search(Request),
    Everything(u32, Vec<u8>),
    Stop,
}

/// The worker should end: the launcher is gone.
struct Stop;

pub struct FileSearch {
    messages: Sender<Message>,
    forget: Arc<AtomicBool>,
}

/// Icons cached for one file (programs, shortcuts), not for a whole file type.
pub fn is_per_file_icon(key: &str) -> bool {
    key.strip_prefix("file:")
        .is_some_and(|rest| !rest.starts_with('.') && rest != "<folder>")
}

impl FileSearch {
    /// Starts the worker. After each result is sent, it posts `notify_msg` to `hwnd`.
    pub fn spawn(hwnd: HWND, notify_msg: u32, results: Sender<FileResults>, icon_size: i32) -> FileSearch {
        let (messages, rx) = channel();
        let replies = messages.clone();
        let forget = Arc::new(AtomicBool::new(false));
        let worker_forget = forget.clone();
        let hwnd = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("file search".into())
            .spawn(move || {
                let notify = move || unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as _)), notify_msg, WPARAM(0), LPARAM(0));
                };
                let output = Output {
                    results,
                    notify,
                    icon_size,
                    icons_sent: HashSet::new(),
                    forget: worker_forget,
                };
                worker(rx, replies, output)
            })
            .expect("could not start file search thread");
        FileSearch { messages, forget }
    }

    /// The UI dropped its per-file icons: send them again when they're needed.
    pub fn forget_file_icons(&self) {
        self.forget.store(true, Ordering::Release);
    }

    /// Searches file names for `query`. `exclude` lists folder names to skip.
    /// With `everything`, asks Everything if it runs, and the index otherwise.
    pub fn search(&self, query: &str, limit: usize, exclude: Arc<[String]>, everything: bool) {
        let _ = self.messages.send(Message::Search(Request {
            query: query.to_string(),
            limit,
            exclude,
            everything,
        }));
    }
}

impl Drop for FileSearch {
    fn drop(&mut self) {
        // The worker holds a sender for Everything's replies, so the channel
        // alone would never tell it to end.
        let _ = self.messages.send(Message::Stop);
    }
}

fn worker(messages: Receiver<Message>, replies: Sender<Message>, mut output: Output<impl Fn()>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let profile = std::env::var("USERPROFILE").unwrap_or_default().to_lowercase();
    let mut index: Option<Index> = None;
    let mut everything: Option<Everything> = None;
    let mut spawn_failed_at: Option<Instant> = None;
    let mut failed_at: Option<Instant> = None;
    let mut matcher = NameMatcher::new();

    let mut next = next_request(&messages);
    while let Some(request) = next.take() {
        // Only the latest query matters.
        let Ok(newest) = newer(&messages) else { return };
        let request = newest.unwrap_or(request);
        let words = words(&request.query);
        if words.is_empty() {
            if !output.send(&request.query, Vec::new()) {
                return;
            }
            next = next_request(&messages);
            continue;
        }

        if request.everything {
            match ask_everything(
                &mut everything,
                &mut spawn_failed_at,
                &replies,
                &messages,
                &request,
                &words,
            ) {
                // No rows can mean Everything doesn't cover the drive; the index may.
                Waited::Rows(rows) if !rows.is_empty() => {
                    let hits = rank(&[rows], &request, &profile, &mut matcher);
                    match newer(&messages) {
                        Ok(None) => {}
                        Ok(newest) => {
                            next = newest;
                            continue;
                        }
                        Err(Stop) => return,
                    }
                    if !output.send(&request.query, hits) {
                        return;
                    }
                    next = next_request(&messages);
                    continue;
                }
                Waited::Newer(newest) => {
                    next = Some(newest);
                    continue;
                }
                Waited::Stop => return,
                Waited::Rows(_) | Waited::Nothing => {}
            }
        }

        if index.is_none() && failed_at.is_none_or(|t| t.elapsed() > REOPEN_AFTER) {
            match unsafe { Index::open() } {
                Ok(opened) => index = Some(opened),
                Err(e) => {
                    eprintln!("could not open the Windows Search index: {e}");
                    failed_at = Some(Instant::now());
                }
            }
        }

        // Shows each pass's hits as soon as it's done, together with the
        // earlier passes'. The later pass is the slow one.
        let mut passes = Vec::new();
        let mut shown: Option<Vec<String>> = None;
        for sql in sql(&words) {
            if let Some(shown) = &shown {
                if shown.len() >= request.limit {
                    break;
                }
                // The slow pass can't be interrupted, so it waits until typing pauses.
                match newer_within(&messages, PAUSE_BEFORE_SLOW_PASS) {
                    Ok(None) => {}
                    Ok(newest) => {
                        next = newest;
                        break;
                    }
                    Err(Stop) => return,
                }
            }
            let rows = fetch(&mut index, &sql);
            if shown.is_some() && rows.is_empty() {
                break;
            }
            passes.push(rows);
            let hits = rank(&passes, &request, &profile, &mut matcher);
            let paths: Vec<String> = hits.iter().map(|hit| hit.path.clone()).collect();
            if shown.as_ref() == Some(&paths) {
                continue;
            }
            // Skip icon loading and sending if a newer query is already waiting.
            match newer(&messages) {
                Ok(None) => {}
                Ok(newest) => {
                    next = newest;
                    break;
                }
                Err(Stop) => return,
            }
            if !output.send(&request.query, hits) {
                return;
            }
            shown = Some(paths);
        }
        if next.is_none() {
            next = next_request(&messages);
        }
    }
}

/// Blocks until the next query. `None` once the worker should end.
fn next_request(messages: &Receiver<Message>) -> Option<Request> {
    loop {
        match messages.recv() {
            Ok(Message::Search(request)) => return Some(request),
            // A reply that came after the worker stopped waiting for it.
            Ok(Message::Everything(..)) => {}
            Ok(Message::Stop) | Err(_) => return None,
        }
    }
}

/// The latest query that arrives within `wait`, if any.
fn newer_within(messages: &Receiver<Message>, wait: Duration) -> std::result::Result<Option<Request>, Stop> {
    let deadline = Instant::now() + wait;
    loop {
        match messages.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Message::Search(request)) => return newer(messages).map(|newest| Some(newest.unwrap_or(request))),
            Ok(Message::Everything(..)) => {}
            Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return Err(Stop),
            Err(RecvTimeoutError::Timeout) => return Ok(None),
        }
    }
}

/// The latest query that's already waiting, if any.
fn newer(messages: &Receiver<Message>) -> std::result::Result<Option<Request>, Stop> {
    let mut newest = None;
    loop {
        match messages.try_recv() {
            Ok(Message::Search(request)) => newest = Some(request),
            Ok(Message::Everything(..)) => {}
            Ok(Message::Stop) | Err(TryRecvError::Disconnected) => return Err(Stop),
            Err(TryRecvError::Empty) => return Ok(newest),
        }
    }
}

enum Waited {
    Rows(Vec<Row>),
    /// Everything isn't running, is busy, or sent a malformed reply.
    Nothing,
    Newer(Request),
    Stop,
}

/// Asks Everything and waits for its reply, or for a newer query.
fn ask_everything(
    everything: &mut Option<Everything>,
    spawn_failed_at: &mut Option<Instant>,
    replies: &Sender<Message>,
    messages: &Receiver<Message>,
    request: &Request,
    words: &[String],
) -> Waited {
    if everything.is_none() && spawn_failed_at.is_none_or(|t| t.elapsed() > REOPEN_AFTER) {
        let replies = replies.clone();
        *everything = Everything::spawn(move |id, bytes| {
            let _ = replies.send(Message::Everything(id, bytes));
        });
        *spawn_failed_at = everything.is_none().then(Instant::now);
    }
    let Some(everything) = everything.as_mut() else {
        return Waited::Nothing;
    };
    let Some((id, deadline)) = everything.ask(words, &request.exclude, EVERYTHING_CANDIDATES) else {
        return Waited::Nothing;
    };
    loop {
        match messages.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Message::Everything(got, bytes)) if got == id => {
                return everything.answer(&bytes).map_or(Waited::Nothing, Waited::Rows);
            }
            Ok(Message::Everything(..)) => {}
            Ok(Message::Search(newest)) => {
                everything.cancel();
                return Waited::Newer(newest);
            }
            Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return Waited::Stop,
            Err(RecvTimeoutError::Timeout) => {
                everything.timed_out();
                return Waited::Nothing;
            }
        }
    }
}

/// Sends results to the UI.
struct Output<F: Fn()> {
    results: Sender<FileResults>,
    notify: F,
    icon_size: i32,
    icons_sent: HashSet<String>,
    forget: Arc<AtomicBool>,
}

impl<F: Fn()> Output<F> {
    /// Sends `hits`, then the icons the UI doesn't have yet. False once the UI
    /// is gone.
    fn send(&mut self, query: &str, hits: Vec<FileHit>) -> bool {
        if self.forget.swap(false, Ordering::AcqRel) {
            self.icons_sent.retain(|key| !is_per_file_icon(key));
        }
        let missing: Vec<(String, String)> = hits
            .iter()
            .filter(|hit| !self.icons_sent.contains(&hit.icon_key))
            .map(|hit| (hit.icon_key.clone(), hit.path.clone()))
            .collect();
        let sent = self.results.send(FileResults {
            query: query.to_string(),
            hits: Some(hits),
            icons: Vec::new(),
        });
        if sent.is_err() {
            return false;
        }
        (self.notify)();

        let mut icons = Vec::new();
        for (key, path) in missing {
            if self.icons_sent.contains(&key) {
                continue;
            }
            if let Some(pixels) = unsafe { load_icon(&path, self.icon_size) } {
                self.icons_sent.insert(key.clone());
                icons.push((key, Arc::new(pixels)));
            }
        }
        if !icons.is_empty() {
            let sent = self.results.send(FileResults {
                query: query.to_string(),
                hits: None,
                icons,
            });
            if sent.is_err() {
                return false;
            }
            (self.notify)();
        }
        true
    }
}

struct Row {
    name: String,
    path: String,
    is_folder: bool,
}

/// Runs one query. On failure, drops the index so the next query reopens it;
/// the index service may have restarted.
fn fetch(index: &mut Option<Index>, sql: &str) -> Vec<Row> {
    match index.as_ref().map(|ix| unsafe { ix.query(sql) }) {
        Some(Ok(rows)) => rows,
        Some(Err(e)) => {
            eprintln!("file search query failed: {e}");
            *index = None;
            Vec::new()
        }
        None => Vec::new(),
    }
}

/// Drops noise, then orders by name match, user folders, path depth and recency.
/// `passes` holds the rows of each query, each newest first.
fn rank(passes: &[Vec<Row>], request: &Request, profile: &str, matcher: &mut NameMatcher) -> Vec<FileHit> {
    let pattern = Pattern::parse(&request.query, CaseMatching::Ignore, Normalization::Smart);
    let word_count = request.query.split_whitespace().count();
    let mut scored: Vec<(i64, FileHit)> = passes
        .iter()
        .flat_map(|rows| {
            // Recency counts the same over Everything's 500 rows as over 100.
            let per = rows.len().max(CANDIDATES);
            rows.iter()
                .enumerate()
                .map(move |(position, row)| (position * CANDIDATES / per, row))
        })
        .filter(|(_, row)| !is_excluded(&row.path, &request.exclude))
        .filter_map(|(position, row)| {
            let (name_score, highlights) = matcher.score_words(&pattern, &row.name, word_count)?;
            let recency = (CANDIDATES - position.min(CANDIDATES)) as i64 / 4;
            let (in_user_folder, depth) = location(&row.path, profile);
            let score = name_score as i64 + recency + if in_user_folder { 40 } else { 0 } - 4 * depth.min(10) as i64;
            Some((
                score,
                FileHit {
                    name: row.name.clone(),
                    path: row.path.clone(),
                    is_folder: row.is_folder,
                    highlights,
                    icon_key: icon_key(row),
                },
            ))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().take(request.limit).map(|(_, hit)| hit).collect()
}

/// True if the file or any folder in `path` starts with a dot, or if any
/// folder in `path` is in `exclude`.
fn is_excluded(path: &str, exclude: &[String]) -> bool {
    let path = Path::new(path);
    let hidden_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'));
    let folders = path.parent().into_iter().flat_map(|p| p.iter());
    hidden_name
        || folders
            .filter_map(|f| f.to_str())
            .any(|folder| folder.starts_with('.') || exclude.iter().any(|e| e.eq_ignore_ascii_case(folder)))
}

/// Whether `path` is in one of the user's own folders, and how many folders
/// deep it is below the user profile.
fn location(path: &str, profile: &str) -> (bool, usize) {
    let lower = path.to_lowercase();
    let Some(rest) = lower.strip_prefix(profile).map(|r| r.trim_start_matches('\\')) else {
        return (false, lower.matches('\\').count());
    };
    let top = rest.split('\\').next().unwrap_or("");
    let in_user_folder = USER_FOLDERS.contains(&top) || top.starts_with("onedrive");
    (in_user_folder, rest.matches('\\').count())
}

fn icon_key(row: &Row) -> String {
    if row.is_folder {
        return "file:<folder>".into();
    }
    let ext = Path::new(&row.path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if PER_FILE_ICONS.contains(&ext.as_str()) {
        format!("file:{}", row.path.to_lowercase())
    } else {
        format!("file:.{ext}")
    }
}

unsafe fn load_icon(path: &str, size: i32) -> Option<IconPixels> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        indexer::load_icon(&item, size)
    }
}

/// An open OLE DB session on the Windows Search index.
struct Index {
    command: ICommandText,
}

/// One row as OLE DB writes it: a status, a byte length and the UTF-16 text,
/// for each of the three selected columns.
#[repr(C)]
struct Column {
    status: u32,
    length: usize,
    value: [u16; MAX_CHARS],
}

#[repr(C)]
struct RowBuffer {
    path: Column,
    name: Column,
    item_type: Column,
}

impl Column {
    fn text(&self) -> String {
        String::from_utf16_lossy(&self.value[..(self.length / 2).min(MAX_CHARS)])
    }
}

impl Index {
    unsafe fn open() -> Result<Index> {
        unsafe {
            let manager: ISearchManager = CoCreateInstance(&CSearchManager, None, CLSCTX_LOCAL_SERVER)?;
            let catalog = manager.GetCatalog(w!("SystemIndex"))?;
            let connection = catalog.GetQueryHelper()?.ConnectionString()?;
            let connection_text = connection.to_string();
            CoTaskMemFree(Some(connection.0 as _));
            let connection_text = connection_text.map_err(|_| windows::core::Error::from(E_FAIL))?;

            let init: IDataInitialize = CoCreateInstance(&MSDAINITIALIZE, None, CLSCTX_INPROC_SERVER)?;
            let mut source: Option<IUnknown> = None;
            init.GetDataSource(
                None,
                CLSCTX_INPROC_SERVER.0,
                &HSTRING::from(connection_text),
                &IDBInitialize::IID,
                &mut source,
            )?;
            let database: IDBInitialize = source.ok_or(windows::core::Error::from(E_FAIL))?.cast()?;
            database.Initialize()?;
            let session: IDBCreateCommand = database
                .cast::<IDBCreateSession>()?
                .CreateSession(None, &IDBCreateCommand::IID)?
                .cast()?;
            let command: ICommandText = session.CreateCommand(None, &ICommandText::IID)?.cast()?;
            Ok(Index { command })
        }
    }

    unsafe fn query(&self, sql: &str) -> Result<Vec<Row>> {
        unsafe {
            self.command.SetCommandText(&DBGUID_DEFAULT, &HSTRING::from(sql))?;
            let mut unknown: Option<IUnknown> = None;
            self.command
                .Execute(None, &IRowset::IID, None, None, Some(&mut unknown))?;
            let rowset: IRowset = unknown.ok_or(windows::core::Error::from(E_FAIL))?.cast()?;
            let accessor: IAccessor = rowset.cast()?;

            let binding = |ordinal: usize, column: usize| DBBINDING {
                iOrdinal: ordinal,
                obValue: column + std::mem::offset_of!(Column, value),
                obLength: column + std::mem::offset_of!(Column, length),
                obStatus: column + std::mem::offset_of!(Column, status),
                dwPart: (DBPART_VALUE.0 | DBPART_LENGTH.0 | DBPART_STATUS.0) as u32,
                dwMemOwner: DBMEMOWNER_CLIENTOWNED.0 as u32,
                eParamIO: DBPARAMIO_NOTPARAM.0 as u32,
                cbMaxLen: MAX_CHARS * 2,
                wType: DBTYPE_WSTR.0 as u16,
                ..Default::default()
            };
            let bindings = [
                binding(1, std::mem::offset_of!(RowBuffer, path)),
                binding(2, std::mem::offset_of!(RowBuffer, name)),
                binding(3, std::mem::offset_of!(RowBuffer, item_type)),
            ];
            let mut handle = HACCESSOR::default();
            accessor.CreateAccessor(
                DBACCESSOR_ROWDATA.0 as u32,
                bindings.len(),
                bindings.as_ptr(),
                size_of::<RowBuffer>(),
                &mut handle,
                None,
            )?;

            let mut buffer: Box<RowBuffer> = Box::new(std::mem::zeroed());
            let mut rows = Vec::new();
            loop {
                // GetNextRows takes HROW**: the first slot points at the handle buffer,
                // and the slice length is the number of rows requested.
                let mut handles = [0usize; 32];
                let mut slots = [std::ptr::null_mut::<usize>(); 32];
                slots[0] = handles.as_mut_ptr();
                let mut fetched = 0usize;
                let _ = rowset.GetNextRows(0, 0, &mut fetched, &mut slots);
                if fetched == 0 {
                    break;
                }
                for &row in &handles[..fetched] {
                    if rowset
                        .GetData(row, handle, (&mut *buffer as *mut RowBuffer).cast())
                        .is_ok()
                    {
                        rows.push(Row {
                            name: buffer.name.text(),
                            path: buffer.path.text(),
                            is_folder: buffer.item_type.text() == "Directory",
                        });
                    }
                }
                let _ = rowset.ReleaseRows(
                    fetched,
                    handles.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
            }
            let _ = accessor.ReleaseAccessor(handle, None);
            Ok(rows)
        }
    }
}

/// The query's words, without double quotes and wildcards, which would break
/// out of the index's `CONTAINS` literal and change Everything's search.
/// Apostrophes stay, so `it's` finds `it's.txt`; `sql` escapes them.
fn words(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|word| word.replace(['"', '*', '?'], ""))
        .filter(|word| word.chars().any(|c| c != '\''))
        .collect()
}

/// Builds the index queries for file names, each newest first: words that
/// start a word in the name, then words inside the name that the first query
/// misses, such as `report` in `bug-reporting.txt`. A second query rather than
/// an `OR`, so newer matches inside names can't push the first ones out of the
/// `TOP` rows. Returns no queries if there are no words.
fn sql(words: &[String]) -> Vec<String> {
    if words.is_empty() {
        return Vec::new();
    }
    let prefix = words
        .iter()
        // Without apostrophes, which would end the CONTAINS literal.
        .map(|word| format!("CONTAINS(System.FileName, '\"{}*\"')", word.replace('\'', "")))
        .collect::<Vec<_>>()
        .join(" AND ");
    let inside = words
        .iter()
        // LIKE treats % _ [ as wildcards; brackets make them literal. A doubled
        // apostrophe is one apostrophe in the SQL string.
        .map(|word| {
            word.replace('[', "[[]")
                .replace('%', "[%]")
                .replace('_', "[_]")
                .replace('\'', "''")
        })
        .map(|word| format!("System.FileName LIKE '%{word}%'"))
        .collect::<Vec<_>>()
        .join(" AND ");
    let select = |filter: String| {
        format!(
            "SELECT TOP {CANDIDATES} System.ItemPathDisplay, System.FileName, System.ItemType \
             FROM SystemIndex WHERE SCOPE='file:' AND {filter} ORDER BY System.DateModified DESC"
        )
    };
    vec![select(prefix.clone()), select(format!("{inside} AND NOT ({prefix})"))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_file_icons_are_programs_and_shortcuts_not_types() {
        assert!(is_per_file_icon(r"file:c:\tools\app.exe"));
        assert!(!is_per_file_icon("file:.pdf"));
        assert!(!is_per_file_icon("file:<folder>"));
        assert!(!is_per_file_icon("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"));
    }

    /// Queries the real Windows Search index through the worker thread.
    /// Needs the Windows Search service, so CI skips it:
    /// `cargo test -- --ignored searches_the_windows_index`
    #[test]
    #[ignore]
    fn searches_the_windows_index() {
        let (tx, rx) = channel();
        let search = FileSearch::spawn(HWND::default(), 0, tx, 32);
        let exclude: Arc<[String]> = Arc::from(crate::config::Config::default().file_search_exclude);
        for query in ["readme", "setup", "read me", "zzqx-no-such-file-qxzz"] {
            let started = Instant::now();
            search.search(query, 5, exclude.clone(), false);
            // Skip icon-only messages left over from the previous query.
            let (results, hits) = loop {
                let mut results = rx
                    .recv_timeout(Duration::from_secs(10))
                    .expect("no results from the worker");
                if let Some(hits) = results.hits.take() {
                    break (results, hits);
                }
            };
            println!("{query}: {} hits in {:?}", hits.len(), started.elapsed());
            for hit in &hits {
                println!("  {} {}", if hit.is_folder { "dir " } else { "file" }, hit.path);
                assert!(
                    !is_excluded(&hit.path, &exclude),
                    "excluded path returned: {}",
                    hit.path
                );
            }
            assert_eq!(results.query, query);
        }
    }

    /// Queries a running Everything, then ranks its rows as the worker does.
    /// Needs Everything, so CI skips it:
    /// `cargo test -- --ignored searches_with_everything`
    #[test]
    #[ignore]
    fn searches_with_everything() {
        let exclude: Arc<[String]> = Arc::from(crate::config::Config::default().file_search_exclude);
        let profile = std::env::var("USERPROFILE").unwrap().to_lowercase();
        let (replies, messages) = channel();
        let mut everything = None;
        let mut matcher = NameMatcher::new();
        for query in [
            "readme",
            "setup",
            "read me",
            "wisp",
            "everything.rs",
            "zzqx-no-such-file-qxzz",
        ] {
            let request = Request {
                query: query.into(),
                limit: 5,
                exclude: exclude.clone(),
                everything: true,
            };
            let started = Instant::now();
            let Waited::Rows(rows) =
                ask_everything(&mut everything, &mut None, &replies, &messages, &request, &words(query))
            else {
                panic!("Everything is not running or did not answer");
            };
            let took = started.elapsed();
            let count = rows.len();
            for row in &rows {
                assert!(
                    !is_excluded(&row.path, &exclude),
                    "excluded path returned: {}",
                    row.path
                );
            }
            let hits = rank(&[rows], &request, &profile, &mut matcher);
            println!("{query}: {count} rows in {took:?}");
            for hit in &hits {
                println!("  {} {}", if hit.is_folder { "dir " } else { "file" }, hit.path);
            }
        }

        // A newer query stops the wait, and the late reply is dropped.
        let request = |query: &str| Request {
            query: query.into(),
            limit: 5,
            exclude: exclude.clone(),
            everything: true,
        };
        replies.send(Message::Search(request("newer"))).unwrap();
        let waited = ask_everything(
            &mut everything,
            &mut None,
            &replies,
            &messages,
            &request("setup"),
            &words("setup"),
        );
        assert!(matches!(waited, Waited::Newer(newer) if newer.query == "newer"));
        std::thread::sleep(Duration::from_millis(300));
        assert!(messages.try_recv().is_err(), "a late reply got through");
        let Waited::Rows(rows) = ask_everything(
            &mut everything,
            &mut None,
            &replies,
            &messages,
            &request("wisp"),
            &words("wisp"),
        ) else {
            panic!("Everything stopped answering after a cancelled query");
        };
        assert!(!rows.is_empty());
    }

    #[test]
    fn sql_escapes_query_text() {
        let sql = |query: &str| sql(&words(query));
        assert!(sql("  ").is_empty());
        assert!(sql("'\"*").is_empty());
        let q = sql("it's \"x\" a*b");
        let prefix = "CONTAINS(System.FileName, '\"its*\"') AND CONTAINS(System.FileName, '\"x*\"') AND CONTAINS(System.FileName, '\"ab*\"')";
        assert!(q[0].contains(&format!("AND {prefix} ORDER BY")));
        assert!(q[1].contains(&format!(
            "System.FileName LIKE '%it''s%' AND System.FileName LIKE '%x%' AND System.FileName LIKE '%ab%' AND NOT ({prefix})"
        )));
        let q = sql("50%_[a]");
        assert!(q[1].contains("System.FileName LIKE '%50[%][_][[]a]%'"));
    }

    #[test]
    fn ranks_word_starts_above_matches_inside_names() {
        let row = |name: &str| Row {
            name: name.into(),
            path: format!(r"C:\Users\me\Downloads\{name}"),
            is_folder: false,
        };
        let request = Request {
            query: "setup".into(),
            limit: 5,
            exclude: Arc::from(Vec::new()),
            everything: false,
        };
        // Both are the newest row of their query; the word start wins.
        let passes = vec![vec![row("setup.exe")], vec![row("rsetup.exe")]];
        let hits = rank(&passes, &request, r"c:\users\me", &mut NameMatcher::new());
        let names: Vec<&str> = hits.iter().map(|hit| hit.name.as_str()).collect();
        assert_eq!(names, ["setup.exe", "rsetup.exe"]);
        assert_eq!(hits[1].highlights, vec![(1, 5)]);
    }

    #[test]
    fn excludes_noise_folders() {
        let exclude: Vec<String> = vec!["node_modules".into(), "AppData".into()];
        assert!(is_excluded(r"C:\Users\me\code\app\node_modules\x\README.md", &exclude));
        assert!(is_excluded(r"C:\Users\me\appdata\Local\x.txt", &exclude));
        assert!(is_excluded(r"C:\Users\me\.git\config", &exclude));
        assert!(is_excluded(r"C:\Users\me\.claude.json.backup", &exclude));
        assert!(!is_excluded(r"C:\Users\me\Documents\resume.pdf", &exclude));
    }

    #[test]
    fn locates_user_folders() {
        let profile = r"c:\users\me";
        assert_eq!(location(r"C:\Users\me\Downloads\a.pdf", profile), (true, 1));
        assert_eq!(location(r"C:\Users\me\OneDrive - Work\Docs\a.pdf", profile), (true, 2));
        assert_eq!(location(r"C:\Users\me\code\proj\src\a.rs", profile), (false, 3));
    }

    #[test]
    fn ranks_user_files_above_deep_matches() {
        let rows = vec![
            Row {
                name: "resume.txt".into(),
                path: r"C:\Users\me\code\a\b\c\d\resume.txt".into(),
                is_folder: false,
            },
            Row {
                name: "Resume.pdf".into(),
                path: r"C:\Users\me\Downloads\Resume.pdf".into(),
                is_folder: false,
            },
        ];
        let request = Request {
            query: "resume".into(),
            limit: 5,
            exclude: Arc::from(Vec::new()),
            everything: false,
        };
        let hits = rank(&[rows], &request, r"c:\users\me", &mut NameMatcher::new());
        assert_eq!(hits[0].path, r"C:\Users\me\Downloads\Resume.pdf");
        assert_eq!(hits[0].highlights, vec![(0, 6)]);
        assert_eq!(hits[0].icon_key, "file:.pdf");
    }
}
