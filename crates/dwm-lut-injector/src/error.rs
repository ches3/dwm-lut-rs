use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookInitializeStatus {
    Success = 0,
    NullManifestPath = 1,
    InvalidManifestPath = 2,
    AlreadyInitialized = 3,
    DwmcoreModuleNotLoaded = 4,
    DwmcoreImageInvalid = 5,
    PresentSignatureNotFound = 6,
    PresentSignatureAmbiguous = 7,
    DirectFlipSignatureNotFound = 8,
    DirectFlipSignatureAmbiguous = 9,
    OverlaysEnabledSignatureNotFound = 10,
    OverlaysEnabledSignatureAmbiguous = 11,
    ManifestLoadFailed = 12,
    ManifestHasNoAssignments = 13,
    LutPipelinePrepareFailed = 14,
    WindowDirectFlipSignatureNotFound = 15,
    WindowDirectFlipSignatureAmbiguous = 16,
    CompSwapChainDirectFlipSignatureNotFound = 17,
    CompSwapChainDirectFlipSignatureAmbiguous = 18,
    CompVisualPromotionSignatureNotFound = 19,
    CompVisualPromotionSignatureAmbiguous = 20,
    OverlayTestModeNotFound = 21,
    OverlayTestModeAmbiguous = 22,
    CompSwapChainIndependentFlipSignatureNotFound = 23,
    CompSwapChainIndependentFlipSignatureAmbiguous = 24,
    MinHookLoadFailed = 25,
    MinHookGetProcAddressFailed = 26,
    MinHookInitializeFailed = 27,
    MinHookCreateHookFailed = 28,
    MinHookEnableHookFailed = 29,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitializeContext {
    FreshInstall,
    AfterShutdown,
    AfterReloadFallback,
}

pub(crate) fn format_hook_initialize_failure(
    context: InitializeContext,
    status: HookInitializeStatus,
) -> String {
    match context {
        InitializeContext::FreshInstall => format!("hook initialize failed: {status}"),
        InitializeContext::AfterShutdown => {
            format!("existing hook was shut down, but initialize failed: {status}")
        }
        InitializeContext::AfterReloadFallback => format!(
            "manifest reload was unavailable, existing hook was shut down, but initialize failed: {status}"
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookShutdownStatus {
    Success = 0,
    NotInitialized = 1,
    AlreadyInProgress = 2,
    AlreadyShutDown = 3,
    MinHookCleanupFailed = 4,
}

impl HookShutdownStatus {
    pub(crate) fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Success),
            1 => Some(Self::NotInitialized),
            2 => Some(Self::AlreadyInProgress),
            3 => Some(Self::AlreadyShutDown),
            4 => Some(Self::MinHookCleanupFailed),
            _ => None,
        }
    }
}

impl fmt::Display for HookShutdownStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::NotInitialized => write!(f, "hook DLL is loaded but not initialized"),
            Self::AlreadyInProgress => write!(f, "hook shutdown is already in progress"),
            Self::AlreadyShutDown => write!(f, "hook DLL is already shut down"),
            Self::MinHookCleanupFailed => write!(f, "MinHook cleanup failed"),
        }
    }
}

impl HookInitializeStatus {
    pub(crate) fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Success),
            1 => Some(Self::NullManifestPath),
            2 => Some(Self::InvalidManifestPath),
            3 => Some(Self::AlreadyInitialized),
            4 => Some(Self::DwmcoreModuleNotLoaded),
            5 => Some(Self::DwmcoreImageInvalid),
            6 => Some(Self::PresentSignatureNotFound),
            7 => Some(Self::PresentSignatureAmbiguous),
            8 => Some(Self::DirectFlipSignatureNotFound),
            9 => Some(Self::DirectFlipSignatureAmbiguous),
            10 => Some(Self::OverlaysEnabledSignatureNotFound),
            11 => Some(Self::OverlaysEnabledSignatureAmbiguous),
            12 => Some(Self::ManifestLoadFailed),
            13 => Some(Self::ManifestHasNoAssignments),
            14 => Some(Self::LutPipelinePrepareFailed),
            15 => Some(Self::WindowDirectFlipSignatureNotFound),
            16 => Some(Self::WindowDirectFlipSignatureAmbiguous),
            17 => Some(Self::CompSwapChainDirectFlipSignatureNotFound),
            18 => Some(Self::CompSwapChainDirectFlipSignatureAmbiguous),
            19 => Some(Self::CompVisualPromotionSignatureNotFound),
            20 => Some(Self::CompVisualPromotionSignatureAmbiguous),
            21 => Some(Self::OverlayTestModeNotFound),
            22 => Some(Self::OverlayTestModeAmbiguous),
            23 => Some(Self::CompSwapChainIndependentFlipSignatureNotFound),
            24 => Some(Self::CompSwapChainIndependentFlipSignatureAmbiguous),
            25 => Some(Self::MinHookLoadFailed),
            26 => Some(Self::MinHookGetProcAddressFailed),
            27 => Some(Self::MinHookInitializeFailed),
            28 => Some(Self::MinHookCreateHookFailed),
            29 => Some(Self::MinHookEnableHookFailed),
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
            Self::DwmcoreModuleNotLoaded => write!(f, "dwmcore.dll was not loaded in the target"),
            Self::DwmcoreImageInvalid => write!(f, "dwmcore.dll was not a valid PE image"),
            Self::PresentSignatureNotFound => write!(f, "Present signature was not found"),
            Self::PresentSignatureAmbiguous => {
                write!(f, "Present signature matched multiple locations")
            }
            Self::DirectFlipSignatureNotFound => {
                write!(f, "IsCandidateDirectFlipCompatible signature was not found")
            }
            Self::DirectFlipSignatureAmbiguous => {
                write!(
                    f,
                    "IsCandidateDirectFlipCompatible signature matched multiple locations"
                )
            }
            Self::OverlaysEnabledSignatureNotFound => {
                write!(f, "OverlaysEnabled signature was not found")
            }
            Self::OverlaysEnabledSignatureAmbiguous => {
                write!(f, "OverlaysEnabled signature matched multiple locations")
            }
            Self::ManifestLoadFailed => write!(f, "manifest could not be loaded"),
            Self::ManifestHasNoAssignments => {
                write!(f, "manifest does not contain any LUT assignments")
            }
            Self::LutPipelinePrepareFailed => {
                write!(f, "LUT pipeline resources could not be prepared")
            }
            Self::WindowDirectFlipSignatureNotFound => write!(
                f,
                "CWindowContext::IsCandidateDirectFlipCompatible signature was not found"
            ),
            Self::WindowDirectFlipSignatureAmbiguous => write!(
                f,
                "CWindowContext::IsCandidateDirectFlipCompatible signature matched multiple locations"
            ),
            Self::CompSwapChainDirectFlipSignatureNotFound => write!(
                f,
                "CCompSwapChain::IsCandidateDirectFlipCompatible signature was not found"
            ),
            Self::CompSwapChainDirectFlipSignatureAmbiguous => write!(
                f,
                "CCompSwapChain::IsCandidateDirectFlipCompatible signature matched multiple locations"
            ),
            Self::CompVisualPromotionSignatureNotFound => {
                write!(
                    f,
                    "CCompVisual::IsCandidateForPromotion signature was not found"
                )
            }
            Self::CompVisualPromotionSignatureAmbiguous => write!(
                f,
                "CCompVisual::IsCandidateForPromotion signature matched multiple locations"
            ),
            Self::OverlayTestModeNotFound => write!(f, "OverlayTestMode reference was not found"),
            Self::OverlayTestModeAmbiguous => {
                write!(f, "OverlayTestMode reference matched multiple locations")
            }
            Self::CompSwapChainIndependentFlipSignatureNotFound => write!(
                f,
                "CCompSwapChain::IsCandidateIndependentFlipCompatible signature was not found"
            ),
            Self::CompSwapChainIndependentFlipSignatureAmbiguous => write!(
                f,
                "CCompSwapChain::IsCandidateIndependentFlipCompatible signature matched multiple locations"
            ),
            Self::MinHookLoadFailed => write!(f, "MinHook DLL could not be loaded"),
            Self::MinHookGetProcAddressFailed => write!(f, "MinHook exports could not be resolved"),
            Self::MinHookInitializeFailed => write!(f, "MH_Initialize failed"),
            Self::MinHookCreateHookFailed => write!(f, "MH_CreateHook failed"),
            Self::MinHookEnableHookFailed => write!(f, "MH_EnableHook failed"),
        }
    }
}

/// FFI status codes returned by `dwm_lut_apply_manifest`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyManifestStatus {
    Success = 0,
    NullManifestPath = 1,
    InvalidManifestPath = 2,
    NotInitialized = 3,
    AlreadyInProgress = 4,
    ManifestLoadFailed = 5,
    ManifestHasNoAssignments = 6,
    LutPipelinePrepareFailed = 7,
}

impl ApplyManifestStatus {
    pub(crate) fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Success),
            1 => Some(Self::NullManifestPath),
            2 => Some(Self::InvalidManifestPath),
            3 => Some(Self::NotInitialized),
            4 => Some(Self::AlreadyInProgress),
            5 => Some(Self::ManifestLoadFailed),
            6 => Some(Self::ManifestHasNoAssignments),
            7 => Some(Self::LutPipelinePrepareFailed),
            _ => None,
        }
    }

    pub(crate) fn should_fallback(self) -> bool {
        matches!(self, Self::NotInitialized)
    }
}

impl fmt::Display for ApplyManifestStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::NullManifestPath => write!(f, "manifest path pointer was null"),
            Self::InvalidManifestPath => write!(f, "manifest path was empty"),
            Self::NotInitialized => write!(f, "hook DLL is loaded but not initialized"),
            Self::AlreadyInProgress => {
                write!(f, "hook initialization or shutdown is in progress")
            }
            Self::ManifestLoadFailed => write!(f, "manifest could not be loaded"),
            Self::ManifestHasNoAssignments => {
                write!(f, "manifest does not contain any LUT assignments")
            }
            Self::LutPipelinePrepareFailed => {
                write!(f, "LUT pipeline resources could not be prepared")
            }
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
    ResolveManifestPath,
    AllocateManifestPath,
    WriteManifestPath,
    StartInitialize,
    WaitInitialize,
    StartShutdown,
    WaitShutdown,
    ResolveApplyManifestExport,
    StartApplyManifest,
    WaitApplyManifest,
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
            Self::ResolveManifestPath => write!(f, "local manifest path validation"),
            Self::AllocateManifestPath => write!(f, "remote manifest path allocation"),
            Self::WriteManifestPath => write!(f, "remote manifest path write"),
            Self::StartInitialize => write!(f, "remote initialize launch"),
            Self::WaitInitialize => write!(f, "remote initialize wait"),
            Self::StartShutdown => write!(f, "remote shutdown launch"),
            Self::WaitShutdown => write!(f, "remote shutdown wait"),
            Self::ResolveApplyManifestExport => {
                write!(f, "dwm_lut_apply_manifest export resolution")
            }
            Self::StartApplyManifest => write!(f, "remote apply manifest launch"),
            Self::WaitApplyManifest => write!(f, "remote apply manifest wait"),
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
    HookInitializeFailed {
        status: HookInitializeStatus,
        context: InitializeContext,
    },
    UnknownHookInitializeStatus(u32),
    HookApplyManifestFailed(ApplyManifestStatus),
    UnknownHookApplyManifestStatus(u32),
    HookShutdownFailed(HookShutdownStatus),
    UnknownHookShutdownStatus(u32),
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
            Self::HookInitializeFailed { status, context } => {
                write!(f, "{}", format_hook_initialize_failure(*context, *status))
            }
            Self::UnknownHookInitializeStatus(code) => {
                write!(f, "hook initialize returned unknown status {code:#x}")
            }
            Self::HookApplyManifestFailed(status) => {
                write!(
                    f,
                    "manifest reload failed: {status} (existing hook unchanged)"
                )
            }
            Self::UnknownHookApplyManifestStatus(code) => {
                write!(f, "manifest reload returned unknown status {code:#x}")
            }
            Self::HookShutdownFailed(status) => {
                write!(f, "hook shutdown failed: {status}")
            }
            Self::UnknownHookShutdownStatus(code) => {
                write!(f, "hook shutdown returned unknown status {code:#x}")
            }
        }
    }
}

impl std::error::Error for InjectorError {}

#[cfg(test)]
mod tests {
    use super::{
        ApplyManifestStatus, HookInitializeStatus, InitializeContext,
        format_hook_initialize_failure,
    };

    #[test]
    fn initialize_failure_message_includes_apply_context() {
        assert_eq!(
            format_hook_initialize_failure(
                InitializeContext::FreshInstall,
                HookInitializeStatus::PresentSignatureNotFound,
            ),
            "hook initialize failed: Present signature was not found"
        );
        assert_eq!(
            format_hook_initialize_failure(
                InitializeContext::AfterShutdown,
                HookInitializeStatus::PresentSignatureNotFound,
            ),
            "existing hook was shut down, but initialize failed: Present signature was not found"
        );
        assert_eq!(
            format_hook_initialize_failure(
                InitializeContext::AfterReloadFallback,
                HookInitializeStatus::PresentSignatureNotFound,
            ),
            "manifest reload was unavailable, existing hook was shut down, but initialize failed: Present signature was not found"
        );
    }

    #[test]
    fn apply_manifest_status_codes_are_stable() {
        assert_eq!(ApplyManifestStatus::Success as u32, 0);
        assert_eq!(ApplyManifestStatus::NullManifestPath as u32, 1);
        assert_eq!(ApplyManifestStatus::InvalidManifestPath as u32, 2);
        assert_eq!(ApplyManifestStatus::NotInitialized as u32, 3);
        assert_eq!(ApplyManifestStatus::AlreadyInProgress as u32, 4);
        assert_eq!(ApplyManifestStatus::ManifestLoadFailed as u32, 5);
        assert_eq!(ApplyManifestStatus::ManifestHasNoAssignments as u32, 6);
        assert_eq!(ApplyManifestStatus::LutPipelinePrepareFailed as u32, 7);
    }

    #[test]
    fn apply_manifest_status_from_code_roundtrips_all_variants() {
        const VARIANTS: &[(u32, ApplyManifestStatus)] = &[
            (0, ApplyManifestStatus::Success),
            (1, ApplyManifestStatus::NullManifestPath),
            (2, ApplyManifestStatus::InvalidManifestPath),
            (3, ApplyManifestStatus::NotInitialized),
            (4, ApplyManifestStatus::AlreadyInProgress),
            (5, ApplyManifestStatus::ManifestLoadFailed),
            (6, ApplyManifestStatus::ManifestHasNoAssignments),
            (7, ApplyManifestStatus::LutPipelinePrepareFailed),
        ];

        for (code, status) in VARIANTS {
            assert_eq!(ApplyManifestStatus::from_code(*code), Some(*status));
        }
        assert_eq!(ApplyManifestStatus::from_code(8), None);
    }

    #[test]
    fn apply_manifest_not_initialized_is_fallback_eligible() {
        assert!(ApplyManifestStatus::NotInitialized.should_fallback());
        assert!(!ApplyManifestStatus::ManifestLoadFailed.should_fallback());
    }
}
