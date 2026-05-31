use std::ffi::OsString;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;

use dwm_lut_payload::{AdapterLuid, MonitorIdentity};
use windows_sys::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS,
    QueryDisplayConfig,
};
use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;

use crate::config::ConfigError;

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

struct ActiveMonitor {
    monitor_device_path: String,
    identity: MonitorIdentity,
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
            identity: MonitorIdentity {
                adapter_luid: luid_to_adapter_luid(path.targetInfo.adapterId),
                target_id: path.targetInfo.id,
            },
        });
    }

    Ok(monitors)
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

fn wide_to_string(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    OsString::from_wide(&units[..end])
        .to_string_lossy()
        .into_owned()
}

fn luid_to_adapter_luid(luid: windows_sys::Win32::Foundation::LUID) -> AdapterLuid {
    AdapterLuid {
        high_part: luid.HighPart,
        low_part: luid.LowPart,
    }
}
