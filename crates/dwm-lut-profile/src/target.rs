#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HookTarget {
    Present,
    IsCandidateDirectFlipCompatible,
    IsCandidateOverlayCompatible,
}

impl HookTarget {
    pub const ALL: &[Self] = &[
        Self::Present,
        Self::IsCandidateDirectFlipCompatible,
        Self::IsCandidateOverlayCompatible,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Present => "Present",
            Self::IsCandidateDirectFlipCompatible => "IsCandidateDirectFlipCompatible",
            Self::IsCandidateOverlayCompatible => "IsCandidateOverlayCompatible",
        }
    }

    pub const fn is_required_signature(self) -> bool {
        matches!(self, Self::Present)
    }

    pub const fn is_flip_gate(self) -> bool {
        match self {
            Self::Present => false,
            Self::IsCandidateDirectFlipCompatible | Self::IsCandidateOverlayCompatible => true,
        }
    }

    #[cfg(feature = "xtask")]
    pub const fn pdb_symbol_prefix(self) -> &'static str {
        match self {
            Self::Present => "?Present@COverlayContext@@",
            Self::IsCandidateDirectFlipCompatible => {
                "?IsCandidateDirectFlipCompatible@COverlayContext@@"
            }
            Self::IsCandidateOverlayCompatible => "?IsCandidateOverlayCompatible@COverlayContext@@",
        }
    }
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VtableIndexTarget {
    GetPhysicalBackBuffer,
    GetD3D11Resource,
    GetOverlaySwapChain,
}

#[cfg(feature = "xtask")]
impl VtableIndexTarget {
    pub const ALL: &[Self] = &[
        Self::GetPhysicalBackBuffer,
        Self::GetD3D11Resource,
        Self::GetOverlaySwapChain,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::GetPhysicalBackBuffer => "GetPhysicalBackBuffer",
            Self::GetD3D11Resource => "GetD3D11Resource",
            Self::GetOverlaySwapChain => "GetOverlaySwapChain",
        }
    }

    pub const fn extract_spec(self) -> VtableIndexExtractSpec {
        match self {
            Self::GetPhysicalBackBuffer => VtableIndexExtractSpec {
                vftable_symbols: &[
                    "??_7CDDisplaySwapChain@@6BIDeviceResource@@@",
                    "??_7CLegacySwapChain@@6BIDeviceResource@@@",
                ],
                method_symbol_prefix: "?GetPhysicalBackBuffer@",
            },
            Self::GetD3D11Resource => VtableIndexExtractSpec {
                vftable_symbols: &[
                    "??_7CDDisplaySwapChainBuffer@@6B@",
                    "??_7CLegacySwapChainBuffer@@6B@",
                ],
                method_symbol_prefix: "?GetD3D11Resource@",
            },
            Self::GetOverlaySwapChain => VtableIndexExtractSpec {
                vftable_symbols: &[
                    "??_7CDDisplayRenderTarget@@6BIPixelFormat@@IOverlayMonitorTarget@@@",
                    "??_7CLegacyRenderTarget@@6BIPixelFormat@@IOverlayMonitorTarget@@@",
                ],
                method_symbol_prefix: "?GetOverlaySwapChain@",
            },
        }
    }
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VtableIndexExtractSpec {
    pub vftable_symbols: &'static [&'static str],
    pub method_symbol_prefix: &'static str,
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FieldOffsetTarget {
    HardwareProtected,
    AdapterLuidLowPart,
    AdapterLuidHighPart,
    VidPnTargetId,
    OverlayMonitorTargetPointer,
}

#[cfg(feature = "xtask")]
impl FieldOffsetTarget {
    pub const ALL: &[Self] = &[
        Self::HardwareProtected,
        Self::AdapterLuidLowPart,
        Self::AdapterLuidHighPart,
        Self::VidPnTargetId,
        Self::OverlayMonitorTargetPointer,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::HardwareProtected => "HardwareProtected",
            Self::AdapterLuidLowPart => "AdapterLuid.LowPart",
            Self::AdapterLuidHighPart => "AdapterLuid.HighPart",
            Self::VidPnTargetId => "VidPnTargetId",
            Self::OverlayMonitorTargetPointer => "IOverlayMonitorTarget*",
        }
    }

    pub const fn extract_spec(self) -> FieldOffsetExtractSpec {
        match self {
            Self::HardwareProtected => FieldOffsetExtractSpec::GetterDisp {
                symbol_prefix: "?IsHardwareProtected@COverlaySwapChain@@",
                derived_add: None,
            },
            Self::AdapterLuidLowPart => FieldOffsetExtractSpec::GetterDisp {
                symbol_prefix: "?GetDisplayAdapterLuid@COverlaySwapChain@@",
                derived_add: None,
            },
            Self::AdapterLuidHighPart => FieldOffsetExtractSpec::GetterDisp {
                symbol_prefix: "?GetDisplayAdapterLuid@COverlaySwapChain@@",
                derived_add: Some(4),
            },
            Self::VidPnTargetId => FieldOffsetExtractSpec::GetterDisp {
                symbol_prefix: "?GetVidPnTargetId@COverlaySwapChain@@",
                derived_add: None,
            },
            Self::OverlayMonitorTargetPointer => FieldOffsetExtractSpec::CtorFirstParamStore {
                ctor_symbol_prefix: "??0COverlayContext@@",
                first_param_type_encoding: "PEAVIOverlayMonitorTarget@@",
            },
        }
    }
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldOffsetExtractSpec {
    GetterDisp {
        symbol_prefix: &'static str,
        derived_add: Option<usize>,
    },
    CtorFirstParamStore {
        ctor_symbol_prefix: &'static str,
        first_param_type_encoding: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::HookTarget;

    #[cfg(feature = "xtask")]
    use super::{FieldOffsetTarget, VtableIndexTarget};

    #[test]
    fn hook_all_covers_every_variant() {
        let mut seen = 0u8;
        for &target in HookTarget::ALL {
            match target {
                HookTarget::Present => seen |= 1 << 0,
                HookTarget::IsCandidateDirectFlipCompatible => seen |= 1 << 1,
                HookTarget::IsCandidateOverlayCompatible => seen |= 1 << 2,
            }
        }
        assert_eq!(seen.count_ones() as usize, HookTarget::ALL.len());
        assert_eq!(seen, (1 << HookTarget::ALL.len()) - 1);
    }

    #[cfg(feature = "xtask")]
    #[test]
    fn vtable_index_all_covers_every_variant() {
        let mut seen = 0u8;
        for &target in VtableIndexTarget::ALL {
            match target {
                VtableIndexTarget::GetPhysicalBackBuffer => seen |= 1 << 0,
                VtableIndexTarget::GetD3D11Resource => seen |= 1 << 1,
                VtableIndexTarget::GetOverlaySwapChain => seen |= 1 << 2,
            }
        }
        assert_eq!(seen.count_ones() as usize, VtableIndexTarget::ALL.len());
        assert_eq!(seen, (1u8 << VtableIndexTarget::ALL.len()) - 1);
    }

    #[cfg(feature = "xtask")]
    #[test]
    fn field_offset_all_covers_every_variant() {
        let mut seen = 0u8;
        for &target in FieldOffsetTarget::ALL {
            match target {
                FieldOffsetTarget::HardwareProtected => seen |= 1 << 0,
                FieldOffsetTarget::AdapterLuidLowPart => seen |= 1 << 1,
                FieldOffsetTarget::AdapterLuidHighPart => seen |= 1 << 2,
                FieldOffsetTarget::VidPnTargetId => seen |= 1 << 3,
                FieldOffsetTarget::OverlayMonitorTargetPointer => seen |= 1 << 4,
            }
        }
        assert_eq!(seen.count_ones() as usize, FieldOffsetTarget::ALL.len());
        assert_eq!(seen, (1u8 << FieldOffsetTarget::ALL.len()) - 1);
    }
}
