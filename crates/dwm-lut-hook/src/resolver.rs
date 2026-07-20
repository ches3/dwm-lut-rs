use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READONLY,
    SEC_IMAGE_NO_EXECUTE, UnmapViewOfFile,
};

use crate::profile::{
    AobToken, HOOK_MODULE_NAME, HookProfile, HookSignature, HookTarget, SignatureLocator,
};

const IMAGE_DOS_SIGNATURE: u16 = 0x5A4D;
const IMAGE_NT_SIGNATURE: u32 = 0x0000_4550;
const IMAGE_OPTIONAL_HDR32_MAGIC: u16 = 0x010B;
const IMAGE_OPTIONAL_HDR64_MAGIC: u16 = 0x020B;
const MAX_MODULE_PATH_CHARS: usize = 32_768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ImageIdentity {
    timestamp: u32,
    size: usize,
}

struct MappedImage {
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    size: usize,
}

impl MappedImage {
    fn open(path: &Path, module_name: &'static str) -> Result<Self, HookResolveError> {
        let file = File::open(path).map_err(|error| HookResolveError::ModuleAccessFailed {
            module_name,
            operation: "open backing file",
            error_code: error.raw_os_error().unwrap_or_default(),
        })?;
        let mapping = unsafe {
            CreateFileMappingW(
                file.as_raw_handle() as HANDLE,
                ptr::null(),
                PAGE_READONLY | SEC_IMAGE_NO_EXECUTE,
                0,
                0,
                ptr::null(),
            )
        };
        if mapping.is_null() {
            return Err(last_module_access_error(
                module_name,
                "create image mapping",
            ));
        }

        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0) };
        if view.Value.is_null() {
            let error = last_module_access_error(module_name, "map image view");
            unsafe {
                CloseHandle(mapping);
            }
            return Err(error);
        }

        let identity = match image_identity(view.Value.cast(), module_name) {
            Ok(identity) => identity,
            Err(error) => {
                unsafe {
                    UnmapViewOfFile(view);
                    CloseHandle(mapping);
                }
                return Err(error);
            }
        };

        Ok(Self {
            mapping,
            view,
            size: identity.size,
        })
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.view.Value.cast(), self.size) }
    }
}

impl Drop for MappedImage {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
            CloseHandle(self.mapping);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedModule {
    pub module_name: &'static str,
    pub base_address: usize,
    pub size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub target: HookTarget,
    pub address: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkippedSignatureReason {
    NotFound,
    Ambiguous { matches: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkippedSignature {
    pub target: HookTarget,
    pub reason: SkippedSignatureReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureResolutionReport {
    pub module: LoadedModule,
    pub targets: Vec<ResolvedTarget>,
    pub skipped_signatures: Vec<SkippedSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookResolveError {
    ModuleNotLoaded {
        module_name: &'static str,
    },
    InvalidModuleImage {
        module_name: &'static str,
        detail: &'static str,
    },
    ModuleAccessFailed {
        module_name: &'static str,
        operation: &'static str,
        error_code: i32,
    },
    ModuleImageMismatch {
        module_name: &'static str,
        live_timestamp: u32,
        backing_timestamp: u32,
        live_size: usize,
        backing_size: usize,
    },
    SignatureNotFound {
        target: HookTarget,
    },
    SignatureAmbiguous {
        target: HookTarget,
        matches: usize,
    },
    ConflictingPrologue {
        target: HookTarget,
        rva: usize,
        mismatch_offset: usize,
        expected: u8,
        actual: u8,
    },
}

impl fmt::Display for HookResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModuleNotLoaded { module_name } => {
                write!(f, "module {module_name} is not loaded")
            }
            Self::InvalidModuleImage {
                module_name,
                detail,
            } => write!(f, "module {module_name} is not a valid PE image: {detail}"),
            Self::ModuleAccessFailed {
                module_name,
                operation,
                error_code,
            } => write!(
                f,
                "module {module_name} backing image {operation} failed with OS error {error_code}"
            ),
            Self::ModuleImageMismatch {
                module_name,
                live_timestamp,
                backing_timestamp,
                live_size,
                backing_size,
            } => write!(
                f,
                "module {module_name} live image does not match its backing file: timestamp {live_timestamp:#x}/{backing_timestamp:#x}, size {live_size:#x}/{backing_size:#x}"
            ),
            Self::SignatureNotFound { target } => {
                write!(f, "signature {} was not found", target.label())
            }
            Self::SignatureAmbiguous { target, matches } => write!(
                f,
                "signature {} matched {matches} locations",
                target.label()
            ),
            Self::ConflictingPrologue {
                target,
                rva,
                mismatch_offset,
                expected,
                actual,
            } => write!(
                f,
                "conflicting modification at {} prologue RVA {rva:#x}+{mismatch_offset:#x}: expected {expected:#04x}, found {actual:#04x}",
                target.label()
            ),
        }
    }
}

impl std::error::Error for HookResolveError {}

pub fn resolve_profile(
    profile: &HookProfile,
) -> Result<SignatureResolutionReport, HookResolveError> {
    let module = load_module(HOOK_MODULE_NAME)?;
    let live_image =
        unsafe { std::slice::from_raw_parts(module.base_address as *const u8, module.size) };
    let live_identity = image_identity(module.base_address as *const u8, module.module_name)?;
    let module_path = module_path(module)?;
    let backing_image = MappedImage::open(&module_path, module.module_name)?;
    let backing_identity = image_identity(backing_image.view.Value.cast(), module.module_name)?;

    if live_identity != backing_identity {
        return Err(HookResolveError::ModuleImageMismatch {
            module_name: module.module_name,
            live_timestamp: live_identity.timestamp,
            backing_timestamp: backing_identity.timestamp,
            live_size: live_identity.size,
            backing_size: backing_identity.size,
        });
    }

    resolve_profile_from_clean_image(profile, module, live_image, backing_image.as_slice())
}

fn resolve_profile_from_clean_image(
    profile: &HookProfile,
    module: LoadedModule,
    live_image: &[u8],
    clean_image: &[u8],
) -> Result<SignatureResolutionReport, HookResolveError> {
    let resolution = resolve_profile_from_image(profile, module, clean_image)?;
    validate_live_prologues(profile, &resolution, live_image)?;
    Ok(resolution)
}

pub(crate) fn resolve_profile_from_image(
    profile: &HookProfile,
    module: LoadedModule,
    image: &[u8],
) -> Result<SignatureResolutionReport, HookResolveError> {
    let mut targets = Vec::with_capacity(profile.signatures.len());
    let mut skipped_signatures = Vec::new();

    for signature in profile.signatures {
        match resolve_signature(module, image, signature) {
            Ok(target) => targets.push(target),
            Err(error) if !signature.target.is_required_signature() => {
                skipped_signatures.push(skipped_signature_from_error(error)?);
            }
            Err(error) => return Err(error),
        }
    }

    Ok(SignatureResolutionReport {
        module,
        targets,
        skipped_signatures,
    })
}

fn skipped_signature_from_error(
    error: HookResolveError,
) -> Result<SkippedSignature, HookResolveError> {
    match error {
        HookResolveError::SignatureNotFound { target } => Ok(SkippedSignature {
            target,
            reason: SkippedSignatureReason::NotFound,
        }),
        HookResolveError::SignatureAmbiguous { target, matches } => Ok(SkippedSignature {
            target,
            reason: SkippedSignatureReason::Ambiguous { matches },
        }),
        error => Err(error),
    }
}

fn validate_live_prologues(
    profile: &HookProfile,
    resolution: &SignatureResolutionReport,
    live_image: &[u8],
) -> Result<(), HookResolveError> {
    for target in resolution
        .targets
        .iter()
        .filter(|target| target.target.is_function_hook_target())
    {
        let signature = profile
            .signatures
            .iter()
            .find(|signature| signature.target == target.target)
            .ok_or(HookResolveError::InvalidModuleImage {
                module_name: resolution.module.module_name,
                detail: "resolved target had no matching profile signature",
            })?;
        let SignatureLocator::Aob { tokens, .. } = signature.locator else {
            return Err(HookResolveError::InvalidModuleImage {
                module_name: resolution.module.module_name,
                detail: "function hook target did not use an AOB locator",
            });
        };
        let rva = target
            .address
            .checked_sub(resolution.module.base_address)
            .ok_or(HookResolveError::InvalidModuleImage {
                module_name: resolution.module.module_name,
                detail: "resolved target address was below the live module base",
            })?;
        let prologue = live_image
            .get(rva..rva.saturating_add(tokens.len()))
            .ok_or(HookResolveError::InvalidModuleImage {
                module_name: resolution.module.module_name,
                detail: "resolved target prologue was outside the live image",
            })?;

        if let Some((mismatch_offset, expected, actual)) =
            tokens.iter().zip(prologue).enumerate().find_map(
                |(offset, (token, actual))| match token {
                    AobToken::Exact(expected) if expected != actual => {
                        Some((offset, *expected, *actual))
                    }
                    _ => None,
                },
            )
        {
            return Err(HookResolveError::ConflictingPrologue {
                target: target.target,
                rva,
                mismatch_offset,
                expected,
                actual,
            });
        }
    }

    Ok(())
}

fn resolve_signature(
    module: LoadedModule,
    image: &[u8],
    signature: &HookSignature,
) -> Result<ResolvedTarget, HookResolveError> {
    match signature.locator {
        SignatureLocator::Aob { tokens, .. } => resolve_unique_match(
            module,
            signature.target,
            find_signature_offsets(image, tokens),
        ),
        SignatureLocator::RipRelativeGlobalAob {
            tokens,
            displacement_offset,
            instruction_size,
            ..
        } => match find_signature_offsets(image, tokens).as_slice() {
            [] => Err(HookResolveError::SignatureNotFound {
                target: signature.target,
            }),
            [offset] => {
                let displacement =
                    read_i32_from_image(image, *offset + displacement_offset, module.module_name)?
                        as isize;
                let base = module.base_address + offset + instruction_size;
                let address = (base as isize + displacement) as usize;

                Ok(ResolvedTarget {
                    target: signature.target,
                    address,
                })
            }
            many => Err(HookResolveError::SignatureAmbiguous {
                target: signature.target,
                matches: many.len(),
            }),
        },
    }
}

fn resolve_unique_match(
    module: LoadedModule,
    target: HookTarget,
    matches: Vec<usize>,
) -> Result<ResolvedTarget, HookResolveError> {
    match matches.as_slice() {
        [] => Err(HookResolveError::SignatureNotFound { target }),
        [offset] => Ok(ResolvedTarget {
            target,
            address: module.base_address + offset,
        }),
        many => Err(HookResolveError::SignatureAmbiguous {
            target,
            matches: many.len(),
        }),
    }
}

fn load_module(module_name: &'static str) -> Result<LoadedModule, HookResolveError> {
    let module_name_wide = wide_null(module_name);
    let handle = unsafe { GetModuleHandleW(module_name_wide.as_ptr()) };
    if handle.is_null() {
        return Err(HookResolveError::ModuleNotLoaded { module_name });
    }

    let base_address = handle as usize;
    let size = image_identity(base_address as *const u8, module_name)?.size;

    Ok(LoadedModule {
        module_name,
        base_address,
        size,
    })
}

fn module_path(module: LoadedModule) -> Result<PathBuf, HookResolveError> {
    let mut buffer = vec![0u16; MAX_MODULE_PATH_CHARS];
    let len = unsafe {
        GetModuleFileNameW(
            module.base_address as _,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if len == 0 {
        return Err(last_module_access_error(
            module.module_name,
            "path resolution",
        ));
    }
    if len == buffer.len() {
        return Err(HookResolveError::ModuleAccessFailed {
            module_name: module.module_name,
            operation: "path resolution",
            error_code: 122,
        });
    }

    buffer.truncate(len);
    Ok(PathBuf::from(OsString::from_wide(&buffer)))
}

fn last_module_access_error(
    module_name: &'static str,
    operation: &'static str,
) -> HookResolveError {
    HookResolveError::ModuleAccessFailed {
        module_name,
        operation,
        error_code: std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or_default(),
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain([0]).collect()
}

fn image_identity(
    base_address: *const u8,
    module_name: &'static str,
) -> Result<ImageIdentity, HookResolveError> {
    if base_address.is_null() {
        return Err(HookResolveError::InvalidModuleImage {
            module_name,
            detail: "null base address",
        });
    }

    let dos_signature = unsafe { read_u16(base_address, 0) };
    if dos_signature != IMAGE_DOS_SIGNATURE {
        return Err(HookResolveError::InvalidModuleImage {
            module_name,
            detail: "missing MZ signature",
        });
    }

    let pe_offset = unsafe { read_u32(base_address, 0x3C) as usize };
    let nt_signature = unsafe { read_u32(base_address, pe_offset) };
    if nt_signature != IMAGE_NT_SIGNATURE {
        return Err(HookResolveError::InvalidModuleImage {
            module_name,
            detail: "missing PE signature",
        });
    }

    let timestamp = unsafe { read_u32(base_address, pe_offset + 0x08) };

    let optional_header_offset = pe_offset + 0x18;
    let optional_magic = unsafe { read_u16(base_address, optional_header_offset) };
    if optional_magic != IMAGE_OPTIONAL_HDR32_MAGIC && optional_magic != IMAGE_OPTIONAL_HDR64_MAGIC
    {
        return Err(HookResolveError::InvalidModuleImage {
            module_name,
            detail: "unexpected optional header magic",
        });
    }

    let size_of_image = unsafe { read_u32(base_address, optional_header_offset + 0x38) as usize };
    if size_of_image == 0 {
        return Err(HookResolveError::InvalidModuleImage {
            module_name,
            detail: "SizeOfImage was zero",
        });
    }

    Ok(ImageIdentity {
        timestamp,
        size: size_of_image,
    })
}

fn find_signature_offsets(image: &[u8], tokens: &[AobToken]) -> Vec<usize> {
    if tokens.is_empty() || tokens.len() > image.len() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let scan_limit = image.len() - tokens.len();

    for offset in 0..=scan_limit {
        if tokens
            .iter()
            .zip(&image[offset..offset + tokens.len()])
            .all(|(token, byte)| matches_token(*token, *byte))
        {
            matches.push(offset);
        }
    }

    matches
}

fn read_i32_from_image(
    image: &[u8],
    offset: usize,
    module_name: &'static str,
) -> Result<i32, HookResolveError> {
    let bytes = image
        .get(offset..offset + 4)
        .ok_or(HookResolveError::InvalidModuleImage {
            module_name,
            detail: "RIP-relative displacement was out of image bounds",
        })?;
    Ok(i32::from_le_bytes(
        bytes.try_into().expect("slice length is fixed to 4"),
    ))
}

const fn matches_token(token: AobToken, byte: u8) -> bool {
    match token {
        AobToken::Exact(expected) => expected == byte,
        AobToken::Wildcard => true,
    }
}

unsafe fn read_u16(base: *const u8, offset: usize) -> u16 {
    unsafe { (base.add(offset) as *const u16).read_unaligned() }
}

unsafe fn read_u32(base: *const u8, offset: usize) -> u32 {
    unsafe { (base.add(offset) as *const u32).read_unaligned() }
}

#[cfg(test)]
mod tests {
    use super::{
        HookResolveError, LoadedModule, find_signature_offsets, resolve_profile_from_clean_image,
        resolve_profile_from_image,
    };
    use crate::profile::{
        AobToken, HOOK_MODULE_NAME, HookProfile, HookSignature, HookTarget, SignatureLocator,
        VERSIONED_PROFILES,
    };

    fn test_profile() -> HookProfile {
        (VERSIONED_PROFILES[0].build)()
    }

    #[test]
    fn aob_scan_honors_wildcards() {
        let tokens = [
            AobToken::Exact(0x40),
            AobToken::Exact(0x55),
            AobToken::Wildcard,
            AobToken::Exact(0x57),
        ];
        let image = [0x90, 0x40, 0x55, 0xAA, 0x57, 0x90];

        assert_eq!(find_signature_offsets(&image, &tokens), vec![1]);
    }

    #[test]
    fn clean_image_resolution_uses_live_address_after_prologue_validation() {
        let clean_image = [0x90, 0x40, 0x55, 0xAA, 0x57, 0x90];
        let live_image = clean_image;
        let module = LoadedModule {
            module_name: HOOK_MODULE_NAME,
            base_address: 0x1000_0000,
            size: live_image.len(),
        };
        let profile = prologue_test_profile();

        let report = resolve_profile_from_clean_image(&profile, module, &live_image, &clean_image)
            .expect("matching live prologue should resolve");

        assert_eq!(report.targets[0].address, module.base_address + 1);
    }

    #[test]
    fn clean_image_resolution_reports_modified_live_prologue() {
        let clean_image = [0x90, 0x40, 0x55, 0xAA, 0x57, 0x90];
        let live_image = [0x90, 0xE9, 0x11, 0x22, 0x33, 0x44];
        let module = LoadedModule {
            module_name: HOOK_MODULE_NAME,
            base_address: 0x1000_0000,
            size: live_image.len(),
        };
        let profile = prologue_test_profile();

        let error = resolve_profile_from_clean_image(&profile, module, &live_image, &clean_image)
            .expect_err("modified live prologue must be rejected");

        assert_eq!(
            error,
            HookResolveError::ConflictingPrologue {
                target: HookTarget::Present,
                rva: 1,
                mismatch_offset: 0,
                expected: 0x40,
                actual: 0xE9,
            }
        );
    }

    const PROLOGUE_TEST_SIGNATURES: &[HookSignature] = &[HookSignature {
        target: HookTarget::Present,
        locator: SignatureLocator::Aob {
            tokens: &[
                AobToken::Exact(0x40),
                AobToken::Exact(0x55),
                AobToken::Wildcard,
                AobToken::Exact(0x57),
            ],
        },
    }];

    fn prologue_test_profile() -> HookProfile {
        HookProfile {
            signatures: PROLOGUE_TEST_SIGNATURES,
            hypotheses: test_profile().hypotheses,
        }
    }

    #[test]
    fn resolve_profile_reports_missing_signature_by_target() {
        let profile = test_profile();
        let image = [0u8; 16];
        let module = LoadedModule {
            module_name: HOOK_MODULE_NAME,
            base_address: 0x1000_0000,
            size: image.len(),
        };

        let error = resolve_profile_from_image(&profile, module, &image)
            .expect_err("resolution should fail when no signatures match");

        assert_eq!(
            error,
            HookResolveError::SignatureNotFound {
                target: crate::profile::HookTarget::Present,
            }
        );
    }

    #[test]
    fn resolve_profile_records_missing_optional_signature() {
        let image = [0xAA, 0xBB, 0xCC];
        let module = LoadedModule {
            module_name: HOOK_MODULE_NAME,
            base_address: 0x2000_0000,
            size: image.len(),
        };
        const SIGNATURES: &[HookSignature] = &[
            HookSignature {
                target: HookTarget::Present,
                locator: SignatureLocator::Aob {
                    tokens: &[AobToken::Exact(0xAA)],
                },
            },
            HookSignature {
                target: HookTarget::WindowContextIsCandidateDirectFlipCompatible,
                locator: SignatureLocator::Aob {
                    tokens: &[AobToken::Exact(0xDD)],
                },
            },
        ];
        let profile = HookProfile {
            signatures: SIGNATURES,
            hypotheses: test_profile().hypotheses,
        };

        let report =
            resolve_profile_from_image(&profile, module, &image).expect("optional miss is allowed");

        assert_eq!(report.targets.len(), 1);
        assert_eq!(
            report.skipped_signatures,
            vec![crate::resolver::SkippedSignature {
                target: crate::profile::HookTarget::WindowContextIsCandidateDirectFlipCompatible,
                reason: crate::resolver::SkippedSignatureReason::NotFound,
            }]
        );
    }

    #[test]
    fn resolve_profile_records_ambiguous_optional_signature() {
        let image = [0xAA, 0xDD, 0xDD];
        let module = LoadedModule {
            module_name: HOOK_MODULE_NAME,
            base_address: 0x2000_0000,
            size: image.len(),
        };
        const SIGNATURES: &[HookSignature] = &[
            HookSignature {
                target: HookTarget::Present,
                locator: SignatureLocator::Aob {
                    tokens: &[AobToken::Exact(0xAA)],
                },
            },
            HookSignature {
                target: HookTarget::CompVisualIsCandidateForPromotion,
                locator: SignatureLocator::Aob {
                    tokens: &[AobToken::Exact(0xDD)],
                },
            },
        ];
        let profile = HookProfile {
            signatures: SIGNATURES,
            hypotheses: test_profile().hypotheses,
        };

        let report = resolve_profile_from_image(&profile, module, &image)
            .expect("optional ambiguity is allowed");

        assert_eq!(report.targets.len(), 1);
        assert_eq!(
            report.skipped_signatures,
            vec![crate::resolver::SkippedSignature {
                target: crate::profile::HookTarget::CompVisualIsCandidateForPromotion,
                reason: crate::resolver::SkippedSignatureReason::Ambiguous { matches: 2 },
            }]
        );
    }

    #[test]
    fn resolve_profile_rejects_ambiguous_match() {
        let image = [0x83, 0x10, 0x20, 0x83, 0x30, 0x40];
        let module = LoadedModule {
            module_name: HOOK_MODULE_NAME,
            base_address: 0x2000_0000,
            size: image.len(),
        };
        const SIGNATURES: &[HookSignature] = &[HookSignature {
            target: HookTarget::OverlayTestMode,
            locator: SignatureLocator::Aob {
                tokens: &[
                    AobToken::Exact(0x83),
                    AobToken::Wildcard,
                    AobToken::Wildcard,
                ],
            },
        }];
        let profile = HookProfile {
            signatures: SIGNATURES,
            hypotheses: test_profile().hypotheses,
        };

        let error = resolve_profile_from_image(&profile, module, &image)
            .expect_err("resolution should fail when the pattern is ambiguous");

        assert_eq!(
            error,
            HookResolveError::SignatureAmbiguous {
                target: crate::profile::HookTarget::OverlayTestMode,
                matches: 2,
            }
        );
    }
}
