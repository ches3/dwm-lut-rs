use dwm_lut_profile::{
    DeviceResourceVirtualBaseAccess, FieldOffsetTarget, HookTarget, VirtualBaseTarget,
    VirtualMethodTarget,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AobStatus {
    Ok,
    NoSymbol,
    IcfStub,
    AmbiguousSymbol,
    UniquifyFailed,
}

impl AobStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoSymbol => "no_symbol",
            Self::IcfStub => "icf_stub",
            Self::AmbiguousSymbol => "ambiguous_symbol",
            Self::UniquifyFailed => "uniquify_failed",
        }
    }

    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

#[derive(Debug, Clone)]
pub struct AobReport {
    pub target: String,
    pub hook_target: HookTarget,
    pub status: AobStatus,
    pub aob: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionStatus {
    Ok,
    NoSymbol,
    NoVftable,
    NoDisp,
    Ambiguous,
}

impl ExtractionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoSymbol => "no_symbol",
            Self::NoVftable => "no_vftable",
            Self::NoDisp => "no_disp",
            Self::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FieldOffsetExtractionRow {
    pub target: FieldOffsetTarget,
    pub status: ExtractionStatus,
    pub value: Option<usize>,
    pub load_size: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualMethodAbiStatus {
    Ok,
    Mismatch,
    Unsupported,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct VirtualMethodExtractionRow {
    pub target: VirtualMethodTarget,
    pub status: ExtractionStatus,
    pub value: Option<usize>,
    pub abi_status: VirtualMethodAbiStatus,
}

#[derive(Debug, Clone)]
pub struct VirtualBaseExtractionRow {
    pub target: VirtualBaseTarget,
    pub status: ExtractionStatus,
    pub value: Option<DeviceResourceVirtualBaseAccess>,
}
