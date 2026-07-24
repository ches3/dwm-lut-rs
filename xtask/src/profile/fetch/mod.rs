mod download;
mod pdb;
mod winbindex;

use std::error::Error;
use std::path::{Path, PathBuf};

use download::http_client;
use winbindex::resolve_amd64_pe;

use super::paths::{ensure_absent, output_dll_path, output_pdb_path, resolve_out_dir};
use super::pe::{FileVersion, PeImage};

pub(super) fn run_dll(version: String, out: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let version = FileVersion::parse(&version)?;
    let out_dir = resolve_out_dir(out.as_deref())?;
    fetch_dll(&version, &out_dir)?;
    Ok(())
}

pub(super) fn run_pdb(
    version: Option<String>,
    dll: Option<PathBuf>,
    out: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let out_dir = resolve_out_dir(out.as_deref())?;
    match (version, dll) {
        (Some(_), Some(_)) => Err("specify exactly one of --version or --dll".into()),
        (None, None) => Err("specify --version or --dll".into()),
        (Some(version), None) => {
            let version = FileVersion::parse(&version)?;
            let dll_path = output_dll_path(&out_dir, &version);
            if !dll_path.exists() {
                return Err(format!(
                    "{} not found; run `cargo xtask profile fetch dll --version {}` first",
                    dll_path.display(),
                    version.label()
                )
                .into());
            }
            fetch_pdb_for_dll(&dll_path, &out_dir)
        }
        (None, Some(dll)) => fetch_pdb_for_dll(&dll, &out_dir),
    }
}

pub(super) fn fetch_dll(version: &FileVersion, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let dll_path = output_dll_path(out_dir, version);
    ensure_absent(&dll_path)?;
    std::fs::create_dir_all(out_dir)
        .map_err(|error| format!("failed to create {}: {error}", out_dir.display()))?;

    let client = http_client()?;
    let candidate = resolve_amd64_pe(&client, version)?;
    download::download_to_file(&client, &candidate.pe_url(), &dll_path)?;

    let pe = match PeImage::load(&dll_path) {
        Ok(pe) => pe,
        Err(error) => {
            let _ = std::fs::remove_file(&dll_path);
            return Err(error.into());
        }
    };
    if pe.file_version != *version {
        let _ = std::fs::remove_file(&dll_path);
        return Err(format!(
            "downloaded DLL FileVersion mismatch: expected {}, got {}",
            version.label(),
            pe.file_version.label()
        )
        .into());
    }

    println!("dll: {}", dll_path.display());
    Ok(())
}

pub(super) fn fetch_pdb_for_dll(dll_path: &Path, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let pe = PeImage::load(dll_path)?;
    let pdb_path = output_pdb_path(out_dir, &pe.file_version);
    ensure_absent(&pdb_path)?;
    std::fs::create_dir_all(out_dir)
        .map_err(|error| format!("failed to create {}: {error}", out_dir.display()))?;

    let client = http_client()?;
    pdb::fetch_and_verify_pdb(&client, &pe, &pdb_path)?;
    println!("pdb: {}", pdb_path.display());
    Ok(())
}
