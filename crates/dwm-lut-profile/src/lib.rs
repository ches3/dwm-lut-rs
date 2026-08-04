mod profile;
mod scan;
mod target;
mod version;
mod versions;

pub use profile::{
    AobToken, HookProfile, HookSignature, MonitorIdentityOffsets, SignatureLocator,
    SwapChainVtablePath,
};
pub use scan::{
    AobMatch, OutOfBoundsReason, ResolvedRva, Rva, SignatureScanError, SignatureScanReport,
    SkippedSignature, SkippedSignatureReason, match_aob, resolve_signature_rva, scan_profile,
};
pub use target::HookTarget;
pub use version::{DWMCORE_MODULE_NAME, DwmcoreVersion, ProfileSelectError, VersionedProfile};
pub use versions::VERSIONED_PROFILES;

pub fn select_versioned_profile(
    version: DwmcoreVersion,
) -> Result<&'static VersionedProfile, ProfileSelectError> {
    version::select_versioned_profile(VERSIONED_PROFILES, version)
}
