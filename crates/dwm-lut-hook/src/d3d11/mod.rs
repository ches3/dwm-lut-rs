use crate::present::DirtyRect;
use crate::state::LutAssignment;
use dwm_lut_payload::MonitorIdentity;

#[cfg(not(test))]
mod back_buffer;
#[cfg(not(test))]
mod context_state;
#[cfg(not(test))]
mod d3d11_api;
#[cfg(test)]
mod fake_renderer;
mod plan;
#[cfg(not(test))]
mod renderer;

#[cfg(test)]
pub(crate) use plan::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrawPlanSkipReason {
    ZeroSize,
    MissingMonitorIdentity,
    UnsupportedFormat,
    MissingAssignment,
    EmptyDirtyRects,
}

#[cfg(debug_assertions)]
impl DrawPlanSkipReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ZeroSize => "zero_size",
            Self::MissingMonitorIdentity => "missing_monitor_identity",
            Self::UnsupportedFormat => "unsupported_format",
            Self::MissingAssignment => "missing_assignment",
            Self::EmptyDirtyRects => "empty_dirty_rects",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
pub(crate) enum RenderAcquireError {
    BackBuffer,
    Device,
    Context,
    Unavailable,
}

#[cfg(debug_assertions)]
impl RenderAcquireError {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::BackBuffer => "back_buffer",
            Self::Device => "device",
            Self::Context => "context",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
pub(crate) enum PresentDrawFailReason {
    ResourcesCreateFailed,
    ResourcesMissing,
    LutIndexOutOfRange,
    DrawRectsEmpty,
    CopyTextureCreateFailed,
    RenderTargetViewCreateFailed,
    DrawFailed,
}

#[cfg(debug_assertions)]
impl PresentDrawFailReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ResourcesCreateFailed => "resources_create_failed",
            Self::ResourcesMissing => "resources_missing",
            Self::LutIndexOutOfRange => "lut_index_out_of_range",
            Self::DrawRectsEmpty => "draw_rects_empty",
            Self::CopyTextureCreateFailed => "copy_texture_create_failed",
            Self::RenderTargetViewCreateFailed => "render_target_view_create_failed",
            Self::DrawFailed => "draw_failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentDrawStatus {
    Applied { full_redraw: bool },
    Skipped(DrawPlanSkipReason),
    Failed(PresentDrawFailReason),
}

#[cfg(debug_assertions)]
impl PresentDrawStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Applied { .. } => "applied",
            Self::Skipped(reason) => reason.as_str(),
            Self::Failed(reason) => reason.as_str(),
        }
    }

    pub(crate) const fn lut_applied(self) -> bool {
        matches!(self, Self::Applied { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PresentLutOutcome {
    pub lut_active: bool,
    pub present_dirty_rect: Option<DirtyRect>,
    pub draw: PresentDrawStatus,
    pub dxgi_format: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub lut_index: Option<usize>,
    #[cfg(debug_assertions)]
    pub(crate) back_buffer_id: Option<BackBufferId>,
}

#[cfg(debug_assertions)]
impl PresentLutOutcome {
    pub(crate) const fn lut_applied(self) -> bool {
        self.draw.lut_applied()
    }

    pub(crate) fn back_buffer_id_for_log(self) -> String {
        self.back_buffer_id
            .map(BackBufferId::format_for_log)
            .unwrap_or_else(|| "none".to_owned())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum BackBufferId {
    PrivateData(u128),
    ComIdentity(usize),
}

#[cfg(debug_assertions)]
impl BackBufferId {
    fn format_for_log(self) -> String {
        match self {
            Self::PrivateData(id) => format!("private:0x{id:x}"),
            Self::ComIdentity(id) => format!("com:0x{id:x}"),
        }
    }
}

pub(crate) unsafe fn render_present_lut(
    overlay_swap_chain: usize,
    swap_chain_path: crate::profile::SwapChainVtablePath,
    monitor_identity: Option<MonitorIdentity>,
    dirty_rects: &[DirtyRect],
    assignments: &[LutAssignment],
) -> Result<PresentLutOutcome, RenderAcquireError> {
    #[cfg(test)]
    {
        unsafe {
            fake_renderer::render_present_lut(
                overlay_swap_chain,
                swap_chain_path,
                monitor_identity,
                dirty_rects,
                assignments,
            )
        }
    }
    #[cfg(not(test))]
    {
        unsafe {
            renderer::render_present_lut(
                overlay_swap_chain,
                swap_chain_path,
                monitor_identity,
                dirty_rects,
                assignments,
            )
        }
    }
}

#[cfg(not(test))]
pub(crate) fn shutdown_renderer_resources() -> usize {
    renderer::shutdown_renderer_resources()
}

#[cfg(test)]
pub(crate) fn shutdown_renderer_resources() -> usize {
    0
}

#[cfg(test)]
pub(crate) use fake_renderer::{
    fake_render_context_active, fake_render_present_lut_call, reset_fake_render_result,
    set_fake_render_result,
};
