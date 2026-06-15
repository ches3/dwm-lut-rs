use std::fmt;
use std::io;
use std::path::PathBuf;

pub(crate) use dwm_lut_payload::{InitializeStatus, ReplaceAssignmentsStatus, ShutdownStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitializeContext {
    FreshInstall,
    AfterShutdown,
    AfterReplaceFallback,
}

pub(crate) fn format_hook_initialize_failure(
    context: InitializeContext,
    status: InitializeStatus,
) -> String {
    match context {
        InitializeContext::FreshInstall => format!("hook initialize failed: {status}"),
        InitializeContext::AfterShutdown => {
            format!("existing hook was shut down, but initialize failed: {status}")
        }
        InitializeContext::AfterReplaceFallback => format!(
            "replace assignments was unavailable, existing hook was shut down, but initialize failed: {status}"
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InjectionStep {
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
    ResolveConfigPath,
    AllocatePayloadBytes,
    WritePayloadBytes,
    AllocatePayloadBuffer,
    WritePayloadBuffer,
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
            Self::ResolveConfigPath => write!(f, "local config path validation"),
            Self::AllocatePayloadBytes => write!(f, "remote payload bytes allocation"),
            Self::WritePayloadBytes => write!(f, "remote payload bytes write"),
            Self::AllocatePayloadBuffer => write!(f, "remote payload buffer allocation"),
            Self::WritePayloadBuffer => write!(f, "remote payload buffer write"),
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
pub(crate) enum InjectorError {
    Usage(String),
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
    UnknownShutdownStatus(u32),
    MonitorEnumeration(String),
}

impl From<crate::config::ConfigError> for InjectorError {
    fn from(value: crate::config::ConfigError) -> Self {
        Self::Config(value)
    }
}

impl From<dwm_lut_payload::PayloadError> for InjectorError {
    fn from(value: dwm_lut_payload::PayloadError) -> Self {
        Self::Payload(value)
    }
}

impl fmt::Display for InjectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
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
            Self::UnknownShutdownStatus(code) => {
                write!(f, "hook shutdown returned unknown status {code:#x}")
            }
            Self::MonitorEnumeration(message) => write!(f, "monitor enumeration failed: {message}"),
        }
    }
}

impl std::error::Error for InjectorError {}
