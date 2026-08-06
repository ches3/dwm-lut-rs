use iced_x86::{Decoder, DecoderOptions, Instruction};

use dwm_lut_profile::{AobMatch, AobToken, HookTarget, Rva, match_aob};

use super::report::{SignatureReport, SignatureStatus, format_rva};
use super::symbols::{
    EXTRACT_TARGETS, MAX_LEN, MIN_INSNS, ResolvedSymbol, SymbolResolveError,
    resolve_function_symbol,
};
use crate::profile::pdb_publics::PdbPublics;
use crate::profile::pe::PeImage;

pub fn extract_signatures(pe: &PeImage, pubs: &PdbPublics) -> Result<Vec<SignatureReport>, String> {
    Ok(EXTRACT_TARGETS
        .iter()
        .copied()
        .map(|target| extract_function_signature(target, pe, pubs))
        .collect())
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
    match uniquify_at_rva(pe, resolved.rva) {
        Ok(pattern) => SignatureReport {
            target: target.label().into(),
            hook_target: target,
            status: SignatureStatus::Ok,
            rva: Some(format_rva(resolved.rva)),
            aob: Some(tokens_to_aob(&pattern.tokens)),
        },
        Err(_error) => SignatureReport {
            target: target.label().into(),
            hook_target: target,
            status: SignatureStatus::UniquifyFailed,
            rva: Some(format_rva(resolved.rva)),
            aob: None,
        },
    }
}

fn missing_symbol_report(target: HookTarget, error: SymbolResolveError) -> SignatureReport {
    SignatureReport {
        target: target.label().into(),
        hook_target: target,
        status: status_from_symbol_error(&error),
        rva: None,
        aob: None,
    }
}

fn status_from_symbol_error(error: &SymbolResolveError) -> SignatureStatus {
    match error {
        SymbolResolveError::IcfStub => SignatureStatus::IcfStub,
        SymbolResolveError::Ambiguous => SignatureStatus::AmbiguousSymbol,
        SymbolResolveError::Missing => SignatureStatus::NoSymbol,
    }
}

struct UniquePattern {
    tokens: Vec<AobToken>,
}

fn uniquify_at_rva(pe: &PeImage, anchor_rva: u32) -> Result<UniquePattern, String> {
    let mut insn_count = MIN_INSNS;
    loop {
        let Some(len) = end_after_insns(pe, anchor_rva, insn_count)? else {
            break;
        };

        let bytes = pe.bytes_at(anchor_rva, len)?;
        let tokens = apply_wildcards(bytes);
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

fn apply_wildcards(bytes: &[u8]) -> Vec<AobToken> {
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
