//! Checks GitHub for a newer release and installs it next to this app.
//!
//! An update downloads the release zip, checks it against SHA256SUMS.txt,
//! extracts `wisp.exe` and `wisp-settings.exe`, then swaps them
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

/// Runs before GPUI starts. Errors leave the previous notice alone.
pub fn check_hidden() {
    if crate::channel::NIGHTLY {
        return;
    }
    let Ok(config_path) = crate::config::path() else { return };
    let Ok(config) = crate::config::Store::new(config_path.clone()).load() else {
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

/// Downloads, verifies and installs `release`. Restarts the launcher if it was
/// running. This app keeps running the old version until it restarts.
pub fn install(release: &Release) -> Result<(), String> {
    if let Some(manager) = crate::install::current_manager() {
        return Err(manager.instruction().into());
    }
    let dir = install_dir()?;
    let _lock = UpdateLock::acquire().ok_or("Another update is running. Try again when it is done.")?;
    let sums = String::from_utf8_lossy(&get(release.asset_url("SHA256SUMS.txt")?)?).into_owned();
    let sums = parse_sums(&sums);

    let staged = stage_release(release, &sums, &dir, get)?;

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

/// Verifies the complete download before writing either staged executable.
/// Only releases without a zip use the legacy standalone assets.
fn stage_release(
    release: &Release,
    sums: &[(&str, &str)],
    dir: &Path,
    mut download: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let zip_name = format!("wisp-{}-x86_64-windows.zip", release.tag);
    let contents = if let Ok(url) = release.asset_url(&zip_name) {
        let bytes = download(url)?;
        verify(&zip_name, &bytes, sums)?;
        extract_executables(&bytes, &release.tag)?
    } else {
        let mut contents = Vec::new();
        for (_, stem) in FILES {
            let name = format!("{stem}-{}-x86_64.exe", release.tag);
            let bytes = download(release.asset_url(&name)?)?;
            verify(&name, &bytes, sums)?;
            contents.push(bytes);
        }
        contents
    };
    let staged: Vec<_> = FILES
        .iter()
        .map(|(file, _)| (dir.join(file), dir.join(format!("{file}.new"))))
        .collect();
    for ((_, new), bytes) in staged.iter().zip(contents) {
        if let Err(error) = std::fs::write(new, bytes) {
            for (_, new) in &staged {
                let _ = std::fs::remove_file(new);
            }
            return Err(format!(
                "Cannot save the update: {error}. Download it from the release page instead."
            ));
        }
    }
    Ok(staged)
}

fn verify(asset: &str, bytes: &[u8], sums: &[(&str, &str)]) -> Result<(), String> {
    let expected = sums
        .iter()
        .find(|(name, _)| *name == asset)
        .map(|(_, hash)| *hash)
        .ok_or_else(|| format!("SHA256SUMS.txt has no checksum for {asset}."))?;
    if !sha256_hex(bytes).eq_ignore_ascii_case(expected) {
        return Err(format!("{asset} does not match its checksum. Nothing was changed."));
    }
    Ok(())
}

fn extract_executables(bytes: &[u8], tag: &str) -> Result<Vec<Vec<u8>>, String> {
    use std::io::{Cursor, Read};

    let fail = |error| format!("Cannot read the release zip: {error}. Nothing was changed.");
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(fail)?;
    let folder = format!("wisp-{tag}-x86_64-windows");
    let mut contents = Vec::new();
    for (file, _) in FILES {
        // Look up exact names; never extract arbitrary paths from the archive.
        // Windows PowerShell Compress-Archive can write backslash separators.
        let slash = format!("{folder}/{file}");
        let backslash = format!("{folder}\\{file}");
        let name = [file, slash.as_str(), backslash.as_str()]
            .into_iter()
            .find(|candidate| archive.file_names().any(|name| name == *candidate))
            .ok_or_else(|| format!("The release zip has no {file}. Nothing was changed."))?;
        let mut entry = archive
            .by_name(name)
            .map_err(|error| format!("Cannot extract {file} from the release zip: {error}. Nothing was changed."))?;
        if entry.is_dir() {
            return Err(format!("The release zip has no {file}. Nothing was changed."));
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Cannot extract {file}: {error}. Nothing was changed."))?;
        contents.push(bytes);
    }
    Ok(contents)
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
    if crate::install::current_manager().is_some() {
        return;
    }
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
        let Ok(hwnd) = FindWindowW(crate::channel::LAUNCHER_CLASS, None) else {
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

    fn test_dir() -> tempfile::TempDir {
        match std::env::var_os("WISP_UPDATE_TEST_ROOT") {
            Some(root) => tempfile::tempdir_in(root).unwrap(),
            None => tempfile::tempdir().unwrap(),
        }
    }

    fn zip_bytes(folder: bool, include_settings: bool) -> Vec<u8> {
        use std::io::{Cursor, Write};
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (file, _) in FILES {
            if file == "wisp-settings.exe" && !include_settings {
                continue;
            }
            let name = if folder {
                format!("wisp-v99.0.0-x86_64-windows/{file}")
            } else {
                file.into()
            };
            writer
                .start_file(
                    name,
                    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(format!("new {file}").as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn release_with_zip() -> Release {
        parse_release(
            br#"{
            "tag_name": "v99.0.0",
            "assets": [
                {"name": "wisp-v99.0.0-x86_64-windows.zip", "browser_download_url": "zip"},
                {"name": "wisp-v99.0.0-x86_64.exe", "browser_download_url": "launcher"},
                {"name": "wisp-settings-v99.0.0-x86_64.exe", "browser_download_url": "settings"}
            ]
        }"#,
        )
        .unwrap()
    }

    fn seed_install(dir: &Path) {
        for (file, _) in FILES {
            std::fs::write(dir.join(file), format!("old {file}")).unwrap();
        }
    }

    fn assert_unchanged(dir: &Path) {
        for (file, _) in FILES {
            assert_eq!(std::fs::read(dir.join(file)).unwrap(), format!("old {file}").as_bytes());
            assert!(!dir.join(format!("{file}.new")).exists());
            assert!(!dir.join(format!("{file}.old")).exists());
        }
    }

    #[test]
    fn installs_verified_zip_with_or_without_top_folder_offline() {
        for folder in [false, true] {
            let fixture = test_dir();
            let install = fixture.path().join("install");
            std::fs::create_dir(&install).unwrap();
            seed_install(&install);
            let release = release_with_zip();
            let zip_name = "wisp-v99.0.0-x86_64-windows.zip";
            let bytes = zip_bytes(folder, true);
            std::fs::write(fixture.path().join(zip_name), &bytes).unwrap();
            std::fs::write(
                fixture.path().join("SHA256SUMS.txt"),
                format!("{}  {zip_name}\n", sha256_hex(&bytes)),
            )
            .unwrap();
            let sums = std::fs::read_to_string(fixture.path().join("SHA256SUMS.txt")).unwrap();
            let staged = stage_release(&release, &parse_sums(&sums), &install, |url| {
                assert_eq!(url, "zip", "the zip must be preferred over standalone assets");
                std::fs::read(fixture.path().join(zip_name)).map_err(|e| e.to_string())
            })
            .unwrap();
            // Exercise the same rename/.old operations as install(), without stopping or starting Wisp.
            for (target, new) in staged {
                swap(&target, &new).unwrap();
                let file = target.file_name().unwrap().to_str().unwrap();
                assert_eq!(std::fs::read(&target).unwrap(), format!("new {file}").as_bytes());
                assert_eq!(
                    std::fs::read(old_path(&target)).unwrap(),
                    format!("old {file}").as_bytes()
                );
                assert!(!new.exists());
            }
        }
    }

    #[test]
    fn reads_backslash_folder_separators() {
        use std::io::{Cursor, Write};
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (file, _) in FILES {
            writer
                .start_file(
                    format!("wisp-v99.0.0-x86_64-windows\\{file}"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(file.as_bytes()).unwrap();
        }
        let bytes = writer.finish().unwrap().into_inner();
        let contents = extract_executables(&bytes, "v99.0.0").unwrap();
        assert_eq!(contents[0], b"wisp.exe");
        assert_eq!(contents[1], b"wisp-settings.exe");
    }

    #[test]
    fn reads_windows_powershell_archive_offline() {
        // Optional fixture produced by dry-running the actual release Package step.
        let Some(path) = std::env::var_os("WISP_UPDATE_TEST_ZIP") else {
            return;
        };
        let path = PathBuf::from(path);
        let bytes = std::fs::read(&path).unwrap();
        let sums = std::fs::read_to_string(path.parent().unwrap().join("SHA256SUMS.txt")).unwrap();
        let dir = test_dir();
        seed_install(dir.path());
        let mut release = release_with_zip();
        let name = path.file_name().unwrap().to_str().unwrap();
        release.tag = name
            .strip_prefix("wisp-")
            .unwrap()
            .strip_suffix("-x86_64-windows.zip")
            .unwrap()
            .into();
        release.assets[0].0 = name.into();
        let staged = stage_release(&release, &parse_sums(&sums), dir.path(), |url| {
            assert_eq!(url, "zip");
            Ok(bytes.clone())
        })
        .unwrap();
        for (target, new) in staged {
            swap(&target, &new).unwrap();
            assert!(old_path(&target).exists());
            assert!(!new.exists());
        }
        assert!(
            std::fs::read(dir.path().join("wisp.exe"))
                .unwrap()
                .starts_with(b"fake launcher")
        );
        assert!(
            std::fs::read(dir.path().join("wisp-settings.exe"))
                .unwrap()
                .starts_with(b"fake settings")
        );
    }

    #[test]
    fn rejects_bad_zip_checksum_missing_file_and_invalid_archive_without_changes() {
        for (bytes, checksum) in [
            (zip_bytes(true, true), Some("wrong")),
            (zip_bytes(true, false), None),
            (zip_bytes(false, false), None),
            (b"invalid zip".to_vec(), None),
        ] {
            let dir = test_dir();
            seed_install(dir.path());
            let hash = checksum.map(str::to_owned).unwrap_or_else(|| sha256_hex(&bytes));
            let sums = [("wisp-v99.0.0-x86_64-windows.zip", hash.as_str())];
            let error = stage_release(&release_with_zip(), &sums, dir.path(), |url| {
                assert_eq!(url, "zip", "invalid zips must not fall back to standalone exes");
                Ok(bytes.clone())
            })
            .unwrap_err();
            assert!(error.contains("Nothing was changed"), "{error}");
            assert_unchanged(dir.path());
        }
    }

    #[test]
    fn requires_zip_checksum_and_does_not_fall_back_after_download_failure() {
        let dir = test_dir();
        seed_install(dir.path());
        let release = release_with_zip();
        let error = stage_release(&release, &[], dir.path(), |_| Ok(zip_bytes(true, true))).unwrap_err();
        assert!(error.contains("no checksum"));
        let error = stage_release(&release, &[], dir.path(), |url| {
            assert_eq!(url, "zip");
            Err("offline".into())
        })
        .unwrap_err();
        assert_eq!(error, "offline");
        assert_unchanged(dir.path());
    }

    #[test]
    fn falls_back_to_verified_standalone_assets_only_when_zip_is_absent() {
        let dir = test_dir();
        seed_install(dir.path());
        let mut release = release_with_zip();
        release.assets.retain(|(_, url)| url != "zip");
        let launcher = b"new launcher";
        let settings = b"new settings";
        let launcher_hash = sha256_hex(launcher);
        let settings_hash = sha256_hex(settings);
        let mut sums = [
            ("wisp-v99.0.0-x86_64.exe", launcher_hash.as_str()),
            ("wisp-settings-v99.0.0-x86_64.exe", "wrong"),
        ];
        let download = |url: &str| match url {
            "launcher" => Ok(launcher.to_vec()),
            "settings" => Ok(settings.to_vec()),
            _ => panic!("unexpected asset {url}"),
        };
        assert!(
            stage_release(&release, &sums, dir.path(), download)
                .unwrap_err()
                .contains("checksum")
        );
        assert_unchanged(dir.path());
        sums[1].1 = &settings_hash;
        let staged = stage_release(&release, &sums, dir.path(), download).unwrap();
        assert_eq!(staged.len(), 2);
        assert_eq!(std::fs::read(&staged[0].1).unwrap(), launcher);
        assert_eq!(std::fs::read(&staged[1].1).unwrap(), settings);
    }

    #[test]
    fn removes_both_staged_files_if_writing_fails() {
        let dir = test_dir();
        seed_install(dir.path());
        let blocked = dir.path().join("wisp-settings.exe.new");
        std::fs::create_dir(&blocked).unwrap();
        let bytes = zip_bytes(true, true);
        let hash = sha256_hex(&bytes);
        assert!(
            stage_release(
                &release_with_zip(),
                &[("wisp-v99.0.0-x86_64-windows.zip", &hash)],
                dir.path(),
                |_| Ok(bytes.clone())
            )
            .is_err()
        );
        std::fs::remove_dir(blocked).unwrap();
        assert_unchanged(dir.path());
    }
}
