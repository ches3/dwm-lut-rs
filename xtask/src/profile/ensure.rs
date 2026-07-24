use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;

use super::fetch;
use super::paths::{output_dll_path, output_pdb_path, resolve_out_dir, system_dwmcore_path};
use super::pe::{FileVersion, PeImage};

pub(super) struct VersionArtifacts {
    pub dll: PathBuf,
    pub pdb: PathBuf,
}

pub(super) fn resolve_inputs(
    system: bool,
    version: Option<&String>,
    dll: Option<&PathBuf>,
    pdb: Option<&PathBuf>,
) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    match (system, version, dll, pdb) {
        (true, None, None, None) => {
            let artifacts = ensure_system_artifacts()?;
            Ok((artifacts.dll, artifacts.pdb))
        }
        (false, Some(version), None, None) => {
            let artifacts = ensure_version_artifacts(version, true)?;
            Ok((artifacts.dll, artifacts.pdb))
        }
        (false, None, Some(dll), Some(pdb)) => Ok((dll.clone(), pdb.clone())),
        (false, None, None, None) => {
            Err("specify --system, --version, or both --dll and --pdb".into())
        }
        _ => Err("specify exactly one of --system, --version, or both --dll and --pdb".into()),
    }
}

pub(super) fn ensure_version_artifacts(
    version: &str,
    need_pdb: bool,
) -> Result<VersionArtifacts, Box<dyn Error>> {
    let version = FileVersion::parse(version)?;
    let out_dir = resolve_out_dir(None)?;
    let dll = output_dll_path(&out_dir, &version);
    let pdb = output_pdb_path(&out_dir, &version);

    let mut kinds = Vec::new();
    if !dll.exists() {
        kinds.push("dll");
    }
    if need_pdb && !pdb.exists() {
        kinds.push("pdb");
    }

    if !kinds.is_empty() {
        let message = format!("Missing {} for {}", kinds.join(", "), version.label());
        if !confirm_fetch(&message)? {
            return Err("fetch cancelled".into());
        }
        if !dll.exists() {
            fetch::fetch_dll(&version, &out_dir)?;
        }
        if need_pdb && !pdb.exists() {
            fetch::fetch_pdb_for_dll(&dll, &out_dir)?;
        }
    }

    Ok(VersionArtifacts { dll, pdb })
}

pub(super) fn ensure_system_artifacts() -> Result<VersionArtifacts, Box<dyn Error>> {
    let dll = system_dwmcore_path()?;
    if !dll.exists() {
        return Err(format!("{} not found", dll.display()).into());
    }

    let pe = PeImage::load(&dll)?;
    let version = pe.file_version;
    let out_dir = resolve_out_dir(None)?;
    let pdb = output_pdb_path(&out_dir, &version);

    if !pdb.exists() {
        let message = format!("Missing pdb for system {}", version.label());
        if !confirm_fetch(&message)? {
            return Err("fetch cancelled".into());
        }
        fetch::fetch_pdb_for_dll(&dll, &out_dir)?;
    }

    Ok(VersionArtifacts { dll, pdb })
}

fn confirm_fetch(message: &str) -> Result<bool, Box<dyn Error>> {
    eprintln!("{message}");
    eprint!("Fetch? [Y/n] ");
    io::stderr().flush()?;

    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        return Ok(false);
    }
    let answer = line.trim();
    Ok(answer.is_empty() || matches!(answer, "y" | "Y"))
}
