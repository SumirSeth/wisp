//! Read-only Jump Lists. No undocumented COM.
//!
//! One background thread reads them, starting when the pointer rests on an
//! app, so the right-click menu rarely waits. Each app's first read loads its
//! list from disk (often 20–200 ms); later reads take a millisecond or two.
use std::io::Read;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Foundation::{ERROR_TIMEOUT, HWND};
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::Common::IObjectArray;
use windows::Win32::UI::Shell::PropertiesSystem::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{GUID, HSTRING, IUnknown, Interface, PCWSTR, Result};

#[path = "jump_lists_format.rs"]
mod format;

pub const MENU_FIRST: usize = 1000; // 18 commands at most; existing app commands are 1..=15.
const LIMIT: usize = 6;
/// How long the menu waits for lists it doesn't have yet.
const WAIT: Duration = Duration::from_millis(200);
/// A read stops after this, so one slow provider can't hold the thread.
const BUDGET: Duration = Duration::from_secs(2);
/// Lists read on hover are used by a menu opened this soon after.
const FRESH: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct Section {
    pub title: &'static str,
    pub items: Vec<Item>,
}
#[derive(Clone)]
pub struct Item {
    pub title: String,
    action: Action,
}
#[derive(Clone)]
enum Action {
    Shell(Vec<u8>),
    Link {
        target: String,
        arguments: String,
        directory: String,
        show: i32,
    },
}
impl Item {
    pub fn invoke(&self) -> Result<()> {
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_FLAG_NO_UI,
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // HSTRINGs and PIDL storage stay alive until ShellExecuteEx returns.
        match &self.action {
            Action::Shell(bytes) => {
                // Use aligned storage for the ITEMIDLIST, including its terminator.
                let mut aligned = vec![0u16; bytes.len().div_ceil(2)];
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), aligned.as_mut_ptr().cast(), bytes.len()) };
                info.fMask |= SEE_MASK_INVOKEIDLIST;
                info.lpIDList = aligned.as_mut_ptr().cast();
                unsafe { ShellExecuteExW(&mut info) }
            }
            Action::Link {
                target,
                arguments,
                directory,
                show,
            } => {
                let target = HSTRING::from(target);
                let arguments = HSTRING::from(arguments);
                let directory = HSTRING::from(directory);
                info.lpFile = PCWSTR(target.as_ptr());
                info.lpParameters = PCWSTR(arguments.as_ptr());
                info.lpDirectory = PCWSTR(directory.as_ptr());
                info.nShow = *show;
                unsafe { ShellExecuteExW(&mut info) }
            }
        }
    }
}

/// Which app's lists: its dock ID, file and windows.
#[derive(Clone, PartialEq)]
struct Key {
    id: String,
    path: Option<String>,
    windows: Vec<isize>,
}

struct Request {
    key: Key,
    reply: Option<Sender<Vec<Section>>>,
}

/// The last lists read, with when they were read.
static LAST: Mutex<Option<(Key, Instant, Vec<Section>)>> = Mutex::new(None);

fn worker() -> Option<&'static Mutex<Sender<Request>>> {
    static WORKER: OnceLock<Option<Mutex<Sender<Request>>>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("jump-list".into())
                .spawn(move || serve(rx))
                .ok()
                .map(|_| Mutex::new(tx))
        })
        .as_ref()
}

/// Reads on one STA thread that keeps COM and the shell loaded. Only the
/// latest request is read; older ones were for an app the pointer has left.
fn serve(requests: Receiver<Request>) {
    if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() {
        return;
    }
    while let Ok(mut request) = requests.recv() {
        let mut replies: Vec<Sender<Vec<Section>>> = request.reply.take().into_iter().collect();
        while let Ok(mut newer) = requests.try_recv() {
            if newer.key != request.key {
                replies.clear();
            }
            replies.extend(newer.reply.take());
            request = newer;
        }
        let sections = collect(&request.key);
        if let Ok(mut last) = LAST.lock() {
            *last = Some((request.key, Instant::now(), sections.clone()));
        }
        for reply in replies {
            let _ = reply.send(sections.clone());
        }
    }
}

fn fresh(key: &Key) -> Option<Vec<Section>> {
    let last = LAST.lock().ok()?;
    let (known, at, sections) = last.as_ref()?;
    (known == key && at.elapsed() < FRESH).then(|| sections.clone())
}

/// Starts reading an app's lists, as the pointer rests on it.
pub fn prefetch(id: &str, path: Option<&str>, windows: &[isize]) {
    let key = Key {
        id: id.into(),
        path: path.map(str::to_owned),
        windows: windows.to_vec(),
    };
    if fresh(&key).is_some() {
        return;
    }
    if let Some(worker) = worker()
        && let Ok(worker) = worker.lock()
    {
        let _ = worker.send(Request { key, reply: None });
    }
}

/// The lists for the menu: read on hover, or now, waiting at most `WAIT`.
pub fn read(id: &str, path: Option<&str>, windows: &[isize]) -> Vec<Section> {
    let key = Key {
        id: id.into(),
        path: path.map(str::to_owned),
        windows: windows.to_vec(),
    };
    if let Some(sections) = fresh(&key) {
        return sections;
    }
    let (tx, rx) = mpsc::channel();
    let sent = worker()
        .and_then(|worker| worker.lock().ok())
        .is_some_and(|worker| worker.send(Request { key, reply: Some(tx) }).is_ok());
    if !sent {
        return Vec::new();
    }
    rx.recv_timeout(WAIT).unwrap_or_default()
}

fn collect(key: &Key) -> Vec<Section> {
    let start = Instant::now();
    let mut sections = Vec::new();
    let Some(aumid) = resolve_id(&key.id, key.path.as_deref(), &key.windows, start) else {
        return sections;
    };
    if let Ok(lists) = unsafe {
        CoCreateInstance::<_, IApplicationDocumentLists>(&ApplicationDocumentLists, None, CLSCTX_INPROC_SERVER)
    } && unsafe { lists.SetAppID(&HSTRING::from(&aumid)) }.is_ok()
    {
        // Like the taskbar, one of the two: they mostly repeat each other.
        for (title, kind) in [("Recent", ADLT_RECENT), ("Frequent", ADLT_FREQUENT)] {
            if let Ok(items) = documents(&lists, kind, start)
                && let Some(section) = section_with_items(title, items)
            {
                sections.push(section);
                break;
            }
        }
    }
    if let Some(items) = tasks(&aumid, start)
        && let Some(section) = section_with_items("Tasks", items)
    {
        sections.push(section);
    }
    sections
}

fn section_with_items(title: &'static str, mut items: Vec<Item>) -> Option<Section> {
    items.truncate(LIMIT);
    (!items.is_empty()).then_some(Section { title, items })
}

const APP_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};
const TITLE: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9),
    pid: 2,
};

fn property(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<String> {
    unsafe {
        let mut value = store.GetValue(key).ok()?;
        let result = StructuredStorage::PropVariantToStringAlloc(&value);
        let _ = StructuredStorage::PropVariantClear(&mut value);
        let ptr = result.ok()?;
        if ptr.0.is_null() {
            return None;
        }
        let text = ptr.to_string().ok();
        CoTaskMemFree(Some(ptr.0.cast()));
        text.filter(|s| !s.trim().is_empty())
    }
}

/// The shell's own answer to "which app is this window?", the ID the taskbar
/// groups, pins and keeps Jump Lists under. It covers the IDs Windows assigns
/// itself (for programs that set none) and IDs a process set for itself,
/// which no documented API reads from another process.
///
/// `IApplicationResolver` is undocumented, but its Windows 8 interface has
/// been stable since then, and Open-Shell and ExplorerPatcher rely on it.
/// If Windows ever drops it, creating it fails and the documented checks in
/// `resolve_id` still run.
mod resolver {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface, PWSTR};

    /// CLSID_StartMenuCacheAndAppResolver.
    const CLSID: GUID = GUID::from_u128(0x660b90c8_73a9_4b58_8cae_355b7f55341b);
    /// IID_IApplicationResolver8 (Windows 8 and later).
    const IID: GUID = GUID::from_u128(0xde25675a_72de_44b4_9373_05170450c140);
    /// `GetAppIDForWindow`, after IUnknown's three methods,
    /// `GetAppIDForShortcut` and `GetAppIDForShortcutObject`.
    const GET_APP_ID_FOR_WINDOW: usize = 5;

    type GetAppIdForWindow = unsafe extern "system" fn(
        this: *mut core::ffi::c_void,
        hwnd: HWND,
        id: *mut PWSTR,
        pinning_prevented: *mut BOOL,
        explicit: *mut BOOL,
        embedded_shortcut_valid: *mut BOOL,
    ) -> HRESULT;

    /// One per thread that reads Jump Lists; call on a COM thread.
    pub struct Resolver(IUnknown);

    impl Resolver {
        pub fn new() -> Option<Self> {
            unsafe {
                let unknown: IUnknown = CoCreateInstance(&CLSID, None, CLSCTX_INPROC_SERVER).ok()?;
                let mut raw = std::ptr::null_mut();
                unknown.query(&IID, &mut raw).ok().ok()?;
                // Owns the reference QueryInterface added.
                Some(Self(IUnknown::from_raw(raw)))
            }
        }

        pub fn window(&self, hwnd: isize) -> Option<String> {
            unsafe {
                let this = self.0.as_raw();
                // The object answered for IID, so its vtable is that interface's.
                let vtable = *(this as *const *const usize);
                let get: GetAppIdForWindow = std::mem::transmute(*vtable.add(GET_APP_ID_FOR_WINDOW));
                let mut id = PWSTR::null();
                let (mut prevented, mut explicit, mut embedded) = (BOOL(0), BOOL(0), BOOL(0));
                let hr = get(
                    this,
                    HWND(hwnd as _),
                    &mut id,
                    &mut prevented,
                    &mut explicit,
                    &mut embedded,
                );
                if hr.is_err() || id.is_null() {
                    return None;
                }
                let text = id.to_string().ok();
                CoTaskMemFree(Some(id.0.cast()));
                text.filter(|id| !id.trim().is_empty())
            }
        }
    }
}

fn resolve_id(id: &str, path: Option<&str>, windows: &[isize], start: Instant) -> Option<String> {
    // The taskbar's own ID for the app's window, when the shell can say.
    if !windows.is_empty()
        && let Some(resolver) = resolver::Resolver::new()
        && let Some(id) = windows.iter().find_map(|&hwnd| resolver.window(hwnd))
    {
        return Some(id);
    }
    // A window can override the process identity. The dock groups by that ID too.
    for &hwnd in windows {
        if start.elapsed() >= BUDGET {
            return None;
        }
        if let Ok(store) = unsafe { SHGetPropertyStoreForWindow::<IPropertyStore>(HWND(hwnd as _)) }
            && let Some(id) = property(&store, &APP_ID)
        {
            return Some(id);
        }
    }
    if !id.is_empty() && !id.contains(['\\', '/', ':']) {
        return Some(id.into());
    }
    for &hwnd in windows {
        if let Some(id) = crate::commands::open_windows::window_process_aumid(hwnd) {
            return Some(id);
        }
    }
    // A file-backed pin may itself be a shortcut. Never guess an implicit ID
    // from an exe path: Windows' heuristics are private and context dependent.
    let shortcut = path.unwrap_or(id);
    if shortcut.to_ascii_lowercase().ends_with(".lnk") {
        let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()? };
        let file: IPersistFile = link.cast().ok()?;
        unsafe {
            file.Load(&HSTRING::from(shortcut), STGM_READ).ok()?;
        }
        return property(&link.cast().ok()?, &APP_ID);
    }
    None
}

fn documents(lists: &IApplicationDocumentLists, kind: APPDOCLISTTYPE, start: Instant) -> Result<Vec<Item>> {
    unsafe {
        let array: IObjectArray = lists.GetList(kind, LIMIT as u32)?;
        let mut items = Vec::new();
        for i in 0..array.GetCount()?.min(64) {
            if start.elapsed() >= BUDGET {
                return Err(ERROR_TIMEOUT.to_hresult().into());
            }
            let object: IUnknown = array.GetAt(i)?;
            let item = if let Ok(link) = object.cast::<IShellLinkW>() {
                link_item(&link)?
            } else {
                let shell: IShellItem = object.cast()?;
                shell_item(&shell)?
            };
            if let Some(item) = item {
                items.push(item);
            }
            if items.len() == LIMIT {
                break;
            }
        }
        Ok(items)
    }
}

fn shell_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    unsafe {
        let ptr = item.GetDisplayName(kind).ok()?;
        if ptr.0.is_null() {
            return None;
        }
        let text = ptr.to_string().ok();
        CoTaskMemFree(Some(ptr.0.cast()));
        text
    }
}
fn shell_item(item: &IShellItem) -> Result<Option<Item>> {
    let title = shell_name(item, SIGDN_NORMALDISPLAY).unwrap_or_default();
    let path = shell_name(item, SIGDN_FILESYSPATH);
    if !visible(&title, path.as_deref(), |p| Path::new(p).exists()) {
        return Ok(None);
    }
    unsafe {
        let pidl = SHGetIDListFromObject(item)?;
        if pidl.is_null() {
            return Ok(None);
        }
        let len = ILGetSize(Some(pidl)) as usize;
        let bytes = if (2..=65536).contains(&len) {
            Some(std::slice::from_raw_parts(pidl.cast::<u8>(), len).to_vec())
        } else {
            None
        };
        CoTaskMemFree(Some(pidl.cast()));
        Ok(bytes.map(|bytes| Item {
            title,
            action: Action::Shell(bytes),
        }))
    }
}
fn text(buffer: &[u16]) -> String {
    String::from_utf16_lossy(&buffer[..buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len())])
}
fn link_item(link: &IShellLinkW) -> Result<Option<Item>> {
    unsafe {
        let mut buffer = vec![0u16; 32768];
        link.GetPath(&mut buffer, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)?;
        let target = text(&buffer);
        // Separators (PKEY_AppUserModel_IsDestListSeparator) and virtual links
        // have no executable target and are deliberately omitted.
        if target.is_empty() {
            return Ok(None);
        }
        link.GetDescription(&mut buffer)?;
        let description = text(&buffer);
        let title = link
            .cast::<IPropertyStore>()
            .ok()
            .and_then(|s| property(&s, &TITLE))
            .unwrap_or(description);
        if !visible(&title, Some(&target), |p| Path::new(p).exists()) {
            return Ok(None);
        }
        link.GetArguments(&mut buffer)?;
        let arguments = text(&buffer);
        link.GetWorkingDirectory(&mut buffer)?;
        let directory = text(&buffer);
        let show = link.GetShowCmd()?.0;
        Ok(Some(Item {
            title,
            action: Action::Link {
                target,
                arguments,
                directory,
                show,
            },
        }))
    }
}
fn visible(title: &str, path: Option<&str>, exists: impl FnOnce(&str) -> bool) -> bool {
    // Avoid network I/O here; UNC paths and shell namespace items are left to
    // the shell. The worker's deadline also covers slow local/removable disks.
    !title.trim().is_empty() && !uri(title) && path.is_none_or(|p| !local_path(p) || exists(p))
}
/// A title such as `ms-settings:network` names a link, not something to show.
/// Windows file names can't contain `:`.
fn uri(title: &str) -> bool {
    title.contains(':') && !local_path(title)
}
fn local_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/')
}
fn tasks(aumid: &str, start: Instant) -> Option<Vec<Item>> {
    let root = std::env::var_os("APPDATA")?;
    tasks_at(aumid, Path::new(&root), start)
}
fn tasks_at(aumid: &str, root: &Path, start: Instant) -> Option<Vec<Item>> {
    let path = root
        .join(r"Microsoft\Windows\Recent\CustomDestinations")
        .join(format!("{:x}.customDestinations-ms", format::app_id_hash(aumid)));
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take((format::MAX_FILE + 1) as u64).read_to_end(&mut bytes).ok()?;
    let links = format::task_links(&bytes)?;
    let mut items = Vec::new();
    for bytes in links {
        if start.elapsed() >= BUDGET {
            return None;
        }
        unsafe {
            let stream = SHCreateMemStream(Some(bytes))?;
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let persist: IPersistStream = link.cast().ok()?;
            persist.Load(&stream).ok()?;
            let item = link_item(&link).ok()?;
            if let Some(item) = item
                && items.len() < LIMIT
            {
                items.push(item);
            }
        }
    }
    Some(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sections_hide_when_empty_and_cap_at_six() {
        assert!(section_with_items("Tasks", Vec::new()).is_none());
        let items = (0..20)
            .map(|i| Item {
                title: i.to_string(),
                action: Action::Shell(Vec::new()),
            })
            .collect();
        let section = section_with_items("Tasks", items).unwrap();
        assert_eq!(section.items.len(), 6);
        assert_eq!(section.items[5].title, "5");
    }

    /// Opt-in, read-only. APPDATA for normal tests remains isolated. This probe
    /// reads only the explicitly supplied Recent root and prints titles, never
    /// targets, arguments or document paths. It does not invoke any item.
    #[test]
    #[ignore]
    fn probe_real_jump_lists() {
        let root = std::env::var_os("WISP_JUMPLIST_PROBE_APPDATA").expect("explicit probe root");
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
        }
        for id in [
            "Microsoft.VisualStudioCode",
            "com.t3tools.t3code",
            "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App",
        ] {
            let start = Instant::now();
            let hash = format::app_id_hash(id);
            let file = Path::new(&root)
                .join(r"Microsoft\Windows\Recent\CustomDestinations")
                .join(format!("{hash:x}.customDestinations-ms"));
            eprintln!("{id}: hash={hash:x}, matching custom file={}", file.is_file());
            let lists: IApplicationDocumentLists =
                unsafe { CoCreateInstance(&ApplicationDocumentLists, None, CLSCTX_INPROC_SERVER).unwrap() };
            unsafe {
                lists.SetAppID(&HSTRING::from(id)).unwrap();
            }
            for (title, kind) in [("Recent", ADLT_RECENT), ("Frequent", ADLT_FREQUENT)] {
                match documents(&lists, kind, start) {
                    Ok(items) => {
                        eprintln!("{title}: {} items, elapsed {:?}", items.len(), start.elapsed());
                        for item in items {
                            eprintln!("  {}", item.title);
                        }
                    }
                    Err(e) => eprintln!("{title}: {e}"),
                }
            }
            let task_start = Instant::now();
            match tasks_at(id, Path::new(&root), task_start) {
                Some(items) => {
                    eprintln!(
                        "Tasks: {} items, section {:?}, total {:?}",
                        items.len(),
                        task_start.elapsed(),
                        start.elapsed()
                    );
                    for item in items {
                        eprintln!("  {}", item.title);
                    }
                }
                None => eprintln!(
                    "Tasks: unavailable, section {:?}, total {:?}",
                    task_start.elapsed(),
                    start.elapsed()
                ),
            }
        }
        let mut valid = 0;
        let mut rejected = 0;
        let start = Instant::now();
        for entry in std::fs::read_dir(Path::new(&root).join(r"Microsoft\Windows\Recent\CustomDestinations"))
            .unwrap()
            .flatten()
        {
            if entry.path().extension().is_none_or(|s| s != "customDestinations-ms") {
                continue;
            }
            let file = std::fs::File::open(entry.path()).unwrap();
            let mut bytes = Vec::new();
            file.take((format::MAX_FILE + 1) as u64)
                .read_to_end(&mut bytes)
                .unwrap();
            if format::task_links(&bytes).is_some() {
                valid += 1;
            } else {
                rejected += 1;
            }
        }
        eprintln!(
            "Local format probe: {valid} recognized, {rejected} rejected, {:?}",
            start.elapsed()
        );
        unsafe {
            CoUninitialize();
        }
    }

    #[test]
    fn hides_empty_and_missing_local_targets() {
        assert!(!visible("", None, |_| true));
        assert!(!visible("ms-settings:network", None, |_| true));
        assert!(!visible("https://example.com", None, |_| true));
        assert!(visible(
            "notes.md",
            Some(
                r"C:
otes.md"
            ),
            |_| true
        ));
        assert!(!visible("File", Some(r"C:\missing"), |_| false));
        assert!(visible("File", Some(r"C:\present"), |_| true));
        assert!(visible("Virtual", None, |_| panic!("no filesystem lookup")));
        assert!(visible("Network", Some(r"\\server\file"), |_| panic!(
            "no network lookup"
        )));
    }
}
