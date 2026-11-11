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
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub battery: Option<u32>,
    pub container: Option<windows::core::GUID>,
    pub audio_id: Option<String>,
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
    if let Ok(found) =
        classic.and_then(|s| DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&s, &properties())?.join())
    {
        for info in found {
            let Ok(id) = info.Id() else { continue };
            let connected = BluetoothDevice::FromIdAsync(&id)
                .and_then(|op| op.join())
                .and_then(|d| d.ConnectionStatus())
                .is_ok_and(|s| s == BluetoothConnectionStatus::Connected);
            devices.push(Device {
                id: id.to_string(),
                battery: battery(&info),
                container: container(&info),
                audio_id: None,
                name: info.Name().map(|n| n.to_string()).unwrap_or_default(),
                connected,
            });
        }
    }
    let low_energy = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true);
    if let Ok(found) = low_energy
        .and_then(|s| DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&s, &properties())?.join())
    {
        for info in found {
            let Ok(id) = info.Id() else { continue };
            let connected = BluetoothLEDevice::FromIdAsync(&id)
                .and_then(|op| op.join())
                .and_then(|d| d.ConnectionStatus())
                .is_ok_and(|s| s == BluetoothConnectionStatus::Connected);
            let name = info.Name().map(|n| n.to_string()).unwrap_or_default();
            // Windows names some LE halves of dual-mode devices "LE-<name>".
            let name = name.strip_prefix("LE-").map(str::to_owned).unwrap_or(name);
            devices.push(Device {
                id: id.to_string(),
                name,
                connected,
                battery: battery(&info),
                container: container(&info),
                audio_id: None,
            });
        }
    }
    model(devices)
}

/// Merge dual-mode halves by physical identity, preserving distinct same-named devices.
pub fn model(devices: Vec<Device>) -> Vec<Device> {
    let mut merged: Vec<Device> = Vec::new();
    for device in devices.into_iter().filter(|d| !d.name.is_empty()) {
        if let Some(existing) = merged
            .iter_mut()
            .find(|d| d.id == device.id || (device.container.is_some() && d.container == device.container))
        {
            existing.connected |= device.connected;
            existing.battery = existing.battery.or(device.battery);
            existing.audio_id = existing.audio_id.take().or(device.audio_id);
        } else {
            merged.push(device);
        }
    }
    let mut devices = merged;
    devices.retain(|d| !d.name.is_empty());
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    devices
}

fn battery(info: &DeviceInformation) -> Option<u32> {
    use windows::{Foundation::IPropertyValue, core::Interface};
    let properties = info.Properties().ok()?;
    ["System.Devices.BatteryLifePercent", "System.Devices.BatteryLife"]
        .into_iter()
        .find_map(|key| {
            let value = properties
                .Lookup(&windows::core::HSTRING::from(key))
                .ok()?
                .cast::<IPropertyValue>()
                .ok()?;
            value
                .GetUInt32()
                .ok()
                .or_else(|| value.GetUInt8().ok().map(u32::from))
                .filter(|v| *v <= 100)
        })
}

fn properties() -> windows_collections::IIterable<windows::core::HSTRING> {
    [
        "System.Devices.BatteryLifePercent",
        "System.Devices.BatteryLife",
        "System.Devices.ContainerId",
    ]
    .into_iter()
    .map(windows::core::HSTRING::from)
    .collect::<Vec<_>>()
    .into()
}
fn container(info: &DeviceInformation) -> Option<windows::core::GUID> {
    use windows::{Foundation::IPropertyValue, core::Interface};
    info.Properties()
        .ok()?
        .Lookup(&windows::core::HSTRING::from("System.Devices.ContainerId"))
        .ok()?
        .cast::<IPropertyValue>()
        .ok()?
        .GetGuid()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(id: &str, container: u128, connected: bool) -> Device {
        Device {
            id: id.into(),
            name: "Headphones".into(),
            connected,
            battery: None,
            container: Some(windows::core::GUID::from_u128(container)),
            audio_id: None,
        }
    }
    #[test]
    fn merges_dual_mode_but_keeps_same_named_hardware() {
        let mut le = device("le", 1, true);
        le.battery = Some(72);
        let devices = model(vec![device("classic", 1, false), device("other", 2, false), le]);
        assert_eq!(devices.len(), 2);
        assert!(devices[0].connected);
        assert_eq!(devices[0].battery, Some(72));
    }
    #[test]
    fn missing_container_never_merges_by_name() {
        let mut a = device("a", 1, false);
        a.container = None;
        let mut b = device("b", 1, false);
        b.container = None;
        assert_eq!(model(vec![a, b]).len(), 2);
    }
}
