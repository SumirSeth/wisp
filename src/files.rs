//! File search through the Windows Search index (`SystemIndex`), on a worker thread.
//!
//! A new query takes about 100–160 ms, so it runs off the UI thread.
//! When the user types faster than queries finish, the worker skips to the
//! latest query.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
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

/// The index has no constant for this OLE DB dialect GUID in the `windows` crate.
const DBGUID_DEFAULT: GUID = GUID::from_u128(0xc8b521fb_5cf3_11ce_ade5_00aa0044773d);
/// Rows fetched per query before filtering and ranking.
const CANDIDATES: usize = 100;
/// Longest path or name read from the index, in UTF-16 units.
const MAX_CHARS: usize = 1024;
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

/// The worker sends each query's results first, then a second message with any
/// icons it had to load. Icon loading takes longer than the query.
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
}

pub struct FileSearch {
    requests: Sender<Request>,
}

impl FileSearch {
    /// Starts the worker. After each result is sent, it posts `notify_msg` to `hwnd`.
    pub fn spawn(hwnd: HWND, notify_msg: u32, results: Sender<FileResults>, icon_size: i32) -> FileSearch {
        let (requests, rx) = channel();
        let hwnd = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("file search".into())
            .spawn(move || {
                worker(rx, results, icon_size, move || unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as _)), notify_msg, WPARAM(0), LPARAM(0));
                })
            })
            .expect("could not start file search thread");
        FileSearch { requests }
    }

    /// Searches file names for `query`. `exclude` lists folder names to skip.
    pub fn search(&self, query: &str, limit: usize, exclude: Arc<[String]>) {
        let _ = self.requests.send(Request {
            query: query.to_string(),
            limit,
            exclude,
        });
    }
}

fn worker(requests: Receiver<Request>, results: Sender<FileResults>, icon_size: i32, notify: impl Fn()) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let profile = std::env::var("USERPROFILE").unwrap_or_default().to_lowercase();
    let mut index: Option<Index> = None;
    let mut failed_at: Option<Instant> = None;
    let mut matcher = NameMatcher::new();
    let mut icons_sent = HashSet::new();

    let mut next = requests.recv().ok();
    while let Some(mut request) = next.take() {
        // Only the latest query matters.
        loop {
            match requests.try_recv() {
                Ok(newer) => request = newer,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
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

        let rows = match &index {
            Some(ix) => unsafe { ix.query(&request.query) }.unwrap_or_else(|e| {
                eprintln!("file search query failed: {e}");
                // Reopen on the next query; the index service may have restarted.
                index = None;
                Vec::new()
            }),
            None => Vec::new(),
        };
        let hits = rank(rows, &request, &profile, &mut matcher);

        // Skip icon loading and sending if a newer query is already waiting.
        match requests.try_recv() {
            Ok(newer) => {
                next = Some(newer);
                continue;
            }
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }

        let missing: Vec<(String, String)> = hits
            .iter()
            .filter(|hit| !icons_sent.contains(&hit.icon_key))
            .map(|hit| (hit.icon_key.clone(), hit.path.clone()))
            .collect();
        let sent = results.send(FileResults {
            query: request.query.clone(),
            hits: Some(hits),
            icons: Vec::new(),
        });
        if sent.is_err() {
            return;
        }
        notify();

        let mut icons = Vec::new();
        for (key, path) in missing {
            if icons_sent.contains(&key) {
                continue;
            }
            if let Some(pixels) = unsafe { load_icon(&path, icon_size) } {
                icons_sent.insert(key.clone());
                icons.push((key, Arc::new(pixels)));
            }
        }
        if !icons.is_empty() {
            let sent = results.send(FileResults {
                query: request.query,
                hits: None,
                icons,
            });
            if sent.is_err() {
                return;
            }
            notify();
        }
        next = requests.recv().ok();
    }
}

struct Row {
    name: String,
    path: String,
    is_folder: bool,
}

/// Drops noise, then orders by name match, user folders, path depth and recency.
fn rank(rows: Vec<Row>, request: &Request, profile: &str, matcher: &mut NameMatcher) -> Vec<FileHit> {
    let pattern = Pattern::parse(&request.query, CaseMatching::Ignore, Normalization::Smart);
    let mut scored: Vec<(i64, FileHit)> = rows
        .into_iter()
        .enumerate()
        .filter(|(_, row)| !is_excluded(&row.path, &request.exclude))
        .filter_map(|(position, row)| {
            let (name_score, highlights) = matcher.score(&pattern, &row.name)?;
            // Rows arrive newest first.
            let recency = (CANDIDATES - position.min(CANDIDATES)) as i64 / 4;
            let (in_user_folder, depth) = location(&row.path, profile);
            let score = name_score as i64 + recency + if in_user_folder { 40 } else { 0 } - 4 * depth.min(10) as i64;
            let icon_key = icon_key(&row);
            Some((
                score,
                FileHit {
                    name: row.name,
                    path: row.path,
                    is_folder: row.is_folder,
                    highlights,
                    icon_key,
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

    unsafe fn query(&self, query: &str) -> Result<Vec<Row>> {
        let Some(sql) = sql(query) else {
            return Ok(Vec::new());
        };
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

/// Builds a prefix search on file names, newest first. Returns `None` if the
/// query has no searchable words.
fn sql(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        // Quotes and wildcards would break out of the CONTAINS literal.
        .map(|word| word.replace(['\'', '"', '*'], ""))
        .filter(|word| !word.is_empty())
        .map(|word| format!("CONTAINS(System.FileName, '\"{word}*\"')"))
        .collect();
    if terms.is_empty() {
        return None;
    }
    Some(format!(
        "SELECT TOP {CANDIDATES} System.ItemPathDisplay, System.FileName, System.ItemType \
         FROM SystemIndex WHERE SCOPE='file:' AND {} ORDER BY System.DateModified DESC",
        terms.join(" AND ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Queries the real Windows Search index through the worker thread.
    /// Needs the Windows Search service, so CI skips it:
    /// `cargo test -- --ignored searches_the_windows_index`
    #[test]
    #[ignore]
    fn searches_the_windows_index() {
        let (tx, rx) = channel();
        let search = FileSearch::spawn(HWND::default(), 0, tx, 32);
        let exclude: Arc<[String]> = Arc::from(crate::config::Config::default().file_search_exclude);
        for query in ["readme", "zzqx-no-such-file-qxzz"] {
            let started = Instant::now();
            search.search(query, 5, exclude.clone());
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

    #[test]
    fn sql_escapes_query_text() {
        assert_eq!(sql("  "), None);
        assert_eq!(sql("'\"*"), None);
        let q = sql("it's \"x\" a*b").unwrap();
        assert!(q.contains("CONTAINS(System.FileName, '\"its*\"') AND CONTAINS(System.FileName, '\"x*\"') AND CONTAINS(System.FileName, '\"ab*\"')"));
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
        };
        let hits = rank(rows, &request, r"c:\users\me", &mut NameMatcher::new());
        assert_eq!(hits[0].path, r"C:\Users\me\Downloads\Resume.pdf");
        assert_eq!(hits[0].highlights, vec![(0, 6)]);
        assert_eq!(hits[0].icon_key, "file:.pdf");
    }
}
