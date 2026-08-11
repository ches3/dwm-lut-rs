use dwm_lut_profile::{HookProfile, SUPPORTED_BUILDS};

pub(crate) fn representative_hook_profile() -> HookProfile {
    const BUILD: u32 = 26100;
    const MIN_REVISION: u32 = 7705;

    let supported = SUPPORTED_BUILDS
        .iter()
        .find(|supported| supported.build == BUILD)
        .expect("representative test build must exist");
    let revision = supported
        .profiles
        .iter()
        .find(|revision| revision.min_revision == MIN_REVISION)
        .expect("representative test revision must exist");

    (revision.profile)()
}
