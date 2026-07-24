use std::env;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};

use super::pe::FileVersion;

pub(super) const DEFAULT_OUT_DIR: &str = "dwmcore";

pub(super) fn workspace_root() -> Result<PathBuf, Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("xtask is located directly under the workspace root")?;
    Ok(root.to_path_buf())
}

pub(super) fn resolve_out_dir(out: Option<&Path>) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(out) = out {
        return Ok(out.to_path_buf());
    }
    Ok(workspace_root()?.join(DEFAULT_OUT_DIR))
}

pub(super) fn system_dwmcore_path() -> Result<PathBuf, Box<dyn Error>> {
    let system_root = env::var_os("SystemRoot").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "SystemRoot environment variable is not set",
        )
    })?;
    Ok(Path::new(&system_root).join("System32").join("dwmcore.dll"))
}

pub(super) fn output_dll_path(out_dir: &Path, version: &FileVersion) -> PathBuf {
    out_dir.join(format!("dwmcore-{}.dll", version.label()))
}

pub(super) fn output_pdb_path(out_dir: &Path, version: &FileVersion) -> PathBuf {
    out_dir.join(format!("dwmcore-{}.pdb", version.label()))
}

pub(super) fn ensure_absent(path: &Path) -> Result<(), Box<dyn Error>> {
    if path.exists() {
        return Err(format!(
            "{} already exists; remove it before fetching",
            path.display()
        )
        .into());
    }
    Ok(())
}
