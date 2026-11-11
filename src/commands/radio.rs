//! Wi-Fi and Bluetooth radios, through Windows.Devices.Radios. This API works
//! from desktop apps without a package.

use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};

fn find(kind: RadioKind) -> windows::core::Result<Option<Radio>> {
    let radios = Radio::GetRadiosAsync()?.join()?;
    Ok(radios.into_iter().find(|radio| radio.Kind().is_ok_and(|k| k == kind)))
}

/// `None` if the PC has no such radio.
pub fn is_on(kind: RadioKind) -> Option<bool> {
    let radio = find(kind).ok()??;
    Some(radio.State().is_ok_and(|state| state == RadioState::On))
}

/// `name` is the radio's name in error messages, such as "Wi-Fi".
pub fn set(kind: RadioKind, name: &str, on: bool) -> Result<(), String> {
    set_with_access(kind, name, on, request_access()?)
}

pub fn set_with_access(kind: RadioKind, name: &str, on: bool, access: Access) -> Result<(), String> {
    let access = access()?;
    if access != RadioAccessStatus::Allowed {
        return Err(format!("{name} change denied. Check Windows radio privacy settings."));
    }
    let radio = find(kind)
        .map_err(|error| error.message())?
        .ok_or_else(|| format!("This PC has no {name} adapter."))?;
    let state = if on { RadioState::On } else { RadioState::Off };
    match radio.SetStateAsync(state).and_then(|op| op.join()) {
        Ok(status) if status == RadioAccessStatus::Allowed => {
            // Allowed acknowledges the request; wait for the actual radio state.
            for _ in 0..40 {
                if radio.State().is_ok_and(|actual| actual == state) {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(format!("{name} did not reach the requested state. Try again."))
        }
        Ok(_) => Err(format!("{name} change denied. Check Windows radio privacy settings.")),
        Err(error) => Err(error.message()),
    }
}

/// Start consent on the UI thread, then move only its wait to the MTA worker.
pub type Access = Box<dyn FnOnce() -> Result<RadioAccessStatus, String> + Send>;
pub fn request_access() -> Result<Access, String> {
    let operation = Radio::RequestAccessAsync().map_err(|error| error.message())?;
    Ok(Box::new(move || operation.join().map_err(|error| error.message())))
}

/// Keep the UI apartment available for consent and its asynchronous completion.
pub struct UiRuntime;
impl UiRuntime {
    pub fn new() -> windows::core::Result<Self> {
        unsafe {
            windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_SINGLETHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for UiRuntime {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::WinRT::RoUninitialize();
        }
    }
}
