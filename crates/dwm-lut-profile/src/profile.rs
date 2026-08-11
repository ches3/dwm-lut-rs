use crate::target::HookTarget;

#[cfg(feature = "xtask")]
use crate::access::{ProfileAccess, ProfileAccessField, ProfileAccessTarget};
#[cfg(feature = "xtask")]
use crate::target::{FieldOffsetTarget, VirtualBaseTarget, VirtualMethodTarget};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlaySwapChainFromContext {
    pub overlay_monitor_target_pointer_offset: i32,
    pub get_overlay_swap_chain_vtable_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum D3D11ResourceAccess {
    PhysicalBackBuffer {
        get_physical_back_buffer_vtable_index: usize,
        get_d3d11_resource_vtable_index: usize,
    },
    CurrentBackBufferRenderTargetView {
        get_current_back_buffer_vtable_index: usize,
        get_render_target_view_vtable_index: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorIdentityAccess {
    pub get_display_adapter_luid_vtable_index: usize,
    pub get_vid_pn_target_id_vtable_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceResourceVirtualBaseAccess {
    pub vbptr_offset: i32,
    pub vbtable_entry_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareProtectedAccess {
    DirectField {
        offset: i32,
    },
    VirtualBaseDeviceResource {
        virtual_base: DeviceResourceVirtualBaseAccess,
        is_hardware_protected_vtable_index: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AobToken {
    Exact(u8),
    Wildcard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiClass {
    Integer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiObjectType {
    Named(&'static str),
    StdVector(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiType {
    Pointer {
        pointee: AbiObjectType,
        is_const: bool,
    },
    Reference {
        referent: AbiObjectType,
        is_const: bool,
    },
}

impl AbiType {
    pub const fn class(self) -> AbiClass {
        match self {
            Self::Pointer { .. } | Self::Reference { .. } => AbiClass::Integer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiArgument {
    OverlaySwapChain,
    DirtyRects,
    Wildcard(AbiClass),
}

impl AbiArgument {
    pub const fn class(self) -> AbiClass {
        match self {
            Self::OverlaySwapChain | Self::DirtyRects => AbiClass::Integer,
            Self::Wildcard(class) => class,
        }
    }

    pub const fn exact_type(self) -> Option<AbiType> {
        match self {
            Self::OverlaySwapChain => Some(AbiType::Pointer {
                pointee: AbiObjectType::Named("IOverlaySwapChain"),
                is_const: false,
            }),
            Self::DirtyRects => Some(AbiType::Reference {
                referent: AbiObjectType::StdVector("tagRECT"),
                is_const: true,
            }),
            Self::Wildcard(_) => None,
        }
    }

    #[cfg(feature = "xtask")]
    pub const fn extract_label(self) -> &'static str {
        match self {
            Self::OverlaySwapChain => "OverlaySwapChain",
            Self::DirtyRects => "DirtyRects",
            Self::Wildcard(AbiClass::Integer) => "AnyInteger",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookEntry {
    pub target: HookTarget,
    pub arguments: &'static [AbiArgument],
    pub aob: &'static [AobToken],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookProfile {
    pub hooks: &'static [HookEntry],
    pub overlay_swap_chain_from_context: OverlaySwapChainFromContext,
    pub d3d11_resource: D3D11ResourceAccess,
    pub monitor_identity: MonitorIdentityAccess,
    pub hardware_protected: HardwareProtectedAccess,
}

#[cfg(feature = "xtask")]
impl HookProfile {
    pub fn accesses(self) -> Vec<ProfileAccess> {
        let d3d11_resource = match self.d3d11_resource {
            D3D11ResourceAccess::PhysicalBackBuffer {
                get_physical_back_buffer_vtable_index,
                get_d3d11_resource_vtable_index,
            } => vec![
                method_index(
                    VirtualMethodTarget::GetPhysicalBackBuffer,
                    get_physical_back_buffer_vtable_index,
                ),
                method_index(
                    VirtualMethodTarget::GetD3D11Resource,
                    get_d3d11_resource_vtable_index,
                ),
            ],
            D3D11ResourceAccess::CurrentBackBufferRenderTargetView {
                get_current_back_buffer_vtable_index,
                get_render_target_view_vtable_index,
            } => vec![
                method_index(
                    VirtualMethodTarget::GetCurrentBackBuffer,
                    get_current_back_buffer_vtable_index,
                ),
                method_index(
                    VirtualMethodTarget::GetRenderTargetView,
                    get_render_target_view_vtable_index,
                ),
            ],
        };

        let MonitorIdentityAccess {
            get_display_adapter_luid_vtable_index,
            get_vid_pn_target_id_vtable_index,
        } = self.monitor_identity;
        let monitor_identity = vec![
            method_index(
                VirtualMethodTarget::GetDisplayAdapterLuid,
                get_display_adapter_luid_vtable_index,
            ),
            method_index(
                VirtualMethodTarget::GetVidPnTargetId,
                get_vid_pn_target_id_vtable_index,
            ),
        ];

        let hardware_protected = match self.hardware_protected {
            HardwareProtectedAccess::DirectField { offset } => {
                vec![ProfileAccessField::FieldOffset {
                    target: FieldOffsetTarget::HardwareProtected,
                    offset,
                }]
            }
            HardwareProtectedAccess::VirtualBaseDeviceResource {
                virtual_base,
                is_hardware_protected_vtable_index,
            } => vec![
                ProfileAccessField::VbptrOffset {
                    target: VirtualBaseTarget::DeviceResourceFromOverlaySwapChain,
                    offset: virtual_base.vbptr_offset,
                },
                ProfileAccessField::VbtableEntryOffset {
                    target: VirtualBaseTarget::DeviceResourceFromOverlaySwapChain,
                    offset: virtual_base.vbtable_entry_offset,
                },
                method_index(
                    VirtualMethodTarget::IsHardwareProtected,
                    is_hardware_protected_vtable_index,
                ),
            ],
        };

        let overlay_swap_chain_from_context = vec![
            ProfileAccessField::FieldOffset {
                target: FieldOffsetTarget::OverlayMonitorTargetPointer,
                offset: self
                    .overlay_swap_chain_from_context
                    .overlay_monitor_target_pointer_offset,
            },
            method_index(
                VirtualMethodTarget::GetOverlaySwapChain,
                self.overlay_swap_chain_from_context
                    .get_overlay_swap_chain_vtable_index,
            ),
        ];

        vec![
            ProfileAccess {
                target: ProfileAccessTarget::D3D11Resource,
                fields: d3d11_resource,
            },
            ProfileAccess {
                target: ProfileAccessTarget::MonitorIdentity,
                fields: monitor_identity,
            },
            ProfileAccess {
                target: ProfileAccessTarget::HardwareProtected,
                fields: hardware_protected,
            },
            ProfileAccess {
                target: ProfileAccessTarget::OverlaySwapChainFromContext,
                fields: overlay_swap_chain_from_context,
            },
        ]
    }
}

#[cfg(feature = "xtask")]
const fn method_index(target: VirtualMethodTarget, index: usize) -> ProfileAccessField {
    ProfileAccessField::VirtualMethodIndex { target, index }
}

#[cfg(all(test, feature = "xtask"))]
mod tests {
    use super::{
        D3D11ResourceAccess, DeviceResourceVirtualBaseAccess, HardwareProtectedAccess, HookProfile,
        MonitorIdentityAccess, OverlaySwapChainFromContext,
    };
    use crate::access::{ProfileAccessField, ProfileAccessTarget};
    use crate::target::{FieldOffsetTarget, VirtualBaseTarget, VirtualMethodTarget};

    fn current_back_buffer_profile() -> HookProfile {
        HookProfile {
            hooks: &[],
            overlay_swap_chain_from_context: OverlaySwapChainFromContext {
                overlay_monitor_target_pointer_offset: 4,
                get_overlay_swap_chain_vtable_index: 22,
            },
            d3d11_resource: D3D11ResourceAccess::CurrentBackBufferRenderTargetView {
                get_current_back_buffer_vtable_index: 0,
                get_render_target_view_vtable_index: 7,
            },
            monitor_identity: MonitorIdentityAccess {
                get_display_adapter_luid_vtable_index: 6,
                get_vid_pn_target_id_vtable_index: 8,
            },
            hardware_protected: HardwareProtectedAccess::DirectField { offset: 0x6c },
        }
    }

    fn virtual_access_profile() -> HookProfile {
        HookProfile {
            monitor_identity: MonitorIdentityAccess {
                get_display_adapter_luid_vtable_index: 6,
                get_vid_pn_target_id_vtable_index: 8,
            },
            hardware_protected: HardwareProtectedAccess::VirtualBaseDeviceResource {
                virtual_base: DeviceResourceVirtualBaseAccess {
                    vbptr_offset: 8,
                    vbtable_entry_offset: 0x0c,
                },
                is_hardware_protected_vtable_index: 3,
            },
            ..current_back_buffer_profile()
        }
    }

    #[test]
    fn current_back_buffer_profile_lists_selected_access_fields() {
        let accesses = current_back_buffer_profile().accesses();

        assert_eq!(
            accesses
                .iter()
                .find(|access| access.target == ProfileAccessTarget::D3D11Resource)
                .expect("d3d11 resource access")
                .fields,
            vec![
                ProfileAccessField::VirtualMethodIndex {
                    target: VirtualMethodTarget::GetCurrentBackBuffer,
                    index: 0,
                },
                ProfileAccessField::VirtualMethodIndex {
                    target: VirtualMethodTarget::GetRenderTargetView,
                    index: 7,
                },
            ]
        );
        assert_eq!(
            accesses
                .iter()
                .find(|access| access.target == ProfileAccessTarget::MonitorIdentity)
                .expect("monitor identity access")
                .fields,
            vec![
                ProfileAccessField::VirtualMethodIndex {
                    target: VirtualMethodTarget::GetDisplayAdapterLuid,
                    index: 6,
                },
                ProfileAccessField::VirtualMethodIndex {
                    target: VirtualMethodTarget::GetVidPnTargetId,
                    index: 8,
                },
            ]
        );
        assert_eq!(
            accesses
                .iter()
                .find(|access| {
                    access.target == ProfileAccessTarget::OverlaySwapChainFromContext
                })
                .expect("overlay swap chain from context access")
                .fields,
            vec![
                ProfileAccessField::FieldOffset {
                    target: FieldOffsetTarget::OverlayMonitorTargetPointer,
                    offset: 4,
                },
                ProfileAccessField::VirtualMethodIndex {
                    target: VirtualMethodTarget::GetOverlaySwapChain,
                    index: 22,
                },
            ]
        );
    }

    #[test]
    fn virtual_access_profile_lists_selected_hardware_fields() {
        let accesses = virtual_access_profile().accesses();

        assert_eq!(
            accesses
                .iter()
                .find(|access| access.target == ProfileAccessTarget::HardwareProtected)
                .expect("hardware protected access")
                .fields,
            vec![
                ProfileAccessField::VbptrOffset {
                    target: VirtualBaseTarget::DeviceResourceFromOverlaySwapChain,
                    offset: 8,
                },
                ProfileAccessField::VbtableEntryOffset {
                    target: VirtualBaseTarget::DeviceResourceFromOverlaySwapChain,
                    offset: 0x0c,
                },
                ProfileAccessField::VirtualMethodIndex {
                    target: VirtualMethodTarget::IsHardwareProtected,
                    index: 3,
                },
            ]
        );
    }

    #[test]
    fn physical_back_buffer_profile_lists_physical_and_direct_field_accesses() {
        let profile = HookProfile {
            d3d11_resource: D3D11ResourceAccess::PhysicalBackBuffer {
                get_physical_back_buffer_vtable_index: 24,
                get_d3d11_resource_vtable_index: 19,
            },
            ..current_back_buffer_profile()
        };
        let accesses = profile.accesses();

        assert_eq!(
            accesses
                .iter()
                .find(|access| access.target == ProfileAccessTarget::D3D11Resource)
                .expect("d3d11 resource access")
                .fields,
            vec![
                ProfileAccessField::VirtualMethodIndex {
                    target: VirtualMethodTarget::GetPhysicalBackBuffer,
                    index: 24,
                },
                ProfileAccessField::VirtualMethodIndex {
                    target: VirtualMethodTarget::GetD3D11Resource,
                    index: 19,
                },
            ]
        );
        assert_eq!(
            accesses
                .iter()
                .find(|access| access.target == ProfileAccessTarget::HardwareProtected)
                .expect("hardware protected access")
                .fields,
            vec![ProfileAccessField::FieldOffset {
                target: FieldOffsetTarget::HardwareProtected,
                offset: 0x6c,
            }]
        );
    }

    #[test]
    fn expected_access_order_matches_profile_output_order() {
        let targets = current_back_buffer_profile()
            .accesses()
            .into_iter()
            .map(|access| access.target)
            .collect::<Vec<_>>();

        assert_eq!(targets, ProfileAccessTarget::ALL);
    }

    #[test]
    fn compiled_profiles_select_declared_access_methods() {
        for supported in crate::SUPPORTED_BUILDS {
            for revision in supported.profiles {
                for access in (revision.profile)().accesses() {
                    let actual = access
                        .fields
                        .iter()
                        .copied()
                        .map(ProfileAccessField::target)
                        .collect::<Vec<_>>();

                    assert!(
                        access
                            .target
                            .extract_spec()
                            .methods
                            .iter()
                            .any(|method| method.fields == actual.as_slice()),
                        "{}.{} {} does not match a declared access method",
                        supported.build,
                        revision.min_revision,
                        access.target.label(),
                    );
                }
            }
        }
    }
}
