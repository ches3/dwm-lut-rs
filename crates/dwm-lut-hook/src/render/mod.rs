use crate::dwmcore::{D3D11TextureResolveError, DirtyRect};

#[cfg_attr(test, allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameSize {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl FrameSize {
    pub(crate) const fn full_rect(self) -> DirtyRect {
        DirtyRect {
            left: 0,
            top: 0,
            right: self.width as i32,
            bottom: self.height as i32,
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
mod context_state;
#[cfg(test)]
mod fake;
#[cfg_attr(test, allow(dead_code))]
mod frame_plan;
#[cfg_attr(test, allow(dead_code))]
mod render_pass;
#[cfg(not(test))]
mod renderer;
#[cfg_attr(test, allow(dead_code))]
mod resources;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SwapChainRoute {
    device_id: usize,
    swap_chain: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RestoreToken {
    session_id: Option<usize>,
    monitor_identity: dwm_lut_payload::MonitorIdentity,
    route: SwapChainRoute,
    back_buffer_id: usize,
    state_generation: u64,
}

#[cfg(test)]
pub(crate) use frame_plan::DXGI_FORMAT_B8G8R8A8_UNORM;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameSkipReason {
    ZeroSize,
    UnsupportedFormat,
    NoValidDirtyRects,
}

#[cfg(debug_assertions)]
impl FrameSkipReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ZeroSize => "zero_size",
            Self::UnsupportedFormat => "unsupported_format",
            Self::NoValidDirtyRects => "no_valid_dirty_rects",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
pub(crate) enum FrameAcquireError {
    BackBuffer(D3D11TextureResolveError),
    BackBufferIdentity,
    Device,
    Context,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
pub(crate) enum FrameProcessingError {
    DeviceResources,
    LutTexture,
    CleanFrameCreate,
    CleanFrameMissing,
    RenderTargetView,
}

#[cfg(debug_assertions)]
impl FrameProcessingError {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::DeviceResources => "device_resources_create_failed",
            Self::LutTexture => "lut_texture_create_failed",
            Self::CleanFrameCreate => "clean_frame_create_failed",
            Self::CleanFrameMissing => "clean_frame_missing",
            Self::RenderTargetView => "render_target_view_create_failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameStatus {
    LutApplied,
    Restored,
    #[cfg_attr(test, allow(dead_code))]
    Unchanged,
    #[cfg_attr(test, allow(dead_code))]
    Skipped(FrameSkipReason),
    #[cfg_attr(test, allow(dead_code))]
    Failed(FrameProcessingError),
}

#[cfg(debug_assertions)]
impl FrameStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::LutApplied => "applied",
            Self::Restored => "restored",
            Self::Unchanged => "unchanged",
            Self::Skipped(reason) => reason.as_str(),
            Self::Failed(reason) => reason.as_str(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FrameOutcome {
    pub present_dirty_rect: Option<DirtyRect>,
    pub status: FrameStatus,
    pub dxgi_format: u32,
    pub frame_size: FrameSize,
    pub lut_index: Option<usize>,
    pub restore_token: Option<RestoreToken>,
    #[cfg(debug_assertions)]
    pub(crate) back_buffer_id: usize,
}

#[cfg(not(test))]
pub(crate) use renderer::{lock_renderer, try_lock_renderer};

#[cfg(test)]
pub(crate) use fake::{
    FakeFrameOperation, fake_clear_resources_calls, fake_frame_call, fake_restore_commit,
    lock_renderer, reset_fake_frame_result, set_fake_frame_result, set_fake_restore_token,
    try_lock_renderer,
};
