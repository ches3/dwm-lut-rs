#[cfg(feature = "xtask")]
mod access;
mod profile;
mod scan;
mod target;
mod version;
mod versions;

pub use profile::{
    AbiArgument, AbiClass, AbiObjectType, AbiType, AobToken, D3D11ResourceAccess,
    DeviceResourceVirtualBaseAccess, HardwareProtectedAccess, HookEntry, HookProfile,
    MonitorIdentityAccess, OverlaySwapChainFromContext,
};
pub use scan::{
    AobMatch, ResolvedRva, Rva, SignatureScanError, SignatureScanReport, SkippedSignature,
    SkippedSignatureReason, match_aob, resolve_signature_rva, scan_profile,
};
pub use target::HookTarget;

#[cfg(feature = "xtask")]
pub use access::{
    AccessExtractSpec, AccessFieldTarget, AccessMethodExtractSpec, ProfileAccess,
    ProfileAccessField, ProfileAccessTarget, VirtualBaseComponent,
};
#[cfg(feature = "xtask")]
pub use target::{
    AbiReturnType, DirectFieldWidth, FieldOffsetExtractSpec, FieldOffsetTarget, HookExtractSpec,
    ParameterSpec, VirtualBaseClassExtractSpec, VirtualBaseExtractSpec, VirtualBaseTarget,
    VirtualMethodExtractSpec, VirtualMethodReturnType, VirtualMethodTarget, VtableExtractSpec,
    VtableView,
};
pub use version::{
    DWMCORE_MODULE_NAME, DwmcoreVersion, ProfileSelectError, RevisionProfile, SelectedProfile,
    SupportedBuild,
};
pub use versions::SUPPORTED_BUILDS;

pub fn select_profile(version: DwmcoreVersion) -> Result<SelectedProfile, ProfileSelectError> {
    version::select_profile(SUPPORTED_BUILDS, version)
}
