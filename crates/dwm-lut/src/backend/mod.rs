mod injector;
pub(crate) mod monitor;
mod staging;
mod win32;

use std::path::PathBuf;

use crate::config;
use crate::error::{InjectionStep, InjectorError};

pub(crate) use injector::{ApplyOutcome, DisableOutcome};
pub(crate) use monitor::{
    DesktopPosition, DesktopResolution, MonitorListing, list_monitor_listings,
};

pub(crate) struct ApplyRequest {
    pub(crate) dll_path: Option<PathBuf>,
    pub(crate) config_path: PathBuf,
    pub(crate) profile: Option<String>,
}

pub(crate) struct ApplyReport {
    pub(crate) outcome: ApplyOutcome,
    pub(crate) pid: u32,
    pub(crate) input_dll_path: PathBuf,
    pub(crate) staged_dll_path: PathBuf,
    pub(crate) config_path: PathBuf,
    pub(crate) profile_name: String,
}

pub(crate) struct DisableReport {
    pub(crate) outcome: DisableOutcome,
    pub(crate) pid: u32,
}

pub(crate) fn apply(request: ApplyRequest) -> Result<ApplyReport, InjectorError> {
    let input_dll_path = request
        .dll_path
        .unwrap_or(staging::default_hook_dll_path()?);
    let input_dll_path = injector::canonicalize_existing_file(
        &input_dll_path,
        InjectionStep::ResolveLocalHookDll,
        "hook DLL",
    )?;
    let config_path = injector::canonicalize_existing_file(
        &request.config_path,
        InjectionStep::ResolveConfigPath,
        "config file",
    )?;
    let loaded = config::load_payload(&config_path, request.profile.as_deref())?;
    let payload_bytes = dwm_lut_payload::serialize_payload(&loaded.payload)?;
    let staged_dll_path = staging::stage_hook_dll(&input_dll_path)?;
    let pid = win32::find_process_id_by_name("dwm.exe")?;

    win32::enable_debug_privilege()?;
    let outcome = injector::apply_config(pid, &staged_dll_path, &payload_bytes)?;

    Ok(ApplyReport {
        outcome,
        pid,
        input_dll_path,
        staged_dll_path,
        config_path,
        profile_name: loaded.profile_name,
    })
}

pub(crate) fn disable() -> Result<DisableReport, InjectorError> {
    let pid = win32::find_process_id_by_name("dwm.exe")?;

    win32::enable_debug_privilege()?;
    let outcome = injector::disable_injected_hook(pid)?;

    Ok(DisableReport { outcome, pid })
}
