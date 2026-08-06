#![cfg_attr(not(debug_assertions), allow(dead_code))]

use dwm_lut_payload::{InitializeStatus, ReplaceAssignmentsStatus, ShutdownStatus};

use crate::minhook::{MinHookCleanupFailure, RegisteredHook};
use crate::resolver::SignatureResolutionReport;
use dwm_lut_profile::DwmcoreVersion;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShutdownFinished {
    NotInitialized,
    InitializationInProgress,
    AssignmentReplacementInProgress,
    ShutdownInProgress,
    AlreadyShutDown,
    HookRuntimeMissing,
    Success,
    MinHookCleanupFailed,
}

impl ShutdownFinished {
    #[cfg(debug_assertions)]
    const fn outcome(self) -> &'static str {
        match self {
            Self::NotInitialized => "not_initialized",
            Self::InitializationInProgress => "initialization_in_progress",
            Self::AssignmentReplacementInProgress => "assignment_replacement_in_progress",
            Self::ShutdownInProgress => "shutdown_in_progress",
            Self::AlreadyShutDown => "already_shutdown",
            Self::HookRuntimeMissing => "hook_runtime_missing",
            Self::Success => "success",
            Self::MinHookCleanupFailed => "minhook_cleanup_failed",
        }
    }

    #[cfg(debug_assertions)]
    const fn status(self) -> ShutdownStatus {
        match self {
            Self::NotInitialized => ShutdownStatus::NotInitialized,
            Self::InitializationInProgress
            | Self::AssignmentReplacementInProgress
            | Self::ShutdownInProgress => ShutdownStatus::AlreadyInProgress,
            Self::AlreadyShutDown => ShutdownStatus::AlreadyShutDown,
            Self::HookRuntimeMissing | Self::Success => ShutdownStatus::Success,
            Self::MinHookCleanupFailed => ShutdownStatus::MinHookCleanupFailed,
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

pub(crate) fn hooks(phase: HooksPhase, hooks: &[RegisteredHook]) {
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
    let _ = (phase, hooks);
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
