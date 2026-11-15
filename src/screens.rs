//! The connected screens, named the way Windows Settings names them.
//!
//! Shared with the settings app, which lists them for the dock's screen.

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MONITORINFOEXW,
    MonitorFromPoint,
};
use windows::core::BOOL;

/// A value of `dock_screen`: where the dock shows.
pub const POINTER: &str = "pointer";
pub const MAIN: &str = "main";

pub struct Screen {
    /// The monitor's device path. It stays the same across restarts, and
    /// while the monitor stays on the same port.
    pub id: String,
    /// The monitor's model, such as "DELL U2720Q".
    pub name: String,
    /// In physical pixels.
    pub bounds: RECT,
}

/// The screens Windows shows, from left to right.
pub fn list() -> Vec<Screen> {
    let names = device_names();
    let mut monitors: Vec<HMONITOR> = Vec::new();
    unsafe extern "system" fn each(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        unsafe { (*(data.0 as *mut Vec<HMONITOR>)).push(monitor) };
        BOOL(1)
    }
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut monitors as *mut _ as isize));
    }
    let mut screens: Vec<Screen> = monitors
        .into_iter()
        .filter_map(|monitor| {
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            if !unsafe { GetMonitorInfoW(monitor, &mut info as *mut _ as *mut MONITORINFO) }.as_bool() {
                return None;
            }
            let device = from_wide(&info.szDevice);
            let target = names.iter().find(|(source, ..)| *source == device);
            Some(Screen {
                // Without a device path, such as over Remote Desktop, the
                // display's own name is the best there is.
                id: target.map_or_else(|| device.clone(), |(_, path, _)| path.clone()),
                name: target.map(|(.., name)| name.clone()).unwrap_or_default(),
                bounds: info.monitorInfo.rcMonitor,
            })
        })
        .collect();
    screens.sort_by_key(|screen| (screen.bounds.left, screen.bounds.top));
    let names: Vec<String> = screens.iter().map(|screen| screen.name.clone()).collect();
    for (screen, name) in screens.iter_mut().zip(label(&names)) {
        screen.name = name;
    }
    screens
}

/// The screen with this `id`, if it is connected.
pub fn find(id: &str) -> Option<Screen> {
    list().into_iter().find(|screen| screen.id == id)
}

/// The main screen's bounds.
pub fn main() -> Option<RECT> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY), &mut info) }
        .as_bool()
        .then_some(info.rcMonitor)
}

/// Names each screen: its model, numbered when two share one, or "Screen N"
/// when Windows doesn't know the model.
fn label(models: &[String]) -> Vec<String> {
    models
        .iter()
        .enumerate()
        .map(|(index, model)| {
            if model.is_empty() {
                return format!("Screen {}", index + 1);
            }
            let same: Vec<usize> = (0..models.len()).filter(|&i| models[i] == *model).collect();
            if same.len() == 1 {
                model.clone()
            } else {
                let n = same.iter().position(|&i| i == index).unwrap_or(0) + 1;
                format!("{model} {n}")
            }
        })
        .collect()
}

/// Each active display's GDI name, such as `\\.\DISPLAY1`, with its
/// monitor's device path and model.
fn device_names() -> Vec<(String, String, String)> {
    unsafe {
        let (mut path_count, mut mode_count) = (0, 0);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        ) != ERROR_SUCCESS
        {
            return Vec::new();
        }
        paths.truncate(path_count as usize);
        paths
            .iter()
            .filter_map(|path| {
                let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                        size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                        adapterId: path.sourceInfo.adapterId,
                        id: path.sourceInfo.id,
                    },
                    ..Default::default()
                };
                let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                        size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                        adapterId: path.targetInfo.adapterId,
                        id: path.targetInfo.id,
                    },
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut source.header) != 0
                    || DisplayConfigGetDeviceInfo(&mut target.header) != 0
                {
                    return None;
                }
                Some((
                    from_wide(&source.viewGdiDeviceName),
                    from_wide(&target.monitorDevicePath),
                    from_wide(&target.monitorFriendlyDeviceName),
                ))
            })
            .collect()
    }
}

fn from_wide(text: &[u16]) -> String {
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..end])
}

#[cfg(test)]
mod tests {
    use super::label;

    #[test]
    fn screens_are_named_by_model_and_numbered_only_when_models_repeat() {
        let models = ["DELL U2720Q", "", "LG 27GL850", "DELL U2720Q"].map(String::from);
        assert_eq!(
            label(&models),
            ["DELL U2720Q 1", "Screen 2", "LG 27GL850", "DELL U2720Q 2"]
        );
    }
}
