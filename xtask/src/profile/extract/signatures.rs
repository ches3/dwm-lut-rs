use iced_x86::{Decoder, DecoderOptions, Instruction};

use dwm_lut_profile::{AobMatch, AobToken, HookTarget, Rva, match_aob};

use super::report::{LocatorKind, SignatureReport, SignatureStatus, format_rva};
use super::symbols::{
    EXTRACT_TARGETS, MAX_LEN, MIN_INSNS, ResolvedSymbol, SymbolResolveError,
    resolve_function_symbol, resolve_global_symbol,
};
use crate::profile::pdb_publics::PdbPublics;
use crate::profile::pe::PeImage;

pub fn extract_signatures(pe: &PeImage, pubs: &PdbPublics) -> Result<Vec<SignatureReport>, String> {
    let mut reports = Vec::new();
    let mut overlays_enabled_tokens = None;

    for &target in EXTRACT_TARGETS {
        match target {
            HookTarget::OverlayTestMode => {
                let (overlay, tokens) = extract_overlay_test_mode(pe, pubs);
                overlays_enabled_tokens = tokens;
                reports.push(overlay);
            }
            HookTarget::OverlaysEnabled => {
                reports.push(extract_overlays_enabled(
                    pe,
                    pubs,
                    overlays_enabled_tokens.take(),
                ));
            }
            HookTarget::DisableIndependentFlip => {
                reports.push(extract_disable_independent_flip(pe, pubs));
            }
            _ => reports.push(extract_function_signature(target, pe, pubs)),
        }
    }
    Ok(reports)
}

fn extract_function_signature(
    target: HookTarget,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> SignatureReport {
    match resolve_function_symbol(target, pubs, pe) {
        Ok(resolved) => uniquify_function(target, &resolved, pe),
        Err(error) => missing_symbol_report(target, error),
    }
}

fn uniquify_function(
    target: HookTarget,
    resolved: &ResolvedSymbol,
    pe: &PeImage,
) -> SignatureReport {
    match uniquify_at_rva(pe, resolved.rva, WildcardProfile::Function(target)) {
        Ok(pattern) => SignatureReport {
            target: target.label().into(),
            hook_target: target,
            status: SignatureStatus::Ok,
            rva: Some(format_rva(resolved.rva)),
            locator_kind: Some(LocatorKind::Aob),
            aob: Some(tokens_to_aob(&pattern.tokens)),
            displacement_offset: None,
            instruction_size: None,
        },
        Err(_error) => SignatureReport {
            target: target.label().into(),
            hook_target: target,
            status: SignatureStatus::UniquifyFailed,
            rva: Some(format_rva(resolved.rva)),
            locator_kind: None,
            aob: None,
            displacement_offset: None,
            instruction_size: None,
        },
    }
}

fn extract_overlay_test_mode(
    pe: &PeImage,
    pubs: &PdbPublics,
) -> (SignatureReport, Option<Vec<AobToken>>) {
    let global = match resolve_global_symbol(HookTarget::OverlayTestMode, pubs) {
        Ok(symbol) => symbol,
        Err(error) => {
            return (
                SignatureReport {
                    target: HookTarget::OverlayTestMode.label().into(),
                    hook_target: HookTarget::OverlayTestMode,
                    status: status_from_symbol_error(&error),
                    rva: None,
                    locator_kind: None,
                    aob: None,
                    displacement_offset: None,
                    instruction_size: None,
                },
                None,
            );
        }
    };

    let overlays = match resolve_function_symbol(HookTarget::OverlaysEnabled, pubs, pe) {
        Ok(resolved) => resolved,
        Err(_error) => {
            return (
                SignatureReport {
                    target: HookTarget::OverlayTestMode.label().into(),
                    hook_target: HookTarget::OverlayTestMode,
                    status: SignatureStatus::NoRefSite,
                    rva: Some(format_rva(global.rva)),
                    locator_kind: None,
                    aob: None,
                    displacement_offset: None,
                    instruction_size: None,
                },
                None,
            );
        }
    };

    if let Err(_error) = verify_rip_relative_points_to(pe, overlays.rva, 2, global.rva) {
        return (
            SignatureReport {
                target: HookTarget::OverlayTestMode.label().into(),
                hook_target: HookTarget::OverlayTestMode,
                status: SignatureStatus::RipVerifyFailed,
                rva: Some(format_rva(overlays.rva)),
                locator_kind: None,
                aob: None,
                displacement_offset: None,
                instruction_size: None,
            },
            None,
        );
    }

    match uniquify_at_rva(pe, overlays.rva, WildcardProfile::RipGlobal) {
        Ok(pattern) => (
            SignatureReport {
                target: HookTarget::OverlayTestMode.label().into(),
                hook_target: HookTarget::OverlayTestMode,
                status: SignatureStatus::Ok,
                rva: Some(format_rva(overlays.rva)),
                locator_kind: Some(LocatorKind::RipRelativeGlobalAob),
                aob: Some(tokens_to_aob(&pattern.tokens)),
                displacement_offset: Some(2),
                instruction_size: Some(7),
            },
            Some(pattern.tokens),
        ),
        Err(_error) => (
            SignatureReport {
                target: HookTarget::OverlayTestMode.label().into(),
                hook_target: HookTarget::OverlayTestMode,
                status: SignatureStatus::UniquifyFailed,
                rva: Some(format_rva(overlays.rva)),
                locator_kind: None,
                aob: None,
                displacement_offset: None,
                instruction_size: None,
            },
            None,
        ),
    }
}

fn extract_overlays_enabled(
    pe: &PeImage,
    pubs: &PdbPublics,
    shared_tokens: Option<Vec<AobToken>>,
) -> SignatureReport {
    let overlays = match resolve_function_symbol(HookTarget::OverlaysEnabled, pubs, pe) {
        Ok(resolved) => resolved,
        Err(error) => return missing_symbol_report(HookTarget::OverlaysEnabled, error),
    };

    let Some(tokens) = shared_tokens else {
        return SignatureReport {
            target: HookTarget::OverlaysEnabled.label().into(),
            hook_target: HookTarget::OverlaysEnabled,
            status: SignatureStatus::NoSharedAob,
            rva: Some(format_rva(overlays.rva)),
            locator_kind: None,
            aob: None,
            displacement_offset: None,
            instruction_size: None,
        };
    };

    SignatureReport {
        target: HookTarget::OverlaysEnabled.label().into(),
        hook_target: HookTarget::OverlaysEnabled,
        status: SignatureStatus::Ok,
        rva: Some(format_rva(overlays.rva)),
        locator_kind: Some(LocatorKind::Aob),
        aob: Some(tokens_to_aob(&tokens)),
        displacement_offset: None,
        instruction_size: None,
    }
}

fn extract_disable_independent_flip(pe: &PeImage, pubs: &PdbPublics) -> SignatureReport {
    let global = match resolve_global_symbol(HookTarget::DisableIndependentFlip, pubs) {
        Ok(symbol) => symbol,
        Err(error) => {
            return SignatureReport {
                target: HookTarget::DisableIndependentFlip.label().into(),
                hook_target: HookTarget::DisableIndependentFlip,
                status: status_from_symbol_error(&error),
                rva: None,
                locator_kind: None,
                aob: None,
                displacement_offset: None,
                instruction_size: None,
            };
        }
    };

    let Some(site_rva) = find_cmp_rip_site(pe, global.rva) else {
        return SignatureReport {
            target: HookTarget::DisableIndependentFlip.label().into(),
            hook_target: HookTarget::DisableIndependentFlip,
            status: SignatureStatus::NoRefSite,
            rva: Some(format_rva(global.rva)),
            locator_kind: None,
            aob: None,
            displacement_offset: None,
            instruction_size: None,
        };
    };

    match uniquify_at_rva(pe, site_rva, WildcardProfile::RipGlobal) {
        Ok(pattern) => SignatureReport {
            target: HookTarget::DisableIndependentFlip.label().into(),
            hook_target: HookTarget::DisableIndependentFlip,
            status: SignatureStatus::Ok,
            rva: Some(format_rva(site_rva)),
            locator_kind: Some(LocatorKind::RipRelativeGlobalAob),
            aob: Some(tokens_to_aob(&pattern.tokens)),
            displacement_offset: Some(2),
            instruction_size: Some(7),
        },
        Err(_error) => SignatureReport {
            target: HookTarget::DisableIndependentFlip.label().into(),
            hook_target: HookTarget::DisableIndependentFlip,
            status: SignatureStatus::UniquifyFailed,
            rva: Some(format_rva(site_rva)),
            locator_kind: None,
            aob: None,
            displacement_offset: None,
            instruction_size: None,
        },
    }
}

fn missing_symbol_report(target: HookTarget, error: SymbolResolveError) -> SignatureReport {
    SignatureReport {
        target: target.label().into(),
        hook_target: target,
        status: status_from_symbol_error(&error),
        rva: None,
        locator_kind: None,
        aob: None,
        displacement_offset: None,
        instruction_size: None,
    }
}

fn status_from_symbol_error(error: &SymbolResolveError) -> SignatureStatus {
    match error {
        SymbolResolveError::IcfStub => SignatureStatus::IcfStub,
        SymbolResolveError::Ambiguous => SignatureStatus::AmbiguousSymbol,
        SymbolResolveError::Missing | SymbolResolveError::UnsupportedTarget => {
            SignatureStatus::NoSymbol
        }
    }
}

#[derive(Clone, Copy)]
enum WildcardProfile {
    Function(HookTarget),
    RipGlobal,
}

struct UniquePattern {
    tokens: Vec<AobToken>,
}

fn uniquify_at_rva(
    pe: &PeImage,
    anchor_rva: u32,
    profile: WildcardProfile,
) -> Result<UniquePattern, String> {
    let mut insn_count = MIN_INSNS;
    loop {
        let Some(len) = end_after_insns(pe, anchor_rva, insn_count)? else {
            break;
        };

        let bytes = pe.bytes_at(anchor_rva, len)?;
        let tokens = apply_wildcards(bytes, profile);
        if matches!(
            match_aob(&pe.image, &tokens),
            AobMatch::Unique(Rva(offset)) if offset == anchor_rva as usize
        ) {
            return Ok(UniquePattern { tokens });
        }

        insn_count += 1;
    }
    Err(format!(
        "failed to uniquify AOB at rva={anchor_rva:#x} within max_len={MAX_LEN}"
    ))
}

fn end_after_insns(
    pe: &PeImage,
    anchor_rva: u32,
    insn_count: usize,
) -> Result<Option<usize>, String> {
    let available = (pe.image.len() - anchor_rva as usize).min(MAX_LEN + 15);
    if available == 0 {
        return Ok(None);
    }
    let bytes = pe.bytes_at(anchor_rva, available)?;
    let mut decoder = Decoder::with_ip(64, bytes, 0, DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    let mut covered = 0usize;
    let mut decoded = 0usize;
    while decoded < insn_count && decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            return Ok(None);
        }
        covered += instruction.len();
        decoded += 1;
    }
    if decoded < insn_count || covered > MAX_LEN {
        return Ok(None);
    }
    Ok(Some(covered))
}

fn apply_wildcards(bytes: &[u8], profile: WildcardProfile) -> Vec<AobToken> {
    let mut tokens: Vec<AobToken> = bytes.iter().copied().map(AobToken::Exact).collect();
    let mut decoder = Decoder::with_ip(64, bytes, 0, DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        let ip = instruction.ip() as usize;
        let len = instruction.len();
        if ip + len > tokens.len() {
            break;
        }

        let offsets = decoder.get_constant_offsets(&instruction);
        if offsets.has_displacement() {
            let start = ip + offsets.displacement_offset();
            let size = offsets.displacement_size();
            for token in tokens.iter_mut().skip(start).take(size) {
                *token = AobToken::Wildcard;
            }
        }

        if matches!(
            profile,
            WildcardProfile::Function(HookTarget::IsAdvancedDirectFlipCompatible)
        ) && bytes.get(ip) == Some(&0x74)
            && len == 2
        {
            tokens[ip + 1] = AobToken::Wildcard;
        }

        if matches!(profile, WildcardProfile::RipGlobal)
            && len >= 7
            && bytes.get(ip) == Some(&0x48)
            && bytes.get(ip + 1) == Some(&0x8B)
            && matches!(bytes.get(ip + 2), Some(0x96 | 0x97 | 0x86 | 0x87))
        {
            tokens[ip + 3] = AobToken::Wildcard;
            tokens[ip + 4] = AobToken::Wildcard;
        }
    }
    tokens
}

fn tokens_to_aob(tokens: &[AobToken]) -> String {
    tokens
        .iter()
        .map(|token| match token {
            AobToken::Exact(value) => format!("{value:02X}"),
            AobToken::Wildcard => "??".to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_cmp_rip_site(pe: &PeImage, global_rva: u32) -> Option<u32> {
    let image = &pe.image;
    let mut i = 0usize;
    while i + 7 <= image.len() {
        if image[i] == 0x83 && image[i + 1] == 0x3D {
            let disp = i32::from_le_bytes([image[i + 2], image[i + 3], image[i + 4], image[i + 5]]);
            let next_ip = (i as u32).wrapping_add(7);
            let target = next_ip.wrapping_add(disp as u32);
            if target == global_rva {
                return Some(i as u32);
            }
        }
        i += 1;
    }
    None
}

fn verify_rip_relative_points_to(
    pe: &PeImage,
    site_rva: u32,
    displacement_offset: usize,
    expected_global: u32,
) -> Result<(), String> {
    let bytes = pe.bytes_at(site_rva, 7)?;
    if bytes.len() < displacement_offset + 4 {
        return Err("instruction too short".into());
    }
    let disp = i32::from_le_bytes([
        bytes[displacement_offset],
        bytes[displacement_offset + 1],
        bytes[displacement_offset + 2],
        bytes[displacement_offset + 3],
    ]);
    let next_ip = site_rva.wrapping_add(7);
    let target = next_ip.wrapping_add(disp as u32);
    if target != expected_global {
        return Err(format!(
            "RIP target {target:#x} != expected {expected_global:#x}"
        ));
    }
    Ok(())
}
