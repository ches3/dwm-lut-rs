pub(super) mod layout;
pub(super) mod report;
mod signatures;
pub(super) mod symbols;

use std::error::Error;
use std::path::{Path, PathBuf};

use layout::extract_layout;
use report::{InspectReport, print_report};
use signatures::extract_signatures;

use super::ensure;
use super::pdb_publics::PdbPublics;
use super::pe::PeImage;

pub(super) struct Args {
    pub system: bool,
    pub dll: Option<PathBuf>,
    pub pdb: Option<PathBuf>,
    pub version: Option<String>,
}

pub(super) fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let (dll, pdb) = ensure::resolve_inputs(
        args.system,
        args.version.as_ref(),
        args.dll.as_ref(),
        args.pdb.as_ref(),
    )?;
    let report = extract(&dll, &pdb)?;
    print_report(&report);
    Ok(())
}

fn extract(dll_path: &Path, pdb_path: &Path) -> Result<InspectReport, Box<dyn Error>> {
    let pe = PeImage::load(dll_path)?;
    let pubs = PdbPublics::load(pdb_path)?;
    pubs.verify_against_pe(&pe.codeview)?;
    let signatures = extract_signatures(&pe, &pubs)?;
    let layout = extract_layout(&pe, &pubs);

    Ok(InspectReport {
        file_version: pe.file_version.label(),
        dll: dll_path.display().to_string(),
        pdb: pdb_path.display().to_string(),
        signatures,
        layout,
    })
}
