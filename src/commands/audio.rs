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
        Foundation::{HWND, LPARAM, WPARAM},
        Media::Audio::{
            Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl},
            *,
        },
        System::Com::{
            CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ, StructuredStorage::PropVariantToStringAlloc,
        },
        UI::WindowsAndMessaging::PostMessageW,
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

/// An app's audio, as in the Volume mixer.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub pid: u32,
    /// The program's path, for its icon. Empty if Windows does not tell.
    pub exe: String,
    /// The program's description, such as "Google Chrome", or its file name.
    pub name: String,
    /// 0 to 100.
    pub volume: u32,
    pub muted: bool,
}

impl Session {
    pub fn icon_key(&self) -> Option<String> {
        (!self.exe.is_empty()).then(|| format!("exe:{}", self.exe.to_lowercase()))
    }
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

/// Where to post a message when the volume, mute or default output changes.
static LEVEL_TARGET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static LEVEL_MESSAGE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Posts `msg` to `hwnd` whenever the level [`level`] reads may have changed:
/// volume keys, other apps, or a new default device. Call on the thread that
/// calls [`level`].
pub fn notify_level_changes(hwnd: HWND, msg: u32) {
    use std::sync::atomic::Ordering;
    LEVEL_MESSAGE.store(msg, Ordering::Release);
    LEVEL_TARGET.store(hwnd.0 as usize, Ordering::Release);
    // Registers the callbacks on the current endpoint.
    let _ = level();
}

fn post_level_change() {
    use std::sync::atomic::Ordering;
    let hwnd = LEVEL_TARGET.load(Ordering::Acquire);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(hwnd as _)),
                LEVEL_MESSAGE.load(Ordering::Acquire),
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

/// The volume (0 to 100) and mute, without the device name that `state` reads,
/// for frequent polling. `None` if there is no output device.
pub fn level() -> Option<(u32, bool)> {
    thread_local! {
        static READER: std::cell::RefCell<Option<LevelReader>> = const { std::cell::RefCell::new(None) };
    }
    READER.with(|reader| {
        let mut reader = reader.borrow_mut();
        if reader.is_none() {
            *reader = LevelReader::new();
        }
        if let Some(reader) = reader.as_mut() {
            reader.read()
        } else {
            // Notification registration can fail independently of level queries.
            let volume = endpoint_volume().ok()?;
            unsafe {
                Some((
                    (volume.GetMasterVolumeLevelScalar().ok()? * 100.0).round() as u32,
                    volume.GetMute().ok()?.as_bool(),
                ))
            }
        }
    })
}

// Keep apartment-bound interfaces on the caller's thread. Device notifications
// invalidate the endpoint; querying a level never recreates the COM enumerator.
struct LevelReader {
    enumerator: IMMDeviceEnumerator,
    callback: IMMNotificationClient,
    dirty: std::sync::Arc<std::sync::atomic::AtomicBool>,
    volume: Option<IAudioEndpointVolume>,
    /// Told about volume and mute changes on `volume`.
    changed: IAudioEndpointVolumeCallback,
}

impl LevelReader {
    fn new() -> Option<Self> {
        let enumerator = enumerator().ok()?;
        let dirty = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let callback: IMMNotificationClient = DeviceChanged(dirty.clone()).into();
        unsafe {
            enumerator.RegisterEndpointNotificationCallback(&callback).ok()?;
        }
        Some(Self {
            enumerator,
            callback,
            dirty,
            volume: None,
            changed: VolumeChanged.into(),
        })
    }

    fn read(&mut self) -> Option<(u32, bool)> {
        use std::sync::atomic::Ordering;
        unsafe {
            if self.dirty.swap(false, Ordering::AcqRel) {
                if let Some(old) = self.volume.take() {
                    let _ = old.UnregisterControlChangeNotify(&self.changed);
                }
                self.volume = self
                    .enumerator
                    .GetDefaultAudioEndpoint(eRender, eConsole)
                    .and_then(|device| device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None))
                    .ok();
                if let Some(volume) = &self.volume {
                    let _ = volume.RegisterControlChangeNotify(&self.changed);
                }
            }
            let Some(volume) = self.volume.as_ref() else {
                // Retry transient service/activation failures just as the uncached path did.
                self.dirty.store(true, Ordering::Release);
                return None;
            };
            match (volume.GetMasterVolumeLevelScalar(), volume.GetMute()) {
                (Ok(level), Ok(muted)) => Some(((level * 100.0).round() as u32, muted.as_bool())),
                _ => {
                    self.dirty.store(true, Ordering::Release);
                    None
                }
            }
        }
    }
}

impl Drop for LevelReader {
    fn drop(&mut self) {
        unsafe {
            let _ = self.enumerator.UnregisterEndpointNotificationCallback(&self.callback);
            if let Some(volume) = &self.volume {
                let _ = volume.UnregisterControlChangeNotify(&self.changed);
            }
        }
    }
}

#[windows::core::implement(IMMNotificationClient)]
struct DeviceChanged(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl DeviceChanged_Impl {
    fn changed(&self) -> windows::core::Result<()> {
        self.0.store(true, std::sync::atomic::Ordering::Release);
        post_level_change();
        Ok(())
    }
}

#[windows::core::implement(IAudioEndpointVolumeCallback)]
struct VolumeChanged;

impl IAudioEndpointVolumeCallback_Impl for VolumeChanged_Impl {
    fn OnNotify(&self, _: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        post_level_change();
        Ok(())
    }
}

impl IMMNotificationClient_Impl for DeviceChanged_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
        self.changed()
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.changed()
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
        self.changed()
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _: &PCWSTR) -> windows::core::Result<()> {
        if flow == eRender && role == eConsole {
            self.changed()?;
        }
        Ok(())
    }
    fn OnPropertyValueChanged(
        &self,
        _: &PCWSTR,
        _: &windows::Win32::Foundation::PROPERTYKEY,
    ) -> windows::core::Result<()> {
        Ok(())
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

/// Volumes of apps that use the default output device, one per process,
/// sorted by name. Skips system sounds and sessions that have ended.
pub fn sessions() -> Vec<Session> {
    let mut sessions: Vec<Session> = Vec::new();
    for (control, volume) in session_volumes() {
        unsafe {
            let Ok(pid) = control.GetProcessId() else { continue };
            if pid == 0 || sessions.iter().any(|s| s.pid == pid) {
                continue;
            }
            let (Ok(level), Ok(muted)) = (volume.GetMasterVolume(), volume.GetMute()) else {
                continue;
            };
            let exe = super::open_windows::process_path(pid);
            if HIDDEN
                .iter()
                .any(|hidden| super::open_windows::program_name(&exe).eq_ignore_ascii_case(hidden))
            {
                continue;
            }
            sessions.push(Session {
                pid,
                name: display_name(&exe),
                exe,
                volume: (level * 100.0).round() as u32,
                muted: muted.as_bool(),
            });
        }
    }
    sessions.sort_by_cached_key(|s| s.name.to_lowercase());
    sessions
}

/// Windows parts that open audio sessions but are not apps to adjust,
/// such as the touch keyboard and emoji panel.
const HIDDEN: [&str; 2] = ["TextInputHost", "ShellExperienceHost"];

/// The description in a program's version info, such as "Google Chrome",
/// or its file name. Cached, since the state refreshes every second.
fn display_name(exe: &str) -> String {
    use std::{collections::HashMap, sync::Mutex};
    static NAMES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    names
        .get_or_insert_with(HashMap::new)
        .entry(exe.to_string())
        .or_insert_with(|| {
            description(exe).unwrap_or_else(|| match super::open_windows::program_name(exe) {
                "" => "App".to_string(),
                name => name.to_string(),
            })
        })
        .clone()
}

fn description(exe: &str) -> Option<String> {
    use windows::Win32::{
        Foundation::PROPERTYKEY,
        UI::Shell::{IShellItem2, SHCreateItemFromParsingName},
    };
    // System.FileDescription, from the file's version info.
    const FILE_DESCRIPTION: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x0cef7d53_fa64_11d1_a203_0000f81fedee),
        pid: 3,
    };
    if exe.is_empty() {
        return None;
    }
    unsafe {
        let item: IShellItem2 = SHCreateItemFromParsingName(&HSTRING::from(exe), None).ok()?;
        let text = item.GetString(&FILE_DESCRIPTION).ok()?;
        let name = text.to_string().ok();
        CoTaskMemFree(Some(text.0 as _));
        name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty())
    }
}

/// The default output device's app sessions that have not ended, without system sounds.
fn session_volumes() -> Vec<(IAudioSessionControl2, ISimpleAudioVolume)> {
    use windows::core::Interface;
    let mut found = Vec::new();
    unsafe {
        let Ok(device) = default_device() else { return found };
        let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
            return found;
        };
        let Ok(list) = manager.GetSessionEnumerator() else {
            return found;
        };
        for index in 0..list.GetCount().unwrap_or(0) {
            let Ok(control) = list.GetSession(index) else { continue };
            if control.GetState().is_ok_and(|state| state == AudioSessionStateExpired) {
                continue;
            }
            let (Ok(control), Ok(volume)) = (
                control.cast::<IAudioSessionControl2>(),
                control.cast::<ISimpleAudioVolume>(),
            ) else {
                continue;
            };
            // S_OK means system sounds; S_FALSE means an app.
            if control.IsSystemSoundsSession() == windows::Win32::Foundation::S_OK {
                continue;
            }
            found.push((control, volume));
        }
    }
    found
}

/// Sets the volume of every session of process `pid`. Setting a volume also unmutes.
pub fn set_app_volume(pid: u32, percent: u32) -> Result<(), String> {
    let mut set = false;
    for (control, volume) in session_volumes() {
        unsafe {
            if control.GetProcessId().ok() != Some(pid) {
                continue;
            }
            volume
                .SetMasterVolume(percent.min(100) as f32 / 100.0, std::ptr::null())
                .and_then(|()| volume.SetMute(false, std::ptr::null()))
                .map_err(|error| error.message())?;
            set = true;
        }
    }
    if set {
        Ok(())
    } else {
        Err("The app stopped playing sound.".into())
    }
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
