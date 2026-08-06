use iced_x86::{Decoder, DecoderOptions, Instruction, OpKind, Register};

use super::report::{LayoutReport, LayoutRow, LayoutStatus, format_hex};
use crate::profile::pdb_publics::PdbPublics;
use crate::profile::pe::PeImage;

pub fn extract_layout(pe: &PeImage, pubs: &PdbPublics) -> LayoutReport {
    let mut rows = Vec::with_capacity(8);
    rows.extend(extract_swap_chain_to_resource(pe, pubs));
    rows.push(extract_hardware_protected(pe, pubs));
    rows.extend(extract_monitor_identity(pe, pubs));
    rows.extend(extract_context_to_swap_chain(pe, pubs));
    LayoutReport { rows }
}

fn extract_swap_chain_to_resource(pe: &PeImage, pubs: &PdbPublics) -> [LayoutRow; 2] {
    let mut container = None;
    let mut resource = None;

    for symbol in &pubs.symbols {
        if !symbol.name.starts_with("??_7") {
            continue;
        }
        let name = &symbol.name;
        let Ok(slots) = read_vftable_slots(pe, pubs, symbol.rva, 48) else {
            continue;
        };

        let is_present_container_table = name.contains("6BIDeviceResource@@@")
            && (name.contains("SwapChain") || name.contains("IOverlaySwapChain"));
        if is_present_container_table
            && let Some(index) = slots.iter().find_map(|(index, method_names)| {
                method_names
                    .iter()
                    .any(|method| method.contains("GetPhysicalBackBuffer"))
                    .then_some(*index)
            })
        {
            let prefer = name.contains("CLegacySwapChain@@")
                || name.contains("CDDisplaySwapChain@@")
                || name.contains("COverlaySwapChain@@");
            if container.is_none() || prefer {
                container = Some(index);
            }
        }

        let is_buffer_table = name.contains("SwapChainBuffer") || name.contains("ISwapChainBuffer");
        if is_buffer_table
            && let Some(index) = slots.iter().find_map(|(index, method_names)| {
                method_names
                    .iter()
                    .any(|method| method.contains("GetD3D11Resource"))
                    .then_some(*index)
            })
        {
            let prefer = name.contains("CLegacySwapChainBuffer@@")
                || name.contains("CDDisplaySwapChainBuffer@@");
            if resource.is_none() || prefer {
                resource = Some(index);
            }
        }
    }

    [
        index_row("container_vtable_index", container),
        index_row("resource_vtable_index", resource),
    ]
}

fn index_row(target: &str, value: Option<usize>) -> LayoutRow {
    match value {
        Some(index) => LayoutRow {
            target: target.into(),
            status: LayoutStatus::Ok,
            value: Some(index.to_string()),
        },
        None => LayoutRow {
            target: target.into(),
            status: LayoutStatus::NoVftable,
            value: None,
        },
    }
}

fn read_vftable_slots(
    pe: &PeImage,
    pubs: &PdbPublics,
    vt_rva: u32,
    max_slots: usize,
) -> Result<Vec<(usize, Vec<String>)>, String> {
    let bytes = pe.bytes_at(vt_rva, max_slots * 8)?;
    let mut slots = Vec::with_capacity(max_slots);
    for index in 0..max_slots {
        let ptr = u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().expect("8 bytes"));
        if ptr == 0 {
            slots.push((index, Vec::new()));
            continue;
        }
        if ptr < pe.image_base || ptr >= pe.image_base + pe.image.len() as u64 {
            break;
        }
        let fn_rva = (ptr - pe.image_base) as u32;
        let names = pubs.rva_names.get(&fn_rva).cloned().unwrap_or_default();
        slots.push((index, names));
    }
    Ok(slots)
}

fn extract_hardware_protected(pe: &PeImage, pubs: &PdbPublics) -> LayoutRow {
    let Some(symbol) = pubs
        .find_by_prefix("?IsHardwareProtected@COverlaySwapChain@@")
        .into_iter()
        .next()
    else {
        return LayoutRow {
            target: "hardware_protected".into(),
            status: LayoutStatus::NoSymbol,
            value: None,
        };
    };

    match first_memory_disp(pe, symbol.rva, 0x80) {
        Some(disp) => LayoutRow {
            target: "hardware_protected".into(),
            status: LayoutStatus::Ok,
            value: Some(format_hex(disp)),
        },
        None => LayoutRow {
            target: "hardware_protected".into(),
            status: LayoutStatus::NoDisp,
            value: None,
        },
    }
}

fn extract_context_to_swap_chain(pe: &PeImage, pubs: &PdbPublics) -> [LayoutRow; 2] {
    [
        extract_monitor_target_offset(pe, pubs),
        extract_swap_chain_vtable_index(pe, pubs),
    ]
}

fn extract_monitor_target_offset(pe: &PeImage, pubs: &PdbPublics) -> LayoutRow {
    let Some(symbol) = pubs
        .find_by_prefix("??0COverlayContext@@")
        .into_iter()
        .next()
    else {
        return LayoutRow {
            target: "monitor_target_offset".into(),
            status: LayoutStatus::NoSymbol,
            value: None,
        };
    };

    match first_this_store_from_second_arg_disp(pe, symbol.rva, 0x200) {
        Some(disp) => LayoutRow {
            target: "monitor_target_offset".into(),
            status: LayoutStatus::Ok,
            value: Some(format_hex(disp)),
        },
        None => LayoutRow {
            target: "monitor_target_offset".into(),
            status: LayoutStatus::NoDisp,
            value: None,
        },
    }
}

fn extract_swap_chain_vtable_index(pe: &PeImage, pubs: &PdbPublics) -> LayoutRow {
    let mut index = None;

    for symbol in &pubs.symbols {
        if !symbol.name.starts_with("??_7") {
            continue;
        }
        if !symbol
            .name
            .contains("6BIPixelFormat@@IOverlayMonitorTarget@@@")
        {
            continue;
        }
        let Ok(slots) = read_vftable_slots(pe, pubs, symbol.rva, 64) else {
            continue;
        };

        let Some(found) = slots.iter().rev().find_map(|(slot, method_names)| {
            method_names
                .iter()
                .any(|method| method.contains("GetOverlaySwapChain"))
                .then_some(*slot)
        }) else {
            continue;
        };

        let prefer = symbol.name.starts_with("??_7CDDisplayRenderTarget@@")
            || symbol.name.starts_with("??_7CLegacyRenderTarget@@");
        if index.is_none() || prefer {
            index = Some(found);
        }
    }

    index_row("swap_chain_vtable_index", index)
}

fn extract_monitor_identity(pe: &PeImage, pubs: &PdbPublics) -> [LayoutRow; 3] {
    let luid_symbol = pubs
        .find_by_prefix("?GetDisplayAdapterLuid@COverlaySwapChain@@")
        .into_iter()
        .next();
    let target_symbol = pubs
        .find_by_prefix("?GetVidPnTargetId@COverlaySwapChain@@")
        .into_iter()
        .next();

    let (luid_status, low_value, high_value) = if let Some(symbol) = luid_symbol {
        let disps = memory_disps(pe, symbol.rva, 0x80);
        if let Some(low) = disps.into_iter().find(|disp| *disp <= 0x200) {
            (
                LayoutStatus::Ok,
                Some(format_hex(low)),
                Some(format_hex(low + 4)),
            )
        } else {
            (LayoutStatus::NoDisp, None, None)
        }
    } else {
        (LayoutStatus::NoSymbol, None, None)
    };

    let target_row = if let Some(symbol) = target_symbol {
        match first_memory_disp(pe, symbol.rva, 0x80) {
            Some(disp) => LayoutRow {
                target: "target_id_offset".into(),
                status: LayoutStatus::Ok,
                value: Some(format_hex(disp)),
            },
            None => LayoutRow {
                target: "target_id_offset".into(),
                status: LayoutStatus::NoDisp,
                value: None,
            },
        }
    } else {
        LayoutRow {
            target: "target_id_offset".into(),
            status: LayoutStatus::NoSymbol,
            value: None,
        }
    };

    [
        LayoutRow {
            target: "adapter_luid_low_offset".into(),
            status: luid_status,
            value: low_value,
        },
        LayoutRow {
            target: "adapter_luid_high_offset".into(),
            status: luid_status,
            value: high_value,
        },
        target_row,
    ]
}

fn first_memory_disp(pe: &PeImage, rva: u32, max_size: usize) -> Option<usize> {
    memory_disps(pe, rva, max_size).into_iter().next()
}

fn first_this_store_from_second_arg_disp(pe: &PeImage, rva: u32, max_size: usize) -> Option<usize> {
    let Ok(bytes) = pe.bytes_at(rva, max_size) else {
        return None;
    };
    let mut decoder = Decoder::with_ip(64, bytes, u64::from(rva), DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    let mut this_derived = [false; 16];
    let mut arg_derived = [false; 16];
    this_derived[gpr64_index(Register::RCX).expect("rcx")] = true;
    arg_derived[gpr64_index(Register::RDX).expect("rdx")] = true;

    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        if instruction.mnemonic() == iced_x86::Mnemonic::Ret {
            break;
        }
        if instruction.mnemonic() == iced_x86::Mnemonic::Mov
            && instruction.op_kind(0) == OpKind::Memory
            && instruction.memory_index() == Register::None
            && instruction.op_kind(1) == OpKind::Register
        {
            let base_ok =
                gpr64_index(instruction.memory_base()).is_some_and(|base| this_derived[base]);
            let src_ok =
                gpr64_index(instruction.op1_register()).is_some_and(|src| arg_derived[src]);
            if base_ok && src_ok {
                let disp = instruction.memory_displacement64() as i64;
                if (0..0x400).contains(&disp) {
                    return Some(disp as usize);
                }
            }
        }
        update_this_derived(&instruction, &mut this_derived);
        update_arg_derived(&instruction, &mut arg_derived);
    }
    None
}

fn update_arg_derived(instruction: &Instruction, arg_derived: &mut [bool; 16]) {
    use iced_x86::Mnemonic;

    match instruction.mnemonic() {
        Mnemonic::Mov => {
            if instruction.op_kind(0) != OpKind::Register {
                return;
            }
            let Some(dst) = gpr64_index(instruction.op0_register()) else {
                return;
            };
            if dst == 4 {
                return;
            }
            arg_derived[dst] = match instruction.op_kind(1) {
                OpKind::Register => gpr64_index(instruction.op1_register())
                    .map(|src| arg_derived[src])
                    .unwrap_or(false),
                _ => false,
            };
        }
        Mnemonic::Cmp | Mnemonic::Test | Mnemonic::Push => {}
        _ => {
            if instruction.op_count() == 0 || instruction.op_kind(0) != OpKind::Register {
                return;
            }
            let Some(dst) = gpr64_index(instruction.op0_register()) else {
                return;
            };
            if dst != 4 {
                arg_derived[dst] = false;
            }
        }
    }
}

fn memory_disps(pe: &PeImage, rva: u32, max_size: usize) -> Vec<usize> {
    let Ok(bytes) = pe.bytes_at(rva, max_size) else {
        return Vec::new();
    };
    let mut decoder = Decoder::with_ip(64, bytes, u64::from(rva), DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    let mut disps = Vec::new();
    let mut this_derived = [false; 16];
    this_derived[gpr64_index(Register::RCX).expect("rcx")] = true;

    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        if instruction.mnemonic() == iced_x86::Mnemonic::Ret {
            break;
        }
        for op in 0..instruction.op_count() {
            if instruction.op_kind(op) != OpKind::Memory {
                continue;
            }
            if instruction.memory_index() != Register::None {
                continue;
            }
            let Some(base_index) = gpr64_index(instruction.memory_base()) else {
                continue;
            };
            if !this_derived[base_index] {
                continue;
            }
            let disp = instruction.memory_displacement64() as i64;
            if (0..0x400).contains(&disp) {
                disps.push(disp as usize);
            }
        }
        update_this_derived(&instruction, &mut this_derived);
    }
    disps
}

fn gpr64_index(reg: Register) -> Option<usize> {
    match reg.full_register() {
        Register::RAX => Some(0),
        Register::RCX => Some(1),
        Register::RDX => Some(2),
        Register::RBX => Some(3),
        Register::RSP => Some(4),
        Register::RBP => Some(5),
        Register::RSI => Some(6),
        Register::RDI => Some(7),
        Register::R8 => Some(8),
        Register::R9 => Some(9),
        Register::R10 => Some(10),
        Register::R11 => Some(11),
        Register::R12 => Some(12),
        Register::R13 => Some(13),
        Register::R14 => Some(14),
        Register::R15 => Some(15),
        _ => None,
    }
}

fn update_this_derived(instruction: &Instruction, this_derived: &mut [bool; 16]) {
    use iced_x86::Mnemonic;

    match instruction.mnemonic() {
        Mnemonic::Mov => {
            if instruction.op_kind(0) != OpKind::Register {
                return;
            }
            let Some(dst) = gpr64_index(instruction.op0_register()) else {
                return;
            };
            if dst == 4 {
                return;
            }
            this_derived[dst] = match instruction.op_kind(1) {
                OpKind::Register => gpr64_index(instruction.op1_register())
                    .map(|src| this_derived[src])
                    .unwrap_or(false),
                _ => false,
            };
        }
        Mnemonic::Lea => {
            if instruction.op_kind(0) != OpKind::Register {
                return;
            }
            let Some(dst) = gpr64_index(instruction.op0_register()) else {
                return;
            };
            if dst == 4 {
                return;
            }
            this_derived[dst] = instruction.op_kind(1) == OpKind::Memory
                && instruction.memory_index() == Register::None
                && gpr64_index(instruction.memory_base())
                    .map(|base| this_derived[base])
                    .unwrap_or(false);
        }
        Mnemonic::Cmp | Mnemonic::Test | Mnemonic::Push => {}
        _ => {
            if instruction.op_count() == 0 || instruction.op_kind(0) != OpKind::Register {
                return;
            }
            let Some(dst) = gpr64_index(instruction.op0_register()) else {
                return;
            };
            if dst != 4 {
                this_derived[dst] = false;
            }
        }
    }
}
