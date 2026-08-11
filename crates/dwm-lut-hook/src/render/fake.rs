use std::sync::LazyLock;

use parking_lot::{Mutex, MutexGuard};

use super::{FrameAcquireError, LutApplyOutcome};
use crate::dwmcore::{D3D11TextureResolveError, DirtyRect};
use dwm_lut_payload::MonitorIdentity;
use dwm_lut_profile::D3D11ResourceAccess;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FakeApplyLutCall {
    pub overlay_swap_chain: usize,
    pub d3d11_resource: D3D11ResourceAccess,
    pub monitor_identity: MonitorIdentity,
    pub dirty_rects: Vec<DirtyRect>,
}

struct FakeRenderer {
    result: Result<LutApplyOutcome, FrameAcquireError>,
    last_call: Option<FakeApplyLutCall>,
}

impl Default for FakeRenderer {
    fn default() -> Self {
        Self {
            result: Err(FrameAcquireError::BackBuffer(
                D3D11TextureResolveError::UnreadableMemory,
            )),
            last_call: None,
        }
    }
}

static RENDERER: LazyLock<Mutex<FakeRenderer>> =
    LazyLock::new(|| Mutex::new(FakeRenderer::default()));

pub(crate) struct RendererGuard {
    renderer: MutexGuard<'static, FakeRenderer>,
}

pub(crate) fn set_fake_apply_result(result: Result<LutApplyOutcome, FrameAcquireError>) {
    RENDERER.lock().result = result;
}

pub(crate) fn reset_fake_apply_result() {
    *RENDERER.lock() = FakeRenderer::default();
}

pub(crate) fn fake_apply_lut_call() -> Option<FakeApplyLutCall> {
    RENDERER.lock().last_call.clone()
}

impl RendererGuard {
    pub(crate) unsafe fn apply_lut(
        &mut self,
        overlay_swap_chain: usize,
        d3d11_resource: D3D11ResourceAccess,
        monitor_identity: MonitorIdentity,
        dirty_rects: &[DirtyRect],
        _assignments: &[crate::state::LutAssignment],
    ) -> Result<LutApplyOutcome, FrameAcquireError> {
        self.renderer.last_call = Some(FakeApplyLutCall {
            overlay_swap_chain,
            d3d11_resource,
            monitor_identity,
            dirty_rects: dirty_rects.to_vec(),
        });
        self.renderer.result
    }

    pub(crate) const fn clear_resources(&mut self) {}
}

pub(crate) fn try_lock_renderer() -> Option<RendererGuard> {
    RENDERER
        .try_lock()
        .map(|renderer| RendererGuard { renderer })
}

pub(crate) fn lock_renderer() -> RendererGuard {
    RendererGuard {
        renderer: RENDERER.lock(),
    }
}

#[test]
fn renderer_lock_is_exclusive() {
    let _test_guard = crate::state::HOOK_GLOBAL_TEST_LOCK
        .lock()
        .expect("test mutex should lock");
    let _guard = lock_renderer();

    assert!(try_lock_renderer().is_none());
}
