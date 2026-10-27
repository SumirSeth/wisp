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
            let path = resolve_path(&id);
            apps.push((AppEntry { name, id, path }, item));
        }
        apps.sort_by_key(|(app, _)| app.name.to_lowercase());
        Ok(apps)
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
    let folder = unsafe {
        let ptr: PWSTR = SHGetKnownFolderPath(&guid, KF_FLAG_DEFAULT, None).ok()?;
        let folder = ptr.to_string().ok();
        CoTaskMemFree(Some(ptr.0 as _));
        folder?
    };
    Some(format!("{folder}{}", &rest[close + 1..]))
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
