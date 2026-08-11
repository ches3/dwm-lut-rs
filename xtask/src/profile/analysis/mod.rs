pub(super) mod abi;
pub(super) mod access;
mod aob;
mod field_offsets;
pub(super) mod model;
pub(super) mod symbols;
mod virtual_bases;
mod virtual_methods;

use std::error::Error;
use std::path::Path;

use dwm_lut_profile::VirtualMethodReturnType;

use self::abi::{AbiExtractionRow, extract_function_abis};
use self::access::RawAccessExtraction;
use self::aob::extract_aobs;
use self::field_offsets::extract_field_offsets;
use self::model::AobReport;
use self::virtual_bases::extract_virtual_bases;
use self::virtual_methods::extract_virtual_methods;
use super::pdb_publics::PdbPublics;
use super::pe::PeImage;

pub(super) struct ProfileAnalysis {
    pub pe: PeImage,
    pub pubs: PdbPublics,
    pub accesses: RawAccessExtraction,
    pub abis: Vec<AbiExtractionRow>,
}

impl ProfileAnalysis {
    pub fn extract_aobs(&self) -> Result<Vec<AobReport>, String> {
        extract_aobs(&self.pe, &self.pubs)
    }
}

pub(super) fn analyze(dll_path: &Path, pdb_path: &Path) -> Result<ProfileAnalysis, Box<dyn Error>> {
    let pe = PeImage::load(dll_path)?;
    let pubs = PdbPublics::load(pdb_path)?;
    pubs.verify_against_pe(&pe.codeview)?;
    let accesses = RawAccessExtraction {
        virtual_methods: extract_virtual_methods(&pe, &pubs),
        field_offsets: extract_field_offsets(&pe, &pubs),
        virtual_bases: extract_virtual_bases(&pe, &pubs),
    };
    let abis = extract_function_abis(&pe, &pubs)?;

    Ok(ProfileAnalysis {
        pe,
        pubs,
        accesses,
        abis,
    })
}

const fn virtual_method_return_type_label(value: VirtualMethodReturnType) -> &'static str {
    match value {
        VirtualMethodReturnType::PointerTo(type_name)
        | VirtualMethodReturnType::SretValue(type_name) => type_name,
        VirtualMethodReturnType::Unsigned32 => "u32",
        VirtualMethodReturnType::CppBool => "bool",
    }
}
