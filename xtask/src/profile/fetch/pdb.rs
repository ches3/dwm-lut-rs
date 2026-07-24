use std::error::Error;
use std::io::{ErrorKind, Write};
use std::path::Path;

use reqwest::blocking::Client;
use tempfile::Builder;

use crate::profile::pdb_publics::PdbPublics;
use crate::profile::pe::{CodeViewInfo, PeImage};

use super::download::download_bytes;

const MSDL_SYMBOLS_BASE: &str = "https://msdl.microsoft.com/download/symbols";

pub fn pdb_symbol_url(codeview: &CodeViewInfo) -> String {
    let pdb_name = Path::new(&codeview.pdb_file_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(codeview.pdb_file_name.as_str());
    let guid = codeview.guid.as_simple().to_string().to_uppercase();
    format!(
        "{MSDL_SYMBOLS_BASE}/{pdb_name}/{guid}{:X}/{pdb_name}",
        codeview.age
    )
}

pub fn fetch_and_verify_pdb(
    client: &Client,
    pe: &PeImage,
    pdb_path: &Path,
) -> Result<(), Box<dyn Error>> {
    let url = pdb_symbol_url(&pe.codeview);
    let parent = pdb_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let bytes = download_bytes(client, &url)?;
    let mut tmp = Builder::new()
        .prefix("dwmcore-")
        .suffix(".pdb.tmp")
        .tempfile_in(parent)
        .map_err(|error| {
            format!(
                "failed to create temporary PDB in {}: {error}",
                parent.display()
            )
        })?;
    tmp.write_all(&bytes)
        .map_err(|error| format!("failed to write temporary PDB: {error}"))?;
    tmp.flush()
        .map_err(|error| format!("failed to flush temporary PDB: {error}"))?;

    let pubs = PdbPublics::load(tmp.path())?;
    pubs.verify_against_pe(&pe.codeview)?;

    tmp.persist_noclobber(pdb_path).map_err(|error| {
        if error.error.kind() == ErrorKind::AlreadyExists {
            format!(
                "{} already exists; remove it before fetching",
                pdb_path.display()
            )
        } else {
            format!(
                "failed to move temporary PDB to {}: {}",
                pdb_path.display(),
                error.error
            )
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::profile::pe::CodeViewInfo;

    use super::pdb_symbol_url;

    #[test]
    fn builds_pdb_symbol_url() {
        let codeview = CodeViewInfo {
            guid: Uuid::parse_str("01234567-89ab-cdef-0123-456789abcdef").expect("guid"),
            age: 1,
            pdb_file_name: "dwmcore.pdb".into(),
        };
        assert_eq!(
            pdb_symbol_url(&codeview),
            "https://msdl.microsoft.com/download/symbols/dwmcore.pdb/0123456789ABCDEF0123456789ABCDEF1/dwmcore.pdb"
        );
    }
}
