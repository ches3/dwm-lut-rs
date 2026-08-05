mod check;
mod ensure;
mod extract;
mod fetch;
mod paths;
mod pdb_publics;
mod pe;

use std::error::Error;
use std::path::PathBuf;

pub(crate) use check::CheckError;

pub(crate) fn run_check(
    system: bool,
    dll: Option<PathBuf>,
    pdb: Option<PathBuf>,
    version: Option<String>,
    build_latest: Option<u16>,
    yes: bool,
) -> Result<(), CheckError> {
    check::run(check::Args {
        system,
        dll,
        pdb,
        version,
        build_latest,
        yes,
    })
}

pub(crate) fn run_extract(
    system: bool,
    dll: Option<PathBuf>,
    pdb: Option<PathBuf>,
    version: Option<String>,
    build_latest: Option<u16>,
    yes: bool,
) -> Result<(), Box<dyn Error>> {
    extract::run(extract::Args {
        system,
        dll,
        pdb,
        version,
        build_latest,
        yes,
    })
}

pub(crate) fn run_fetch_dll(
    version: Option<String>,
    build_latest: Option<u16>,
    out: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    fetch::run_dll(version, build_latest, out)
}

pub(crate) fn run_fetch_pdb(
    version: Option<String>,
    build_latest: Option<u16>,
    dll: Option<PathBuf>,
    out: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    fetch::run_pdb(version, build_latest, dll, out)
}
