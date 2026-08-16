#![cfg_attr(not(debug_assertions), allow(dead_code))]

use dwm_lut_payload::MonitorIdentity;

use crate::DirtyRect;
use crate::present::PresentInputError;
#[cfg(debug_assertions)]
use crate::render::FrameStatus;
use crate::render::{FrameAcquireError, FrameOutcome};

#[cfg(debug_assertions)]
use super::SharedLimiter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameUnavailableReason {
    RendererBusy,
    AssignmentsUnavailable,
    Frame(FrameAcquireError),
}

impl From<FrameAcquireError> for FrameUnavailableReason {
    fn from(value: FrameAcquireError) -> Self {
        Self::Frame(value)
    }
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct FrameLogKey {
    overlay_swap_chain: usize,
    back_buffer: usize,
}

#[cfg(debug_assertions)]
static FRAME_LOG_LIMITER: SharedLimiter<FrameLogKey> = SharedLimiter::new(300);

#[cfg(debug_assertions)]
static SKIPPED_HARDWARE_PROTECTED_LIMITER: SharedLimiter<usize> = SharedLimiter::new(300);

pub(crate) fn present_input_collect_error(
    this: usize,
    overlay_swap_chain: usize,
    rect_vec: usize,
    error: PresentInputError,
) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=present_input_collect_error this=0x{this:x} overlay_swap_chain=0x{overlay_swap_chain:x} rect_vec=0x{rect_vec:x} error={error:?}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (this, overlay_swap_chain, rect_vec, error);
}

pub(crate) fn skipped_hardware_protected(overlay_swap_chain: usize) {
    #[cfg(debug_assertions)]
    {
        if !SKIPPED_HARDWARE_PROTECTED_LIMITER
            .sample(overlay_swap_chain)
            .should_log
        {
            return;
        }
        super::write(format_args!(
            "event=skipped_hardware_protected overlay_swap_chain=0x{overlay_swap_chain:x}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = overlay_swap_chain;
}

pub(crate) fn frame_unavailable(overlay_swap_chain: usize, reason: FrameUnavailableReason) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=frame_unavailable overlay_swap_chain=0x{overlay_swap_chain:x} reason={reason:?}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (overlay_swap_chain, reason);
}

pub(crate) fn frame_processed(
    overlay_swap_chain: usize,
    monitor_identity: MonitorIdentity,
    dirty_rects: &[DirtyRect],
    outcome: FrameOutcome,
) {
    #[cfg(debug_assertions)]
    {
        let is_err = matches!(outcome.status, FrameStatus::Failed(_));
        if !is_err {
            let key = FrameLogKey {
                overlay_swap_chain,
                back_buffer: outcome.back_buffer_id,
            };
            if !FRAME_LOG_LIMITER.sample(key).should_log {
                return;
            }
        }

        let lut_applied = matches!(outcome.status, FrameStatus::LutApplied);
        let lut_active = outcome.lut_index.is_some();
        let monitor_identity = format!(
            "{}:{}",
            monitor_identity.adapter_luid, monitor_identity.target_id
        );
        super::write(format_args!(
            "event=frame_processed overlay_swap_chain=0x{:x} applied={} status={} lut_active={} dxgi_format={} width={} height={} lut_index={:?} back_buffer_id={} dirty_rects={:?} present_dirty_rect={:?} monitor_identity={}",
            overlay_swap_chain,
            u8::from(lut_applied),
            outcome.status.as_str(),
            u8::from(lut_active),
            outcome.dxgi_format,
            outcome.frame_size.width,
            outcome.frame_size.height,
            outcome.lut_index,
            super::quoted(format_args!("0x{:x}", outcome.back_buffer_id)),
            dirty_rects,
            outcome.present_dirty_rect,
            super::quoted(monitor_identity),
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (overlay_swap_chain, monitor_identity, dirty_rects, outcome);
}
