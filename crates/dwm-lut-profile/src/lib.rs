mod profile;
mod scan;
mod target;
mod version;
mod versions;

pub use profile::{
    AobToken, ContextToSwapChainPath, HookProfile, HookSignature, MonitorIdentityOffsets,
    SwapChainToResourcePath,
};
pub use scan::{
    AobMatch, ResolvedRva, Rva, SignatureScanError, SignatureScanReport, SkippedSignature,
    SkippedSignatureReason, match_aob, resolve_signature_rva, scan_profile,
};
pub use target::HookTarget;
pub use version::{
    DWMCORE_MODULE_NAME, DwmcoreVersion, ProfileSelectError, RevisionProfile, SelectedProfile,
    SupportedBuild,
};
pub use versions::SUPPORTED_BUILDS;

pub fn select_profile(version: DwmcoreVersion) -> Result<SelectedProfile, ProfileSelectError> {
    version::select_profile(SUPPORTED_BUILDS, version)
}
