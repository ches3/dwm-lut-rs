use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookInitializeStatus {
    Success = 0,
    NullManifestPath = 1,
    InvalidManifestPath = 2,
    AlreadyInitialized = 3,
}

impl HookInitializeStatus {
    pub(crate) fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Success),
            1 => Some(Self::NullManifestPath),
            2 => Some(Self::InvalidManifestPath),
            3 => Some(Self::AlreadyInitialized),
            _ => None,
        }
    }
}

impl fmt::Display for HookInitializeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::NullManifestPath => write!(f, "manifest path pointer was null"),
            Self::InvalidManifestPath => write!(f, "manifest path was empty"),
            Self::AlreadyInitialized => write!(f, "hook DLL is already initialized"),
        }
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
    ResolveInitializeExport,
    ResolveManifestPath,
    AllocateManifestPath,
    WriteManifestPath,
    StartInitialize,
    WaitInitialize,
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
            Self::ResolveInitializeExport => write!(f, "dwm_lut_initialize export resolution"),
            Self::ResolveManifestPath => write!(f, "local manifest path validation"),
            Self::AllocateManifestPath => write!(f, "remote manifest path allocation"),
            Self::WriteManifestPath => write!(f, "remote manifest path write"),
            Self::StartInitialize => write!(f, "remote initialize launch"),
            Self::WaitInitialize => write!(f, "remote initialize wait"),
        }
    }
}

#[derive(Debug)]
pub(crate) enum InjectorError {
    Usage(String),
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
    HookInitializeFailed(HookInitializeStatus),
    UnknownHookInitializeStatus(u32),
}

impl fmt::Display for InjectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
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
            Self::HookInitializeFailed(status) => {
                write!(f, "hook initialize failed: {status}")
            }
            Self::UnknownHookInitializeStatus(code) => {
                write!(f, "hook initialize returned unknown status {code:#x}")
            }
        }
    }
}

impl std::error::Error for InjectorError {}
