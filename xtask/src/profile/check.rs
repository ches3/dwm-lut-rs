use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use comfy_table::presets::NOTHING;
use comfy_table::{Cell, Color, Table, TableComponent};
use dwm_lut_profile::{
    DwmcoreVersion, HookProfile, HookTarget, ProfileSelectError, SignatureScanError,
    resolve_signature_rva, select_profile,
};

use super::ensure;
use super::extract::layout::extract_layout;
use super::extract::report::{LayoutRow, LayoutStatus, format_hex};
use super::extract::symbols::{SymbolResolveError, resolve_function_symbol, resolve_global_symbol};
use super::pdb_publics::PdbPublics;
use super::pe::PeImage;

#[derive(Debug)]
pub enum CheckError {
    Mismatch(String),
    Other(Box<dyn Error>),
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mismatch(message) => f.write_str(message),
            Self::Other(error) => write!(f, "{error}"),
        }
    }
}

impl Error for CheckError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Mismatch(_) => None,
            Self::Other(error) => Some(error.as_ref()),
        }
    }
}

impl From<Box<dyn Error>> for CheckError {
    fn from(error: Box<dyn Error>) -> Self {
        Self::Other(error)
    }
}

impl From<String> for CheckError {
    fn from(error: String) -> Self {
        Self::Other(error.into())
    }
}

impl From<ProfileSelectError> for CheckError {
    fn from(error: ProfileSelectError) -> Self {
        Self::Other(Box::new(error))
    }
}

pub(super) struct Args {
    pub system: bool,
    pub dll: Option<PathBuf>,
    pub pdb: Option<PathBuf>,
    pub version: Option<String>,
    pub build_latest: Option<u16>,
    pub yes: bool,
}

pub(super) fn run(args: Args) -> Result<(), CheckError> {
    let (dll_path, pdb_path) = ensure::resolve_inputs(
        args.system,
        args.version.as_ref(),
        args.build_latest,
        args.dll.as_ref(),
        args.pdb.as_ref(),
        args.yes,
    )?;

    let pe = PeImage::load(&dll_path)?;
    let version = DwmcoreVersion {
        build: u32::from(pe.file_version.build),
        revision: u32::from(pe.file_version.revision),
    };
    let selected = select_profile(version)?;
    let profile = (selected.profile)();

    let pubs = PdbPublics::load(&pdb_path)?;
    pubs.verify_against_pe(&pe.codeview)?;
    let layout = extract_layout(&pe, &pubs);

    println!();
    println!("version  10.0.{version}");
    println!("profile  >= 10.0.{}", selected.min_version);
    println!("dll      {}", dll_path.display());
    println!("pdb      {}", pdb_path.display());
    println!();

    let layout_failed = print_layout_table(&profile, &layout.rows);
    let signature_failed = print_signatures_table(&profile, &pe, &pubs);
    mismatch_result(layout_failed, signature_failed)
}

fn mismatch_result(layout_failed: usize, signature_failed: usize) -> Result<(), CheckError> {
    match (layout_failed, signature_failed) {
        (0, 0) => Ok(()),
        (layouts, 0) => Err(CheckError::Mismatch(format!(
            "{layouts} layout value{} failed",
            if layouts == 1 { "" } else { "s" }
        ))),
        (0, signatures) => Err(CheckError::Mismatch(format!(
            "{signatures} signature{} failed",
            if signatures == 1 { "" } else { "s" }
        ))),
        (layouts, signatures) => Err(CheckError::Mismatch(format!(
            "{layouts} layout value{} failed, {signatures} signature{} failed",
            if layouts == 1 { "" } else { "s" },
            if signatures == 1 { "" } else { "s" }
        ))),
    }
}

fn print_layout_table(profile: &HookProfile, extracted: &[LayoutRow]) -> usize {
    let expected = profile_layout_values(profile);
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_style(TableComponent::HeaderLines, '─');
    table.set_header(vec![
        Cell::new("Layout"),
        Cell::new("Status"),
        Cell::new("Extracted"),
        Cell::new("Profile"),
    ]);

    let mut failed = 0usize;
    for (name, profile_value) in &expected {
        let row = extracted.iter().find(|row| row.target == *name);
        let (status, extracted_value) = match row {
            Some(row) if row.status == LayoutStatus::Ok => {
                let value = row.value.as_deref().unwrap_or("-");
                if value == profile_value {
                    (status_cell("ok", Color::Green), value.to_string())
                } else {
                    failed += 1;
                    (status_cell("mismatch", Color::Red), value.to_string())
                }
            }
            Some(row) => {
                failed += 1;
                (status_cell(row.status.as_str(), Color::Red), "-".into())
            }
            None => {
                failed += 1;
                (status_cell("missing", Color::Red), "-".into())
            }
        };

        table.add_row(vec![
            Cell::new(*name),
            status,
            Cell::new(extracted_value),
            Cell::new(profile_value),
        ]);
    }

    println!("{table}");
    println!();
    failed
}

fn profile_layout_values(profile: &HookProfile) -> [(&'static str, String); 6] {
    [
        (
            "container_vtable_index",
            profile.swap_chain.container_vtable_index.to_string(),
        ),
        (
            "resource_vtable_index",
            profile.swap_chain.resource_vtable_index.to_string(),
        ),
        (
            "hardware_protected",
            format_hex(profile.hardware_protected_offset),
        ),
        (
            "adapter_luid_low_offset",
            format_hex(profile.monitor_identity.adapter_luid_low_offset),
        ),
        (
            "adapter_luid_high_offset",
            format_hex(profile.monitor_identity.adapter_luid_high_offset),
        ),
        (
            "target_id_offset",
            format_hex(profile.monitor_identity.target_id_offset),
        ),
    ]
}

fn print_signatures_table(profile: &HookProfile, pe: &PeImage, pubs: &PdbPublics) -> usize {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_style(TableComponent::HeaderLines, '─');
    table.set_header(vec![
        Cell::new("Signature"),
        Cell::new("Status"),
        Cell::new("RVA"),
    ]);

    let mut signature_failed = 0usize;

    for signature in profile.signatures {
        let label = signature.target.label();

        let (status, rva) = match resolve_signature_rva(&pe.image, signature) {
            Ok(resolved) => match expected_symbol_rva(signature.target, pe, pubs) {
                Ok(symbol_rva) if symbol_rva as usize == resolved.rva.0 => (
                    status_cell("ok", Color::Green),
                    format!("{:#x}", resolved.rva.0),
                ),
                Ok(_) => {
                    signature_failed += 1;
                    (
                        status_cell("symbol_mismatch", Color::Red),
                        format!("{:#x}", resolved.rva.0),
                    )
                }
                Err(error) => {
                    signature_failed += 1;
                    let status = if error == SymbolResolveError::Ambiguous {
                        "ambiguous_symbol"
                    } else {
                        "no_symbol"
                    };
                    (
                        status_cell(status, Color::Red),
                        format!("{:#x}", resolved.rva.0),
                    )
                }
            },
            Err(SignatureScanError::NotFound { .. }) => {
                signature_failed += 1;
                (status_cell("not_found", Color::Red), "-".into())
            }
            Err(SignatureScanError::Ambiguous { .. }) => {
                signature_failed += 1;
                (status_cell("ambiguous", Color::Red), "-".into())
            }
            Err(SignatureScanError::OutOfBounds { .. }) => {
                signature_failed += 1;
                (status_cell("out_of_bounds", Color::Red), "-".into())
            }
            Err(SignatureScanError::IncompatibleLocator { .. }) => {
                signature_failed += 1;
                (status_cell("incompatible_locator", Color::Red), "-".into())
            }
        };

        table.add_row(vec![Cell::new(label), status, Cell::new(rva)]);
    }

    println!("{table}");
    signature_failed
}

fn expected_symbol_rva(
    target: HookTarget,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<u32, SymbolResolveError> {
    if target.is_function_hook_target() {
        resolve_function_symbol(target, pubs, pe).map(|symbol| symbol.rva)
    } else {
        resolve_global_symbol(target, pubs).map(|symbol| symbol.rva)
    }
}

fn status_cell(text: &str, color: Color) -> Cell {
    let cell = Cell::new(text);
    if stdout_supports_color() {
        cell.fg(color)
    } else {
        cell
    }
}

fn stdout_supports_color() -> bool {
    supports_color::on_cached(supports_color::Stream::Stdout).is_some()
}

#[cfg(test)]
mod tests {
    use super::{CheckError, mismatch_result};

    #[test]
    fn zero_failures_succeed() {
        assert!(mismatch_result(0, 0).is_ok());
    }

    #[test]
    fn layout_failures_are_mismatch() {
        let error = mismatch_result(2, 0).expect_err("mismatch");
        assert!(matches!(
            error,
            CheckError::Mismatch(message) if message == "2 layout values failed"
        ));
    }

    #[test]
    fn signature_failures_are_mismatch() {
        let error = mismatch_result(0, 1).expect_err("mismatch");
        assert!(matches!(
            error,
            CheckError::Mismatch(message) if message == "1 signature failed"
        ));
    }

    #[test]
    fn combined_failures_are_mismatch() {
        let error = mismatch_result(1, 2).expect_err("mismatch");
        assert!(matches!(
            error,
            CheckError::Mismatch(message)
                if message == "1 layout value failed, 2 signatures failed"
        ));
    }
}
