use AobToken::{Exact, Wildcard};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildProfile {
    Windows11_25H2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTarget {
    Present,
    IsCandidateDirectFlipCompatible,
    OverlaysEnabled,
    WindowContextIsCandidateDirectFlipCompatible,
    CompSwapChainIsCandidateDirectFlipCompatible,
    CompSwapChainIsCandidateIndependentFlipCompatible,
    CompVisualIsCandidateForPromotion,
    OverlayTestMode,
}

impl HookTarget {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Present => "Present",
            Self::IsCandidateDirectFlipCompatible => "IsCandidateDirectFlipCompatible",
            Self::OverlaysEnabled => "OverlaysEnabled",
            Self::WindowContextIsCandidateDirectFlipCompatible => {
                "CWindowContext::IsCandidateDirectFlipCompatible"
            }
            Self::CompSwapChainIsCandidateDirectFlipCompatible => {
                "CCompSwapChain::IsCandidateDirectFlipCompatible"
            }
            Self::CompSwapChainIsCandidateIndependentFlipCompatible => {
                "CCompSwapChain::IsCandidateIndependentFlipCompatible"
            }
            Self::CompVisualIsCandidateForPromotion => "CCompVisual::IsCandidateForPromotion",
            Self::OverlayTestMode => "OverlayTestMode",
        }
    }

    pub const fn is_function_hook_target(self) -> bool {
        !matches!(self, Self::OverlayTestMode)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AobToken {
    Exact(u8),
    Wildcard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureLocator {
    Aob {
        module_name: &'static str,
        capture_key: &'static str,
        tokens: &'static [AobToken],
    },
    AobExcludingFollowingBytes {
        module_name: &'static str,
        capture_key: &'static str,
        tokens: &'static [AobToken],
        excluded_following: &'static [&'static [u8]],
    },
    RipRelativeGlobalAob {
        module_name: &'static str,
        capture_key: &'static str,
        tokens: &'static [AobToken],
        displacement_offset: usize,
        instruction_size: usize,
    },
    FollowingAob {
        module_name: &'static str,
        capture_key: &'static str,
        anchor_tokens: &'static [AobToken],
        tokens: &'static [AobToken],
        search_range: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookSignature {
    pub target: HookTarget,
    pub locator: SignatureLocator,
    pub note: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapChainPathHypothesis {
    pub accessor_key: &'static str,
    pub container_vtable_index: usize,
    pub resource_vtable_index: usize,
    pub note: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipBoxOwner {
    OverlayContextStateObject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipBoxPathHypothesis {
    pub accessor_key: &'static str,
    pub owner: ClipBoxOwner,
    pub context_state_pointer_offset: usize,
    pub offset: usize,
    pub note: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareProtectedPathHypothesis {
    pub accessor_key: &'static str,
    pub offset: usize,
    pub note: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileHypotheses {
    pub swap_chain: SwapChainPathHypothesis,
    pub clip_box: ClipBoxPathHypothesis,
    pub hardware_protected: HardwareProtectedPathHypothesis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookProfile {
    pub build: BuildProfile,
    pub module_name: &'static str,
    pub signatures: Vec<HookSignature>,
    pub hypotheses: ProfileHypotheses,
}

impl HookProfile {
    pub fn for_build(build: BuildProfile) -> Self {
        match build {
            BuildProfile::Windows11_25H2 => windows_11_25h2(),
        }
    }
}

const PRESENT_AOB: &[AobToken] = &[
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
    Exact(0x8D),
    Exact(0x6C),
    Exact(0x24),
    Exact(0xF9),
    Exact(0x48),
    Exact(0x81),
    Exact(0xEC),
    Exact(0xF8),
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
    Exact(0x45),
    Exact(0xEF),
    Exact(0x4C),
    Exact(0x8B),
    Exact(0x65),
    Wildcard,
    Exact(0x48),
    Exact(0x8B),
    Exact(0xD9),
];

const DIRECT_FLIP_AOB: &[AobToken] = &[
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
];

const COMP_SWAP_CHAIN_DIRECT_FLIP_FOLLOWING_BYTES: &[u8] = &[0x41, 0x8B, 0xF0];

const OVERLAYS_ENABLED_AOB: &[AobToken] = &[
    Exact(0x83),
    Exact(0x3D),
    Wildcard,
    Wildcard,
    Wildcard,
    Wildcard,
    Exact(0x05),
    Exact(0x74),
    Exact(0x09),
    Exact(0x83),
    Exact(0x79),
    Exact(0x28),
    Exact(0x01),
    Exact(0x0F),
    Exact(0x97),
    Exact(0xC0),
    Exact(0xC3),
];

const WINDOW_CONTEXT_DIRECT_FLIP_AOB: &[AobToken] = &[
    Exact(0x48),
    Exact(0x89),
    Exact(0x5C),
    Exact(0x24),
    Exact(0x08),
    Exact(0x48),
    Exact(0x89),
    Exact(0x74),
    Exact(0x24),
    Exact(0x10),
    Exact(0x57),
    Exact(0x48),
    Exact(0x83),
    Exact(0xEC),
    Exact(0x20),
    Exact(0x41),
    Exact(0x8B),
    Exact(0xD9),
    Exact(0x48),
    Exact(0x8B),
    Exact(0xF2),
    Exact(0x4C),
    Exact(0x8B),
    Exact(0x01),
    Exact(0x48),
    Exact(0x8B),
    Exact(0xF9),
];

const COMP_SWAP_CHAIN_DIRECT_FLIP_AOB: &[AobToken] = &[
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
    Exact(0x41),
    Exact(0x8B),
    Exact(0xF0),
];

const COMP_VISUAL_PROMOTION_AOB: &[AobToken] = &[
    Exact(0x48),
    Exact(0x89),
    Exact(0x5C),
    Exact(0x24),
    Exact(0x10),
    Exact(0x48),
    Exact(0x89),
    Exact(0x74),
    Exact(0x24),
    Exact(0x18),
    Exact(0x57),
    Exact(0x48),
    Exact(0x83),
    Exact(0xEC),
    Exact(0x20),
    Exact(0x48),
    Exact(0x8B),
    Exact(0x01),
    Exact(0x41),
    Exact(0x8B),
    Exact(0xD1),
    Exact(0x48),
    Exact(0x8B),
    Exact(0xF1),
];

const COMP_SWAP_CHAIN_INDEPENDENT_FLIP_AOB: &[AobToken] = &[Exact(0x48), Exact(0x8D), Exact(0x05)];

fn windows_11_25h2() -> HookProfile {
    HookProfile {
        build: BuildProfile::Windows11_25H2,
        module_name: "dwmcore.dll",
        signatures: vec![
            HookSignature {
                target: HookTarget::Present,
                locator: SignatureLocator::Aob {
                    module_name: "dwmcore.dll",
                    capture_key: "present_25h2",
                    tokens: PRESENT_AOB,
                },
                note: "Matches the 25H2 COverlayContext::Present prologue used by dwm_lut_fixed.",
            },
            HookSignature {
                target: HookTarget::IsCandidateDirectFlipCompatible,
                locator: SignatureLocator::AobExcludingFollowingBytes {
                    module_name: "dwmcore.dll",
                    capture_key: "direct_flip_compat_25h2",
                    tokens: DIRECT_FLIP_AOB,
                    excluded_following: &[COMP_SWAP_CHAIN_DIRECT_FLIP_FOLLOWING_BYTES],
                },
                note: "Matches the 25H2 direct-flip compatibility gate while excluding the CCompSwapChain prologue that shares the same prefix.",
            },
            HookSignature {
                target: HookTarget::OverlaysEnabled,
                locator: SignatureLocator::Aob {
                    module_name: "dwmcore.dll",
                    capture_key: "overlays_enabled_25h2",
                    tokens: OVERLAYS_ENABLED_AOB,
                },
                note: "Matches the 25H2 overlay enablement check and preserves the nearby RIP-relative global access.",
            },
            HookSignature {
                target: HookTarget::WindowContextIsCandidateDirectFlipCompatible,
                locator: SignatureLocator::Aob {
                    module_name: "dwmcore.dll",
                    capture_key: "window_direct_flip_compat_25h2",
                    tokens: WINDOW_CONTEXT_DIRECT_FLIP_AOB,
                },
                note: "Matches the 25H2 CWindowContext direct-flip gate used by dwm_lut_fixed to close promotion paths outside COverlayContext.",
            },
            HookSignature {
                target: HookTarget::CompSwapChainIsCandidateDirectFlipCompatible,
                locator: SignatureLocator::Aob {
                    module_name: "dwmcore.dll",
                    capture_key: "comp_swap_chain_direct_flip_compat_25h2",
                    tokens: COMP_SWAP_CHAIN_DIRECT_FLIP_AOB,
                },
                note: "Matches the 25H2 CCompSwapChain direct-flip gate used by dwm_lut_fixed to suppress promotion after DWM refactors.",
            },
            HookSignature {
                target: HookTarget::CompVisualIsCandidateForPromotion,
                locator: SignatureLocator::Aob {
                    module_name: "dwmcore.dll",
                    capture_key: "comp_visual_promotion_25h2",
                    tokens: COMP_VISUAL_PROMOTION_AOB,
                },
                note: "Matches the 25H2 CCompVisual promotion gate used by dwm_lut_fixed.",
            },
            HookSignature {
                target: HookTarget::CompSwapChainIsCandidateIndependentFlipCompatible,
                locator: SignatureLocator::FollowingAob {
                    module_name: "dwmcore.dll",
                    capture_key: "comp_swap_chain_independent_flip_compat_25h2",
                    anchor_tokens: OVERLAYS_ENABLED_AOB,
                    tokens: COMP_SWAP_CHAIN_INDEPENDENT_FLIP_AOB,
                    search_range: 500,
                },
                note: "Matches the 25H2 CCompSwapChain independent-flip gate located near the OverlaysEnabled global access in dwm_lut_fixed.",
            },
            HookSignature {
                target: HookTarget::OverlayTestMode,
                locator: SignatureLocator::RipRelativeGlobalAob {
                    module_name: "dwmcore.dll",
                    capture_key: "overlay_test_mode_25h2",
                    tokens: OVERLAYS_ENABLED_AOB,
                    displacement_offset: 2,
                    instruction_size: 7,
                },
                note: "Resolves the RIP-relative OverlayTestMode global referenced by the 25H2 OverlaysEnabled check.",
            },
        ],
        hypotheses: ProfileHypotheses {
            swap_chain: SwapChainPathHypothesis {
                accessor_key: "overlay_swap_chain_back_buffer_vtbl_24_19",
                container_vtable_index: 24,
                resource_vtable_index: 19,
                note: "25H2 path mirrored from ed1ii/dwm_lut_fixed: call IOverlaySwapChain vtable[24], then returned object vtable[19], then QueryInterface for ID3D11Texture2D.",
            },
            clip_box: ClipBoxPathHypothesis {
                accessor_key: "overlay_context_state_clip_origin_0x4d0",
                owner: ClipBoxOwner::OverlayContextStateObject,
                context_state_pointer_offset: 0,
                offset: 0x4D0,
                note: "Initial 25H2 hypothesis: read the overlay-context state object pointer from COverlayContext + 0, then read the clip origin from state object + 0x4d0.",
            },
            hardware_protected: HardwareProtectedPathHypothesis {
                accessor_key: "overlay_swap_chain_hardware_protected_0x4c",
                offset: 0x4c,
                note: "Initial 25H2 hypothesis: read the hardware-protected flag from IOverlaySwapChain at offset 0x4c.",
            },
        },
    }
}
