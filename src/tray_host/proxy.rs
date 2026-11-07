//! The proxy thread: our `Shell_TrayWnd`, a control window for broadcasts and
//! timers, and the Explorer lifetime it forwards to.

use std::cell::{Cell, RefCell};
use std::sync::{Arc, mpsc};

use windows::Win32::Foundation::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::{KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{GUID, HSTRING, PCWSTR, PWSTR, w};

use super::model::{self, Owner, Promotion};
use super::wire::{self, Notify};
use super::{OwnedIcon, Shared, lock};

/// Marks our window so Wisp and its watchdog never mistake it for Explorer's.
pub const PROXY_PROP: PCWSTR = w!("Wisp.TrayProxy");
const CONTROL_CLASS: PCWSTR = w!("Wisp.TrayHost");
const TRAY_CLASS: PCWSTR = w!("Shell_TrayWnd");
/// Checks Explorer, our FindWindow ordering and icon owners.
const CHECK_TIMER: usize = 1;
const CHECK_MS: u32 = 1000;
/// Checks in a row Explorer must outrank the proxy before apps are asked to
/// add their icons again through it.
const OUTRANKED_REFRESH: u32 = 3;
/// Rereads promotion settings after they settle.
const SETTINGS_TIMER: usize = 2;
/// Below shell32's own 7 second timeout, so senders hear our answer.
const FORWARD_MS: u32 = 5000;
/// Other shell messages to the taskbar; their senders' waits are unknown.
const OTHER_MS: u32 = 2000;
const SETTINGS_KEY: PCWSTR = w!(r"Control Panel\NotifyIconSettings");

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
    /// Read by the proxy's wndproc without borrowing HOST, which may be
    /// borrowed while Win32 delivers a sent message.
    static EXPLORER: Cell<usize> = const { Cell::new(0) };
    static SHARED: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) };
    static PROMOTIONS: RefCell<Vec<Promotion>> = const { RefCell::new(Vec::new()) };
}

struct Explorer {
    tray: HWND,
    pid: u32,
    process: HANDLE,
}

struct Host {
    shared: Arc<Shared>,
    instance: HINSTANCE,
    control: HWND,
    proxy: Option<HWND>,
    explorer: Option<Explorer>,
    taskbar_created: u32,
    mutex: HANDLE,
    settings: Option<HKEY>,
    settings_event: HANDLE,
    /// Checks a new Explorer tray has been seen, before attaching to it.
    waiting: u8,
    /// Checks in a row another Shell_TrayWnd won FindWindow.
    outranked: u32,
}

pub(super) fn run(shared: Arc<Shared>, ready: mpsc::Sender<Result<usize, String>>) {
    SHARED.with(|s| *s.borrow_mut() = Some(shared.clone()));
    match setup(shared) {
        Ok(host) => {
            let control = host.control.0 as usize;
            HOST.with(|h| *h.borrow_mut() = Some(host));
            // The dock gave up waiting; don't leave a proxy nobody owns.
            if ready.send(Ok(control)).is_err() {
                teardown();
                return;
            }
            refresh_icons();
            pump();
        }
        Err(message) => {
            let _ = ready.send(Err(message));
        }
    }
    teardown();
}

fn setup(shared: Arc<Shared>) -> Result<Host, String> {
    unsafe {
        let mutex = CreateMutexW(None, false, w!(r"Local\Wisp.TrayHost")).map_err(|e| e.message())?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(mutex);
            return Err("Another Wisp already hosts the notification area".into());
        }
        let instance: HINSTANCE = GetModuleHandleW(None).map_err(|e| e.message())?.into();
        let procs: [(PCWSTR, WNDPROC); 2] = [
            (CONTROL_CLASS, Some(control_wndproc)),
            (TRAY_CLASS, Some(proxy_wndproc)),
        ];
        for (name, proc) in procs {
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: proc,
                hInstance: instance,
                lpszClassName: name,
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                let _ = CloseHandle(mutex);
                return Err(format!("Could not register a tray window class: {:?}", GetLastError()));
            }
        }
        // Top-level, so it receives the TaskbarCreated broadcast.
        let control = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CONTROL_CLASS,
            w!("Wisp tray host"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
        .map_err(|e| {
            let _ = CloseHandle(mutex);
            e.message()
        })?;
        let mut host = Host {
            shared,
            instance,
            control,
            proxy: None,
            explorer: None,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            mutex,
            settings: None,
            settings_event: CreateEventW(None, false, false, None).unwrap_or_default(),
            waiting: 0,
            outranked: 0,
        };
        // Our own process may be elevated above Explorer's broadcast.
        let _ = ChangeWindowMessageFilterEx(control, host.taskbar_created, MSGFLT_ALLOW, None);
        host.watch_settings();
        if let Err(message) = host.attach() {
            host.close();
            return Err(message);
        }
        SetTimer(Some(control), CHECK_TIMER, CHECK_MS, None);
        Ok(host)
    }
}

/// Waits for messages, Explorer's exit and settings changes.
fn pump() {
    let mut msg = MSG::default();
    loop {
        let handles: Vec<HANDLE> = HOST.with(|h| {
            h.borrow().as_ref().map_or_else(Vec::new, |host| {
                [host.explorer.as_ref().map(|e| e.process), Some(host.settings_event)]
                    .into_iter()
                    .flatten()
                    .filter(|handle| !handle.is_invalid())
                    .collect()
            })
        });
        let wait = unsafe { MsgWaitForMultipleObjectsEx(Some(&handles), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
        let index = wait.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
        if let Some(&handle) = handles.get(index) {
            with_host(|host| host.signaled(handle));
        }
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            if msg.message == WM_QUIT {
                return;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

fn with_host(f: impl FnOnce(&mut Host)) {
    HOST.with(|h| {
        if let Ok(mut host) = h.try_borrow_mut()
            && let Some(host) = host.as_mut()
        {
            f(host);
        }
    });
}

fn teardown() {
    if let Some(mut host) = HOST.with(|h| h.borrow_mut().take()) {
        host.close();
    }
    EXPLORER.with(|e| e.set(0));
    SHARED.with(|s| s.borrow_mut().take());
}

/// Asks running apps to add their icons again, as after an Explorer
/// restart. Posted, so hung apps can't block us.
fn refresh_icons() {
    let message = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    unsafe {
        let _ = PostMessageW(Some(HWND_BROADCAST), message, WPARAM(0), LPARAM(0));
    }
}

impl Host {
    /// Pins Explorer's tray and puts our window ahead of it.
    fn attach(&mut self) -> Result<(), String> {
        let (tray, pid) = explorer_tray().ok_or("Explorer's taskbar was not found")?;
        unsafe {
            let process = OpenProcess(PROCESS_SYNCHRONIZE, false, pid).map_err(|e| e.message())?;
            self.explorer = Some(Explorer { tray, pid, process });
            EXPLORER.with(|e| e.set(tray.0 as usize));
            let mut rect = RECT::default();
            let _ = GetWindowRect(tray, &mut rect);
            let proxy = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                TRAY_CLASS,
                PCWSTR::null(),
                WS_POPUP,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                None,
                None,
                Some(self.instance),
                None,
            )
            .map_err(|e| e.message())?;
            self.proxy = Some(proxy);
            let _ = SetPropW(proxy, PROXY_PROP, Some(HANDLE(1 as _)));
            // Explorer's tray accepts these from lower integrity apps, so ours must too.
            let _ = ChangeWindowMessageFilterEx(proxy, WM_COPYDATA, MSGFLT_ALLOW, None);
            // Start and taskbar flyouts lift Explorer's tray into a higher
            // z-order band for a while; `check` keeps trying to outrank it.
            self.outranked = 0;
            if !self.outrank() {
                let found = FindWindowW(TRAY_CLASS, None).unwrap_or_default();
                eprintln!("Tray host: {:?} outranks the proxy in FindWindow for now", found.0);
                // Apps answering the startup refresh reach Explorer.
                self.outranked = OUTRANKED_REFRESH;
            }
        }
        self.waiting = 0;
        Ok(())
    }

    /// Topmost windows come first in FindWindow's z-order search.
    fn outrank(&self) -> bool {
        let Some(proxy) = self.proxy else { return false };
        unsafe {
            let _ = SetWindowPos(
                proxy,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
            FindWindowW(TRAY_CLASS, None).is_ok_and(|found| found == proxy)
        }
    }

    /// Removes the proxy, so apps reach Explorer directly, and forgets the icons.
    fn detach(&mut self) {
        EXPLORER.with(|e| e.set(0));
        if let Some(proxy) = self.proxy.take() {
            unsafe {
                let _ = RemovePropW(proxy, PROXY_PROP);
                let _ = DestroyWindow(proxy);
            }
        }
        if let Some(explorer) = self.explorer.take() {
            unsafe {
                let _ = CloseHandle(explorer.process);
            }
        }
        lock(&self.shared.model).icons.clear();
        self.shared.changed();
        self.waiting = 0;
    }

    fn close(&mut self) {
        self.detach();
        unsafe {
            let _ = KillTimer(Some(self.control), CHECK_TIMER);
            let _ = KillTimer(Some(self.control), SETTINGS_TIMER);
            let _ = DestroyWindow(self.control);
            if let Some(key) = self.settings.take() {
                let _ = RegCloseKey(key);
            }
            let _ = CloseHandle(self.settings_event);
            let _ = CloseHandle(self.mutex);
        }
    }

    fn signaled(&mut self, handle: HANDLE) {
        if self.explorer.as_ref().is_some_and(|e| e.process == handle) {
            // Leave before a new Explorer starts, so it never meets our window.
            eprintln!("Tray host: Explorer exited");
            self.detach();
        } else if handle == self.settings_event {
            unsafe {
                SetTimer(Some(self.control), SETTINGS_TIMER, 500, None);
            }
            self.watch_settings();
        }
    }

    /// Explorer restarted, or someone else asked apps to re-add their icons.
    fn taskbar_created(&mut self) {
        let current = explorer_tray();
        if let (Some(explorer), Some((tray, _))) = (&self.explorer, current)
            && explorer.tray == tray
        {
            // Our own refresh, or another tool's; the icons came through us.
            return;
        }
        self.detach();
        if current.is_some() {
            self.reattach();
        }
    }

    fn reattach(&mut self) {
        match self.attach() {
            Ok(()) => {
                eprintln!("Tray host: attached to the new Explorer");
                // Apps re-added their icons straight to the new Explorer.
                refresh_icons();
            }
            Err(message) => eprintln!("Tray host: {message}"),
        }
    }

    fn check(&mut self) {
        match &self.explorer {
            Some(explorer) => {
                let mut pid = 0;
                let alive = unsafe {
                    IsWindow(Some(explorer.tray)).as_bool()
                        && GetWindowThreadProcessId(explorer.tray, Some(&mut pid)) != 0
                        && pid == explorer.pid
                };
                if !alive {
                    eprintln!("Tray host: Explorer's taskbar is gone");
                    self.detach();
                    return;
                }
                let found = unsafe { FindWindowW(TRAY_CLASS, None).unwrap_or_default() };
                if Some(found) != self.proxy && !self.outrank() {
                    // Apps reach Explorer directly meanwhile; nothing is lost.
                    if self.outranked == 0 {
                        eprintln!("Tray host: {:?} outranks the proxy in FindWindow", found.0);
                    }
                    self.outranked += 1;
                } else {
                    // Apps that added icons meanwhile reached Explorer. Only
                    // for long spells: brief ones come and go with flyouts.
                    if self.outranked >= OUTRANKED_REFRESH {
                        refresh_icons();
                    }
                    self.outranked = 0;
                }
            }
            None => {
                if explorer_tray().is_some() {
                    self.waiting += 1;
                    // Give a new Explorer a moment to finish starting.
                    if self.waiting >= 3 {
                        self.reattach();
                    }
                } else {
                    self.waiting = 0;
                }
            }
        }
        let changed = lock(&self.shared.model).retain(|icon| owner_alive(icon.hwnd, icon.pid));
        if changed {
            self.shared.changed();
        }
    }

    /// Asks for a signal on the next change under NotifyIconSettings.
    fn watch_settings(&mut self) {
        unsafe {
            if self.settings.is_none() {
                let mut key = HKEY::default();
                if RegOpenKeyExW(HKEY_CURRENT_USER, SETTINGS_KEY, None, KEY_READ | KEY_NOTIFY, &mut key).is_ok() {
                    self.settings = Some(key);
                }
            }
            if let Some(key) = self.settings {
                let _ = RegNotifyChangeKeyValue(
                    key,
                    true,
                    REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET,
                    Some(self.settings_event),
                    true,
                );
            }
        }
        reload_promotions();
    }

    fn reresolve(&mut self) {
        reload_promotions();
        let mut model = lock(&self.shared.model);
        let mut changed = false;
        for icon in &mut model.icons {
            changed |= resolve(icon);
        }
        drop(model);
        if changed {
            self.shared.changed();
        }
    }
}

fn reload_promotions() {
    let promotions = read_promotions();
    PROMOTIONS.with(|p| *p.borrow_mut() = promotions);
}

/// Applies Windows' placement to an icon; true if it changed.
fn resolve(icon: &mut super::Icon) -> bool {
    PROMOTIONS.with(|p| {
        let promotions = p.borrow();
        let entry = model::promotion(&promotions, icon.guid, &icon.exes, icon.uid);
        let (promoted, order) = entry.map_or((None, None), |e| (e.promoted, e.order));
        let changed = icon.promoted != promoted || icon.order != order;
        icon.promoted = promoted;
        icon.order = order;
        changed
    })
}

fn owner_alive(hwnd: u32, pid: u32) -> bool {
    let hwnd = HWND(wire::handle(hwnd) as _);
    let mut current = 0;
    unsafe {
        IsWindow(Some(hwnd)).as_bool()
            && GetWindowThreadProcessId(hwnd, Some(&mut current)) != 0
            && (pid == 0 || current == pid)
    }
}

/// Explorer's own primary taskbar: the class, minus our proxy, in an explorer.exe.
pub fn explorer_tray() -> Option<(HWND, u32)> {
    crate::taskbar::taskbar_windows().into_iter().find_map(|hwnd| {
        let mut class = [0u16; 32];
        let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        (class[..len] == *"Shell_TrayWnd".encode_utf16().collect::<Vec<_>>()
            && pid != std::process::id()
            && unsafe { GetPropW(hwnd, PROXY_PROP) }.is_invalid()
            && process_path(pid).is_some_and(|path| path.to_lowercase().ends_with(r"\explorer.exe")))
        .then_some((hwnd, pid))
    })
}

pub fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&buffer[..len as usize]))
    }
}

/// shell32 and NotifyIconSettings write `{known folder}\rest`; this expands it.
fn expand_known_folder(path: &str) -> Option<String> {
    let rest = path.strip_prefix('{')?;
    let (guid, rest) = rest.split_once('}')?;
    let guid = GUID::try_from(guid).ok()?;
    unsafe {
        let folder = SHGetKnownFolderPath(&guid, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let base = folder.to_string().ok();
        CoTaskMemFree(Some(folder.0.cast()));
        Some(format!("{}{rest}", base?))
    }
}

fn exe_names(raw: &str) -> Vec<String> {
    let mut names = vec![model::normalize_exe(raw)];
    names.extend(expand_known_folder(raw).map(|path| model::normalize_exe(&path)));
    names
}

fn owner(n: &Notify) -> Owner {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(HWND(wire::handle(n.hwnd) as _), Some(&mut pid)) };
    let mut exes: Vec<String> = process_path(pid)
        .map(|path| model::normalize_exe(&path))
        .into_iter()
        .collect();
    if !n.exe.is_empty() {
        exes.extend(exe_names(&n.exe));
    }
    exes.dedup();
    Owner { pid, exes }
}

fn read_promotions() -> Vec<Promotion> {
    let mut entries = Vec::new();
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, SETTINGS_KEY, None, KEY_READ, &mut key).is_err() {
            return entries;
        }
        let order = model::order_list(&reg_binary(key, PCWSTR::null(), w!("UIOrderList")));
        let mut index = 0;
        loop {
            let mut name = [0u16; 64];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
            .is_err()
            {
                break;
            }
            index += 1;
            let name = String::from_utf16_lossy(&name[..len as usize]);
            let sub = HSTRING::from(&name);
            let exe = reg_string(key, &sub, w!("ExecutablePath")).unwrap_or_default();
            entries.push(Promotion {
                exes: exe_names(&exe),
                uid: reg_dword(key, &sub, w!("UID")),
                guid: reg_string(key, &sub, w!("IconGuid"))
                    .and_then(|guid| GUID::try_from(guid.trim_matches(['{', '}'])).ok()),
                promoted: reg_dword(key, &sub, w!("IsPromoted")).map(|value| value != 0),
                order: name
                    .parse::<u64>()
                    .ok()
                    .and_then(|id| order.iter().position(|&o| o == id)),
            });
        }
        let _ = RegCloseKey(key);
    }
    entries
}

fn reg_string(key: HKEY, sub: &HSTRING, value: PCWSTR) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            key,
            sub,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    let units = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buffer[..units.min(buffer.len())]))
}

fn reg_dword(key: HKEY, sub: &HSTRING, value: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(
            key,
            sub,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    Some(data)
}

fn reg_binary(key: HKEY, sub: PCWSTR, value: PCWSTR) -> Vec<u8> {
    let mut size = 0u32;
    unsafe {
        if RegGetValueW(key, sub, value, RRF_RT_REG_BINARY, None, None, Some(&mut size)).is_err() {
            return Vec::new();
        }
        let mut data = vec![0u8; size as usize];
        if RegGetValueW(
            key,
            sub,
            value,
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .is_err()
        {
            return Vec::new();
        }
        data.truncate(size as usize);
        data
    }
}

unsafe extern "system" fn control_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wp.0 == CHECK_TIMER => with_host(Host::check),
        WM_TIMER if wp.0 == SETTINGS_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), SETTINGS_TIMER);
            }
            with_host(Host::reresolve);
        }
        WM_CLOSE => unsafe {
            PostQuitMessage(0);
        },
        _ => {
            let created = HOST.with(|h| h.try_borrow().ok().and_then(|h| h.as_ref().map(|h| h.taskbar_created)));
            if created.is_some_and(|created| created != 0 && created == msg) {
                with_host(Host::taskbar_created);
                return LRESULT(0);
            }
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
    }
    LRESULT(0)
}

/// Sends `bytes` on to Explorer as the same WM_COPYDATA and returns its answer.
/// Blocking, so nested sends can't reorder icon updates.
fn forward_copy(wp: WPARAM, tag: usize, bytes: &[u8]) -> LRESULT {
    let tray = HWND(EXPLORER.with(Cell::get) as _);
    if tray.is_invalid() {
        return LRESULT(0);
    }
    let data = COPYDATASTRUCT {
        dwData: tag,
        cbData: bytes.len() as u32,
        lpData: bytes.as_ptr() as *mut _,
    };
    let mut result = 0usize;
    // Not SMTO_BLOCK: while Explorer handles an app bar message it sends
    // broadcasts back to this thread, and a blocked thread stalls Explorer
    // until the broadcast times out.
    let sent = unsafe {
        SendMessageTimeoutW(
            tray,
            WM_COPYDATA,
            wp,
            LPARAM(&data as *const _ as isize),
            SMTO_NORMAL | SMTO_ABORTIFHUNG,
            FORWARD_MS,
            Some(&mut result),
        )
    };
    // A timeout is a failure, never a success.
    if sent.0 == 0 {
        LRESULT(0)
    } else {
        LRESULT(result as isize)
    }
}

fn on_copy_data(wp: WPARAM, lp: LPARAM) -> LRESULT {
    let data = unsafe { (lp.0 as *const COPYDATASTRUCT).as_ref() };
    let Some(data) = data else { return LRESULT(0) };
    // Copy now: the pointer is only valid during this message.
    let bytes = if data.lpData.is_null() || data.cbData == 0 || data.cbData > 1 << 20 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize).to_vec() }
    };
    let Some(shared) = SHARED.with(|s| s.borrow().clone()) else {
        return forward_copy(wp, data.dwData, &bytes);
    };
    match data.dwData {
        wire::TAG_NOTIFY => {
            let Some(n) = wire::parse_notify(&bytes) else {
                return forward_copy(wp, data.dwData, &bytes);
            };
            let image = notify_image(&n);
            let known = {
                let model = lock(&shared.model);
                model.icons.iter().any(|icon| match n.guid {
                    Some(guid) => icon.guid == Some(guid),
                    None => icon.guid.is_none() && icon.hwnd == n.hwnd && icon.uid == n.uid,
                })
            };
            let owner = if n.op == wire::NIM_ADD || !known {
                owner(&n)
            } else {
                Owner::default()
            };
            // Explorer's own icons may come from its taskbar's thread, which
            // waits on us and can't take the forward; free it first.
            let tray = HWND(EXPLORER.with(Cell::get) as _);
            let same_thread = unsafe {
                let sender = GetWindowThreadProcessId(HWND(wire::handle(n.hwnd) as _), None);
                sender != 0 && sender == GetWindowThreadProcessId(tray, None)
            };
            if same_thread {
                unsafe {
                    let _ = ReplyMessage(LRESULT(1));
                }
            }
            let result = forward_copy(wp, data.dwData, &bytes);
            let mut model = lock(&shared.model);
            if model.apply(&n, result.0 == 1, image, owner) {
                let key = n.guid.map_or(
                    model::Key::Window {
                        hwnd: n.hwnd,
                        uid: n.uid,
                    },
                    model::Key::Guid,
                );
                if let Some(icon) = model.icons.iter_mut().find(|icon| icon.key() == key) {
                    resolve(icon);
                }
                drop(model);
                shared.changed();
            }
            result
        }
        wire::TAG_GET_RECT => {
            if let Some(query) = wire::parse_rect(&bytes) {
                let key = lock(&shared.model).find_query(&query).map(|icon| icon.key());
                if let Some(key) = key {
                    let rects = lock(&shared.rects);
                    if let Some(rect) = rects.icons.get(&key).or(rects.fallback.as_ref())
                        && let Some(reply) = wire::rect_reply(query.query, rect.left, rect.top, rect.right, rect.bottom)
                    {
                        return LRESULT(reply as isize);
                    }
                }
            }
            forward_copy(wp, data.dwData, &bytes)
        }
        // Appbar messages and anything unknown pass through unchanged.
        _ => forward_copy(wp, data.dwData, &bytes),
    }
}

/// Copies the sender's icon while it waits, before it can destroy it.
fn notify_image(n: &Notify) -> Option<Option<Arc<OwnedIcon>>> {
    if !matches!(n.op, wire::NIM_ADD | wire::NIM_MODIFY) || n.flags & wire::NIF_ICON == 0 {
        return None;
    }
    if n.hicon == 0 {
        return Some(None);
    }
    match unsafe { CopyIcon(HICON(wire::handle(n.hicon) as _)) } {
        Ok(copy) => Some(Some(Arc::new(OwnedIcon(copy)))),
        // Keep the previous image; Explorer still gets the packet.
        Err(_) => None,
    }
}

/// Other shell messages for the taskbar go to Explorer: commands such as
/// Show Desktop and private WM_USER messages from shell components.
fn forward_other(msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    let shell = msg == WM_COMMAND
        || (msg == WM_SYSCOMMAND && wp.0 & 0xFFF0 == SC_TASKLIST as usize)
        || (WM_USER..0xC000).contains(&msg);
    let tray = HWND(EXPLORER.with(Cell::get) as _);
    if !shell || tray.is_invalid() {
        return None;
    }
    unsafe {
        if InSendMessageEx(None) & (ISMEX_SEND | ISMEX_NOTIFY) == ISMEX_SEND {
            let mut result = 0usize;
            let _ = SendMessageTimeoutW(
                tray,
                msg,
                wp,
                lp,
                SMTO_NORMAL | SMTO_ABORTIFHUNG,
                OTHER_MS,
                Some(&mut result),
            );
            Some(LRESULT(result as isize))
        } else {
            let _ = PostMessageW(Some(tray), msg, wp, lp);
            Some(LRESULT(0))
        }
    }
}

unsafe extern "system" fn proxy_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA {
        return on_copy_data(wp, lp);
    }
    // Lifecycle, paint and position messages stay with our window.
    forward_other(msg, wp, lp).unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}
