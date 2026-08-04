use std::fmt;

use crate::profile::{AobToken, HookProfile, HookSignature, SignatureLocator};
use crate::target::HookTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rva(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRva {
    pub target: HookTarget,
    pub rva: Rva,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutOfBoundsReason {
    InstructionEndOverflow,
    TargetAddressOverflow,
    NegativeRva,
    OutsideImage,
    DisplacementOutOfBounds,
}

impl fmt::Display for OutOfBoundsReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InstructionEndOverflow => "RIP-relative instruction end overflowed",
            Self::TargetAddressOverflow => "RIP-relative target address overflowed",
            Self::NegativeRva => "RIP-relative target RVA was negative",
            Self::OutsideImage => "RIP-relative target RVA was outside the image",
            Self::DisplacementOutOfBounds => "RIP-relative displacement was out of image bounds",
        })
    }
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
    NotFound {
        target: HookTarget,
    },
    Ambiguous {
        target: HookTarget,
    },
    OutOfBounds {
        target: HookTarget,
        reason: OutOfBoundsReason,
    },
    IncompatibleLocator {
        target: HookTarget,
    },
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
            Self::OutOfBounds { target, reason } => {
                write!(
                    f,
                    "signature {} scan out of bounds: {reason}",
                    target.label()
                )
            }
            Self::IncompatibleLocator { target } => {
                write!(
                    f,
                    "signature {} locator is incompatible with the target",
                    target.label()
                )
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
            other => Err(other),
        }
    }
}

pub fn resolve_signature_rva(
    image: &[u8],
    signature: &HookSignature,
) -> Result<ResolvedRva, SignatureScanError> {
    match signature.locator {
        SignatureLocator::Aob { tokens, .. } => {
            resolve_unique_rva(signature.target, match_aob(image, tokens))
        }
        SignatureLocator::RipRelativeGlobalAob {
            tokens,
            displacement_offset,
            instruction_size,
            ..
        } => {
            let value_size = signature.target.global_value_size().ok_or(
                SignatureScanError::IncompatibleLocator {
                    target: signature.target,
                },
            )?;
            match match_aob(image, tokens) {
                AobMatch::NotFound => Err(SignatureScanError::NotFound {
                    target: signature.target,
                }),
                AobMatch::Unique(Rva(offset)) => {
                    let displacement_rva = offset.checked_add(displacement_offset).ok_or(
                        SignatureScanError::OutOfBounds {
                            target: signature.target,
                            reason: OutOfBoundsReason::DisplacementOutOfBounds,
                        },
                    )?;
                    let displacement =
                        read_i32_from_image(image, displacement_rva, signature.target)? as isize;
                    let instruction_end = offset.checked_add(instruction_size).ok_or(
                        SignatureScanError::OutOfBounds {
                            target: signature.target,
                            reason: OutOfBoundsReason::InstructionEndOverflow,
                        },
                    )?;
                    let signed = (instruction_end as isize).checked_add(displacement).ok_or(
                        SignatureScanError::OutOfBounds {
                            target: signature.target,
                            reason: OutOfBoundsReason::TargetAddressOverflow,
                        },
                    )?;
                    let rva =
                        usize::try_from(signed).map_err(|_| SignatureScanError::OutOfBounds {
                            target: signature.target,
                            reason: OutOfBoundsReason::NegativeRva,
                        })?;
                    let end =
                        rva.checked_add(value_size)
                            .ok_or(SignatureScanError::OutOfBounds {
                                target: signature.target,
                                reason: OutOfBoundsReason::OutsideImage,
                            })?;
                    if image.get(rva..end).is_none() {
                        return Err(SignatureScanError::OutOfBounds {
                            target: signature.target,
                            reason: OutOfBoundsReason::OutsideImage,
                        });
                    }

                    Ok(ResolvedRva {
                        target: signature.target,
                        rva: Rva(rva),
                    })
                }
                AobMatch::Ambiguous => Err(SignatureScanError::Ambiguous {
                    target: signature.target,
                }),
            }
        }
    }
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

fn read_i32_from_image(
    image: &[u8],
    offset: usize,
    target: HookTarget,
) -> Result<i32, SignatureScanError> {
    let bytes = image
        .get(offset..offset + 4)
        .ok_or(SignatureScanError::OutOfBounds {
            target,
            reason: OutOfBoundsReason::DisplacementOutOfBounds,
        })?;
    Ok(i32::from_le_bytes(
        bytes.try_into().expect("slice length is fixed to 4"),
    ))
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
        AobMatch, OutOfBoundsReason, ResolvedRva, Rva, SignatureScanError, SkippedSignature,
        SkippedSignatureReason, match_aob, resolve_signature_rva, scan_profile,
    };
    use crate::profile::{
        AobToken, HookProfile, HookSignature, MonitorIdentityOffsets, SignatureLocator,
        SwapChainVtablePath,
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

    fn prologue_test_profile() -> HookProfile {
        HookProfile {
            signatures: &[HookSignature {
                target: HookTarget::Present,
                locator: SignatureLocator::Aob {
                    tokens: &[
                        AobToken::Exact(0x40),
                        AobToken::Exact(0x55),
                        AobToken::Wildcard,
                        AobToken::Exact(0x57),
                    ],
                },
            }],
            swap_chain: SwapChainVtablePath {
                container_vtable_index: 0,
                resource_vtable_index: 0,
            },
            hardware_protected_offset: 0,
            monitor_identity: MonitorIdentityOffsets {
                adapter_luid_low_offset: 0,
                adapter_luid_high_offset: 0,
                target_id_offset: 0,
            },
        }
    }

    #[test]
    fn scan_profile_collects_all_resolved_targets() {
        let image = [0xAA, 0xBB, 0xCC];
        const SIGNATURES: &[HookSignature] = &[
            HookSignature {
                target: HookTarget::Present,
                locator: SignatureLocator::Aob {
                    tokens: &[AobToken::Exact(0xAA)],
                },
            },
            HookSignature {
                target: HookTarget::OverlayTestMode,
                locator: SignatureLocator::Aob {
                    tokens: &[AobToken::Exact(0xBB)],
                },
            },
            HookSignature {
                target: HookTarget::DisableIndependentFlip,
                locator: SignatureLocator::Aob {
                    tokens: &[AobToken::Exact(0xCC)],
                },
            },
        ];
        let profile = HookProfile {
            signatures: SIGNATURES,
            swap_chain: SwapChainVtablePath {
                container_vtable_index: 0,
                resource_vtable_index: 0,
            },
            hardware_protected_offset: 0,
            monitor_identity: MonitorIdentityOffsets {
                adapter_luid_low_offset: 0,
                adapter_luid_high_offset: 0,
                target_id_offset: 0,
            },
        };

        let report = scan_profile(&profile, &image).expect("scan");
        assert_eq!(
            report.resolved,
            vec![
                ResolvedRva {
                    target: HookTarget::Present,
                    rva: Rva(0),
                },
                ResolvedRva {
                    target: HookTarget::OverlayTestMode,
                    rva: Rva(1),
                },
                ResolvedRva {
                    target: HookTarget::DisableIndependentFlip,
                    rva: Rva(2),
                },
            ]
        );
    }

    #[test]
    fn scan_profile_reports_missing_required_signature() {
        let image = [0x00];
        let profile = prologue_test_profile();
        let error = scan_profile(&profile, &image).expect_err("required miss");
        assert!(matches!(
            error,
            SignatureScanError::NotFound {
                target: HookTarget::Present
            }
        ));
    }

    fn profile_with_signatures(signatures: &'static [HookSignature]) -> HookProfile {
        HookProfile {
            signatures,
            swap_chain: SwapChainVtablePath {
                container_vtable_index: 0,
                resource_vtable_index: 0,
            },
            hardware_protected_offset: 0,
            monitor_identity: MonitorIdentityOffsets {
                adapter_luid_low_offset: 0,
                adapter_luid_high_offset: 0,
                target_id_offset: 0,
            },
        }
    }

    #[test]
    fn scan_profile_records_optional_signature_failures() {
        const PRESENT_AND_OVERLAYS_ENABLED: &[HookSignature] = &[
            HookSignature {
                target: HookTarget::Present,
                locator: SignatureLocator::Aob {
                    tokens: &[AobToken::Exact(0xAA)],
                },
            },
            HookSignature {
                target: HookTarget::OverlaysEnabled,
                locator: SignatureLocator::Aob {
                    tokens: &[AobToken::Exact(0xDD)],
                },
            },
        ];

        let cases: &[(&[u8], SkippedSignatureReason)] = &[
            (&[0xAA], SkippedSignatureReason::NotFound),
            (&[0xAA, 0xDD, 0xDD], SkippedSignatureReason::Ambiguous),
        ];

        for (image, reason) in cases {
            let report = scan_profile(
                &profile_with_signatures(PRESENT_AND_OVERLAYS_ENABLED),
                image,
            )
            .expect("optional signature failure is allowed");
            assert_eq!(report.resolved.len(), 1);
            assert_eq!(report.resolved[0].target, HookTarget::Present);
            assert_eq!(
                report.skipped,
                vec![SkippedSignature {
                    target: HookTarget::OverlaysEnabled,
                    reason: *reason,
                }]
            );
        }
    }

    #[test]
    fn scan_profile_rejects_optional_incompatible_locator() {
        const SIGNATURES: &[HookSignature] = &[
            HookSignature {
                target: HookTarget::Present,
                locator: SignatureLocator::Aob {
                    tokens: &[AobToken::Exact(0xAA)],
                },
            },
            HookSignature {
                target: HookTarget::OverlaysEnabled,
                locator: SignatureLocator::RipRelativeGlobalAob {
                    tokens: &[AobToken::Exact(0xDD)],
                    displacement_offset: 1,
                    instruction_size: 5,
                },
            },
        ];
        let image = [0xAA, 0xDD, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let error = scan_profile(&profile_with_signatures(SIGNATURES), &image)
            .expect_err("incompatible locator is not skippable");
        assert!(matches!(
            error,
            SignatureScanError::IncompatibleLocator {
                target: HookTarget::OverlaysEnabled,
            }
        ));
    }

    #[test]
    fn scan_profile_rejects_optional_out_of_bounds() {
        const SIGNATURES: &[HookSignature] = &[
            HookSignature {
                target: HookTarget::Present,
                locator: SignatureLocator::Aob {
                    tokens: &[AobToken::Exact(0xBB)],
                },
            },
            HookSignature {
                target: HookTarget::DisableIndependentFlip,
                locator: SignatureLocator::RipRelativeGlobalAob {
                    tokens: &[AobToken::Exact(0xAA)],
                    displacement_offset: 1,
                    instruction_size: 5,
                },
            },
        ];
        // instruction_end = 5; disp = 0x10 -> rva = 0x15, image len = 7
        let image = [0xBB, 0xAA, 0x10, 0x00, 0x00, 0x00, 0x00];
        let error = scan_profile(&profile_with_signatures(SIGNATURES), &image)
            .expect_err("out of bounds is not skippable");
        assert!(matches!(
            error,
            SignatureScanError::OutOfBounds {
                target: HookTarget::DisableIndependentFlip,
                reason: OutOfBoundsReason::OutsideImage,
            }
        ));
    }

    #[test]
    fn scan_profile_rejects_ambiguous_required_match() {
        let image = [0x83, 0x00, 0x00, 0x83, 0x00, 0x00];
        const SIGNATURES: &[HookSignature] = &[HookSignature {
            target: HookTarget::Present,
            locator: SignatureLocator::Aob {
                tokens: &[
                    AobToken::Exact(0x83),
                    AobToken::Wildcard,
                    AobToken::Wildcard,
                ],
            },
        }];
        let profile = HookProfile {
            signatures: SIGNATURES,
            swap_chain: SwapChainVtablePath {
                container_vtable_index: 0,
                resource_vtable_index: 0,
            },
            hardware_protected_offset: 0,
            monitor_identity: MonitorIdentityOffsets {
                adapter_luid_low_offset: 0,
                adapter_luid_high_offset: 0,
                target_id_offset: 0,
            },
        };

        let error = scan_profile(&profile, &image).expect_err("ambiguous");
        assert!(matches!(
            error,
            SignatureScanError::Ambiguous {
                target: HookTarget::Present,
            }
        ));
    }

    fn rip_relative_signature(
        displacement_offset: usize,
        instruction_size: usize,
    ) -> HookSignature {
        HookSignature {
            target: HookTarget::OverlayTestMode,
            locator: SignatureLocator::RipRelativeGlobalAob {
                tokens: &[AobToken::Exact(0xAA)],
                displacement_offset,
                instruction_size,
            },
        }
    }

    #[test]
    fn resolve_rip_relative_global_returns_target_rva() {
        // instruction at 0: tokens match; displacement at offset 1; instruction_size 5
        // bytes: AA [disp=0x00000010 LE] -> target rva = 5 + 0x10 = 0x15
        let mut image = [0u8; 0x20];
        image[0] = 0xAA;
        image[1] = 0x10;
        let resolved =
            resolve_signature_rva(&image, &rip_relative_signature(1, 5)).expect("resolve");
        assert_eq!(resolved.rva, Rva(0x15));
    }

    #[test]
    fn resolve_rip_relative_global_rejects_negative_rva() {
        // instruction_end = 5; disp = -8 -> signed target = -3
        let image = [0xAA, 0xF8, 0xFF, 0xFF, 0xFF, 0x00];
        let error =
            resolve_signature_rva(&image, &rip_relative_signature(1, 5)).expect_err("negative rva");
        assert!(matches!(
            error,
            SignatureScanError::OutOfBounds {
                target: HookTarget::OverlayTestMode,
                reason: OutOfBoundsReason::NegativeRva,
            }
        ));
    }

    #[test]
    fn resolve_rip_relative_global_rejects_address_without_i32_span() {
        // instruction_end = 5; disp = 2 -> rva = 7, image len = 10 (only 3 bytes remain)
        let mut image = [0u8; 10];
        image[0] = 0xAA;
        image[1] = 0x02;
        let error = resolve_signature_rva(&image, &rip_relative_signature(1, 5))
            .expect_err("incomplete i32 span");
        assert!(matches!(
            error,
            SignatureScanError::OutOfBounds {
                target: HookTarget::OverlayTestMode,
                reason: OutOfBoundsReason::OutsideImage,
            }
        ));
    }

    #[test]
    fn resolve_rip_relative_global_accepts_address_with_i32_span() {
        // instruction_end = 5; disp = 1 -> rva = 6, image len = 10 (exactly 4 bytes)
        let mut image = [0u8; 10];
        image[0] = 0xAA;
        image[1] = 0x01;
        let resolved =
            resolve_signature_rva(&image, &rip_relative_signature(1, 5)).expect("resolve");
        assert_eq!(resolved.rva, Rva(6));
    }

    #[test]
    fn resolve_rip_relative_global_rejects_rva_outside_image() {
        // instruction_end = 5; disp = 0x10 -> rva = 0x15, image len = 6
        let image = [0xAA, 0x10, 0x00, 0x00, 0x00, 0x00];
        let error = resolve_signature_rva(&image, &rip_relative_signature(1, 5))
            .expect_err("outside image");
        assert!(matches!(
            error,
            SignatureScanError::OutOfBounds {
                target: HookTarget::OverlayTestMode,
                reason: OutOfBoundsReason::OutsideImage,
            }
        ));
    }

    #[test]
    fn resolve_rip_relative_global_rejects_displacement_offset_overflow() {
        let image = [0x00, 0xAA];
        let error = resolve_signature_rva(&image, &rip_relative_signature(usize::MAX, 5))
            .expect_err("displacement offset overflow");
        assert!(matches!(
            error,
            SignatureScanError::OutOfBounds {
                target: HookTarget::OverlayTestMode,
                reason: OutOfBoundsReason::DisplacementOutOfBounds,
            }
        ));
    }

    #[test]
    fn resolve_rip_relative_global_rejects_incompatible_locator_target() {
        let signature = HookSignature {
            target: HookTarget::Present,
            locator: SignatureLocator::RipRelativeGlobalAob {
                tokens: &[AobToken::Exact(0xAA)],
                displacement_offset: 1,
                instruction_size: 5,
            },
        };
        let image = [0xAA, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let error =
            resolve_signature_rva(&image, &signature).expect_err("incompatible locator target");
        assert!(matches!(
            error,
            SignatureScanError::IncompatibleLocator {
                target: HookTarget::Present,
            }
        ));
    }
}
