use crate::target::{FieldOffsetTarget, VirtualBaseTarget, VirtualMethodTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileAccess {
    pub target: ProfileAccessTarget,
    pub fields: Vec<ProfileAccessField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileAccessField {
    VirtualMethodIndex {
        target: VirtualMethodTarget,
        index: usize,
    },
    FieldOffset {
        target: FieldOffsetTarget,
        offset: i32,
    },
    VbptrOffset {
        target: VirtualBaseTarget,
        offset: i32,
    },
    VbtableEntryOffset {
        target: VirtualBaseTarget,
        offset: usize,
    },
}

impl ProfileAccessField {
    pub const fn target(self) -> AccessFieldTarget {
        match self {
            Self::VirtualMethodIndex { target, .. } => AccessFieldTarget::VirtualMethod(target),
            Self::FieldOffset { target, .. } => AccessFieldTarget::FieldOffset(target),
            Self::VbptrOffset { target, .. } => AccessFieldTarget::VirtualBaseOffset {
                target,
                component: VirtualBaseComponent::VbptrOffset,
            },
            Self::VbtableEntryOffset { target, .. } => AccessFieldTarget::VirtualBaseOffset {
                target,
                component: VirtualBaseComponent::VbtableEntryOffset,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileAccessTarget {
    D3D11Resource,
    MonitorIdentity,
    HardwareProtected,
    OverlaySwapChainFromContext,
}

impl ProfileAccessTarget {
    pub const ALL: &[Self] = &[
        Self::D3D11Resource,
        Self::MonitorIdentity,
        Self::HardwareProtected,
        Self::OverlaySwapChainFromContext,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::D3D11Resource => "D3D11ResourceAccess",
            Self::MonitorIdentity => "MonitorIdentityAccess",
            Self::HardwareProtected => "HardwareProtectedAccess",
            Self::OverlaySwapChainFromContext => "OverlaySwapChainFromContext",
        }
    }

    pub const fn extract_spec(self) -> AccessExtractSpec {
        match self {
            Self::D3D11Resource => AccessExtractSpec {
                methods: &[
                    AccessMethodExtractSpec {
                        fields: &[
                            AccessFieldTarget::VirtualMethod(
                                VirtualMethodTarget::GetPhysicalBackBuffer,
                            ),
                            AccessFieldTarget::VirtualMethod(VirtualMethodTarget::GetD3D11Resource),
                        ],
                    },
                    AccessMethodExtractSpec {
                        fields: &[
                            AccessFieldTarget::VirtualMethod(
                                VirtualMethodTarget::GetCurrentBackBuffer,
                            ),
                            AccessFieldTarget::VirtualMethod(
                                VirtualMethodTarget::GetRenderTargetView,
                            ),
                        ],
                    },
                ],
            },
            Self::MonitorIdentity => AccessExtractSpec {
                methods: &[AccessMethodExtractSpec {
                    fields: &[
                        AccessFieldTarget::VirtualMethod(
                            VirtualMethodTarget::GetDisplayAdapterLuid,
                        ),
                        AccessFieldTarget::VirtualMethod(VirtualMethodTarget::GetVidPnTargetId),
                    ],
                }],
            },
            Self::HardwareProtected => AccessExtractSpec {
                methods: &[
                    AccessMethodExtractSpec {
                        fields: &[AccessFieldTarget::FieldOffset(
                            FieldOffsetTarget::HardwareProtected,
                        )],
                    },
                    AccessMethodExtractSpec {
                        fields: &[
                            AccessFieldTarget::VirtualBaseOffset {
                                target: VirtualBaseTarget::DeviceResourceFromOverlaySwapChain,
                                component: VirtualBaseComponent::VbptrOffset,
                            },
                            AccessFieldTarget::VirtualBaseOffset {
                                target: VirtualBaseTarget::DeviceResourceFromOverlaySwapChain,
                                component: VirtualBaseComponent::VbtableEntryOffset,
                            },
                            AccessFieldTarget::VirtualMethod(
                                VirtualMethodTarget::IsHardwareProtected,
                            ),
                        ],
                    },
                ],
            },
            Self::OverlaySwapChainFromContext => AccessExtractSpec {
                methods: &[AccessMethodExtractSpec {
                    fields: &[
                        AccessFieldTarget::FieldOffset(
                            FieldOffsetTarget::OverlayMonitorTargetPointer,
                        ),
                        AccessFieldTarget::VirtualMethod(VirtualMethodTarget::GetOverlaySwapChain),
                    ],
                }],
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessExtractSpec {
    pub methods: &'static [AccessMethodExtractSpec],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessMethodExtractSpec {
    pub fields: &'static [AccessFieldTarget],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessFieldTarget {
    VirtualMethod(VirtualMethodTarget),
    FieldOffset(FieldOffsetTarget),
    VirtualBaseOffset {
        target: VirtualBaseTarget,
        component: VirtualBaseComponent,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualBaseComponent {
    VbptrOffset,
    VbtableEntryOffset,
}

#[cfg(test)]
mod tests {
    use super::{AccessFieldTarget, ProfileAccessTarget, VirtualBaseComponent};
    use crate::target::{FieldOffsetTarget, VirtualBaseTarget, VirtualMethodTarget};

    #[test]
    fn all_lists_access_targets_in_report_order() {
        assert_eq!(
            ProfileAccessTarget::ALL,
            &[
                ProfileAccessTarget::D3D11Resource,
                ProfileAccessTarget::MonitorIdentity,
                ProfileAccessTarget::HardwareProtected,
                ProfileAccessTarget::OverlaySwapChainFromContext,
            ]
        );
    }

    #[test]
    fn d3d11_methods_are_in_preference_order() {
        let spec = ProfileAccessTarget::D3D11Resource.extract_spec();

        assert_eq!(spec.methods.len(), 2);
        assert_eq!(
            spec.methods[0].fields,
            &[
                AccessFieldTarget::VirtualMethod(VirtualMethodTarget::GetPhysicalBackBuffer),
                AccessFieldTarget::VirtualMethod(VirtualMethodTarget::GetD3D11Resource),
            ]
        );
        assert_eq!(
            spec.methods[1].fields,
            &[
                AccessFieldTarget::VirtualMethod(VirtualMethodTarget::GetCurrentBackBuffer),
                AccessFieldTarget::VirtualMethod(VirtualMethodTarget::GetRenderTargetView),
            ]
        );
    }

    #[test]
    fn hardware_protected_methods_are_in_preference_order() {
        let spec = ProfileAccessTarget::HardwareProtected.extract_spec();
        let virtual_base_target = VirtualBaseTarget::DeviceResourceFromOverlaySwapChain;

        assert_eq!(spec.methods.len(), 2);
        assert_eq!(
            spec.methods[0].fields,
            &[AccessFieldTarget::FieldOffset(
                FieldOffsetTarget::HardwareProtected,
            )]
        );
        assert_eq!(
            spec.methods[1].fields,
            &[
                AccessFieldTarget::VirtualBaseOffset {
                    target: virtual_base_target,
                    component: VirtualBaseComponent::VbptrOffset,
                },
                AccessFieldTarget::VirtualBaseOffset {
                    target: virtual_base_target,
                    component: VirtualBaseComponent::VbtableEntryOffset,
                },
                AccessFieldTarget::VirtualMethod(VirtualMethodTarget::IsHardwareProtected),
            ]
        );
    }
}
