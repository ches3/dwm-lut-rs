#[cfg(test)]
use std::cell::RefCell;
#[cfg(not(test))]
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};

use dwm_lut_payload::{HookPayload, MonitorIdentity};

use crate::lut_bypass::{LutBypassRuntime, PresentHookOutcome};
use crate::lut_pipeline::LutPipeline;
use crate::minhook::{MinHookRuntime, RegisteredHook};
use crate::profile::{HookProfile, HookTarget};
use crate::resolver::SignatureResolutionReport;
use crate::{ClipBox, DirtyRect};

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

#[cfg(not(test))]
static STATE: OnceLock<Mutex<Option<HookState>>> = OnceLock::new();

static PRESENT_APPLY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(not(test))]
static LIFECYCLE: AtomicU8 = AtomicU8::new(LIFECYCLE_IDLE);

const LIFECYCLE_IDLE: u8 = 0;
const LIFECYCLE_RUNNING: u8 = 1;
const LIFECYCLE_SHUTTING_DOWN: u8 = 2;
const LIFECYCLE_SHUT_DOWN: u8 = 3;
const LIFECYCLE_APPLYING_PAYLOAD: u8 = 4;

#[cfg(test)]
thread_local! {
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
    static LIFECYCLE: RefCell<u8> = const { RefCell::new(LIFECYCLE_IDLE) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownStart {
    Started,
    NotInitialized,
    AlreadyInProgress,
    AlreadyShutDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyPayloadStart {
    Started,
    NotInitialized,
    AlreadyInProgress,
}

pub(crate) fn install_state(state: HookState) -> Result<(), Box<HookState>> {
    #[cfg(not(test))]
    {
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

    #[cfg(test)]
    {
        STATE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                Err(Box::new(state))
            } else {
                *slot = Some(state);
                LIFECYCLE.with(|lifecycle| *lifecycle.borrow_mut() = LIFECYCLE_RUNNING);
                Ok(())
            }
        })
    }
}

pub fn is_initialized() -> bool {
    #[cfg(not(test))]
    {
        STATE
            .get()
            .and_then(|state| state.lock().ok().map(|guard| guard.is_some()))
            .unwrap_or(false)
    }

    #[cfg(test)]
    {
        STATE.with(|slot| slot.borrow().is_some())
    }
}

pub fn hook_profile() -> Option<HookProfile> {
    with_state(|state| state.profile.clone())
}

pub fn lut_bypass_runtime() -> Option<LutBypassRuntime> {
    with_state(|state| state.runtime.lut_bypass.clone())
}

pub(crate) fn evaluate_present_hook(
    context_address: usize,
    monitor_identity: Option<MonitorIdentity>,
    clip_box: ClipBox,
    dxgi_format: u32,
    dirty_rects: &[DirtyRect],
    _lut_applied: bool,
) -> Option<PresentHookOutcome> {
    with_state_mut(|state| {
        let runtime = &mut state.runtime;
        runtime.lut_bypass.update_present(
            &runtime.lut_pipeline,
            context_address,
            monitor_identity,
            clip_box,
            dxgi_format,
            dirty_rects,
        )
    })
}

pub(crate) fn is_shutting_down() -> bool {
    #[cfg(not(test))]
    {
        matches!(
            LIFECYCLE.load(Ordering::Acquire),
            LIFECYCLE_SHUTTING_DOWN | LIFECYCLE_SHUT_DOWN
        )
    }

    #[cfg(test)]
    {
        LIFECYCLE.with(|lifecycle| {
            matches!(
                *lifecycle.borrow(),
                LIFECYCLE_SHUTTING_DOWN | LIFECYCLE_SHUT_DOWN
            )
        })
    }
}

pub(crate) fn begin_apply_payload() -> ApplyPayloadStart {
    #[cfg(not(test))]
    {
        match LIFECYCLE.compare_exchange(
            LIFECYCLE_RUNNING,
            LIFECYCLE_APPLYING_PAYLOAD,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => ApplyPayloadStart::Started,
            Err(LIFECYCLE_IDLE) | Err(LIFECYCLE_SHUT_DOWN) => ApplyPayloadStart::NotInitialized,
            Err(LIFECYCLE_SHUTTING_DOWN) | Err(LIFECYCLE_APPLYING_PAYLOAD) => {
                ApplyPayloadStart::AlreadyInProgress
            }
            Err(_) => ApplyPayloadStart::NotInitialized,
        }
    }

    #[cfg(test)]
    {
        LIFECYCLE.with(|lifecycle| {
            let mut lifecycle = lifecycle.borrow_mut();
            match *lifecycle {
                LIFECYCLE_RUNNING => {
                    *lifecycle = LIFECYCLE_APPLYING_PAYLOAD;
                    ApplyPayloadStart::Started
                }
                LIFECYCLE_IDLE | LIFECYCLE_SHUT_DOWN => ApplyPayloadStart::NotInitialized,
                LIFECYCLE_SHUTTING_DOWN | LIFECYCLE_APPLYING_PAYLOAD => {
                    ApplyPayloadStart::AlreadyInProgress
                }
                _ => ApplyPayloadStart::NotInitialized,
            }
        })
    }
}

pub(crate) fn finish_apply_payload() {
    #[cfg(not(test))]
    {
        let _ = LIFECYCLE.compare_exchange(
            LIFECYCLE_APPLYING_PAYLOAD,
            LIFECYCLE_RUNNING,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    #[cfg(test)]
    {
        LIFECYCLE.with(|lifecycle| {
            let mut lifecycle = lifecycle.borrow_mut();
            if *lifecycle == LIFECYCLE_APPLYING_PAYLOAD {
                *lifecycle = LIFECYCLE_RUNNING;
            }
        });
    }
}

fn present_apply_lock() -> &'static Mutex<()> {
    PRESENT_APPLY_LOCK.get_or_init(|| Mutex::new(()))
}

pub(crate) fn lock_present_apply() -> MutexGuard<'static, ()> {
    present_apply_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn try_lock_present_apply() -> Option<MutexGuard<'static, ()>> {
    match present_apply_lock().try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

pub(crate) fn begin_shutdown() -> ShutdownStart {
    #[cfg(not(test))]
    {
        match LIFECYCLE.compare_exchange(
            LIFECYCLE_RUNNING,
            LIFECYCLE_SHUTTING_DOWN,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => ShutdownStart::Started,
            Err(LIFECYCLE_IDLE) => ShutdownStart::NotInitialized,
            Err(LIFECYCLE_SHUTTING_DOWN) | Err(LIFECYCLE_APPLYING_PAYLOAD) => {
                ShutdownStart::AlreadyInProgress
            }
            Err(LIFECYCLE_SHUT_DOWN) => ShutdownStart::AlreadyShutDown,
            Err(_) => ShutdownStart::NotInitialized,
        }
    }

    #[cfg(test)]
    {
        LIFECYCLE.with(|lifecycle| {
            let mut lifecycle = lifecycle.borrow_mut();
            match *lifecycle {
                LIFECYCLE_RUNNING => {
                    *lifecycle = LIFECYCLE_SHUTTING_DOWN;
                    ShutdownStart::Started
                }
                LIFECYCLE_IDLE => ShutdownStart::NotInitialized,
                LIFECYCLE_SHUTTING_DOWN | LIFECYCLE_APPLYING_PAYLOAD => {
                    ShutdownStart::AlreadyInProgress
                }
                LIFECYCLE_SHUT_DOWN => ShutdownStart::AlreadyShutDown,
                _ => ShutdownStart::NotInitialized,
            }
        })
    }
}

pub(crate) fn mark_process_detaching() {
    #[cfg(not(test))]
    {
        let _ = LIFECYCLE.compare_exchange(
            LIFECYCLE_RUNNING,
            LIFECYCLE_SHUT_DOWN,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    #[cfg(test)]
    {
        LIFECYCLE.with(|lifecycle| {
            let mut lifecycle = lifecycle.borrow_mut();
            if *lifecycle == LIFECYCLE_RUNNING {
                *lifecycle = LIFECYCLE_SHUT_DOWN;
            }
        });
    }
}

pub(crate) fn clear_state_after_shutdown() {
    #[cfg(not(test))]
    {
        if let Some(state) = STATE.get()
            && let Ok(mut guard) = state.lock()
        {
            *guard = None;
        }
        LIFECYCLE.store(LIFECYCLE_IDLE, Ordering::Release);
    }

    #[cfg(test)]
    {
        STATE.with(|slot| {
            *slot.borrow_mut() = None;
        });
        LIFECYCLE.with(|lifecycle| *lifecycle.borrow_mut() = LIFECYCLE_IDLE);
    }
}

pub(crate) fn finish_failed_shutdown() {
    #[cfg(not(test))]
    {
        LIFECYCLE.store(LIFECYCLE_SHUT_DOWN, Ordering::Release);
    }

    #[cfg(test)]
    {
        LIFECYCLE.with(|lifecycle| *lifecycle.borrow_mut() = LIFECYCLE_SHUT_DOWN);
    }
}

pub(crate) fn minhook_cleanup_plan() -> Option<(MinHookRuntime, Vec<RegisteredHook>)> {
    with_state(|state| (state.runtime.minhook, state.runtime.hooks.clone()))
}

pub(crate) fn evaluate_rendered_present_hook(
    context_address: usize,
    clip_box: ClipBox,
    dxgi_format: u32,
    dirty_rects: &[DirtyRect],
    render_result: crate::d3d11_renderer::RenderPresentLutResult,
) -> Option<PresentHookOutcome> {
    with_state_mut(|state| {
        let runtime = &mut state.runtime;
        if render_result.lut_index.is_some() {
            runtime.lut_bypass.update_present_with_lut_index(
                &runtime.lut_pipeline,
                context_address,
                clip_box,
                dxgi_format,
                dirty_rects,
                render_result.lut_index,
            )
        } else {
            runtime.lut_bypass.update_present(
                &runtime.lut_pipeline,
                context_address,
                None,
                clip_box,
                dxgi_format,
                dirty_rects,
            )
        }
    })
}

pub(crate) fn render_present_lut(
    overlay_swap_chain: usize,
    monitor_identity: Option<MonitorIdentity>,
    clip_box: ClipBox,
    dirty_rects: &[DirtyRect],
) -> crate::d3d11_renderer::RenderPresentLutResult {
    let Some((lut_pipeline, swap_chain_path)) = with_state(|state| {
        (
            state.runtime.lut_pipeline.clone(),
            state.profile.hypotheses.swap_chain,
        )
    }) else {
        return crate::d3d11_renderer::RenderPresentLutResult::default();
    };

    unsafe {
        crate::d3d11_renderer::render_present_lut(
            overlay_swap_chain,
            swap_chain_path,
            monitor_identity,
            clip_box,
            dirty_rects,
            &lut_pipeline,
        )
    }
}

pub(crate) fn prepare_present_lut_context(
    context_address: usize,
    monitor_identity: Option<MonitorIdentity>,
    clip_box: ClipBox,
    dxgi_format: u32,
    dirty_rects: &[DirtyRect],
) -> Option<PresentHookOutcome> {
    evaluate_present_hook(
        context_address,
        monitor_identity,
        clip_box,
        dxgi_format,
        dirty_rects,
        false,
    )
}

pub fn evaluate_overlays_enabled(context_address: usize, original_enabled: bool) -> Option<bool> {
    with_state_mut(|state| {
        state
            .runtime
            .lut_bypass
            .overlays_enabled(context_address, original_enabled)
    })
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

pub fn evaluate_window_context_direct_flip_compatible(original_compatible: bool) -> Option<bool> {
    with_state(|state| {
        state
            .runtime
            .lut_bypass
            .window_context_direct_flip_compatible(original_compatible)
    })
}

pub fn evaluate_comp_swap_chain_direct_flip_compatible(original_compatible: bool) -> Option<bool> {
    with_state(|state| {
        state
            .runtime
            .lut_bypass
            .comp_swap_chain_direct_flip_compatible(original_compatible)
    })
}

pub fn evaluate_comp_swap_chain_independent_flip_compatible(
    original_compatible: bool,
) -> Option<bool> {
    with_state(|state| {
        state
            .runtime
            .lut_bypass
            .comp_swap_chain_independent_flip_compatible(original_compatible)
    })
}

pub fn evaluate_comp_visual_candidate_for_promotion(original_candidate: bool) -> Option<bool> {
    with_state(|state| {
        state
            .runtime
            .lut_bypass
            .comp_visual_candidate_for_promotion(original_candidate)
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
    let has_lut_assignments = !payload.assignments.is_empty();

    with_state_mut(|state| {
        state.payload = payload;
        state.runtime.lut_pipeline = Arc::new(lut_pipeline);
        state
            .runtime
            .lut_bypass
            .reload_for_new_payload(has_lut_assignments);
    })
    .ok_or(ReplacePayloadPipelineError::NotInitialized)
}

#[cfg(not(test))]
fn with_state<R>(f: impl FnOnce(&HookState) -> R) -> Option<R> {
    let state = STATE.get()?;
    let guard = state.lock().ok()?;
    guard.as_ref().map(f)
}

#[cfg(test)]
fn with_state<R>(f: impl FnOnce(&HookState) -> R) -> Option<R> {
    STATE.with(|slot| slot.borrow().as_ref().map(f))
}

#[cfg(not(test))]
fn with_state_mut<R>(f: impl FnOnce(&mut HookState) -> R) -> Option<R> {
    let state = STATE.get()?;
    let mut guard = state.lock().ok()?;
    guard.as_mut().map(f)
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
    LIFECYCLE.with(|lifecycle| *lifecycle.borrow_mut() = LIFECYCLE_IDLE);
    crate::bootstrap::reset_initialization_guard_for_tests();
    crate::d3d11_renderer::reset_test_render_present_lut_result();
    crate::desktop_redraw::reset_for_tests();
    crate::minhook::reset_test_minhook_behavior(None, None, None, None);
    crate::minhook::reset_test_original_slots();
}

#[cfg(test)]
mod tests {
    use super::{lock_present_apply, try_lock_present_apply};

    #[test]
    fn present_apply_lock_is_exclusive() {
        let _apply_guard = lock_present_apply();

        assert!(try_lock_present_apply().is_none());
    }
}
