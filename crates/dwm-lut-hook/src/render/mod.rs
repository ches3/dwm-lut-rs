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

#[cfg(test)]
pub(crate) use frame_plan::DXGI_FORMAT_B8G8R8A8_UNORM;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameSkipReason {
    ZeroSize,
    UnsupportedFormat,
    MissingAssignment,
    NoValidDirtyRects,
}

#[cfg(debug_assertions)]
impl FrameSkipReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ZeroSize => "zero_size",
            Self::UnsupportedFormat => "unsupported_format",
            Self::MissingAssignment => "missing_assignment",
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
pub(crate) enum LutRenderFailure {
    DeviceResourcesCreateFailed,
    LutIndexOutOfRange,
    SourceTextureCreateFailed,
    RenderTargetViewCreateFailed,
}

#[cfg(debug_assertions)]
impl LutRenderFailure {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::DeviceResourcesCreateFailed => "device_resources_create_failed",
            Self::LutIndexOutOfRange => "lut_index_out_of_range",
            Self::SourceTextureCreateFailed => "source_texture_create_failed",
            Self::RenderTargetViewCreateFailed => "render_target_view_create_failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LutApplyStatus {
    Applied,
    #[cfg_attr(test, allow(dead_code))]
    Skipped(FrameSkipReason),
    #[cfg_attr(test, allow(dead_code))]
    Failed(LutRenderFailure),
}

#[cfg(debug_assertions)]
impl LutApplyStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Skipped(reason) => reason.as_str(),
            Self::Failed(reason) => reason.as_str(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LutApplyOutcome {
    pub present_dirty_rect: Option<DirtyRect>,
    pub status: LutApplyStatus,
    pub dxgi_format: u32,
    pub frame_size: FrameSize,
    pub lut_index: Option<usize>,
    #[cfg(debug_assertions)]
    pub(crate) back_buffer_id: u128,
}

#[cfg(not(test))]
pub(crate) use renderer::{lock_renderer, try_lock_renderer};

#[cfg(test)]
pub(crate) use fake::{
    fake_apply_lut_call, lock_renderer, reset_fake_apply_result, set_fake_apply_result,
    try_lock_renderer,
};
