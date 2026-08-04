use crate::target::HookTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapChainVtablePath {
    pub container_vtable_index: usize,
    pub resource_vtable_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorIdentityOffsets {
    pub adapter_luid_low_offset: usize,
    pub adapter_luid_high_offset: usize,
    pub target_id_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AobToken {
    Exact(u8),
    Wildcard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureLocator {
    Aob {
        tokens: &'static [AobToken],
    },
    RipRelativeGlobalAob {
        tokens: &'static [AobToken],
        displacement_offset: usize,
        instruction_size: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookSignature {
    pub target: HookTarget,
    pub locator: SignatureLocator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookProfile {
    pub signatures: &'static [HookSignature],
    pub swap_chain: SwapChainVtablePath,
    pub hardware_protected_offset: usize,
    pub monitor_identity: MonitorIdentityOffsets,
}
