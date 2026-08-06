use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr;

use dwm_lut_profile::{
    AobToken, DWMCORE_MODULE_NAME, HookProfile, HookTarget, Rva, SignatureScanError,
    SignatureScanReport, SkippedSignature, scan_profile,
};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READONLY,
    SEC_IMAGE_NO_EXECUTE, UnmapViewOfFile,
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
pub struct Va(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedFunctionVa {
    target: HookTarget,
    va: Va,
}

impl ResolvedFunctionVa {
    pub(crate) fn new(target: HookTarget, va: Va) -> Self {
        Self { target, va }
    }

    pub fn target(self) -> HookTarget {
        self.target
    }

    pub fn va(self) -> Va {
        self.va
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureResolutionReport {
    pub module: LoadedModule,
    pub function_targets: Vec<ResolvedFunctionVa>,
    pub skipped: Vec<SkippedSignature>,
}

impl SignatureResolutionReport {
    #[cfg(test)]
    pub(crate) fn synthetic_for_tests(profile: &HookProfile) -> Self {
        let base_address = 0x1800_0000usize;
        let function_targets = profile
            .signatures
            .iter()
            .enumerate()
            .map(|(index, signature)| {
                ResolvedFunctionVa::new(signature.target, Va(base_address + 0x1000 + index * 0x100))
            })
            .collect();

        Self {
            module: LoadedModule {
                module_name: DWMCORE_MODULE_NAME,
                base_address,
                size: 0x20_0000,
            },
            function_targets,
            skipped: Vec::new(),
        }
    }
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
    ConflictingPrologue {
        target: HookTarget,
        rva: usize,
        mismatch_offset: usize,
        expected: u8,
        actual: u8,
    },
    Scan(SignatureScanError),
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
            Self::Scan(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HookResolveError {}

impl From<SignatureScanError> for HookResolveError {
    fn from(value: SignatureScanError) -> Self {
        Self::Scan(value)
    }
}

pub fn resolve_profile(
    profile: &HookProfile,
) -> Result<SignatureResolutionReport, HookResolveError> {
    let module = load_module(DWMCORE_MODULE_NAME)?;
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
    let scan = scan_profile(profile, clean_image)?;
    let resolution = report_with_vas(module, scan)?;
    validate_live_prologues(profile, &resolution, live_image)?;
    Ok(resolution)
}

fn report_with_vas(
    module: LoadedModule,
    scan: SignatureScanReport,
) -> Result<SignatureResolutionReport, HookResolveError> {
    let base = module.base_address;
    let module_name = module.module_name;
    let mut function_targets = Vec::new();

    for hit in scan.resolved {
        let va = va_from_rva(base, hit.rva, module_name)?;
        function_targets.push(ResolvedFunctionVa::new(hit.target, va));
    }

    Ok(SignatureResolutionReport {
        module,
        function_targets,
        skipped: scan.skipped,
    })
}

fn va_from_rva(base: usize, rva: Rva, module_name: &'static str) -> Result<Va, HookResolveError> {
    base.checked_add(rva.0)
        .map(Va)
        .ok_or(HookResolveError::InvalidModuleImage {
            module_name,
            detail: "module base + RVA overflowed",
        })
}

fn validate_live_prologues(
    profile: &HookProfile,
    resolution: &SignatureResolutionReport,
    live_image: &[u8],
) -> Result<(), HookResolveError> {
    for target in &resolution.function_targets {
        let signature = profile
            .signatures
            .iter()
            .find(|signature| signature.target == target.target())
            .ok_or(HookResolveError::InvalidModuleImage {
                module_name: resolution.module.module_name,
                detail: "resolved target had no matching profile signature",
            })?;
        let tokens = signature.aob;
        let rva = target
            .va()
            .0
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
                target: target.target(),
                rva,
                mismatch_offset,
                expected,
                actual,
            });
        }
    }

    Ok(())
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

unsafe fn read_u16(base: *const u8, offset: usize) -> u16 {
    unsafe { (base.add(offset) as *const u16).read_unaligned() }
}

unsafe fn read_u32(base: *const u8, offset: usize) -> u32 {
    unsafe { (base.add(offset) as *const u32).read_unaligned() }
}

#[cfg(test)]
mod tests {
    use super::{
        HookResolveError, LoadedModule, ResolvedFunctionVa, Va, resolve_profile_from_clean_image,
    };
    use dwm_lut_profile::{
        AobToken, ContextToSwapChainPath, DWMCORE_MODULE_NAME, HookProfile, HookSignature,
        HookTarget, MonitorIdentityOffsets, SignatureScanError, SwapChainToResourcePath,
    };

    fn test_profile(signatures: &'static [HookSignature]) -> HookProfile {
        HookProfile {
            signatures,
            swap_chain_to_resource_path: SwapChainToResourcePath {
                container_vtable_index: 0,
                resource_vtable_index: 0,
            },
            hardware_protected_offset: 0,
            monitor_identity_offsets: MonitorIdentityOffsets {
                adapter_luid_low_offset: 0,
                adapter_luid_high_offset: 0,
                target_id_offset: 0,
            },
            context_to_swap_chain_path: ContextToSwapChainPath {
                monitor_target_offset: 0,
                swap_chain_vtable_index: 0,
            },
        }
    }

    fn prologue_test_profile() -> HookProfile {
        test_profile(&[HookSignature {
            target: HookTarget::Present,
            aob: &[
                AobToken::Exact(0x40),
                AobToken::Exact(0x55),
                AobToken::Wildcard,
                AobToken::Exact(0x57),
            ],
        }])
    }

    #[test]
    fn clean_image_resolution_uses_live_address_after_prologue_validation() {
        let clean_image = [0x90, 0x40, 0x55, 0xAA, 0x57, 0x90];
        let live_image = clean_image;
        let module = LoadedModule {
            module_name: DWMCORE_MODULE_NAME,
            base_address: 0x1000_0000,
            size: live_image.len(),
        };
        let profile = prologue_test_profile();

        let report = resolve_profile_from_clean_image(&profile, module, &live_image, &clean_image)
            .expect("matching live prologue should resolve");

        assert_eq!(report.function_targets[0].va(), Va(module.base_address + 1));
    }

    #[test]
    fn clean_image_resolution_reports_modified_live_prologue() {
        let clean_image = [0x90, 0x40, 0x55, 0xAA, 0x57, 0x90];
        let live_image = [0x90, 0xE9, 0x11, 0x22, 0x33, 0x44];
        let module = LoadedModule {
            module_name: DWMCORE_MODULE_NAME,
            base_address: 0x1000_0000,
            size: live_image.len(),
        };
        let profile = prologue_test_profile();

        let error = resolve_profile_from_clean_image(&profile, module, &live_image, &clean_image)
            .expect_err("modified live prologue should fail");

        assert!(matches!(
            error,
            HookResolveError::ConflictingPrologue {
                target: HookTarget::Present,
                rva: 1,
                mismatch_offset: 0,
                expected: 0x40,
                actual: 0xE9,
            }
        ));
    }

    #[test]
    fn resolve_profile_from_clean_image_maps_scan_failures() {
        let image = [0x00];
        let module = LoadedModule {
            module_name: DWMCORE_MODULE_NAME,
            base_address: 0x1000_0000,
            size: image.len(),
        };
        let profile = prologue_test_profile();
        let error = resolve_profile_from_clean_image(&profile, module, &image, &image)
            .expect_err("missing required signature");
        assert!(matches!(
            error,
            HookResolveError::Scan(SignatureScanError::NotFound {
                target: HookTarget::Present
            })
        ));
    }

    #[test]
    fn resolve_profile_from_clean_image_maps_function_targets_to_vas() {
        let image = [0xAA, 0xBB];
        let module = LoadedModule {
            module_name: DWMCORE_MODULE_NAME,
            base_address: 0x2000_0000,
            size: image.len(),
        };
        const SIGNATURES: &[HookSignature] = &[
            HookSignature {
                target: HookTarget::Present,
                aob: &[AobToken::Exact(0xAA)],
            },
            HookSignature {
                target: HookTarget::IsCandidateOverlayCompatible,
                aob: &[AobToken::Exact(0xBB)],
            },
        ];
        let profile = test_profile(SIGNATURES);

        let report = resolve_profile_from_clean_image(&profile, module, &image, &image)
            .expect("resolution should succeed");

        assert_eq!(
            report.function_targets,
            vec![
                ResolvedFunctionVa::new(HookTarget::Present, Va(module.base_address)),
                ResolvedFunctionVa::new(
                    HookTarget::IsCandidateOverlayCompatible,
                    Va(module.base_address + 1),
                ),
            ]
        );
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn resolve_profile_from_clean_image_rejects_base_plus_rva_overflow() {
        let image = [0x90, 0x40, 0x55, 0xAA, 0x57, 0x90];
        let module = LoadedModule {
            module_name: DWMCORE_MODULE_NAME,
            base_address: usize::MAX,
            size: image.len(),
        };
        let profile = prologue_test_profile();

        let error = resolve_profile_from_clean_image(&profile, module, &image, &image)
            .expect_err("base + RVA overflow should fail");

        assert!(matches!(
            error,
            HookResolveError::InvalidModuleImage {
                module_name: DWMCORE_MODULE_NAME,
                detail: "module base + RVA overflowed",
            }
        ));
    }
}
