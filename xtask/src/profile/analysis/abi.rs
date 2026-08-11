//! ABI extraction and comparison used by profile analysis.

use dwm_lut_profile::{
    AbiArgument, AbiClass, AbiObjectType, AbiReturnType, AbiType, HookEntry, HookExtractSpec,
    HookTarget, VirtualMethodExtractSpec, VirtualMethodReturnType,
};
use msvc_demangler::{DemangleFlags, demangle};

use super::symbols::{
    EXTRACT_TARGETS, decorated_name_for_resolved_symbol, demangle_marker, resolve_function_symbol,
};
use super::virtual_method_return_type_label;
use crate::profile::pdb_publics::PdbPublics;
use crate::profile::pe::PeImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiExtractionStatus {
    Ok,
    NoSymbol,
    Unsupported,
}

impl AbiExtractionStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoSymbol => "no_symbol",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnPassing {
    Register,
    Sret,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedFunctionAbi {
    pub receiver: ExtractedReceiver,
    pub return_type: ExtractedAbiType,
    pub arguments: Vec<ExtractedAbiType>,
    pub return_passing: ReturnPassing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedReceiver {
    pub type_name: String,
    pub is_const: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarKind {
    HResult,
    Signed32,
    Unsigned32,
    CppBool,
    OtherInteger,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Indirection {
    Pointer,
    Reference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedAbiType {
    display: String,
    scalar: Option<ScalarKind>,
    indirection: Option<Indirection>,
    type_name: Option<String>,
    is_const: bool,
    class: Option<AbiClass>,
}

impl ExtractedAbiType {
    pub fn display(&self) -> &str {
        &self.display
    }

    pub const fn class(&self) -> Option<AbiClass> {
        self.class
    }

    pub fn matches_return_type(&self, expected: AbiReturnType) -> bool {
        match expected {
            AbiReturnType::HResult => matches!(
                self.scalar,
                Some(ScalarKind::HResult | ScalarKind::Signed32)
            ),
            AbiReturnType::CppBool => self.scalar == Some(ScalarKind::CppBool),
        }
    }

    pub fn matches_type(&self, expected: AbiType) -> bool {
        let (indirection, object_type, is_const) = match expected {
            AbiType::Pointer { pointee, is_const } => (Indirection::Pointer, pointee, is_const),
            AbiType::Reference { referent, is_const } => {
                (Indirection::Reference, referent, is_const)
            }
        };

        self.indirection == Some(indirection)
            && self.is_const == is_const
            && self
                .type_name
                .as_deref()
                .is_some_and(|actual| matches_abi_object_type(actual, object_type))
    }

    fn matches_argument(&self, expected: AbiArgument) -> bool {
        match expected.exact_type() {
            Some(exact) => self.matches_type(exact),
            None => self.class == Some(expected.class()),
        }
    }

    fn matches_virtual_return_type(&self, expected: VirtualMethodReturnType) -> bool {
        match expected {
            VirtualMethodReturnType::PointerTo(type_name) => {
                self.indirection == Some(Indirection::Pointer)
                    && self.type_name.as_deref().is_some_and(|actual| {
                        normalize_type_name(actual) == normalize_type_name(type_name)
                    })
            }
            VirtualMethodReturnType::SretValue(type_name) => {
                self.indirection.is_none()
                    && self.type_name.as_deref().is_some_and(|actual| {
                        normalize_type_name(actual) == normalize_type_name(type_name)
                    })
            }
            VirtualMethodReturnType::Unsigned32 => self.scalar == Some(ScalarKind::Unsigned32),
            VirtualMethodReturnType::CppBool => self.scalar == Some(ScalarKind::CppBool),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AbiExtractionRow {
    pub target: HookTarget,
    pub status: AbiExtractionStatus,
    pub abi: Option<ExtractedFunctionAbi>,
    pub detail: Option<String>,
}

pub fn extract_function_abis(
    pe: &PeImage,
    publics: &PdbPublics,
) -> Result<Vec<AbiExtractionRow>, String> {
    let mut rows = Vec::new();
    for &target in EXTRACT_TARGETS {
        let symbol = match resolve_function_symbol(target, publics, pe) {
            Ok(symbol) => symbol,
            Err(_) => {
                rows.push(AbiExtractionRow {
                    target,
                    status: AbiExtractionStatus::NoSymbol,
                    abi: None,
                    detail: None,
                });
                continue;
            }
        };
        let Some(decorated) = decorated_name_for_resolved_symbol(target, publics, symbol.rva)
        else {
            rows.push(AbiExtractionRow {
                target,
                status: AbiExtractionStatus::Unsupported,
                abi: None,
                detail: Some("resolved symbol has no decorated public name".into()),
            });
            continue;
        };
        match parse_decorated_function_abi(target, &decorated) {
            Ok(abi) => rows.push(AbiExtractionRow {
                target,
                status: AbiExtractionStatus::Ok,
                abi: Some(abi),
                detail: None,
            }),
            Err(detail) => rows.push(AbiExtractionRow {
                target,
                status: AbiExtractionStatus::Unsupported,
                abi: None,
                detail: Some(detail),
            }),
        }
    }

    rows.sort_by_key(|row| {
        HookTarget::ALL
            .iter()
            .position(|target| *target == row.target)
            .unwrap_or(usize::MAX)
    });
    Ok(rows)
}

fn normalize_type_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn matches_abi_object_type(actual: &str, expected: AbiObjectType) -> bool {
    match expected {
        AbiObjectType::Named(type_name) => {
            normalize_type_name(actual) == normalize_type_name(type_name)
        }
        AbiObjectType::StdVector(element) => {
            let expected = format!("std::vector<{element},std::allocator<{element}>>");
            normalize_type_name(actual) == normalize_type_name(&expected)
        }
    }
}

fn parse_decorated_function_abi(
    target: HookTarget,
    decorated: &str,
) -> Result<ExtractedFunctionAbi, String> {
    let demangled = demangle(decorated, DemangleFlags::llvm())
        .map_err(|error| format!("failed to demangle {decorated}: {error}"))?;
    let spec = target.extract_spec();
    let (marker_start, marker_len) = spec
        .method_names
        .iter()
        .find_map(|method_name| {
            let marker = demangle_marker(spec.class_name, method_name);
            demangled
                .find(&marker)
                .map(|marker_start| (marker_start, marker.len()))
        })
        .ok_or_else(|| {
            let markers = spec
                .method_names
                .iter()
                .map(|method_name| demangle_marker(spec.class_name, method_name))
                .collect::<Vec<_>>()
                .join(", ");
            format!("demangled symbol has no {{{markers}}}: {demangled}")
        })?;
    let abi = parse_demangled_function_abi(
        &demangled,
        marker_start,
        marker_start + marker_len,
        spec.class_name,
    )?;
    validate_required_argument_roles(spec, &abi)?;
    Ok(abi)
}

fn validate_required_argument_roles(
    spec: HookExtractSpec,
    abi: &ExtractedFunctionAbi,
) -> Result<(), String> {
    for &role in spec.required_argument_roles {
        let Some(exact_type) = role.exact_type() else {
            return Err(format!(
                "required argument role {} has no exact type",
                role.extract_label()
            ));
        };
        let match_count = abi
            .arguments
            .iter()
            .filter(|argument| argument.matches_type(exact_type))
            .count();
        match match_count {
            1 => {}
            0 => {
                return Err(format!(
                    "required argument role {} was not found",
                    role.extract_label()
                ));
            }
            _ => {
                return Err(format!(
                    "required argument role {} matched {match_count} arguments",
                    role.extract_label()
                ));
            }
        }
    }
    Ok(())
}

pub fn classify_hook_argument(
    target: HookTarget,
    argument: &ExtractedAbiType,
) -> Option<AbiArgument> {
    target
        .extract_spec()
        .required_argument_roles
        .iter()
        .copied()
        .find(|role| {
            role.exact_type()
                .is_some_and(|exact_type| argument.matches_type(exact_type))
        })
        .or_else(|| argument.class().map(AbiArgument::Wildcard))
}

pub fn parse_decorated_virtual_method_abi(
    spec: VirtualMethodExtractSpec,
    decorated: &str,
) -> Result<ExtractedFunctionAbi, String> {
    let demangled = demangle_virtual_method_symbol(decorated)?;
    let (marker_start, marker_len) = spec
        .method_names
        .iter()
        .find_map(|method_name| {
            let marker = format!("::{method_name}(");
            demangled
                .find(&marker)
                .map(|marker_start| (marker_start, marker.len()))
        })
        .ok_or_else(|| {
            let methods = spec.method_names.join(", ");
            format!("demangled symbol has no virtual method {{{methods}}}: {demangled}")
        })?;
    let receiver_start = demangled[..marker_start]
        .rfind(char::is_whitespace)
        .map_or(0, |position| position + 1);
    let receiver = demangled[receiver_start..marker_start].trim();
    if receiver.is_empty() {
        return Err(format!("demangled symbol has no receiver: {demangled}"));
    }

    parse_demangled_function_abi(
        &demangled,
        receiver_start,
        marker_start + marker_len,
        receiver,
    )
}

fn demangle_virtual_method_symbol(decorated: &str) -> Result<String, String> {
    match demangle(decorated, DemangleFlags::llvm()) {
        Ok(demangled) => Ok(demangled),
        Err(original_error) => {
            let Some(normalized) =
                normalize_virtual_this_adjust_thunk(decorated).map_err(|detail| {
                    format!("failed to demangle {decorated}: {original_error}; {detail}")
                })?
            else {
                return Err(format!("failed to demangle {decorated}: {original_error}"));
            };

            demangle(&normalized, DemangleFlags::llvm()).map_err(|error| {
                format!(
                    "failed to demangle {decorated} after normalizing virtual-this-adjust thunk: {error}"
                )
            })
        }
    }
}

fn normalize_virtual_this_adjust_thunk(decorated: &str) -> Result<Option<String>, String> {
    const FUNCTION_CLASS_PREFIX: &str = "@@$";

    let Some(marker_start) = decorated.find(FUNCTION_CLASS_PREFIX) else {
        return Ok(None);
    };
    let mut encoding = &decorated[marker_start + FUNCTION_CLASS_PREFIX.len()..];
    let extended = encoding.starts_with('R');
    if extended {
        encoding = &encoding[1..];
    }

    let Some((&function_class, remainder)) = encoding.as_bytes().split_first() else {
        return Err("virtual-this-adjust thunk has no function class".into());
    };
    let normalized_class = match function_class {
        b'0' => 'E',
        b'1' => 'F',
        b'2' => 'M',
        b'3' => 'N',
        b'4' => 'U',
        b'5' => 'V',
        _ => {
            return Err(format!(
                "unsupported virtual-this-adjust function class ${}",
                char::from(function_class)
            ));
        }
    };

    encoding = std::str::from_utf8(remainder)
        .map_err(|_| "virtual-this-adjust thunk has a non-ASCII function class")?;
    let adjustment_count = if extended { 4 } else { 2 };
    for adjustment_index in 0..adjustment_count {
        encoding = skip_msvc_signed_number(encoding).ok_or_else(|| {
            format!(
                "virtual-this-adjust thunk has an invalid adjustment at index {adjustment_index}"
            )
        })?;
    }

    let mut normalized = String::with_capacity(decorated.len());
    normalized.push_str(&decorated[..marker_start + 2]);
    normalized.push(normalized_class);
    normalized.push_str(encoding);
    Ok(Some(normalized))
}

fn skip_msvc_signed_number(value: &str) -> Option<&str> {
    let value = value.strip_prefix('?').unwrap_or(value);
    let first = *value.as_bytes().first()?;
    if first.is_ascii_digit() {
        return Some(&value[1..]);
    }

    let terminator = value.find('@')?;
    let digits = &value.as_bytes()[..terminator];
    if digits.is_empty() || !digits.iter().all(|digit| matches!(digit, b'A'..=b'P')) {
        return None;
    }
    Some(&value[terminator + 1..])
}

fn parse_demangled_function_abi(
    demangled: &str,
    return_end: usize,
    arguments_start: usize,
    receiver: &str,
) -> Result<ExtractedFunctionAbi, String> {
    let arguments_end = find_closing_parenthesis(demangled, arguments_start)
        .ok_or_else(|| format!("demangled symbol has no closing parenthesis: {demangled}"))?;
    let return_prefix = demangled[..return_end].trim();
    let return_name = return_prefix
        .split_whitespace()
        .filter(|word| {
            !matches!(
                *word,
                "public:"
                    | "private:"
                    | "protected:"
                    | "static"
                    | "virtual"
                    | "__cdecl"
                    | "__thiscall"
                    | "__stdcall"
                    | "__fastcall"
                    | "__vectorcall"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    if return_name.is_empty() {
        return Err(format!("demangled symbol has no return type: {demangled}"));
    }
    let return_type = parse_demangled_type(&return_name)?;
    let argument_text = demangled[arguments_start..arguments_end].trim();
    let arguments = if argument_text.is_empty() || argument_text == "void" {
        Vec::new()
    } else {
        split_top_level_arguments(argument_text)
            .into_iter()
            .map(parse_demangled_type)
            .collect::<Result<Vec<_>, _>>()?
    };
    let suffix = &demangled[arguments_end + 1..];

    Ok(ExtractedFunctionAbi {
        receiver: ExtractedReceiver {
            type_name: receiver.into(),
            is_const: suffix.split_whitespace().any(|word| word == "const"),
        },
        return_type,
        arguments,
        return_passing: ReturnPassing::Register,
    })
}

fn find_closing_parenthesis(value: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, character) in value[start..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(start + offset),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn split_top_level_arguments(arguments: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut angle_depth = 0usize;
    let mut parenthesis_depth = 0usize;
    let mut start = 0usize;
    for (offset, character) in arguments.char_indices() {
        match character {
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            '(' => parenthesis_depth += 1,
            ')' => parenthesis_depth = parenthesis_depth.saturating_sub(1),
            ',' if angle_depth == 0 && parenthesis_depth == 0 => {
                result.push(arguments[start..offset].trim());
                start = offset + 1;
            }
            _ => {}
        }
    }
    result.push(arguments[start..].trim());
    result
}

fn parse_demangled_type(value: &str) -> Result<ExtractedAbiType, String> {
    let cleaned = remove_type_keywords(value.trim());
    let compact = cleaned.trim();
    let is_reference = compact.ends_with('&');
    let is_pointer = compact.ends_with('*');
    if is_reference || is_pointer {
        let without_indirection = compact[..compact.len() - 1].trim();
        let is_const = without_indirection.ends_with(" const");
        let type_name = without_indirection
            .strip_suffix(" const")
            .unwrap_or(without_indirection)
            .trim()
            .to_string();
        return Ok(ExtractedAbiType {
            display: compact.into(),
            scalar: None,
            indirection: Some(if is_reference {
                Indirection::Reference
            } else {
                Indirection::Pointer
            }),
            type_name: Some(type_name),
            is_const,
            class: Some(AbiClass::Integer),
        });
    }

    let scalar = match compact {
        "HRESULT" => ScalarKind::HResult,
        "long" | "int" | "__int32" => ScalarKind::Signed32,
        "bool" => ScalarKind::CppBool,
        "unsigned int" | "unsigned long" => ScalarKind::Unsigned32,
        "unsigned __int64" | "__int64" => ScalarKind::OtherInteger,
        value if value.starts_with("enum ") => ScalarKind::OtherInteger,
        _ => ScalarKind::Other,
    };
    let (scalar, type_name, class) = if scalar == ScalarKind::Other {
        (None, Some(compact.into()), None)
    } else {
        (Some(scalar), None, Some(AbiClass::Integer))
    };
    Ok(ExtractedAbiType {
        display: compact.into(),
        scalar,
        indirection: None,
        type_name,
        is_const: false,
        class,
    })
}

fn remove_type_keywords(value: &str) -> String {
    value
        .replace("class ", "")
        .replace("struct ", "")
        .replace(" __ptr64", "")
}

pub fn abi_matches_profile(entry: HookEntry, extracted: &ExtractedFunctionAbi) -> bool {
    extracted
        .return_type
        .matches_return_type(entry.target.extract_spec().return_type)
        && extracted.arguments.len() == entry.arguments.len()
        && entry
            .arguments
            .iter()
            .zip(&extracted.arguments)
            .all(|(expected, actual)| actual.matches_argument(*expected))
}

pub fn abi_matches_virtual_method(
    spec: VirtualMethodExtractSpec,
    extracted: &ExtractedFunctionAbi,
) -> bool {
    let expected_passing = if matches!(spec.return_type, VirtualMethodReturnType::SretValue(_)) {
        ReturnPassing::Sret
    } else {
        ReturnPassing::Register
    };
    demangled_virtual_return_matches(spec, extracted)
        && extracted.return_passing == expected_passing
}

pub fn demangled_virtual_return_matches(
    spec: VirtualMethodExtractSpec,
    extracted: &ExtractedFunctionAbi,
) -> bool {
    extracted.arguments.is_empty()
        && extracted
            .return_type
            .matches_virtual_return_type(spec.return_type)
}

/// Detects the common MSVC x64 sret prologue that stores an 8-byte aggregate through RDX:
/// `mov rax, [rcx+disp]; mov [rdx], rax; mov rax, rdx; ret`.
pub fn is_msvc_x64_sret8_prologue(bytes: &[u8]) -> bool {
    if bytes.len() < 11 {
        return false;
    }
    // mov rax, qword ptr [rcx+disp8]
    if bytes[0..3] == [0x48, 0x8B, 0x41]
        && bytes[4..11] == [0x48, 0x89, 0x02, 0x48, 0x8B, 0xC2, 0xC3]
    {
        return true;
    }
    // mov rax, qword ptr [rcx+disp32]
    if bytes.len() >= 14
        && bytes[0..3] == [0x48, 0x8B, 0x81]
        && bytes[7..14] == [0x48, 0x89, 0x02, 0x48, 0x8B, 0xC2, 0xC3]
    {
        return true;
    }
    false
}

pub fn apply_return_passing_from_prologue(
    spec: VirtualMethodExtractSpec,
    abi: &mut ExtractedFunctionAbi,
    prologue: &[u8],
) -> Result<(), String> {
    if !matches!(spec.return_type, VirtualMethodReturnType::SretValue(_)) {
        abi.return_passing = ReturnPassing::Register;
        return Ok(());
    }
    if is_msvc_x64_sret8_prologue(prologue) {
        abi.return_passing = ReturnPassing::Sret;
        Ok(())
    } else {
        abi.return_passing = ReturnPassing::Register;
        Err(format!(
            "expected MSVC x64 sret prologue for {}",
            virtual_method_return_type_label(spec.return_type)
        ))
    }
}

#[cfg(test)]
mod tests {
    use dwm_lut_profile::{
        AbiArgument, AbiClass, AbiReturnType, HookEntry, HookTarget, VirtualMethodReturnType,
        VirtualMethodTarget,
    };

    #[test]
    fn present_decorated_signatures_preserve_bound_argument_positions() {
        let five = super::parse_decorated_function_abi(
            HookTarget::Present,
            "?Present@COverlayContext@@QEAAJPEAVIOverlaySwapChain@@IAEBV?$vector@UtagRECT@@V?$allocator@UtagRECT@@@std@@@std@@I_N@Z",
        )
        .expect("five-argument Present should parse");
        let six = super::parse_decorated_function_abi(
            HookTarget::Present,
            "?Present@COverlayContext@@QEAAJPEAVIOverlaySwapChain@@IAEBV?$vector@UtagRECT@@V?$allocator@UtagRECT@@@std@@@std@@IPEA_N_N@Z",
        )
        .expect("six-argument Present should parse");

        for arguments in [&five.arguments[..], &six.arguments[..]] {
            assert!(
                arguments[0].matches_type(
                    AbiArgument::OverlaySwapChain
                        .exact_type()
                        .expect("exact type")
                )
            );
            assert!(
                arguments[2]
                    .matches_type(AbiArgument::DirtyRects.exact_type().expect("exact type"))
            );
        }
        assert_eq!(five.arguments.len(), 5);
        assert_eq!(six.arguments.len(), 6);
        assert_eq!(
            five.arguments
                .iter()
                .map(|argument| {
                    super::classify_hook_argument(HookTarget::Present, argument)
                        .expect("Present argument should be classified")
                        .extract_label()
                })
                .collect::<Vec<_>>(),
            vec![
                "OverlaySwapChain",
                "AnyInteger",
                "DirtyRects",
                "AnyInteger",
                "AnyInteger",
            ]
        );
    }

    #[test]
    fn present_requires_each_argument_role_exactly_once() {
        let extracted = super::parse_decorated_function_abi(
            HookTarget::Present,
            "?Present@COverlayContext@@QEAAJPEAVIOverlaySwapChain@@IAEBV?$vector@UtagRECT@@V?$allocator@UtagRECT@@@std@@@std@@I_N@Z",
        )
        .expect("Present should parse");

        let mut missing = extracted.clone();
        missing.arguments[0] =
            super::parse_demangled_type("unsigned int").expect("integer should parse");
        assert_eq!(
            super::validate_required_argument_roles(HookTarget::Present.extract_spec(), &missing)
                .expect_err("missing role should be rejected"),
            "required argument role OverlaySwapChain was not found"
        );

        let mut duplicate = extracted;
        duplicate.arguments[1] = duplicate.arguments[0].clone();
        assert_eq!(
            super::validate_required_argument_roles(HookTarget::Present.extract_spec(), &duplicate)
                .expect_err("duplicate role should be rejected"),
            "required argument role OverlaySwapChain matched 2 arguments"
        );
    }

    #[test]
    fn bound_abi_types_reject_changed_value_types() {
        let changed_element =
            super::parse_demangled_type("std::vector<tagPOINT,std::allocator<tagPOINT>> const &")
                .expect("changed element type should parse");
        let changed_allocator =
            super::parse_demangled_type("std::vector<tagRECT,RectAllocator> const &")
                .expect("changed allocator should parse");
        let dirty_rects = AbiArgument::DirtyRects.exact_type().expect("exact type");

        assert!(!changed_element.matches_type(dirty_rects));
        assert!(!changed_allocator.matches_type(dirty_rects));
    }

    #[test]
    fn gate_decorated_signatures_extract_each_supported_argument_count() {
        let cases = [
            (
                HookTarget::IsCandidateDirectFlipCompatible,
                "?IsCandidateDirectFlipCompatible@COverlayContext@@AEBA_NPEAVCCompositionSurfaceInfo@@PEAVISwapChainRealization@@AEBUDXGI_MULTIPLANE_OVERLAY_ATTRIBUTES@@W4DXGI_MODE_ROTATION@@II_N@Z",
                7,
            ),
            (
                HookTarget::IsCandidateOverlayCompatible,
                "?IsCandidateOverlayCompatible@COverlayContext@@AEBA_NPEAVCVisual@@PEBVCCompositionSurfaceInfo@@PEAVISwapChainRealization@@AEBUDXGI_MULTIPLANE_OVERLAY_ATTRIBUTES@@W4DXGI_MODE_ROTATION@@I_N@Z",
                7,
            ),
            (
                HookTarget::IsCandidateDirectFlipCompatible,
                "?IsCandidateDirectFlipCompatible@COverlayContext@@AEBA_NPEAVCCompositionSurfaceInfo@@PEAVISwapChainRealization@@AEBUDXGI_MULTIPLANE_OVERLAY_ATTRIBUTES@@I_N@Z",
                5,
            ),
            (
                HookTarget::IsCandidateOverlayCompatible,
                "?IsCandidateOverlayCompatible@COverlayContext@@AEBA_NPEAVCVisual@@PEBVCCompositionSurfaceInfo@@PEAVISwapChainRealization@@AEAUDXGI_MULTIPLANE_OVERLAY_ATTRIBUTES@@W4OverlaySize@@_NI5@Z",
                8,
            ),
            (
                HookTarget::IsCandidateDirectFlipCompatible,
                "?IsCandidateDirectFlipCompatbile@COverlayContext@@AEBA_NPEBVCCompositionSurfaceInfo@@PEAVISwapChainRealization@@AEBUDXGI_MULTIPLANE_OVERLAY_ATTRIBUTES@@W4DXGI_MODE_ROTATION@@II_N@Z",
                7,
            ),
            (
                HookTarget::IsCandidateOverlayCompatible,
                "?IsCandidateOverlayCompatbile@COverlayContext@@AEBA_NPEAVCVisual@@PEBVCCompositionSurfaceInfo@@PEAVISwapChainRealization@@AEBUDXGI_MULTIPLANE_OVERLAY_ATTRIBUTES@@W4DXGI_MODE_ROTATION@@I_N@Z",
                7,
            ),
        ];

        for (target, decorated, expected_count) in cases {
            let abi = super::parse_decorated_function_abi(target, decorated)
                .expect("gate signature should parse");
            assert!(abi.receiver.is_const);
            assert!(abi.return_type.matches_return_type(AbiReturnType::CppBool));
            assert_eq!(abi.arguments.len(), expected_count);
            assert!(
                abi.arguments
                    .iter()
                    .all(|argument| argument.class() == Some(AbiClass::Integer))
            );
        }
    }

    #[test]
    fn extracted_present_abi_matches_profile_schema() {
        let extracted = super::parse_decorated_function_abi(
            HookTarget::Present,
            "?Present@COverlayContext@@QEAAJPEAVIOverlaySwapChain@@IAEBV?$vector@UtagRECT@@V?$allocator@UtagRECT@@@std@@@std@@I_N@Z",
        )
        .expect("Present should parse");
        let entry = HookEntry {
            target: HookTarget::Present,
            arguments: &[
                AbiArgument::OverlaySwapChain,
                AbiArgument::Wildcard(AbiClass::Integer),
                AbiArgument::DirtyRects,
                AbiArgument::Wildcard(AbiClass::Integer),
                AbiArgument::Wildcard(AbiClass::Integer),
            ],
            aob: &[],
        };

        assert!(super::abi_matches_profile(entry, &extracted));
    }

    #[test]
    fn virtual_method_decorated_signatures_match_extract_specs() {
        let cases = [
            (
                VirtualMethodTarget::GetPhysicalBackBuffer,
                "?GetPhysicalBackBuffer@CDDisplaySwapChain@@UEBAPEAVISwapChainBuffer@@XZ",
            ),
            (
                VirtualMethodTarget::GetD3D11Resource,
                "?GetD3D11Resource@CDDisplaySwapChainBuffer@@UEAAPEAUID3D11Resource@@XZ",
            ),
            (
                VirtualMethodTarget::GetCurrentBackBuffer,
                "?GetCurrentBackBuffer@CDDisplaySwapChain@@UEBAPEAVIDeviceTarget@@XZ",
            ),
            (
                VirtualMethodTarget::GetRenderTargetView,
                "?GetRenderTargetView@CDeviceTextureTarget@@UEBAPEAUID3D11RenderTargetView@@XZ",
            ),
            (
                VirtualMethodTarget::GetDisplayAdapterLuid,
                "?GetDisplayAdapterLuid@CDDisplaySwapChain@@UEBA?AU_LUID@@XZ",
            ),
            (
                VirtualMethodTarget::GetVidPnTargetId,
                "?GetVidPnTargetId@CDDisplaySwapChain@@UEBAIXZ",
            ),
            (
                VirtualMethodTarget::IsHardwareProtected,
                "?IsHardwareProtected@CDDisplaySwapChain@@UEBA_NXZ",
            ),
            (
                VirtualMethodTarget::GetOverlaySwapChain,
                "?GetOverlaySwapChain@CDDisplayRenderTarget@@UEBAPEAVIOverlaySwapChain@@XZ",
            ),
        ];

        for (target, decorated) in cases {
            let spec = target.extract_spec();
            let mut abi = super::parse_decorated_virtual_method_abi(spec, decorated)
                .expect("virtual method signature should parse");
            if matches!(spec.return_type, VirtualMethodReturnType::SretValue(_)) {
                super::apply_return_passing_from_prologue(
                    spec,
                    &mut abi,
                    &sret8_disp8_prologue(0x34),
                )
                .expect("sret prologue should apply");
            }
            assert!(
                super::abi_matches_virtual_method(spec, &abi),
                "{}: {} {:?}",
                target.label(),
                abi.return_type.display(),
                abi.return_passing
            );
        }
    }

    #[test]
    fn actual_virtual_this_adjust_thunks_match_extract_specs() {
        let cases = [
            (
                VirtualMethodTarget::GetRenderTargetView,
                "?GetRenderTargetView@CDeviceTextureTarget@@$4PPPPPPPM@A@EBAPEAUID3D11RenderTargetView@@XZ",
            ),
            (
                VirtualMethodTarget::IsHardwareProtected,
                "?IsHardwareProtected@CDDisplaySwapChain@@$4PPPPPPPM@A@EBA_NXZ",
            ),
        ];

        for (target, decorated) in cases {
            let spec = target.extract_spec();
            let abi = super::parse_decorated_virtual_method_abi(spec, decorated)
                .expect("virtual-this-adjust thunk signature should parse");
            assert!(
                super::abi_matches_virtual_method(spec, &abi),
                "{}: {} {:?}",
                target.label(),
                abi.return_type.display(),
                abi.return_passing
            );
        }
    }

    #[test]
    fn normalizes_every_virtual_this_adjust_function_class() {
        let classes = [
            ('0', 'E'),
            ('1', 'F'),
            ('2', 'M'),
            ('3', 'N'),
            ('4', 'U'),
            ('5', 'V'),
        ];

        for (adjust_class, normal_class) in classes {
            for (extended, adjustments) in [(false, "A@0"), (true, "A@0B@1")] {
                let extended_marker = if extended { "R" } else { "" };
                let decorated = format!(
                    "?GetVidPnTargetId@CDDisplaySwapChain@@${extended_marker}{adjust_class}{adjustments}EBAIXZ"
                );
                let expected =
                    format!("?GetVidPnTargetId@CDDisplaySwapChain@@{normal_class}EBAIXZ");

                assert_eq!(
                    super::normalize_virtual_this_adjust_thunk(&decorated)
                        .expect("valid thunk should normalize")
                        .as_deref(),
                    Some(expected.as_str())
                );
            }
        }
    }

    #[test]
    fn rejects_invalid_virtual_this_adjust_thunks() {
        let invalid = [
            "?GetVidPnTargetId@CDDisplaySwapChain@@$6A@0EBAIXZ",
            "?GetVidPnTargetId@CDDisplaySwapChain@@$4A@EBAIXZ",
            "?GetVidPnTargetId@CDDisplaySwapChain@@$R4A@0B@EBAIXZ",
            "?GetVidPnTargetId@CDDisplaySwapChain@@$4!A@0EBAIXZ",
        ];

        for decorated in invalid {
            assert!(
                super::normalize_virtual_this_adjust_thunk(decorated).is_err(),
                "invalid thunk should fail: {decorated}"
            );
        }
    }

    #[test]
    fn sret8_prologue_detects_disp8_and_disp32_forms() {
        assert!(super::is_msvc_x64_sret8_prologue(&sret8_disp8_prologue(
            0x34
        )));
        assert!(super::is_msvc_x64_sret8_prologue(&sret8_disp32_prologue(
            0x144
        )));
        assert!(!super::is_msvc_x64_sret8_prologue(&[
            0x48, 0x8B, 0x41, 0x34, 0xC3, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC,
            0xCC, 0xCC
        ]));
    }

    #[test]
    fn get_display_adapter_luid_rejects_register_return_passing() {
        let spec = VirtualMethodTarget::GetDisplayAdapterLuid.extract_spec();
        let abi = super::parse_decorated_virtual_method_abi(
            spec,
            "?GetDisplayAdapterLuid@CDDisplaySwapChain@@UEBA?AU_LUID@@XZ",
        )
        .expect("signature should parse");

        assert_eq!(abi.return_passing, super::ReturnPassing::Register);
        assert!(!super::abi_matches_virtual_method(spec, &abi));
    }

    fn sret8_disp8_prologue(disp: u8) -> [u8; 16] {
        let mut bytes = [0xCCu8; 16];
        bytes[0..11].copy_from_slice(&[
            0x48, 0x8B, 0x41, disp, 0x48, 0x89, 0x02, 0x48, 0x8B, 0xC2, 0xC3,
        ]);
        bytes
    }

    fn sret8_disp32_prologue(disp: u32) -> [u8; 16] {
        let mut bytes = [0xCCu8; 16];
        bytes[0..3].copy_from_slice(&[0x48, 0x8B, 0x81]);
        bytes[3..7].copy_from_slice(&disp.to_le_bytes());
        bytes[7..14].copy_from_slice(&[0x48, 0x89, 0x02, 0x48, 0x8B, 0xC2, 0xC3]);
        bytes
    }

    #[test]
    fn virtual_method_abi_rejects_a_changed_return_type() {
        let spec = VirtualMethodTarget::GetVidPnTargetId.extract_spec();
        let abi = super::parse_decorated_virtual_method_abi(
            spec,
            "?GetVidPnTargetId@CDDisplaySwapChain@@UEBAJXZ",
        )
        .expect("changed signature should still parse");

        assert!(!super::abi_matches_virtual_method(spec, &abi));
    }

    #[test]
    fn virtual_method_abi_rejects_an_explicit_argument() {
        let spec = VirtualMethodTarget::GetVidPnTargetId.extract_spec();
        let abi = super::parse_decorated_virtual_method_abi(
            spec,
            "?GetVidPnTargetId@CDDisplaySwapChain@@UEBAII@Z",
        )
        .expect("changed signature should still parse");

        assert_eq!(abi.arguments.len(), 1);
        assert!(!super::abi_matches_virtual_method(spec, &abi));
    }
}
