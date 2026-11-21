//! Checks GitHub for a newer release and installs it next to this app.
//!
//! An update downloads the single release executable, checks SHA256SUMS.txt, then swaps it
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
    core::{PCWSTR, w},
};

const LATEST_URL: &str = "https://api.github.com/repos/SumirSeth/wisp/releases/latest";

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
            .ok_or_else(|| {
                format!(
                    "Release {} has no file named {name}. Download it from {}.",
                    self.tag, self.page
                )
            })
    }
}

/// Returns the latest release if it is newer than this app.
pub fn check() -> Result<Option<Release>, String> {
    // Nightly builds are installed by hand; stable releases would replace them.
    if crate::channel::NIGHTLY {
        return Ok(None);
    }
    // Tests can provide a local release response without contacting GitHub.
    // Only local files: the release it names is what gets installed.
    let body = match std::env::var("WISP_RELEASE_URL")
        .ok()
        .and_then(|url| url.strip_prefix("file://").map(String::from))
    {
        Some(path) => std::fs::read(path).map_err(|error| format!("Cannot check for updates: {error}"))?,
        None => get(LATEST_URL).map_err(|error| format!("Cannot check for updates: {error}"))?,
    };
    let release = parse_release(&body)?;
    Ok(is_newer(release.version(), env!("CARGO_PKG_VERSION")).then_some(release))
}

/// Runs before the launcher starts. Errors leave the previous notice alone.
pub fn check_hidden() {
    if crate::channel::NIGHTLY {
        return;
    }
    let config_path = crate::config::Config::path();
    let Ok(config) = crate::settings_config::Store::new(config_path.clone()).load() else {
        return;
    };
    if !config.check_for_updates || !unmetered() {
        return;
    }
    let Ok(release) = check() else { return };
    let path = config_path.with_file_name("update.toml");
    let mut state = crate::update_notice::State::load(&path);
    state.version = release.map(|r| r.version().to_owned()).unwrap_or_default();
    debug_assert!(state.version.is_empty() || state.available());
    if state.save(&path).is_err() {
        return;
    }
    unsafe {
        if let Ok(hwnd) = FindWindowW(crate::channel::LAUNCHER_CLASS, None) {
            let _ = PostMessageW(Some(hwnd), crate::update_notice::MESSAGE, WPARAM(0), LPARAM(0));
        }
    }
}

fn unmetered() -> bool {
    use windows::Win32::{Networking::NetworkListManager::*, System::Com::*};
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return false;
        }
        let result = (|| {
            let manager: INetworkCostManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok()?;
            let mut cost = 0;
            manager.GetCost(&mut cost, std::ptr::null()).ok()?;
            Some(cost == NLM_CONNECTION_COST_UNRESTRICTED.0 as u32)
        })()
        .unwrap_or(false);
        CoUninitialize();
        result
    }
}

/// Downloads and verifies on the caller's worker thread, then swaps this executable.
/// The worker retains the update mutex until shutdown has released launcher resources.
pub fn install(release: &Release, ready: impl FnOnce(Result<(), String>)) {
    let result = (|| {
        if let Some(manager) = crate::install::current_manager() {
            return Err(manager.instruction().into());
        }
        let lock = UpdateLock::acquire().ok_or("Another update is running. Try again when it is done.")?;
        let dir = install_dir()?;
        install_files(release, &dir, get)?;
        Ok((lock, dir))
    })();
    match result {
        Err(error) => ready(Err(error)),
        Ok((_lock, dir)) => {
            let (send, recv) = std::sync::mpsc::channel();
            *SHUTDOWN.lock().unwrap_or_else(|e| e.into_inner()) = Some(send);
            ready(Ok(()));
            if !EXITING.load(std::sync::atomic::Ordering::Acquire) && recv.recv().is_err() {
                rollback(&dir);
                return;
            }
            if let Err(error) = restart(&dir) {
                rollback(&dir);
                let _ = restart(&dir);
                crate::util::error_box(&format!(
                    "Cannot start the update; the previous Wisp was restored: {error}"
                ));
            }
        }
    }
}
static EXITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SHUTDOWN: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>> = std::sync::Mutex::new(None);
static WORKER: std::sync::Mutex<Option<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(None);
pub fn set_worker(worker: std::thread::JoinHandle<()>) {
    *WORKER.lock().unwrap_or_else(|e| e.into_inner()) = Some(worker);
}
/// Called after app::run drops hotkeys, windows, COM and the instance mutex.
pub fn finish_restart() {
    EXITING.store(true, std::sync::atomic::Ordering::Release);
    if let Some(send) = SHUTDOWN.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = send.send(());
    }
    if let Some(worker) = WORKER.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = worker.join();
    }
}
fn restart(dir: &Path) -> Result<(), String> {
    std::process::Command::new(dir.join("wisp.exe"))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
fn rollback(dir: &Path) {
    let target = dir.join("wisp.exe");
    let old = old_path(&target);
    if old.is_file() {
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::rename(old, target);
    }
}
fn install_files(
    release: &Release,
    dir: &Path,
    mut download: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<(), String> {
    let name = format!("wisp-{}-x86_64.exe", release.tag);
    // Resolve both assets before downloading anything.
    let url = release.asset_url(&name)?;
    let sums = download(release.asset_url("SHA256SUMS.txt")?)?;
    let sums = String::from_utf8(sums).map_err(|e| e.to_string())?;
    let bytes = download(url)?;
    verify(&name, &bytes, &parse_sums(&sums))?;
    let target = dir.join("wisp.exe");
    let new = dir.join("wisp.exe.new");
    if let Err(error) = std::fs::write(&new, bytes) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("Cannot save the update: {error}"));
    }
    if let Err(error) = swap(&target, &new) {
        let _ = std::fs::remove_file(new);
        return Err(error);
    }
    Ok(())
}
fn verify(asset: &str, bytes: &[u8], sums: &[(&str, &str)]) -> Result<(), String> {
    let expected = sums
        .iter()
        .find(|(name, _)| *name == asset)
        .map(|(_, hash)| *hash)
        .ok_or_else(|| format!("SHA256SUMS.txt has no checksum for {asset}. Nothing was changed."))?;
    if !sha256_hex(bytes).eq_ignore_ascii_case(expected) {
        return Err(format!("{asset} does not match its checksum. Nothing was changed."));
    }
    Ok(())
}
/// Only exact legacy filenames beside the running executable are eligible.
fn cleanup(dir: &Path) -> bool {
    if crate::migration::settings_running(dir) {
        return false;
    }
    ["wisp.exe.old", "wisp-settings.exe", "wisp-settings.exe.old"]
        .iter()
        .all(|name| {
            let path = dir.join(name);
            // Never follow a directory or symlink into another installation.
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_file() => std::fs::remove_file(path).is_ok(),
                Ok(_) => true,
                Err(e) => e.kind() == std::io::ErrorKind::NotFound,
            }
        })
}
pub fn remove_old_files() {
    if crate::install::current_manager().is_some() {
        return;
    }
    let Ok(dir) = install_dir() else {
        return;
    };
    std::thread::spawn(move || {
        for _ in 0..20 {
            if let Some(_lock) = UpdateLock::acquire()
                && cleanup(&dir)
            {
                return;
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
    swap_with(target, new, |from, to| std::fs::rename(from, to))
}
fn swap_with(
    target: &Path,
    new: &Path,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let old = old_path(target);
    match std::fs::remove_file(&old) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Cannot remove the previous backup: {e}")),
    }
    let existed = target.exists();
    if existed {
        rename(target, &old).map_err(|e| format!("Cannot back up Wisp: {e}"))?;
    }
    if let Err(error) = rename(new, target) {
        if existed && let Err(restore) = std::fs::rename(&old, target) {
            return Err(format!(
                "Cannot install Wisp: {error}. The working backup is at {}: {restore}",
                old.display()
            ));
        }
        return Err(format!("Cannot replace Wisp: {error}"));
    }
    Ok(())
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
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path).map_err(|e| e.to_string());
    }
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
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 30_000).map_err(|e| e.to_string())?;
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
    fn release() -> Release {
        parse_release(br#"{"tag_name":"v0.9.1","assets":[{"name":"wisp-v0.9.1-x86_64.exe","browser_download_url":"exe"},{"name":"SHA256SUMS.txt","browser_download_url":"sums"}]}"#).unwrap()
    }
    #[test]
    fn single_executable_assets_and_checksums_are_required_before_staging() {
        for missing in ["exe", "sums", ""] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("wisp.exe"), b"old").unwrap();
            let mut release = release();
            release.assets.retain(|(_, url)| url != missing);
            let result = install_files(&release, dir.path(), |url| {
                Ok(if url == "sums" {
                    b"wrong  wisp-v0.9.1-x86_64.exe".to_vec()
                } else {
                    b"new".to_vec()
                })
            });
            assert!(result.is_err());
            assert_eq!(std::fs::read(dir.path().join("wisp.exe")).unwrap(), b"old");
            assert!(!dir.path().join("wisp.exe.new").exists());
            assert!(!dir.path().join("wisp.exe.old").exists());
        }
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(verify("file", b"abc", &[("file", &sha256_hex(b"abc").to_uppercase())]).is_ok());
    }
    #[test]
    fn fixture_install_swaps_an_open_executable_and_restart_failure_restores_it() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("wisp.exe");
        std::fs::write(&target, b"old 0.9.0").unwrap();
        let _running = std::fs::File::open(&target).unwrap();
        let bytes = b"new 0.9.1";
        let hash = sha256_hex(bytes);
        install_files(&release(), dir.path(), |url| {
            Ok(if url == "sums" {
                format!("{hash}  wisp-v0.9.1-x86_64.exe\r\n").into_bytes()
            } else {
                bytes.to_vec()
            })
        })
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert_eq!(std::fs::read(old_path(&target)).unwrap(), b"old 0.9.0");
        assert!(restart(dir.path()).is_err()); // Fixture bytes are deliberately not a PE image.
        rollback(dir.path());
        assert_eq!(std::fs::read(target).unwrap(), b"old 0.9.0");
    }
    #[test]
    fn rename_failures_restore_old_executable_and_keep_blocked_backups() {
        for fail_at in [1, 2] {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("wisp.exe");
            let new = dir.path().join("wisp.exe.new");
            std::fs::write(&target, b"old").unwrap();
            std::fs::write(&new, b"new").unwrap();
            let mut calls = 0;
            assert!(
                swap_with(&target, &new, |from, to| {
                    calls += 1;
                    if calls == fail_at {
                        Err(std::io::Error::other("injected rename failure"))
                    } else {
                        std::fs::rename(from, to)
                    }
                })
                .is_err()
            );
            assert_eq!(std::fs::read(&target).unwrap(), b"old");
        }
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("wisp.exe");
        std::fs::write(&target, b"old").unwrap();
        std::fs::create_dir(old_path(&target)).unwrap();
        assert!(swap(&target, &dir.path().join("absent")).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"old");
    }
    #[test]
    fn staging_write_failure_and_cleanup_stay_inside_install_folder() {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("install");
        std::fs::create_dir(&install).unwrap();
        std::fs::create_dir(install.join("wisp.exe.new")).unwrap();
        let hash = sha256_hex(b"new");
        assert!(
            install_files(&release(), &install, |url| Ok(if url == "sums" {
                format!("{hash}  wisp-v0.9.1-x86_64.exe").into_bytes()
            } else {
                b"new".to_vec()
            }))
            .is_err()
        );
        for name in ["wisp.exe.old", "wisp-settings.exe", "wisp-settings.exe.old"] {
            std::fs::write(root.path().join(name), b"outside").unwrap();
            std::fs::write(install.join(name), b"inside").unwrap();
        }
        std::fs::write(install.join("wisp.exe"), b"keep").unwrap();
        assert!(cleanup(&install));
        for name in ["wisp.exe.old", "wisp-settings.exe", "wisp-settings.exe.old"] {
            assert!(root.path().join(name).exists());
            assert!(!install.join(name).exists());
        }
        assert!(install.join("wisp.exe").exists());
    }
}
