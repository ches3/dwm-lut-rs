#[cfg(feature = "xtask")]
use crate::profile::AbiArgument;

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

    pub const fn is_required_hook(self) -> bool {
        matches!(self, Self::Present)
    }

    pub const fn is_flip_gate(self) -> bool {
        match self {
            Self::Present => false,
            Self::IsCandidateDirectFlipCompatible | Self::IsCandidateOverlayCompatible => true,
        }
    }

    #[cfg(feature = "xtask")]
    pub const fn extract_spec(self) -> HookExtractSpec {
        match self {
            Self::Present => HookExtractSpec {
                class_name: "COverlayContext",
                method_names: &["Present"],
                return_type: AbiReturnType::HResult,
                required_argument_roles: &[AbiArgument::OverlaySwapChain, AbiArgument::DirtyRects],
            },
            // 22000.x PDB exports the historical misspelling Compatbile.
            Self::IsCandidateDirectFlipCompatible => HookExtractSpec {
                class_name: "COverlayContext",
                method_names: &[
                    "IsCandidateDirectFlipCompatible",
                    "IsCandidateDirectFlipCompatbile",
                ],
                return_type: AbiReturnType::CppBool,
                required_argument_roles: &[],
            },
            Self::IsCandidateOverlayCompatible => HookExtractSpec {
                class_name: "COverlayContext",
                method_names: &[
                    "IsCandidateOverlayCompatible",
                    "IsCandidateOverlayCompatbile",
                ],
                return_type: AbiReturnType::CppBool,
                required_argument_roles: &[],
            },
        }
    }
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookExtractSpec {
    pub class_name: &'static str,
    pub method_names: &'static [&'static str],
    pub return_type: AbiReturnType,
    pub required_argument_roles: &'static [AbiArgument],
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiReturnType {
    HResult,
    CppBool,
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtableView {
    Primary,
    BasePath(&'static [&'static str]),
    DecoratedBasePath(&'static str),
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VtableExtractSpec {
    pub class_name: &'static str,
    pub view: VtableView,
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VirtualMethodTarget {
    GetPhysicalBackBuffer,
    GetD3D11Resource,
    GetCurrentBackBuffer,
    GetRenderTargetView,
    GetDisplayAdapterLuid,
    GetVidPnTargetId,
    IsHardwareProtected,
    GetOverlaySwapChain,
}

#[cfg(feature = "xtask")]
impl VirtualMethodTarget {
    pub const ALL: &[Self] = &[
        Self::GetPhysicalBackBuffer,
        Self::GetD3D11Resource,
        Self::GetCurrentBackBuffer,
        Self::GetRenderTargetView,
        Self::GetDisplayAdapterLuid,
        Self::GetVidPnTargetId,
        Self::IsHardwareProtected,
        Self::GetOverlaySwapChain,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::GetPhysicalBackBuffer => "GetPhysicalBackBuffer",
            Self::GetD3D11Resource => "GetD3D11Resource",
            Self::GetCurrentBackBuffer => "GetCurrentBackBuffer",
            Self::GetRenderTargetView => "GetRenderTargetView",
            Self::GetDisplayAdapterLuid => "GetDisplayAdapterLuid",
            Self::GetVidPnTargetId => "GetVidPnTargetId",
            Self::IsHardwareProtected => "IsHardwareProtected",
            Self::GetOverlaySwapChain => "GetOverlaySwapChain",
        }
    }

    pub const fn extract_spec(self) -> VirtualMethodExtractSpec {
        match self {
            Self::GetPhysicalBackBuffer => VirtualMethodExtractSpec {
                vtables: &[
                    VtableExtractSpec {
                        class_name: "CDDisplaySwapChain",
                        view: VtableView::BasePath(&["IDeviceResource"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacySwapChain",
                        view: VtableView::BasePath(&["IDeviceResource"]),
                    },
                ],
                method_names: &["GetPhysicalBackBuffer"],
                return_type: VirtualMethodReturnType::PointerTo("ISwapChainBuffer"),
            },
            Self::GetD3D11Resource => VirtualMethodExtractSpec {
                vtables: &[
                    VtableExtractSpec {
                        class_name: "CDDisplaySwapChainBuffer",
                        view: VtableView::Primary,
                    },
                    VtableExtractSpec {
                        class_name: "CLegacySwapChainBuffer",
                        view: VtableView::Primary,
                    },
                ],
                method_names: &["GetD3D11Resource"],
                return_type: VirtualMethodReturnType::PointerTo("ID3D11Resource"),
            },
            Self::GetCurrentBackBuffer => VirtualMethodExtractSpec {
                vtables: &[
                    VtableExtractSpec {
                        class_name: "CDDisplaySwapChain",
                        view: VtableView::DecoratedBasePath(
                            "?$CMILCOMBaseT@VIOverlaySwapChain@@@@",
                        ),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacySwapChain",
                        view: VtableView::BasePath(&["ILegacySwapChain"]),
                    },
                    VtableExtractSpec {
                        class_name: "CDDisplaySwapChain",
                        view: VtableView::BasePath(&["IOverlaySwapChain"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacySwapChain",
                        view: VtableView::BasePath(&["IOverlaySwapChain"]),
                    },
                ],
                method_names: &["GetCurrentBackBuffer"],
                return_type: VirtualMethodReturnType::PointerTo("IDeviceTarget"),
            },
            Self::GetRenderTargetView => VirtualMethodExtractSpec {
                vtables: &[VtableExtractSpec {
                    class_name: "CDeviceTextureTarget",
                    view: VtableView::BasePath(&["IDeviceTarget"]),
                }],
                method_names: &["GetRenderTargetView"],
                return_type: VirtualMethodReturnType::PointerTo("ID3D11RenderTargetView"),
            },
            Self::GetDisplayAdapterLuid => VirtualMethodExtractSpec {
                vtables: &[
                    VtableExtractSpec {
                        class_name: "CDDisplaySwapChain",
                        view: VtableView::BasePath(&["IDeviceResource"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacySwapChain",
                        view: VtableView::BasePath(&["IDeviceResource"]),
                    },
                    VtableExtractSpec {
                        class_name: "CDDisplaySwapChain",
                        view: VtableView::DecoratedBasePath(
                            "?$CMILCOMBaseT@VIOverlaySwapChain@@@@",
                        ),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacySwapChain",
                        view: VtableView::BasePath(&["ILegacySwapChain"]),
                    },
                    VtableExtractSpec {
                        class_name: "CDDisplaySwapChain",
                        view: VtableView::BasePath(&["IOverlaySwapChain"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacySwapChain",
                        view: VtableView::BasePath(&["IOverlaySwapChain"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacyStereoSwapChain",
                        view: VtableView::BasePath(&["IOverlaySwapChain"]),
                    },
                    VtableExtractSpec {
                        class_name: "CConversionSwapChain",
                        view: VtableView::BasePath(&["IOverlaySwapChain"]),
                    },
                ],
                method_names: &["GetDisplayAdapterLuid"],
                return_type: VirtualMethodReturnType::SretValue("_LUID"),
            },
            Self::GetVidPnTargetId => VirtualMethodExtractSpec {
                vtables: &[
                    VtableExtractSpec {
                        class_name: "CDDisplaySwapChain",
                        view: VtableView::BasePath(&["IDeviceResource"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacySwapChain",
                        view: VtableView::BasePath(&["IDeviceResource"]),
                    },
                    VtableExtractSpec {
                        class_name: "CDDisplaySwapChain",
                        view: VtableView::DecoratedBasePath(
                            "?$CMILCOMBaseT@VIOverlaySwapChain@@@@",
                        ),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacySwapChain",
                        view: VtableView::BasePath(&["ILegacySwapChain"]),
                    },
                    VtableExtractSpec {
                        class_name: "CDDisplaySwapChain",
                        view: VtableView::BasePath(&["IOverlaySwapChain"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacySwapChain",
                        view: VtableView::BasePath(&["IOverlaySwapChain"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacyStereoSwapChain",
                        view: VtableView::BasePath(&["IOverlaySwapChain"]),
                    },
                    VtableExtractSpec {
                        class_name: "CConversionSwapChain",
                        view: VtableView::BasePath(&["IOverlaySwapChain"]),
                    },
                ],
                method_names: &["GetVidPnTargetId"],
                return_type: VirtualMethodReturnType::Unsigned32,
            },
            Self::IsHardwareProtected => VirtualMethodExtractSpec {
                vtables: &[
                    VtableExtractSpec {
                        class_name: "CDDisplaySwapChain",
                        view: VtableView::BasePath(&["IDeviceResource"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacySwapChain",
                        view: VtableView::BasePath(&["IDeviceResource"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacyStereoSwapChain",
                        view: VtableView::BasePath(&["IDeviceResource"]),
                    },
                    VtableExtractSpec {
                        class_name: "CConversionSwapChain",
                        view: VtableView::BasePath(&["IDeviceResource"]),
                    },
                ],
                method_names: &["IsHardwareProtected"],
                return_type: VirtualMethodReturnType::CppBool,
            },
            Self::GetOverlaySwapChain => VirtualMethodExtractSpec {
                vtables: &[
                    VtableExtractSpec {
                        class_name: "CDDisplayRenderTarget",
                        view: VtableView::BasePath(&["IPixelFormat", "IOverlayMonitorTarget"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacyRenderTarget",
                        view: VtableView::BasePath(&["IPixelFormat", "IOverlayMonitorTarget"]),
                    },
                    VtableExtractSpec {
                        class_name: "CDDisplayRenderTarget",
                        view: VtableView::BasePath(&["IOverlayMonitorTarget"]),
                    },
                    VtableExtractSpec {
                        class_name: "CLegacyRenderTarget",
                        view: VtableView::BasePath(&["IOverlayMonitorTarget"]),
                    },
                ],
                method_names: &["GetOverlaySwapChain"],
                return_type: VirtualMethodReturnType::PointerTo("IOverlaySwapChain"),
            },
        }
    }
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualMethodExtractSpec {
    pub vtables: &'static [VtableExtractSpec],
    pub method_names: &'static [&'static str],
    pub return_type: VirtualMethodReturnType,
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualMethodReturnType {
    PointerTo(&'static str),
    SretValue(&'static str),
    Unsigned32,
    CppBool,
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VirtualBaseTarget {
    DeviceResourceFromOverlaySwapChain,
}

#[cfg(feature = "xtask")]
impl VirtualBaseTarget {
    pub const ALL: &[Self] = &[Self::DeviceResourceFromOverlaySwapChain];

    pub const fn label(self) -> &'static str {
        match self {
            Self::DeviceResourceFromOverlaySwapChain => "IOverlaySwapChain -> IDeviceResource",
        }
    }

    pub const fn extract_spec(self) -> VirtualBaseExtractSpec {
        match self {
            Self::DeviceResourceFromOverlaySwapChain => VirtualBaseExtractSpec {
                classes: &[
                    VirtualBaseClassExtractSpec {
                        class_name: "CDDisplaySwapChain",
                        source_vftable_views: &[
                            VtableView::BasePath(&["IOverlaySwapChain"]),
                            VtableView::DecoratedBasePath("?$CMILCOMBaseT@VIOverlaySwapChain@@@@"),
                        ],
                        source_vbtable_views: &[VtableView::BasePath(&["ISwapChain"])],
                        target_virtual_base: VtableView::BasePath(&["IDeviceResource"]),
                    },
                    VirtualBaseClassExtractSpec {
                        class_name: "CLegacySwapChain",
                        source_vftable_views: &[
                            VtableView::BasePath(&["IOverlaySwapChain"]),
                            VtableView::BasePath(&["ILegacySwapChain"]),
                        ],
                        source_vbtable_views: &[
                            VtableView::BasePath(&["ISwapChain"]),
                            VtableView::BasePath(&["ISwapChain", "COverlaySwapChainBase"]),
                        ],
                        target_virtual_base: VtableView::BasePath(&["IDeviceResource"]),
                    },
                ],
            },
        }
    }
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualBaseExtractSpec {
    pub classes: &'static [VirtualBaseClassExtractSpec],
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualBaseClassExtractSpec {
    pub class_name: &'static str,
    pub source_vftable_views: &'static [VtableView],
    pub source_vbtable_views: &'static [VtableView],
    pub target_virtual_base: VtableView,
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FieldOffsetTarget {
    HardwareProtected,
    OverlayMonitorTargetPointer,
}

#[cfg(feature = "xtask")]
impl FieldOffsetTarget {
    pub const ALL: &[Self] = &[Self::HardwareProtected, Self::OverlayMonitorTargetPointer];

    pub const fn label(self) -> &'static str {
        match self {
            Self::HardwareProtected => "HardwareProtected",
            Self::OverlayMonitorTargetPointer => "IOverlayMonitorTarget*",
        }
    }

    pub const fn extract_spec(self) -> FieldOffsetExtractSpec {
        match self {
            Self::HardwareProtected => FieldOffsetExtractSpec::GetterDisp {
                class_name: "COverlaySwapChain",
                method_names: &["IsHardwareProtected"],
                derived_add: None,
                width: DirectFieldWidth::U8,
            },
            Self::OverlayMonitorTargetPointer => FieldOffsetExtractSpec::CtorFirstParamStore {
                class_name: "COverlayContext",
                first_parameter: ParameterSpec::PointerToClass("IOverlayMonitorTarget"),
            },
        }
    }
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectFieldWidth {
    U8,
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldOffsetExtractSpec {
    GetterDisp {
        class_name: &'static str,
        method_names: &'static [&'static str],
        derived_add: Option<usize>,
        width: DirectFieldWidth,
    },
    CtorFirstParamStore {
        class_name: &'static str,
        first_parameter: ParameterSpec,
    },
}

#[cfg(feature = "xtask")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterSpec {
    PointerToClass(&'static str),
}

#[cfg(test)]
mod tests {
    use super::HookTarget;

    #[cfg(feature = "xtask")]
    use super::{FieldOffsetTarget, VirtualMethodTarget};

    #[cfg(feature = "xtask")]
    use super::VirtualBaseTarget;

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
    fn virtual_method_all_covers_every_variant() {
        let mut seen = 0u16;
        for &target in VirtualMethodTarget::ALL {
            match target {
                VirtualMethodTarget::GetPhysicalBackBuffer => seen |= 1 << 0,
                VirtualMethodTarget::GetD3D11Resource => seen |= 1 << 1,
                VirtualMethodTarget::GetCurrentBackBuffer => seen |= 1 << 2,
                VirtualMethodTarget::GetRenderTargetView => seen |= 1 << 3,
                VirtualMethodTarget::GetDisplayAdapterLuid => seen |= 1 << 4,
                VirtualMethodTarget::GetVidPnTargetId => seen |= 1 << 5,
                VirtualMethodTarget::IsHardwareProtected => seen |= 1 << 6,
                VirtualMethodTarget::GetOverlaySwapChain => seen |= 1 << 7,
            }
        }
        assert_eq!(seen.count_ones() as usize, VirtualMethodTarget::ALL.len());
        assert_eq!(seen, (1u16 << VirtualMethodTarget::ALL.len()) - 1);
    }

    #[cfg(feature = "xtask")]
    #[test]
    fn field_offset_all_covers_every_variant() {
        let mut seen = 0u8;
        for &target in FieldOffsetTarget::ALL {
            match target {
                FieldOffsetTarget::HardwareProtected => seen |= 1 << 0,
                FieldOffsetTarget::OverlayMonitorTargetPointer => seen |= 1 << 1,
            }
        }
        assert_eq!(seen.count_ones() as usize, FieldOffsetTarget::ALL.len());
        assert_eq!(seen, (1u8 << FieldOffsetTarget::ALL.len()) - 1);
    }

    #[cfg(feature = "xtask")]
    #[test]
    fn virtual_base_all_covers_every_variant() {
        let mut seen = 0u8;
        for &target in VirtualBaseTarget::ALL {
            match target {
                VirtualBaseTarget::DeviceResourceFromOverlaySwapChain => seen |= 1 << 0,
            }
        }
        assert_eq!(seen.count_ones() as usize, VirtualBaseTarget::ALL.len());
        assert_eq!(seen, (1u8 << VirtualBaseTarget::ALL.len()) - 1);
    }
}
