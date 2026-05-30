use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SetFileSecurityW,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath};

use crate::error::{InjectionStep, InjectorError};

const HOOK_DLL_NAME: &str = "dwm_lut_hook.dll";
const HASH_PREFIX_BYTES: usize = 16;
const SDDL_REVISION_1: u32 = 1;
const DIRECTORY_DACL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;BU)";
const FILE_DACL: &str = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGX;;;BU)";

pub(crate) fn default_hook_dll_path() -> Result<PathBuf, InjectorError> {
    let exe_path = env::current_exe().map_err(|source| InjectorError::StepFailed {
        step: InjectionStep::ResolveDefaultHookDll,
        source,
    })?;
    let exe_dir = exe_path.parent().ok_or_else(|| InjectorError::StepFailed {
        step: InjectionStep::ResolveDefaultHookDll,
        source: io::Error::new(
            io::ErrorKind::InvalidInput,
            "injector executable path has no parent directory",
        ),
    })?;

    Ok(exe_dir.join(HOOK_DLL_NAME))
}

pub(crate) fn stage_hook_dll(input_path: &Path) -> Result<PathBuf, InjectorError> {
    let dll_bytes = fs::read(input_path).map_err(|source| InjectorError::StepFailed {
        step: InjectionStep::ReadLocalHookDll,
        source,
    })?;
    let hash = Sha256::digest(&dll_bytes);
    let staged_dir = staging_directory()?;

    fs::create_dir_all(&staged_dir).map_err(|source| InjectorError::StepFailed {
        step: InjectionStep::CreateStagingDirectory,
        source,
    })?;
    set_path_dacl(
        &staged_dir,
        DIRECTORY_DACL,
        InjectionStep::SecureStagingDirectory,
    )?;

    let staged_path = staged_dir.join(format!(
        "dwm_lut_hook-{}.dll",
        hex_prefix(&hash, HASH_PREFIX_BYTES)
    ));
    cleanup_stale_staged_dlls(&staged_dir, &staged_path);
    if staged_path.is_file() {
        verify_staged_file(&staged_path, &hash)?;
    } else {
        write_staged_file(&staged_path, &dll_bytes)?;
        verify_staged_file(&staged_path, &hash)?;
    }
    set_path_dacl(&staged_path, FILE_DACL, InjectionStep::SecureStagedHookDll)?;

    Ok(staged_path)
}

fn staging_directory() -> Result<PathBuf, InjectorError> {
    Ok(program_data_directory()?.join("dwm-lut-rs").join("hook"))
}

fn program_data_directory() -> Result<PathBuf, InjectorError> {
    let mut path = std::ptr::null_mut();
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, std::ptr::null_mut(), &mut path) };
    if result < 0 {
        return Err(InjectorError::StepFailed {
            step: InjectionStep::ResolveStagingDirectory,
            source: io::Error::from_raw_os_error(result),
        });
    }

    Ok(KnownFolderPath { ptr: path }.to_path_buf())
}

fn write_staged_file(staged_path: &Path, dll_bytes: &[u8]) -> Result<(), InjectorError> {
    let temp_path = staged_path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temp_path, dll_bytes).map_err(|source| InjectorError::StepFailed {
        step: InjectionStep::WriteStagedHookDll,
        source,
    })?;

    match fs::rename(&temp_path, staged_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(&temp_path);
            Ok(())
        }
        Err(source) => Err(InjectorError::StepFailed {
            step: InjectionStep::WriteStagedHookDll,
            source,
        }),
    }
}

fn cleanup_stale_staged_dlls(staged_dir: &Path, current_path: &Path) {
    let Ok(entries) = fs::read_dir(staged_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path == current_path || !is_staged_hook_dll_path(&path) {
            continue;
        }

        let _ = fs::remove_file(path);
    }
}

fn is_staged_hook_dll_path(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };

    let lower = file_name.to_ascii_lowercase();
    let Some(hex) = lower
        .strip_prefix("dwm_lut_hook-")
        .and_then(|value| value.strip_suffix(".dll"))
    else {
        return false;
    };

    hex.len() == HASH_PREFIX_BYTES * 2 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn verify_staged_file(staged_path: &Path, expected_hash: &[u8]) -> Result<(), InjectorError> {
    let staged_bytes = fs::read(staged_path).map_err(|source| InjectorError::StepFailed {
        step: InjectionStep::VerifyStagedHookDll,
        source,
    })?;
    let staged_hash = Sha256::digest(staged_bytes);
    if staged_hash.as_slice() == expected_hash {
        return Ok(());
    }

    Err(InjectorError::StepFailed {
        step: InjectionStep::VerifyStagedHookDll,
        source: io::Error::new(
            io::ErrorKind::InvalidData,
            "staged DLL content does not match its content-addressed name",
        ),
    })
}

fn hex_prefix(hash: &[u8], prefix_bytes: usize) -> String {
    let mut output = String::with_capacity(prefix_bytes * 2);
    for byte in &hash[..prefix_bytes] {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn set_path_dacl(path: &Path, sddl: &str, step: InjectionStep) -> Result<(), InjectorError> {
    let path_wide = wide_null(path.as_os_str());
    let sddl_wide = wide_null(OsStr::new(sddl));
    let security_descriptor = SecurityDescriptor::from_sddl(&sddl_wide, step)?;

    let ok = unsafe {
        SetFileSecurityW(
            path_wide.as_ptr(),
            DACL_SECURITY_INFORMATION,
            security_descriptor.as_ptr(),
        )
    };
    if ok != 0 {
        return Ok(());
    }

    Err(InjectorError::StepFailed {
        step,
        source: io::Error::from_raw_os_error(unsafe { GetLastError() } as i32),
    })
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

struct SecurityDescriptor {
    ptr: PSECURITY_DESCRIPTOR,
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

impl SecurityDescriptor {
    fn from_sddl(sddl: &[u16], step: InjectionStep) -> Result<Self, InjectorError> {
        let mut ptr = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut ptr,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 {
            return Ok(Self { ptr });
        }

        Err(InjectorError::StepFailed {
            step,
            source: io::Error::from_raw_os_error(unsafe { GetLastError() } as i32),
        })
    }

    fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.ptr
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                LocalFree(self.ptr);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use sha2::{Digest, Sha256};

    use super::{
        HASH_PREFIX_BYTES, cleanup_stale_staged_dlls, hex_prefix, is_staged_hook_dll_path,
    };

    #[test]
    fn hash_prefix_uses_128_bits() {
        let hash = Sha256::digest(b"hook dll bytes");

        assert_eq!(hex_prefix(&hash, HASH_PREFIX_BYTES).len(), 32);
    }

    #[test]
    fn staged_hook_dll_cleanup_matches_only_content_addressed_hook_dlls() {
        assert!(is_staged_hook_dll_path(Path::new(
            r"C:\ProgramData\dwm-lut-rs\hook\dwm_lut_hook-0123456789abcdef0123456789abcdef.dll"
        )));
        assert!(is_staged_hook_dll_path(Path::new(
            r"C:\ProgramData\dwm-lut-rs\hook\DWM_LUT_HOOK-0123456789ABCDEF0123456789ABCDEF.DLL"
        )));
        assert!(!is_staged_hook_dll_path(Path::new(
            r"C:\ProgramData\dwm-lut-rs\hook\dwm_lut_hook.dll"
        )));
        assert!(!is_staged_hook_dll_path(Path::new(
            r"C:\ProgramData\dwm-lut-rs\hook\other-0123456789abcdef0123456789abcdef.dll"
        )));
        assert!(!is_staged_hook_dll_path(Path::new(
            r"C:\ProgramData\dwm-lut-rs\hook\dwm_lut_hook-0123456789abcdef.dll"
        )));
        assert!(!is_staged_hook_dll_path(Path::new(
            r"C:\ProgramData\dwm-lut-rs\hook\dwm_lut_hook-0123456789abcdef0123456789abcdeg.dll"
        )));
        assert!(!is_staged_hook_dll_path(Path::new(
            r"C:\ProgramData\dwm-lut-rs\hook\dwm_lut_hook-0123456789abcdef0123456789abcdef-extra.dll"
        )));
    }

    #[test]
    fn cleanup_removes_stale_staged_hook_dlls_only() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("dwm-lut-staging-test-{unique}"));
        fs::create_dir(&dir).expect("test staging directory should be created");

        let current = dir.join("dwm_lut_hook-11111111111111111111111111111111.dll");
        let stale = dir.join("dwm_lut_hook-22222222222222222222222222222222.dll");
        let unrelated = dir.join("dwm_lut_hook.dll");
        let prefix_only = dir.join("dwm_lut_hook-not-a-content-address.dll");
        fs::write(&current, b"current").expect("current staged DLL should be written");
        fs::write(&stale, b"stale").expect("stale staged DLL should be written");
        fs::write(&unrelated, b"unrelated").expect("unrelated DLL should be written");
        fs::write(&prefix_only, b"unrelated").expect("prefix-only DLL should be written");

        cleanup_stale_staged_dlls(&dir, &current);

        assert!(current.exists());
        assert!(!stale.exists());
        assert!(unrelated.exists());
        assert!(prefix_only.exists());

        let _ = fs::remove_file(current);
        let _ = fs::remove_file(unrelated);
        let _ = fs::remove_file(prefix_only);
        let _ = fs::remove_dir(dir);
    }
}
