use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};

use dwm_lut_payload::{HookPayload, MonitorIdentity};

use crate::DirtyRect;
use crate::lut_bypass::LutBypassRuntime;
use crate::lut_pipeline::{LutDecision, LutPipeline};
use crate::minhook::{MinHookRuntime, RegisteredHook};
use crate::profile::{HookProfile, HookTarget};
use crate::resolver::SignatureResolutionReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookRegistrationTarget {
    pub target: HookTarget,
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
                    address: target.address,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookRuntime {
    pub minhook: MinHookRuntime,
    pub lut_pipeline: Arc<LutPipeline>,
    pub hooks: Vec<RegisteredHook>,
    pub lut_bypass: LutBypassRuntime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookState {
    pub payload: HookPayload,
    pub profile: HookProfile,
    pub runtime: HookRuntime,
}

static STATE: OnceLock<Mutex<Option<HookState>>> = OnceLock::new();

static RETAINED_STATE: OnceLock<Mutex<Option<HookState>>> = OnceLock::new();

static PRESENT_RUNTIME_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
pub(crate) static HOOK_GLOBAL_TEST_LOCK: Mutex<()> = Mutex::new(());

static LIFECYCLE: AtomicU8 = AtomicU8::new(LIFECYCLE_IDLE);

const LIFECYCLE_IDLE: u8 = 0;
const LIFECYCLE_RUNNING: u8 = 1;
const LIFECYCLE_SHUTTING_DOWN: u8 = 2;
const LIFECYCLE_SHUT_DOWN: u8 = 3;
const LIFECYCLE_REPLACING_ASSIGNMENTS: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownStart {
    Started,
    NotInitialized,
    AlreadyInProgress,
    AlreadyShutDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplaceAssignmentsStart {
    Started,
    NotInitialized,
    AlreadyInProgress,
}

pub(crate) fn install_state(state: HookState) -> Result<(), Box<HookState>> {
    let slot = STATE.get_or_init(|| Mutex::new(None));
    let Ok(mut slot) = slot.lock() else {
        return Err(Box::new(state));
    };
    if slot.is_some() {
        return Err(Box::new(state));
    }
    *slot = Some(state);
    LIFECYCLE.store(LIFECYCLE_RUNNING, Ordering::Release);
    Ok(())
}

pub fn is_initialized() -> bool {
    is_runtime_active()
}

pub(crate) fn can_initialize() -> bool {
    matches!(
        LIFECYCLE.load(Ordering::Acquire),
        LIFECYCLE_IDLE | LIFECYCLE_SHUT_DOWN
    )
}

pub(crate) fn has_retained_state() -> bool {
    RETAINED_STATE
        .get()
        .and_then(|state| state.lock().ok().map(|guard| guard.is_some()))
        .unwrap_or(false)
}

pub fn hook_profile() -> Option<HookProfile> {
    with_state(|state| state.profile)
}

pub fn lut_bypass_runtime() -> Option<LutBypassRuntime> {
    with_state(|state| state.runtime.lut_bypass.clone())
}

pub(crate) fn is_runtime_active() -> bool {
    matches!(
        LIFECYCLE.load(Ordering::Acquire),
        LIFECYCLE_RUNNING | LIFECYCLE_REPLACING_ASSIGNMENTS
    )
}

pub(crate) fn update_present_context(context_address: usize, decision: LutDecision) {
    let _ = with_state_mut(|state| {
        state
            .runtime
            .lut_bypass
            .update_from_decision(context_address, decision);
    });
}

pub(crate) fn deactivate_present_context(context_address: usize) {
    update_present_context(context_address, LutDecision::NotApplicable);
}

pub(crate) fn begin_replace_assignments() -> ReplaceAssignmentsStart {
    match LIFECYCLE.compare_exchange(
        LIFECYCLE_RUNNING,
        LIFECYCLE_REPLACING_ASSIGNMENTS,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => ReplaceAssignmentsStart::Started,
        Err(LIFECYCLE_IDLE) | Err(LIFECYCLE_SHUT_DOWN) => ReplaceAssignmentsStart::NotInitialized,
        Err(LIFECYCLE_SHUTTING_DOWN) | Err(LIFECYCLE_REPLACING_ASSIGNMENTS) => {
            ReplaceAssignmentsStart::AlreadyInProgress
        }
        Err(_) => ReplaceAssignmentsStart::NotInitialized,
    }
}

pub(crate) fn finish_replace_assignments() {
    let _ = LIFECYCLE.compare_exchange(
        LIFECYCLE_REPLACING_ASSIGNMENTS,
        LIFECYCLE_RUNNING,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

fn present_runtime_lock() -> &'static Mutex<()> {
    PRESENT_RUNTIME_LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn lock_present_runtime() -> MutexGuard<'static, ()> {
    present_runtime_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn try_lock_present_runtime() -> Option<MutexGuard<'static, ()>> {
    match present_runtime_lock().try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

pub(crate) fn begin_shutdown() -> ShutdownStart {
    match LIFECYCLE.compare_exchange(
        LIFECYCLE_RUNNING,
        LIFECYCLE_SHUTTING_DOWN,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => ShutdownStart::Started,
        Err(LIFECYCLE_IDLE) => ShutdownStart::NotInitialized,
        Err(LIFECYCLE_SHUTTING_DOWN) | Err(LIFECYCLE_REPLACING_ASSIGNMENTS) => {
            ShutdownStart::AlreadyInProgress
        }
        Err(LIFECYCLE_SHUT_DOWN) => ShutdownStart::AlreadyShutDown,
        Err(_) => ShutdownStart::NotInitialized,
    }
}

pub(crate) fn mark_process_detaching() {
    let _ = LIFECYCLE.compare_exchange(
        LIFECYCLE_RUNNING,
        LIFECYCLE_SHUT_DOWN,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

pub(crate) fn clear_state_after_shutdown() {
    if let Some(state) = STATE.get()
        && let Ok(mut guard) = state.lock()
    {
        *guard = None;
    }
    if let Some(state) = RETAINED_STATE.get()
        && let Ok(mut guard) = state.lock()
    {
        *guard = None;
    }
    LIFECYCLE.store(LIFECYCLE_IDLE, Ordering::Release);
}

pub(crate) fn retain_state_after_shutdown() {
    let active = STATE.get_or_init(|| Mutex::new(None));
    let retained = RETAINED_STATE.get_or_init(|| Mutex::new(None));
    if let (Ok(mut active), Ok(mut retained)) = (active.lock(), retained.lock()) {
        *retained = active.take();
    }
}

pub(crate) fn finish_shutdown() {
    LIFECYCLE.store(LIFECYCLE_SHUT_DOWN, Ordering::Release);
}

pub(crate) fn finish_reactivation() {
    LIFECYCLE.store(LIFECYCLE_RUNNING, Ordering::Release);
}

pub(crate) fn reactivate_retained_state(
    payload: HookPayload,
    lut_pipeline: LutPipeline,
) -> Option<(MinHookRuntime, Vec<RegisteredHook>)> {
    let active = STATE.get_or_init(|| Mutex::new(None));
    let retained = RETAINED_STATE.get_or_init(|| Mutex::new(None));
    let (Ok(mut active), Ok(mut retained)) = (active.lock(), retained.lock()) else {
        return None;
    };
    if active.is_some() {
        return None;
    }
    let mut state = retained.take()?;
    update_payload_pipeline(&mut state, payload, lut_pipeline);
    let plan = (state.runtime.minhook, state.runtime.hooks.clone());
    *active = Some(state);
    Some(plan)
}

pub(crate) fn minhook_cleanup_plan() -> Option<(MinHookRuntime, Vec<RegisteredHook>)> {
    with_state(|state| (state.runtime.minhook, state.runtime.hooks.clone()))
}

pub(crate) fn render_present_lut(
    overlay_swap_chain: usize,
    monitor_identity: Option<MonitorIdentity>,
    hardware_protected: bool,
    dirty_rects: &[DirtyRect],
) -> Result<crate::d3d11_renderer::PresentLutOutcome, crate::d3d11_renderer::RenderAcquireError> {
    let Some((lut_pipeline, swap_chain_path)) = with_state(|state| {
        (
            state.runtime.lut_pipeline.clone(),
            state.profile.hypotheses.swap_chain,
        )
    }) else {
        return Err(crate::d3d11_renderer::RenderAcquireError::Unavailable);
    };

    unsafe {
        crate::d3d11_renderer::render_present_lut(
            overlay_swap_chain,
            swap_chain_path,
            monitor_identity,
            hardware_protected,
            dirty_rects,
            &lut_pipeline,
        )
    }
}

pub fn evaluate_direct_flip_compatible(
    context_address: usize,
    original_compatible: bool,
) -> Option<bool> {
    with_state_mut(|state| {
        state
            .runtime
            .lut_bypass
            .direct_flip_compatible(context_address, original_compatible)
    })
}

pub fn evaluate_ensure_independent_flip_state() -> Option<i32> {
    with_state(|state| state.runtime.lut_bypass.ensure_independent_flip_state()).flatten()
}

pub fn evaluate_direct_flip_support_compatible(original_compatible: bool) -> Option<bool> {
    with_state(|state| {
        state
            .runtime
            .lut_bypass
            .direct_flip_support_compatible(original_compatible)
    })
}

pub fn evaluate_overlay_test_mode(original_mode: i32) -> Option<i32> {
    with_state(|state| state.runtime.lut_bypass.overlay_test_mode(original_mode))
}

pub(crate) fn restore_overlay_test_mode() {
    let _ = with_state_mut(|state| state.runtime.lut_bypass.restore_overlay_test_mode());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplacePayloadPipelineError {
    NotInitialized,
}

pub fn replace_payload_pipeline(
    payload: HookPayload,
    lut_pipeline: LutPipeline,
) -> Result<(), ReplacePayloadPipelineError> {
    with_state_mut(|state| {
        update_payload_pipeline(state, payload, lut_pipeline);
    })
    .ok_or(ReplacePayloadPipelineError::NotInitialized)
}

fn update_payload_pipeline(state: &mut HookState, payload: HookPayload, lut_pipeline: LutPipeline) {
    let has_lut_assignments = !payload.assignments.is_empty();
    state.payload = payload;
    state.runtime.lut_pipeline = Arc::new(lut_pipeline);
    state
        .runtime
        .lut_bypass
        .reload_for_new_payload(has_lut_assignments);
}

fn with_state<R>(f: impl FnOnce(&HookState) -> R) -> Option<R> {
    let state = STATE.get()?;
    let guard = state.lock().ok()?;
    guard.as_ref().map(f)
}

fn with_state_mut<R>(f: impl FnOnce(&mut HookState) -> R) -> Option<R> {
    let state = STATE.get()?;
    let mut guard = state.lock().ok()?;
    guard.as_mut().map(f)
}

#[cfg(test)]
pub(crate) fn reset_state_for_tests() {
    if let Some(state) = STATE.get() {
        let mut guard = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = None;
    }
    if let Some(state) = RETAINED_STATE.get() {
        let mut guard = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = None;
    }
    LIFECYCLE.store(LIFECYCLE_IDLE, Ordering::Release);
    crate::bootstrap::reset_initialization_guard_for_tests();
    crate::d3d11_renderer::reset_fake_render_result();
    crate::minhook::reset_test_minhook_behavior(None, None, None, None);
    crate::minhook::reset_test_original_slots();
}
