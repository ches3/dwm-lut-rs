#![cfg_attr(not(debug_assertions), allow(dead_code))]

use dwm_lut_payload::{
    InitializeStatus, MonitorIdentity, ReplaceAssignmentsStatus, ShutdownStatus,
};

use crate::DirtyRect;
use crate::d3d11::{PresentLutOutcome, RenderAcquireError};
use crate::minhook::MinHookCleanupFailure;
use crate::present::PresentInputError;
use crate::profile::{DwmcoreVersion, HookTarget};
use crate::resolver::SkippedSignatureReason;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisableIndependentFlipRejectReason {
    PageNotWritable,
    UnexpectedValue(i32),
}

pub(crate) fn initialize_start() {
    #[cfg(debug_assertions)]
    super::write(format_args!("event=initialize_start"));
}

pub(crate) fn initialize_success() {
    #[cfg(debug_assertions)]
    super::write(format_args!("event=initialize_success"));
}

pub(crate) fn initialize_failed(status: InitializeStatus, error: impl std::fmt::Display) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=initialize_failed status={} error={}",
            status as u32,
            super::quoted(error)
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (status, error);
}

pub(crate) fn shutdown_start() {
    #[cfg(debug_assertions)]
    super::write(format_args!("event=shutdown_start"));
}

pub(crate) fn shutdown_finished_reason(status: ShutdownStatus, reason: &'static str) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=shutdown_finished_reason status={} reason={}",
            status as u32,
            super::quoted(reason)
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (status, reason);
}

pub(crate) fn shutdown_finished_cleanup(status: ShutdownStatus, cleanup_failure_count: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=shutdown_finished_cleanup status={} cleanup_failure_count={}",
            status as u32, cleanup_failure_count
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (status, cleanup_failure_count);
}

pub(crate) fn renderer_resources_released(device_resource_count: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=renderer_resources_released device_resource_count={device_resource_count}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = device_resource_count;
}

pub(crate) fn minhook_cleanup_failed(failure: MinHookCleanupFailure) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=minhook_cleanup_failed operation={:?} target={} status={}",
            failure.operation,
            super::quoted(failure.target.label()),
            failure.status
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = failure;
}

pub(crate) fn replace_assignments_start() {
    #[cfg(debug_assertions)]
    super::write(format_args!("event=replace_assignments_start"));
}

pub(crate) fn replace_assignments_success() {
    #[cfg(debug_assertions)]
    super::write(format_args!("event=replace_assignments_success"));
}

pub(crate) fn replace_assignments_failed(
    status: ReplaceAssignmentsStatus,
    error: impl std::fmt::Display,
) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=replace_assignments_failed status={} error={}",
            status as u32,
            super::quoted(error)
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (status, error);
}

pub(crate) fn replace_assignments_decoded(assignment_count: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=replace_assignments_decoded assignment_count={assignment_count}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = assignment_count;
}

pub(crate) fn replace_assignments_luts_prepared(lut_count: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=replace_assignments_luts_prepared lut_count={lut_count}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = lut_count;
}

pub(crate) fn replace_assignments_renderer_resources_released(device_resource_count: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=replace_assignments_renderer_resources_released device_resource_count={device_resource_count}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = device_resource_count;
}

pub(crate) fn profile_selected(min_version: DwmcoreVersion, dwmcore_version: DwmcoreVersion) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=profile_selected min_version={min_version} dwmcore_version={dwmcore_version}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (min_version, dwmcore_version);
}

pub(crate) fn payload_decoded(assignment_count: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=payload_decoded assignment_count={assignment_count}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = assignment_count;
}

pub(crate) fn luts_prepared(lut_count: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!("event=luts_prepared lut_count={lut_count}"));
    }
    #[cfg(not(debug_assertions))]
    let _ = lut_count;
}

pub(crate) fn hooks_reenabled() {
    #[cfg(debug_assertions)]
    super::write(format_args!("event=hooks_reenabled"));
}

pub(crate) fn hooks_enabled(hook_count: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!("event=hooks_enabled hook_count={hook_count}"));
    }
    #[cfg(not(debug_assertions))]
    let _ = hook_count;
}

pub(crate) fn hooks_created(hook_count: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!("event=hooks_created hook_count={hook_count}"));
    }
    #[cfg(not(debug_assertions))]
    let _ = hook_count;
}

pub(crate) fn signatures_resolved(
    module_name: &str,
    module_base: usize,
    module_size: usize,
    target_count: usize,
    skipped_count: usize,
) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=signatures_resolved module={} module_base=0x{:x} module_size=0x{:x} target_count={} skipped_count={}",
            super::quoted(module_name),
            module_base,
            module_size,
            target_count,
            skipped_count
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (
        module_name,
        module_base,
        module_size,
        target_count,
        skipped_count,
    );
}

pub(crate) fn signature_resolved(target: HookTarget, address: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=signature_resolved target={} address=0x{:x}",
            super::quoted(target.label()),
            address
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (target, address);
}

pub(crate) fn signature_skipped(target: HookTarget, reason: SkippedSignatureReason) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=signature_skipped target={} reason={reason:?}",
            super::quoted(target.label())
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (target, reason);
}

pub(crate) fn disable_independent_flip_address(present: bool, address: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=disable_independent_flip_address present={present} address=0x{address:x}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (present, address);
}

pub(crate) fn disable_independent_flip_rejected(reason: DisableIndependentFlipRejectReason) {
    #[cfg(debug_assertions)]
    {
        match reason {
            DisableIndependentFlipRejectReason::PageNotWritable => {
                super::write(format_args!(
                    "event=disable_independent_flip_rejected reason={}",
                    super::quoted("page_not_writable")
                ));
            }
            DisableIndependentFlipRejectReason::UnexpectedValue(value) => {
                super::write(format_args!(
                    "event=disable_independent_flip_rejected reason={} value={value}",
                    super::quoted("unexpected_value")
                ));
            }
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = reason;
}

pub(crate) fn disable_independent_flip_applied() {
    #[cfg(debug_assertions)]
    super::write(format_args!(
        "event=disable_independent_flip_applied value=1"
    ));
}

pub(crate) fn disable_independent_flip_restored() {
    #[cfg(debug_assertions)]
    super::write(format_args!("event=disable_independent_flip_restored"));
}

pub(crate) fn overlays_enabled_override(value: Option<bool>) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=overlays_enabled_override value={value:?}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = value;
}

pub(crate) fn flip_gate_denied(gate: &str, denied_total: u64) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=flip_gate_denied gate={gate} denied_total={denied_total}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (gate, denied_total);
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn desktop_redraw_requested(result: i32, flags: u32) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=desktop_redraw_requested result={result} flags=0x{flags:x}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (result, flags);
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn back_buffer_identity_fallback(reason: &str, back_buffer: usize, identity: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=back_buffer_identity_fallback reason={reason} back_buffer=0x{back_buffer:x} identity=0x{identity:x}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (reason, back_buffer, identity);
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
            "event=present_lut_frame overlay_swap_chain=0x{:x} acquired=1 applied={} draw={} lut_active={} dxgi_format={:?} width={:?} height={:?} lut_index={:?} back_buffer_id={} dirty_rects={:?} present_dirty_rect={:?} monitor_identity={} hardware_protected={}",
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
