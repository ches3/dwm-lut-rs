use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use dwm_lut_config::LutManifest;

use crate::minhook::MinHookRuntime;
use crate::profile::{BuildProfile, HookProfile, HookTarget};
use crate::resolver::SignatureResolutionReport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookConfig {
    pub manifest_path: PathBuf,
    pub profile: BuildProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoggerState {
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestLoadState {
    Deferred { manifest_path: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitializationStage {
    LoggerReady,
    ManifestLoadDeferred,
    MinHookBoundaryReady,
    ProfileSelected,
    TargetModuleResolved,
    SignaturesResolved,
    HookRegistrationDeferred,
    GlobalStateCommitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookRegistrationTarget {
    pub target: HookTarget,
    pub capture_key: &'static str,
    pub address: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRegistrationPlan {
    pub module_name: &'static str,
    pub module_base_address: usize,
    pub module_size: usize,
    pub targets: Vec<HookRegistrationTarget>,
}

impl HookRegistrationPlan {
    pub fn from_resolution(resolution: &SignatureResolutionReport) -> Self {
        Self {
            module_name: resolution.module.module_name,
            module_base_address: resolution.module.base_address,
            module_size: resolution.module.size,
            targets: resolution
                .targets
                .iter()
                .map(|target| HookRegistrationTarget {
                    target: target.target,
                    capture_key: target.capture_key,
                    address: target.address,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureResolutionState {
    Resolved(SignatureResolutionReport),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookRegistrationState {
    Deferred(HookRegistrationPlan),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRuntime {
    pub logger: LoggerState,
    pub manifest_load: ManifestLoadState,
    pub minhook: MinHookRuntime,
    pub resolution: SignatureResolutionState,
    pub hook_registration: HookRegistrationState,
    pub initialization_trace: Vec<InitializationStage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookState {
    pub manifest: LutManifest,
    pub config: HookConfig,
    pub profile: HookProfile,
    pub runtime: HookRuntime,
}

static STATE: OnceLock<Mutex<HookState>> = OnceLock::new();

pub(crate) fn install_state(state: HookState) -> Result<(), Box<HookState>> {
    STATE
        .set(Mutex::new(state))
        .map_err(|mutex| match mutex.into_inner() {
            Ok(state) => Box::new(state),
            Err(poisoned) => Box::new(poisoned.into_inner()),
        })
}

pub fn is_initialized() -> bool {
    STATE.get().is_some()
}

pub fn manifest_path() -> Option<PathBuf> {
    let state = STATE.get()?;
    let guard = state.lock().ok()?;
    Some(guard.config.manifest_path.clone())
}

pub fn hook_profile() -> Option<HookProfile> {
    let state = STATE.get()?;
    let guard = state.lock().ok()?;
    Some(guard.profile.clone())
}

pub fn signature_resolution() -> Option<SignatureResolutionReport> {
    let state = STATE.get()?;
    let guard = state.lock().ok()?;
    match &guard.runtime.resolution {
        SignatureResolutionState::Resolved(report) => Some(report.clone()),
    }
}

pub fn initialization_trace() -> Option<Vec<InitializationStage>> {
    let state = STATE.get()?;
    let guard = state.lock().ok()?;
    Some(guard.runtime.initialization_trace.clone())
}
