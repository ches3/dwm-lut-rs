use dwm_lut_profile::{FieldOffsetExtractSpec, FieldOffsetTarget};

use iced_x86::{Decoder, DecoderOptions, Instruction, OpKind, Register};

use super::report::{ExtractionRow, ExtractionStatus};
use crate::profile::pdb_publics::PdbPublics;
use crate::profile::pe::PeImage;

const GETTER_DISP_MAX_SCAN: usize = 0x80;
const CTOR_ARG_STORE_MAX_SCAN: usize = 0x200;
const GPR_COUNT: usize = 16;
const VOLATILE_GPRS: &[usize] = &[0, 1, 2, 8, 9, 10, 11];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GetterOrigin {
    Unknown,
    This(usize),
    Field(usize),
    Output,
}

pub fn extract_field_offsets(
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Vec<ExtractionRow<FieldOffsetTarget>> {
    FieldOffsetTarget::ALL
        .iter()
        .copied()
        .map(|target| extract_target(target, pe, pubs))
        .collect()
}

fn extract_target(
    target: FieldOffsetTarget,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> ExtractionRow<FieldOffsetTarget> {
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
    spec: FieldOffsetExtractSpec,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<usize, ExtractionStatus> {
    match spec {
        FieldOffsetExtractSpec::GetterDisp {
            symbol_prefix,
            derived_add,
        } => interpret_getter_disp(symbol_prefix, derived_add, pe, pubs),
        FieldOffsetExtractSpec::CtorFirstParamStore {
            ctor_symbol_prefix,
            first_param_type_encoding,
        } => interpret_ctor_first_param_store(
            ctor_symbol_prefix,
            first_param_type_encoding,
            pe,
            pubs,
        ),
    }
}

fn interpret_getter_disp(
    symbol_prefix: &str,
    derived_add: Option<usize>,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<usize, ExtractionStatus> {
    let rva = unique_symbol_rva(symbol_prefix, pubs)?;
    let bytes = pe
        .bytes_at(rva, GETTER_DISP_MAX_SCAN)
        .map_err(|_| ExtractionStatus::NoDisp)?;
    let disp = returned_memory_disp(bytes, u64::from(rva))?;
    match derived_add {
        Some(add) => disp.checked_add(add).ok_or(ExtractionStatus::NoDisp),
        None => Ok(disp),
    }
}

fn interpret_ctor_first_param_store(
    ctor_symbol_prefix: &str,
    first_param_type_encoding: &str,
    pe: &PeImage,
    pubs: &PdbPublics,
) -> Result<usize, ExtractionStatus> {
    let mut rvas = Vec::new();
    for symbol in &pubs.symbols {
        if ctor_has_first_param(&symbol.name, ctor_symbol_prefix, first_param_type_encoding)
            && !rvas.contains(&symbol.rva)
        {
            rvas.push(symbol.rva);
        }
    }
    if rvas.is_empty() {
        return Err(ExtractionStatus::NoSymbol);
    }

    select_ctor_first_param_store_disp(rvas.into_iter().map(|rva| {
        let bytes = pe
            .bytes_at(rva, CTOR_ARG_STORE_MAX_SCAN)
            .map_err(|_| ExtractionStatus::NoDisp)?;
        this_store_from_first_param_disp(bytes, u64::from(rva))
    }))
}

fn select_ctor_first_param_store_disp(
    results: impl IntoIterator<Item = Result<usize, ExtractionStatus>>,
) -> Result<usize, ExtractionStatus> {
    let mut chosen = None;
    for result in results {
        let disp = match result {
            Ok(disp) => disp,
            Err(ExtractionStatus::NoDisp) => continue,
            Err(status) => return Err(status),
        };
        match chosen {
            Some(existing) if existing != disp => return Err(ExtractionStatus::Ambiguous),
            Some(_) => {}
            None => chosen = Some(disp),
        }
    }
    chosen.ok_or(ExtractionStatus::NoDisp)
}

fn ctor_has_first_param(
    symbol: &str,
    ctor_symbol_prefix: &str,
    first_param_type_encoding: &str,
) -> bool {
    let Some(signature) = symbol.strip_prefix(ctor_symbol_prefix) else {
        return false;
    };
    let Some((_, params)) = signature.split_once('@') else {
        return false;
    };
    params.starts_with(first_param_type_encoding)
}

fn unique_symbol_rva(symbol_prefix: &str, pubs: &PdbPublics) -> Result<u32, ExtractionStatus> {
    unique_rva(
        pubs.symbols
            .iter()
            .filter(|symbol| symbol.name.starts_with(symbol_prefix))
            .map(|symbol| symbol.rva),
    )
}

fn unique_rva(rvas: impl IntoIterator<Item = u32>) -> Result<u32, ExtractionStatus> {
    let mut chosen = None;
    for rva in rvas {
        match chosen {
            Some(existing) if existing != rva => return Err(ExtractionStatus::Ambiguous),
            Some(_) => {}
            None => chosen = Some(rva),
        }
    }
    chosen.ok_or(ExtractionStatus::NoSymbol)
}

fn returned_memory_disp(bytes: &[u8], ip: u64) -> Result<usize, ExtractionStatus> {
    let mut decoder = Decoder::with_ip(64, bytes, ip, DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    let mut origins = [GetterOrigin::Unknown; GPR_COUNT];
    origins[gpr64_index(Register::RCX).expect("rcx")] = GetterOrigin::This(0);
    origins[gpr64_index(Register::RDX).expect("rdx")] = GetterOrigin::Output;
    let mut output_field = None;

    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        if instruction.mnemonic() == iced_x86::Mnemonic::Ret {
            return match origins[gpr64_index(Register::RAX).expect("rax")] {
                GetterOrigin::Field(disp) => Ok(disp),
                GetterOrigin::Output => output_field.ok_or(ExtractionStatus::NoDisp),
                GetterOrigin::Unknown | GetterOrigin::This(_) => Err(ExtractionStatus::NoDisp),
            };
        }
        if let Some(disp) = output_store_field(&instruction, &origins) {
            match output_field {
                Some(existing) if existing != disp => return Err(ExtractionStatus::Ambiguous),
                Some(_) => {}
                None => output_field = Some(disp),
            }
        }
        update_getter_origins(&instruction, &mut origins);
    }
    Err(ExtractionStatus::NoDisp)
}

fn output_store_field(
    instruction: &Instruction,
    origins: &[GetterOrigin; GPR_COUNT],
) -> Option<usize> {
    if instruction.mnemonic() != iced_x86::Mnemonic::Mov
        || instruction.op_kind(0) != OpKind::Memory
        || instruction.memory_index() != Register::None
        || instruction.op_kind(1) != OpKind::Register
    {
        return None;
    }
    let base = gpr64_index(instruction.memory_base())?;
    let src = gpr64_index(instruction.op1_register())?;
    (origins[base] == GetterOrigin::Output).then_some(())?;
    match origins[src] {
        GetterOrigin::Field(disp) => Some(disp),
        GetterOrigin::Unknown | GetterOrigin::This(_) | GetterOrigin::Output => None,
    }
}

fn update_getter_origins(instruction: &Instruction, origins: &mut [GetterOrigin; GPR_COUNT]) {
    use iced_x86::Mnemonic;

    match instruction.mnemonic() {
        Mnemonic::Mov | Mnemonic::Movsx | Mnemonic::Movsxd | Mnemonic::Movzx => {
            let Some(dst) = instruction_destination_gpr(instruction) else {
                return;
            };
            origins[dst] = match instruction.op_kind(1) {
                OpKind::Register => gpr64_index(instruction.op1_register())
                    .map(|src| origins[src])
                    .unwrap_or(GetterOrigin::Unknown),
                OpKind::Memory if instruction.memory_index() == Register::None => {
                    this_relative_disp(instruction, origins)
                        .map(GetterOrigin::Field)
                        .unwrap_or(GetterOrigin::Unknown)
                }
                _ => GetterOrigin::Unknown,
            };
        }
        Mnemonic::Lea => {
            let Some(dst) = instruction_destination_gpr(instruction) else {
                return;
            };
            origins[dst] = if instruction.op_kind(1) == OpKind::Memory
                && instruction.memory_index() == Register::None
            {
                this_relative_disp(instruction, origins)
                    .map(GetterOrigin::This)
                    .unwrap_or(GetterOrigin::Unknown)
            } else {
                GetterOrigin::Unknown
            };
        }
        Mnemonic::Call => clear_volatile(origins, GetterOrigin::Unknown),
        Mnemonic::Cmp | Mnemonic::Test | Mnemonic::Push => {}
        _ => {
            if let Some(dst) = instruction_destination_gpr(instruction) {
                origins[dst] = GetterOrigin::Unknown;
            }
        }
    }
}

fn this_relative_disp(
    instruction: &Instruction,
    origins: &[GetterOrigin; GPR_COUNT],
) -> Option<usize> {
    let base = gpr64_index(instruction.memory_base())?;
    let GetterOrigin::This(base_disp) = origins[base] else {
        return None;
    };
    let disp = instruction.memory_displacement64() as i64;
    let total = i64::try_from(base_disp).ok()?.checked_add(disp)?;
    (0..0x400).contains(&total).then_some(total as usize)
}

fn instruction_destination_gpr(instruction: &Instruction) -> Option<usize> {
    (instruction.op_count() > 0 && instruction.op_kind(0) == OpKind::Register)
        .then(|| gpr64_index(instruction.op0_register()))
        .flatten()
        .filter(|dst| *dst != 4)
}

fn this_store_from_first_param_disp(bytes: &[u8], ip: u64) -> Result<usize, ExtractionStatus> {
    let mut decoder = Decoder::with_ip(64, bytes, ip, DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    let mut this_offsets = [None; GPR_COUNT];
    let mut arg_derived = [false; GPR_COUNT];
    this_offsets[gpr64_index(Register::RCX).expect("rcx")] = Some(0i64);
    arg_derived[gpr64_index(Register::RDX).expect("rdx")] = true;
    let mut chosen = None;

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
            let base_offset =
                gpr64_index(instruction.memory_base()).and_then(|base| this_offsets[base]);
            let src_ok =
                gpr64_index(instruction.op1_register()).is_some_and(|src| arg_derived[src]);
            if let (Some(base_offset), true) = (base_offset, src_ok) {
                let disp = base_offset
                    .checked_add(instruction.memory_displacement64() as i64)
                    .filter(|disp| (0..0x400).contains(disp));
                if let Some(disp) = disp {
                    let disp = disp as usize;
                    match chosen {
                        Some(existing) if existing != disp => {
                            return Err(ExtractionStatus::Ambiguous);
                        }
                        Some(_) => {}
                        None => chosen = Some(disp),
                    }
                }
            }
        }
        update_this_offsets(&instruction, &mut this_offsets);
        update_arg_derived(&instruction, &mut arg_derived);
    }
    chosen.ok_or(ExtractionStatus::NoDisp)
}

fn update_arg_derived(instruction: &Instruction, arg_derived: &mut [bool; GPR_COUNT]) {
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
        Mnemonic::Call => clear_volatile(arg_derived, false),
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

fn update_this_offsets(instruction: &Instruction, this_offsets: &mut [Option<i64>; GPR_COUNT]) {
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
            this_offsets[dst] = match instruction.op_kind(1) {
                OpKind::Register => {
                    gpr64_index(instruction.op1_register()).and_then(|src| this_offsets[src])
                }
                _ => None,
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
            this_offsets[dst] = if instruction.op_kind(1) == OpKind::Memory
                && instruction.memory_index() == Register::None
            {
                gpr64_index(instruction.memory_base())
                    .and_then(|base| this_offsets[base])
                    .and_then(|offset| {
                        offset.checked_add(instruction.memory_displacement64() as i64)
                    })
            } else {
                None
            };
        }
        Mnemonic::Call => clear_volatile(this_offsets, None),
        Mnemonic::Cmp | Mnemonic::Test | Mnemonic::Push => {}
        _ => {
            if instruction.op_count() == 0 || instruction.op_kind(0) != OpKind::Register {
                return;
            }
            let Some(dst) = gpr64_index(instruction.op0_register()) else {
                return;
            };
            if dst != 4 {
                this_offsets[dst] = None;
            }
        }
    }
}

fn clear_volatile<T: Copy>(values: &mut [T; GPR_COUNT], cleared: T) {
    for &index in VOLATILE_GPRS {
        values[index] = cleared;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExtractionStatus, ctor_has_first_param, returned_memory_disp,
        select_ctor_first_param_store_disp, this_store_from_first_param_disp, unique_rva,
    };

    #[test]
    fn unique_rva_requires_one_distinct_address() {
        assert_eq!(unique_rva([0x1000, 0x1000]), Ok(0x1000));
        assert_eq!(
            unique_rva([0x1000, 0x2000]),
            Err(ExtractionStatus::Ambiguous)
        );
        assert_eq!(unique_rva([]), Err(ExtractionStatus::NoSymbol));
    }

    #[test]
    fn getter_disp_follows_the_returned_field() {
        let direct = [0x8B, 0x51, 0x10, 0x8B, 0x41, 0x34, 0xC3];
        assert_eq!(returned_memory_disp(&direct, 0), Ok(0x34));

        let output_buffer = [
            0x48, 0x8B, 0x41, 0x34, 0x48, 0x89, 0x02, 0x48, 0x8B, 0xC2, 0xC3,
        ];
        assert_eq!(returned_memory_disp(&output_buffer, 0), Ok(0x34));
    }

    #[test]
    fn ctor_first_param_store_rejects_argument_clobbered_by_call() {
        let clobbered = [0xE8, 0x00, 0x00, 0x00, 0x00, 0x48, 0x89, 0x51, 0x20, 0xC3];
        assert_eq!(
            this_store_from_first_param_disp(&clobbered, 0),
            Err(ExtractionStatus::NoDisp)
        );
    }

    #[test]
    fn ctor_first_param_store_tracks_callee_saved_registers_across_call() {
        let preserved = [
            0x48, 0x8B, 0xD9, 0x48, 0x8B, 0xFA, 0xE8, 0x00, 0x00, 0x00, 0x00, 0x48, 0x89, 0x7B,
            0x38, 0xC3,
        ];
        assert_eq!(this_store_from_first_param_disp(&preserved, 0), Ok(0x38));
    }

    #[test]
    fn ctor_first_param_store_accounts_for_adjusted_this() {
        let adjusted_this = [0x48, 0x8D, 0x59, 0x10, 0x48, 0x89, 0x53, 0x20, 0xC3];
        assert_eq!(
            this_store_from_first_param_disp(&adjusted_this, 0),
            Ok(0x30)
        );
    }

    #[test]
    fn ctor_first_param_store_ignores_overloads_without_a_displacement() {
        assert_eq!(
            select_ctor_first_param_store_disp([Err(ExtractionStatus::NoDisp), Ok(0x30),]),
            Ok(0x30)
        );
    }

    #[test]
    fn ctor_first_param_match_uses_parameter_list() {
        let ctor = "??0COverlayContext@@";
        let first_param = "PEAVIOverlayMonitorTarget@@";
        assert!(ctor_has_first_param(
            "??0COverlayContext@@QEAA@PEAVIOverlayMonitorTarget@@@Z",
            ctor,
            first_param
        ));
        assert!(ctor_has_first_param(
            "??0COverlayContext@@IEAA@PEAVIOverlayMonitorTarget@@_N@Z",
            ctor,
            first_param
        ));
        assert!(!ctor_has_first_param(
            "??0COverlayContext@@QEAA@AEBV0@@Z",
            ctor,
            first_param
        ));
    }
}
