use std::fmt;

use crate::profile::HookProfile;

pub const DWMCORE_MODULE_NAME: &str = "dwmcore.dll";

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

pub struct RevisionProfile {
    pub min_revision: u32,
    pub profile: fn() -> HookProfile,
}

pub struct SupportedBuild {
    pub build: u32,
    pub profiles: &'static [RevisionProfile],
}

pub struct SelectedProfile {
    pub min_version: DwmcoreVersion,
    pub profile: fn() -> HookProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSelectError {
    UnsupportedDwmcoreVersion { version: DwmcoreVersion },
}

impl fmt::Display for ProfileSelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedDwmcoreVersion { version } => {
                write!(
                    f,
                    "dwmcore.dll FileVersion 10.0.{version} has no matching hook profile"
                )
            }
        }
    }
}

impl std::error::Error for ProfileSelectError {}

pub fn select_profile(
    builds: &[SupportedBuild],
    version: DwmcoreVersion,
) -> Result<SelectedProfile, ProfileSelectError> {
    let supported = builds
        .iter()
        .find(|build| build.build == version.build)
        .ok_or(ProfileSelectError::UnsupportedDwmcoreVersion { version })?;
    let entry = supported
        .profiles
        .iter()
        .filter(|profile| profile.min_revision <= version.revision)
        .max_by_key(|profile| profile.min_revision)
        .ok_or(ProfileSelectError::UnsupportedDwmcoreVersion { version })?;
    Ok(SelectedProfile {
        min_version: DwmcoreVersion {
            build: supported.build,
            revision: entry.min_revision,
        },
        profile: entry.profile,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DwmcoreVersion, ProfileSelectError, RevisionProfile, SupportedBuild, select_profile,
    };
    use crate::profile::{
        D3D11ResourceAccess, HardwareProtectedAccess, HookProfile, MonitorIdentityAccess,
        OverlaySwapChainFromContext,
    };

    const fn marked_profile(build: usize, revision: usize) -> HookProfile {
        HookProfile {
            hooks: &[],
            overlay_swap_chain_from_context: OverlaySwapChainFromContext {
                overlay_monitor_target_pointer_offset: 0,
                get_overlay_swap_chain_vtable_index: 0,
            },
            d3d11_resource: D3D11ResourceAccess::PhysicalBackBuffer {
                get_physical_back_buffer_vtable_index: build,
                get_d3d11_resource_vtable_index: revision,
            },
            monitor_identity: MonitorIdentityAccess {
                get_display_adapter_luid_vtable_index: 0,
                get_vid_pn_target_id_vtable_index: 0,
            },
            hardware_protected: HardwareProtectedAccess::DirectField { offset: 0 },
        }
    }

    fn profile_100_10() -> HookProfile {
        marked_profile(100, 10)
    }

    fn profile_100_50() -> HookProfile {
        marked_profile(100, 50)
    }

    fn profile_200_10() -> HookProfile {
        marked_profile(200, 10)
    }

    fn profile_200_30() -> HookProfile {
        marked_profile(200, 30)
    }

    const FIXTURE_BUILDS: &[SupportedBuild] = &[
        SupportedBuild {
            build: 100,
            profiles: &[
                RevisionProfile {
                    min_revision: 10,
                    profile: profile_100_10,
                },
                RevisionProfile {
                    min_revision: 50,
                    profile: profile_100_50,
                },
            ],
        },
        SupportedBuild {
            build: 200,
            profiles: &[
                RevisionProfile {
                    min_revision: 10,
                    profile: profile_200_10,
                },
                RevisionProfile {
                    min_revision: 30,
                    profile: profile_200_30,
                },
            ],
        },
    ];

    fn assert_selected(version: DwmcoreVersion, expected_min: DwmcoreVersion) {
        let selected = select_profile(FIXTURE_BUILDS, version).expect("profile must be selected");
        assert_eq!(selected.min_version, expected_min);
        let profile = (selected.profile)();
        assert_eq!(
            profile.d3d11_resource,
            D3D11ResourceAccess::PhysicalBackBuffer {
                get_physical_back_buffer_vtable_index: expected_min.build as usize,
                get_d3d11_resource_vtable_index: expected_min.revision as usize,
            }
        );
    }

    #[test]
    fn select_profile_rejects_unsupported_build() {
        let version = DwmcoreVersion {
            build: 999,
            revision: 0,
        };
        assert!(matches!(
            select_profile(FIXTURE_BUILDS, version),
            Err(ProfileSelectError::UnsupportedDwmcoreVersion {
                version: rejected
            }) if rejected == version
        ));
    }

    #[test]
    fn select_profile_rejects_revision_before_first_min() {
        let version = DwmcoreVersion {
            build: 100,
            revision: 9,
        };
        assert!(matches!(
            select_profile(FIXTURE_BUILDS, version),
            Err(ProfileSelectError::UnsupportedDwmcoreVersion {
                version: rejected
            }) if rejected == version
        ));
    }

    #[test]
    fn select_profile_selects_at_each_min_revision() {
        assert_selected(
            DwmcoreVersion {
                build: 100,
                revision: 10,
            },
            DwmcoreVersion {
                build: 100,
                revision: 10,
            },
        );
        assert_selected(
            DwmcoreVersion {
                build: 100,
                revision: 50,
            },
            DwmcoreVersion {
                build: 100,
                revision: 50,
            },
        );
        assert_selected(
            DwmcoreVersion {
                build: 200,
                revision: 10,
            },
            DwmcoreVersion {
                build: 200,
                revision: 10,
            },
        );
        assert_selected(
            DwmcoreVersion {
                build: 200,
                revision: 30,
            },
            DwmcoreVersion {
                build: 200,
                revision: 30,
            },
        );
    }

    #[test]
    fn select_profile_keeps_previous_until_next_min() {
        assert_selected(
            DwmcoreVersion {
                build: 100,
                revision: 49,
            },
            DwmcoreVersion {
                build: 100,
                revision: 10,
            },
        );
    }

    #[test]
    fn select_profile_selects_latest_for_newer_revision() {
        assert_selected(
            DwmcoreVersion {
                build: 100,
                revision: 51,
            },
            DwmcoreVersion {
                build: 100,
                revision: 50,
            },
        );
        assert_selected(
            DwmcoreVersion {
                build: 200,
                revision: 31,
            },
            DwmcoreVersion {
                build: 200,
                revision: 30,
            },
        );
    }

    #[test]
    fn select_profile_selects_matching_build_not_other() {
        assert_selected(
            DwmcoreVersion {
                build: 200,
                revision: 50,
            },
            DwmcoreVersion {
                build: 200,
                revision: 30,
            },
        );
        assert_selected(
            DwmcoreVersion {
                build: 100,
                revision: 30,
            },
            DwmcoreVersion {
                build: 100,
                revision: 10,
            },
        );
    }
}
