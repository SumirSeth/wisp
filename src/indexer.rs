//! Indexes desktop and Microsoft Store apps from `shell:AppsFolder`,
//! the source for Start's "All apps". The worker thread sends the app list
//! before loading icons so the launcher can show results sooner.
//! Loaded icons stay cached across rescans.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{HSTRING, Interface, PWSTR, Result};

#[derive(Debug, Clone)]
pub struct AppEntry {
    pub name: String,
    /// Lowercase program names, computed once by the indexer.
    pub aliases: Vec<String>,
    /// Windows shell parsing name in `shell:AppsFolder`: a path or an AppUserModelID.
    pub id: String,
    /// File path for desktop apps, if Windows can resolve it.
    pub path: Option<String>,
}

/// Premultiplied BGRA pixels.
pub struct IconPixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

pub enum IndexEvent {
    Apps(Vec<AppEntry>),
    Icons(Vec<(String, Arc<IconPixels>)>),
}

pub struct Indexer {
    requests: Sender<()>,
}

impl Indexer {
    /// Posts `notify_msg` to `hwnd` after each event to wake the message loop.
    pub fn spawn(hwnd: HWND, notify_msg: u32, events: Sender<IndexEvent>, icon_size: i32) -> Indexer {
        let (requests, rx) = channel();
        let hwnd = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("indexer".into())
            .spawn(move || {
                worker(rx, events, icon_size, move || unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as _)), notify_msg, WPARAM(0), LPARAM(0));
                })
            })
            .expect("could not start app indexing thread");
        Indexer { requests }
    }

    pub fn rescan(&self) {
        let _ = self.requests.send(());
    }
}

fn worker(requests: Receiver<()>, events: Sender<IndexEvent>, icon_size: i32, notify: impl Fn()) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let mut icons_loaded = HashSet::new();
    while requests.recv().is_ok() {
        // One scan handles all pending requests.
        while requests.try_recv().is_ok() {}

        let items = match unsafe { enumerate_apps() } {
            Ok(items) => items,
            Err(e) => {
                eprintln!("could not list apps: {e}");
                continue;
            }
        };
        let apps = items.iter().map(|(app, _)| app.clone()).collect();
        if events.send(IndexEvent::Apps(apps)).is_err() {
            return;
        }
        notify();

        let mut batch = Vec::new();
        for (app, item) in &items {
            if icons_loaded.contains(&app.id) {
                continue;
            }
            // Cache only loaded icons so the next rescan retries failed loads.
            if let Some(pixels) = unsafe { load_icon(item, icon_size) } {
                icons_loaded.insert(app.id.clone());
                batch.push((app.id.clone(), Arc::new(pixels)));
            }
            if batch.len() >= 32 {
                if events.send(IndexEvent::Icons(std::mem::take(&mut batch))).is_err() {
                    return;
                }
                notify();
            }
        }
        if !batch.is_empty() {
            let _ = events.send(IndexEvent::Icons(batch));
            notify();
        }
    }
}

unsafe fn enumerate_apps() -> Result<Vec<(AppEntry, IShellItem)>> {
    unsafe {
        let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
        let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
        let mut apps = Vec::new();
        let mut seen = HashSet::new();
        loop {
            let mut slot = [None];
            let mut fetched = 0;
            if items.Next(&mut slot, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = slot[0].take() else { break };
            let (Some(name), Some(id)) = (
                display_name(&item, SIGDN_NORMALDISPLAY),
                display_name(&item, SIGDN_PARENTRELATIVEPARSING),
            ) else {
                continue;
            };
            if !is_launchable(&name, &id) || !seen.insert(id.clone()) {
                continue;
            }
            let path = display_name(&item, SIGDN_FILESYSPATH).or_else(|| resolve_path(&id));
            let target = path
                .as_deref()
                .and_then(shortcut_target)
                .or_else(|| shell_target(&item));
            let aliases = app_aliases(&id, path.as_deref(), target.as_deref());
            apps.push((
                AppEntry {
                    name,
                    id,
                    path,
                    aliases,
                },
                item,
            ));
        }
        // AppsFolder may omit the shortcuts inside Windows Tools (formerly
        // Administrative Tools). Include only installed shortcuts, never commands.
        for folder in [FOLDERID_AdminTools, FOLDERID_CommonAdminTools] {
            if let Some(path) = known_folder_path(&folder) {
                add_tool_shortcuts(std::path::Path::new(&path), &mut apps, &mut seen);
            }
        }
        apps.sort_by_key(|(app, _)| app.name.to_lowercase());
        Ok(apps)
    }
}

/// Only filenames of actual executable/console targets become keywords. In
/// particular, a localized .lnk filename and directories are never aliases.
fn app_aliases(id: &str, path: Option<&str>, shortcut_target: Option<&str>) -> Vec<String> {
    let mut aliases = Vec::new();
    let target = shortcut_target.or(path).unwrap_or(id);
    let file = target.rsplit(['\\', '/']).next().unwrap_or(target);
    if let Some((stem, ext)) = file.rsplit_once('.')
        && !stem.is_empty()
        && (ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("msc"))
    {
        aliases.push(stem.to_lowercase());
    }
    // Inbox Store apps have opaque AUMIDs rather than executable paths. Match
    // the complete package family, not a prefix that could match another app.
    const INBOX: &[(&str, &str)] = &[
        ("Microsoft.WindowsCalculator_8wekyb3d8bbwe", "calc"),
        ("Microsoft.WindowsTerminal_8wekyb3d8bbwe", "wt"),
        ("Microsoft.Paint_8wekyb3d8bbwe", "mspaint"),
        ("Microsoft.WindowsNotepad_8wekyb3d8bbwe", "notepad"),
    ];
    if let Some((family, _)) = id.split_once('!') {
        for &(package, alias) in INBOX {
            if family.eq_ignore_ascii_case(package) && !aliases.iter().any(|a| a == alias) {
                aliases.push(alias.into());
            }
        }
    }
    aliases
}

/// AppsFolder can expose an opaque desktop AUMID; ask its property store for
/// the saved link target rather than guessing a program from that identifier.
unsafe fn shell_target(item: &IShellItem) -> Option<String> {
    const TARGET: windows::Win32::Foundation::PROPERTYKEY = windows::Win32::Foundation::PROPERTYKEY {
        fmtid: windows::core::GUID::from_u128(0xb9b4b3fc_2b51_4a42_b5d8_324146afcf25),
        pid: 2, // System.Link.TargetParsingPath
    };
    unsafe {
        let item: IShellItem2 = item.cast().ok()?;
        let ptr = item.GetString(&TARGET).ok()?;
        let target = ptr.to_string().ok().filter(|s| !s.is_empty());
        CoTaskMemFree(Some(ptr.0 as _));
        target
    }
}

/// Load the saved target only: no Resolve call, network search or UI.
fn shortcut_target(path: &str) -> Option<String> {
    if !path.to_ascii_lowercase().ends_with(".lnk") {
        return None;
    }
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let persist: IPersistFile = link.cast().ok()?;
        persist.Load(&HSTRING::from(path), STGM_READ).ok()?;
        let mut buffer = [0u16; 32768];
        link.GetPath(&mut buffer, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
            .ok()?;
        let end = buffer.iter().position(|&c| c == 0)?;
        if end == 0 {
            return None;
        }
        let target = String::from_utf16_lossy(&buffer[..end]);
        // MMC shortcuts save the console in their arguments, not GetPath.
        if target.rsplit(['\\', '/']).next()?.eq_ignore_ascii_case("mmc.exe") {
            buffer.fill(0);
            if link.GetArguments(&mut buffer).is_ok() {
                let end = buffer.iter().position(|&c| c == 0)?;
                let arguments = String::from_utf16_lossy(&buffer[..end]);
                if let Some(console) = console_target(&arguments) {
                    return Some(console.into());
                }
            }
        }
        Some(target)
    }
}

/// MMC's first argument is a console path, optionally quoted; flags are not
/// keywords. This reads saved shortcut data and never executes the arguments.
fn console_target(arguments: &str) -> Option<&str> {
    let arguments = arguments.trim();
    let first = if let Some(quoted) = arguments.strip_prefix('"') {
        quoted.split_once('"')?.0
    } else {
        arguments.split_whitespace().next()?
    };
    first.to_ascii_lowercase().ends_with(".msc").then_some(first)
}

unsafe fn add_tool_shortcuts(
    folder: &std::path::Path,
    apps: &mut Vec<(AppEntry, IShellItem)>,
    seen: &mut HashSet<String>,
) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            unsafe { add_tool_shortcuts(&path, apps, seen) };
            continue;
        }
        let id = path.to_string_lossy().into_owned();
        if !id.to_ascii_lowercase().ends_with(".lnk") || !seen.insert(id.clone()) {
            continue;
        }
        let Some(target) = shortcut_target(&id) else { continue };
        let aliases = app_aliases(&id, Some(&id), Some(&target));
        if aliases.is_empty() || apps.iter().any(|(app, _)| app.aliases == aliases) {
            continue;
        }
        unsafe {
            let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(&id), None) else {
                continue;
            };
            let Some(name) = display_name(&item, SIGDN_NORMALDISPLAY) else {
                continue;
            };
            if is_launchable(&name, &id) {
                apps.push((
                    AppEntry {
                        name,
                        aliases,
                        path: Some(id.clone()),
                        id,
                    },
                    item,
                ));
            }
        }
    }
}

/// Excludes uninstallers, documents and web links from app results.
fn is_launchable(name: &str, id: &str) -> bool {
    const SKIP_EXTENSIONS: &[&str] = &[
        ".url", ".txt", ".chm", ".pdf", ".htm", ".html", ".rtf", ".ini", ".log", ".md", ".xml",
    ];
    let id = id.to_ascii_lowercase();
    !name.is_empty()
        && !name.to_ascii_lowercase().starts_with("uninstall")
        && !id.starts_with("http:")
        && !id.starts_with("https:")
        && !SKIP_EXTENSIONS.iter().any(|ext| id.ends_with(ext))
}

unsafe fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    unsafe {
        let ptr = item.GetDisplayName(kind).ok()?;
        let text = ptr.to_string().ok();
        CoTaskMemFree(Some(ptr.0 as _));
        text
    }
}

/// Desktop app IDs use paths such as `{KnownFolderGUID}\sub\app.exe` or `C:\path\app.exe`.
fn resolve_path(id: &str) -> Option<String> {
    if id.as_bytes().get(1..3) == Some(b":\\") {
        return Some(id.to_string());
    }
    let rest = id.strip_prefix('{')?;
    let close = rest.find('}')?;
    let guid = unsafe { CLSIDFromString(&HSTRING::from(&id[..close + 2])).ok()? };
    let folder = known_folder_path(&guid)?;
    Some(format!("{folder}{}", &rest[close + 1..]))
}

fn known_folder_path(guid: &windows::core::GUID) -> Option<String> {
    unsafe {
        let ptr: PWSTR = SHGetKnownFolderPath(guid, KF_FLAG_DEFAULT, None).ok()?;
        let folder = ptr.to_string().ok();
        CoTaskMemFree(Some(ptr.0 as _));
        folder
    }
}

pub(crate) unsafe fn load_icon(item: &IShellItem, size: i32) -> Option<IconPixels> {
    unsafe {
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let bitmap = factory
            .GetImage(windows::Win32::Foundation::SIZE { cx: size, cy: size }, SIIGBF_ICONONLY)
            .ok()?;
        let pixels = bitmap_pixels(bitmap);
        let _ = DeleteObject(bitmap.into());
        pixels
    }
}

unsafe fn bitmap_pixels(bitmap: HBITMAP) -> Option<IconPixels> {
    unsafe {
        let mut info = BITMAP::default();
        if GetObjectW(
            bitmap.into(),
            size_of::<BITMAP>() as i32,
            Some((&mut info as *mut BITMAP).cast()),
        ) == 0
        {
            return None;
        }
        let (width, height) = (info.bmWidth, info.bmHeight.abs());
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // Negative height stores pixels from top to bottom.
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (width * height * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            Some(bgra.as_mut_ptr().cast()),
            &mut header,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        if lines == 0 {
            return None;
        }

        let (pixels, _) = bgra.as_chunks_mut::<4>();
        if pixels.iter().all(|p| p[3] == 0) {
            // Icons without an alpha channel would be invisible unless made opaque.
            pixels.iter_mut().for_each(|p| p[3] = 255);
        } else if pixels.iter().any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]) {
            // Direct2D needs premultiplied alpha for these icons.
            for p in pixels.iter_mut() {
                let a = p[3] as u32;
                for c in &mut p[..3] {
                    *c = ((*c as u32 * a + 127) / 255) as u8;
                }
            }
        }
        Some(IconPixels {
            width: width as u32,
            height: height as u32,
            bgra,
        })
    }
}

#[cfg(test)]
mod alias_tests {
    use super::{app_aliases, console_target};

    #[test]
    fn derives_mmc_console_stems_from_saved_arguments() {
        for args in [
            r"C:\Windows\System32\devmgmt.msc /s",
            r#""C:\Windows Tools\devmgmt.msc" /s"#,
        ] {
            let target = console_target(args).unwrap();
            assert_eq!(app_aliases("localized.lnk", None, Some(target)), ["devmgmt"]);
        }
        for args in ["", "/s", "tool.exe", "tool.msc.exe", "\"unterminated.msc"] {
            assert!(console_target(args).is_none());
        }
    }

    #[test]
    fn derives_only_program_stems_from_paths_and_link_targets() {
        for stem in [
            "cmd", "taskmgr", "regedit", "mspaint", "notepad", "control", "msconfig", "mmc", "explorer", "calc",
        ] {
            let path = format!(r"C:\Windows\System32\{stem}.EXE");
            assert_eq!(app_aliases(&path, Some(&path), None), [stem]);
            assert_eq!(app_aliases("localized.lnk", Some("localized.lnk"), Some(&path)), [stem]);
        }
        assert_eq!(
            app_aliases("localized.lnk", None, Some(r"C:\Windows\System32\devmgmt.msc")),
            ["devmgmt"]
        );
        for path in [
            r"C:\cmd\manual.txt",
            r"C:\cmd\localized.lnk",
            "https://example.com",
            "Unknown.Package!App",
        ] {
            assert!(app_aliases(path, Some(path), None).is_empty());
        }
    }

    #[test]
    fn maps_only_known_inbox_package_families() {
        for (family, alias) in [
            ("Microsoft.WindowsCalculator", "calc"),
            ("Microsoft.WindowsTerminal", "wt"),
            ("Microsoft.Paint", "mspaint"),
            ("Microsoft.WindowsNotepad", "notepad"),
        ] {
            let id = format!("{family}_8wekyb3d8bbwe!App");
            assert_eq!(app_aliases(&id, None, None), [alias]);
            assert_eq!(app_aliases(&id.to_uppercase(), None, None), [alias]);
        }
        assert!(app_aliases("Microsoft.WindowsCalculator_fake!App", None, None).is_empty());
        assert!(app_aliases("Microsoft.WindowsCalculator_8wekyb3d8bbwe.extra!App", None, None).is_empty());
    }
}
