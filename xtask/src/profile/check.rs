use std::error::Error;
use std::io;
use std::path::PathBuf;

use comfy_table::presets::NOTHING;
use comfy_table::{Cell, Color, Table, TableComponent};
use dwm_lut_hook::{
    HookProfile, HookResolveError, HookTarget, LoadedModule, MappedModuleImage, ProfileSelectError,
    file_version_from_path, resolve_signature, select_versioned_profile,
};

use super::ensure;
use super::extract::layout::extract_layout;
use super::extract::report::{LayoutRow, LayoutStatus, format_hex};
use super::extract::symbols::{
    SymbolResolveError, resolve_disable_independent_flip_global, resolve_function_symbol,
    resolve_overlay_test_mode_global,
};
use super::pdb_publics::PdbPublics;
use super::pe::PeImage;

pub(super) struct Args {
    pub system: bool,
    pub dll: Option<PathBuf>,
    pub pdb: Option<PathBuf>,
    pub version: Option<String>,
}

pub(super) fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let (dll_path, pdb_path) = ensure::resolve_inputs(
        args.system,
        args.version.as_ref(),
        args.dll.as_ref(),
        args.pdb.as_ref(),
    )?;

    let version = file_version_from_path(&dll_path).map_err(profile_select_io_error)?;
    let entry = select_versioned_profile(version).map_err(profile_select_io_error)?;
    let profile = (entry.profile)();

    let pe = PeImage::load(&dll_path)?;
    let pubs = PdbPublics::load(&pdb_path)?;
    pubs.verify_against_pe(&pe.codeview)?;
    let layout = extract_layout(&pe, &pubs);

    let image = MappedModuleImage::open(&dll_path).map_err(io::Error::other)?;
    let module = image.module();

    println!();
    println!("version  10.0.{version}");
    println!("profile  >= 10.0.{}", entry.min_version);
    println!("dll      {}", dll_path.display());
    println!("pdb      {}", pdb_path.display());
    println!();

    let layout_failed = print_layout_table(&profile, &layout.rows);
    let required_failed = print_signatures_table(&profile, module, image.as_slice(), &pe, &pubs)?;

    match (layout_failed, required_failed) {
        (0, 0) => Ok(()),
        (layouts, 0) => Err(io::Error::other(format!(
            "{layouts} layout value{} failed",
            if layouts == 1 { "" } else { "s" }
        ))
        .into()),
        (0, signatures) => Err(io::Error::other(format!(
            "{signatures} required signature{} failed",
            if signatures == 1 { "" } else { "s" }
        ))
        .into()),
        (layouts, signatures) => Err(io::Error::other(format!(
            "{layouts} layout value{} failed, {signatures} required signature{} failed",
            if layouts == 1 { "" } else { "s" },
            if signatures == 1 { "" } else { "s" }
        ))
        .into()),
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

fn print_signatures_table(
    profile: &HookProfile,
    module: LoadedModule,
    image: &[u8],
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<usize, Box<dyn Error>> {
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

    let mut required_failed = 0usize;

    for signature in profile.signatures {
        let label = signature.target.label();
        let required = signature.target.is_required_signature();
        let required_label = if required { "yes" } else { "no" };

        let (status, rva) = match resolve_signature(module, image, signature) {
            Ok(resolved) => {
                let rva = resolved
                    .address
                    .checked_sub(module.base_address)
                    .ok_or_else(|| io::Error::other("resolved address was below module base"))?;
                match expected_symbol_rva(signature.target, pe, pubs) {
                    Ok(symbol_rva) if symbol_rva as usize == rva => {
                        (status_cell("ok", Color::Green), format!("{rva:#x}"))
                    }
                    Ok(_) => {
                        if required {
                            required_failed += 1;
                        }
                        (
                            status_cell("symbol_mismatch", severity_color(required)),
                            format!("{rva:#x}"),
                        )
                    }
                    Err(error) => {
                        if required {
                            required_failed += 1;
                        }
                        let status = if error == SymbolResolveError::Ambiguous {
                            "ambiguous_symbol"
                        } else {
                            "no_symbol"
                        };
                        (
                            status_cell(status, severity_color(required)),
                            format!("{rva:#x}"),
                        )
                    }
                }
            }
            Err(HookResolveError::SignatureNotFound { .. }) => {
                if required {
                    required_failed += 1;
                }
                (
                    status_cell("not_found", severity_color(required)),
                    "-".into(),
                )
            }
            Err(HookResolveError::SignatureAmbiguous { matches, .. }) => {
                if required {
                    required_failed += 1;
                }
                (
                    status_cell(&format!("ambiguous({matches})"), severity_color(required)),
                    "-".into(),
                )
            }
            Err(error) => return Err(io::Error::other(error).into()),
        };

        table.add_row(vec![
            Cell::new(label),
            status,
            Cell::new(required_label),
            Cell::new(rva),
        ]);
    }

    println!("{table}");
    Ok(required_failed)
}

fn expected_symbol_rva(
    target: HookTarget,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<u32, SymbolResolveError> {
    match target {
        HookTarget::OverlayTestMode => {
            resolve_overlay_test_mode_global(pubs).map(|symbol| symbol.rva)
        }
        HookTarget::DisableIndependentFlip => {
            resolve_disable_independent_flip_global(pubs).map(|symbol| symbol.rva)
        }
        _ => resolve_function_symbol(target, pubs, pe).map(|symbol| symbol.rva),
    }
}

fn severity_color(required: bool) -> Color {
    if required { Color::Red } else { Color::Yellow }
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

fn profile_select_io_error(error: ProfileSelectError) -> io::Error {
    io::Error::other(error.to_string())
}
