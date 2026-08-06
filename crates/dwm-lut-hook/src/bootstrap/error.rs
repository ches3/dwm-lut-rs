use std::fmt;

use dwm_lut_payload::{
    HookTargetId, InitializeStatus, PayloadError, ReplaceAssignmentsStatus, ResolveFailureKind,
};
use dwm_lut_profile::{HookTarget, ProfileSelectError, SignatureScanError};

use crate::dwmcore::DwmcoreVersionError;
use crate::minhook::MinHookError;
use crate::resolver::HookResolveError;
use crate::state::HookStateError;

#[derive(Debug)]
pub enum HookError {
    AlreadyInitialized,
    ProfileSelect(ProfileSelectError),
    DwmcoreVersion(DwmcoreVersionError),
    Payload(PayloadError),
    MinHook(MinHookError),
    Resolve(HookResolveError),
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => write!(f, "hook is already initialized"),
            Self::ProfileSelect(error) => write!(f, "{error}"),
            Self::DwmcoreVersion(error) => write!(f, "{error}"),
            Self::Payload(error) => write!(f, "{error}"),
            Self::MinHook(error) => write!(f, "{error}"),
            Self::Resolve(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HookError {}

impl From<HookResolveError> for HookError {
    fn from(value: HookResolveError) -> Self {
        Self::Resolve(value)
    }
}

impl From<PayloadError> for HookError {
    fn from(value: PayloadError) -> Self {
        Self::Payload(value)
    }
}

impl From<MinHookError> for HookError {
    fn from(value: MinHookError) -> Self {
        Self::MinHook(value)
    }
}

impl From<ProfileSelectError> for HookError {
    fn from(value: ProfileSelectError) -> Self {
        Self::ProfileSelect(value)
    }
}

impl From<DwmcoreVersionError> for HookError {
    fn from(value: DwmcoreVersionError) -> Self {
        Self::DwmcoreVersion(value)
    }
}

#[derive(Debug)]
pub enum ReplaceAssignmentsError {
    NotInitialized,
    AlreadyInProgress,
    Payload(PayloadError),
    State(HookStateError),
    MinHook(MinHookError),
    MinHookCleanupFailed,
}

impl fmt::Display for ReplaceAssignmentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "hook is not initialized"),
            Self::AlreadyInProgress => write!(
                f,
                "hook initialization, assignment replacement, or shutdown is in progress"
            ),
            Self::Payload(error) => write!(f, "{error}"),
            Self::State(HookStateError::NotInitialized) => {
                write!(f, "hook is not initialized")
            }
            Self::MinHook(error) => write!(f, "{error}"),
            Self::MinHookCleanupFailed => write!(
                f,
                "flip-gate hooks could not be restored and were force-disabled"
            ),
        }
    }
}

impl std::error::Error for ReplaceAssignmentsError {}

impl From<PayloadError> for ReplaceAssignmentsError {
    fn from(value: PayloadError) -> Self {
        Self::Payload(value)
    }
}

impl From<HookStateError> for ReplaceAssignmentsError {
    fn from(value: HookStateError) -> Self {
        Self::State(value)
    }
}

impl From<MinHookError> for ReplaceAssignmentsError {
    fn from(value: MinHookError) -> Self {
        Self::MinHook(value)
    }
}

impl From<HookResolveError> for InitializeStatus {
    fn from(error: HookResolveError) -> Self {
        match error {
            HookResolveError::ModuleNotLoaded { .. } => Self::DwmcoreModuleNotLoaded,
            HookResolveError::InvalidModuleImage { .. } => Self::DwmcoreImageInvalid,
            HookResolveError::ModuleAccessFailed { .. } => Self::DwmcoreImageAccessFailed,
            HookResolveError::ModuleImageMismatch { .. } => Self::DwmcoreImageMismatch,
            HookResolveError::ConflictingPrologue { target, .. } => Self::Resolve {
                kind: ResolveFailureKind::PrologueConflict,
                target: to_hook_target_id(target),
            },
            HookResolveError::Scan(SignatureScanError::NotFound { target }) => Self::Resolve {
                kind: ResolveFailureKind::NotFound,
                target: to_hook_target_id(target),
            },
            HookResolveError::Scan(SignatureScanError::Ambiguous { target }) => Self::Resolve {
                kind: ResolveFailureKind::Ambiguous,
                target: to_hook_target_id(target),
            },
        }
    }
}

const fn to_hook_target_id(target: HookTarget) -> HookTargetId {
    match target {
        HookTarget::Present => HookTargetId::Present,
        HookTarget::IsCandidateDirectFlipCompatible => {
            HookTargetId::IsCandidateDirectFlipCompatible
        }
        HookTarget::IsCandidateOverlayCompatible => HookTargetId::IsCandidateOverlayCompatible,
    }
}

impl From<HookError> for InitializeStatus {
    fn from(error: HookError) -> Self {
        match error {
            HookError::AlreadyInitialized => Self::AlreadyInitialized,
            HookError::ProfileSelect(ProfileSelectError::UnsupportedDwmcoreVersion { .. }) => {
                Self::UnsupportedDwmcoreVersion
            }
            HookError::DwmcoreVersion(DwmcoreVersionError::ModuleNotLoaded) => {
                Self::DwmcoreModuleNotLoaded
            }
            HookError::DwmcoreVersion(DwmcoreVersionError::QueryFailed) => {
                Self::DwmcoreVersionQueryFailed
            }
            HookError::Resolve(error) => error.into(),
            HookError::Payload(error) => InitializeStatus::from(&error),
            HookError::MinHook(error) => match error.operation {
                crate::minhook::MinHookOperation::Initialize => Self::MinHookInitializeFailed,
                crate::minhook::MinHookOperation::CreateHook(_) => Self::MinHookCreateHookFailed,
                crate::minhook::MinHookOperation::EnableHook(_) => Self::MinHookEnableHookFailed,
                crate::minhook::MinHookOperation::DisableHook(_) => Self::MinHookDisableHookFailed,
            },
        }
    }
}

impl From<&ReplaceAssignmentsError> for ReplaceAssignmentsStatus {
    fn from(error: &ReplaceAssignmentsError) -> Self {
        match error {
            ReplaceAssignmentsError::NotInitialized
            | ReplaceAssignmentsError::State(HookStateError::NotInitialized) => {
                Self::NotInitialized
            }
            ReplaceAssignmentsError::AlreadyInProgress => Self::AlreadyInProgress,
            ReplaceAssignmentsError::Payload(error) => Self::from(error),
            ReplaceAssignmentsError::MinHook(_) => Self::MinHookFailed,
            ReplaceAssignmentsError::MinHookCleanupFailed => Self::MinHookCleanupFailed,
        }
    }
}
