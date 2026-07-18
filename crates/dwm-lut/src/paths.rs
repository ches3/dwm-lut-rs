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

pub(crate) fn default_config_path() -> Result<PathBuf, InjectorError> {
    Ok(local_app_data_directory(InjectionStep::ResolveConfigPath)?
        .join("dwm-lut-rs")
        .join("config.json"))
}

pub(crate) fn absolute_path(path: PathBuf) -> Result<PathBuf, InjectorError> {
    if path.is_absolute() {
        return Ok(path);
    }

    let cwd = std::env::current_dir().map_err(|source| InjectorError::ControlPipe {
        operation: "resolve current directory",
        source,
    })?;
    Ok(cwd.join(path))
}

pub(crate) fn resolve_config_path(config_path: Option<PathBuf>) -> Result<PathBuf, InjectorError> {
    match config_path {
        Some(config_path) => absolute_path(config_path),
        None => default_config_path(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_config_path_uses_current_directory_for_relative_path() {
        let resolved = resolve_config_path(Some(PathBuf::from("config.json")))
            .expect("relative config path should resolve");

        assert_eq!(
            resolved,
            std::env::current_dir().unwrap().join("config.json")
        );
    }

    #[test]
    fn resolve_config_path_uses_default_config() {
        let resolved = resolve_config_path(None).expect("default config path should resolve");

        assert_eq!(resolved, default_config_path().unwrap());
    }
}
