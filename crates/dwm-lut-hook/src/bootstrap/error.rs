use std::fmt;

use dwm_lut_payload::{
    InitializeStatus, PayloadError, ReplaceAssignmentsStatus, ResolveFailureKind,
};

use crate::minhook::MinHookError;
use crate::profile::ProfileSelectError;
use crate::resolver::HookResolveError;
use crate::state::ReplaceLutAssignmentsError;

#[derive(Debug)]
pub enum HookError {
    AlreadyInitialized,
    ProfileSelect(ProfileSelectError),
    Payload(PayloadError),
    MinHook(MinHookError),
    Resolve(HookResolveError),
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => write!(f, "hook is already initialized"),
            Self::ProfileSelect(error) => write!(f, "{error}"),
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

#[derive(Debug)]
pub enum ReplaceAssignmentsError {
    NotInitialized,
    AlreadyInProgress,
    Payload(PayloadError),
    State(ReplaceLutAssignmentsError),
}

impl fmt::Display for ReplaceAssignmentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "hook is not initialized"),
            Self::AlreadyInProgress => write!(f, "hook initialization or shutdown is in progress"),
            Self::Payload(error) => write!(f, "{error}"),
            Self::State(ReplaceLutAssignmentsError::NotInitialized) => {
                write!(f, "hook is not initialized")
            }
        }
    }
}

impl std::error::Error for ReplaceAssignmentsError {}

impl From<PayloadError> for ReplaceAssignmentsError {
    fn from(value: PayloadError) -> Self {
        Self::Payload(value)
    }
}

impl From<ReplaceLutAssignmentsError> for ReplaceAssignmentsError {
    fn from(value: ReplaceLutAssignmentsError) -> Self {
        Self::State(value)
    }
}

impl From<HookResolveError> for InitializeStatus {
    fn from(error: HookResolveError) -> Self {
        match error {
            HookResolveError::ModuleNotLoaded { .. } => Self::DwmcoreModuleNotLoaded,
            HookResolveError::InvalidModuleImage { .. } => Self::DwmcoreImageInvalid,
            HookResolveError::ModuleAccessFailed { .. } => Self::DwmcoreImageAccessFailed,
            HookResolveError::ModuleImageMismatch { .. } => Self::DwmcoreImageMismatch,
            HookResolveError::SignatureNotFound { target } => Self::Resolve {
                kind: ResolveFailureKind::NotFound,
                target: target.into(),
            },
            HookResolveError::SignatureAmbiguous { target, .. } => Self::Resolve {
                kind: ResolveFailureKind::Ambiguous,
                target: target.into(),
            },
            HookResolveError::ConflictingPrologue { target, .. } => Self::Resolve {
                kind: ResolveFailureKind::PrologueConflict,
                target: target.into(),
            },
        }
    }
}

impl From<HookError> for InitializeStatus {
    fn from(error: HookError) -> Self {
        match error {
            HookError::AlreadyInitialized => Self::AlreadyInitialized,
            HookError::ProfileSelect(error) => match error {
                ProfileSelectError::UnsupportedDwmcoreVersion { .. } => {
                    Self::UnsupportedDwmcoreVersion
                }
                ProfileSelectError::DwmcoreModuleNotLoaded => Self::DwmcoreModuleNotLoaded,
                ProfileSelectError::DwmcoreVersionQueryFailed => Self::DwmcoreVersionQueryFailed,
            },
            HookError::Resolve(error) => error.into(),
            HookError::Payload(error) => InitializeStatus::from(&error),
            HookError::MinHook(error) => match error.operation {
                crate::minhook::MinHookOperation::Initialize => Self::MinHookInitializeFailed,
                crate::minhook::MinHookOperation::CreateHook(_) => Self::MinHookCreateHookFailed,
                crate::minhook::MinHookOperation::EnableHook => Self::MinHookEnableHookFailed,
            },
        }
    }
}

impl From<&ReplaceAssignmentsError> for ReplaceAssignmentsStatus {
    fn from(error: &ReplaceAssignmentsError) -> Self {
        match error {
            ReplaceAssignmentsError::NotInitialized
            | ReplaceAssignmentsError::State(ReplaceLutAssignmentsError::NotInitialized) => {
                Self::NotInitialized
            }
            ReplaceAssignmentsError::AlreadyInProgress => Self::AlreadyInProgress,
            ReplaceAssignmentsError::Payload(error) => Self::from(error),
        }
    }
}
