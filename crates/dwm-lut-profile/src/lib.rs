mod profile;
mod scan;
mod target;
mod version;
mod versions;

pub use profile::{
    AobToken, D3D11ResourcePath, FieldOffsets, HookProfile, HookSignature,
    OverlaySwapChainIdentityOffsets, OverlaySwapChainPath, VtableIndices,
};
pub use scan::{
    AobMatch, ResolvedRva, Rva, SignatureScanError, SignatureScanReport, SkippedSignature,
    SkippedSignatureReason, match_aob, resolve_signature_rva, scan_profile,
};
pub use target::HookTarget;

#[cfg(feature = "xtask")]
pub use target::{
    FieldOffsetExtractSpec, FieldOffsetTarget, VtableIndexExtractSpec, VtableIndexTarget,
};
pub use version::{
    DWMCORE_MODULE_NAME, DwmcoreVersion, ProfileSelectError, RevisionProfile, SelectedProfile,
    SupportedBuild,
};
pub use versions::SUPPORTED_BUILDS;

pub fn select_profile(version: DwmcoreVersion) -> Result<SelectedProfile, ProfileSelectError> {
    version::select_profile(SUPPORTED_BUILDS, version)
}
