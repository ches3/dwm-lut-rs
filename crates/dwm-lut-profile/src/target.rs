#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTarget {
    Present,
    IsCandidateDirectFlipCompatible,
    DirectFlipInfoEnsureIndependentFlipState,
    IsDirectFlipSupportedOnTarget,
    LegacySwapChainCheckDirectFlipSupport,
    IsAdvancedDirectFlipCompatible,
    OverlayTestMode,
    DisableIndependentFlip,
    OverlaysEnabled,
}

impl HookTarget {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Present => "Present",
            Self::IsCandidateDirectFlipCompatible => "IsCandidateDirectFlipCompatible",
            Self::DirectFlipInfoEnsureIndependentFlipState => {
                "CDirectFlipInfo::EnsureIndependentFlipState"
            }
            Self::IsDirectFlipSupportedOnTarget => "COverlayContext::IsDirectFlipSupportedOnTarget",
            Self::LegacySwapChainCheckDirectFlipSupport => {
                "CLegacySwapChain::CheckDirectFlipSupport"
            }
            Self::IsAdvancedDirectFlipCompatible => {
                "CGlobalCompositionSurfaceInfo::IsAdvancedDirectFlipCompatible"
            }
            Self::OverlayTestMode => "OverlayTestMode",
            Self::DisableIndependentFlip => "DisableIndependentFlip",
            Self::OverlaysEnabled => "COverlayContext::OverlaysEnabled",
        }
    }

    pub const fn is_function_hook_target(self) -> bool {
        !matches!(self, Self::OverlayTestMode | Self::DisableIndependentFlip)
    }

    pub const fn global_value_size(self) -> Option<usize> {
        match self {
            Self::OverlayTestMode | Self::DisableIndependentFlip => Some(4),
            _ => None,
        }
    }

    pub const fn is_required_signature(self) -> bool {
        matches!(self, Self::Present | Self::OverlayTestMode)
    }

    #[cfg(feature = "xtask")]
    pub const fn pdb_symbol_prefix(self) -> &'static str {
        match self {
            Self::Present => "?Present@COverlayContext@@",
            Self::IsCandidateDirectFlipCompatible => {
                "?IsCandidateDirectFlipCompatible@COverlayContext@@"
            }
            Self::DirectFlipInfoEnsureIndependentFlipState => {
                "?EnsureIndependentFlipState@CDirectFlipInfo@@"
            }
            Self::IsDirectFlipSupportedOnTarget => {
                "?IsDirectFlipSupportedOnTarget@COverlayContext@@"
            }
            Self::LegacySwapChainCheckDirectFlipSupport => {
                "?CheckDirectFlipSupport@CLegacySwapChain@@"
            }
            Self::IsAdvancedDirectFlipCompatible => {
                "?IsAdvancedDirectFlipCompatible@CGlobalCompositionSurfaceInfo@@"
            }
            Self::OverlayTestMode => "?m_dwOverlayTestMode@CCommonRegistryData@@",
            Self::DisableIndependentFlip => "?m_fDisableIndependentFlip@CCommonRegistryData@@",
            Self::OverlaysEnabled => "?OverlaysEnabled@COverlayContext@@",
        }
    }
}
