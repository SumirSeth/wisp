//! Lock, sleep, hibernate, restart, shut down, sign out and empty the Recycle Bin.

use std::sync::OnceLock;

use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, LUID},
        Security::{
            AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
            TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        System::{
            Power::{GetPwrCapabilities, SYSTEM_POWER_CAPABILITIES, SetSuspendState},
            Shutdown::*,
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
        UI::Shell::{SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW},
    },
    core::PCWSTR,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    Lock,
    Sleep,
    Hibernate,
    Restart,
    ShutDown,
    SignOut,
    EmptyRecycleBin,
}

impl Power {
    pub const ALL: [Power; 7] = [
        Power::Lock,
        Power::Sleep,
        Power::Hibernate,
        Power::Restart,
        Power::ShutDown,
        Power::SignOut,
        Power::EmptyRecycleBin,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Power::Lock => "Lock",
            Power::Sleep => "Sleep",
            Power::Hibernate => "Hibernate",
            Power::Restart => "Restart",
            Power::ShutDown => "Shut down",
            Power::SignOut => "Sign out",
            Power::EmptyRecycleBin => "Empty Recycle Bin",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Power::Lock => "Locks the PC",
            Power::Sleep => "Puts the PC to sleep",
            Power::Hibernate => "Saves your session and turns off the PC",
            Power::Restart => "Restarts the PC",
            Power::ShutDown => "Turns off the PC",
            Power::SignOut => "Signs you out of Windows",
            Power::EmptyRecycleBin => "Deletes the files in the Recycle Bin",
        }
    }

    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            Power::Lock => &["lock", "lock screen"],
            Power::Sleep => &["sleep", "suspend"],
            Power::Hibernate => &["hibernate", "hibernation", "hiber"],
            Power::Restart => &["restart", "reboot"],
            Power::ShutDown => &["shut down", "shutdown", "power off", "turn off pc"],
            Power::SignOut => &["sign out", "log out", "log off", "logout", "logoff"],
            Power::EmptyRecycleBin => &["empty recycle bin", "recycle bin", "empty trash", "trash"],
        }
    }

    /// Commands that lose work ask for a second Enter.
    pub fn needs_confirmation(self) -> bool {
        matches!(
            self,
            Power::Restart | Power::ShutDown | Power::SignOut | Power::EmptyRecycleBin
        )
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Power::Lock => "\u{E72E}",
            Power::Sleep => "\u{E708}",
            Power::Hibernate => "\u{E823}",
            Power::Restart => "\u{E777}",
            Power::ShutDown => "\u{E7E8}",
            Power::SignOut => "\u{F3B1}",
            Power::EmptyRecycleBin => "\u{E74D}",
        }
    }
}

/// Power capabilities are cheap to read and cached for this process.
pub fn can_hibernate() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let mut capabilities = SYSTEM_POWER_CAPABILITIES::default();
        (unsafe { GetPwrCapabilities(&mut capabilities) }) && supports_hibernate(&capabilities)
    })
}

fn supports_hibernate(capabilities: &SYSTEM_POWER_CAPABILITIES) -> bool {
    capabilities.SystemS4 && capabilities.HiberFilePresent
}

pub fn run(power: Power) -> Result<(), String> {
    let error = |e: windows::core::Error| e.message();
    unsafe {
        match power {
            Power::Lock => LockWorkStation().map_err(error),
            Power::Sleep | Power::Hibernate => {
                if power == Power::Hibernate {
                    enable_shutdown_privilege()?;
                }
                if SetSuspendState(power == Power::Hibernate, false, false) {
                    Ok(())
                } else {
                    Err(windows::core::Error::from_thread().message())
                }
            }
            Power::Restart => exit(EWX_REBOOT),
            Power::ShutDown => exit(EWX_POWEROFF),
            Power::SignOut => ExitWindowsEx(EWX_LOGOFF, SHTDN_REASON_MAJOR_OTHER).map_err(error),
            Power::EmptyRecycleBin => SHEmptyRecycleBinW(
                None,
                PCWSTR::null(),
                SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
            )
            .map_err(error),
        }
    }
}

unsafe fn exit(flags: EXIT_WINDOWS_FLAGS) -> Result<(), String> {
    unsafe {
        enable_shutdown_privilege()?;
        ExitWindowsEx(flags, SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_FLAG_PLANNED).map_err(|e| e.message())
    }
}

/// Restart, shut down and hibernate need the shutdown privilege, which
/// processes have but must turn on.
unsafe fn enable_shutdown_privilege() -> Result<(), String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)
            .map_err(|e| e.message())?;
        let mut luid = LUID::default();
        let enabled = LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid).and_then(|()| {
            let privileges = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None)
        });
        let _ = CloseHandle(token);
        enabled.map_err(|e| e.message())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hibernate_requires_s4_and_a_hibernation_file() {
        for system_s4 in [false, true] {
            for hiber_file_present in [false, true] {
                let capabilities = SYSTEM_POWER_CAPABILITIES {
                    SystemS4: system_s4,
                    HiberFilePresent: hiber_file_present,
                    ..Default::default()
                };
                assert_eq!(supports_hibernate(&capabilities), system_s4 && hiber_file_present);
            }
        }
    }
}
