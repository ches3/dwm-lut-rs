use std::collections::HashSet;

#[cfg(not(test))]
use dwm_lut_payload::AdapterLuid;
use dwm_lut_payload::MonitorIdentity;
#[cfg(not(test))]
use windows_sys::Win32::Devices::Display::{
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
#[cfg(not(test))]
use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;

#[cfg(not(test))]
const DISPLAYCONFIG_PATH_ACTIVE: u32 = 0x0000_0001;

#[cfg(not(test))]
pub(crate) fn active_monitor_identities() -> Result<HashSet<MonitorIdentity>, u32> {
    for _ in 0..8 {
        let mut path_count = 0;
        let mut mode_count = 0;
        let result = unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
        };
        if result != 0 {
            return Err(result);
        }

        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count.max(1) as usize];
        let result = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
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
        return Ok(paths
            .into_iter()
            .filter(|path| {
                path.flags & DISPLAYCONFIG_PATH_ACTIVE != 0 && path.targetInfo.targetAvailable != 0
            })
            .map(|path| MonitorIdentity {
                adapter_luid: AdapterLuid {
                    high_part: path.targetInfo.adapterId.HighPart,
                    low_part: path.targetInfo.adapterId.LowPart,
                },
                target_id: path.targetInfo.id,
            })
            .collect());
    }

    Err(ERROR_INSUFFICIENT_BUFFER)
}

#[cfg(test)]
pub(crate) fn active_monitor_identities() -> Result<HashSet<MonitorIdentity>, u32> {
    Ok(HashSet::new())
}
