//! Lock, sleep, restart, shut down, sign out and empty the Recycle Bin.

use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, LUID},
        Security::{
            AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
            TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        System::{
            Power::SetSuspendState,
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
    Restart,
    ShutDown,
    SignOut,
    EmptyRecycleBin,
}

impl Power {
    pub const ALL: [Power; 6] = [
        Power::Lock,
        Power::Sleep,
        Power::Restart,
        Power::ShutDown,
        Power::SignOut,
        Power::EmptyRecycleBin,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Power::Lock => "Lock",
            Power::Sleep => "Sleep",
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
            Power::Restart => "\u{E777}",
            Power::ShutDown => "\u{E7E8}",
            Power::SignOut => "\u{F3B1}",
            Power::EmptyRecycleBin => "\u{E74D}",
        }
    }
}

pub fn run(power: Power) -> Result<(), String> {
    let error = |e: windows::core::Error| e.message();
    unsafe {
        match power {
            Power::Lock => LockWorkStation().map_err(error),
            Power::Sleep => {
                if SetSuspendState(false, false, false) {
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

/// Restart and shut down need the shutdown privilege, which processes have
/// but must turn on.
unsafe fn exit(flags: EXIT_WINDOWS_FLAGS) -> Result<(), String> {
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
        enabled.map_err(|e| e.message())?;
        ExitWindowsEx(flags, SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_FLAG_PLANNED).map_err(|e| e.message())
    }
}
