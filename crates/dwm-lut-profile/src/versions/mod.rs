mod dwmcore_26100_1;
mod dwmcore_26100_1591;
mod dwmcore_26100_2161;
mod dwmcore_26100_2454;
mod dwmcore_26100_3912;
mod dwmcore_26100_4484;
mod dwmcore_26100_7309;
mod dwmcore_26100_7705;

use crate::version::{RevisionProfile, SupportedBuild};

pub const SUPPORTED_BUILDS: &[SupportedBuild] = &[SupportedBuild {
    build: 26100,
    profiles: &[
        RevisionProfile {
            min_revision: 1,
            profile: dwmcore_26100_1::profile,
        },
        RevisionProfile {
            min_revision: 1591,
            profile: dwmcore_26100_1591::profile,
        },
        RevisionProfile {
            min_revision: 2161,
            profile: dwmcore_26100_2161::profile,
        },
        RevisionProfile {
            min_revision: 2454,
            profile: dwmcore_26100_2454::profile,
        },
        RevisionProfile {
            min_revision: 3912,
            profile: dwmcore_26100_3912::profile,
        },
        RevisionProfile {
            min_revision: 4484,
            profile: dwmcore_26100_4484::profile,
        },
        RevisionProfile {
            min_revision: 7309,
            profile: dwmcore_26100_7309::profile,
        },
        RevisionProfile {
            min_revision: 7705,
            profile: dwmcore_26100_7705::profile,
        },
    ],
}];

#[cfg(test)]
mod tests {
    use super::SUPPORTED_BUILDS;
    use crate::target::HookTarget;

    #[test]
    fn supported_builds_are_non_empty_with_unique_builds() {
        assert!(!SUPPORTED_BUILDS.is_empty());
        for (index, build) in SUPPORTED_BUILDS.iter().enumerate() {
            assert!(
                SUPPORTED_BUILDS[..index]
                    .iter()
                    .all(|earlier| earlier.build != build.build),
                "SUPPORTED_BUILDS must have unique build numbers"
            );
        }
    }

    #[test]
    fn supported_build_profiles_are_non_empty_and_sorted() {
        for supported in SUPPORTED_BUILDS {
            assert!(
                !supported.profiles.is_empty(),
                "SUPPORTED_BUILDS entry {} must include at least one profile",
                supported.build
            );
            for window in supported.profiles.windows(2) {
                assert!(
                    window[0].min_revision < window[1].min_revision,
                    "profiles for build {} must be strictly ascending by min_revision",
                    supported.build
                );
            }
        }
    }

    #[test]
    fn revision_profiles_include_required_signatures() {
        for supported in SUPPORTED_BUILDS {
            for entry in supported.profiles {
                let profile = (entry.profile)();
                for &target in HookTarget::ALL {
                    if !target.is_required_signature() {
                        continue;
                    }
                    assert!(
                        profile
                            .signatures
                            .iter()
                            .any(|signature| signature.target == target),
                        "build {}.{} must include required target {}",
                        supported.build,
                        entry.min_revision,
                        target.label()
                    );
                }
            }
        }
    }
}
