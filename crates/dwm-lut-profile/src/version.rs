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

pub struct VersionedProfile {
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
                    "dwmcore.dll FileVersion 10.0.{version} is below the minimum supported hook profile"
                )
            }
        }
    }
}

impl std::error::Error for ProfileSelectError {}

pub fn select_versioned_profile(
    profiles: &[VersionedProfile],
    version: DwmcoreVersion,
) -> Result<&VersionedProfile, ProfileSelectError> {
    profiles
        .iter()
        .filter(|profile| profile.min_version <= version)
        .max_by_key(|profile| profile.min_version)
        .ok_or(ProfileSelectError::UnsupportedDwmcoreVersion { version })
}
