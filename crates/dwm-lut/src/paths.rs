use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath};

use crate::error::{InjectionStep, InjectorError};

pub(crate) fn local_app_data_directory(step: InjectionStep) -> Result<PathBuf, InjectorError> {
    let mut path = std::ptr::null_mut();
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, std::ptr::null_mut(), &mut path) };
    if result < 0 {
        return Err(InjectorError::StepFailed {
            step,
            source: io::Error::from_raw_os_error(result),
        });
    }

    Ok(KnownFolderPath { ptr: path }.to_path_buf())
}

struct KnownFolderPath {
    ptr: *mut u16,
}

impl KnownFolderPath {
    fn to_path_buf(&self) -> PathBuf {
        let mut len = 0usize;
        while unsafe { *self.ptr.add(len) } != 0 {
            len += 1;
        }

        let units = unsafe { std::slice::from_raw_parts(self.ptr, len) };
        PathBuf::from(OsString::from_wide(units))
    }
}

impl Drop for KnownFolderPath {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                CoTaskMemFree(self.ptr.cast());
            }
        }
    }
}
