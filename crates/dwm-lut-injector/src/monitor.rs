use std::ffi::OsString;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;

use dwm_lut_payload::{AdapterLuid, MonitorIdentity};
use windows_sys::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, LUID};
use windows_sys::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};

use crate::config::ConfigError;
use crate::error::InjectorError;

const DISPLAYCONFIG_PATH_ACTIVE: u32 = 0x0000_0001;

pub(crate) fn resolve_monitor_identity(
    monitor_device_path: &str,
) -> Result<MonitorIdentity, ConfigError> {
    for monitor in enumerate_active_monitors()? {
        if monitor
            .monitor_device_path
            .eq_ignore_ascii_case(monitor_device_path)
        {
            return Ok(monitor.identity);
        }
    }

    Err(ConfigError::parse_message(format!(
        "monitor_device_path not found: {monitor_device_path}"
    )))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MonitorListing {
    pub(crate) number: u32,
    pub(crate) friendly_name: String,
    pub(crate) edid_pnp_id: String,
    pub(crate) position: DesktopPosition,
    pub(crate) resolution: DesktopResolution,
    pub(crate) monitor_device_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DesktopPosition {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DesktopResolution {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

struct ActiveMonitor {
    monitor_device_path: String,
    identity: MonitorIdentity,
}

pub(crate) fn list_monitor_listings() -> Result<Vec<MonitorListing>, InjectorError> {
    let paths = query_active_paths().map_err(|error| {
        InjectorError::MonitorEnumeration(format!("failed to query active display paths: {error}"))
    })?;

    let mut listings = Vec::new();
    for path in paths {
        if path.flags & DISPLAYCONFIG_PATH_ACTIVE == 0 {
            continue;
        }

        let target = query_target_name(&path).map_err(|error| {
            InjectorError::MonitorEnumeration(format!(
                "failed to query target device name for target_id={}: {error}",
                path.targetInfo.id
            ))
        })?;
        let source = query_source_name(&path).map_err(|error| {
            InjectorError::MonitorEnumeration(format!(
                "failed to query source device name for source_id={}: {error}",
                path.sourceInfo.id
            ))
        })?;

        let gdi_device_name = wide_to_string(&source.viewGdiDeviceName);
        let number = display_number_from_gdi_name(&gdi_device_name).ok_or_else(|| {
            InjectorError::MonitorEnumeration(format!(
                "failed to parse display number from source name: {gdi_device_name}"
            ))
        })?;
        let bounds = query_desktop_bounds(&gdi_device_name).map_err(|error| {
            InjectorError::MonitorEnumeration(format!(
                "failed to query desktop bounds for {gdi_device_name}: {error}"
            ))
        })?;
        let monitor_device_path = wide_to_string(&target.monitorDevicePath);

        listings.push(MonitorListing {
            number,
            friendly_name: wide_to_string(&target.monitorFriendlyDeviceName),
            edid_pnp_id: extract_edid_pnp_id(&monitor_device_path)
                .unwrap_or_default()
                .to_string(),
            position: bounds.0,
            resolution: bounds.1,
            monitor_device_path,
        });
    }

    Ok(listings)
}

fn enumerate_active_monitors() -> Result<Vec<ActiveMonitor>, ConfigError> {
    let paths = query_active_paths().map_err(|error| {
        ConfigError::parse_message(format!("failed to query active display paths: {error}"))
    })?;

    let mut monitors = Vec::new();
    for path in paths {
        if path.flags & DISPLAYCONFIG_PATH_ACTIVE == 0 {
            continue;
        }

        let target = query_target_name(&path).map_err(|error| {
            ConfigError::parse_message(format!(
                "failed to query target device name for target_id={}: {error}",
                path.targetInfo.id
            ))
        })?;

        monitors.push(ActiveMonitor {
            monitor_device_path: wide_to_string(&target.monitorDevicePath),
            identity: active_monitor_identity(&path),
        });
    }

    Ok(monitors)
}

fn active_monitor_identity(path: &DISPLAYCONFIG_PATH_INFO) -> MonitorIdentity {
    MonitorIdentity {
        adapter_luid: luid_to_adapter_luid(path.targetInfo.adapterId),
        target_id: path.targetInfo.id,
    }
}

fn query_active_paths() -> Result<Vec<DISPLAYCONFIG_PATH_INFO>, u32> {
    let flags = QDC_ONLY_ACTIVE_PATHS;
    for _ in 0..8 {
        let mut path_count = 0;
        let mut mode_count = 0;
        let size_result =
            unsafe { GetDisplayConfigBufferSizes(flags, &mut path_count, &mut mode_count) };
        if size_result != 0 {
            return Err(size_result);
        }

        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count.max(1) as usize];
        let result = unsafe {
            QueryDisplayConfig(
                flags,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if result == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if result != 0 {
            return Err(result);
        }

        paths.truncate(path_count as usize);
        return Ok(paths);
    }

    Err(ERROR_INSUFFICIENT_BUFFER)
}

fn query_target_name(
    path: &DISPLAYCONFIG_PATH_INFO,
) -> Result<DISPLAYCONFIG_TARGET_DEVICE_NAME, u32> {
    let mut info = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        ..Default::default()
    };
    let result = unsafe { DisplayConfigGetDeviceInfo(&mut info.header) };
    if result != 0 {
        return Err(result as u32);
    }
    Ok(info)
}

fn query_source_name(
    path: &DISPLAYCONFIG_PATH_INFO,
) -> Result<DISPLAYCONFIG_SOURCE_DEVICE_NAME, u32> {
    let mut info = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        },
        ..Default::default()
    };
    let result = unsafe { DisplayConfigGetDeviceInfo(&mut info.header) };
    if result != 0 {
        return Err(result as u32);
    }
    Ok(info)
}

fn query_desktop_bounds(
    gdi_device_name: &str,
) -> Result<(DesktopPosition, DesktopResolution), std::io::Error> {
    let mut device_name: Vec<u16> = gdi_device_name.encode_utf16().collect();
    device_name.push(0);

    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let result =
        unsafe { EnumDisplaySettingsW(device_name.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode) };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }

    let position = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
    Ok((
        DesktopPosition {
            x: position.x,
            y: position.y,
        },
        DesktopResolution {
            width: mode.dmPelsWidth,
            height: mode.dmPelsHeight,
        },
    ))
}

fn display_number_from_gdi_name(gdi_device_name: &str) -> Option<u32> {
    gdi_device_name
        .strip_prefix(r"\\.\DISPLAY")
        .and_then(|number| number.parse().ok())
}

fn extract_edid_pnp_id(monitor_device_path: &str) -> Option<&str> {
    let mut parts = monitor_device_path.split('#');
    match (parts.next(), parts.next()) {
        (Some(prefix), Some(model)) if prefix.eq_ignore_ascii_case(r"\\?\DISPLAY") => Some(model),
        _ => None,
    }
}

fn wide_to_string(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    OsString::from_wide(&units[..end])
        .to_string_lossy()
        .into_owned()
}

fn luid_to_adapter_luid(luid: LUID) -> AdapterLuid {
    AdapterLuid {
        high_part: luid.HighPart,
        low_part: luid.LowPart,
    }
}
