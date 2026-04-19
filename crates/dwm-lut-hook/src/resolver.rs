use std::ffi::OsStr;
use std::fmt;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

use crate::profile::{AobToken, HookProfile, HookSignature, HookTarget, SignatureLocator};

const IMAGE_DOS_SIGNATURE: u16 = 0x5A4D;
const IMAGE_NT_SIGNATURE: u32 = 0x0000_4550;
const IMAGE_OPTIONAL_HDR32_MAGIC: u16 = 0x010B;
const IMAGE_OPTIONAL_HDR64_MAGIC: u16 = 0x020B;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedModule {
    pub module_name: &'static str,
    pub base_address: usize,
    pub size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub target: HookTarget,
    pub capture_key: &'static str,
    pub address: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureResolutionReport {
    pub module: LoadedModule,
    pub targets: Vec<ResolvedTarget>,
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
    SignatureNotFound {
        target: HookTarget,
        capture_key: &'static str,
    },
    SignatureAmbiguous {
        target: HookTarget,
        capture_key: &'static str,
        matches: usize,
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
            Self::SignatureNotFound {
                target,
                capture_key,
            } => write!(
                f,
                "signature {} ({capture_key}) was not found",
                target.label()
            ),
            Self::SignatureAmbiguous {
                target,
                capture_key,
                matches,
            } => write!(
                f,
                "signature {} ({capture_key}) matched {matches} locations",
                target.label()
            ),
        }
    }
}

impl std::error::Error for HookResolveError {}

pub fn resolve_profile(
    profile: &HookProfile,
) -> Result<SignatureResolutionReport, HookResolveError> {
    let module = load_module(profile.module_name)?;
    let image =
        unsafe { std::slice::from_raw_parts(module.base_address as *const u8, module.size) };
    resolve_profile_from_image(profile, module, image)
}

pub(crate) fn resolve_profile_from_image(
    profile: &HookProfile,
    module: LoadedModule,
    image: &[u8],
) -> Result<SignatureResolutionReport, HookResolveError> {
    let mut targets = Vec::with_capacity(profile.signatures.len());

    for signature in &profile.signatures {
        targets.push(resolve_signature(module, image, signature)?);
    }

    Ok(SignatureResolutionReport { module, targets })
}

fn resolve_signature(
    module: LoadedModule,
    image: &[u8],
    signature: &HookSignature,
) -> Result<ResolvedTarget, HookResolveError> {
    match signature.locator {
        SignatureLocator::Aob {
            module_name,
            capture_key,
            tokens,
        } => {
            debug_assert_eq!(module.module_name, module_name);

            let matches = find_signature_offsets(image, tokens);
            match matches.as_slice() {
                [] => Err(HookResolveError::SignatureNotFound {
                    target: signature.target,
                    capture_key,
                }),
                [offset] => Ok(ResolvedTarget {
                    target: signature.target,
                    capture_key,
                    address: module.base_address + offset,
                }),
                many => Err(HookResolveError::SignatureAmbiguous {
                    target: signature.target,
                    capture_key,
                    matches: many.len(),
                }),
            }
        }
    }
}

fn load_module(module_name: &'static str) -> Result<LoadedModule, HookResolveError> {
    let module_name_wide = wide_null(module_name);
    let handle = unsafe { GetModuleHandleW(module_name_wide.as_ptr()) };
    if handle.is_null() {
        return Err(HookResolveError::ModuleNotLoaded { module_name });
    }

    let base_address = handle as usize;
    let size = module_image_size(base_address as *const u8, module_name)?;

    Ok(LoadedModule {
        module_name,
        base_address,
        size,
    })
}

fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain([0]).collect()
}

fn module_image_size(
    base_address: *const u8,
    module_name: &'static str,
) -> Result<usize, HookResolveError> {
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

    Ok(size_of_image)
}

fn find_signature_offsets(image: &[u8], tokens: &[AobToken]) -> Vec<usize> {
    if tokens.is_empty() || tokens.len() > image.len() {
        return Vec::new();
    }

    let mut matches = Vec::new();

    for offset in 0..=image.len() - tokens.len() {
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
        HookResolveError, LoadedModule, find_signature_offsets, resolve_profile_from_image,
    };
    use crate::profile::{AobToken, BuildProfile, HookProfile};

    #[test]
    fn aob_scan_honors_wildcards() {
        let image = [0x90, 0x40, 0x55, 0xAA, 0x57, 0x90];
        let tokens = [
            AobToken::Exact(0x40),
            AobToken::Exact(0x55),
            AobToken::Wildcard,
            AobToken::Exact(0x57),
        ];

        assert_eq!(find_signature_offsets(&image, &tokens), vec![1]);
    }

    #[test]
    fn resolve_profile_reports_missing_signature_by_target() {
        let profile = HookProfile::for_build(BuildProfile::Windows11_25H2);
        let module = LoadedModule {
            module_name: "dwmcore.dll",
            base_address: 0x1000_0000,
            size: 128,
        };
        let image = [0u8; 128];

        let error = resolve_profile_from_image(&profile, module, &image)
            .expect_err("resolution should fail when no signatures match");

        assert_eq!(
            error,
            HookResolveError::SignatureNotFound {
                target: crate::profile::HookTarget::Present,
                capture_key: "present_25h2",
            }
        );
    }

    #[test]
    fn resolve_profile_rejects_ambiguous_match() {
        let image = [0x83, 0x10, 0x20, 0x83, 0x30, 0x40];
        let module = LoadedModule {
            module_name: "dwmcore.dll",
            base_address: 0x2000_0000,
            size: image.len(),
        };
        let profile = HookProfile {
            build: BuildProfile::Windows11_25H2,
            module_name: "dwmcore.dll",
            signatures: vec![crate::profile::HookSignature {
                target: crate::profile::HookTarget::OverlaysEnabled,
                locator: crate::profile::SignatureLocator::Aob {
                    module_name: "dwmcore.dll",
                    capture_key: "overlay_test",
                    tokens: &[
                        AobToken::Exact(0x83),
                        AobToken::Wildcard,
                        AobToken::Wildcard,
                    ],
                },
                note: "",
            }],
            hypotheses: HookProfile::for_build(BuildProfile::Windows11_25H2).hypotheses,
        };

        let error = resolve_profile_from_image(&profile, module, &image)
            .expect_err("resolution should fail when the pattern is ambiguous");

        assert_eq!(
            error,
            HookResolveError::SignatureAmbiguous {
                target: crate::profile::HookTarget::OverlaysEnabled,
                capture_key: "overlay_test",
                matches: 2,
            }
        );
    }
}
