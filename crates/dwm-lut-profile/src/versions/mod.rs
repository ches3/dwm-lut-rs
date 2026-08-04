mod dwmcore_26100_1;
mod dwmcore_26100_1591;
mod dwmcore_26100_2161;
mod dwmcore_26100_2454;
mod dwmcore_26100_3912;
mod dwmcore_26100_4484;
mod dwmcore_26100_7309;
mod dwmcore_26100_7705;
mod dwmcore_26100_8737;

use crate::version::{DwmcoreVersion, VersionedProfile};

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
mod tests {
    use super::VERSIONED_PROFILES;
    use crate::target::HookTarget;
    use crate::version::{DwmcoreVersion, ProfileSelectError, select_versioned_profile};

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
            select_versioned_profile(VERSIONED_PROFILES, version_before_first),
            Err(ProfileSelectError::UnsupportedDwmcoreVersion {
                version
            }) if version == version_before_first
        ));

        for entry in VERSIONED_PROFILES {
            assert_eq!(
                select_versioned_profile(VERSIONED_PROFILES, entry.min_version)
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
                select_versioned_profile(VERSIONED_PROFILES, version_before_next)
                    .expect("previous profile must remain selected until the next minimum version")
                    .min_version,
                window[0].min_version
            );
        }

        let latest = VERSIONED_PROFILES
            .last()
            .expect("VERSIONED_PROFILES is non-empty");
        assert_eq!(
            select_versioned_profile(
                VERSIONED_PROFILES,
                DwmcoreVersion {
                    build: latest.min_version.build + 1,
                    revision: 0,
                },
            )
            .expect("latest profile must be selected for a newer version")
            .min_version,
            latest.min_version
        );
    }
}
