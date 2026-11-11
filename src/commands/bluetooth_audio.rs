//! Bluetooth audio controls are issued to the KS adapter behind an endpoint.
//! Match by container ID, never by a possibly duplicated friendly name.
use super::bluetooth::Device;
use windows::{
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_ContainerId,
        Media::{Audio::*, KernelStreaming::*},
        System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ, StructuredStorage::PropVariantToGUID},
    },
    core::{HSTRING, Interface, Result},
};

unsafe fn control(endpoint: &IMMDevice, enumerator: &IMMDeviceEnumerator) -> Result<IKsControl> {
    unsafe {
        let topology: IDeviceTopology = endpoint.Activate(CLSCTX_ALL, None)?;
        for index in 0..topology.GetConnectorCount()? {
            let connector = topology.GetConnector(index)?;
            let Ok(other) = connector.GetConnectedTo() else {
                continue;
            };
            let part: IPart = other.cast()?;
            let id = part.GetTopologyObject()?.GetDeviceId()?;
            let adapter = enumerator.GetDevice(id);
            CoTaskMemFree(Some(id.0.cast()));
            if let Ok(adapter) = adapter
                && let Ok(control) = adapter.Activate(CLSCTX_ALL, None)
            {
                return Ok(control);
            }
        }
        Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_NOINTERFACE,
        ))
    }
}

unsafe fn property(control: &IKsControl, id: KSPROPERTY_BTAUDIO, flags: u32, buffer: &mut [u8]) -> Result<()> {
    let property = KSIDENTIFIER {
        Anonymous: KSIDENTIFIER_0 {
            Anonymous: KSIDENTIFIER_0_0 {
                Set: KSPROPSETID_BtAudio,
                Id: id.0 as u32,
                Flags: flags,
            },
        },
    };
    let mut returned = 0;
    unsafe {
        control.KsProperty(
            &property,
            size_of::<KSIDENTIFIER>() as u32,
            if buffer.is_empty() {
                std::ptr::null_mut()
            } else {
                buffer.as_mut_ptr().cast()
            },
            buffer.len() as u32,
            &mut returned,
        )
    }
}

/// Read-only discovery, performed only for an open Bluetooth view.
pub fn identify(devices: &mut [Device]) {
    unsafe {
        let Ok(enumerator) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else {
            return;
        };
        let Ok(endpoints) =
            enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE(DEVICE_STATE_ACTIVE.0 | DEVICE_STATE_UNPLUGGED.0))
        else {
            return;
        };
        for index in 0..endpoints.GetCount().unwrap_or(0) {
            let Ok(endpoint) = endpoints.Item(index) else { continue };
            let container = endpoint
                .OpenPropertyStore(STGM_READ)
                .and_then(|s| s.GetValue(&PKEY_Device_ContainerId))
                .and_then(|v| PropVariantToGUID(&v));
            let Ok(container) = container else { continue };
            let Some(device) = devices.iter_mut().find(|d| d.container == Some(container)) else {
                continue;
            };
            let Ok(ks) = control(&endpoint, &enumerator) else {
                continue;
            };
            if property(
                &ks,
                KSPROPERTY_ONESHOT_RECONNECT,
                KSPROPERTY_TYPE_BASICSUPPORT,
                &mut [0; 64],
            )
            .is_err()
            {
                continue;
            }
            if (device.audio_id.is_none() || endpoint.GetState().is_ok_and(|s| s == DEVICE_STATE_ACTIVE))
                && let Ok(id) = endpoint.GetId()
            {
                device.connected = endpoint.GetState().is_ok_and(|s| s == DEVICE_STATE_ACTIVE);
                device.audio_id = id.to_string().ok();
                CoTaskMemFree(Some(id.0.cast()));
            }
        }
    }
}

pub fn set(id: &str, on: bool) -> std::result::Result<(), String> {
    unsafe {
        let run = || -> Result<()> {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let endpoint = enumerator.GetDevice(&HSTRING::from(id))?;
            let ks = control(&endpoint, &enumerator)?;
            property(
                &ks,
                if on {
                    KSPROPERTY_ONESHOT_RECONNECT
                } else {
                    KSPROPERTY_ONESHOT_DISCONNECT
                },
                KSPROPERTY_TYPE_GET,
                &mut [],
            )?;
            // Acceptance is not connection. Confirm the endpoint state with a bounded wait.
            for _ in 0..20 {
                if endpoint.GetState().is_ok_and(|s| (s == DEVICE_STATE_ACTIVE) == on) {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))
        };
        run().map_err(|_| "Could not change this audio connection. Use Bluetooth settings.".into())
    }
}
