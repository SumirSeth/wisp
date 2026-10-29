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
    let radio = find(kind)
        .map_err(|error| error.message())?
        .ok_or_else(|| format!("This PC has no {name} adapter."))?;
    let state = if on { RadioState::On } else { RadioState::Off };
    match radio.SetStateAsync(state).and_then(|op| op.join()) {
        Ok(status) if status == RadioAccessStatus::Allowed => Ok(()),
        Ok(_) => Err(format!(
            "Windows did not allow Wisp to change {name}. Check Settings > Privacy & security > Radios."
        )),
        Err(error) => Err(error.message()),
    }
}
