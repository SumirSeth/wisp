//! Opens apps and URLs and runs commands through the Windows shell.

use windows::Win32::Foundation::{E_FAIL, ERROR_CANCELLED};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows::Win32::System::SystemInformation::GetWindowsDirectoryW;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONINFORMATION, MB_YESNO, MessageBoxW, SW_SHOWNORMAL};
use windows::core::{HSTRING, PCWSTR, Result, w};

/// Uses `AppsFolder` to open both desktop and Microsoft Store apps.
pub fn app(id: &str, path: Option<&str>, admin: bool) -> Result<()> {
    if admin && let Some(path) = path {
        return execute(path, None, true);
    }
    unsafe {
        let name = HSTRING::from(format!("shell:AppsFolder\\{id}"));
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(&name, None, &mut pidl, 0, None)?;
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_FLAG_NO_UI,
            lpIDList: pidl.cast(),
            lpVerb: if admin { w!("runas") } else { PCWSTR::null() },
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let result = ShellExecuteExW(&mut info);
        CoTaskMemFree(Some(pidl.cast()));
        result
    }
}

pub fn open(target: &str) -> Result<()> {
    execute(target, None, false)
}

pub fn shell(command: &str, admin: bool) -> Result<()> {
    execute(&system_tool(r"System32\cmd.exe"), Some(&format!("/k {command}")), admin)
}

pub fn notepad(path: &str) -> Result<()> {
    execute(
        &system_tool(r"System32\notepad.exe"),
        Some(&format!("\"{path}\"")),
        false,
    )
}

pub fn reveal(path: &str) -> Result<()> {
    execute(
        &system_tool("explorer.exe"),
        Some(&format!("/select,\"{path}\"")),
        false,
    )
}

/// Starts `wisp-settings.exe` from the folder that contains `wisp.exe`.
/// Offers the release download if the settings app is not there.
pub fn settings(welcome: bool) -> Result<()> {
    let exe = std::env::current_exe().map_err(|e| windows::core::Error::new(E_FAIL, e.to_string()))?;
    let settings = exe.with_file_name("wisp-settings.exe");
    if !settings.is_file() {
        // Keep onboarding optional and avoid a modal loop while the launcher is borrowed.
        std::thread::spawn(|| {
            let com_initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
            let answer = unsafe {
                MessageBoxW(
                    None,
                    w!(
                        "Settings and updates need wisp-settings.exe beside wisp.exe. Both come in the release zip.\n\nOpen the download page?"
                    ),
                    w!("Wisp Settings"),
                    MB_YESNO | MB_ICONINFORMATION,
                )
            };
            if answer == IDYES
                && let Err(error) = open("https://github.com/SumirSeth/wisp/releases/latest")
            {
                crate::util::error_box(&format!("Could not open the download page: {}", error.message()));
            }
            if com_initialized {
                unsafe { CoUninitialize() };
            }
        });
        return Ok(());
    }
    execute(&settings.to_string_lossy(), welcome.then_some("--welcome"), false)
}

/// Uses an absolute path to avoid running a same-named executable in the
/// current directory.
fn system_tool(relative: &str) -> String {
    let mut buf = [0u16; 260];
    let len = unsafe { GetWindowsDirectoryW(Some(&mut buf)) } as usize;
    let windows = if len > 0 && len < buf.len() {
        String::from_utf16_lossy(&buf[..len])
    } else {
        r"C:\Windows".into()
    };
    format!(r"{windows}\{relative}")
}

fn execute(file: &str, params: Option<&str>, admin: bool) -> Result<()> {
    let file = HSTRING::from(file);
    let params = params.map(HSTRING::from);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI,
        lpVerb: if admin { w!("runas") } else { w!("open") },
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: params.as_ref().map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }
}

/// Identifies user cancellation, including a dismissed UAC prompt.
pub fn is_cancelled(error: &windows::core::Error) -> bool {
    error.code() == ERROR_CANCELLED.to_hresult()
}
