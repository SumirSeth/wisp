//! Read-only Jump Lists, collected afresh for each menu. No undocumented COM.
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
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
const BUDGET: Duration = Duration::from_millis(150);
static READING: AtomicBool = AtomicBool::new(false);

pub struct Section {
    pub title: &'static str,
    pub items: Vec<Item>,
}
pub struct Item {
    pub title: String,
    action: Action,
}
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

/// A blocked provider must not block the message loop or accumulate workers.
/// Only owned bytes/strings cross apartments; all COM objects die on their STA.
pub fn read(id: &str, path: Option<&str>, windows: &[isize]) -> Vec<Section> {
    if READING.swap(true, Ordering::AcqRel) {
        return Vec::new();
    }
    let start = Instant::now();
    let id = id.to_owned();
    let path = path.map(str::to_owned);
    let windows = windows.to_vec();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::Builder::new().name("jump-list".into()).spawn(move || {
        struct Reading;
        impl Drop for Reading {
            fn drop(&mut self) {
                READING.store(false, Ordering::Release);
            }
        }
        let _reading = Reading;
        if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() {
            return;
        }
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() };
            }
        }
        let _apartment = Apartment;
        let Some(aumid) = resolve_id(&id, path.as_deref(), &windows, start) else {
            return;
        };
        if start.elapsed() >= BUDGET {
            return;
        }
        if let Ok(lists) = unsafe {
            CoCreateInstance::<_, IApplicationDocumentLists>(&ApplicationDocumentLists, None, CLSCTX_INPROC_SERVER)
        } && unsafe { lists.SetAppID(&HSTRING::from(&aumid)) }.is_ok()
        {
            for (title, kind) in [("Recent", ADLT_RECENT), ("Frequent", ADLT_FREQUENT)] {
                if start.elapsed() >= BUDGET {
                    return;
                }
                if let Ok(items) = documents(&lists, kind, start)
                    && start.elapsed() < BUDGET
                    && !items.is_empty()
                {
                    let _ = tx.send(Section { title, items });
                }
            }
        }
        if start.elapsed() < BUDGET
            && let Some(items) = tasks(&aumid, start)
            && start.elapsed() < BUDGET
            && !items.is_empty()
        {
            let _ = tx.send(Section { title: "Tasks", items });
        }
    });
    if worker.is_err() {
        READING.store(false, Ordering::Release);
        return Vec::new();
    }
    let mut sections = Vec::new();
    while let Some(remaining) = BUDGET.checked_sub(start.elapsed()) {
        match rx.recv_timeout(remaining) {
            Ok(section) => {
                if let Some(section) = section_with_items(section.title, section.items) {
                    sections.push(section);
                }
            }
            Err(_) => break,
        }
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

fn resolve_id(id: &str, path: Option<&str>, windows: &[isize], start: Instant) -> Option<String> {
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
    !title.trim().is_empty() && path.is_none_or(|p| !local_path(p) || exists(p))
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
        assert!(!visible("File", Some(r"C:\missing"), |_| false));
        assert!(visible("File", Some(r"C:\present"), |_| true));
        assert!(visible("Virtual", None, |_| panic!("no filesystem lookup")));
        assert!(visible("Network", Some(r"\\server\file"), |_| panic!(
            "no network lookup"
        )));
    }
}
