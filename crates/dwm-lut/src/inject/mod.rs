mod injector;
mod staging;
mod status;
mod win32;

use std::fmt;
use std::io;
use std::path::PathBuf;

pub use dwm_lut_payload::{HookStatus, InitializeStatus, ReplaceAssignmentsStatus, ShutdownStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitializeContext {
    FreshInstall,
    AfterShutdown,
    AfterReplaceRecovery,
}

pub fn format_hook_initialize_failure(
    context: InitializeContext,
    status: InitializeStatus,
) -> String {
    match context {
        InitializeContext::FreshInstall => format!("hook initialize failed: {status}"),
        InitializeContext::AfterShutdown => {
            format!("existing hook was shut down, but initialize failed: {status}")
        }
        InitializeContext::AfterReplaceRecovery => {
            format!("initialize after replace-assignment recovery failed: {status}")
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionStep {
    FindDwmProcess,
    ResolveCurrentSession,
    EnableDebugPrivilege,
    OpenTargetProcess,
    ResolveKernel32,
    ResolveGetModuleHandleW,
    ResolveLoadLibraryW,
    AllocateDllPath,
    WriteDllPath,
    AllocateDllLoadContext,
    WriteDllLoadContext,
    AllocateDllLoadStub,
    WriteDllLoadStub,
    StartDllLoad,
    WaitDllLoad,
    ReadDllLoadResult,
    ResolveLocalHookDll,
    ResolveDefaultHookDll,
    ResolveStagingDirectory,
    CreateStagingDirectory,
    SecureStagingDirectory,
    ReadLocalHookDll,
    WriteStagedHookDll,
    VerifyStagedHookDll,
    SecureStagedHookDll,
    ResolveInitializeExport,
    ResolveShutdownExport,
    ResolveStatusExport,
    ResolveConfigPath,
    AllocatePayloadBytes,
    WritePayloadBytes,
    AllocatePayloadBuffer,
    WritePayloadBuffer,
    ReadStatusSnapshot,
    StartInitialize,
    WaitInitialize,
    StartShutdown,
    WaitShutdown,
    ResolveReplaceAssignmentsExport,
    StartReplaceAssignments,
    WaitReplaceAssignments,
}

impl fmt::Display for InjectionStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FindDwmProcess => write!(f, "dwm.exe PID lookup"),
            Self::ResolveCurrentSession => write!(f, "current session lookup"),
            Self::EnableDebugPrivilege => write!(f, "SeDebugPrivilege enable"),
            Self::OpenTargetProcess => write!(f, "target process open"),
            Self::ResolveKernel32 => write!(f, "kernel32.dll resolution"),
            Self::ResolveGetModuleHandleW => write!(f, "GetModuleHandleW resolution"),
            Self::ResolveLoadLibraryW => write!(f, "LoadLibraryW resolution"),
            Self::AllocateDllPath => write!(f, "remote DLL path allocation"),
            Self::WriteDllPath => write!(f, "remote DLL path write"),
            Self::AllocateDllLoadContext => write!(f, "remote DLL load context allocation"),
            Self::WriteDllLoadContext => write!(f, "remote DLL load context write"),
            Self::AllocateDllLoadStub => write!(f, "remote DLL load stub allocation"),
            Self::WriteDllLoadStub => write!(f, "remote DLL load stub write"),
            Self::StartDllLoad => write!(f, "remote LoadLibraryW launch"),
            Self::WaitDllLoad => write!(f, "remote LoadLibraryW wait"),
            Self::ReadDllLoadResult => write!(f, "remote DLL load result read"),
            Self::ResolveLocalHookDll => write!(f, "local hook DLL load"),
            Self::ResolveDefaultHookDll => write!(f, "default hook DLL path resolution"),
            Self::ResolveStagingDirectory => write!(f, "hook staging directory resolution"),
            Self::CreateStagingDirectory => write!(f, "hook staging directory creation"),
            Self::SecureStagingDirectory => write!(f, "hook staging directory ACL update"),
            Self::ReadLocalHookDll => write!(f, "local hook DLL read"),
            Self::WriteStagedHookDll => write!(f, "staged hook DLL write"),
            Self::VerifyStagedHookDll => write!(f, "staged hook DLL verification"),
            Self::SecureStagedHookDll => write!(f, "staged hook DLL ACL update"),
            Self::ResolveInitializeExport => write!(f, "dwm_lut_initialize export resolution"),
            Self::ResolveShutdownExport => write!(f, "dwm_lut_shutdown export resolution"),
            Self::ResolveStatusExport => write!(f, "dwm_lut_status export resolution"),
            Self::ResolveConfigPath => write!(f, "local config path validation"),
            Self::AllocatePayloadBytes => write!(f, "remote payload bytes allocation"),
            Self::WritePayloadBytes => write!(f, "remote payload bytes write"),
            Self::AllocatePayloadBuffer => write!(f, "remote payload buffer allocation"),
            Self::WritePayloadBuffer => write!(f, "remote payload buffer write"),
            Self::ReadStatusSnapshot => write!(f, "remote hook status snapshot read"),
            Self::StartInitialize => write!(f, "remote initialize launch"),
            Self::WaitInitialize => write!(f, "remote initialize wait"),
            Self::StartShutdown => write!(f, "remote shutdown launch"),
            Self::WaitShutdown => write!(f, "remote shutdown wait"),
            Self::ResolveReplaceAssignmentsExport => {
                write!(f, "dwm_lut_replace_assignments export resolution")
            }
            Self::StartReplaceAssignments => write!(f, "remote replace assignments launch"),
            Self::WaitReplaceAssignments => write!(f, "remote replace assignments wait"),
        }
    }
}

#[derive(Debug)]
pub enum InjectError {
    Config(crate::config::ConfigError),
    Payload(dwm_lut_payload::PayloadError),
    DebugPrivilegeUnavailable,
    MissingFile {
        kind: &'static str,
        path: PathBuf,
    },
    StepFailed {
        step: InjectionStep,
        source: io::Error,
    },
    DwmProcessNotFound,
    TargetAccessDenied {
        pid: u32,
    },
    RemoteCallFailed {
        step: InjectionStep,
        exit_code: u32,
    },
    RemoteModuleNotFound {
        module: String,
    },
    ExportNotFound {
        export: String,
        dll_path: PathBuf,
    },
    HookInitializeFailed {
        status: InitializeStatus,
        context: InitializeContext,
    },
    UnknownInitializeStatus(u32),
    HookReplaceAssignmentsFailed(ReplaceAssignmentsStatus),
    UnknownReplaceAssignmentsStatus(u32),
    HookShutdownFailed(ShutdownStatus),
    HookShutdownModulesFailed {
        failures: Vec<(PathBuf, InjectError)>,
    },
    UnknownShutdownStatus(u32),
    HookStatusModulesFailed {
        failures: Vec<(PathBuf, InjectError)>,
    },
    InvalidHookStatusSnapshot(String),
    HookStatusProfileMismatch {
        first: String,
        second: String,
    },
}

impl From<crate::config::ConfigError> for InjectError {
    fn from(value: crate::config::ConfigError) -> Self {
        Self::Config(value)
    }
}

impl From<dwm_lut_payload::PayloadError> for InjectError {
    fn from(value: dwm_lut_payload::PayloadError) -> Self {
        Self::Payload(value)
    }
}

impl fmt::Display for InjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(f, "config load failed: {error}"),
            Self::Payload(error) => write!(f, "payload build failed: {error}"),
            Self::DebugPrivilegeUnavailable => {
                write!(
                    f,
                    "SeDebugPrivilege is unavailable; run the injector elevated"
                )
            }
            Self::MissingFile { kind, path } => {
                write!(f, "{kind} was not found: {}", path.display())
            }
            Self::StepFailed { step, source } => write!(f, "{step} failed: {source}"),
            Self::DwmProcessNotFound => write!(f, "dwm.exe was not found"),
            Self::TargetAccessDenied { pid } => {
                write!(
                    f,
                    "access denied while opening dwm.exe (pid={pid}); run the injector elevated"
                )
            }
            Self::RemoteCallFailed { step, exit_code } => {
                write!(f, "{step} returned failure exit code {exit_code:#x}")
            }
            Self::RemoteModuleNotFound { module } => {
                write!(f, "remote module was not found after injection: {module}")
            }
            Self::ExportNotFound { export, dll_path } => {
                write!(f, "export {export} was not found in {}", dll_path.display())
            }
            Self::HookInitializeFailed { status, context } => {
                write!(f, "{}", format_hook_initialize_failure(*context, *status))
            }
            Self::UnknownInitializeStatus(code) => {
                write!(f, "hook initialize returned unknown status {code:#x}")
            }
            Self::HookReplaceAssignmentsFailed(status) => {
                write!(
                    f,
                    "replace assignments failed: {status} (existing hook unchanged)"
                )
            }
            Self::UnknownReplaceAssignmentsStatus(code) => {
                write!(f, "replace assignments returned unknown status {code:#x}")
            }
            Self::HookShutdownFailed(status) => write!(f, "hook shutdown failed: {status}"),
            Self::HookShutdownModulesFailed { failures } => {
                write!(
                    f,
                    "hook shutdown failed for {} staged module(s)",
                    failures.len()
                )?;
                for (module_path, error) in failures {
                    write!(f, "; {}: {error}", module_path.display())?;
                }
                Ok(())
            }
            Self::UnknownShutdownStatus(code) => {
                write!(f, "hook shutdown returned unknown status {code:#x}")
            }
            Self::HookStatusModulesFailed { failures } => {
                write!(
                    f,
                    "hook status query failed for {} staged module(s)",
                    failures.len()
                )?;
                for (module_path, error) in failures {
                    write!(f, "; {}: {error}", module_path.display())?;
                }
                Ok(())
            }
            Self::InvalidHookStatusSnapshot(message) => {
                write!(f, "hook status query returned invalid data: {message}")
            }
            Self::HookStatusProfileMismatch { first, second } => write!(
                f,
                "active hook modules reported different profiles: {first:?} and {second:?}"
            ),
        }
    }
}

impl std::error::Error for InjectError {}

use crate::config;

pub(crate) use dwm_lut_payload::HookStatusSnapshot;
pub(crate) use injector::{ApplyOutcome, DisableOutcome};

pub(crate) struct ApplyRequest {
    pub(crate) dll_path: Option<PathBuf>,
    pub(crate) config_path: PathBuf,
    pub(crate) profile: Option<String>,
}

pub(crate) struct ApplyReport {
    pub(crate) outcome: ApplyOutcome,
    pub(crate) pid: u32,
    pub(crate) input_dll_path: PathBuf,
    pub(crate) staged_dll_path: PathBuf,
    pub(crate) config_path: PathBuf,
    pub(crate) profile_name: String,
}

pub(crate) struct DisableReport {
    pub(crate) outcome: DisableOutcome,
    pub(crate) pid: u32,
}

pub fn ensure_host_privileges() -> Result<(), InjectError> {
    win32::enable_debug_privilege()
}

pub(crate) fn apply(request: ApplyRequest) -> Result<ApplyReport, InjectError> {
    let input_dll_path = request
        .dll_path
        .unwrap_or(staging::default_hook_dll_path()?);
    let input_dll_path = injector::canonicalize_existing_file(
        &input_dll_path,
        InjectionStep::ResolveLocalHookDll,
        "hook DLL",
    )?;
    let config_path = injector::canonicalize_existing_file(
        &request.config_path,
        InjectionStep::ResolveConfigPath,
        "config file",
    )?;
    let loaded = config::load_payload(&config_path, request.profile.as_deref())?;
    let payload_bytes = dwm_lut_payload::serialize_payload(&loaded.payload)?;
    let staged_dll_path = staging::stage_hook_dll(&input_dll_path)?;
    let pid = win32::find_process_id_by_name("dwm.exe")?;

    win32::enable_debug_privilege()?;
    let outcome = injector::apply_config(pid, &staged_dll_path, &payload_bytes)?;

    Ok(ApplyReport {
        outcome,
        pid,
        input_dll_path,
        staged_dll_path,
        config_path,
        profile_name: loaded.profile_name,
    })
}

pub(crate) fn disable() -> Result<DisableReport, InjectError> {
    let pid = win32::find_process_id_by_name("dwm.exe")?;

    win32::enable_debug_privilege()?;
    let outcome = injector::disable_injected_hook(pid)?;

    Ok(DisableReport { outcome, pid })
}

pub(crate) fn query_status() -> Result<HookStatusSnapshot, InjectError> {
    let pid = win32::find_process_id_by_name("dwm.exe")?;

    win32::enable_debug_privilege()?;
    status::query_hook_status(pid)
}
