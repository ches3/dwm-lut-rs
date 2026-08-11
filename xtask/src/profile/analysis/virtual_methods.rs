use dwm_lut_profile::{VirtualMethodExtractSpec, VirtualMethodTarget};

use super::abi::{
    abi_matches_virtual_method, apply_return_passing_from_prologue,
    demangled_virtual_return_matches, parse_decorated_virtual_method_abi,
};
use super::model::{ExtractionStatus, VirtualMethodAbiStatus, VirtualMethodExtractionRow};
use super::symbols::{unqualified_method_symbol_prefix, vftable_symbol};
use super::virtual_method_return_type_label;
use crate::profile::pdb_publics::PdbPublics;
use crate::profile::pe::PeImage;

const VTABLE_MAX_SLOTS: usize = 64;

pub fn extract_virtual_methods(pe: &PeImage, pubs: &PdbPublics) -> Vec<VirtualMethodExtractionRow> {
    VirtualMethodTarget::ALL
        .iter()
        .copied()
        .map(|target| extract_target(target, pe, pubs))
        .collect()
}

fn extract_target(
    target: VirtualMethodTarget,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> VirtualMethodExtractionRow {
    match interpret(target.extract_spec(), pe, pubs) {
        Ok(extracted) => VirtualMethodExtractionRow {
            target,
            status: ExtractionStatus::Ok,
            value: Some(extracted.index),
            abi_status: extracted.abi.status,
        },
        Err(status) => VirtualMethodExtractionRow {
            target,
            status,
            value: None,
            abi_status: VirtualMethodAbiStatus::Unavailable,
        },
    }
}

struct InterpretedVirtualMethod {
    index: usize,
    abi: InterpretedAbi,
}

struct InterpretedAbi {
    status: VirtualMethodAbiStatus,
}

fn interpret(
    spec: VirtualMethodExtractSpec,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<InterpretedVirtualMethod, ExtractionStatus> {
    let method_prefixes: Vec<String> = spec
        .method_names
        .iter()
        .map(|method_name| unqualified_method_symbol_prefix(method_name))
        .collect();
    let mut chosen = None::<usize>;
    let mut chosen_abi = None::<InterpretedAbi>;

    for vtable in spec.vtables {
        let vftable_symbol = vftable_symbol(vtable.class_name, vtable.view);
        for symbol in &pubs.symbols {
            if symbol.name != vftable_symbol {
                continue;
            }
            let Ok(slots) = read_vftable_slots(pe, pubs, symbol.rva) else {
                continue;
            };
            let matches: Vec<(usize, u32)> = slots
                .iter()
                .filter(|(_, _, names)| {
                    names.iter().any(|name| {
                        method_prefixes
                            .iter()
                            .any(|prefix| name.starts_with(prefix.as_str()))
                    })
                })
                .map(|(index, rva, _)| (*index, *rva))
                .collect();
            if matches.is_empty() {
                continue;
            }
            let Some(index) = select_slot_index(&matches) else {
                return Err(ExtractionStatus::Ambiguous);
            };
            let function_rva = matches[0].1;
            let method_names = slots
                .iter()
                .filter(|(_, rva, _)| *rva == function_rva)
                .flat_map(|(_, _, names)| names)
                .filter(|name| {
                    method_prefixes
                        .iter()
                        .any(|prefix| name.starts_with(prefix.as_str()))
                })
                .map(String::as_str)
                .collect::<Vec<_>>();
            let extracted_abi = interpret_abi(spec, &method_names, pe, function_rva);
            chosen_abi = Some(match chosen_abi {
                Some(existing) => prefer_abi(existing, extracted_abi),
                None => extracted_abi,
            });
            match chosen {
                Some(existing) if existing != index => return Err(ExtractionStatus::Ambiguous),
                Some(_) => {}
                None => chosen = Some(index),
            }
        }
    }

    Ok(InterpretedVirtualMethod {
        index: chosen.ok_or(ExtractionStatus::NoVftable)?,
        abi: chosen_abi.expect("an extracted vtable index has matching method names"),
    })
}

fn interpret_abi(
    spec: VirtualMethodExtractSpec,
    names: &[&str],
    pe: &PeImage,
    function_rva: u32,
) -> InterpretedAbi {
    let mut found_mismatch = false;
    let prologue = pe.bytes_at(function_rva, 16).ok();

    for name in names {
        if let Ok(mut abi) = parse_decorated_virtual_method_abi(spec, name) {
            let prologue_result = match &prologue {
                Some(bytes) => apply_return_passing_from_prologue(spec, &mut abi, bytes),
                None if matches!(
                    spec.return_type,
                    dwm_lut_profile::VirtualMethodReturnType::SretValue(_)
                ) =>
                {
                    Err(format!(
                        "unable to read prologue at {function_rva:#x} for {}",
                        virtual_method_return_type_label(spec.return_type)
                    ))
                }
                None => Ok(()),
            };
            match prologue_result {
                Ok(()) if abi_matches_virtual_method(spec, &abi) => {
                    return InterpretedAbi {
                        status: VirtualMethodAbiStatus::Ok,
                    };
                }
                Ok(()) => {
                    found_mismatch = true;
                }
                Err(_) => {
                    if demangled_virtual_return_matches(spec, &abi) {
                        return InterpretedAbi {
                            status: VirtualMethodAbiStatus::Mismatch,
                        };
                    }
                }
            }
        }
    }

    if found_mismatch {
        InterpretedAbi {
            status: VirtualMethodAbiStatus::Mismatch,
        }
    } else {
        InterpretedAbi {
            status: VirtualMethodAbiStatus::Unsupported,
        }
    }
}

fn prefer_abi(existing: InterpretedAbi, extracted: InterpretedAbi) -> InterpretedAbi {
    match (existing.status, extracted.status) {
        (VirtualMethodAbiStatus::Ok, _) => existing,
        (_, VirtualMethodAbiStatus::Ok) => extracted,
        _ => existing,
    }
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
    use super::{InterpretedAbi, prefer_abi, select_slot_index};
    use crate::profile::analysis::model::VirtualMethodAbiStatus;

    fn abi(status: VirtualMethodAbiStatus) -> InterpretedAbi {
        InterpretedAbi { status }
    }

    #[test]
    fn vtable_slot_selection_uses_highest_index_for_alias_slots() {
        assert_eq!(select_slot_index(&[(3, 0x1000), (7, 0x1000)]), Some(7));
    }

    #[test]
    fn vtable_slot_selection_rejects_distinct_function_rvas() {
        assert_eq!(select_slot_index(&[(3, 0x1000), (7, 0x2000)]), None);
    }

    #[test]
    fn prefer_abi_keeps_existing_ok_over_later_non_ok() {
        let chosen = prefer_abi(
            abi(VirtualMethodAbiStatus::Ok),
            abi(VirtualMethodAbiStatus::Mismatch),
        );
        assert_eq!(chosen.status, VirtualMethodAbiStatus::Ok);
    }

    #[test]
    fn prefer_abi_upgrades_non_ok_to_later_ok() {
        let chosen = prefer_abi(
            abi(VirtualMethodAbiStatus::Unsupported),
            abi(VirtualMethodAbiStatus::Ok),
        );
        assert_eq!(chosen.status, VirtualMethodAbiStatus::Ok);
    }

    #[test]
    fn prefer_abi_keeps_first_non_ok_when_both_fail() {
        let chosen = prefer_abi(
            abi(VirtualMethodAbiStatus::Mismatch),
            abi(VirtualMethodAbiStatus::Unsupported),
        );
        assert_eq!(chosen.status, VirtualMethodAbiStatus::Mismatch);
    }
}
