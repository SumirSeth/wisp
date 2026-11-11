//! Wi-Fi: radio state, the connected network, nearby or saved networks, and
//! connecting to a saved network.
//!
//! Since Windows 11 24H2, nearby networks and the connected network's name
//! need location access. Without it, Wisp shows only the current network,
//! using the Network List Manager, and offers Location settings.

use windows::{
    Devices::Radios::RadioKind,
    Win32::{
        Foundation::{ERROR_ACCESS_DENIED, HANDLE},
        NetworkManagement::WiFi::*,
        Networking::NetworkListManager::{INetworkConnection, INetworkListManager, NetworkListManager},
        System::Com::{CLSCTX_ALL, CoCreateInstance},
    },
    core::{GUID, HSTRING, PCWSTR},
};

use super::radio;

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub on: bool,
    /// The connected network's name.
    pub network: Option<String>,
    /// Connected network quality, 0 to 100; absent if querying is denied or disconnected.
    pub signal: Option<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    pub name: String,
    /// The saved profile, used to connect.
    pub profile: Option<String>,
    pub connected: bool,
    pub secured: bool,
    /// Signal quality from 0 to 100. Saved networks listed without location
    /// access have no signal.
    pub signal: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Networks {
    pub list: Vec<Network>,
    /// Nearby networks are unavailable; `list` holds only the current network.
    pub location_off: bool,
}

/// `None` if the PC has no Wi-Fi adapter.
pub fn state() -> Option<State> {
    let on = radio::is_on(RadioKind::WiFi)?;
    let connection = || {
        let client = Client::open().ok()?;
        let interface = client.interface()?;
        Some(match client.current_connection(&interface) {
            Some((name, signal)) => (Some(name), Some(signal)),
            None => (nlm_name(&interface), None),
        })
    };
    let (network, signal) = if on {
        connection().unwrap_or_default()
    } else {
        (None, None)
    };
    Some(State { on, network, signal })
}

pub fn set_power(on: bool) -> Result<(), String> {
    radio::set(RadioKind::WiFi, "Wi-Fi", on)
}

/// Scans nearby networks, or shows only the current network without Location access.
pub fn networks() -> Result<Networks, String> {
    read_networks(true)
}

/// Refresh connection flags after an action without starting another RF scan.
pub fn cached_networks() -> Result<Networks, String> {
    read_networks(false)
}

fn read_networks(scan: bool) -> Result<Networks, String> {
    let client = Client::open()?;
    let interface = client.interface().ok_or("This PC has no Wi-Fi adapter.")?;
    let code = if scan { client.scan(&interface) } else { 0 };
    if code != 0 && code != ERROR_ACCESS_DENIED.0 {
        return Err(windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code)).message());
    }
    match client.available(&interface) {
        Ok(list) if code == 0 => Ok(Networks {
            list,
            location_off: false,
        }),
        Ok(_) | Err(5) => Ok(Networks {
            list: connected_name()
                .map(|name| Network {
                    profile: None,
                    name,
                    connected: true,
                    secured: true,
                    signal: None,
                })
                .into_iter()
                .collect(),
            location_off: true,
        }),
        Err(code) => Err(windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code)).message()),
    }
}

/// Connects to a saved network. Windows reports failures later, so Ok means
/// Windows accepted the request.
pub fn connect(profile: &str) -> Result<(), String> {
    let client = Client::open()?;
    let interface = client.interface().ok_or("This PC has no Wi-Fi adapter.")?;
    let profile = HSTRING::from(profile);
    let parameters = WLAN_CONNECTION_PARAMETERS {
        wlanConnectionMode: wlan_connection_mode_profile,
        strProfile: PCWSTR(profile.as_ptr()),
        pDot11Ssid: std::ptr::null_mut(),
        pDesiredBssidList: std::ptr::null_mut(),
        dot11BssType: dot11_BSS_type_any,
        dwFlags: 0,
    };
    let code = unsafe { WlanConnect(client.0, &interface, &parameters, None) };
    if code == 0 {
        Ok(())
    } else {
        Err(windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code)).message())
    }
}

/// The connected network's name. WLAN needs location access for it, so the
/// Network List Manager's name for the Wi-Fi connection is the fallback.
fn connected_name() -> Option<String> {
    let client = Client::open().ok()?;
    let interface = client.interface()?;
    client
        .current_connection(&interface)
        .map(|(name, _)| name)
        .or_else(|| nlm_name(&interface))
}

fn nlm_name(interface: &GUID) -> Option<String> {
    unsafe {
        let manager: INetworkListManager = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok()?;
        let connections = manager.GetNetworkConnections().ok()?;
        loop {
            let mut item: [Option<INetworkConnection>; 1] = [None];
            let mut fetched = 0;
            connections.Next(&mut item, Some(&mut fetched)).ok()?;
            let connection = item[0].take().filter(|_| fetched == 1)?;
            if connection.GetAdapterId().ok().as_ref() == Some(interface)
                && connection.IsConnected().is_ok_and(|v| v.as_bool())
            {
                return connection
                    .GetNetwork()
                    .and_then(|n| n.GetName())
                    .ok()
                    .map(|name| name.to_string());
            }
        }
    }
}

pub fn sorted(list: Vec<Network>) -> Vec<Network> {
    let mut unique: Vec<Network> = Vec::new();
    for network in list {
        if let Some(existing) = unique.iter_mut().find(|n| n.name == network.name) {
            existing.connected |= network.connected;
            existing.profile = existing.profile.take().or(network.profile);
            existing.signal = existing.signal.max(network.signal);
            existing.secured |= network.secured;
        } else {
            unique.push(network);
        }
    }
    let mut list = unique;
    list.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.signal.cmp(&a.signal))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    list
}

fn utf16(units: &[u16]) -> String {
    let len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..len])
}

fn ssid(ssid: &DOT11_SSID) -> String {
    let len = (ssid.uSSIDLength as usize).min(ssid.ucSSID.len());
    String::from_utf8_lossy(&ssid.ucSSID[..len]).into_owned()
}

struct ScanContext {
    interface: GUID,
    tx: std::sync::mpsc::SyncSender<bool>,
}
unsafe extern "system" fn scan_notification(data: *mut L2_NOTIFICATION_DATA, context: *mut std::ffi::c_void) {
    if data.is_null() || context.is_null() {
        return;
    }
    // WlanRegisterNotification owns access until unregister has drained callbacks.
    let (data, context) = unsafe { (&*data, &*context.cast::<ScanContext>()) };
    if data.InterfaceGuid == context.interface && data.NotificationSource == WLAN_NOTIFICATION_SOURCE_ACM {
        let code = data.NotificationCode;
        if code == wlan_notification_acm_scan_complete.0 as u32 || code == wlan_notification_acm_scan_fail.0 as u32 {
            let _ = context
                .tx
                .try_send(code == wlan_notification_acm_scan_complete.0 as u32);
        }
    }
}

/// A WLAN client handle, closed on drop.
struct Client(HANDLE);

impl Client {
    fn open() -> Result<Self, String> {
        let (mut version, mut handle) = (0, HANDLE::default());
        match unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) } {
            0 => Ok(Self(handle)),
            _ => Err("Cannot reach the WLAN service.".into()),
        }
    }

    fn interface(&self) -> Option<GUID> {
        unsafe {
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            if WlanEnumInterfaces(self.0, None, &mut list) != 0 || list.is_null() {
                return None;
            }
            let count = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), count);
            let guid = items.first().map(|info| info.InterfaceGuid);
            WlanFreeMemory(list.cast());
            guid
        }
    }

    fn scan(&self, interface: &GUID) -> u32 {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let context = Box::new(ScanContext {
            interface: *interface,
            tx,
        });
        let registered = unsafe {
            WlanRegisterNotification(
                self.0,
                WLAN_NOTIFICATION_SOURCE_ACM,
                true,
                Some(scan_notification),
                Some((&*context as *const ScanContext).cast()),
                None,
                None,
            )
        };
        if registered != 0 {
            return registered;
        }
        let mut code = unsafe { WlanScan(self.0, interface, None, None, None) };
        if code == 0 {
            match rx.recv_timeout(std::time::Duration::from_secs(4)) {
                Ok(true) => {}
                Ok(false) => code = 31, // ERROR_GEN_FAILURE
                Err(_) => code = 1460,  // ERROR_TIMEOUT
            }
        }
        // Unregister waits for any callback to finish before releasing its context.
        let unregistered =
            unsafe { WlanRegisterNotification(self.0, WLAN_NOTIFICATION_SOURCE_NONE, true, None, None, None, None) };
        if unregistered != 0 {
            // A failed unregister cannot prove the context is unused. Retain this
            // tiny allocation rather than let a late system callback dereference it.
            let _ = Box::leak(context);
        }
        code
    }
    fn current_connection(&self, interface: &GUID) -> Option<(String, u8)> {
        unsafe {
            let (mut size, mut data) = (0, std::ptr::null_mut());
            let code = WlanQueryInterface(
                self.0,
                interface,
                wlan_intf_opcode_current_connection,
                None,
                &mut size,
                &mut data,
                None,
            );
            if code != 0 || data.is_null() {
                return None;
            }
            let attributes = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
            let name = (attributes.isState == wlan_interface_state_connected).then(|| {
                (
                    ssid(&attributes.wlanAssociationAttributes.dot11Ssid),
                    attributes.wlanAssociationAttributes.wlanSignalQuality.min(100) as u8,
                )
            });
            WlanFreeMemory(data);
            name
        }
    }

    /// Nearby networks, one entry per name. Err holds the Win32 error code.
    fn available(&self, interface: &GUID) -> Result<Vec<Network>, u32> {
        unsafe {
            let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
            let code = WlanGetAvailableNetworkList(self.0, interface, 0, None, &mut list);
            if code != 0 {
                return Err(code);
            }
            if list.is_null() {
                return Ok(Vec::new());
            }
            let count = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).Network.as_ptr(), count);
            let mut networks: Vec<Network> = Vec::new();
            for item in items {
                let name = ssid(&item.dot11Ssid);
                if name.is_empty() {
                    continue;
                }
                let profile = utf16(&item.strProfileName);
                let network = Network {
                    name,
                    profile: (!profile.is_empty()).then_some(profile),
                    connected: item.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
                    secured: item.bSecurityEnabled.as_bool(),
                    signal: Some(item.wlanSignalQuality),
                };
                // Windows lists a network once per profile and once without one.
                match networks.iter_mut().find(|n| n.name == network.name) {
                    Some(existing) => {
                        existing.connected |= network.connected;
                        existing.profile = existing.profile.take().or(network.profile);
                        existing.signal = existing.signal.max(network.signal);
                    }
                    None => networks.push(network),
                }
            }
            WlanFreeMemory(list.cast());
            Ok(sorted(networks))
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        unsafe {
            WlanCloseHandle(self.0, None);
        }
    }
}

/// Disconnects the same adapter used by the list.
pub fn disconnect() -> Result<(), String> {
    let client = Client::open()?;
    let interface = client.interface().ok_or("This PC has no Wi-Fi adapter.")?;
    win32(unsafe { WlanDisconnect(client.0, &interface, None) })?;
    for _ in 0..16 {
        if connected_name().is_none() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err("Could not disconnect. Try Windows Wi-Fi settings.".into())
}

fn win32(code: u32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code)).message())
    }
}

/// Builds a per-user automatic profile. Enterprise authentication remains in Settings.
fn profile_xml(name: &str, password: Option<&str>, auth: &str) -> String {
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    };
    let hex: String = name.as_bytes().iter().map(|b| format!("{b:02X}")).collect();
    let key = password.map(|p| format!("<sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>", escape(p))).unwrap_or_default();
    let encryption = if password.is_some() { "AES" } else { "none" };
    format!(
        r#"<?xml version="1.0"?><WLANProfile xmlns="http://www.microsoft.com/networking/WLAN/profile/v1"><name>{}</name><SSIDConfig><SSID><hex>{hex}</hex></SSID></SSIDConfig><connectionType>ESS</connectionType><connectionMode>auto</connectionMode><MSM><security><authEncryption><authentication>{auth}</authentication><encryption>{encryption}</encryption><useOneX>false</useOneX></authEncryption>{key}</security></MSM></WLANProfile>"#,
        escape(name)
    )
}

pub fn join(network: &Network, password: Option<&str>) -> Result<(), String> {
    // A profile Wisp creates for this attempt, removed again if it doesn't connect.
    let mut created = None;
    if let Some(profile) = &network.profile {
        connect(profile)?;
    } else {
        let client = Client::open()?;
        let interface = client.interface().ok_or("This PC has no Wi-Fi adapter.")?;
        // Read the advertised authentication, never silently downgrade a network.
        let mut list = std::ptr::null_mut();
        win32(unsafe { WlanGetAvailableNetworkList(client.0, &interface, 0, None, &mut list) })?;
        if list.is_null() {
            return Err("No networks available.".into());
        }
        let auth = unsafe {
            let items = std::slice::from_raw_parts((*list).Network.as_ptr(), (*list).dwNumberOfItems as usize);
            let auth = items
                .iter()
                .find(|n| ssid(&n.dot11Ssid) == network.name)
                .map(|n| n.dot11DefaultAuthAlgorithm);
            WlanFreeMemory(list.cast());
            auth
        };
        let auth = match auth {
            Some(DOT11_AUTH_ALGO_80211_OPEN) if !network.secured => "open",
            Some(DOT11_AUTH_ALGO_RSNA_PSK) => "WPA2PSK",
            Some(DOT11_AUTH_ALGO_WPA3_SAE) => "WPA3SAE",
            _ => return Err("This network needs Windows Wi-Fi settings to connect.".into()),
        };
        if network.secured && password.is_none_or(str::is_empty) {
            return Err("Enter the network password.".into());
        }
        let xml = HSTRING::from(profile_xml(&network.name, password, auth));
        let mut reason = 0;
        win32(unsafe {
            WlanSetProfile(
                client.0,
                &interface,
                WLAN_PROFILE_USER,
                &xml,
                PCWSTR::null(),
                true,
                None,
                &mut reason,
            )
        })?;
        created = Some((client, interface));
        if let Err(error) = connect(&network.name) {
            forget(created, &network.name);
            return Err(error);
        }
    }
    // WlanConnect accepts a request; confirm association rather than claiming success.
    for _ in 0..24 {
        if connected_name().as_deref() == Some(&network.name) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    // A wrong password must not stay saved, or the next attempt would skip asking for it.
    forget(created, &network.name);
    Err("Could not connect. Check the password and try again.".into())
}

fn forget(created: Option<(Client, GUID)>, name: &str) {
    if let Some((client, interface)) = created {
        unsafe {
            WlanDeleteProfile(client.0, &interface, &HSTRING::from(name), None);
        }
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    #[test]
    fn escapes_secrets_and_encodes_ssids() {
        let xml = profile_xml("A&B", Some("<&password>"), "WPA2PSK");
        assert!(xml.contains("<name>A&amp;B</name>"));
        assert!(xml.contains("<hex>412642</hex>"));
        assert!(xml.contains("&lt;&amp;password&gt;"));
        assert!(xml.contains("<connectionMode>auto</connectionMode>"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(name: &str, saved: bool, connected: bool, signal: Option<u32>) -> Network {
        Network {
            name: name.into(),
            profile: saved.then(|| name.into()),
            connected,
            secured: true,
            signal,
        }
    }

    #[test]
    fn deduplicates_profiles_and_signal() {
        let list = sorted(vec![
            network("Home", false, true, Some(30)),
            network("Home", true, false, Some(90)),
        ]);
        assert_eq!(list.len(), 1);
        assert!(list[0].connected && list[0].profile.is_some());
        assert_eq!(list[0].signal, Some(90));
    }

    #[test]
    fn lists_connected_then_strongest() {
        let list = sorted(vec![
            network("cafe", false, false, Some(90)),
            network("office", true, false, Some(40)),
            network("home", true, true, Some(70)),
            network("library", true, false, Some(80)),
        ]);
        let names: Vec<_> = list.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["home", "cafe", "library", "office"]);
    }
}
