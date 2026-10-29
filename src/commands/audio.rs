//! Volume, mute and the default output device, through Core Audio.
//!
//! Windows has no public API to change the default output device. Sound
//! settings and most audio switchers use the undocumented IPolicyConfig
//! interface, so Wisp does too.

// IPolicyConfig keeps the Windows method names.
#![allow(non_snake_case)]

use windows::{
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::Audio::{Endpoints::IAudioEndpointVolume, *},
        System::Com::{
            CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ, StructuredStorage::PropVariantToStringAlloc,
        },
    },
    core::{GUID, HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, PCWSTR, interface},
};

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// 0 to 100.
    pub volume: u32,
    pub muted: bool,
    /// The default output device's name.
    pub device: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub id: String,
    pub name: String,
    pub default: bool,
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

fn default_device() -> windows::core::Result<IMMDevice> {
    unsafe { enumerator()?.GetDefaultAudioEndpoint(eRender, eConsole) }
}

fn endpoint_volume() -> windows::core::Result<IAudioEndpointVolume> {
    unsafe { default_device()?.Activate(CLSCTX_ALL, None) }
}

fn name(device: &IMMDevice) -> String {
    unsafe {
        let Ok(store) = device.OpenPropertyStore(STGM_READ) else {
            return String::new();
        };
        let Ok(value) = store.GetValue(&PKEY_Device_FriendlyName) else {
            return String::new();
        };
        match PropVariantToStringAlloc(&value) {
            Ok(text) => {
                let name = text.to_string().unwrap_or_default();
                CoTaskMemFree(Some(text.0 as _));
                name
            }
            Err(_) => String::new(),
        }
    }
}

/// `None` if there is no output device.
pub fn state() -> Option<State> {
    let device = default_device().ok()?;
    let volume: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None).ok()? };
    unsafe {
        Some(State {
            volume: (volume.GetMasterVolumeLevelScalar().ok()? * 100.0).round() as u32,
            muted: volume.GetMute().ok()?.as_bool(),
            device: name(&device),
        })
    }
}

/// `percent` is clamped to 0–100. Setting a volume also unmutes.
pub fn set_volume(percent: u32) -> Result<(), String> {
    let volume = endpoint_volume().map_err(|error| error.message())?;
    unsafe {
        volume
            .SetMasterVolumeLevelScalar(percent.min(100) as f32 / 100.0, std::ptr::null())
            .and_then(|()| volume.SetMute(false, std::ptr::null()))
            .map_err(|error| error.message())
    }
}

pub fn set_mute(muted: bool) -> Result<(), String> {
    let volume = endpoint_volume().map_err(|error| error.message())?;
    unsafe { volume.SetMute(muted, std::ptr::null()).map_err(|error| error.message()) }
}

/// Active output devices, the default first.
pub fn outputs() -> Result<Vec<Output>, String> {
    unsafe {
        let enumerator = enumerator().map_err(|error| error.message())?;
        let default_id = default_device().and_then(|d| d.GetId()).ok().map(|id| {
            let text = id.to_string().unwrap_or_default();
            CoTaskMemFree(Some(id.0 as _));
            text
        });
        let devices = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|error| error.message())?;
        let mut outputs = Vec::new();
        for index in 0..devices.GetCount().unwrap_or(0) {
            let Ok(device) = devices.Item(index) else { continue };
            let Ok(id) = device.GetId() else { continue };
            let id_text = id.to_string().unwrap_or_default();
            CoTaskMemFree(Some(id.0 as _));
            outputs.push(Output {
                default: default_id.as_deref() == Some(id_text.as_str()),
                id: id_text,
                name: name(&device),
            });
        }
        outputs.sort_by(|a, b| b.default.cmp(&a.default).then(a.name.cmp(&b.name)));
        Ok(outputs)
    }
}

/// Makes `id` the default output device for all roles.
pub fn set_output(id: &str) -> Result<(), String> {
    const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
    unsafe {
        let policy: IPolicyConfig =
            CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL).map_err(|error| error.message())?;
        let id = HSTRING::from(id);
        for role in [eConsole, eMultimedia, eCommunications] {
            policy
                .SetDefaultEndpoint(PCWSTR(id.as_ptr()), role)
                .ok()
                .map_err(|error| error.message())?;
        }
        Ok(())
    }
}

/// The undocumented interface behind Sound settings. Only `SetDefaultEndpoint`
/// is called; the earlier methods only reserve their vtable slots.
#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    fn GetMixFormat(&self) -> HRESULT;
    fn GetDeviceFormat(&self) -> HRESULT;
    fn ResetDeviceFormat(&self) -> HRESULT;
    fn SetDeviceFormat(&self) -> HRESULT;
    fn GetProcessingPeriod(&self) -> HRESULT;
    fn SetProcessingPeriod(&self) -> HRESULT;
    fn GetShareMode(&self) -> HRESULT;
    fn SetShareMode(&self) -> HRESULT;
    fn GetPropertyValue(&self) -> HRESULT;
    fn SetPropertyValue(&self) -> HRESULT;
    fn SetDefaultEndpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
    fn SetEndpointVisibility(&self) -> HRESULT;
}
