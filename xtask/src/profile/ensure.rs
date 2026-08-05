use std::error::Error;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use super::fetch;
use super::paths::{output_dll_path, output_pdb_path, resolve_out_dir, system_dwmcore_path};
use super::pe::{FileVersion, PeImage};

pub(super) struct VersionArtifacts {
    pub dll: PathBuf,
    pub pdb: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum VersionSource {
    Exact,
    BuildLatest { build: u16 },
}

pub(super) fn resolve_inputs(
    system: bool,
    version: Option<&String>,
    build_latest: Option<u16>,
    dll: Option<&PathBuf>,
    pdb: Option<&PathBuf>,
    yes: bool,
) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    match (system, version, build_latest, dll, pdb) {
        (true, None, None, None, None) => {
            let artifacts = ensure_system_artifacts(yes)?;
            Ok((artifacts.dll, artifacts.pdb))
        }
        (false, Some(version), None, None, None) => {
            let version = FileVersion::parse(version)?;
            let artifacts = ensure_version_artifacts(&version, VersionSource::Exact, true, yes)?;
            Ok((artifacts.dll, artifacts.pdb))
        }
        (false, None, Some(build), None, None) => {
            let version = fetch::resolve_build_latest_version(build)?;
            let artifacts = ensure_version_artifacts(
                &version,
                VersionSource::BuildLatest { build },
                true,
                yes,
            )?;
            Ok((artifacts.dll, artifacts.pdb))
        }
        (false, None, None, Some(dll), Some(pdb)) => Ok((dll.clone(), pdb.clone())),
        (false, None, None, None, None) => {
            Err("specify --system, --version, --build-latest, or both --dll and --pdb".into())
        }
        _ => Err(
            "specify exactly one of --system, --version, --build-latest, or both --dll and --pdb"
                .into(),
        ),
    }
}

pub(super) fn ensure_version_artifacts(
    version: &FileVersion,
    source: VersionSource,
    need_pdb: bool,
    yes: bool,
) -> Result<VersionArtifacts, Box<dyn Error>> {
    let out_dir = resolve_out_dir(None)?;
    let dll = output_dll_path(&out_dir, version);
    let pdb = output_pdb_path(&out_dir, version);

    let mut kinds = Vec::new();
    if !dll.exists() {
        kinds.push("dll");
    }
    if need_pdb && !pdb.exists() {
        kinds.push("pdb");
    }

    if !kinds.is_empty() {
        let hint = version_fetch_hint(source, version);
        let message = format!(
            "Missing {} for {}; re-run with -y or: {hint}",
            kinds.join(", "),
            version.label()
        );
        if !confirm_fetch(&message, yes)? {
            return Err("fetch cancelled".into());
        }
        if !dll.exists() {
            fetch::fetch_dll(version, &out_dir)?;
        }
        if need_pdb && !pdb.exists() {
            fetch::fetch_pdb_for_dll(&dll, &out_dir)?;
        }
    }

    Ok(VersionArtifacts { dll, pdb })
}

pub(super) fn ensure_system_artifacts(yes: bool) -> Result<VersionArtifacts, Box<dyn Error>> {
    let dll = system_dwmcore_path()?;
    if !dll.exists() {
        return Err(format!("{} not found", dll.display()).into());
    }

    let pe = PeImage::load(&dll)?;
    let version = pe.file_version;
    let out_dir = resolve_out_dir(None)?;
    let pdb = output_pdb_path(&out_dir, &version);

    if !pdb.exists() {
        let hint = format!("cargo xtask profile fetch pdb --dll {}", dll.display());
        let message = format!(
            "Missing pdb for system {}; re-run with -y or: {hint}",
            version.label()
        );
        if !confirm_fetch(&message, yes)? {
            return Err("fetch cancelled".into());
        }
        fetch::fetch_pdb_for_dll(&dll, &out_dir)?;
    }

    Ok(VersionArtifacts { dll, pdb })
}

fn version_fetch_hint(source: VersionSource, version: &FileVersion) -> String {
    match source {
        VersionSource::BuildLatest { build } => format!(
            "cargo xtask profile fetch dll --build-latest {build} && cargo xtask profile fetch pdb --build-latest {build}"
        ),
        VersionSource::Exact => format!(
            "cargo xtask profile fetch dll --version {0} && cargo xtask profile fetch pdb --version {0}",
            version.label()
        ),
    }
}

fn confirm_fetch(message: &str, yes: bool) -> Result<bool, Box<dyn Error>> {
    if yes {
        return Ok(true);
    }

    let interactive = io::stdin().is_terminal();
    eprintln!("{message}");
    if interactive {
        eprint!("Fetch? [Y/n] ");
        io::stderr().flush()?;
    }

    let mut line = String::new();
    if io::stdin().read_line(&mut line)? == 0 {
        if !interactive {
            return Err(message.to_string().into());
        }
        return Ok(false);
    }
    let answer = line.trim();
    Ok(answer.is_empty() || matches!(answer, "y" | "Y"))
}
