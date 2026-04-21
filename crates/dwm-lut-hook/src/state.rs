#[cfg(test)]
use std::cell::RefCell;
use std::path::PathBuf;
#[cfg(not(test))]
use std::sync::{Mutex, OnceLock};

use dwm_lut_config::LutManifest;

use crate::lut_pipeline::{LutPipeline, LutPipelineSummary};
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
    Loaded {
        manifest_path: PathBuf,
        assignment_count: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitializationStage {
    LoggerReady,
    MinHookBoundaryReady,
    ProfileSelected,
    TargetModuleResolved,
    SignaturesResolved,
    ManifestLoaded,
    LutPipelinePrepared,
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

#[derive(Debug, Clone, PartialEq)]
pub enum LutPipelineState {
    Ready(LutPipeline),
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookRuntime {
    pub logger: LoggerState,
    pub manifest_load: ManifestLoadState,
    pub minhook: MinHookRuntime,
    pub resolution: SignatureResolutionState,
    pub lut_pipeline: LutPipelineState,
    pub hook_registration: HookRegistrationState,
    pub initialization_trace: Vec<InitializationStage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookState {
    pub manifest: LutManifest,
    pub config: HookConfig,
    pub profile: HookProfile,
    pub runtime: HookRuntime,
}

#[cfg(not(test))]
static STATE: OnceLock<Mutex<HookState>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

pub(crate) fn install_state(state: HookState) -> Result<(), Box<HookState>> {
    #[cfg(not(test))]
    {
        STATE
            .set(Mutex::new(state))
            .map_err(|mutex| match mutex.into_inner() {
                Ok(state) => Box::new(state),
                Err(poisoned) => Box::new(poisoned.into_inner()),
            })
    }

    #[cfg(test)]
    {
        STATE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                Err(Box::new(state))
            } else {
                *slot = Some(state);
                Ok(())
            }
        })
    }
}

pub fn is_initialized() -> bool {
    #[cfg(not(test))]
    {
        STATE.get().is_some()
    }

    #[cfg(test)]
    {
        STATE.with(|slot| slot.borrow().is_some())
    }
}

pub fn manifest_path() -> Option<PathBuf> {
    #[cfg(not(test))]
    {
        let state = STATE.get()?;
        let guard = state.lock().ok()?;
        Some(guard.config.manifest_path.clone())
    }

    #[cfg(test)]
    {
        STATE.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|state| state.config.manifest_path.clone())
        })
    }
}

pub fn hook_profile() -> Option<HookProfile> {
    #[cfg(not(test))]
    {
        let state = STATE.get()?;
        let guard = state.lock().ok()?;
        Some(guard.profile.clone())
    }

    #[cfg(test)]
    {
        STATE.with(|slot| slot.borrow().as_ref().map(|state| state.profile.clone()))
    }
}

pub fn signature_resolution() -> Option<SignatureResolutionReport> {
    #[cfg(not(test))]
    {
        let state = STATE.get()?;
        let guard = state.lock().ok()?;
        match &guard.runtime.resolution {
            SignatureResolutionState::Resolved(report) => Some(report.clone()),
        }
    }

    #[cfg(test)]
    {
        STATE.with(|slot| {
            let slot = slot.borrow();
            let state = slot.as_ref()?;
            match &state.runtime.resolution {
                SignatureResolutionState::Resolved(report) => Some(report.clone()),
            }
        })
    }
}

pub fn initialization_trace() -> Option<Vec<InitializationStage>> {
    #[cfg(not(test))]
    {
        let state = STATE.get()?;
        let guard = state.lock().ok()?;
        Some(guard.runtime.initialization_trace.clone())
    }

    #[cfg(test)]
    {
        STATE.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|state| state.runtime.initialization_trace.clone())
        })
    }
}

pub fn lut_pipeline_summary() -> Option<LutPipelineSummary> {
    #[cfg(not(test))]
    {
        let state = STATE.get()?;
        let guard = state.lock().ok()?;
        match &guard.runtime.lut_pipeline {
            LutPipelineState::Ready(runtime) => Some(runtime.summary()),
        }
    }

    #[cfg(test)]
    {
        STATE.with(|slot| {
            let slot = slot.borrow();
            let state = slot.as_ref()?;
            match &state.runtime.lut_pipeline {
                LutPipelineState::Ready(runtime) => Some(runtime.summary()),
            }
        })
    }
}
