use std::fmt;

const RESOLVE_CODE_BASE: u32 = 0x0001_0000;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookStatus {
    Inactive = 0,
    Active = 1,
    Transitioning = 2,
}

impl HookStatus {
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Inactive),
            1 => Some(Self::Active),
            2 => Some(Self::Transitioning),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookStatusSnapshot {
    Inactive,
    Active { profile_name: String },
    Transitioning,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveFailureKind {
    NotFound = 1,
    Ambiguous = 2,
    PrologueConflict = 3,
}

impl ResolveFailureKind {
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::NotFound),
            2 => Some(Self::Ambiguous),
            3 => Some(Self::PrologueConflict),
            _ => None,
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTargetId {
    Present = 1,
    IsCandidateDirectFlipCompatible = 2,
    DirectFlipInfoEnsureIndependentFlipState = 3,
    IsDirectFlipSupportedOnTarget = 4,
    LegacySwapChainCheckDirectFlipSupport = 5,
    IsAdvancedDirectFlipCompatible = 6,
    OverlayTestMode = 7,
    DisableIndependentFlip = 8,
    OverlaysEnabled = 9,
}

impl HookTargetId {
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Present),
            2 => Some(Self::IsCandidateDirectFlipCompatible),
            3 => Some(Self::DirectFlipInfoEnsureIndependentFlipState),
            4 => Some(Self::IsDirectFlipSupportedOnTarget),
            5 => Some(Self::LegacySwapChainCheckDirectFlipSupport),
            6 => Some(Self::IsAdvancedDirectFlipCompatible),
            7 => Some(Self::OverlayTestMode),
            8 => Some(Self::DisableIndependentFlip),
            9 => Some(Self::OverlaysEnabled),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Present => "Present",
            Self::IsCandidateDirectFlipCompatible => "IsCandidateDirectFlipCompatible",
            Self::DirectFlipInfoEnsureIndependentFlipState => {
                "CDirectFlipInfo::EnsureIndependentFlipState"
            }
            Self::IsDirectFlipSupportedOnTarget => "COverlayContext::IsDirectFlipSupportedOnTarget",
            Self::LegacySwapChainCheckDirectFlipSupport => {
                "CLegacySwapChain::CheckDirectFlipSupport"
            }
            Self::IsAdvancedDirectFlipCompatible => {
                "CGlobalCompositionSurfaceInfo::IsAdvancedDirectFlipCompatible"
            }
            Self::OverlayTestMode => "OverlayTestMode",
            Self::DisableIndependentFlip => "DisableIndependentFlip",
            Self::OverlaysEnabled => "COverlayContext::OverlaysEnabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitializeStatus {
    Success,
    NullPayload,
    InvalidPayload,
    AlreadyInitialized,
    DwmcoreModuleNotLoaded,
    DwmcoreImageInvalid,
    DwmcoreImageAccessFailed,
    DwmcoreImageMismatch,
    DwmcoreVersionQueryFailed,
    UnsupportedDwmcoreVersion,
    PayloadDecodeFailed,
    PayloadHasNoAssignments,
    MinHookInitializeFailed,
    MinHookCreateHookFailed,
    MinHookEnableHookFailed,
    Resolve {
        kind: ResolveFailureKind,
        target: HookTargetId,
    },
}

impl InitializeStatus {
    pub const fn to_code(self) -> u32 {
        match self {
            Self::Success => 0,
            Self::NullPayload => 1,
            Self::InvalidPayload => 2,
            Self::AlreadyInitialized => 3,
            Self::DwmcoreModuleNotLoaded => 4,
            Self::DwmcoreImageInvalid => 5,
            Self::DwmcoreImageAccessFailed => 6,
            Self::DwmcoreImageMismatch => 7,
            Self::DwmcoreVersionQueryFailed => 8,
            Self::UnsupportedDwmcoreVersion => 9,
            Self::PayloadDecodeFailed => 10,
            Self::PayloadHasNoAssignments => 11,
            Self::MinHookInitializeFailed => 12,
            Self::MinHookCreateHookFailed => 13,
            Self::MinHookEnableHookFailed => 14,
            Self::Resolve { kind, target } => {
                RESOLVE_CODE_BASE | ((kind as u32) << 8) | (target as u32)
            }
        }
    }

    pub const fn from_code(code: u32) -> Option<Self> {
        if code & 0xFFFF_0000 == RESOLVE_CODE_BASE {
            let kind = match ResolveFailureKind::from_u8(((code >> 8) & 0xFF) as u8) {
                Some(kind) => kind,
                None => return None,
            };
            let target = match HookTargetId::from_u8((code & 0xFF) as u8) {
                Some(target) => target,
                None => return None,
            };
            return Some(Self::Resolve { kind, target });
        }

        match code {
            0 => Some(Self::Success),
            1 => Some(Self::NullPayload),
            2 => Some(Self::InvalidPayload),
            3 => Some(Self::AlreadyInitialized),
            4 => Some(Self::DwmcoreModuleNotLoaded),
            5 => Some(Self::DwmcoreImageInvalid),
            6 => Some(Self::DwmcoreImageAccessFailed),
            7 => Some(Self::DwmcoreImageMismatch),
            8 => Some(Self::DwmcoreVersionQueryFailed),
            9 => Some(Self::UnsupportedDwmcoreVersion),
            10 => Some(Self::PayloadDecodeFailed),
            11 => Some(Self::PayloadHasNoAssignments),
            12 => Some(Self::MinHookInitializeFailed),
            13 => Some(Self::MinHookCreateHookFailed),
            14 => Some(Self::MinHookEnableHookFailed),
            _ => None,
        }
    }
}

impl fmt::Display for InitializeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::NullPayload => write!(f, "payload buffer pointer was null"),
            Self::InvalidPayload => write!(f, "payload buffer was invalid"),
            Self::AlreadyInitialized => write!(f, "hook DLL is already initialized"),
            Self::DwmcoreModuleNotLoaded => write!(f, "dwmcore.dll was not loaded in the target"),
            Self::DwmcoreImageInvalid => write!(f, "dwmcore.dll was not a valid PE image"),
            Self::DwmcoreImageAccessFailed => {
                write!(f, "dwmcore.dll backing image could not be accessed")
            }
            Self::DwmcoreImageMismatch => {
                write!(f, "loaded dwmcore.dll does not match its backing file")
            }
            Self::DwmcoreVersionQueryFailed => {
                write!(f, "dwmcore.dll FileVersion could not be queried")
            }
            Self::UnsupportedDwmcoreVersion => {
                write!(
                    f,
                    "dwmcore.dll FileVersion is below the minimum supported hook profile"
                )
            }
            Self::PayloadDecodeFailed => write!(f, "payload could not be decoded"),
            Self::PayloadHasNoAssignments => {
                write!(f, "payload does not contain any LUT assignments")
            }
            Self::MinHookInitializeFailed => write!(f, "MH_Initialize failed"),
            Self::MinHookCreateHookFailed => write!(f, "MH_CreateHook failed"),
            Self::MinHookEnableHookFailed => write!(f, "MH_EnableHook failed"),
            Self::Resolve { kind, target } => {
                let label = target.label();
                match kind {
                    ResolveFailureKind::NotFound => write!(f, "{label} was not found"),
                    ResolveFailureKind::Ambiguous => {
                        write!(f, "{label} matched multiple locations")
                    }
                    ResolveFailureKind::PrologueConflict => {
                        write!(f, "{label} prologue is modified by a conflicting hook")
                    }
                }
            }
        }
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaceAssignmentsStatus {
    Success = 0,
    NullPayload = 1,
    InvalidPayload = 2,
    NotInitialized = 3,
    AlreadyInProgress = 4,
    PayloadDecodeFailed = 5,
    PayloadHasNoAssignments = 6,
}

impl ReplaceAssignmentsStatus {
    pub fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            0 => Self::Success,
            1 => Self::NullPayload,
            2 => Self::InvalidPayload,
            3 => Self::NotInitialized,
            4 => Self::AlreadyInProgress,
            5 => Self::PayloadDecodeFailed,
            6 => Self::PayloadHasNoAssignments,
            _ => return None,
        })
    }

    pub const fn should_fallback(self) -> bool {
        matches!(self, Self::NotInitialized)
    }
}

impl fmt::Display for ReplaceAssignmentsStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::NullPayload => write!(f, "payload buffer pointer was null"),
            Self::InvalidPayload => write!(f, "payload buffer was invalid"),
            Self::NotInitialized => write!(f, "hook DLL is loaded but not initialized"),
            Self::AlreadyInProgress => write!(
                f,
                "hook initialization, assignment replacement, or shutdown is in progress"
            ),
            Self::PayloadDecodeFailed => write!(f, "payload could not be decoded"),
            Self::PayloadHasNoAssignments => {
                write!(f, "payload does not contain any LUT assignments")
            }
        }
    }
}

impl From<crate::PayloadFailureKind> for InitializeStatus {
    fn from(kind: crate::PayloadFailureKind) -> Self {
        match kind {
            crate::PayloadFailureKind::Invalid => Self::InvalidPayload,
            crate::PayloadFailureKind::NoAssignments => Self::PayloadHasNoAssignments,
            crate::PayloadFailureKind::DecodeFailed => Self::PayloadDecodeFailed,
        }
    }
}

impl From<&crate::PayloadError> for InitializeStatus {
    fn from(error: &crate::PayloadError) -> Self {
        error.failure_kind().into()
    }
}

impl From<crate::PayloadFailureKind> for ReplaceAssignmentsStatus {
    fn from(kind: crate::PayloadFailureKind) -> Self {
        match kind {
            crate::PayloadFailureKind::Invalid => Self::InvalidPayload,
            crate::PayloadFailureKind::NoAssignments => Self::PayloadHasNoAssignments,
            crate::PayloadFailureKind::DecodeFailed => Self::PayloadDecodeFailed,
        }
    }
}

impl From<&crate::PayloadError> for ReplaceAssignmentsStatus {
    fn from(error: &crate::PayloadError) -> Self {
        error.failure_kind().into()
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownStatus {
    Success = 0,
    NotInitialized = 1,
    AlreadyInProgress = 2,
    AlreadyShutDown = 3,
    MinHookCleanupFailed = 4,
}

impl ShutdownStatus {
    pub fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            0 => Self::Success,
            1 => Self::NotInitialized,
            2 => Self::AlreadyInProgress,
            3 => Self::AlreadyShutDown,
            4 => Self::MinHookCleanupFailed,
            _ => return None,
        })
    }
}

impl fmt::Display for ShutdownStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::NotInitialized => write!(f, "hook DLL is loaded but not initialized"),
            Self::AlreadyInProgress => write!(
                f,
                "hook initialization, assignment replacement, or shutdown is in progress"
            ),
            Self::AlreadyShutDown => write!(f, "hook DLL is already shut down"),
            Self::MinHookCleanupFailed => write!(f, "MinHook cleanup failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HookStatus, HookTargetId, InitializeStatus, RESOLVE_CODE_BASE, ResolveFailureKind,
    };

    #[test]
    fn hook_status_codes_round_trip() {
        for status in [
            HookStatus::Inactive,
            HookStatus::Active,
            HookStatus::Transitioning,
        ] {
            assert_eq!(HookStatus::from_code(status as u32), Some(status));
        }
        assert_eq!(HookStatus::from_code(3), None);
        assert_eq!(HookStatus::from_code(u32::MAX), None);
    }

    #[test]
    fn success_code_is_zero() {
        assert_eq!(InitializeStatus::Success.to_code(), 0);
        assert_eq!(
            InitializeStatus::from_code(0),
            Some(InitializeStatus::Success)
        );
    }

    #[test]
    fn resolve_codes_pack_kind_and_target() {
        let present_not_found = InitializeStatus::Resolve {
            kind: ResolveFailureKind::NotFound,
            target: HookTargetId::Present,
        };
        assert_eq!(present_not_found.to_code(), 0x0001_0101);
        assert_eq!(
            InitializeStatus::from_code(0x0001_0101),
            Some(present_not_found)
        );

        let overlays_prologue = InitializeStatus::Resolve {
            kind: ResolveFailureKind::PrologueConflict,
            target: HookTargetId::OverlaysEnabled,
        };
        assert_eq!(overlays_prologue.to_code(), 0x0001_0309);
        assert_eq!(
            InitializeStatus::from_code(0x0001_0309),
            Some(overlays_prologue)
        );
    }

    #[test]
    fn invalid_codes_are_rejected() {
        assert_eq!(InitializeStatus::from_code(RESOLVE_CODE_BASE), None);
        assert_eq!(InitializeStatus::from_code(0x0001_0001), None);
        assert_eq!(InitializeStatus::from_code(0x0001_0100), None);
        assert_eq!(InitializeStatus::from_code(0x0002_0101), None);
        assert_eq!(InitializeStatus::from_code(15), None);
        assert_eq!(InitializeStatus::from_code(u32::MAX), None);
    }
}
