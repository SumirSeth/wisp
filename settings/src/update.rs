//! Checks GitHub for a newer release and installs it next to this app.
//!
//! An update downloads `wisp.exe` and `wisp-settings.exe` from the latest
//! release, checks them against the release's SHA256SUMS.txt, then swaps them
//! in. Windows lets a running .exe be renamed but not overwritten, so each old
//! file moves to `<name>.old` first. The next start deletes those files.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use sha2::{Digest, Sha256};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, LPARAM, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM},
        Networking::WinHttp::*,
        System::Threading::*,
        UI::WindowsAndMessaging::*,
    },
    core::{PCWSTR, PWSTR, w},
};

const LATEST_URL: &str = "https://api.github.com/repos/SumirSeth/wisp/releases/latest";
const FILES: [(&str, &str); 2] = [("wisp.exe", "wisp"), ("wisp-settings.exe", "wisp-settings")];
/// The launcher quits when it receives this message (WM_APP_QUIT in the launcher).
const LAUNCHER_QUIT: u32 = WM_APP + 4;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// The tag, such as `v0.2.3`.
    pub tag: String,
    /// The release page.
    pub page: String,
    assets: Vec<(String, String)>,
}

impl Release {
    /// The version without the leading `v`.
    pub fn version(&self) -> &str {
        self.tag.trim_start_matches('v')
    }

    fn asset_url(&self, name: &str) -> Result<&str, String> {
        self.assets
            .iter()
            .find(|(asset, _)| asset == name)
            .map(|(_, url)| url.as_str())
            .ok_or_else(|| format!("Release {} has no file named {name}.", self.tag))
    }
}

/// Returns the latest release if it is newer than this app.
pub fn check() -> Result<Option<Release>, String> {
    let body = get(LATEST_URL).map_err(|error| format!("Cannot check for updates: {error}"))?;
    let release = parse_release(&body)?;
    Ok(is_newer(release.version(), env!("CARGO_PKG_VERSION")).then_some(release))
}

/// Downloads, verifies and installs `release`. Restarts the launcher if it was
/// running. This app keeps running the old version until it restarts.
pub fn install(release: &Release) -> Result<(), String> {
    let dir = install_dir()?;
    let _lock = UpdateLock::acquire().ok_or("Another update is running. Try again when it is done.")?;
    let sums = String::from_utf8_lossy(&get(release.asset_url("SHA256SUMS.txt")?)?).into_owned();
    let sums = parse_sums(&sums);

    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (file, stem) in FILES {
        let new = dir.join(format!("{file}.new"));
        if let Err(error) = stage(release, &sums, stem, &new) {
            for (_, new) in &staged {
                let _ = std::fs::remove_file(new);
            }
            return Err(error);
        }
        staged.push((dir.join(file), new));
    }

    let remove_staged = || {
        for (_, new) in &staged {
            let _ = std::fs::remove_file(new);
        }
    };
    let launcher_was_running = stop_launcher(&dir).inspect_err(|_| remove_staged())?;
    let mut swapped: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (target, new) in &staged {
        if let Err(error) = swap(target, new) {
            // Put back what was already replaced, so both files stay the same version.
            for (target, old) in swapped.iter().rev().filter(|(_, old)| old.exists()) {
                let _ = std::fs::remove_file(target);
                let _ = std::fs::rename(old, target);
            }
            remove_staged();
            if launcher_was_running {
                let _ = std::process::Command::new(dir.join("wisp.exe")).spawn();
            }
            return Err(error);
        }
        swapped.push((target.clone(), old_path(target)));
    }
    if launcher_was_running {
        std::process::Command::new(dir.join("wisp.exe"))
            .spawn()
            .map_err(|error| format!("Wisp is updated, but it did not restart: {error}"))?;
    }
    Ok(())
}

/// Downloads one release file to `new` and checks its checksum.
fn stage(release: &Release, sums: &[(&str, &str)], stem: &str, new: &Path) -> Result<(), String> {
    let asset = format!("{stem}-{}-x86_64.exe", release.tag);
    let expected = sums
        .iter()
        .find(|(name, _)| *name == asset)
        .map(|(_, hash)| *hash)
        .ok_or_else(|| format!("SHA256SUMS.txt has no checksum for {asset}."))?;
    let bytes = get(release.asset_url(&asset)?)?;
    if !sha256_hex(&bytes).eq_ignore_ascii_case(expected) {
        return Err(format!("{asset} does not match its checksum. Nothing was changed."));
    }
    std::fs::write(new, &bytes).map_err(|error| {
        let _ = std::fs::remove_file(new);
        format!("Cannot save the update: {error}. Download it from the release page instead.")
    })
}

/// Starts the new settings app on the About page. The caller then quits.
pub fn restart_settings() -> Result<(), String> {
    let exe = install_dir()?.join("wisp-settings.exe");
    std::process::Command::new(exe)
        .args(["--page", "about"])
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Cannot restart settings: {error}"))
}

/// Deletes the `.old` files a previous update left behind, on a background
/// thread. Right after Restart, the old settings app may still be running from
/// `wisp-settings.exe.old`, so this retries for a few seconds. It skips files
/// while an update runs, because they are that update's backups.
pub fn remove_old_files() {
    let Ok(dir) = install_dir() else {
        return;
    };
    std::thread::spawn(move || {
        for _ in 0..20 {
            if let Some(_lock) = UpdateLock::acquire() {
                let mut left = false;
                for (file, _) in FILES {
                    let old = old_path(&dir.join(file));
                    left |= old.exists() && std::fs::remove_file(&old).is_err();
                }
                if !left {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

/// A named mutex held while an update replaces files. Each thread that takes
/// it releases it on the same thread, as Windows requires.
struct UpdateLock(HANDLE);

impl UpdateLock {
    fn acquire() -> Option<Self> {
        unsafe {
            let mutex = CreateMutexW(None, false, w!("Local\\Wisp.Settings.Update")).ok()?;
            let wait = WaitForSingleObject(mutex, 0);
            if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
                Some(Self(mutex))
            } else {
                let _ = CloseHandle(mutex);
                None
            }
        }
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

fn install_dir() -> Result<PathBuf, String> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .ok_or_else(|| "Cannot find the Wisp folder.".into())
}

fn old_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

fn swap(target: &Path, new: &Path) -> Result<(), String> {
    let old = old_path(target);
    let _ = std::fs::remove_file(&old);
    let fail = |error: std::io::Error| format!("Cannot replace {}: {error}", target.display());
    if target.exists() {
        std::fs::rename(target, &old).map_err(fail)?;
    }
    std::fs::rename(new, target).map_err(|error| {
        let _ = std::fs::rename(&old, target);
        fail(error)
    })
}

/// Asks the launcher running from `dir` to quit and waits up to 5 s for it to
/// exit. Returns whether it was running. A launcher from another folder keeps
/// running.
fn stop_launcher(dir: &Path) -> Result<bool, String> {
    unsafe {
        let Ok(hwnd) = FindWindowW(w!("Wisp.Launcher"), None) else {
            return Ok(false);
        };
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let process = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
            .map_err(|error| format!("Cannot find the running Wisp: {error}"))?;
        let result = if !same_file(&process_path(process), &dir.join("wisp.exe")) {
            Ok(false)
        } else if PostMessageW(Some(hwnd), LAUNCHER_QUIT, WPARAM(0), LPARAM(0)).is_ok()
            && WaitForSingleObject(process, 5000) == WAIT_OBJECT_0
        {
            Ok(true)
        } else {
            Err("Wisp did not close. Quit it from its notification area icon, then try again.".into())
        };
        let _ = CloseHandle(process);
        result
    }
}

fn process_path(process: HANDLE) -> PathBuf {
    let mut buffer = vec![0u16; 32_768];
    let mut len = buffer.len() as u32;
    unsafe {
        if QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len).is_err() {
            return PathBuf::new();
        }
    }
    PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize]))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
        _ => false,
    }
}

fn parse_release(body: &[u8]) -> Result<Release, String> {
    let json: serde_json::Value =
        serde_json::from_slice(body).map_err(|error| format!("Cannot read the release from GitHub: {error}"))?;
    let text = |value: &serde_json::Value, key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    let tag = text(&json, "tag_name").ok_or("The release from GitHub has no version.")?;
    let page = text(&json, "html_url").unwrap_or_else(|| "https://github.com/SumirSeth/wisp/releases".into());
    let assets = json
        .get("assets")
        .and_then(|assets| assets.as_array())
        .into_iter()
        .flatten()
        .filter_map(|asset| Some((text(asset, "name")?, text(asset, "browser_download_url")?)))
        .collect();
    Ok(Release { tag, page, assets })
}

/// Reads `HASH  name` lines, the format the release workflow writes.
fn parse_sums(text: &str) -> Vec<(&str, &str)> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim().split_once(char::is_whitespace)?;
            Some((name.trim().trim_start_matches('*'), hash))
        })
        .collect()
}

fn version_parts(version: &str) -> Option<Vec<u64>> {
    let version = version.trim().trim_start_matches('v');
    // A pre-release suffix such as `-beta` is ignored.
    let version = version.split(['-', '+']).next()?;
    version.split('.').map(|part| part.parse().ok()).collect()
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (version_parts(latest), version_parts(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Requests an https URL and returns the body. WinHTTP follows redirects and
/// uses the system proxy settings.
fn get(url: &str) -> Result<Vec<u8>, String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or("Only https downloads are allowed.")?;
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let path = if path.is_empty() { "/" } else { path };
    let (host, path) = (wide(host), wide(path));
    let fail = |step: &str| format!("{step} failed: {}", windows::core::Error::from_thread().message());

    struct Handle(*mut core::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    unsafe {
        let agent = wide(format!("Wisp/{}", env!("CARGO_PKG_VERSION")));
        let session = Handle(WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(fail("Opening a connection"));
        }
        let connection = Handle(WinHttpConnect(
            session.0,
            PCWSTR(host.as_ptr()),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ));
        if connection.0.is_null() {
            return Err(fail("Connecting"));
        }
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(fail("Creating the request"));
        }
        let headers: Vec<u16> = "Accept: application/vnd.github+json, application/octet-stream\r\n"
            .encode_utf16()
            .collect();
        WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0)
            .and_then(|()| WinHttpReceiveResponse(request.0, std::ptr::null_mut()))
            .map_err(|error| format!("The download failed: {}", error.message()))?;

        let mut status = 0u32;
        let mut size = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(|error| error.message())?;
        if status != 200 {
            return Err(format!("GitHub returned HTTP {status}."));
        }

        let mut body = Vec::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read)
                .map_err(|error| format!("The download failed: {}", error.message()))?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buffer[..read as usize]);
        }
        Ok(body)
    }
}

fn wide(value: impl AsRef<str>) -> Vec<u16> {
    value.as_ref().encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_by_number() {
        assert!(is_newer("v0.2.3", "0.2.2"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.0"));
        assert!(!is_newer("v0.2.2", "0.2.2"));
        assert!(!is_newer("v0.2.1", "0.2.2"));
        assert!(!is_newer("v0.2.3-beta", "0.2.3"));
        assert!(!is_newer("nightly", "0.2.2"));
    }

    #[test]
    fn reads_the_release_json() {
        let body = br#"{
            "tag_name": "v0.2.3",
            "html_url": "https://github.com/SumirSeth/wisp/releases/tag/v0.2.3",
            "assets": [
                {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.com/sums"},
                {"name": "broken"}
            ]
        }"#;
        let release = parse_release(body).unwrap();
        assert_eq!(release.version(), "0.2.3");
        assert_eq!(release.asset_url("SHA256SUMS.txt").unwrap(), "https://example.com/sums");
        assert!(release.asset_url("broken").is_err());
        assert!(parse_release(b"{}").is_err());
    }

    #[test]
    fn reads_checksum_lines() {
        let sums = parse_sums("ABC123  wisp-v0.2.3-x86_64.exe\r\ndef456 *wisp-settings-v0.2.3-x86_64.exe\n\n");
        assert_eq!(
            sums,
            [
                ("wisp-v0.2.3-x86_64.exe", "ABC123"),
                ("wisp-settings-v0.2.3-x86_64.exe", "def456")
            ]
        );
    }

    #[test]
    fn hashes_with_sha256() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
