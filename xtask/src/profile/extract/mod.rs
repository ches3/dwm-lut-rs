mod report;

use std::error::Error;
use std::path::PathBuf;

use report::{InspectReport, print_report};

use super::analysis::access::build_access_reports;
use super::analysis::analyze;
use super::ensure;

pub(super) struct Args {
    pub system: bool,
    pub dll: Option<PathBuf>,
    pub pdb: Option<PathBuf>,
    pub version: Option<String>,
    pub build_latest: Option<u16>,
    pub yes: bool,
}

pub(super) fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let (dll, pdb) = ensure::resolve_inputs(
        args.system,
        args.version.as_ref(),
        args.build_latest,
        args.dll.as_ref(),
        args.pdb.as_ref(),
        args.yes,
    )?;
    let analysis = analyze(&dll, &pdb)?;
    let report = InspectReport {
        file_version: analysis.pe.file_version.label(),
        dll: dll.display().to_string(),
        pdb: pdb.display().to_string(),
        accesses: build_access_reports(&analysis.accesses),
        aobs: analysis.extract_aobs()?,
        abis: analysis.abis,
    };
    print_report(&report);
    Ok(())
}
