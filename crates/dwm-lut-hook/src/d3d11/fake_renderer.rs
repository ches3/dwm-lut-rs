use super::{PresentLutOutcome, RenderAcquireError};
use crate::dwmcore::DirtyRect;
use dwm_lut_payload::MonitorIdentity;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FakeRenderPresentLutCall {
    pub overlay_swap_chain: usize,
    pub swap_chain_to_resource_path: dwm_lut_profile::SwapChainToResourcePath,
    pub monitor_identity: Option<MonitorIdentity>,
    pub dirty_rects: Vec<DirtyRect>,
}

static FAKE_RENDER_RESULT: OnceLock<Mutex<Result<PresentLutOutcome, RenderAcquireError>>> =
    OnceLock::new();
static FAKE_RENDER_CALL: OnceLock<Mutex<Option<FakeRenderPresentLutCall>>> = OnceLock::new();

fn result_slot() -> &'static Mutex<Result<PresentLutOutcome, RenderAcquireError>> {
    FAKE_RENDER_RESULT.get_or_init(|| Mutex::new(Err(RenderAcquireError::BackBuffer)))
}

fn call_slot() -> &'static Mutex<Option<FakeRenderPresentLutCall>> {
    FAKE_RENDER_CALL.get_or_init(|| Mutex::new(None))
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
}

pub(crate) fn fake_render_present_lut_call() -> Option<FakeRenderPresentLutCall> {
    call_slot().lock().ok().and_then(|call| call.clone())
}

pub(crate) unsafe fn render_present_lut(
    overlay_swap_chain: usize,
    swap_chain_to_resource_path: dwm_lut_profile::SwapChainToResourcePath,
    monitor_identity: Option<MonitorIdentity>,
    dirty_rects: &[DirtyRect],
    _assignments: &[crate::state::LutAssignment],
) -> Result<PresentLutOutcome, RenderAcquireError> {
    if let Ok(mut calls) = call_slot().lock() {
        *calls = Some(FakeRenderPresentLutCall {
            overlay_swap_chain,
            swap_chain_to_resource_path,
            monitor_identity,
            dirty_rects: dirty_rects.to_vec(),
        });
    }
    result_slot()
        .lock()
        .map(|result| *result)
        .unwrap_or(Err(RenderAcquireError::Unavailable))
}
