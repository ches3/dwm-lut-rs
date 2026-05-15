#[cfg(test)]
use std::cell::RefCell;
use std::path::PathBuf;
#[cfg(not(test))]
use std::sync::{Mutex, OnceLock};

use dwm_lut_config::LutManifest;

use crate::lut_bypass::{LutBypassRuntime, PresentHookOutcome};
use crate::lut_pipeline::{LutPipeline, LutPipelineSummary};
use crate::minhook::{MinHookRuntime, RegisteredHook};
use crate::profile::{BuildProfile, HookProfile, HookTarget};
use crate::resolver::SignatureResolutionReport;
use crate::{ClipBox, DirtyRect};

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
    ProfileSelected,
    TargetModuleResolved,
    SignaturesResolved,
    ManifestLoaded,
    LutPipelinePrepared,
    HookRegistrationEnabled,
    LutBypassStatePrepared,
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
                .filter(|target| target.target.is_function_hook_target())
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
pub struct HookRegistrationState {
    pub plan: HookRegistrationPlan,
    pub hooks: Vec<RegisteredHook>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LutPipelineState {
    Ready(LutPipeline),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LutBypassState {
    Ready(LutBypassRuntime),
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookRuntime {
    pub logger: LoggerState,
    pub manifest_load: ManifestLoadState,
    pub minhook: MinHookRuntime,
    pub resolution: SignatureResolutionState,
    pub lut_pipeline: LutPipelineState,
    pub hook_registration: HookRegistrationState,
    pub lut_bypass: LutBypassState,
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
    with_state(|state| state.config.manifest_path.clone())
}

pub fn hook_profile() -> Option<HookProfile> {
    with_state(|state| state.profile.clone())
}

pub fn signature_resolution() -> Option<SignatureResolutionReport> {
    with_state(|state| match &state.runtime.resolution {
        SignatureResolutionState::Resolved(report) => report.clone(),
    })
}

pub fn initialization_trace() -> Option<Vec<InitializationStage>> {
    with_state(|state| state.runtime.initialization_trace.clone())
}

pub fn lut_pipeline_summary() -> Option<LutPipelineSummary> {
    with_state(|state| match &state.runtime.lut_pipeline {
        LutPipelineState::Ready(runtime) => runtime.summary(),
    })
}

pub fn lut_bypass_runtime() -> Option<LutBypassRuntime> {
    with_state(|state| match &state.runtime.lut_bypass {
        LutBypassState::Ready(runtime) => runtime.clone(),
    })
}

pub fn evaluate_present_hook(
    context_address: usize,
    clip_box: ClipBox,
    dxgi_format: u32,
    dirty_rects: &[DirtyRect],
    lut_applied: bool,
) -> Option<PresentHookOutcome> {
    with_state_mut(|state| {
        let runtime = &mut state.runtime;
        let LutPipelineState::Ready(lut_pipeline) = &runtime.lut_pipeline;
        let LutBypassState::Ready(lut_bypass) = &mut runtime.lut_bypass;

        lut_bypass.update_present(
            lut_pipeline,
            context_address,
            clip_box,
            dxgi_format,
            dirty_rects,
            lut_applied,
        )
    })
}

pub fn evaluate_overlays_enabled(context_address: usize, original_enabled: bool) -> Option<bool> {
    with_state_mut(|state| match &mut state.runtime.lut_bypass {
        LutBypassState::Ready(runtime) => {
            runtime.overlays_enabled(context_address, original_enabled)
        }
    })
}

pub fn evaluate_direct_flip_compatible(
    context_address: usize,
    original_compatible: bool,
) -> Option<bool> {
    with_state_mut(|state| match &mut state.runtime.lut_bypass {
        LutBypassState::Ready(runtime) => {
            runtime.direct_flip_compatible(context_address, original_compatible)
        }
    })
}

pub fn evaluate_window_context_direct_flip_compatible(original_compatible: bool) -> Option<bool> {
    with_state(|state| match &state.runtime.lut_bypass {
        LutBypassState::Ready(runtime) => {
            runtime.window_context_direct_flip_compatible(original_compatible)
        }
    })
}

pub fn evaluate_comp_swap_chain_direct_flip_compatible(original_compatible: bool) -> Option<bool> {
    with_state(|state| match &state.runtime.lut_bypass {
        LutBypassState::Ready(runtime) => {
            runtime.comp_swap_chain_direct_flip_compatible(original_compatible)
        }
    })
}

pub fn evaluate_comp_swap_chain_independent_flip_compatible(
    original_compatible: bool,
) -> Option<bool> {
    with_state(|state| match &state.runtime.lut_bypass {
        LutBypassState::Ready(runtime) => {
            runtime.comp_swap_chain_independent_flip_compatible(original_compatible)
        }
    })
}

pub fn evaluate_comp_visual_candidate_for_promotion(original_candidate: bool) -> Option<bool> {
    with_state(|state| match &state.runtime.lut_bypass {
        LutBypassState::Ready(runtime) => {
            runtime.comp_visual_candidate_for_promotion(original_candidate)
        }
    })
}

pub fn evaluate_overlay_test_mode(original_mode: i32) -> Option<i32> {
    with_state(|state| match &state.runtime.lut_bypass {
        LutBypassState::Ready(runtime) => runtime.overlay_test_mode(original_mode),
    })
}

#[cfg(not(test))]
fn with_state<R>(f: impl FnOnce(&HookState) -> R) -> Option<R> {
    let state = STATE.get()?;
    let guard = state.lock().ok()?;
    Some(f(&guard))
}

#[cfg(test)]
fn with_state<R>(f: impl FnOnce(&HookState) -> R) -> Option<R> {
    STATE.with(|slot| slot.borrow().as_ref().map(f))
}

#[cfg(not(test))]
fn with_state_mut<R>(f: impl FnOnce(&mut HookState) -> R) -> Option<R> {
    let state = STATE.get()?;
    let mut guard = state.lock().ok()?;
    Some(f(&mut guard))
}

#[cfg(test)]
fn with_state_mut<R>(f: impl FnOnce(&mut HookState) -> R) -> Option<R> {
    STATE.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.as_mut().map(f)
    })
}

#[cfg(test)]
pub(crate) fn reset_state_for_tests() {
    STATE.with(|slot| {
        *slot.borrow_mut() = None;
    });
    crate::bootstrap::reset_initialization_guard_for_tests();
    crate::minhook::reset_test_minhook_behavior(None, None, None, None);
}
