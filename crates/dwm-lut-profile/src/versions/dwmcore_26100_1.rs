use crate::profile::{AobToken, FieldOffsets, HookProfile, HookSignature, VtableIndices};
use crate::target::HookTarget;
use AobToken::{Exact, Wildcard};

const PRESENT_AOB: &[AobToken] = &[
    Exact(0x40),
    Exact(0x53),
    Exact(0x55),
    Exact(0x56),
    Exact(0x57),
    Exact(0x41),
    Exact(0x54),
    Exact(0x41),
    Exact(0x56),
    Exact(0x41),
    Exact(0x57),
    Exact(0x48),
    Exact(0x81),
    Exact(0xEC),
    Exact(0x80),
    Exact(0x00),
    Exact(0x00),
    Exact(0x00),
    Exact(0x48),
    Exact(0x8B),
    Exact(0x05),
    Wildcard,
    Wildcard,
    Wildcard,
    Wildcard,
    Exact(0x48),
    Exact(0x33),
    Exact(0xC4),
    Exact(0x48),
    Exact(0x89),
    Exact(0x44),
    Exact(0x24),
    Exact(0x78),
    Exact(0x48),
    Exact(0x8B),
    Exact(0xB4),
    Exact(0x24),
    Exact(0xE8),
    Exact(0x00),
    Exact(0x00),
    Exact(0x00),
    Exact(0x48),
    Exact(0x8B),
    Exact(0xD9),
];

const SIGNATURES: &[HookSignature] = &[HookSignature {
    target: HookTarget::Present,
    aob: PRESENT_AOB,
}];

static VTABLE_INDICES: VtableIndices = VtableIndices {
    get_physical_back_buffer: 24,
    get_d3d11_resource: 14,
    get_overlay_swap_chain: 34,
};

static FIELD_OFFSETS: FieldOffsets = FieldOffsets {
    hardware_protected: 0x64,
    adapter_luid_low_part: 0x34,
    adapter_luid_high_part: 0x38,
    vid_pn_target_id: 0x3c,
    overlay_monitor_target_pointer: 0,
};

pub(super) fn profile() -> HookProfile {
    HookProfile {
        signatures: SIGNATURES,
        vtable_indices: &VTABLE_INDICES,
        field_offsets: &FIELD_OFFSETS,
    }
}
