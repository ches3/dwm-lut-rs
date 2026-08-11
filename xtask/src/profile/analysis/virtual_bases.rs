use dwm_lut_profile::{
    DeviceResourceVirtualBaseAccess, VirtualBaseClassExtractSpec, VirtualBaseTarget,
};
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};

use super::model::{ExtractionStatus, VirtualBaseExtractionRow};
use super::symbols::{ctor_symbol_prefix, vbtable_class_prefix, vbtable_symbol, vftable_symbol};
use crate::profile::pdb_publics::PdbPublics;
use crate::profile::pe::PeImage;

const CTOR_MAX_SCAN: usize = 0x800;
const VBTABLE_MAX_ENTRIES: usize = 16;
const GPR_COUNT: usize = 16;
const VOLATILE_GPRS: &[usize] = &[0, 1, 2, 8, 9, 10, 11];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Unknown,
    This(i64),
    Vbtable {
        vbptr_offset: i32,
    },
    VbtableDisplacement(DeviceResourceVirtualBaseAccess),
    VirtualBase {
        access: DeviceResourceVirtualBaseAccess,
        fixed_offset: i64,
    },
    TargetPointer,
}

pub fn extract_virtual_bases(pe: &PeImage, pubs: &PdbPublics) -> Vec<VirtualBaseExtractionRow> {
    VirtualBaseTarget::ALL
        .iter()
        .copied()
        .map(|target| extract_target(target, pe, pubs))
        .collect()
}

fn extract_target(
    target: VirtualBaseTarget,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> VirtualBaseExtractionRow {
    match interpret(target, pe, pubs) {
        Ok(value) => VirtualBaseExtractionRow {
            target,
            status: ExtractionStatus::Ok,
            value: Some(value),
        },
        Err(status) => VirtualBaseExtractionRow {
            target,
            status,
            value: None,
        },
    }
}

fn interpret(
    target: VirtualBaseTarget,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<DeviceResourceVirtualBaseAccess, ExtractionStatus> {
    let mut class_values = Vec::new();
    for class in target.extract_spec().classes {
        class_values.push(extract_class(*class, pe, pubs)?);
    }
    select_access(class_values.into_iter().map(Ok))
}

fn extract_class(
    spec: VirtualBaseClassExtractSpec,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<DeviceResourceVirtualBaseAccess, ExtractionStatus> {
    let virtual_base_vftable_symbol = vftable_symbol(spec.class_name, spec.target_virtual_base);
    let ctor_prefix = ctor_symbol_prefix(spec.class_name);
    let vftable_rva = unique_rva(
        pubs.symbols
            .iter()
            .filter(|symbol| symbol.name == virtual_base_vftable_symbol)
            .map(|symbol| symbol.rva),
    )?;
    let ctor_rvas = distinct_rvas(
        pubs.symbols
            .iter()
            .filter(|symbol| symbol.name.starts_with(ctor_prefix.as_str()))
            .map(|symbol| symbol.rva),
    );
    if ctor_rvas.is_empty() {
        return Err(ExtractionStatus::NoSymbol);
    }

    let install_access = select_access(ctor_rvas.iter().copied().map(|rva| {
        let bytes = pe
            .bytes_at(rva, CTOR_MAX_SCAN)
            .map_err(|_| ExtractionStatus::NoDisp)?;
        extract_access_from_ctor(bytes, u64::from(rva), u64::from(vftable_rva))
    }))?;
    let virtual_base_offset =
        resolve_virtual_base_offset(spec, install_access, &ctor_rvas, pe, pubs)?;
    resolve_overlay_access(spec, virtual_base_offset, &ctor_rvas, pe, pubs)
}

fn select_access(
    results: impl IntoIterator<Item = Result<DeviceResourceVirtualBaseAccess, ExtractionStatus>>,
) -> Result<DeviceResourceVirtualBaseAccess, ExtractionStatus> {
    let mut chosen = None;
    for result in results {
        let access = match result {
            Ok(access) => access,
            Err(ExtractionStatus::NoDisp) => continue,
            Err(status) => return Err(status),
        };
        match chosen {
            Some(existing) if existing != access => return Err(ExtractionStatus::Ambiguous),
            Some(_) => {}
            None => chosen = Some(access),
        }
    }
    chosen.ok_or(ExtractionStatus::NoDisp)
}

fn extract_access_from_ctor(
    bytes: &[u8],
    ip: u64,
    target_vftable_rva: u64,
) -> Result<DeviceResourceVirtualBaseAccess, ExtractionStatus> {
    let mut decoder = Decoder::with_ip(64, bytes, ip, DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    let mut origins = [Origin::Unknown; GPR_COUNT];
    origins[gpr64_index(Register::RCX).expect("rcx")] = Origin::This(0);
    let mut chosen = None;

    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }

        if let Some(access) = virtual_base_vftable_store(&instruction, &origins) {
            match chosen {
                Some(existing) if existing != access => return Err(ExtractionStatus::Ambiguous),
                Some(_) => {}
                None => chosen = Some(access),
            }
        }

        update_origins(&instruction, &mut origins, target_vftable_rva);
        if instruction.mnemonic() == Mnemonic::Ret {
            break;
        }
    }

    chosen.ok_or(ExtractionStatus::NoDisp)
}

fn virtual_base_vftable_store(
    instruction: &Instruction,
    origins: &[Origin; GPR_COUNT],
) -> Option<DeviceResourceVirtualBaseAccess> {
    if instruction.mnemonic() != Mnemonic::Mov
        || instruction.op_kind(0) != OpKind::Memory
        || instruction.op_kind(1) != OpKind::Register
    {
        return None;
    }
    let src = gpr64_index(instruction.op1_register())?;
    if origins[src] != Origin::TargetPointer {
        return None;
    }
    let Origin::VirtualBase {
        access,
        fixed_offset,
    } = memory_origin(instruction, origins)?
    else {
        return None;
    };
    (fixed_offset == i64::from(access.vbptr_offset)).then_some(access)
}

fn update_origins(
    instruction: &Instruction,
    origins: &mut [Origin; GPR_COUNT],
    target_vftable_rva: u64,
) {
    match instruction.mnemonic() {
        Mnemonic::Mov => {
            let Some(dst) = instruction_destination_gpr(instruction) else {
                return;
            };
            origins[dst] = match instruction.op_kind(1) {
                OpKind::Register => gpr64_index(instruction.op1_register())
                    .map(|src| origins[src])
                    .unwrap_or(Origin::Unknown),
                OpKind::Memory => memory_origin(instruction, origins)
                    .and_then(|origin| match origin {
                        Origin::This(offset) => i32::try_from(offset)
                            .ok()
                            .map(|vbptr_offset| Origin::Vbtable { vbptr_offset }),
                        _ => None,
                    })
                    .unwrap_or(Origin::Unknown),
                _ => Origin::Unknown,
            };
        }
        Mnemonic::Movsxd => {
            let Some(dst) = instruction_destination_gpr(instruction) else {
                return;
            };
            origins[dst] =
                vbtable_displacement_origin(instruction, origins).unwrap_or(Origin::Unknown);
        }
        Mnemonic::Lea => {
            let Some(dst) = instruction_destination_gpr(instruction) else {
                return;
            };
            origins[dst] = if instruction.is_ip_rel_memory_operand()
                && instruction.ip_rel_memory_address() == target_vftable_rva
            {
                Origin::TargetPointer
            } else {
                memory_origin(instruction, origins).unwrap_or(Origin::Unknown)
            };
        }
        Mnemonic::Call => {
            for &index in VOLATILE_GPRS {
                origins[index] = Origin::Unknown;
            }
        }
        Mnemonic::Cmp | Mnemonic::Test | Mnemonic::Push | Mnemonic::Nop => {}
        _ => {
            if let Some(dst) = instruction_destination_gpr(instruction) {
                origins[dst] = Origin::Unknown;
            }
        }
    }
}

fn resolve_virtual_base_offset(
    spec: VirtualBaseClassExtractSpec,
    install_access: DeviceResourceVirtualBaseAccess,
    ctor_rvas: &[u32],
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<i64, ExtractionStatus> {
    let class_vbtable_prefix = vbtable_class_prefix(spec.class_name);
    let mut offsets = Vec::new();
    for symbol in pubs
        .symbols
        .iter()
        .filter(|symbol| symbol.name.starts_with(class_vbtable_prefix.as_str()))
    {
        let Ok(store_offset) = pointer_store_offset(symbol.rva, ctor_rvas, pe) else {
            continue;
        };
        if store_offset != install_access.vbptr_offset {
            continue;
        }
        let displacement = read_vbtable_entry(pe, symbol.rva, install_access.vbtable_entry_offset)?;
        let absolute = i64::from(store_offset)
            .checked_add(i64::from(displacement))
            .ok_or(ExtractionStatus::NoDisp)?;
        if !offsets.contains(&absolute) {
            offsets.push(absolute);
        }
    }
    match offsets.as_slice() {
        [] => Err(ExtractionStatus::NoDisp),
        [offset] => Ok(*offset),
        _ => Err(ExtractionStatus::Ambiguous),
    }
}

fn resolve_overlay_access(
    spec: VirtualBaseClassExtractSpec,
    virtual_base_offset: i64,
    ctor_rvas: &[u32],
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<DeviceResourceVirtualBaseAccess, ExtractionStatus> {
    let overlay_this_offset = resolve_overlay_this_offset(spec, ctor_rvas, pe, pubs)?;
    for view in spec.source_vbtable_views {
        let symbol_name = vbtable_symbol(spec.class_name, *view);
        let mut accesses = Vec::new();
        for symbol in pubs
            .symbols
            .iter()
            .filter(|symbol| symbol.name == symbol_name)
        {
            let Ok(vbptr_offset) = pointer_store_offset(symbol.rva, ctor_rvas, pe) else {
                continue;
            };
            let mut matching_entry_offsets = Vec::new();
            for index in 1..VBTABLE_MAX_ENTRIES {
                let entry_offset = index * size_of::<i32>();
                let Ok(displacement) = read_vbtable_entry(pe, symbol.rva, entry_offset) else {
                    break;
                };
                let Some(candidate_offset) =
                    i64::from(vbptr_offset).checked_add(i64::from(displacement))
                else {
                    continue;
                };
                if candidate_offset != virtual_base_offset {
                    continue;
                }
                matching_entry_offsets.push(entry_offset);
            }
            if let Some(vbtable_entry_offset) = matching_entry_offsets.into_iter().max() {
                let access = DeviceResourceVirtualBaseAccess {
                    vbptr_offset: vbptr_offset
                        .checked_sub(overlay_this_offset)
                        .ok_or(ExtractionStatus::NoDisp)?,
                    vbtable_entry_offset,
                };
                if !accesses.contains(&access) {
                    accesses.push(access);
                }
            }
        }
        match accesses.as_slice() {
            [] => {}
            [access] => return Ok(*access),
            _ => return Err(ExtractionStatus::Ambiguous),
        }
    }
    Err(ExtractionStatus::NoDisp)
}

fn resolve_overlay_this_offset(
    spec: VirtualBaseClassExtractSpec,
    ctor_rvas: &[u32],
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<i32, ExtractionStatus> {
    for view in spec.source_vftable_views {
        let symbol_name = vftable_symbol(spec.class_name, *view);
        let mut offsets = Vec::new();
        for symbol in pubs
            .symbols
            .iter()
            .filter(|symbol| symbol.name == symbol_name)
        {
            let offset = match pointer_store_offset(symbol.rva, ctor_rvas, pe) {
                Ok(offset) => i64::from(offset),
                Err(ExtractionStatus::NoDisp) => {
                    let install_access = select_access(ctor_rvas.iter().copied().map(|rva| {
                        let bytes = pe
                            .bytes_at(rva, CTOR_MAX_SCAN)
                            .map_err(|_| ExtractionStatus::NoDisp)?;
                        extract_access_from_ctor(bytes, u64::from(rva), u64::from(symbol.rva))
                    }))?;
                    resolve_virtual_base_offset(spec, install_access, ctor_rvas, pe, pubs)?
                }
                Err(status) => return Err(status),
            };
            let offset = i32::try_from(offset).map_err(|_| ExtractionStatus::NoDisp)?;
            if !offsets.contains(&offset) {
                offsets.push(offset);
            }
        }
        match offsets.as_slice() {
            [] => {}
            [offset] => return Ok(*offset),
            _ => return Err(ExtractionStatus::Ambiguous),
        }
    }
    Err(ExtractionStatus::NoDisp)
}

fn pointer_store_offset(
    target_rva: u32,
    ctor_rvas: &[u32],
    pe: &PeImage,
) -> Result<i32, ExtractionStatus> {
    let mut chosen = None;
    for &ctor_rva in ctor_rvas {
        let bytes = pe
            .bytes_at(ctor_rva, CTOR_MAX_SCAN)
            .map_err(|_| ExtractionStatus::NoDisp)?;
        let offset =
            match extract_pointer_store_offset(bytes, u64::from(ctor_rva), u64::from(target_rva)) {
                Ok(offset) => offset,
                Err(ExtractionStatus::NoDisp) => continue,
                Err(status) => return Err(status),
            };
        match chosen {
            Some(existing) if existing != offset => return Err(ExtractionStatus::Ambiguous),
            Some(_) => {}
            None => chosen = Some(offset),
        }
    }
    chosen.ok_or(ExtractionStatus::NoDisp)
}

fn extract_pointer_store_offset(
    bytes: &[u8],
    ip: u64,
    target_rva: u64,
) -> Result<i32, ExtractionStatus> {
    let mut decoder = Decoder::with_ip(64, bytes, ip, DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    let mut origins = [Origin::Unknown; GPR_COUNT];
    origins[gpr64_index(Register::RCX).expect("rcx")] = Origin::This(0);
    let mut chosen = None;

    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        if instruction.mnemonic() == Mnemonic::Mov
            && instruction.op_kind(0) == OpKind::Memory
            && instruction.op_kind(1) == OpKind::Register
            && gpr64_index(instruction.op1_register())
                .is_some_and(|src| origins[src] == Origin::TargetPointer)
            && let Some(Origin::This(offset)) = memory_origin(&instruction, &origins)
            && let Ok(offset) = i32::try_from(offset)
        {
            match chosen {
                Some(existing) if existing != offset => return Err(ExtractionStatus::Ambiguous),
                Some(_) => {}
                None => chosen = Some(offset),
            }
        }
        update_origins(&instruction, &mut origins, target_rva);
        if instruction.mnemonic() == Mnemonic::Ret {
            break;
        }
    }
    chosen.ok_or(ExtractionStatus::NoDisp)
}

fn read_vbtable_entry(
    pe: &PeImage,
    vbtable_rva: u32,
    entry_offset: usize,
) -> Result<i32, ExtractionStatus> {
    let entry_rva = vbtable_rva
        .checked_add(u32::try_from(entry_offset).map_err(|_| ExtractionStatus::NoDisp)?)
        .ok_or(ExtractionStatus::NoDisp)?;
    let bytes = pe
        .bytes_at(entry_rva, size_of::<i32>())
        .map_err(|_| ExtractionStatus::NoDisp)?;
    Ok(i32::from_le_bytes(bytes.try_into().expect("four bytes")))
}

fn vbtable_displacement_origin(
    instruction: &Instruction,
    origins: &[Origin; GPR_COUNT],
) -> Option<Origin> {
    if instruction.op_kind(1) != OpKind::Memory || instruction.memory_index() != Register::None {
        return None;
    }
    let base = gpr64_index(instruction.memory_base())?;
    let Origin::Vbtable { vbptr_offset } = origins[base] else {
        return None;
    };
    let entry_offset = i64::try_from(instruction.memory_displacement64()).ok()?;
    let vbtable_entry_offset = usize::try_from(entry_offset).ok()?;
    Some(Origin::VbtableDisplacement(
        DeviceResourceVirtualBaseAccess {
            vbptr_offset,
            vbtable_entry_offset,
        },
    ))
}

fn memory_origin(instruction: &Instruction, origins: &[Origin; GPR_COUNT]) -> Option<Origin> {
    if instruction.op_kind(0) != OpKind::Memory && instruction.op_kind(1) != OpKind::Memory {
        return None;
    }
    let displacement = instruction.memory_displacement64() as i64;
    let base = register_origin(instruction.memory_base(), origins);
    let index = register_origin(instruction.memory_index(), origins);
    let scale = i64::from(instruction.memory_index_scale());

    match (base, index) {
        (Some(Origin::This(base_offset)), None) => {
            base_offset.checked_add(displacement).map(Origin::This)
        }
        (
            Some(Origin::VirtualBase {
                access,
                fixed_offset,
            }),
            None,
        ) => fixed_offset
            .checked_add(displacement)
            .map(|fixed_offset| Origin::VirtualBase {
                access,
                fixed_offset,
            }),
        (Some(Origin::This(base_offset)), Some(Origin::VbtableDisplacement(access)))
            if scale == 1 =>
        {
            base_offset
                .checked_add(displacement)
                .map(|fixed_offset| Origin::VirtualBase {
                    access,
                    fixed_offset,
                })
        }
        (Some(Origin::VbtableDisplacement(access)), Some(Origin::This(base_offset)))
            if scale == 1 =>
        {
            base_offset
                .checked_add(displacement)
                .map(|fixed_offset| Origin::VirtualBase {
                    access,
                    fixed_offset,
                })
        }
        _ => None,
    }
}

fn register_origin(reg: Register, origins: &[Origin; GPR_COUNT]) -> Option<Origin> {
    (reg != Register::None)
        .then(|| gpr64_index(reg).map(|index| origins[index]))
        .flatten()
}

fn instruction_destination_gpr(instruction: &Instruction) -> Option<usize> {
    (instruction.op_count() > 0 && instruction.op_kind(0) == OpKind::Register)
        .then(|| gpr64_index(instruction.op0_register()))
        .flatten()
        .filter(|dst| *dst != 4)
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

fn unique_rva(rvas: impl IntoIterator<Item = u32>) -> Result<u32, ExtractionStatus> {
    let values = distinct_rvas(rvas);
    match values.as_slice() {
        [] => Err(ExtractionStatus::NoSymbol),
        [rva] => Ok(*rva),
        _ => Err(ExtractionStatus::Ambiguous),
    }
}

fn distinct_rvas(rvas: impl IntoIterator<Item = u32>) -> Vec<u32> {
    let mut values = Vec::new();
    for rva in rvas {
        if !values.contains(&rva) {
            values.push(rva);
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use dwm_lut_profile::DeviceResourceVirtualBaseAccess;

    use super::{ExtractionStatus, extract_access_from_ctor, select_access};

    fn sample_ctor() -> Vec<u8> {
        let ip = 0x1000u64;
        let target = 0x2000u64;
        let mut bytes = vec![
            0x48, 0x8B, 0xD9, // mov rbx,rcx
            0x48, 0x8B, 0x43, 0x08, // mov rax,[rbx+8]
            0x48, 0x63, 0x50, 0x0C, // movsxd rdx,dword ptr [rax+0xc]
            0x48, 0x8D, 0x05, 0, 0, 0, 0, // lea rax,[rip+target]
            0x48, 0x89, 0x44, 0x13, 0x08, // mov [rbx+rdx+8],rax
            0xC3,
        ];
        let lea_next_ip = ip + 18;
        let displacement = i32::try_from(target - lea_next_ip).expect("sample displacement");
        bytes[14..18].copy_from_slice(&displacement.to_le_bytes());
        bytes
    }

    #[test]
    fn extracts_vbptr_and_vbtable_entry_offsets() {
        let bytes = sample_ctor();
        assert_eq!(
            extract_access_from_ctor(&bytes, 0x1000, 0x2000),
            Ok(DeviceResourceVirtualBaseAccess {
                vbptr_offset: 8,
                vbtable_entry_offset: 0x0c,
            })
        );
    }

    #[test]
    fn matching_candidates_are_accepted() {
        let access = DeviceResourceVirtualBaseAccess {
            vbptr_offset: 8,
            vbtable_entry_offset: 0x0c,
        };
        assert_eq!(select_access([Ok(access), Ok(access)]), Ok(access));
    }

    #[test]
    fn conflicting_candidates_are_ambiguous() {
        assert_eq!(
            select_access([
                Ok(DeviceResourceVirtualBaseAccess {
                    vbptr_offset: 8,
                    vbtable_entry_offset: 0x0c,
                }),
                Ok(DeviceResourceVirtualBaseAccess {
                    vbptr_offset: 0x18,
                    vbtable_entry_offset: 0x0c,
                }),
            ]),
            Err(ExtractionStatus::Ambiguous)
        );
    }

    #[test]
    fn candidates_without_a_layout_report_no_disp() {
        assert_eq!(
            select_access([Err(ExtractionStatus::NoDisp)]),
            Err(ExtractionStatus::NoDisp)
        );
    }

    #[test]
    fn candidates_without_required_symbols_report_no_symbol() {
        assert_eq!(
            select_access([Err(ExtractionStatus::NoSymbol)]),
            Err(ExtractionStatus::NoSymbol)
        );
    }
}
