//! Wi-Fi: radio state, the connected network, nearby or saved networks, and
//! connecting to a saved network.
//!
//! Since Windows 11 24H2, nearby networks and the connected network's name
//! need location access. Without it, Wisp lists saved networks and reads the
//! connected network's name from the Network List Manager.

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
    /// Windows denied the list of nearby networks, so `list` holds saved networks.
    pub location_off: bool,
}

/// `None` if the PC has no Wi-Fi adapter.
pub fn state() -> Option<State> {
    let on = radio::is_on(RadioKind::WiFi)?;
    Some(State {
        on,
        network: if on { connected_name() } else { None },
    })
}

pub fn set_power(on: bool) -> Result<(), String> {
    radio::set(RadioKind::WiFi, "Wi-Fi", on)
}

/// Lists nearby networks, or saved networks if location access is off.
pub fn networks() -> Result<Networks, String> {
    let client = Client::open()?;
    let interface = client.interface().ok_or("This PC has no Wi-Fi adapter.")?;
    let connected = connected_name();
    match client.available(&interface) {
        Ok(list) => Ok(Networks {
            list,
            location_off: false,
        }),
        Err(code) if code == ERROR_ACCESS_DENIED.0 => {
            let list = client
                .profiles(&interface)
                .into_iter()
                .map(|name| Network {
                    connected: connected.as_deref() == Some(name.as_str()),
                    profile: Some(name.clone()),
                    name,
                    secured: true,
                    signal: None,
                })
                .collect();
            Ok(Networks {
                list: sorted(list),
                location_off: true,
            })
        }
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
    client.current_ssid(&interface).or_else(|| nlm_name(&interface))
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
            if connection.GetAdapterId().ok().as_ref() == Some(interface) {
                return connection
                    .GetNetwork()
                    .and_then(|n| n.GetName())
                    .ok()
                    .map(|name| name.to_string());
            }
        }
    }
}

fn sorted(mut list: Vec<Network>) -> Vec<Network> {
    list.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.profile.is_some().cmp(&a.profile.is_some()))
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

    fn current_ssid(&self, interface: &GUID) -> Option<String> {
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
            let name = (attributes.isState == wlan_interface_state_connected)
                .then(|| ssid(&attributes.wlanAssociationAttributes.dot11Ssid));
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

    fn profiles(&self, interface: &GUID) -> Vec<String> {
        unsafe {
            let mut list: *mut WLAN_PROFILE_INFO_LIST = std::ptr::null_mut();
            if WlanGetProfileList(self.0, interface, None, &mut list) != 0 || list.is_null() {
                return Vec::new();
            }
            let count = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).ProfileInfo.as_ptr(), count);
            let names = items.iter().map(|info| utf16(&info.strProfileName)).collect();
            WlanFreeMemory(list.cast());
            names
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
    fn lists_connected_then_saved_then_strongest() {
        let list = sorted(vec![
            network("cafe", false, false, Some(90)),
            network("office", true, false, Some(40)),
            network("home", true, true, Some(70)),
            network("library", true, false, Some(80)),
        ]);
        let names: Vec<_> = list.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["home", "library", "office", "cafe"]);
    }
}
