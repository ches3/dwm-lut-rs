use super::{PresentLutOutcome, RenderAcquireError};
use crate::present::DirtyRect;
use dwm_lut_payload::MonitorIdentity;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FakeRenderPresentLutCall {
    pub overlay_swap_chain: usize,
    pub swap_chain_path: crate::profile::SwapChainVtablePath,
    pub monitor_identity: Option<MonitorIdentity>,
    pub dirty_rects: Vec<DirtyRect>,
}

static FAKE_RENDER_RESULT: OnceLock<Mutex<Result<PresentLutOutcome, RenderAcquireError>>> =
    OnceLock::new();
static FAKE_RENDER_CALL: OnceLock<Mutex<Option<FakeRenderPresentLutCall>>> = OnceLock::new();
static FAKE_RENDER_CONTEXT_ACTIVE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();

fn result_slot() -> &'static Mutex<Result<PresentLutOutcome, RenderAcquireError>> {
    FAKE_RENDER_RESULT.get_or_init(|| Mutex::new(Err(RenderAcquireError::BackBuffer)))
}

fn call_slot() -> &'static Mutex<Option<FakeRenderPresentLutCall>> {
    FAKE_RENDER_CALL.get_or_init(|| Mutex::new(None))
}

fn context_active_slot() -> &'static Mutex<Option<bool>> {
    FAKE_RENDER_CONTEXT_ACTIVE.get_or_init(|| Mutex::new(None))
}

pub(crate) fn set_fake_render_result(result: Result<PresentLutOutcome, RenderAcquireError>) {
    if let Ok(mut slot) = result_slot().lock() {
        *slot = result;
    }
}

pub(crate) fn reset_fake_render_result() {
    set_fake_render_result(Err(RenderAcquireError::BackBuffer));
    if let Ok(mut calls) = call_slot().lock() {
        *calls = None;
    }
    if let Ok(mut context_active) = context_active_slot().lock() {
        *context_active = None;
    }
}

pub(crate) fn fake_render_present_lut_call() -> Option<FakeRenderPresentLutCall> {
    call_slot().lock().ok().and_then(|call| call.clone())
}

pub(crate) fn fake_render_context_active() -> Option<bool> {
    context_active_slot().lock().ok().and_then(|active| *active)
}

pub(crate) unsafe fn render_present_lut(
    overlay_swap_chain: usize,
    swap_chain_path: crate::profile::SwapChainVtablePath,
    monitor_identity: Option<MonitorIdentity>,
    dirty_rects: &[DirtyRect],
    _assignments: &[crate::state::LutAssignment],
) -> Result<PresentLutOutcome, RenderAcquireError> {
    if let Ok(mut calls) = call_slot().lock() {
        *calls = Some(FakeRenderPresentLutCall {
            overlay_swap_chain,
            swap_chain_path,
            monitor_identity,
            dirty_rects: dirty_rects.to_vec(),
        });
    }
    if let Ok(mut context_active) = context_active_slot().lock() {
        *context_active = Some(crate::state::has_active_contexts());
    }
    result_slot()
        .lock()
        .map(|result| *result)
        .unwrap_or(Err(RenderAcquireError::Unavailable))
}
