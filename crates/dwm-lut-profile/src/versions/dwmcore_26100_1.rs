use crate::profile::{
    AobToken, ContextToSwapChainPath, HookProfile, HookSignature, MonitorIdentityOffsets,
    SwapChainToResourcePath,
};
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

pub(super) fn profile() -> HookProfile {
    HookProfile {
        signatures: SIGNATURES,
        swap_chain_to_resource_path: SwapChainToResourcePath {
            container_vtable_index: 24,
            resource_vtable_index: 14,
        },
        hardware_protected_offset: 0x64,
        monitor_identity_offsets: MonitorIdentityOffsets {
            adapter_luid_low_offset: 0x34,
            adapter_luid_high_offset: 0x38,
            target_id_offset: 0x3c,
        },
        context_to_swap_chain_path: ContextToSwapChainPath {
            monitor_target_offset: 0,
            swap_chain_vtable_index: 34,
        },
    }
}
