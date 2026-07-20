use std::fmt;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitializeStatus {
    Success = 0,
    NullPayload = 1,
    InvalidPayload = 2,
    AlreadyInitialized = 3,
    DwmcoreModuleNotLoaded = 4,
    DwmcoreImageInvalid = 5,
    PresentSignatureNotFound = 6,
    PresentSignatureAmbiguous = 7,
    DirectFlipSignatureNotFound = 8,
    DirectFlipSignatureAmbiguous = 9,
    PayloadDecodeFailed = 12,
    PayloadHasNoAssignments = 13,
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
    DwmcoreImageMismatch = 30,
    PresentPrologueConflict = 31,
    DirectFlipPrologueConflict = 32,
    WindowDirectFlipPrologueConflict = 33,
    CompSwapChainDirectFlipPrologueConflict = 34,
    CompVisualPromotionPrologueConflict = 35,
    CompSwapChainIndependentFlipPrologueConflict = 36,
    DwmcoreImageAccessFailed = 37,
}

impl InitializeStatus {
    pub fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            0 => Self::Success,
            1 => Self::NullPayload,
            2 => Self::InvalidPayload,
            3 => Self::AlreadyInitialized,
            4 => Self::DwmcoreModuleNotLoaded,
            5 => Self::DwmcoreImageInvalid,
            6 => Self::PresentSignatureNotFound,
            7 => Self::PresentSignatureAmbiguous,
            8 => Self::DirectFlipSignatureNotFound,
            9 => Self::DirectFlipSignatureAmbiguous,
            12 => Self::PayloadDecodeFailed,
            13 => Self::PayloadHasNoAssignments,
            15 => Self::WindowDirectFlipSignatureNotFound,
            16 => Self::WindowDirectFlipSignatureAmbiguous,
            17 => Self::CompSwapChainDirectFlipSignatureNotFound,
            18 => Self::CompSwapChainDirectFlipSignatureAmbiguous,
            19 => Self::CompVisualPromotionSignatureNotFound,
            20 => Self::CompVisualPromotionSignatureAmbiguous,
            21 => Self::OverlayTestModeNotFound,
            22 => Self::OverlayTestModeAmbiguous,
            23 => Self::CompSwapChainIndependentFlipSignatureNotFound,
            24 => Self::CompSwapChainIndependentFlipSignatureAmbiguous,
            25 => Self::MinHookLoadFailed,
            26 => Self::MinHookGetProcAddressFailed,
            27 => Self::MinHookInitializeFailed,
            28 => Self::MinHookCreateHookFailed,
            29 => Self::MinHookEnableHookFailed,
            30 => Self::DwmcoreImageMismatch,
            31 => Self::PresentPrologueConflict,
            32 => Self::DirectFlipPrologueConflict,
            33 => Self::WindowDirectFlipPrologueConflict,
            34 => Self::CompSwapChainDirectFlipPrologueConflict,
            35 => Self::CompVisualPromotionPrologueConflict,
            36 => Self::CompSwapChainIndependentFlipPrologueConflict,
            37 => Self::DwmcoreImageAccessFailed,
            _ => return None,
        })
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
            Self::PresentSignatureNotFound => write!(f, "Present signature was not found"),
            Self::PresentSignatureAmbiguous => {
                write!(f, "Present signature matched multiple locations")
            }
            Self::DirectFlipSignatureNotFound => {
                write!(f, "IsCandidateDirectFlipCompatible signature was not found")
            }
            Self::DirectFlipSignatureAmbiguous => write!(
                f,
                "IsCandidateDirectFlipCompatible signature matched multiple locations"
            ),
            Self::PayloadDecodeFailed => write!(f, "payload could not be decoded"),
            Self::PayloadHasNoAssignments => {
                write!(f, "payload does not contain any LUT assignments")
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
            Self::CompVisualPromotionSignatureNotFound => write!(
                f,
                "CCompVisual::IsCandidateForPromotion signature was not found"
            ),
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
            Self::DwmcoreImageMismatch => {
                write!(f, "loaded dwmcore.dll does not match its backing file")
            }
            Self::PresentPrologueConflict => {
                write!(f, "Present prologue is modified by a conflicting hook")
            }
            Self::DirectFlipPrologueConflict => write!(
                f,
                "IsCandidateDirectFlipCompatible prologue is modified by a conflicting hook"
            ),
            Self::WindowDirectFlipPrologueConflict => write!(
                f,
                "CWindowContext::IsCandidateDirectFlipCompatible prologue is modified by a conflicting hook"
            ),
            Self::CompSwapChainDirectFlipPrologueConflict => write!(
                f,
                "CCompSwapChain::IsCandidateDirectFlipCompatible prologue is modified by a conflicting hook"
            ),
            Self::CompVisualPromotionPrologueConflict => write!(
                f,
                "CCompVisual::IsCandidateForPromotion prologue is modified by a conflicting hook"
            ),
            Self::CompSwapChainIndependentFlipPrologueConflict => write!(
                f,
                "CCompSwapChain::IsCandidateIndependentFlipCompatible prologue is modified by a conflicting hook"
            ),
            Self::DwmcoreImageAccessFailed => {
                write!(f, "dwmcore.dll backing image could not be accessed")
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
            Self::AlreadyInProgress => write!(f, "hook initialization or shutdown is in progress"),
            Self::PayloadDecodeFailed => write!(f, "payload could not be decoded"),
            Self::PayloadHasNoAssignments => {
                write!(f, "payload does not contain any LUT assignments")
            }
        }
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
            Self::AlreadyInProgress => write!(f, "hook shutdown is already in progress"),
            Self::AlreadyShutDown => write!(f, "hook DLL is already shut down"),
            Self::MinHookCleanupFailed => write!(f, "MinHook cleanup failed"),
        }
    }
}
