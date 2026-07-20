mod profile_26200;

use std::fmt;

use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

pub const HOOK_MODULE_NAME: &str = "dwmcore.dll";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MinBuild(pub u32);

pub struct VersionedProfile {
    pub min_build: MinBuild,
    pub id: &'static str,
    pub build: fn() -> HookProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSelectError {
    UnsupportedBuild { build: u32 },
    OsVersionQueryFailed,
}

impl fmt::Display for ProfileSelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedBuild { build } => {
                write!(f, "OS build {build} is below the minimum supported profile")
            }
            Self::OsVersionQueryFailed => write!(f, "failed to query OS build number"),
        }
    }
}

impl std::error::Error for ProfileSelectError {}

pub const VERSIONED_PROFILES: &[VersionedProfile] = &[VersionedProfile {
    min_build: MinBuild(26200),
    id: "25h2-26200",
    build: profile_26200::build,
}];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTarget {
    Present,
    IsCandidateDirectFlipCompatible,
    WindowContextIsCandidateDirectFlipCompatible,
    CompSwapChainIsCandidateDirectFlipCompatible,
    CompSwapChainIsCandidateIndependentFlipCompatible,
    CompVisualIsCandidateForPromotion,
    DirectFlipInfoEnsureIndependentFlipState,
    IsDirectFlipSupportedOnTarget,
    LegacySwapChainCheckDirectFlipSupport,
    IsAdvancedDirectFlipCompatible,
    OverlayTestMode,
    DisableIndependentFlip,
}

impl HookTarget {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Present => "Present",
            Self::IsCandidateDirectFlipCompatible => "IsCandidateDirectFlipCompatible",
            Self::WindowContextIsCandidateDirectFlipCompatible => {
                "CWindowContext::IsCandidateDirectFlipCompatible"
            }
            Self::CompSwapChainIsCandidateDirectFlipCompatible => {
                "CCompSwapChain::IsCandidateDirectFlipCompatible"
            }
            Self::CompSwapChainIsCandidateIndependentFlipCompatible => {
                "CCompSwapChain::IsCandidateIndependentFlipCompatible"
            }
            Self::CompVisualIsCandidateForPromotion => "CCompVisual::IsCandidateForPromotion",
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
        }
    }

    pub const fn is_function_hook_target(self) -> bool {
        !matches!(self, Self::OverlayTestMode | Self::DisableIndependentFlip)
    }

    pub const fn is_required_signature(self) -> bool {
        matches!(
            self,
            Self::Present | Self::IsCandidateDirectFlipCompatible | Self::OverlayTestMode
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AobToken {
    Exact(u8),
    Wildcard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureLocator {
    Aob {
        tokens: &'static [AobToken],
    },
    RipRelativeGlobalAob {
        tokens: &'static [AobToken],
        displacement_offset: usize,
        instruction_size: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSignature {
    pub target: HookTarget,
    pub locator: SignatureLocator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapChainPathHypothesis {
    pub container_vtable_index: usize,
    pub resource_vtable_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipBoxPathHypothesis {
    pub context_state_pointer_offset: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareProtectedPathHypothesis {
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorIdentityPathHypothesis {
    pub adapter_luid_low_offset: usize,
    pub adapter_luid_high_offset: usize,
    pub target_id_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileHypotheses {
    pub swap_chain: SwapChainPathHypothesis,
    pub clip_box: ClipBoxPathHypothesis,
    pub hardware_protected: HardwareProtectedPathHypothesis,
    pub monitor_identity: MonitorIdentityPathHypothesis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookProfile {
    pub signatures: &'static [HookSignature],
    pub hypotheses: ProfileHypotheses,
}

pub fn os_build_number() -> Result<u32, ProfileSelectError> {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        dwMajorVersion: 0,
        dwMinorVersion: 0,
        dwBuildNumber: 0,
        dwPlatformId: 0,
        szCSDVersion: [0; 128],
    };
    let status = unsafe { RtlGetVersion(&mut info) };
    if status < 0 {
        return Err(ProfileSelectError::OsVersionQueryFailed);
    }
    Ok(info.dwBuildNumber)
}

pub fn select_versioned_profile(
    build: u32,
) -> Result<&'static VersionedProfile, ProfileSelectError> {
    VERSIONED_PROFILES
        .iter()
        .filter(|profile| profile.min_build.0 <= build)
        .max_by_key(|profile| profile.min_build)
        .ok_or(ProfileSelectError::UnsupportedBuild { build })
}

#[cfg(test)]
mod tests {
    use super::{HookTarget, ProfileSelectError, VERSIONED_PROFILES, select_versioned_profile};

    #[test]
    fn versioned_profiles_are_sorted_and_unique() {
        assert!(!VERSIONED_PROFILES.is_empty());
        for window in VERSIONED_PROFILES.windows(2) {
            assert!(
                window[0].min_build < window[1].min_build,
                "VERSIONED_PROFILES must be strictly ascending by min_build"
            );
        }
    }

    #[test]
    fn versioned_profile_entries_build_required_signatures() {
        for entry in VERSIONED_PROFILES {
            let profile = (entry.build)();
            for target in [
                HookTarget::Present,
                HookTarget::IsCandidateDirectFlipCompatible,
                HookTarget::OverlayTestMode,
            ] {
                assert!(
                    profile
                        .signatures
                        .iter()
                        .any(|signature| signature.target == target),
                    "snapshot {} must include required target {:?}",
                    entry.id,
                    target
                );
            }
        }
    }

    #[test]
    fn select_profile_picks_highest_min_build() {
        assert!(matches!(
            select_versioned_profile(26199),
            Err(ProfileSelectError::UnsupportedBuild { build: 26199 })
        ));
        assert_eq!(
            select_versioned_profile(26200).expect("26200").id,
            "25h2-26200"
        );
        assert_eq!(
            select_versioned_profile(26244).expect("26244").id,
            "25h2-26200"
        );
        assert_eq!(
            select_versioned_profile(26300)
                .expect("high build fallback")
                .id,
            "25h2-26200"
        );
    }
}
