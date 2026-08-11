//! PDB symbol resolution used by profile analysis.

use dwm_lut_profile::{HookTarget, ParameterSpec, VtableView};

use crate::profile::pdb_publics::{PdbPublics, PublicSymbol, is_icf_false_stub};
use crate::profile::pe::PeImage;

pub const MIN_INSNS: usize = 4;
pub const MAX_LEN: usize = 128;

pub const EXTRACT_TARGETS: &[HookTarget] = HookTarget::ALL;

#[derive(Debug, Clone)]
pub struct ResolvedSymbol {
    pub rva: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolResolveError {
    Missing,
    IcfStub,
    Ambiguous,
}

pub fn resolve_function_symbol(
    target: HookTarget,
    pubs: &PdbPublics,
    pe: &PeImage,
) -> Result<ResolvedSymbol, SymbolResolveError> {
    let spec = target.extract_spec();
    let mut candidates = Vec::new();
    for prefix in qualified_method_symbol_prefixes(spec.class_name, spec.method_names) {
        candidates.extend(pubs.find_by_prefix(&prefix));
    }
    pick_function(&candidates, pe)
}

pub fn decorated_name_for_resolved_symbol(
    target: HookTarget,
    pubs: &PdbPublics,
    rva: u32,
) -> Option<String> {
    let spec = target.extract_spec();
    for prefix in qualified_method_symbol_prefixes(spec.class_name, spec.method_names) {
        if let Some(public) = pubs
            .find_by_prefix(&prefix)
            .into_iter()
            .find(|public| public.rva == rva)
        {
            return Some(public.name.clone());
        }
    }
    None
}

pub(super) fn qualified_method_symbol_prefixes(
    class_name: &str,
    method_names: &[&str],
) -> Vec<String> {
    method_names
        .iter()
        .map(|method_name| qualified_method_symbol_prefix(class_name, method_name))
        .collect()
}

fn qualified_method_symbol_prefix(class_name: &str, method_name: &str) -> String {
    format!("?{method_name}@{class_name}@@")
}

pub(super) fn demangle_marker(class_name: &str, method_name: &str) -> String {
    format!("{class_name}::{method_name}(")
}

pub(super) fn unqualified_method_symbol_prefix(method_name: &str) -> String {
    format!("?{method_name}@")
}

pub(super) fn ctor_symbol_prefix(class_name: &str) -> String {
    format!("??0{class_name}@@")
}

pub(super) fn vbtable_class_prefix(class_name: &str) -> String {
    format!("??_8{class_name}@@")
}

fn pointer_to_class_encoding(type_name: &str) -> String {
    format!("PEAV{type_name}@@")
}

pub(super) fn parameter_type_encoding(parameter: ParameterSpec) -> String {
    match parameter {
        ParameterSpec::PointerToClass(type_name) => pointer_to_class_encoding(type_name),
    }
}

pub(super) fn vftable_symbol(class_name: &str, view: VtableView) -> String {
    format!("??_7{class_name}@@{}", encode_vftable_view(view))
}

pub(super) fn vbtable_symbol(class_name: &str, view: VtableView) -> String {
    format!("??_8{class_name}@@{}", encode_vbtable_view(view))
}

fn encode_vftable_view(view: VtableView) -> String {
    match view {
        VtableView::Primary => "6B@".to_string(),
        VtableView::BasePath(bases) => encode_base_path("6B", bases),
        VtableView::DecoratedBasePath(fragment) => format!("6B{fragment}@"),
    }
}

fn encode_vbtable_view(view: VtableView) -> String {
    match view {
        VtableView::Primary => "7B@".to_string(),
        VtableView::BasePath(bases) => encode_base_path("7B", bases),
        VtableView::DecoratedBasePath(fragment) => format!("7B{fragment}@"),
    }
}

fn encode_base_path(qualifier: &str, bases: &[&str]) -> String {
    let mut encoded = qualifier.to_string();
    for base in bases {
        encoded.push_str(base);
        encoded.push_str("@@");
    }
    encoded.push('@');
    encoded
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

#[cfg(test)]
mod tests {
    use dwm_lut_profile::{ParameterSpec, VtableView};

    use super::{
        ctor_symbol_prefix, parameter_type_encoding, qualified_method_symbol_prefixes,
        unqualified_method_symbol_prefix, vbtable_class_prefix, vbtable_symbol, vftable_symbol,
    };

    #[test]
    fn encodes_qualified_and_unqualified_method_prefixes() {
        assert_eq!(
            qualified_method_symbol_prefixes("COverlayContext", &["Present", "PresentCompat"],),
            [
                "?Present@COverlayContext@@",
                "?PresentCompat@COverlayContext@@"
            ]
        );
        assert_eq!(
            unqualified_method_symbol_prefix("GetDisplayAdapterLuid"),
            "?GetDisplayAdapterLuid@"
        );
    }

    #[test]
    fn encodes_constructor_and_parameter_types() {
        assert_eq!(
            ctor_symbol_prefix("COverlayContext"),
            "??0COverlayContext@@"
        );
        assert_eq!(
            ctor_symbol_prefix("CDDisplaySwapChain"),
            "??0CDDisplaySwapChain@@"
        );
        assert_eq!(
            parameter_type_encoding(ParameterSpec::PointerToClass("IOverlayMonitorTarget")),
            "PEAVIOverlayMonitorTarget@@"
        );
    }

    #[test]
    fn encodes_vftable_symbols() {
        assert_eq!(
            vftable_symbol("CDDisplaySwapChainBuffer", VtableView::Primary),
            "??_7CDDisplaySwapChainBuffer@@6B@"
        );
        assert_eq!(
            vftable_symbol(
                "CDDisplaySwapChain",
                VtableView::BasePath(&["IDeviceResource"])
            ),
            "??_7CDDisplaySwapChain@@6BIDeviceResource@@@"
        );
        assert_eq!(
            vftable_symbol(
                "CDDisplayRenderTarget",
                VtableView::BasePath(&["IPixelFormat", "IOverlayMonitorTarget"])
            ),
            "??_7CDDisplayRenderTarget@@6BIPixelFormat@@IOverlayMonitorTarget@@@"
        );
        assert_eq!(
            vftable_symbol(
                "CDDisplaySwapChain",
                VtableView::DecoratedBasePath("?$CMILCOMBaseT@VIOverlaySwapChain@@@@")
            ),
            "??_7CDDisplaySwapChain@@6B?$CMILCOMBaseT@VIOverlaySwapChain@@@@@"
        );
    }

    #[test]
    fn encodes_vbtable_symbols() {
        assert_eq!(
            vbtable_class_prefix("CDDisplaySwapChain"),
            "??_8CDDisplaySwapChain@@"
        );
        assert_eq!(
            vbtable_symbol("CDDisplaySwapChain", VtableView::BasePath(&["ISwapChain"])),
            "??_8CDDisplaySwapChain@@7BISwapChain@@@"
        );
        assert_eq!(
            vbtable_symbol(
                "CLegacySwapChain",
                VtableView::BasePath(&["ISwapChain", "COverlaySwapChainBase"])
            ),
            "??_8CLegacySwapChain@@7BISwapChain@@COverlaySwapChainBase@@@"
        );
    }
}
