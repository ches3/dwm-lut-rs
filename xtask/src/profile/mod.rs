mod check;
mod ensure;
mod extract;
mod fetch;
mod paths;
mod pdb_publics;
mod pe;

use std::error::Error;
use std::path::PathBuf;

pub(crate) fn run_check(
    system: bool,
    dll: Option<PathBuf>,
    pdb: Option<PathBuf>,
    version: Option<String>,
) -> Result<(), Box<dyn Error>> {
    check::run(check::Args {
        system,
        dll,
        pdb,
        version,
    })
}

pub(crate) fn run_extract(
    system: bool,
    dll: Option<PathBuf>,
    pdb: Option<PathBuf>,
    version: Option<String>,
) -> Result<(), Box<dyn Error>> {
    extract::run(extract::Args {
        system,
        dll,
        pdb,
        version,
    })
}

pub(crate) fn run_fetch_dll(version: String, out: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    fetch::run_dll(version, out)
}

pub(crate) fn run_fetch_pdb(
    version: Option<String>,
    dll: Option<PathBuf>,
    out: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    fetch::run_pdb(version, dll, out)
}
