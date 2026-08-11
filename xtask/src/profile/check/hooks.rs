use dwm_lut_profile::{
    HookEntry, HookProfile, HookTarget, SignatureScanError, resolve_signature_rva,
};

use super::super::analysis::abi::{AbiExtractionRow, AbiExtractionStatus, abi_matches_profile};
use super::super::analysis::symbols::{SymbolResolveError, resolve_function_symbol};
use super::super::pdb_publics::PdbPublics;
use super::super::pe::PeImage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCheckReport {
    pub rows: Vec<HookCheckRow>,
}

impl HookCheckReport {
    pub fn aob_failed_count(&self) -> usize {
        self.rows.iter().filter(|row| !row.aob.is_ok()).count()
    }

    pub fn abi_failed_count(&self) -> usize {
        self.rows.iter().filter(|row| !row.abi.is_ok()).count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookCheckRow {
    pub target: HookTarget,
    pub aob: AobCheckStatus,
    pub abi: AbiCheckStatus,
}

impl HookCheckRow {
    pub const fn status_label(self) -> &'static str {
        if self.aob.is_ok() {
            self.abi.as_str()
        } else {
            self.aob.as_str()
        }
    }

    pub const fn is_ok(self) -> bool {
        self.aob.is_ok() && self.abi.is_ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AobCheckStatus {
    Ok,
    SymbolMismatch,
    AmbiguousSymbol,
    NoSymbol,
    NotFound,
    Ambiguous,
}

impl AobCheckStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::SymbolMismatch => "symbol_mismatch",
            Self::AmbiguousSymbol => "ambiguous_symbol",
            Self::NoSymbol => "no_symbol",
            Self::NotFound => "not_found",
            Self::Ambiguous => "ambiguous",
        }
    }

    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiCheckStatus {
    Ok,
    Mismatch,
    Extraction(AbiExtractionStatus),
    Missing,
}

impl AbiCheckStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Mismatch => "mismatch",
            Self::Extraction(status) => status.as_str(),
            Self::Missing => "missing",
        }
    }

    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

pub fn build_hook_check_report(
    profile: &HookProfile,
    pe: &PeImage,
    pubs: &PdbPublics,
    extracted_abis: &[AbiExtractionRow],
) -> HookCheckReport {
    HookCheckReport {
        rows: profile
            .hooks
            .iter()
            .copied()
            .map(|entry| HookCheckRow {
                target: entry.target,
                aob: check_aob(entry, pe, pubs),
                abi: check_abi(entry, extracted_abis),
            })
            .collect(),
    }
}

fn check_aob(entry: HookEntry, pe: &PeImage, pubs: &PdbPublics) -> AobCheckStatus {
    match resolve_signature_rva(&pe.image, &entry) {
        Ok(resolved) => match resolve_function_symbol(entry.target, pubs, pe) {
            Ok(symbol) if symbol.rva as usize == resolved.rva.0 => AobCheckStatus::Ok,
            Ok(_) => AobCheckStatus::SymbolMismatch,
            Err(SymbolResolveError::Ambiguous) => AobCheckStatus::AmbiguousSymbol,
            Err(SymbolResolveError::Missing | SymbolResolveError::IcfStub) => {
                AobCheckStatus::NoSymbol
            }
        },
        Err(SignatureScanError::NotFound { .. }) => AobCheckStatus::NotFound,
        Err(SignatureScanError::Ambiguous { .. }) => AobCheckStatus::Ambiguous,
    }
}

fn check_abi(entry: HookEntry, extracted_abis: &[AbiExtractionRow]) -> AbiCheckStatus {
    match extracted_abis.iter().find(|row| row.target == entry.target) {
        Some(row)
            if row.status == AbiExtractionStatus::Ok
                && row
                    .abi
                    .as_ref()
                    .is_some_and(|abi| abi_matches_profile(entry, abi)) =>
        {
            AbiCheckStatus::Ok
        }
        Some(row) if row.status == AbiExtractionStatus::Ok => AbiCheckStatus::Mismatch,
        Some(row) => AbiCheckStatus::Extraction(row.status),
        None => AbiCheckStatus::Missing,
    }
}

#[cfg(test)]
mod tests {
    use dwm_lut_profile::HookTarget;

    use super::{AbiCheckStatus, AobCheckStatus, HookCheckReport, HookCheckRow};

    #[test]
    fn report_counts_aob_and_abi_failures_independently() {
        let report = HookCheckReport {
            rows: vec![
                HookCheckRow {
                    target: HookTarget::Present,
                    aob: AobCheckStatus::SymbolMismatch,
                    abi: AbiCheckStatus::Ok,
                },
                HookCheckRow {
                    target: HookTarget::IsCandidateDirectFlipCompatible,
                    aob: AobCheckStatus::Ok,
                    abi: AbiCheckStatus::Mismatch,
                },
            ],
        };

        assert_eq!(report.aob_failed_count(), 1);
        assert_eq!(report.abi_failed_count(), 1);
        assert_eq!(report.rows[0].status_label(), "symbol_mismatch");
        assert_eq!(report.rows[1].status_label(), "mismatch");
    }
}
