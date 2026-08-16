use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use parking_lot::{Mutex, MutexGuard};

use super::{FrameAcquireError, FrameOutcome, RestoreToken, SwapChainRoute};
use crate::dwmcore::{D3D11TextureResolveError, DirtyRect};
use dwm_lut_payload::MonitorIdentity;
use dwm_lut_profile::D3D11ResourceAccess;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FakeFrameOperation {
    Process,
    Restore,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FakeFrameCall {
    pub operation: FakeFrameOperation,
    pub overlay_swap_chain: usize,
    pub d3d11_resource: D3D11ResourceAccess,
    pub monitor_identity: MonitorIdentity,
    pub dirty_rects: Vec<DirtyRect>,
}

struct FakeRenderer {
    result: Result<FrameOutcome, FrameAcquireError>,
    last_call: Option<FakeFrameCall>,
    last_restore_commit: Option<RestoreToken>,
    clear_resources_calls: usize,
}

impl Default for FakeRenderer {
    fn default() -> Self {
        Self {
            result: Err(FrameAcquireError::BackBuffer(
                D3D11TextureResolveError::UnreadableMemory,
            )),
            last_call: None,
            last_restore_commit: None,
            clear_resources_calls: 0,
        }
    }
}

static RENDERER: LazyLock<Mutex<FakeRenderer>> =
    LazyLock::new(|| Mutex::new(FakeRenderer::default()));

pub(crate) struct RendererGuard {
    renderer: MutexGuard<'static, FakeRenderer>,
}

pub(crate) fn set_fake_frame_result(result: Result<FrameOutcome, FrameAcquireError>) {
    RENDERER.lock().result = result;
}

pub(crate) fn reset_fake_frame_result() {
    *RENDERER.lock() = FakeRenderer::default();
}

pub(crate) fn fake_frame_call() -> Option<FakeFrameCall> {
    RENDERER.lock().last_call.clone()
}

pub(crate) fn set_fake_restore_token(monitor_identity: MonitorIdentity, swap_chain: usize) {
    if let Ok(outcome) = &mut RENDERER.lock().result {
        outcome.restore_token = Some(RestoreToken {
            session_id: None,
            monitor_identity,
            route: SwapChainRoute {
                device_id: 1,
                swap_chain,
            },
            back_buffer_id: 1,
            state_generation: 1,
        });
    }
}

pub(crate) fn fake_restore_commit() -> Option<RestoreToken> {
    RENDERER.lock().last_restore_commit
}

pub(crate) fn fake_clear_resources_calls() -> usize {
    RENDERER.lock().clear_resources_calls
}

impl RendererGuard {
    pub(crate) unsafe fn process_frame(
        &mut self,
        overlay_swap_chain: usize,
        d3d11_resource: D3D11ResourceAccess,
        monitor_identity: MonitorIdentity,
        dirty_rects: &[DirtyRect],
        _assignments: &[crate::state::LutAssignment],
    ) -> Result<FrameOutcome, FrameAcquireError> {
        self.renderer.last_call = Some(FakeFrameCall {
            operation: FakeFrameOperation::Process,
            overlay_swap_chain,
            d3d11_resource,
            monitor_identity,
            dirty_rects: dirty_rects.to_vec(),
        });
        self.renderer.result
    }

    pub(crate) unsafe fn restore_back_buffer(
        &mut self,
        overlay_swap_chain: usize,
        d3d11_resource: D3D11ResourceAccess,
        monitor_identity: MonitorIdentity,
        dirty_rects: &[DirtyRect],
    ) -> Result<FrameOutcome, FrameAcquireError> {
        self.renderer.last_call = Some(FakeFrameCall {
            operation: FakeFrameOperation::Restore,
            overlay_swap_chain,
            d3d11_resource,
            monitor_identity,
            dirty_rects: dirty_rects.to_vec(),
        });
        self.renderer.result
    }

    pub(crate) const fn assignments_changed(&mut self) {}

    pub(crate) const fn begin_restore(
        &mut self,
        _active_monitors: Option<&HashSet<MonitorIdentity>>,
    ) -> usize {
        0
    }

    pub(crate) const fn pending_restore_back_buffer_count(&mut self) -> usize {
        0
    }

    pub(crate) fn commit_restore(&mut self, token: RestoreToken) {
        self.renderer.last_restore_commit = Some(token);
    }

    pub(crate) fn wait_for_restore_progress(&mut self, _timeout: Duration) {}

    pub(crate) fn clear_resources(&mut self) {
        self.renderer.clear_resources_calls += 1;
    }
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
