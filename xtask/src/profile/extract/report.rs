use comfy_table::presets::NOTHING;
use comfy_table::{Cell, Color, Table, TableComponent};
use dwm_lut_profile::HookTarget;

use super::super::analysis::abi::{
    AbiExtractionRow, AbiExtractionStatus, ExtractedFunctionAbi, classify_hook_argument,
};
use super::super::analysis::access::{AccessReport, AccessStatus, access_field_label};
use super::super::analysis::model::{AobReport, AobStatus};

#[derive(Debug, Clone)]
pub struct InspectReport {
    pub file_version: String,
    pub dll: String,
    pub pdb: String,
    pub accesses: Vec<AccessReport>,
    pub aobs: Vec<AobReport>,
    pub abis: Vec<AbiExtractionRow>,
}

const AOB_BYTES_PER_LINE: usize = 16;

pub fn print_report(report: &InspectReport) {
    println!();
    println!("version  {}", report.file_version);
    println!("dll      {}", report.dll);
    println!("pdb      {}", report.pdb);
    println!();

    print_access_table(&report.accesses);
    println!();
    print_hooks(report);
}

fn print_hooks(report: &InspectReport) {
    println!("hooks");
    push_section_rule_stdout();

    for row in &report.aobs {
        let abi_row = report.abis.iter().find(|abi| abi.target == row.hook_target);
        let abi_status = abi_row.map(|abi| abi.status);
        let arguments = abi_row.map_or_else(
            || "-".to_string(),
            |abi_row| {
                abi_row.abi.as_ref().map_or_else(
                    || abi_row.detail.clone().unwrap_or_else(|| "-".into()),
                    |abi| format_hook_arguments(row.hook_target, abi),
                )
            },
        );
        let status = format_hook_status(row.status, abi_status, row.hook_target.is_required_hook());

        println!();
        println!("[{}]", row.target);
        println!("  status  {status}");
        println!("  arguments");
        println!("    {arguments}");
        if let Some(aob) = row.aob.as_deref() {
            println!("  aob");
            for line in wrap_aob(aob, AOB_BYTES_PER_LINE) {
                println!("    {line}");
            }
        }
    }
}

fn format_hook_arguments(target: HookTarget, abi: &ExtractedFunctionAbi) -> String {
    abi.arguments
        .iter()
        .map(|argument| match classify_hook_argument(target, argument) {
            Some(role) => role.extract_label(),
            None => argument.display(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_access_table(accesses: &[AccessReport]) {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_style(TableComponent::HeaderLines, '─');
    table.set_header(vec![
        Cell::new("Access"),
        Cell::new("Field"),
        Cell::new("Status"),
        Cell::new("Value"),
    ]);

    for access in accesses {
        for (index, row) in access.result.rows().iter().enumerate() {
            table.add_row(vec![
                Cell::new(if index == 0 {
                    access.target.label()
                } else {
                    ""
                }),
                Cell::new(access_field_label(row.field)),
                access_status_cell(row.status),
                Cell::new(row.value.as_deref().unwrap_or("-")),
            ]);
        }
    }

    println!("{table}");
}

fn access_status_cell(status: AccessStatus) -> Cell {
    let cell = Cell::new(status.as_str());
    if !stdout_supports_color() {
        return cell;
    }
    if status.is_ok() {
        cell.fg(Color::Green)
    } else {
        cell.fg(Color::Red)
    }
}

fn hook_status_label(aob: AobStatus, abi: Option<AbiExtractionStatus>) -> &'static str {
    if !aob.is_ok() {
        return aob.as_str();
    }
    abi.map_or("missing", AbiExtractionStatus::as_str)
}

fn format_hook_status(aob: AobStatus, abi: Option<AbiExtractionStatus>, required: bool) -> String {
    let label = hook_status_label(aob, abi);
    if !stdout_supports_color() {
        return label.to_string();
    }
    let ok = aob.is_ok() && abi == Some(AbiExtractionStatus::Ok);
    let code = if ok {
        32
    } else if required {
        31
    } else {
        33
    };
    format!("\x1b[{code}m{label}\x1b[0m")
}

fn stdout_supports_color() -> bool {
    supports_color::on_cached(supports_color::Stream::Stdout).is_some()
}

fn push_section_rule_stdout() {
    println!("{}", "─".repeat(60));
}

fn wrap_aob(aob: &str, bytes_per_line: usize) -> Vec<String> {
    let tokens: Vec<_> = aob.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    tokens
        .chunks(bytes_per_line)
        .map(|chunk| chunk.join(" "))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::wrap_aob;

    #[test]
    fn wraps_aob_into_fixed_width_lines() {
        let aob = "40 55 53 56 57 41 54 41 55 41 56 41 57 48 8D 6C 24 ?? 48 81";
        let lines = wrap_aob(aob, 8);
        assert_eq!(
            lines,
            vec![
                "40 55 53 56 57 41 54 41".to_string(),
                "55 41 56 41 57 48 8D 6C".to_string(),
                "24 ?? 48 81".to_string(),
            ]
        );
    }
}
