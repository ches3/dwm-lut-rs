mod dwmcore_26100_1;
mod dwmcore_26100_1591;
mod dwmcore_26100_2161;
mod dwmcore_26100_2454;
mod dwmcore_26100_3912;
mod dwmcore_26100_4484;
mod dwmcore_26100_7309;
mod dwmcore_26100_7705;
mod dwmcore_26100_8737;

use std::ffi::{OsString, c_void};
use std::fmt;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use windows_sys::Win32::Foundation::MAX_PATH;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};

pub const HOOK_MODULE_NAME: &str = "dwmcore.dll";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DwmcoreVersion {
    pub build: u32,
    pub revision: u32,
}

impl fmt::Display for DwmcoreVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.build, self.revision)
    }
}

pub struct VersionedProfile {
    pub min_version: DwmcoreVersion,
    pub profile: fn() -> HookProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSelectError {
    UnsupportedDwmcoreVersion { version: DwmcoreVersion },
    DwmcoreModuleNotLoaded,
    DwmcoreVersionQueryFailed,
}

impl fmt::Display for ProfileSelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedDwmcoreVersion { version } => {
                write!(
                    f,
                    "dwmcore.dll FileVersion 10.0.{version} is below the minimum supported hook profile"
                )
            }
            Self::DwmcoreModuleNotLoaded => write!(f, "dwmcore.dll was not loaded in the target"),
            Self::DwmcoreVersionQueryFailed => {
                write!(f, "failed to query dwmcore.dll FileVersion")
            }
        }
    }
}

impl std::error::Error for ProfileSelectError {}

pub const VERSIONED_PROFILES: &[VersionedProfile] = &[
    VersionedProfile {
        min_version: DwmcoreVersion {
            build: 26100,
            revision: 1,
        },
        profile: dwmcore_26100_1::profile,
    },
    VersionedProfile {
        min_version: DwmcoreVersion {
            build: 26100,
            revision: 1591,
        },
        profile: dwmcore_26100_1591::profile,
    },
    VersionedProfile {
        min_version: DwmcoreVersion {
            build: 26100,
            revision: 2161,
        },
        profile: dwmcore_26100_2161::profile,
    },
    VersionedProfile {
        min_version: DwmcoreVersion {
            build: 26100,
            revision: 2454,
        },
        profile: dwmcore_26100_2454::profile,
    },
    VersionedProfile {
        min_version: DwmcoreVersion {
            build: 26100,
            revision: 3912,
        },
        profile: dwmcore_26100_3912::profile,
    },
    VersionedProfile {
        min_version: DwmcoreVersion {
            build: 26100,
            revision: 4484,
        },
        profile: dwmcore_26100_4484::profile,
    },
    VersionedProfile {
        min_version: DwmcoreVersion {
            build: 26100,
            revision: 7309,
        },
        profile: dwmcore_26100_7309::profile,
    },
    VersionedProfile {
        min_version: DwmcoreVersion {
            build: 26100,
            revision: 7705,
        },
        profile: dwmcore_26100_7705::profile,
    },
    VersionedProfile {
        min_version: DwmcoreVersion {
            build: 26100,
            revision: 8737,
        },
        profile: dwmcore_26100_8737::profile,
    },
];

#[cfg(test)]
pub(crate) fn latest_registered_profile() -> HookProfile {
    (VERSIONED_PROFILES
        .last()
        .expect("VERSIONED_PROFILES is non-empty")
        .profile)()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTarget {
    Present,
    IsCandidateDirectFlipCompatible,
    DirectFlipInfoEnsureIndependentFlipState,
    IsDirectFlipSupportedOnTarget,
    LegacySwapChainCheckDirectFlipSupport,
    IsAdvancedDirectFlipCompatible,
    OverlayTestMode,
    DisableIndependentFlip,
    OverlaysEnabled,
}

impl HookTarget {
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

    pub const fn is_function_hook_target(self) -> bool {
        !matches!(self, Self::OverlayTestMode | Self::DisableIndependentFlip)
    }

    pub const fn is_required_signature(self) -> bool {
        matches!(self, Self::Present | Self::OverlayTestMode)
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
pub struct SwapChainVtablePath {
    pub container_vtable_index: usize,
    pub resource_vtable_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorIdentityOffsets {
    pub adapter_luid_low_offset: usize,
    pub adapter_luid_high_offset: usize,
    pub target_id_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookProfile {
    pub signatures: &'static [HookSignature],
    pub swap_chain: SwapChainVtablePath,
    pub hardware_protected_offset: usize,
    pub monitor_identity: MonitorIdentityOffsets,
}

#[repr(C)]
struct VsFixedFileInfo {
    signature: u32,
    struct_version: u32,
    file_version_ms: u32,
    file_version_ls: u32,
    product_version_ms: u32,
    product_version_ls: u32,
    file_flags_mask: u32,
    file_flags: u32,
    file_os: u32,
    file_type: u32,
    file_subtype: u32,
    file_date_ms: u32,
    file_date_ls: u32,
}

#[link(name = "version")]
unsafe extern "system" {
    fn GetFileVersionInfoSizeW(filename: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoW(filename: *const u16, handle: u32, len: u32, data: *mut c_void) -> i32;
    fn VerQueryValueW(
        block: *const c_void,
        sub_block: *const u16,
        buffer: *mut *mut c_void,
        len: *mut u32,
    ) -> i32;
}

pub fn dwmcore_file_version() -> Result<DwmcoreVersion, ProfileSelectError> {
    let module_name = wide_null(HOOK_MODULE_NAME);
    let handle = unsafe { GetModuleHandleW(module_name.as_ptr()) };
    if handle.is_null() {
        return Err(ProfileSelectError::DwmcoreModuleNotLoaded);
    }

    let mut path = vec![0u16; MAX_PATH as usize];
    let len = unsafe { GetModuleFileNameW(handle, path.as_mut_ptr(), path.len() as u32) } as usize;
    if len == 0 || len >= path.len() {
        return Err(ProfileSelectError::DwmcoreVersionQueryFailed);
    }
    path.truncate(len);
    file_version_from_path(&PathBuf::from(OsString::from_wide(&path)))
}

fn file_version_from_path(path: &std::path::Path) -> Result<DwmcoreVersion, ProfileSelectError> {
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut handle = 0u32;
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle) };
    if size == 0 {
        return Err(ProfileSelectError::DwmcoreVersionQueryFailed);
    }

    let mut buffer = vec![0u8; size as usize];
    let ok = unsafe {
        GetFileVersionInfoW(wide.as_ptr(), 0, size, buffer.as_mut_ptr().cast::<c_void>())
    };
    if ok == 0 {
        return Err(ProfileSelectError::DwmcoreVersionQueryFailed);
    }

    let sub_block = wide_null("\\");
    let mut value: *mut c_void = std::ptr::null_mut();
    let mut value_len = 0u32;
    let ok = unsafe {
        VerQueryValueW(
            buffer.as_ptr().cast::<c_void>(),
            sub_block.as_ptr(),
            &mut value,
            &mut value_len,
        )
    };
    if ok == 0 || value.is_null() || (value_len as usize) < std::mem::size_of::<VsFixedFileInfo>() {
        return Err(ProfileSelectError::DwmcoreVersionQueryFailed);
    }

    let info = unsafe { &*value.cast::<VsFixedFileInfo>() };
    if info.signature != 0xFEEF_04BD {
        return Err(ProfileSelectError::DwmcoreVersionQueryFailed);
    }

    Ok(DwmcoreVersion {
        build: info.file_version_ls >> 16,
        revision: info.file_version_ls & 0xFFFF,
    })
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn select_versioned_profile(
    version: DwmcoreVersion,
) -> Result<&'static VersionedProfile, ProfileSelectError> {
    VERSIONED_PROFILES
        .iter()
        .filter(|profile| profile.min_version <= version)
        .max_by_key(|profile| profile.min_version)
        .ok_or(ProfileSelectError::UnsupportedDwmcoreVersion { version })
}

#[cfg(test)]
mod tests {
    use super::{
        DwmcoreVersion, HookTarget, ProfileSelectError, VERSIONED_PROFILES,
        select_versioned_profile,
    };

    #[test]
    fn versioned_profiles_are_sorted_and_unique() {
        assert!(!VERSIONED_PROFILES.is_empty());
        for window in VERSIONED_PROFILES.windows(2) {
            assert!(
                window[0].min_version < window[1].min_version,
                "VERSIONED_PROFILES must be strictly ascending by min_version"
            );
        }
    }

    #[test]
    fn versioned_profile_entries_include_required_signatures() {
        for entry in VERSIONED_PROFILES {
            let profile = (entry.profile)();
            for target in [HookTarget::Present, HookTarget::OverlayTestMode] {
                assert!(
                    profile
                        .signatures
                        .iter()
                        .any(|signature| signature.target == target),
                    "snapshot {} must include required target {:?}",
                    entry.min_version,
                    target
                );
            }
        }
    }

    #[test]
    fn select_profile_picks_highest_min_version() {
        let first = VERSIONED_PROFILES
            .first()
            .expect("VERSIONED_PROFILES is non-empty");
        let version_before_first = if first.min_version.revision > 0 {
            DwmcoreVersion {
                build: first.min_version.build,
                revision: first.min_version.revision - 1,
            }
        } else {
            DwmcoreVersion {
                build: first.min_version.build - 1,
                revision: u32::MAX,
            }
        };
        assert!(matches!(
            select_versioned_profile(version_before_first),
            Err(ProfileSelectError::UnsupportedDwmcoreVersion {
                version
            }) if version == version_before_first
        ));

        for entry in VERSIONED_PROFILES {
            assert_eq!(
                select_versioned_profile(entry.min_version)
                    .expect("profile must be selected at its minimum version")
                    .min_version,
                entry.min_version
            );
        }

        for window in VERSIONED_PROFILES.windows(2) {
            let next = window[1].min_version;
            let version_before_next = if next.revision > 0 {
                DwmcoreVersion {
                    build: next.build,
                    revision: next.revision - 1,
                }
            } else {
                DwmcoreVersion {
                    build: next.build - 1,
                    revision: u32::MAX,
                }
            };
            assert_eq!(
                select_versioned_profile(version_before_next)
                    .expect("previous profile must remain selected until the next minimum version")
                    .min_version,
                window[0].min_version
            );
        }

        let latest = VERSIONED_PROFILES
            .last()
            .expect("VERSIONED_PROFILES is non-empty");
        assert_eq!(
            select_versioned_profile(DwmcoreVersion {
                build: latest.min_version.build + 1,
                revision: 0,
            })
            .expect("latest profile must be selected for a newer version")
            .min_version,
            latest.min_version
        );
    }
}
