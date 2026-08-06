#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HookTarget {
    Present,
    IsCandidateDirectFlipCompatible,
    IsCandidateOverlayCompatible,
}

impl HookTarget {
    pub const ALL: &[Self] = &[
        Self::Present,
        Self::IsCandidateDirectFlipCompatible,
        Self::IsCandidateOverlayCompatible,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Present => "Present",
            Self::IsCandidateDirectFlipCompatible => "IsCandidateDirectFlipCompatible",
            Self::IsCandidateOverlayCompatible => "IsCandidateOverlayCompatible",
        }
    }

    pub const fn is_required_signature(self) -> bool {
        matches!(self, Self::Present)
    }

    pub const fn is_flip_gate(self) -> bool {
        match self {
            Self::Present => false,
            Self::IsCandidateDirectFlipCompatible | Self::IsCandidateOverlayCompatible => true,
        }
    }

    #[cfg(feature = "xtask")]
    pub const fn pdb_symbol_prefix(self) -> &'static str {
        match self {
            Self::Present => "?Present@COverlayContext@@",
            Self::IsCandidateDirectFlipCompatible => {
                "?IsCandidateDirectFlipCompatible@COverlayContext@@"
            }
            Self::IsCandidateOverlayCompatible => "?IsCandidateOverlayCompatible@COverlayContext@@",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HookTarget;

    #[test]
    fn all_covers_every_variant() {
        let mut seen = 0u8;
        for &target in HookTarget::ALL {
            match target {
                HookTarget::Present => seen |= 1 << 0,
                HookTarget::IsCandidateDirectFlipCompatible => seen |= 1 << 1,
                HookTarget::IsCandidateOverlayCompatible => seen |= 1 << 2,
            }
        }
        assert_eq!(seen.count_ones() as usize, HookTarget::ALL.len());
        assert_eq!(seen, (1 << HookTarget::ALL.len()) - 1);
    }
}
