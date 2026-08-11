use super::{PresentLutOutcome, RenderAcquireError};
use crate::dwmcore::{D3D11TextureResolveError, DirtyRect};
use dwm_lut_payload::MonitorIdentity;
use parking_lot::{Mutex as ParkingMutex, MutexGuard as ParkingMutexGuard};
use std::sync::{LazyLock, Mutex, OnceLock};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FakeRenderPresentLutCall {
    pub overlay_swap_chain: usize,
    pub d3d11_resource: dwm_lut_profile::D3D11ResourceAccess,
    pub monitor_identity: MonitorIdentity,
    pub dirty_rects: Vec<DirtyRect>,
}

static FAKE_RENDER_RESULT: OnceLock<Mutex<Result<PresentLutOutcome, RenderAcquireError>>> =
    OnceLock::new();
static FAKE_RENDER_CALL: OnceLock<Mutex<Option<FakeRenderPresentLutCall>>> = OnceLock::new();
static RENDERER_LOCK: LazyLock<ParkingMutex<()>> = LazyLock::new(|| ParkingMutex::new(()));

pub(crate) struct RendererGuard {
    _guard: ParkingMutexGuard<'static, ()>,
}

fn result_slot() -> &'static Mutex<Result<PresentLutOutcome, RenderAcquireError>> {
    FAKE_RENDER_RESULT.get_or_init(|| {
        Mutex::new(Err(RenderAcquireError::BackBuffer(
            D3D11TextureResolveError::UnreadableMemory,
        )))
    })
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
    set_fake_render_result(Err(RenderAcquireError::BackBuffer(
        D3D11TextureResolveError::UnreadableMemory,
    )));
    if let Ok(mut calls) = call_slot().lock() {
        *calls = None;
    }
}

pub(crate) fn fake_render_present_lut_call() -> Option<FakeRenderPresentLutCall> {
    call_slot().lock().ok().and_then(|call| call.clone())
}

impl RendererGuard {
    pub(crate) unsafe fn render_present_lut(
        &mut self,
        overlay_swap_chain: usize,
        d3d11_resource: dwm_lut_profile::D3D11ResourceAccess,
        monitor_identity: MonitorIdentity,
        dirty_rects: &[DirtyRect],
        _assignments: &[crate::state::LutAssignment],
    ) -> Result<PresentLutOutcome, RenderAcquireError> {
        if let Ok(mut calls) = call_slot().lock() {
            *calls = Some(FakeRenderPresentLutCall {
                overlay_swap_chain,
                d3d11_resource,
                monitor_identity,
                dirty_rects: dirty_rects.to_vec(),
            });
        }
        result_slot()
            .lock()
            .map(|result| *result)
            .unwrap_or(Err(RenderAcquireError::Unavailable))
    }

    pub(crate) const fn clear_resources(&mut self) -> usize {
        0
    }
}

pub(crate) fn try_lock_renderer() -> Option<RendererGuard> {
    RENDERER_LOCK
        .try_lock()
        .map(|guard| RendererGuard { _guard: guard })
}

pub(crate) fn lock_renderer() -> RendererGuard {
    let guard = RENDERER_LOCK.lock();
    RendererGuard { _guard: guard }
}

#[test]
fn renderer_lock_is_exclusive() {
    let _test_guard = crate::state::HOOK_GLOBAL_TEST_LOCK
        .lock()
        .expect("test mutex should lock");
    let _guard = lock_renderer();

    assert!(try_lock_renderer().is_none());
}
