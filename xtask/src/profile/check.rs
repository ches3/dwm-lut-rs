use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use comfy_table::presets::NOTHING;
use comfy_table::{Cell, Color, Table, TableComponent};
use dwm_lut_profile::{
    DwmcoreVersion, FieldOffsetTarget, HookProfile, HookTarget, ProfileSelectError,
    SignatureScanError, VtableIndexTarget, resolve_signature_rva, select_profile,
};

use super::ensure;
use super::extract::field_offsets::extract_field_offsets;
use super::extract::report::{ExtractionRow, ExtractionStatus};
use super::extract::symbols::{SymbolResolveError, resolve_function_symbol};
use super::extract::vtable_indices::extract_vtable_indices;
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
    let vtable_indices = extract_vtable_indices(&pe, &pubs);
    let field_offsets = extract_field_offsets(&pe, &pubs);

    println!();
    println!("version  10.0.{version}");
    println!("profile  >= 10.0.{}", selected.min_version);
    println!("dll      {}", dll_path.display());
    println!("pdb      {}", pdb_path.display());
    println!();

    let vtable_index_failed = print_vtable_index_table(&profile, &vtable_indices);
    let field_offset_failed = print_field_offset_table(&profile, &field_offsets);
    let signature_failed = print_signatures_table(&profile, &pe, &pubs);
    mismatch_result(vtable_index_failed, field_offset_failed, signature_failed)
}

fn mismatch_result(
    vtable_index_failed: usize,
    field_offset_failed: usize,
    signature_failed: usize,
) -> Result<(), CheckError> {
    let mut failures = Vec::new();

    if vtable_index_failed != 0 {
        failures.push(format!(
            "{vtable_index_failed} vtable {} failed",
            if vtable_index_failed == 1 {
                "index"
            } else {
                "indices"
            }
        ));
    }
    if field_offset_failed != 0 {
        failures.push(format!(
            "{field_offset_failed} field offset{} failed",
            if field_offset_failed == 1 { "" } else { "s" }
        ));
    }
    if signature_failed != 0 {
        failures.push(format!(
            "{signature_failed} signature{} failed",
            if signature_failed == 1 { "" } else { "s" }
        ));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(CheckError::Mismatch(failures.join(", ")))
    }
}

fn print_vtable_index_table(
    profile: &HookProfile,
    extracted: &[ExtractionRow<VtableIndexTarget>],
) -> usize {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_style(TableComponent::HeaderLines, '─');
    table.set_header(vec![
        Cell::new("Vtable index"),
        Cell::new("Status"),
        Cell::new("Extracted"),
        Cell::new("Profile"),
    ]);

    let mut failed = 0usize;
    for &target in VtableIndexTarget::ALL {
        let profile_value = profile.vtable_index(target);
        let row = extracted.iter().find(|row| row.target == target);
        let (status, extracted_value) = match row {
            Some(row) if row.status == ExtractionStatus::Ok && row.value == Some(profile_value) => {
                (status_cell("ok", Color::Green), profile_value.to_string())
            }
            Some(row) if row.status == ExtractionStatus::Ok => {
                failed += 1;
                (
                    status_cell("mismatch", Color::Red),
                    row.value
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".into()),
                )
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
            Cell::new(target.label()),
            status,
            Cell::new(extracted_value),
            Cell::new(profile_value),
        ]);
    }

    println!("{table}");
    println!();
    failed
}

fn print_field_offset_table(
    profile: &HookProfile,
    extracted: &[ExtractionRow<FieldOffsetTarget>],
) -> usize {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_style(TableComponent::HeaderLines, '─');
    table.set_header(vec![
        Cell::new("Field offset"),
        Cell::new("Status"),
        Cell::new("Extracted"),
        Cell::new("Profile"),
    ]);

    let mut failed = 0usize;
    for &target in FieldOffsetTarget::ALL {
        let profile_value = profile.field_offset(target);
        let row = extracted.iter().find(|row| row.target == target);
        let (status, extracted_value) = match row {
            Some(row) if row.status == ExtractionStatus::Ok && row.value == Some(profile_value) => {
                (
                    status_cell("ok", Color::Green),
                    format!("{profile_value:#x}"),
                )
            }
            Some(row) if row.status == ExtractionStatus::Ok => {
                failed += 1;
                (
                    status_cell("mismatch", Color::Red),
                    row.value
                        .map(|value| format!("{value:#x}"))
                        .unwrap_or_else(|| "-".into()),
                )
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
            Cell::new(target.label()),
            status,
            Cell::new(extracted_value),
            Cell::new(format!("{profile_value:#x}")),
        ]);
    }

    println!("{table}");
    println!();
    failed
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
    resolve_function_symbol(target, pubs, pe).map(|symbol| symbol.rva)
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
        assert!(mismatch_result(0, 0, 0).is_ok());
    }

    #[test]
    fn vtable_index_failures_are_mismatch() {
        let error = mismatch_result(2, 0, 0).expect_err("mismatch");
        assert!(matches!(
            error,
            CheckError::Mismatch(message) if message == "2 vtable indices failed"
        ));
    }

    #[test]
    fn signature_failures_are_mismatch() {
        let error = mismatch_result(0, 0, 1).expect_err("mismatch");
        assert!(matches!(
            error,
            CheckError::Mismatch(message) if message == "1 signature failed"
        ));
    }

    #[test]
    fn combined_failures_are_mismatch() {
        let error = mismatch_result(1, 2, 3).expect_err("mismatch");
        assert!(matches!(
            error,
            CheckError::Mismatch(message)
                if message
                    == "1 vtable index failed, 2 field offsets failed, 3 signatures failed"
        ));
    }
}
