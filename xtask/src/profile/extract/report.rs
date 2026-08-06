use comfy_table::presets::NOTHING;
use comfy_table::{Cell, Color, Table, TableComponent};
use dwm_lut_profile::HookTarget;

#[derive(Debug, Clone)]
pub struct InspectReport {
    pub file_version: String,
    pub dll: String,
    pub pdb: String,
    pub layout: LayoutReport,
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
pub enum LayoutStatus {
    Ok,
    NoSymbol,
    NoVftable,
    NoDisp,
}

impl LayoutStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoSymbol => "no_symbol",
            Self::NoVftable => "no_vftable",
            Self::NoDisp => "no_disp",
        }
    }

    fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

#[derive(Debug, Clone)]
pub struct LayoutRow {
    pub target: String,
    pub status: LayoutStatus,
    pub value: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LayoutReport {
    pub rows: Vec<LayoutRow>,
}

pub fn format_rva(rva: u32) -> String {
    format!("{rva:#x}")
}

pub fn format_hex(value: usize) -> String {
    format!("{value:#x}")
}

const AOB_BYTES_PER_LINE: usize = 16;

pub fn print_report(report: &InspectReport) {
    println!();
    println!("version  {}", report.file_version);
    println!("dll      {}", report.dll);
    println!("pdb      {}", report.pdb);
    println!();

    let mut layout_table = Table::new();
    layout_table
        .load_preset(NOTHING)
        .set_style(TableComponent::HeaderLines, '─');
    layout_table.set_header(vec![
        Cell::new("Layout"),
        Cell::new("Status"),
        Cell::new("Value"),
    ]);
    for row in &report.layout.rows {
        layout_table.add_row(vec![
            Cell::new(&row.target),
            layout_status_cell(row.status),
            Cell::new(row.value.as_deref().unwrap_or("-")),
        ]);
    }
    println!("{layout_table}");
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

fn layout_status_cell(status: LayoutStatus) -> Cell {
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
