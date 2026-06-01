use super::RenderPresentLutResult;
use crate::lut_pipeline::{ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DirtyRect, LutPipeline};
use dwm_lut_payload::MonitorIdentity;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static TEST_RENDER_PRESENT_LUT_RESULT: AtomicBool = AtomicBool::new(false);

static TEST_RENDER_PRESENT_DIRTY_RECT: OnceLock<Mutex<Option<DirtyRect>>> = OnceLock::new();

static TEST_RENDER_PRESENT_DXGI_FORMAT: OnceLock<Mutex<Option<u32>>> = OnceLock::new();

static TEST_RENDER_CONTEXT_ACTIVE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TestRenderPresentLutCall {
    pub overlay_swap_chain: usize,
    pub swap_chain_path: crate::profile::SwapChainPathHypothesis,
    pub monitor_identity: Option<MonitorIdentity>,
    pub clip_box: ClipBox,
    pub dirty_rects: Vec<DirtyRect>,
}

static TEST_RENDER_PRESENT_LUT_CALL: OnceLock<Mutex<Option<TestRenderPresentLutCall>>> =
    OnceLock::new();

pub(crate) fn set_test_render_present_lut_result(result: bool) {
    TEST_RENDER_PRESENT_LUT_RESULT.store(result, Ordering::Release);
    set_test_render_present_dirty_rect(None);
    set_test_render_present_dxgi_format(None);
}

pub(crate) fn set_test_render_present_lut_result_with_present_rect(
    result: bool,
    rect: Option<DirtyRect>,
) {
    TEST_RENDER_PRESENT_LUT_RESULT.store(result, Ordering::Release);
    set_test_render_present_dirty_rect(rect);
    set_test_render_present_dxgi_format(None);
}

fn set_test_render_present_dirty_rect(rect: Option<DirtyRect>) {
    let result = TEST_RENDER_PRESENT_DIRTY_RECT.get_or_init(|| Mutex::new(None));
    if let Ok(mut result) = result.lock() {
        *result = rect;
    }
}

pub(crate) fn set_test_render_present_dxgi_format(format: Option<u32>) {
    let result = TEST_RENDER_PRESENT_DXGI_FORMAT.get_or_init(|| Mutex::new(None));
    if let Ok(mut result) = result.lock() {
        *result = format;
    }
}

pub(crate) fn reset_test_render_present_lut_result() {
    set_test_render_present_lut_result(false);
    let calls = TEST_RENDER_PRESENT_LUT_CALL.get_or_init(|| Mutex::new(None));
    if let Ok(mut calls) = calls.lock() {
        *calls = None;
    }
    set_test_render_present_dirty_rect(None);
    set_test_render_present_dxgi_format(None);
    let context_active = TEST_RENDER_CONTEXT_ACTIVE.get_or_init(|| Mutex::new(None));
    if let Ok(mut context_active) = context_active.lock() {
        *context_active = None;
    }
}

pub(crate) fn test_render_present_lut_call() -> Option<TestRenderPresentLutCall> {
    TEST_RENDER_PRESENT_LUT_CALL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|call| call.clone())
}

pub(crate) fn test_render_context_active() -> Option<bool> {
    TEST_RENDER_CONTEXT_ACTIVE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|active| *active)
}

pub(crate) unsafe fn render_present_lut(
    overlay_swap_chain: usize,
    swap_chain_path: crate::profile::SwapChainPathHypothesis,
    monitor_identity: Option<MonitorIdentity>,
    clip_box: ClipBox,
    dirty_rects: &[DirtyRect],
    pipeline: &LutPipeline,
) -> RenderPresentLutResult {
    let calls = TEST_RENDER_PRESENT_LUT_CALL.get_or_init(|| Mutex::new(None));
    if let Ok(mut calls) = calls.lock() {
        *calls = Some(TestRenderPresentLutCall {
            overlay_swap_chain,
            swap_chain_path,
            monitor_identity,
            clip_box,
            dirty_rects: dirty_rects.to_vec(),
        });
    }
    let context_active = TEST_RENDER_CONTEXT_ACTIVE.get_or_init(|| Mutex::new(None));
    if let Ok(mut context_active) = context_active.lock() {
        *context_active = Some(
            crate::state::lut_bypass_runtime().is_some_and(|runtime| runtime.has_active_contexts()),
        );
    }
    let present_dirty_rect = TEST_RENDER_PRESENT_DIRTY_RECT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|rect| *rect);
    let dxgi_format = TEST_RENDER_PRESENT_DXGI_FORMAT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|format| *format);
    RenderPresentLutResult {
        lut_applied: TEST_RENDER_PRESENT_LUT_RESULT.load(Ordering::Acquire),
        dxgi_format,
        lut_index: dxgi_format
            .or(Some(DXGI_FORMAT_B8G8R8A8_UNORM))
            .and_then(|format| {
                monitor_identity.and_then(|identity| {
                    pipeline.build_present_plan_for_monitor_identity(
                        identity,
                        clip_box,
                        format,
                        dirty_rects,
                    )
                })
            })
            .map(|plan| plan.lut_index),
        present_dirty_rect,
    }
}

pub(crate) fn shutdown_renderer_resources() -> usize {
    0
}
