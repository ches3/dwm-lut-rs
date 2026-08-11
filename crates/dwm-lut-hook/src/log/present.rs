#![cfg_attr(not(debug_assertions), allow(dead_code))]

use dwm_lut_payload::MonitorIdentity;

use crate::DirtyRect;
#[cfg(debug_assertions)]
use crate::d3d11::{BackBufferId, PresentDrawStatus};
use crate::d3d11::{PresentLutOutcome, RenderAcquireError};
use crate::present::PresentInputError;

#[cfg(debug_assertions)]
use super::SharedLimiter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentLutAcquireFailReason {
    LockMiss,
    Acquire(RenderAcquireError),
}

impl From<RenderAcquireError> for PresentLutAcquireFailReason {
    fn from(value: RenderAcquireError) -> Self {
        Self::Acquire(value)
    }
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PresentLutLogKey {
    overlay_swap_chain: usize,
    back_buffer: Option<BackBufferId>,
}

#[cfg(debug_assertions)]
static PRESENT_LUT_LIMITER: SharedLimiter<PresentLutLogKey> = SharedLimiter::new(300);

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

pub(crate) fn present_lut_acquire_failed(
    overlay_swap_chain: usize,
    error: PresentLutAcquireFailReason,
) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=present_lut_acquire_failed overlay_swap_chain=0x{overlay_swap_chain:x} error={error:?}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (overlay_swap_chain, error);
}

pub(crate) fn present_lut_frame(
    overlay_swap_chain: usize,
    monitor_identity: MonitorIdentity,
    dirty_rects: &[DirtyRect],
    outcome: PresentLutOutcome,
) {
    #[cfg(debug_assertions)]
    {
        let is_err = matches!(outcome.draw, PresentDrawStatus::Failed(_));
        if !is_err {
            let key = PresentLutLogKey {
                overlay_swap_chain,
                back_buffer: outcome.back_buffer_id,
            };
            if !PRESENT_LUT_LIMITER.sample(key).should_log {
                return;
            }
        }

        let monitor_identity = format!(
            "{}:{}",
            monitor_identity.adapter_luid, monitor_identity.target_id
        );
        super::write(format_args!(
            "event=present_lut_frame overlay_swap_chain=0x{:x} applied={} draw={} lut_active={} dxgi_format={:?} width={:?} height={:?} lut_index={:?} back_buffer_id={} dirty_rects={:?} present_dirty_rect={:?} monitor_identity={}",
            overlay_swap_chain,
            u8::from(outcome.lut_applied()),
            outcome.draw.as_str(),
            u8::from(outcome.lut_active),
            outcome.dxgi_format,
            outcome.width,
            outcome.height,
            outcome.lut_index,
            super::quoted(outcome.back_buffer_id_for_log()),
            dirty_rects,
            outcome.present_dirty_rect,
            super::quoted(monitor_identity),
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (overlay_swap_chain, monitor_identity, dirty_rects, outcome);
}
