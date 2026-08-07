use dwm_lut_profile::{VtableIndexExtractSpec, VtableIndexTarget};

use super::report::{ExtractionRow, ExtractionStatus};
use crate::profile::pdb_publics::PdbPublics;
use crate::profile::pe::PeImage;

const VTABLE_MAX_SLOTS: usize = 64;

pub fn extract_vtable_indices(
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Vec<ExtractionRow<VtableIndexTarget>> {
    VtableIndexTarget::ALL
        .iter()
        .copied()
        .map(|target| extract_target(target, pe, pubs))
        .collect()
}

fn extract_target(
    target: VtableIndexTarget,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> ExtractionRow<VtableIndexTarget> {
    match interpret(target.extract_spec(), pe, pubs) {
        Ok(value) => ExtractionRow {
            target,
            status: ExtractionStatus::Ok,
            value: Some(value),
        },
        Err(status) => ExtractionRow {
            target,
            status,
            value: None,
        },
    }
}

fn interpret(
    spec: VtableIndexExtractSpec,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<usize, ExtractionStatus> {
    let mut chosen = None::<usize>;

    for vftable_symbol in spec.vftable_symbols {
        for symbol in &pubs.symbols {
            if symbol.name != *vftable_symbol {
                continue;
            }
            let Ok(slots) = read_vftable_slots(pe, pubs, symbol.rva) else {
                continue;
            };
            let matches: Vec<(usize, u32)> = slots
                .iter()
                .filter(|(_, _, names)| {
                    names
                        .iter()
                        .any(|name| name.starts_with(spec.method_symbol_prefix))
                })
                .map(|(index, rva, _)| (*index, *rva))
                .collect();
            if matches.is_empty() {
                continue;
            }
            let Some(index) = select_slot_index(&matches) else {
                return Err(ExtractionStatus::Ambiguous);
            };
            match chosen {
                Some(existing) if existing != index => return Err(ExtractionStatus::Ambiguous),
                Some(_) => {}
                None => chosen = Some(index),
            }
        }
    }

    chosen.ok_or(ExtractionStatus::NoVftable)
}

fn select_slot_index(matches: &[(usize, u32)]) -> Option<usize> {
    let first_rva = matches.first()?.1;
    if matches.iter().any(|(_, rva)| *rva != first_rva) {
        return None;
    }
    matches.iter().map(|(index, _)| *index).max()
}

fn read_vftable_slots(
    pe: &PeImage,
    pubs: &PdbPublics,
    vt_rva: u32,
) -> Result<Vec<(usize, u32, Vec<String>)>, String> {
    let slot_count = pubs
        .symbols
        .iter()
        .filter(|symbol| symbol.name.starts_with("??_7") && symbol.rva > vt_rva)
        .filter_map(|symbol| {
            let byte_len = (symbol.rva - vt_rva) as usize;
            byte_len.is_multiple_of(8).then_some(byte_len / 8)
        })
        .min()
        .unwrap_or(VTABLE_MAX_SLOTS)
        .min(VTABLE_MAX_SLOTS);
    let bytes = pe.bytes_at(vt_rva, slot_count * 8)?;
    let mut slots = Vec::with_capacity(slot_count);
    for index in 0..slot_count {
        let ptr = u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().expect("8 bytes"));
        if ptr == 0 {
            slots.push((index, 0, Vec::new()));
            continue;
        }
        if ptr < pe.image_base || ptr >= pe.image_base + pe.image.len() as u64 {
            break;
        }
        let fn_rva = (ptr - pe.image_base) as u32;
        let names = pubs.rva_names.get(&fn_rva).cloned().unwrap_or_default();
        slots.push((index, fn_rva, names));
    }
    Ok(slots)
}

#[cfg(test)]
mod tests {
    use super::select_slot_index;

    #[test]
    fn vtable_slot_selection_uses_highest_index_for_alias_slots() {
        assert_eq!(select_slot_index(&[(3, 0x1000), (7, 0x1000)]), Some(7));
    }

    #[test]
    fn vtable_slot_selection_rejects_distinct_function_rvas() {
        assert_eq!(select_slot_index(&[(3, 0x1000), (7, 0x2000)]), None);
    }
}
