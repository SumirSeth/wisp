//! Package-manager paths. Keep these checks lexical: resolving a Scoop junction
//! would turn its stable `current` path back into a versioned path.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    Winget,
    Scoop,
}

impl Manager {
    pub fn instruction(self) -> &'static str {
        match self {
            Self::Winget => "Installed with winget. Update with winget upgrade SumirSeth.Wisp.",
            Self::Scoop => "Installed with Scoop. Update with scoop update wisp.",
        }
    }
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_start_matches(r"\\?\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn under(path: &str, root: &Path, suffix: &str) -> bool {
    let root = normalized(root);
    let suffix = suffix.to_ascii_lowercase();
    !root.is_empty() && path.starts_with(&format!("{root}\\{suffix}\\"))
}

/// Pure detection with explicit environment roots, including custom Scoop roots.
pub fn manager(
    path: &Path,
    local_app_data: Option<&Path>,
    program_files: Option<&Path>,
    scoop: Option<&Path>,
) -> Option<Manager> {
    let path = normalized(path);
    if local_app_data.is_some_and(|root| under(&path, root, r"Microsoft\WinGet\Packages"))
        || program_files.is_some_and(|root| under(&path, root, r"WinGet\Packages"))
    {
        return Some(Manager::Winget);
    }
    if path.contains(r"\scoop\apps\") || scoop.is_some_and(|root| under(&path, root, "apps")) {
        return Some(Manager::Scoop);
    }
    None
}

pub fn current_manager() -> Option<Manager> {
    let exe = std::env::current_exe().ok()?;
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let scoop = std::env::var_os("SCOOP").map(PathBuf::from);
    manager(&exe, local.as_deref(), program_files.as_deref(), scoop.as_deref())
}

/// Use Scoop's junction for Wisp autostart so upgrades can remove old versions.
pub fn autostart_path(exe: &Path, scoop: Option<&Path>) -> PathBuf {
    if manager(exe, None, None, scoop) != Some(Manager::Scoop) {
        return exe.to_path_buf();
    }
    let Some(version) = exe.parent() else {
        return exe.to_path_buf();
    };
    let Some(app) = version.parent() else {
        return exe.to_path_buf();
    };
    if !app.file_name().is_some_and(|name| name.eq_ignore_ascii_case("wisp"))
        || !app
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name.eq_ignore_ascii_case("apps"))
    {
        return exe.to_path_buf();
    }
    app.join("current").join(exe.file_name().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_package_roots_without_prefix_collisions() {
        let local = Some(Path::new(r"C:\Users\Me\AppData\Local"));
        let programs = Some(Path::new(r"C:\Program Files"));
        for path in [
            r"c:\USERS\me\APPDATA\local\Microsoft\WinGet\Packages\SumirSeth.Wisp_source\wisp-settings.exe",
            r"C:\Program Files\WinGet\Packages\SumirSeth.Wisp_source\wisp.exe",
            r"\\?\C:\Program Files\WinGet\Packages\SumirSeth.Wisp_source\wisp.exe",
        ] {
            assert_eq!(
                manager(Path::new(path), local, programs, None),
                Some(Manager::Winget),
                "{path}"
            );
        }
        for path in [
            r"C:\Apps\wisp.exe",
            r"C:\Program Files\WinGet\PackagesExtra\wisp.exe",
            r"C:\Users\Me\AppData\LocalExtra\Microsoft\WinGet\Packages\wisp.exe",
        ] {
            assert_eq!(manager(Path::new(path), local, programs, None), None);
        }
        assert_eq!(
            manager(
                Path::new(r"C:\Program Files\WinGet\Packages\wisp.exe"),
                None,
                None,
                None
            ),
            None
        );
    }

    #[test]
    fn detects_default_and_custom_scoop_roots() {
        for path in [
            r"C:\Users\Me\SCOOP\apps\wisp\0.6.0\wisp.exe",
            r"C:\Users\Me\scoop\apps\wisp\current\wisp-settings.exe",
            r"D:\Tools\apps\wisp\0.6.0\wisp.exe",
        ] {
            assert_eq!(
                manager(Path::new(path), None, None, Some(Path::new(r"D:\Tools"))),
                Some(Manager::Scoop)
            );
        }
        for path in [r"C:\scoop\apps-old\wisp.exe", r"D:\ToolsExtra\apps\wisp.exe"] {
            assert_eq!(manager(Path::new(path), None, None, Some(Path::new(r"D:\Tools"))), None);
        }
    }

    #[test]
    fn autostart_uses_scoop_current_only_for_wisp() {
        let scoop = Some(Path::new(r"D:\Tools"));
        for path in [
            r"D:\Tools\apps\wisp\0.6.0\wisp.exe",
            r"D:\Tools\apps\wisp\current\wisp.exe",
        ] {
            assert_eq!(
                autostart_path(Path::new(path), scoop),
                PathBuf::from(r"D:\Tools\apps\wisp\current\wisp.exe")
            );
        }
        assert_eq!(
            autostart_path(Path::new(r"C:\Users\Me\scoop\apps\Wisp\0.6.0\wisp.exe"), None),
            PathBuf::from(r"C:\Users\Me\scoop\apps\Wisp\current\wisp.exe")
        );
        for path in [
            r"D:\Tools\apps\another\0.6.0\wisp.exe",
            r"C:\Wisp\wisp.exe",
            r"C:\Program Files\WinGet\Packages\SumirSeth.Wisp_source\wisp.exe",
        ] {
            assert_eq!(autostart_path(Path::new(path), scoop), PathBuf::from(path));
        }
    }
}
