use crate::profile::{
    AbiArgument, AbiClass, AobToken, D3D11ResourceAccess, HardwareProtectedAccess, HookEntry,
    HookProfile, MonitorIdentityAccess, OverlaySwapChainFromContext,
};
use crate::target::HookTarget;
use AobToken::{Exact, Wildcard};

const PRESENT_AOB: &[AobToken] = &[
    Exact(0x4C),
    Exact(0x8B),
    Exact(0xDC),
    Exact(0x56),
    Exact(0x41),
    Exact(0x56),
    Exact(0x48),
    Exact(0x81),
    Exact(0xEC),
    Exact(0xA8),
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
    Exact(0x4C),
    Exact(0x8B),
    Exact(0xB4),
    Exact(0x24),
    Exact(0xE8),
    Exact(0x00),
    Exact(0x00),
    Exact(0x00),
    Exact(0x48),
    Exact(0x8B),
    Exact(0xF2),
];

const IS_CANDIDATE_DIRECT_FLIP_COMPATIBLE_AOB: &[AobToken] = &[
    Exact(0x48),
    Exact(0x8B),
    Exact(0xC4),
    Exact(0x48),
    Exact(0x89),
    Exact(0x58),
    Exact(0x08),
    Exact(0x48),
    Exact(0x89),
    Exact(0x68),
    Exact(0x10),
    Exact(0x48),
    Exact(0x89),
    Exact(0x70),
    Exact(0x18),
    Exact(0x48),
    Exact(0x89),
    Exact(0x78),
    Exact(0x20),
    Exact(0x41),
    Exact(0x56),
    Exact(0x48),
    Exact(0x83),
    Exact(0xEC),
    Exact(0x20),
    Exact(0x33),
    Exact(0xDB),
    Exact(0x4D),
    Exact(0x8B),
    Exact(0xF1),
];

const HOOKS: &[HookEntry] = &[
    HookEntry {
        target: HookTarget::Present,
        arguments: &[
            AbiArgument::OverlaySwapChain,
            AbiArgument::Wildcard(AbiClass::Integer),
            AbiArgument::DirtyRects,
            AbiArgument::Wildcard(AbiClass::Integer),
            AbiArgument::Wildcard(AbiClass::Integer),
            AbiArgument::Wildcard(AbiClass::Integer),
        ],
        aob: PRESENT_AOB,
    },
    HookEntry {
        target: HookTarget::IsCandidateDirectFlipCompatible,
        arguments: &[AbiArgument::Wildcard(AbiClass::Integer); 5],
        aob: IS_CANDIDATE_DIRECT_FLIP_COMPATIBLE_AOB,
    },
];

pub(super) fn profile() -> HookProfile {
    HookProfile {
        hooks: HOOKS,
        overlay_swap_chain_from_context: OverlaySwapChainFromContext {
            overlay_monitor_target_pointer_offset: 0,
            get_overlay_swap_chain_vtable_index: 34,
        },
        d3d11_resource: D3D11ResourceAccess::PhysicalBackBuffer {
            get_physical_back_buffer_vtable_index: 24,
            get_d3d11_resource_vtable_index: 17,
        },
        monitor_identity: MonitorIdentityAccess {
            get_display_adapter_luid_vtable_index: 27,
            get_vid_pn_target_id_vtable_index: 29,
        },
        hardware_protected: HardwareProtectedAccess::DirectField { offset: 0x64 },
    }
}
