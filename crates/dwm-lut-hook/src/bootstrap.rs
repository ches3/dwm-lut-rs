use std::ffi::OsString;
use std::fmt;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use dwm_lut_config::LutManifest;

use crate::minhook::MinHookRuntime;
use crate::profile::{BuildProfile, HookProfile};
use crate::state::{
    HookConfig, HookRegistrationPlan, HookRegistrationState, HookRuntime, HookState,
    InitializationStage, LoggerState, ManifestLoadState, install_state,
};

#[derive(Debug)]
pub enum HookError {
    AlreadyInitialized,
    InvalidPath,
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => write!(f, "hook is already initialized"),
            Self::InvalidPath => write!(f, "manifest path must not be empty"),
        }
    }
}

impl std::error::Error for HookError {}

#[repr(u32)]
enum InitializeStatus {
    Success = 0,
    NullManifestPath = 1,
    InvalidManifestPath = 2,
    AlreadyInitialized = 3,
}

pub fn build_profile() -> BuildProfile {
    BuildProfile::Windows11_25H2
}

pub fn initialize(config: HookConfig, manifest: LutManifest) -> Result<(), HookError> {
    let state = prepare_initial_state(config, manifest)?;
    install_state(state).map_err(|_| HookError::AlreadyInitialized)
}

pub(crate) fn ffi_initialize(manifest_path: *const u16) -> u32 {
    let manifest_path = match unsafe { wide_path_from_ptr(manifest_path) } {
        Some(path) => path,
        None => return InitializeStatus::NullManifestPath as u32,
    };

    let config = HookConfig {
        manifest_path,
        profile: build_profile(),
    };

    match initialize(config, LutManifest::empty()) {
        Ok(()) => InitializeStatus::Success as u32,
        Err(HookError::InvalidPath) => InitializeStatus::InvalidManifestPath as u32,
        Err(HookError::AlreadyInitialized) => InitializeStatus::AlreadyInitialized as u32,
    }
}

pub(crate) fn prepare_initial_state(
    config: HookConfig,
    manifest: LutManifest,
) -> Result<HookState, HookError> {
    if config.manifest_path.as_os_str().is_empty() {
        return Err(HookError::InvalidPath);
    }

    let profile = HookProfile::for_build(config.profile);
    let registration_plan = HookRegistrationPlan::from_profile(&profile);

    let initialization_trace = vec![
        InitializationStage::LoggerReady,
        InitializationStage::ManifestLoadDeferred,
        InitializationStage::MinHookBoundaryReady,
        InitializationStage::ProfileSelected,
        InitializationStage::HookRegistrationDeferred,
        InitializationStage::GlobalStateCommitted,
    ];

    Ok(HookState {
        manifest,
        config: config.clone(),
        profile,
        runtime: HookRuntime {
            logger: LoggerState::Ready,
            manifest_load: ManifestLoadState::Deferred {
                manifest_path: config.manifest_path,
            },
            minhook: MinHookRuntime::boundary_defined(),
            hook_registration: HookRegistrationState::Deferred(registration_plan),
            initialization_trace,
        },
    })
}

unsafe fn wide_path_from_ptr(ptr: *const u16) -> Option<PathBuf> {
    if ptr.is_null() {
        return None;
    }

    let mut len = 0usize;
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }

    if len == 0 {
        return Some(PathBuf::new());
    }

    let units = unsafe { std::slice::from_raw_parts(ptr, len) };
    Some(PathBuf::from(OsString::from_wide(units)))
}
