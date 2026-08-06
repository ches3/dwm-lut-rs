use std::fmt;

use crate::profile::{AobToken, HookProfile, HookSignature};
use crate::target::HookTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rva(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRva {
    pub target: HookTarget,
    pub rva: Rva,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkippedSignatureReason {
    NotFound,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AobMatch {
    NotFound,
    Unique(Rva),
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkippedSignature {
    pub target: HookTarget,
    pub reason: SkippedSignatureReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureScanReport {
    pub resolved: Vec<ResolvedRva>,
    pub skipped: Vec<SkippedSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureScanError {
    NotFound { target: HookTarget },
    Ambiguous { target: HookTarget },
}

impl fmt::Display for SignatureScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { target } => {
                write!(f, "signature {} was not found", target.label())
            }
            Self::Ambiguous { target } => {
                write!(f, "signature {} matched multiple locations", target.label())
            }
        }
    }
}

impl std::error::Error for SignatureScanError {}

impl SignatureScanError {
    fn into_optional_skip(self) -> Result<SkippedSignature, Self> {
        match self {
            Self::NotFound { target } => Ok(SkippedSignature {
                target,
                reason: SkippedSignatureReason::NotFound,
            }),
            Self::Ambiguous { target } => Ok(SkippedSignature {
                target,
                reason: SkippedSignatureReason::Ambiguous,
            }),
        }
    }
}

pub fn resolve_signature_rva(
    image: &[u8],
    signature: &HookSignature,
) -> Result<ResolvedRva, SignatureScanError> {
    resolve_unique_rva(signature.target, match_aob(image, signature.aob))
}

pub fn scan_profile(
    profile: &HookProfile,
    image: &[u8],
) -> Result<SignatureScanReport, SignatureScanError> {
    let mut resolved = Vec::with_capacity(profile.signatures.len());
    let mut skipped = Vec::new();

    for signature in profile.signatures {
        match resolve_signature_rva(image, signature) {
            Ok(hit) => resolved.push(hit),
            Err(error) if !signature.target.is_required_signature() => {
                skipped.push(error.into_optional_skip()?);
            }
            Err(error) => return Err(error),
        }
    }

    Ok(SignatureScanReport { resolved, skipped })
}

fn resolve_unique_rva(
    target: HookTarget,
    aob_match: AobMatch,
) -> Result<ResolvedRva, SignatureScanError> {
    match aob_match {
        AobMatch::NotFound => Err(SignatureScanError::NotFound { target }),
        AobMatch::Unique(rva) => Ok(ResolvedRva { target, rva }),
        AobMatch::Ambiguous => Err(SignatureScanError::Ambiguous { target }),
    }
}

pub fn match_aob(image: &[u8], tokens: &[AobToken]) -> AobMatch {
    if tokens.is_empty() || tokens.len() > image.len() {
        return AobMatch::NotFound;
    }

    let mut unique = None;
    let scan_limit = image.len() - tokens.len();

    for offset in 0..=scan_limit {
        if tokens
            .iter()
            .zip(&image[offset..offset + tokens.len()])
            .all(|(token, byte)| matches_token(*token, *byte))
        {
            match unique {
                None => unique = Some(Rva(offset)),
                Some(_) => return AobMatch::Ambiguous,
            }
        }
    }

    match unique {
        None => AobMatch::NotFound,
        Some(rva) => AobMatch::Unique(rva),
    }
}

const fn matches_token(token: AobToken, byte: u8) -> bool {
    match token {
        AobToken::Exact(expected) => expected == byte,
        AobToken::Wildcard => true,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AobMatch, ResolvedRva, Rva, SignatureScanError, SkippedSignature, SkippedSignatureReason,
        match_aob, scan_profile,
    };
    use crate::profile::{
        AobToken, ContextToSwapChainPath, HookProfile, HookSignature, MonitorIdentityOffsets,
        SwapChainToResourcePath,
    };
    use crate::target::HookTarget;

    #[test]
    fn match_aob_honors_wildcards() {
        let tokens = [
            AobToken::Exact(0x40),
            AobToken::Exact(0x55),
            AobToken::Wildcard,
            AobToken::Exact(0x57),
        ];
        let image = [0x90, 0x40, 0x55, 0xAA, 0x57, 0x90];

        assert_eq!(match_aob(&image, &tokens), AobMatch::Unique(Rva(1)));
    }

    #[test]
    fn match_aob_stops_at_second_match() {
        let tokens = [AobToken::Exact(0x11)];
        let image = [0x11, 0x00, 0x11, 0x00, 0x11];

        assert_eq!(match_aob(&image, &tokens), AobMatch::Ambiguous);
    }

    fn test_profile(signatures: &'static [HookSignature]) -> HookProfile {
        HookProfile {
            signatures,
            swap_chain_to_resource_path: SwapChainToResourcePath {
                container_vtable_index: 0,
                resource_vtable_index: 0,
            },
            hardware_protected_offset: 0,
            monitor_identity_offsets: MonitorIdentityOffsets {
                adapter_luid_low_offset: 0,
                adapter_luid_high_offset: 0,
                target_id_offset: 0,
            },
            context_to_swap_chain_path: ContextToSwapChainPath {
                monitor_target_offset: 0,
                swap_chain_vtable_index: 0,
            },
        }
    }

    #[test]
    fn scan_profile_collects_all_resolved_targets() {
        let image = [0xAA, 0xBB, 0xCC];
        const SIGNATURES: &[HookSignature] = &[
            HookSignature {
                target: HookTarget::Present,
                aob: &[AobToken::Exact(0xAA)],
            },
            HookSignature {
                target: HookTarget::IsCandidateOverlayCompatible,
                aob: &[AobToken::Exact(0xBB)],
            },
            HookSignature {
                target: HookTarget::IsCandidateDirectFlipCompatible,
                aob: &[AobToken::Exact(0xCC)],
            },
        ];

        let report = scan_profile(&test_profile(SIGNATURES), &image).expect("scan");
        assert_eq!(
            report.resolved,
            vec![
                ResolvedRva {
                    target: HookTarget::Present,
                    rva: Rva(0),
                },
                ResolvedRva {
                    target: HookTarget::IsCandidateOverlayCompatible,
                    rva: Rva(1),
                },
                ResolvedRva {
                    target: HookTarget::IsCandidateDirectFlipCompatible,
                    rva: Rva(2),
                },
            ]
        );
    }

    #[test]
    fn scan_profile_reports_missing_required_signature() {
        let image = [0x00];
        const SIGNATURES: &[HookSignature] = &[HookSignature {
            target: HookTarget::Present,
            aob: &[
                AobToken::Exact(0x40),
                AobToken::Exact(0x55),
                AobToken::Wildcard,
                AobToken::Exact(0x57),
            ],
        }];
        let error = scan_profile(&test_profile(SIGNATURES), &image).expect_err("required miss");
        assert!(matches!(
            error,
            SignatureScanError::NotFound {
                target: HookTarget::Present
            }
        ));
    }

    #[test]
    fn scan_profile_records_optional_signature_failures() {
        const PRESENT_AND_OPTIONAL: &[HookSignature] = &[
            HookSignature {
                target: HookTarget::Present,
                aob: &[AobToken::Exact(0xAA)],
            },
            HookSignature {
                target: HookTarget::IsCandidateOverlayCompatible,
                aob: &[AobToken::Exact(0xDD)],
            },
        ];

        let cases: &[(&[u8], SkippedSignatureReason)] = &[
            (&[0xAA], SkippedSignatureReason::NotFound),
            (&[0xAA, 0xDD, 0xDD], SkippedSignatureReason::Ambiguous),
        ];

        for (image, reason) in cases {
            let report = scan_profile(&test_profile(PRESENT_AND_OPTIONAL), image)
                .expect("optional signature failure is allowed");
            assert_eq!(report.resolved.len(), 1);
            assert_eq!(report.resolved[0].target, HookTarget::Present);
            assert_eq!(
                report.skipped,
                vec![SkippedSignature {
                    target: HookTarget::IsCandidateOverlayCompatible,
                    reason: *reason,
                }]
            );
        }
    }

    #[test]
    fn scan_profile_rejects_ambiguous_required_match() {
        let image = [0x83, 0x00, 0x00, 0x83, 0x00, 0x00];
        const SIGNATURES: &[HookSignature] = &[HookSignature {
            target: HookTarget::Present,
            aob: &[
                AobToken::Exact(0x83),
                AobToken::Wildcard,
                AobToken::Wildcard,
            ],
        }];

        let error = scan_profile(&test_profile(SIGNATURES), &image).expect_err("ambiguous");
        assert!(matches!(
            error,
            SignatureScanError::Ambiguous {
                target: HookTarget::Present,
            }
        ));
    }
}
