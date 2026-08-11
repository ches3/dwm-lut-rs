use crate::profile::{
    AbiArgument, AbiClass, AobToken, D3D11ResourceAccess, DeviceResourceVirtualBaseAccess,
    HardwareProtectedAccess, HookEntry, HookProfile, MonitorIdentityAccess,
    OverlaySwapChainFromContext,
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
    Exact(0x56),
    Exact(0x41),
    Exact(0x57),
    Exact(0x48),
    Exact(0x81),
    Exact(0xEC),
    Exact(0x88),
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
    Wildcard,
    Exact(0x48),
    Exact(0x8B),
    Exact(0xF1),
];

const IS_CANDIDATE_DIRECT_FLIP_COMPATIBLE_AOB: &[AobToken] = &[
    Exact(0x40),
    Exact(0x55),
    Exact(0x53),
    Exact(0x56),
    Exact(0x57),
    Exact(0x41),
    Exact(0x54),
    Exact(0x41),
    Exact(0x55),
    Exact(0x41),
    Exact(0x56),
    Exact(0x41),
    Exact(0x57),
    Exact(0x48),
    Exact(0x8B),
    Exact(0xEC),
    Exact(0x48),
    Exact(0x83),
    Exact(0xEC),
    Exact(0x68),
    Exact(0x48),
    Exact(0x8B),
    Exact(0x05),
    Wildcard,
    Wildcard,
    Wildcard,
    Wildcard,
];

const IS_CANDIDATE_OVERLAY_COMPATIBLE_AOB: &[AobToken] = &[
    Exact(0x48),
    Exact(0x89),
    Exact(0x5C),
    Exact(0x24),
    Wildcard,
    Exact(0x48),
    Exact(0x89),
    Exact(0x6C),
    Exact(0x24),
    Wildcard,
    Exact(0x48),
    Exact(0x89),
    Exact(0x74),
    Exact(0x24),
    Wildcard,
    Exact(0x57),
    Exact(0x41),
    Exact(0x56),
    Exact(0x41),
    Exact(0x57),
    Exact(0x48),
    Exact(0x83),
    Exact(0xEC),
    Exact(0x30),
    Exact(0x49),
    Exact(0x8B),
    Exact(0x01),
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
        ],
        aob: PRESENT_AOB,
    },
    HookEntry {
        target: HookTarget::IsCandidateDirectFlipCompatible,
        arguments: &[AbiArgument::Wildcard(AbiClass::Integer); 7],
        aob: IS_CANDIDATE_DIRECT_FLIP_COMPATIBLE_AOB,
    },
    HookEntry {
        target: HookTarget::IsCandidateOverlayCompatible,
        arguments: &[AbiArgument::Wildcard(AbiClass::Integer); 7],
        aob: IS_CANDIDATE_OVERLAY_COMPATIBLE_AOB,
    },
];

pub(super) fn profile() -> HookProfile {
    HookProfile {
        hooks: HOOKS,
        overlay_swap_chain_from_context: OverlaySwapChainFromContext {
            overlay_monitor_target_pointer_offset: 0,
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
        hardware_protected: HardwareProtectedAccess::VirtualBaseDeviceResource {
            virtual_base: DeviceResourceVirtualBaseAccess {
                vbptr_offset: 0x8,
                vbtable_entry_offset: 0x0c,
            },
            is_hardware_protected_vtable_index: 3,
        },
    }
}
