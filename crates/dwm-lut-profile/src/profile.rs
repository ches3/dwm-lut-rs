use crate::target::HookTarget;

/// `IOverlaySwapChain` → `ID3D11Resource`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapChainToResourcePath {
    /// `GetPhysicalBackBuffer`
    pub container_vtable_index: usize,
    /// `GetD3D11Resource`
    pub resource_vtable_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorIdentityOffsets {
    pub adapter_luid_low_offset: usize,
    pub adapter_luid_high_offset: usize,
    pub target_id_offset: usize,
}

/// `COverlayContext` → `IOverlaySwapChain`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextToSwapChainPath {
    /// `IOverlayMonitorTarget*`
    pub monitor_target_offset: usize,
    /// `GetOverlaySwapChain`
    pub swap_chain_vtable_index: usize,
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
pub struct HookProfile {
    pub signatures: &'static [HookSignature],
    pub swap_chain_to_resource_path: SwapChainToResourcePath,
    pub hardware_protected_offset: usize,
    pub monitor_identity_offsets: MonitorIdentityOffsets,
    pub context_to_swap_chain_path: ContextToSwapChainPath,
}
