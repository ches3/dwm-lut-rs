use std::ffi::{OsString, c_void};
use std::fmt;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use dwm_lut_profile::{DWMCORE_MODULE_NAME, DwmcoreVersion};
use windows_sys::Win32::Foundation::MAX_PATH;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DwmcoreVersionError {
    ModuleNotLoaded,
    QueryFailed,
}

impl fmt::Display for DwmcoreVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModuleNotLoaded => write!(f, "dwmcore.dll was not loaded in the target"),
            Self::QueryFailed => write!(f, "failed to query dwmcore.dll FileVersion"),
        }
    }
}

impl std::error::Error for DwmcoreVersionError {}

#[repr(C)]
struct VsFixedFileInfo {
    signature: u32,
    struct_version: u32,
    file_version_ms: u32,
    file_version_ls: u32,
    product_version_ms: u32,
    product_version_ls: u32,
    file_flags_mask: u32,
    file_flags: u32,
    file_os: u32,
    file_type: u32,
    file_subtype: u32,
    file_date_ms: u32,
    file_date_ls: u32,
}

#[link(name = "version")]
unsafe extern "system" {
    fn GetFileVersionInfoSizeW(filename: *const u16, handle: *mut u32) -> u32;
    fn GetFileVersionInfoW(filename: *const u16, handle: u32, len: u32, data: *mut c_void) -> i32;
    fn VerQueryValueW(
        block: *const c_void,
        sub_block: *const u16,
        buffer: *mut *mut c_void,
        len: *mut u32,
    ) -> i32;
}

pub fn dwmcore_file_version() -> Result<DwmcoreVersion, DwmcoreVersionError> {
    let module_name = wide_null(DWMCORE_MODULE_NAME);
    let handle = unsafe { GetModuleHandleW(module_name.as_ptr()) };
    if handle.is_null() {
        return Err(DwmcoreVersionError::ModuleNotLoaded);
    }

    let mut path = vec![0u16; MAX_PATH as usize];
    let len = unsafe { GetModuleFileNameW(handle, path.as_mut_ptr(), path.len() as u32) } as usize;
    if len == 0 || len >= path.len() {
        return Err(DwmcoreVersionError::QueryFailed);
    }
    path.truncate(len);
    file_version_from_path(&PathBuf::from(OsString::from_wide(&path)))
}

fn file_version_from_path(path: &std::path::Path) -> Result<DwmcoreVersion, DwmcoreVersionError> {
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut handle = 0u32;
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), &mut handle) };
    if size == 0 {
        return Err(DwmcoreVersionError::QueryFailed);
    }

    let mut buffer = vec![0u8; size as usize];
    let ok = unsafe {
        GetFileVersionInfoW(wide.as_ptr(), 0, size, buffer.as_mut_ptr().cast::<c_void>())
    };
    if ok == 0 {
        return Err(DwmcoreVersionError::QueryFailed);
    }

    let sub_block = wide_null("\\");
    let mut value: *mut c_void = std::ptr::null_mut();
    let mut value_len = 0u32;
    let ok = unsafe {
        VerQueryValueW(
            buffer.as_ptr().cast::<c_void>(),
            sub_block.as_ptr(),
            &mut value,
            &mut value_len,
        )
    };
    if ok == 0 || value.is_null() || (value_len as usize) < std::mem::size_of::<VsFixedFileInfo>() {
        return Err(DwmcoreVersionError::QueryFailed);
    }

    let info = unsafe { &*value.cast::<VsFixedFileInfo>() };
    if info.signature != 0xFEEF_04BD {
        return Err(DwmcoreVersionError::QueryFailed);
    }

    Ok(DwmcoreVersion {
        build: info.file_version_ls >> 16,
        revision: info.file_version_ls & 0xFFFF,
    })
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
