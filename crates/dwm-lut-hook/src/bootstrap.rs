use std::ffi::OsString;
use std::fmt;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use dwm_lut_config::LutManifest;

use crate::minhook::MinHookRuntime;
use crate::profile::{BuildProfile, HookProfile};
use crate::resolver::{HookResolveError, SignatureResolutionReport, resolve_profile};
use crate::state::{
    HookConfig, HookRegistrationPlan, HookRegistrationState, HookRuntime, HookState,
    InitializationStage, LoggerState, ManifestLoadState, SignatureResolutionState, install_state,
    is_initialized,
};

#[derive(Debug)]
pub enum HookError {
    AlreadyInitialized,
    InvalidPath,
    Resolve(HookResolveError),
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => write!(f, "hook is already initialized"),
            Self::InvalidPath => write!(f, "manifest path must not be empty"),
            Self::Resolve(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HookError {}

impl From<HookResolveError> for HookError {
    fn from(value: HookResolveError) -> Self {
        Self::Resolve(value)
    }
}

#[repr(u32)]
enum InitializeStatus {
    Success = 0,
    NullManifestPath = 1,
    InvalidManifestPath = 2,
    AlreadyInitialized = 3,
    DwmcoreModuleNotLoaded = 4,
    DwmcoreImageInvalid = 5,
    PresentSignatureNotFound = 6,
    PresentSignatureAmbiguous = 7,
    DirectFlipSignatureNotFound = 8,
    DirectFlipSignatureAmbiguous = 9,
    OverlaysEnabledSignatureNotFound = 10,
    OverlaysEnabledSignatureAmbiguous = 11,
}

pub fn build_profile() -> BuildProfile {
    BuildProfile::Windows11_25H2
}

pub fn initialize(config: HookConfig, manifest: LutManifest) -> Result<(), HookError> {
    if is_initialized() {
        return Err(HookError::AlreadyInitialized);
    }

    if config.manifest_path.as_os_str().is_empty() {
        return Err(HookError::InvalidPath);
    }

    let state = prepare_initial_state(config, manifest)?;
    install_state(state).map_err(|_| HookError::AlreadyInitialized)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn initialize_with_resolution(
    config: HookConfig,
    manifest: LutManifest,
    profile: HookProfile,
    resolution: SignatureResolutionReport,
) -> Result<(), HookError> {
    let state = prepare_initial_state_with_resolution(config, manifest, profile, resolution)?;
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
        Err(HookError::Resolve(error)) => map_resolve_status(error) as u32,
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
    let resolution = resolve_profile(&profile)?;
    prepare_initial_state_with_resolution(config, manifest, profile, resolution)
}

pub(crate) fn prepare_initial_state_with_resolution(
    config: HookConfig,
    manifest: LutManifest,
    profile: HookProfile,
    resolution: SignatureResolutionReport,
) -> Result<HookState, HookError> {
    if config.manifest_path.as_os_str().is_empty() {
        return Err(HookError::InvalidPath);
    }

    let registration_plan = HookRegistrationPlan::from_resolution(&resolution);
    let initialization_trace = vec![
        InitializationStage::LoggerReady,
        InitializationStage::ManifestLoadDeferred,
        InitializationStage::MinHookBoundaryReady,
        InitializationStage::ProfileSelected,
        InitializationStage::TargetModuleResolved,
        InitializationStage::SignaturesResolved,
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
            resolution: SignatureResolutionState::Resolved(resolution),
            hook_registration: HookRegistrationState::Deferred(registration_plan),
            initialization_trace,
        },
    })
}

fn map_resolve_status(error: HookResolveError) -> InitializeStatus {
    match error {
        HookResolveError::ModuleNotLoaded { .. } => InitializeStatus::DwmcoreModuleNotLoaded,
        HookResolveError::InvalidModuleImage { .. } => InitializeStatus::DwmcoreImageInvalid,
        HookResolveError::SignatureNotFound { target, .. } => match target {
            crate::profile::HookTarget::Present => InitializeStatus::PresentSignatureNotFound,
            crate::profile::HookTarget::IsCandidateDirectFlipCompatible => {
                InitializeStatus::DirectFlipSignatureNotFound
            }
            crate::profile::HookTarget::OverlaysEnabled => {
                InitializeStatus::OverlaysEnabledSignatureNotFound
            }
        },
        HookResolveError::SignatureAmbiguous { target, .. } => match target {
            crate::profile::HookTarget::Present => InitializeStatus::PresentSignatureAmbiguous,
            crate::profile::HookTarget::IsCandidateDirectFlipCompatible => {
                InitializeStatus::DirectFlipSignatureAmbiguous
            }
            crate::profile::HookTarget::OverlaysEnabled => {
                InitializeStatus::OverlaysEnabledSignatureAmbiguous
            }
        },
    }
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
