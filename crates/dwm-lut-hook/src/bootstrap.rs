use std::ffi::OsString;
use std::fmt;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use dwm_lut_config::{ConfigError, LutManifest, load_manifest};

use crate::LutBypassRuntime;
use crate::lut_pipeline::{LutPipeline, LutPipelineError};
use crate::minhook::MinHookRuntime;
use crate::profile::{BuildProfile, HookProfile};
use crate::resolver::{HookResolveError, SignatureResolutionReport, resolve_profile};
use crate::state::{
    HookConfig, HookRegistrationPlan, HookRegistrationState, HookRuntime, HookState,
    InitializationStage, LoggerState, LutBypassState, LutPipelineState, ManifestLoadState,
    SignatureResolutionState, install_state, is_initialized,
};

#[derive(Debug)]
pub enum HookError {
    AlreadyInitialized,
    InvalidPath,
    Manifest(ConfigError),
    LutPipeline(LutPipelineError),
    Resolve(HookResolveError),
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => write!(f, "hook is already initialized"),
            Self::InvalidPath => write!(f, "manifest path must not be empty"),
            Self::Manifest(error) => write!(f, "{error}"),
            Self::LutPipeline(error) => write!(f, "{error}"),
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

impl From<ConfigError> for HookError {
    fn from(value: ConfigError) -> Self {
        Self::Manifest(value)
    }
}

impl From<LutPipelineError> for HookError {
    fn from(value: LutPipelineError) -> Self {
        Self::LutPipeline(value)
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
    ManifestLoadFailed = 12,
    ManifestHasNoAssignments = 13,
    LutPipelinePrepareFailed = 14,
    WindowDirectFlipSignatureNotFound = 15,
    WindowDirectFlipSignatureAmbiguous = 16,
    CompSwapChainDirectFlipSignatureNotFound = 17,
    CompSwapChainDirectFlipSignatureAmbiguous = 18,
    CompVisualPromotionSignatureNotFound = 19,
    CompVisualPromotionSignatureAmbiguous = 20,
    OverlayTestModeNotFound = 21,
    OverlayTestModeAmbiguous = 22,
    CompSwapChainIndependentFlipSignatureNotFound = 23,
    CompSwapChainIndependentFlipSignatureAmbiguous = 24,
}

pub fn build_profile() -> BuildProfile {
    BuildProfile::Windows11_25H2
}

#[cfg(test)]
pub(crate) fn initialize_with_resolution(
    config: HookConfig,
    resolution: SignatureResolutionReport,
) -> Result<(), HookError> {
    let state = prepare_initial_state_with_resolution(config, resolution)?;
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

    match initialize_from_manifest_path(config) {
        Ok(()) => InitializeStatus::Success as u32,
        Err(error) => map_hook_error(error) as u32,
    }
}

fn initialize_from_manifest_path(config: HookConfig) -> Result<(), HookError> {
    if is_initialized() {
        return Err(HookError::AlreadyInitialized);
    }

    if config.manifest_path.as_os_str().is_empty() {
        return Err(HookError::InvalidPath);
    }

    let state = prepare_initial_state_from_manifest_path(config)?;
    install_state(state).map_err(|_| HookError::AlreadyInitialized)
}

fn prepare_initial_state_from_manifest_path(config: HookConfig) -> Result<HookState, HookError> {
    prepare_initial_state_from_manifest_path_with_profile_resolver(config, resolve_profile)
}

fn prepare_initial_state_from_manifest_path_with_profile_resolver<F>(
    config: HookConfig,
    resolver: F,
) -> Result<HookState, HookError>
where
    F: FnOnce(&HookProfile) -> Result<SignatureResolutionReport, HookResolveError>,
{
    if config.manifest_path.as_os_str().is_empty() {
        return Err(HookError::InvalidPath);
    }

    let manifest = load_manifest(&config.manifest_path).map_err(HookError::Manifest)?;
    let lut_pipeline = LutPipeline::load(&manifest)?;
    let profile = HookProfile::for_build(config.profile);
    let resolution = resolver(&profile)?;
    Ok(finalize_initial_state(
        config,
        manifest,
        profile,
        resolution,
        lut_pipeline,
    ))
}

#[cfg(test)]
pub(crate) fn prepare_initial_state_with_resolution(
    config: HookConfig,
    resolution: SignatureResolutionReport,
) -> Result<HookState, HookError> {
    prepare_initial_state_from_manifest_path_with_profile_resolver(config, |_| Ok(resolution))
}

fn finalize_initial_state(
    config: HookConfig,
    manifest: LutManifest,
    profile: HookProfile,
    resolution: SignatureResolutionReport,
    lut_pipeline: LutPipeline,
) -> HookState {
    let assignment_count = manifest.assignments.len();
    let registration_plan = HookRegistrationPlan::from_resolution(&resolution);
    let overlay_test_mode_address = resolution
        .targets
        .iter()
        .find(|target| target.target == crate::profile::HookTarget::OverlayTestMode)
        .map(|target| target.address);
    let lut_bypass = LutBypassRuntime::new(
        lut_pipeline.summary().lut_count > 0,
        overlay_test_mode_address,
    );
    let initialization_trace = vec![
        InitializationStage::LoggerReady,
        InitializationStage::MinHookBoundaryReady,
        InitializationStage::ManifestLoaded,
        InitializationStage::LutPipelinePrepared,
        InitializationStage::ProfileSelected,
        InitializationStage::TargetModuleResolved,
        InitializationStage::SignaturesResolved,
        InitializationStage::HookRegistrationDeferred,
        InitializationStage::LutBypassStatePrepared,
        InitializationStage::GlobalStateCommitted,
    ];

    HookState {
        manifest,
        config: config.clone(),
        profile,
        runtime: HookRuntime {
            logger: LoggerState::Ready,
            manifest_load: ManifestLoadState::Loaded {
                manifest_path: config.manifest_path,
                assignment_count,
            },
            minhook: MinHookRuntime::boundary_defined(),
            resolution: SignatureResolutionState::Resolved(resolution),
            lut_pipeline: LutPipelineState::Ready(lut_pipeline),
            hook_registration: HookRegistrationState::Deferred(registration_plan),
            lut_bypass: LutBypassState::Ready(lut_bypass),
            initialization_trace,
        },
    }
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
            crate::profile::HookTarget::WindowContextIsCandidateDirectFlipCompatible => {
                InitializeStatus::WindowDirectFlipSignatureNotFound
            }
            crate::profile::HookTarget::CompSwapChainIsCandidateDirectFlipCompatible => {
                InitializeStatus::CompSwapChainDirectFlipSignatureNotFound
            }
            crate::profile::HookTarget::CompVisualIsCandidateForPromotion => {
                InitializeStatus::CompVisualPromotionSignatureNotFound
            }
            crate::profile::HookTarget::CompSwapChainIsCandidateIndependentFlipCompatible => {
                InitializeStatus::CompSwapChainIndependentFlipSignatureNotFound
            }
            crate::profile::HookTarget::OverlayTestMode => {
                InitializeStatus::OverlayTestModeNotFound
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
            crate::profile::HookTarget::WindowContextIsCandidateDirectFlipCompatible => {
                InitializeStatus::WindowDirectFlipSignatureAmbiguous
            }
            crate::profile::HookTarget::CompSwapChainIsCandidateDirectFlipCompatible => {
                InitializeStatus::CompSwapChainDirectFlipSignatureAmbiguous
            }
            crate::profile::HookTarget::CompVisualIsCandidateForPromotion => {
                InitializeStatus::CompVisualPromotionSignatureAmbiguous
            }
            crate::profile::HookTarget::CompSwapChainIsCandidateIndependentFlipCompatible => {
                InitializeStatus::CompSwapChainIndependentFlipSignatureAmbiguous
            }
            crate::profile::HookTarget::OverlayTestMode => {
                InitializeStatus::OverlayTestModeAmbiguous
            }
        },
    }
}

fn map_hook_error(error: HookError) -> InitializeStatus {
    match error {
        HookError::InvalidPath => InitializeStatus::InvalidManifestPath,
        HookError::AlreadyInitialized => InitializeStatus::AlreadyInitialized,
        HookError::Resolve(error) => map_resolve_status(error),
        HookError::Manifest(_) => InitializeStatus::ManifestLoadFailed,
        HookError::LutPipeline(LutPipelineError::NoAssignments) => {
            InitializeStatus::ManifestHasNoAssignments
        }
        HookError::LutPipeline(_) => InitializeStatus::LutPipelinePrepareFailed,
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use dwm_lut_config::ConfigError;

    use crate::profile::HookTarget;
    use crate::resolver::{HookResolveError, LoadedModule, SignatureResolutionReport};

    use super::{
        BuildProfile, HookConfig, HookError, InitializeStatus, map_hook_error,
        prepare_initial_state_from_manifest_path_with_profile_resolver,
    };

    fn write_test_manifest(contents: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dwm-lut-bootstrap-{unique}.json"));
        fs::write(&path, contents).expect("manifest file should be written");
        path
    }

    #[test]
    fn manifest_failures_take_precedence_over_signature_resolution() {
        let error = prepare_initial_state_from_manifest_path_with_profile_resolver(
            HookConfig {
                manifest_path: PathBuf::from(r"C:\missing\manifest.json"),
                profile: BuildProfile::Windows11_25H2,
            },
            |_| {
                Err(HookResolveError::SignatureNotFound {
                    target: HookTarget::Present,
                    capture_key: "present",
                })
            },
        )
        .expect_err("manifest loading should fail before resolution");

        assert!(matches!(error, HookError::Manifest(ConfigError::Io(_))));
    }

    #[test]
    fn empty_manifest_file_reports_no_assignments() {
        let manifest_path = write_test_manifest(r#"{ "assignments": [] }"#);
        let error = prepare_initial_state_from_manifest_path_with_profile_resolver(
            HookConfig {
                manifest_path: manifest_path.clone(),
                profile: BuildProfile::Windows11_25H2,
            },
            |_| {
                Ok(SignatureResolutionReport {
                    module: LoadedModule {
                        module_name: "dwmcore.dll",
                        base_address: 0x1800_0000,
                        size: 0x20_0000,
                    },
                    targets: Vec::new(),
                })
            },
        )
        .expect_err("empty manifest file should fail in LUT pipeline");

        assert!(matches!(
            error,
            HookError::LutPipeline(crate::lut_pipeline::LutPipelineError::NoAssignments)
        ));
        let _ = fs::remove_file(manifest_path);
    }

    #[test]
    fn manifest_validation_runs_before_signature_resolution() {
        let manifest_path = write_test_manifest(r#"{ "assignments": [] }"#);
        let error = prepare_initial_state_from_manifest_path_with_profile_resolver(
            HookConfig {
                manifest_path: manifest_path.clone(),
                profile: BuildProfile::Windows11_25H2,
            },
            |_| {
                Err(HookResolveError::SignatureNotFound {
                    target: HookTarget::Present,
                    capture_key: "present",
                })
            },
        )
        .expect_err("manifest validation should fail before resolution");

        assert!(matches!(
            error,
            HookError::LutPipeline(crate::lut_pipeline::LutPipelineError::NoAssignments)
        ));
        let _ = fs::remove_file(manifest_path);
    }

    #[test]
    fn no_assignments_maps_to_manifest_has_no_assignments_status() {
        assert_eq!(
            map_hook_error(HookError::LutPipeline(
                crate::lut_pipeline::LutPipelineError::NoAssignments,
            )) as u32,
            InitializeStatus::ManifestHasNoAssignments as u32
        );
    }
}
