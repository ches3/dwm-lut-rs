#![cfg_attr(not(debug_assertions), allow(dead_code))]

use dwm_lut_payload::MonitorIdentity;

use crate::DirtyRect;
use crate::d3d11::{PresentLutOutcome, RenderAcquireError};
use crate::present::PresentInputError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresentLutAcquireFailReason {
    LockMiss,
    Acquire(RenderAcquireError),
}

impl PresentLutAcquireFailReason {
    #[cfg(debug_assertions)]
    const fn as_str(self) -> &'static str {
        match self {
            Self::LockMiss => "lock_miss",
            Self::Acquire(error) => error.as_str(),
        }
    }
}

impl From<RenderAcquireError> for PresentLutAcquireFailReason {
    fn from(value: RenderAcquireError) -> Self {
        Self::Acquire(value)
    }
}

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

pub(crate) fn present_lut_acquire_failed(
    overlay_swap_chain: usize,
    reason: PresentLutAcquireFailReason,
) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=present_lut_acquire_failed overlay_swap_chain=0x{overlay_swap_chain:x} reason={}",
            reason.as_str()
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (overlay_swap_chain, reason);
}

pub(crate) fn present_lut_frame(
    overlay_swap_chain: usize,
    hardware_protected: bool,
    monitor_identity: Option<MonitorIdentity>,
    dirty_rects: &[DirtyRect],
    outcome: PresentLutOutcome,
) {
    #[cfg(debug_assertions)]
    {
        let monitor_identity = monitor_identity
            .map(|identity| format!("{}:{}", identity.adapter_luid, identity.target_id))
            .unwrap_or_else(|| "none".to_owned());
        super::write(format_args!(
            "event=present_lut_frame overlay_swap_chain=0x{:x} applied={} draw={} lut_active={} dxgi_format={:?} width={:?} height={:?} lut_index={:?} back_buffer_id={} dirty_rects={:?} present_dirty_rect={:?} monitor_identity={} hardware_protected={}",
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
            u8::from(hardware_protected)
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (
        overlay_swap_chain,
        hardware_protected,
        monitor_identity,
        dirty_rects,
        outcome,
    );
}
