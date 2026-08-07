use crate::target::HookTarget;

#[cfg(feature = "xtask")]
use crate::target::{FieldOffsetTarget, VtableIndexTarget};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct D3D11ResourcePath {
    pub get_physical_back_buffer_vtable_index: usize,
    pub get_d3d11_resource_vtable_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlaySwapChainIdentityOffsets {
    pub adapter_luid_low_part_offset: usize,
    pub adapter_luid_high_part_offset: usize,
    pub vid_pn_target_id_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlaySwapChainPath {
    pub overlay_monitor_target_pointer_offset: usize,
    pub get_overlay_swap_chain_vtable_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AobToken {
    Exact(u8),
    Wildcard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSignature {
    pub target: HookTarget,
    pub aob: &'static [AobToken],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VtableIndices {
    pub get_physical_back_buffer: usize,
    pub get_d3d11_resource: usize,
    pub get_overlay_swap_chain: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldOffsets {
    pub hardware_protected: usize,
    pub adapter_luid_low_part: usize,
    pub adapter_luid_high_part: usize,
    pub vid_pn_target_id: usize,
    pub overlay_monitor_target_pointer: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookProfile {
    pub signatures: &'static [HookSignature],
    pub vtable_indices: &'static VtableIndices,
    pub field_offsets: &'static FieldOffsets,
}

impl HookProfile {
    #[cfg(feature = "xtask")]
    pub const fn vtable_index(self, target: VtableIndexTarget) -> usize {
        match target {
            VtableIndexTarget::GetPhysicalBackBuffer => {
                self.vtable_indices.get_physical_back_buffer
            }
            VtableIndexTarget::GetD3D11Resource => self.vtable_indices.get_d3d11_resource,
            VtableIndexTarget::GetOverlaySwapChain => self.vtable_indices.get_overlay_swap_chain,
        }
    }

    #[cfg(feature = "xtask")]
    pub const fn field_offset(self, target: FieldOffsetTarget) -> usize {
        match target {
            FieldOffsetTarget::HardwareProtected => self.field_offsets.hardware_protected,
            FieldOffsetTarget::AdapterLuidLowPart => self.field_offsets.adapter_luid_low_part,
            FieldOffsetTarget::AdapterLuidHighPart => self.field_offsets.adapter_luid_high_part,
            FieldOffsetTarget::VidPnTargetId => self.field_offsets.vid_pn_target_id,
            FieldOffsetTarget::OverlayMonitorTargetPointer => {
                self.field_offsets.overlay_monitor_target_pointer
            }
        }
    }

    pub fn d3d11_resource_path(self) -> D3D11ResourcePath {
        D3D11ResourcePath {
            get_physical_back_buffer_vtable_index: self.vtable_indices.get_physical_back_buffer,
            get_d3d11_resource_vtable_index: self.vtable_indices.get_d3d11_resource,
        }
    }

    pub fn hardware_protected_offset(self) -> usize {
        self.field_offsets.hardware_protected
    }

    pub fn overlay_swap_chain_identity_offsets(self) -> OverlaySwapChainIdentityOffsets {
        OverlaySwapChainIdentityOffsets {
            adapter_luid_low_part_offset: self.field_offsets.adapter_luid_low_part,
            adapter_luid_high_part_offset: self.field_offsets.adapter_luid_high_part,
            vid_pn_target_id_offset: self.field_offsets.vid_pn_target_id,
        }
    }

    pub fn overlay_swap_chain_path(self) -> OverlaySwapChainPath {
        OverlaySwapChainPath {
            overlay_monitor_target_pointer_offset: self
                .field_offsets
                .overlay_monitor_target_pointer,
            get_overlay_swap_chain_vtable_index: self.vtable_indices.get_overlay_swap_chain,
        }
    }
}
