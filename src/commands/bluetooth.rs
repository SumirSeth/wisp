//! Bluetooth: radio state and paired devices.

use windows::Devices::{
    Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice},
    Enumeration::DeviceInformation,
    Radios::RadioKind,
};

use super::radio;

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub on: bool,
    /// Names of connected devices.
    pub connected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub name: String,
    pub connected: bool,
}

/// `None` if the PC has no Bluetooth adapter.
pub fn state() -> Option<State> {
    let on = radio::is_on(RadioKind::Bluetooth)?;
    let connected = if on {
        devices().into_iter().filter(|d| d.connected).map(|d| d.name).collect()
    } else {
        Vec::new()
    };
    Some(State { on, connected })
}

pub fn set_power(on: bool) -> Result<(), String> {
    radio::set(RadioKind::Bluetooth, "Bluetooth", on)
}

/// Paired devices, classic and Low Energy, connected ones first.
pub fn devices() -> Vec<Device> {
    let mut devices = Vec::new();
    let classic = BluetoothDevice::GetDeviceSelectorFromPairingState(true);
    if let Ok(found) = classic.and_then(|s| DeviceInformation::FindAllAsyncAqsFilter(&s)?.join()) {
        for info in found {
            let Ok(id) = info.Id() else { continue };
            let connected = BluetoothDevice::FromIdAsync(&id)
                .and_then(|op| op.join())
                .and_then(|d| d.ConnectionStatus())
                .is_ok_and(|s| s == BluetoothConnectionStatus::Connected);
            devices.push(Device {
                name: info.Name().map(|n| n.to_string()).unwrap_or_default(),
                connected,
            });
        }
    }
    let low_energy = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true);
    if let Ok(found) = low_energy.and_then(|s| DeviceInformation::FindAllAsyncAqsFilter(&s)?.join()) {
        for info in found {
            let Ok(id) = info.Id() else { continue };
            let connected = BluetoothLEDevice::FromIdAsync(&id)
                .and_then(|op| op.join())
                .and_then(|d| d.ConnectionStatus())
                .is_ok_and(|s| s == BluetoothConnectionStatus::Connected);
            let name = info.Name().map(|n| n.to_string()).unwrap_or_default();
            // Windows names some LE halves of dual-mode devices "LE-<name>".
            let name = name.strip_prefix("LE-").map(str::to_owned).unwrap_or(name);
            match devices.iter_mut().find(|d: &&mut Device| d.name == name) {
                Some(existing) => existing.connected |= connected,
                None => devices.push(Device { name, connected }),
            }
        }
    }
    devices.retain(|d| !d.name.is_empty());
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    devices
}
