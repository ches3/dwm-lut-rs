#![cfg_attr(not(debug_assertions), allow(dead_code))]

#[cfg(debug_assertions)]
use dwm_lut_payload::ShutdownStatus;
use dwm_lut_payload::{InitializeStatus, MonitorIdentity, ReplaceAssignmentsStatus};

use crate::minhook::RegisteredHook;
use crate::resolver::SignatureResolutionReport;
use dwm_lut_profile::DwmcoreVersion;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShutdownFinished {
    InitializationInProgress,
    AssignmentReplacementInProgress,
    ShutdownInProgress,
    AlreadyShutDown,
    HookRuntimeMissing,
    Success,
    MinHookDisableAllFailed,
    BackBufferRestoreTimedOut,
}

impl ShutdownFinished {
    #[cfg(debug_assertions)]
    const fn outcome(self) -> &'static str {
        match self {
            Self::InitializationInProgress => "initialization_in_progress",
            Self::AssignmentReplacementInProgress => "assignment_replacement_in_progress",
            Self::ShutdownInProgress => "shutdown_in_progress",
            Self::AlreadyShutDown => "already_shutdown",
            Self::HookRuntimeMissing => "hook_runtime_missing",
            Self::Success => "success",
            Self::MinHookDisableAllFailed => "minhook_disable_all_failed",
            Self::BackBufferRestoreTimedOut => "back_buffer_restore_timed_out",
        }
    }

    #[cfg(debug_assertions)]
    const fn status(self) -> ShutdownStatus {
        match self {
            Self::InitializationInProgress
            | Self::AssignmentReplacementInProgress
            | Self::ShutdownInProgress => ShutdownStatus::AlreadyInProgress,
            Self::AlreadyShutDown => ShutdownStatus::AlreadyShutDown,
            Self::HookRuntimeMissing | Self::Success => ShutdownStatus::Success,
            Self::MinHookDisableAllFailed => ShutdownStatus::MinHookDisableAllFailed,
            Self::BackBufferRestoreTimedOut => ShutdownStatus::BackBufferRestoreTimedOut,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HooksPhase {
    Created,
    Enabled,
    Reenabled,
}

impl HooksPhase {
    #[cfg(debug_assertions)]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Enabled => "enabled",
            Self::Reenabled => "reenabled",
        }
    }
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
            status.to_code(),
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

pub(crate) fn shutdown_finished(finished: ShutdownFinished) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=shutdown_finished outcome={} status={}",
            finished.outcome(),
            finished.status() as u32,
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = finished;
}

pub(crate) fn shutdown_restore_started(pending_back_buffer_count: usize) {
    #[cfg(debug_assertions)]
    super::write(format_args!(
        "event=shutdown_restore_started pending_back_buffers={pending_back_buffer_count}"
    ));
    #[cfg(not(debug_assertions))]
    let _ = pending_back_buffer_count;
}

pub(crate) fn shutdown_active_monitors_query_failed(error: u32) {
    #[cfg(debug_assertions)]
    super::write(format_args!(
        "event=shutdown_active_monitors_query_failed error={error}"
    ));
    #[cfg(not(debug_assertions))]
    let _ = error;
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn shutdown_back_buffer_restored(
    monitor: MonitorIdentity,
    back_buffer: usize,
    remaining_back_buffer_count: usize,
) {
    #[cfg(debug_assertions)]
    super::write(format_args!(
        "event=shutdown_back_buffer_restored monitor={}:{} back_buffer=0x{back_buffer:x} remaining_back_buffers={remaining_back_buffer_count}",
        monitor.adapter_luid, monitor.target_id
    ));
    #[cfg(not(debug_assertions))]
    let _ = (monitor, back_buffer, remaining_back_buffer_count);
}

pub(crate) fn shutdown_restore_completed() {
    #[cfg(debug_assertions)]
    super::write(format_args!("event=shutdown_restore_completed"));
}

pub(crate) fn shutdown_restore_timed_out(pending_back_buffer_count: usize) {
    #[cfg(debug_assertions)]
    super::write(format_args!(
        "event=shutdown_restore_timed_out pending_back_buffers={pending_back_buffer_count}"
    ));
    #[cfg(not(debug_assertions))]
    let _ = pending_back_buffer_count;
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

pub(crate) fn hooks<'a>(phase: HooksPhase, hooks: impl IntoIterator<Item = &'a RegisteredHook>) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!("event=hooks phase={}", phase.as_str()));
        for hook in hooks {
            super::write(format_args!(
                "event=hook target={}",
                super::quoted(hook.target.label())
            ));
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = phase;
        for _ in hooks {}
    }
}

pub(crate) fn signatures(report: &SignatureResolutionReport) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=signatures module={} module_size=0x{:x}",
            super::quoted(report.module.module_name),
            report.module.size,
        ));
        for target in &report.function_targets {
            super::write(format_args!(
                "event=signature_resolved target={}",
                super::quoted(target.target().label())
            ));
        }
        for skipped in &report.skipped {
            super::write(format_args!(
                "event=signature_skipped target={} reason={:?}",
                super::quoted(skipped.target.label()),
                skipped.reason
            ));
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = report;
}
