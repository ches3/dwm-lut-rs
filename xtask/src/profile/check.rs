mod access;
mod hooks;

use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use comfy_table::presets::NOTHING;
use comfy_table::{Cell, Color, Table, TableComponent};
use dwm_lut_profile::{DwmcoreVersion, ProfileSelectError, select_profile};

use self::access::{AccessCheckReport, build_access_check_reports};
use self::hooks::{HookCheckReport, build_hook_check_report};
use super::analysis::access::access_field_label;
use super::analysis::analyze;
use super::ensure;

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

    let analysis = analyze(&dll_path, &pdb_path)?;
    let version = DwmcoreVersion {
        build: u32::from(analysis.pe.file_version.build),
        revision: u32::from(analysis.pe.file_version.revision),
    };
    let selected = select_profile(version)?;
    let profile = (selected.profile)();

    let access_reports = build_access_check_reports(profile.accesses(), &analysis.accesses);
    let hook_report =
        build_hook_check_report(&profile, &analysis.pe, &analysis.pubs, &analysis.abis);

    println!();
    println!("version  10.0.{version}");
    println!("profile  >= 10.0.{}", selected.min_version);
    println!("dll      {}", dll_path.display());
    println!("pdb      {}", pdb_path.display());
    println!();

    print_access_table(&access_reports);
    println!();
    print_hooks_table(&hook_report);
    mismatch_result(
        access_reports
            .iter()
            .map(AccessCheckReport::failed_count)
            .sum(),
        hook_report.aob_failed_count(),
        hook_report.abi_failed_count(),
    )
}

fn mismatch_result(
    access_failed: usize,
    aob_failed: usize,
    abi_failed: usize,
) -> Result<(), CheckError> {
    let mut failures = Vec::new();

    if access_failed != 0 {
        failures.push(format!(
            "{access_failed} access field{} failed",
            if access_failed == 1 { "" } else { "s" }
        ));
    }
    if aob_failed != 0 {
        failures.push(format!(
            "{aob_failed} aob{} failed",
            if aob_failed == 1 { "" } else { "s" }
        ));
    }
    if abi_failed != 0 {
        failures.push(format!(
            "{abi_failed} ABI definition{} failed",
            if abi_failed == 1 { "" } else { "s" }
        ));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(CheckError::Mismatch(failures.join(", ")))
    }
}

fn print_access_table(reports: &[AccessCheckReport]) {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_style(TableComponent::HeaderLines, '─');
    table.set_header(vec![
        Cell::new("Access"),
        Cell::new("Field"),
        Cell::new("Status"),
    ]);

    for report in reports {
        for (index, row) in report.rows.iter().enumerate() {
            let status_color = if row.status.is_ok() {
                Color::Green
            } else {
                Color::Red
            };
            table.add_row(vec![
                Cell::new(if index == 0 {
                    report.target.label()
                } else {
                    ""
                }),
                Cell::new(access_field_label(row.field)),
                status_cell(row.status.as_str(), status_color),
            ]);
        }
    }

    println!("{table}");
}

fn print_hooks_table(report: &HookCheckReport) {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_style(TableComponent::HeaderLines, '─');
    table.set_header(vec![Cell::new("Hook"), Cell::new("Status")]);

    for row in &report.rows {
        let status_color = if row.is_ok() {
            Color::Green
        } else {
            Color::Red
        };

        table.add_row(vec![
            Cell::new(row.target.label()),
            status_cell(row.status_label(), status_color),
        ]);
    }

    println!("{table}");
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
    fn access_field_failures_are_mismatch() {
        let error = mismatch_result(2, 0, 0).expect_err("mismatch");
        assert!(matches!(
            error,
            CheckError::Mismatch(message) if message == "2 access fields failed"
        ));
    }

    #[test]
    fn aob_failures_are_mismatch() {
        let error = mismatch_result(0, 1, 0).expect_err("mismatch");
        assert!(matches!(
            error,
            CheckError::Mismatch(message) if message == "1 aob failed"
        ));
    }

    #[test]
    fn combined_failures_are_mismatch() {
        let error = mismatch_result(2, 3, 1).expect_err("mismatch");
        assert!(matches!(
            error,
            CheckError::Mismatch(message)
                if message
                    == "2 access fields failed, 3 aobs failed, 1 ABI definition failed"
        ));
    }
}
