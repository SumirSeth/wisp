//! Open windows, for switching to one or closing it.

use windows::{
    Win32::{
        Foundation::{CloseHandle, E_ACCESSDENIED, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM},
        Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
        System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
            QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
        },
        UI::Input::KeyboardAndMouse::{KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_ESCAPE, keybd_event},
        UI::WindowsAndMessaging::*,
    },
    core::{BOOL, PWSTR},
};

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub hwnd: isize,
    pub title: String,
    /// The program's path, for its icon. Empty if Windows does not tell.
    pub exe: String,
}

impl Window {
    /// Key into the launcher's icon map.
    pub fn icon_key(&self) -> Option<String> {
        (!self.exe.is_empty()).then(|| format!("exe:{}", self.exe.to_lowercase()))
    }
}

/// Windows that Alt+Tab would show, in z-order (most recently used first).
/// Skips this process's windows.
pub fn list() -> Vec<Window> {
    let mut windows: Vec<Window> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&raw mut windows as isize));
    }
    windows
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let windows = &mut *(lparam.0 as *mut Vec<Window>);
        if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
            return true.into();
        }
        let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if style & WS_EX_TOOLWINDOW.0 != 0 {
            return true.into();
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut cloaked).cast(), 4);
        if cloaked != 0 {
            return true.into();
        }
        let mut title = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut title);
        if len <= 0 {
            return true.into();
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == std::process::id() {
            return true.into();
        }
        windows.push(Window {
            hwnd: hwnd.0 as isize,
            title: String::from_utf16_lossy(&title[..len as usize]),
            exe: process_path(pid),
        });
        true.into()
    }
}

pub fn process_path(pid: u32) -> String {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        if ok {
            String::from_utf16_lossy(&buffer[..len as usize])
        } else {
            String::new()
        }
    }
}

/// Brings the window to the front, restoring it if minimized. Works while the
/// launcher is the foreground window.
pub fn focus(hwnd: isize) -> Result<(), String> {
    cancel_focus();
    let target = HWND(hwnd as _);
    unsafe {
        if !IsWindow(Some(target)).as_bool() {
            return Err("The window was closed.".into());
        }
        let mut pid = 0;
        GetWindowThreadProcessId(target, Some(&mut pid));
        if IsIconic(target).as_bool() {
            ShowWindowAsync(target, SW_RESTORE).ok().map_err(|e| e.message())?;
        }
        let _ = SetForegroundWindow(target);
        if GetForegroundWindow() == target {
            return Ok(());
        }
        dismiss_shell_flyout(GetForegroundWindow());
        new_press();
        let timer = SetTimer(None, 0, 20, Some(focus_tick));
        if timer == 0 {
            return Err("Could not schedule window activation.".into());
        }
        PENDING_FOCUS.with(|pending| {
            *pending.borrow_mut() = Some(FocusRequest {
                hwnd,
                pid,
                timer,
                started: std::time::Instant::now(),
                generation: FOCUS_GENERATION.with(|g| g.get()),
            })
        });
    }
    Ok(())
}

struct FocusRequest {
    hwnd: isize,
    pid: u32,
    timer: usize,
    started: std::time::Instant,
    generation: u64,
}

/// Whether a mouse button went down since the last call: the user
/// clicked somewhere, maybe in another window.
fn new_press() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON]
        .iter()
        .any(|vk| unsafe { GetAsyncKeyState(vk.0 as i32) } & 1 != 0)
}
thread_local! {
    static FOCUS_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static PENDING_FOCUS: std::cell::RefCell<Option<FocusRequest>> = const { std::cell::RefCell::new(None) };
}
fn cancel_focus() {
    FOCUS_GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
    PENDING_FOCUS.with(|pending| {
        if let Some(request) = pending.borrow_mut().take() {
            unsafe {
                let _ = KillTimer(None, request.timer);
            }
        }
    });
}
unsafe extern "system" fn focus_tick(_: HWND, _: u32, timer: usize, _: u32) {
    let request = PENDING_FOCUS.with(|pending| {
        let mut pending = pending.borrow_mut();
        if pending.as_ref().is_some_and(|r| r.timer == timer) {
            pending.take()
        } else {
            None
        }
    });
    let Some(request) = request else { return };
    let target = HWND(request.hwnd as _);
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(target, Some(&mut pid));
    }
    let done = pid != request.pid
        || !unsafe { IsWindow(Some(target)) }.as_bool()
        || unsafe { GetForegroundWindow() } == target
        || request.started.elapsed() >= std::time::Duration::from_millis(600)
        // The user moved on, maybe to another window: don't take it back.
        || new_press();
    if !done {
        unsafe {
            let _ = SetForegroundWindow(target);
        }
    }
    if done || FOCUS_GENERATION.with(|g| g.get()) != request.generation {
        unsafe {
            let _ = KillTimer(None, timer);
        }
    } else {
        PENDING_FOCUS.with(|pending| *pending.borrow_mut() = Some(request));
    }
}

/// True when the window is the foreground one, so the dock can toggle it.
pub fn is_foreground(hwnd: isize) -> bool {
    unsafe { GetForegroundWindow() == HWND(hwnd as _) }
}

/// Hides the window the way its minimize button does.
pub fn minimize(hwnd: isize) -> Result<(), String> {
    unsafe {
        let hwnd = HWND(hwnd as _);
        if !IsWindow(Some(hwnd)).as_bool() {
            return Err("The window was closed.".into());
        }
        cancel_focus();
        ShowWindowAsync(hwnd, SW_MINIMIZE).ok().map_err(|e| e.message())?;
        Ok(())
    }
}

/// Closes the Action Center, Start menu or search, which otherwise keep the
/// foreground and stop the dock from switching apps. Returns whether one was
/// in front.
fn dismiss_shell_flyout(foreground: HWND) -> bool {
    const SHELL: [&str; 4] = [
        r"\shellexperiencehost.exe",
        r"\startmenuexperiencehost.exe",
        r"\searchhost.exe",
        r"\shellhost.exe",
    ];
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(foreground, Some(&mut pid));
        if pid == 0 {
            return false;
        }
        let exe = process_path(pid).to_ascii_lowercase();
        if !SHELL.iter().any(|name| exe.ends_with(name)) {
            return false;
        }
        keybd_event(VK_ESCAPE.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_ESCAPE.0 as u8, 0, KEYEVENTF_KEYUP, 0);
        true
    }
}

/// Asks the window to close, as its close button does.
pub fn close(hwnd: isize) -> Result<(), String> {
    unsafe { PostMessageW(Some(HWND(hwnd as _)), WM_CLOSE, WPARAM(0), LPARAM(0)).map_err(|error| error.message()) }
}

/// Where a window's content runs, which is what End task would end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Owner {
    Process(u32),
    /// Explorer, or Wisp itself. Ending it would take the desktop with it.
    Shell,
    /// Windows does not say which process draws the window.
    Unknown,
}

const CORE_WINDOW: &str = "Windows.UI.Core.CoreWindow";

fn class_is(hwnd: HWND, name: &str) -> bool {
    let mut class = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut class) };
    String::from_utf16_lossy(&class[..len.max(0) as usize]) == name
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// The process that draws the window's content. A Microsoft Store app's
/// window belongs to the shared ApplicationFrameHost, which is never the
/// answer. The app's own process owns the `CoreWindow`: inside the frame
/// while the app shows, and a window of its own while it is minimized, where
/// the app's ID (`aumid`) finds it.
pub fn owner(hwnd: isize, aumid: Option<&str>) -> Owner {
    let hwnd = HWND(hwnd as _);
    let mut pid = window_pid(hwnd);
    let mut exe = process_path(pid);
    if program_name(&exe).eq_ignore_ascii_case("ApplicationFrameHost") {
        pid = hosted_content(hwnd)
            .or_else(|| aumid.and_then(detached_content))
            .unwrap_or(0);
        exe = process_path(pid);
    }
    classify(pid, &exe)
}

fn classify(pid: u32, exe: &str) -> Owner {
    if pid == 0 || exe.is_empty() || program_name(exe).eq_ignore_ascii_case("ApplicationFrameHost") {
        Owner::Unknown
    } else if pid == std::process::id() || program_name(exe).eq_ignore_ascii_case("explorer") {
        Owner::Shell
    } else {
        Owner::Process(pid)
    }
}

/// The app process whose `CoreWindow` sits inside this frame.
fn hosted_content(frame: HWND) -> Option<u32> {
    unsafe extern "system" fn find(child: HWND, lparam: LPARAM) -> BOOL {
        if class_is(child, CORE_WINDOW) {
            unsafe { *(lparam.0 as *mut u32) = window_pid(child) };
            return false.into();
        }
        true.into()
    }
    let mut pid = 0u32;
    unsafe {
        let _ = EnumChildWindows(Some(frame), Some(find), LPARAM(&raw mut pid as isize));
    }
    (pid != 0).then_some(pid)
}

/// The process of the app `aumid` whose `CoreWindow` Windows has taken out
/// of its frame, as it does while a Store app is minimized.
fn detached_content(aumid: &str) -> Option<u32> {
    struct Search<'a> {
        aumid: &'a str,
        pid: u32,
    }
    unsafe extern "system" fn find(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        if class_is(hwnd, CORE_WINDOW) {
            let pid = window_pid(hwnd);
            if process_aumid(pid).is_some_and(|id| id.eq_ignore_ascii_case(search.aumid)) {
                search.pid = pid;
                return false.into();
            }
        }
        true.into()
    }
    let mut search = Search { aumid, pid: 0 };
    unsafe {
        let _ = EnumWindows(Some(find), LPARAM(&raw mut search as isize));
    }
    (search.pid != 0).then_some(search.pid)
}

// Declared here to avoid the packaging feature of the `windows` crate.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetApplicationUserModelId(process: HANDLE, length: *mut u32, id: *mut u16) -> u32;
}

/// The AppUserModelID of a packaged app's process: the exact app, where a
/// package's folder only names the package. None for an unpackaged program.
pub fn process_aumid(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 256];
        let mut len = buffer.len() as u32;
        let status = GetApplicationUserModelId(process, &mut len, buffer.as_mut_ptr());
        let _ = CloseHandle(process);
        // The length counts the terminating null.
        (status == 0 && len > 1).then(|| String::from_utf16_lossy(&buffer[..len as usize - 1]))
    }
}

/// The AppUserModelID of the process that owns the window.
pub fn window_process_aumid(hwnd: isize) -> Option<String> {
    process_aumid(window_pid(HWND(hwnd as _)))
}

/// Why End task did not end an app.
#[derive(Debug)]
pub struct EndError {
    pub message: String,
    /// Windows refused access, as for another user's process.
    pub denied: bool,
}

/// Processes held open for End task. An open handle pins its process: if the
/// app exits while the menu is up, Windows cannot give its ID to another
/// program, so the click can only ever end the processes the menu was for.
pub struct EndTask {
    processes: Vec<HANDLE>,
    denied: Option<String>,
}

impl EndTask {
    /// Call when the menu opens. A process that has already gone is left out.
    pub fn open(pids: &[u32]) -> Self {
        let mut task = Self {
            processes: Vec::new(),
            denied: None,
        };
        for &pid in pids {
            if pid == 0 || pid == std::process::id() {
                continue;
            }
            match unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) } {
                // An exited process stays openable while anything holds it.
                Ok(process) if unsafe { WaitForSingleObject(process, 0) } == WAIT_OBJECT_0 => unsafe {
                    let _ = CloseHandle(process);
                },
                Ok(process) => task.processes.push(process),
                Err(error) if error.code() == E_ACCESSDENIED => {
                    task.denied.get_or_insert(error.message());
                }
                Err(_) => {}
            }
        }
        task
    }

    /// Whether there is anything to end, or a refusal to report.
    pub fn has_target(&self) -> bool {
        !self.processes.is_empty() || self.denied.is_some()
    }

    /// Force-closes the processes, as Task Manager's End task does. Unsaved
    /// work is lost. A process that has already exited counts as ended.
    pub fn end(self) -> Result<(), EndError> {
        let mut first = self.denied.clone().map(|message| EndError { message, denied: true });
        for &process in &self.processes {
            let ended = unsafe { TerminateProcess(process, 1) };
            if let Err(error) = ended
                && unsafe { WaitForSingleObject(process, 0) } != WAIT_OBJECT_0
            {
                first.get_or_insert(EndError {
                    message: error.message(),
                    denied: error.code() == E_ACCESSDENIED,
                });
            }
        }
        first.map_or(Ok(()), Err)
    }
}

impl Drop for EndTask {
    fn drop(&mut self) {
        for &process in &self.processes {
            unsafe {
                let _ = CloseHandle(process);
            }
        }
    }
}

/// The program's name from its path: `C:\…\Code.exe` → "Code".
pub fn program_name(exe: &str) -> &str {
    let file = exe.rsplit('\\').next().unwrap_or(exe);
    file.strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file)
}

/// The window the launcher opened over. Never resolve it from the current
/// foreground window: that would be the launcher by the time Enter is pressed.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub hwnd: isize,
    pid: u32,
    class: Vec<u16>,
    title: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
    Topmost(bool),
}

impl Target {
    pub fn capture(hwnd: HWND) -> Option<Self> {
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool()
                || !IsWindowVisible(hwnd).as_bool()
                || hwnd == GetDesktopWindow()
                || hwnd == GetShellWindow()
                || GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0
            {
                return None;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 || pid == std::process::id() {
                return None;
            }
            let mut class = vec![0u16; 256];
            let len = GetClassNameW(hwnd, &mut class);
            if len == 0 {
                return None;
            }
            class.truncate(len as usize);
            let mut title = [0u16; 512];
            let len = GetWindowTextW(hwnd, &mut title);
            if len == 0 {
                return None;
            }
            Some(Self {
                hwnd: hwnd.0 as isize,
                pid,
                class,
                title: String::from_utf16_lossy(&title[..len as usize]),
            })
        }
    }

    pub fn valid(&self) -> bool {
        unsafe {
            let hwnd = HWND(self.hwnd as _);
            let mut pid = 0;
            let mut class = [0u16; 256];
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let len = GetClassNameW(hwnd, &mut class).max(0) as usize;
            IsWindow(Some(hwnd)).as_bool() && pid == self.pid && class[..len] == self.class
        }
    }

    pub fn run(&self, control: Control) -> Result<(), String> {
        if !self.valid() {
            return Err("The original window was closed or replaced.".into());
        }
        unsafe {
            let hwnd = HWND(self.hwnd as _);
            match control {
                Control::Minimize | Control::Maximize | Control::Restore => {
                    let show = match control {
                        Control::Minimize => SW_MINIMIZE,
                        Control::Maximize => SW_MAXIMIZE,
                        _ => SW_RESTORE,
                    };
                    ShowWindowAsync(hwnd, show).ok().map_err(|e| e.message())
                }
                Control::Close => close(self.hwnd),
                Control::Topmost(on) => SetWindowPos(
                    hwnd,
                    Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
                .map_err(|e| e.message()),
            }
        }
    }
}

pub fn target_items(query: &str, target: Option<&Target>) -> Vec<crate::search::ResultItem> {
    use super::{Command, glyph, row};
    use crate::search::Action;
    let Some(target) = target.filter(|t| t.valid()) else {
        return Vec::new();
    };
    let query = query.trim().to_lowercase();
    if query.len() < 3 {
        return Vec::new();
    }
    let hwnd = HWND(target.hwnd as _);
    let (zoomed, topmost, style) = unsafe {
        (
            IsZoomed(hwnd).as_bool(),
            GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0,
            GetWindowLongW(hwnd, GWL_STYLE) as u32,
        )
    };
    let choices = [
        (
            "Minimize window",
            "minimize window",
            Control::Minimize,
            style & WS_MINIMIZEBOX.0 != 0,
        ),
        (
            if zoomed { "Restore window" } else { "Maximize window" },
            if zoomed { "restore window" } else { "maximize window" },
            if zoomed { Control::Restore } else { Control::Maximize },
            style & WS_MAXIMIZEBOX.0 != 0,
        ),
        ("Close window", "close window", Control::Close, true),
        (
            if topmost {
                "Stop keeping window on top"
            } else {
                "Keep window always on top"
            },
            "always on top",
            Control::Topmost(!topmost),
            true,
        ),
    ];
    choices
        .into_iter()
        .filter(|(_, keyword, _, enabled)| *enabled && (keyword.starts_with(&query) || query == "window"))
        .map(|(title, _, control, _)| {
            row(
                title,
                &target.title,
                Action::Command(Command::WindowControl(target.clone(), control)),
            )
            .glyph(glyph::WINDOW)
        })
        .collect()
}

#[cfg(test)]
mod target_tests {
    use super::*;
    #[test]
    fn validates_both_pid_and_class_on_an_owned_message_window() {
        use windows::core::w;
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("Wisp identity test"),
                WINDOW_STYLE(0),
                0,
                0,
                1,
                1,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .unwrap();
            let target = Target {
                hwnd: hwnd.0 as isize,
                pid: std::process::id(),
                class: "Static".encode_utf16().collect(),
                title: "Wisp identity test".into(),
            };
            // GetClassName returns the registered casing on this Windows version.
            let mut class = [0u16; 256];
            let len = GetClassNameW(hwnd, &mut class) as usize;
            let target = Target {
                class: class[..len].to_vec(),
                ..target
            };
            assert!(target.valid());
            let wrong_pid = Target {
                pid: 0,
                ..target.clone()
            };
            let wrong_class = Target {
                class: vec![1],
                ..target.clone()
            };
            for invalid in [wrong_pid, wrong_class] {
                assert!(!invalid.valid());
                assert!(invalid.run(Control::Close).is_err());
                assert!(IsWindow(Some(hwnd)).as_bool());
            }
            target.run(Control::Close).unwrap();
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, Some(hwnd), 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            assert!(!IsWindow(Some(hwnd)).as_bool());
            assert!(!target.valid());
        }
    }

    #[test]
    fn rejects_closed_targets_before_every_control() {
        let target = Target {
            hwnd: 0,
            pid: std::process::id(),
            class: vec![1],
            title: "Closed test window".into(),
        };
        assert!(Target::capture(HWND::default()).is_none());
        assert!(target_items("window", Some(&target)).is_empty());
        for control in [
            Control::Minimize,
            Control::Maximize,
            Control::Restore,
            Control::Close,
            Control::Topmost(true),
        ] {
            assert!(target.run(control).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// A process that would run for a minute if nothing ended it.
    fn sleeper() -> std::process::Child {
        Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn an_unknown_process_is_not_the_shell() {
        assert_eq!(classify(0, ""), Owner::Unknown);
        assert_eq!(classify(4242, ""), Owner::Unknown);
        assert_eq!(
            classify(4242, r"C:\Windows\System32\ApplicationFrameHost.exe"),
            Owner::Unknown
        );
        assert_eq!(classify(4242, r"C:\Windows\explorer.exe"), Owner::Shell);
        assert_eq!(classify(std::process::id(), r"C:\Apps\wisp.exe"), Owner::Shell);
        assert_eq!(classify(4242, r"C:\Windows\System32\calc.exe"), Owner::Process(4242));
    }

    #[test]
    fn end_task_ends_the_process_it_opened() {
        let mut child = sleeper();
        let task = EndTask::open(&[child.id()]);
        assert!(task.has_target());
        task.end().unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(1));
    }

    #[test]
    fn a_process_that_already_exited_counts_as_ended() {
        // It exits while the menu is open: the handle still names it, so
        // nothing else can be ended in its place, and there is no error.
        let mut child = sleeper();
        let task = EndTask::open(&[child.id()]);
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(task.end().is_ok());
        // It exited before the menu opened: there is nothing to end.
        assert!(!EndTask::open(&[child.id()]).has_target());
        // Wisp never ends itself.
        assert!(!EndTask::open(&[0, std::process::id()]).has_target());
    }
}
