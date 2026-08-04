use dwm_lut_profile::HookTarget;

use crate::profile::pdb_publics::{PdbPublics, PublicSymbol, is_icf_false_stub};
use crate::profile::pe::PeImage;

pub const MIN_INSNS: usize = 4;
pub const MAX_LEN: usize = 128;

pub const EXTRACT_TARGETS: &[HookTarget] = &[
    HookTarget::Present,
    HookTarget::IsCandidateDirectFlipCompatible,
    HookTarget::DirectFlipInfoEnsureIndependentFlipState,
    HookTarget::IsDirectFlipSupportedOnTarget,
    HookTarget::LegacySwapChainCheckDirectFlipSupport,
    HookTarget::IsAdvancedDirectFlipCompatible,
    HookTarget::OverlayTestMode,
    HookTarget::DisableIndependentFlip,
    HookTarget::OverlaysEnabled,
];

#[derive(Debug, Clone)]
pub struct ResolvedSymbol {
    pub rva: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolResolveError {
    Missing,
    IcfStub,
    Ambiguous,
    UnsupportedTarget,
}

pub fn resolve_function_symbol(
    target: HookTarget,
    pubs: &PdbPublics,
    pe: &PeImage,
) -> Result<ResolvedSymbol, SymbolResolveError> {
    if !target.is_function_hook_target() {
        return Err(SymbolResolveError::UnsupportedTarget);
    }

    pick_function(&pubs.find_by_prefix(target.pdb_symbol_prefix()), pe)
}

pub fn resolve_global_symbol(
    target: HookTarget,
    pubs: &PdbPublics,
) -> Result<&PublicSymbol, SymbolResolveError> {
    if target.is_function_hook_target() {
        return Err(SymbolResolveError::UnsupportedTarget);
    }

    pubs.find_by_prefix(target.pdb_symbol_prefix())
        .into_iter()
        .next()
        .ok_or(SymbolResolveError::Missing)
}

fn pick_function(
    candidates: &[&PublicSymbol],
    pe: &PeImage,
) -> Result<ResolvedSymbol, SymbolResolveError> {
    if candidates.is_empty() {
        return Err(SymbolResolveError::Missing);
    }

    let mut usable_rvas = Vec::new();
    for candidate in candidates {
        let Ok(prologue) = pe.bytes_at(candidate.rva, 3) else {
            continue;
        };
        if is_icf_false_stub(prologue) {
            continue;
        }
        usable_rvas.push(candidate.rva);
    }

    if usable_rvas.is_empty() {
        return Err(SymbolResolveError::IcfStub);
    }

    usable_rvas.sort_unstable();
    usable_rvas.dedup();
    if usable_rvas.len() > 1 {
        return Err(SymbolResolveError::Ambiguous);
    }

    Ok(ResolvedSymbol {
        rva: usable_rvas[0],
    })
}
