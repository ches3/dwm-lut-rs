use comfy_table::presets::NOTHING;
use comfy_table::{Cell, Color, Table, TableComponent};
use dwm_lut_profile::{FieldOffsetTarget, HookTarget, VtableIndexTarget};

#[derive(Debug, Clone)]
pub struct InspectReport {
    pub file_version: String,
    pub dll: String,
    pub pdb: String,
    pub vtable_indices: Vec<ExtractionRow<VtableIndexTarget>>,
    pub field_offsets: Vec<ExtractionRow<FieldOffsetTarget>>,
    pub signatures: Vec<SignatureReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureStatus {
    Ok,
    NoSymbol,
    IcfStub,
    AmbiguousSymbol,
    UniquifyFailed,
}

impl SignatureStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoSymbol => "no_symbol",
            Self::IcfStub => "icf_stub",
            Self::AmbiguousSymbol => "ambiguous_symbol",
            Self::UniquifyFailed => "uniquify_failed",
        }
    }

    fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

#[derive(Debug, Clone)]
pub struct SignatureReport {
    pub target: String,
    pub hook_target: HookTarget,
    pub status: SignatureStatus,
    pub rva: Option<String>,
    pub aob: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionStatus {
    Ok,
    NoSymbol,
    NoVftable,
    NoDisp,
    Ambiguous,
}

impl ExtractionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoSymbol => "no_symbol",
            Self::NoVftable => "no_vftable",
            Self::NoDisp => "no_disp",
            Self::Ambiguous => "ambiguous",
        }
    }

    fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

#[derive(Debug, Clone)]
pub struct ExtractionRow<T> {
    pub target: T,
    pub status: ExtractionStatus,
    pub value: Option<usize>,
}

pub fn format_rva(rva: u32) -> String {
    format!("{rva:#x}")
}

const AOB_BYTES_PER_LINE: usize = 16;

pub fn print_report(report: &InspectReport) {
    println!();
    println!("version  {}", report.file_version);
    println!("dll      {}", report.dll);
    println!("pdb      {}", report.pdb);
    println!();

    let mut vtable_index_table = Table::new();
    vtable_index_table
        .load_preset(NOTHING)
        .set_style(TableComponent::HeaderLines, '─');
    vtable_index_table.set_header(vec![
        Cell::new("Vtable index"),
        Cell::new("Status"),
        Cell::new("Value"),
    ]);
    for row in &report.vtable_indices {
        let value = row
            .value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".into());
        vtable_index_table.add_row(vec![
            Cell::new(row.target.label()),
            extraction_status_cell(row.status),
            Cell::new(value),
        ]);
    }
    println!("{vtable_index_table}");
    println!();

    let mut field_offset_table = Table::new();
    field_offset_table
        .load_preset(NOTHING)
        .set_style(TableComponent::HeaderLines, '─');
    field_offset_table.set_header(vec![
        Cell::new("Field offset"),
        Cell::new("Status"),
        Cell::new("Value"),
    ]);
    for row in &report.field_offsets {
        let value = row
            .value
            .map(|value| format!("{value:#x}"))
            .unwrap_or_else(|| "-".into());
        field_offset_table.add_row(vec![
            Cell::new(row.target.label()),
            extraction_status_cell(row.status),
            Cell::new(value),
        ]);
    }
    println!("{field_offset_table}");
    println!();

    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_style(TableComponent::HeaderLines, '─');
    table.set_header(vec![
        Cell::new("Signature"),
        Cell::new("Status"),
        Cell::new("Required"),
        Cell::new("RVA"),
    ]);

    for signature in &report.signatures {
        let required = signature.hook_target.is_required_signature();
        let required_label = if required { "yes" } else { "no" };
        table.add_row(vec![
            Cell::new(&signature.target),
            status_cell(signature.status, required),
            Cell::new(required_label),
            Cell::new(signature.rva.as_deref().unwrap_or("-")),
        ]);
    }

    println!("{table}");

    let aob_signatures: Vec<&SignatureReport> = report
        .signatures
        .iter()
        .filter(|signature| signature.aob.is_some())
        .collect();
    if !aob_signatures.is_empty() {
        println!();
        println!("aob");
        push_section_rule_stdout();
        for signature in aob_signatures {
            let aob = signature.aob.as_deref().expect("filtered to Some");
            println!();
            println!("[{}]", signature.target);
            for line in wrap_aob(aob, AOB_BYTES_PER_LINE) {
                println!("  {line}");
            }
        }
    }
}

fn extraction_status_cell(status: ExtractionStatus) -> Cell {
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

fn status_cell(status: SignatureStatus, required: bool) -> Cell {
    let cell = Cell::new(status.as_str());
    if !stdout_supports_color() {
        return cell;
    }
    if status.is_ok() {
        cell.fg(Color::Green)
    } else if required {
        cell.fg(Color::Red)
    } else {
        cell.fg(Color::Yellow)
    }
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
