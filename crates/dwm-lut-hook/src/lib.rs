use std::ffi::OsString;
use std::fmt;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use dwm_lut_config::LutManifest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildProfile {
    Windows11_25H2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookConfig {
    pub manifest_path: PathBuf,
    pub profile: BuildProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookState {
    pub manifest: LutManifest,
    pub config: HookConfig,
}

#[derive(Debug)]
pub enum HookError {
    AlreadyInitialized,
    InvalidPath,
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => write!(f, "hook is already initialized"),
            Self::InvalidPath => write!(f, "manifest path must not be empty"),
        }
    }
}

impl std::error::Error for HookError {}

static STATE: OnceLock<Mutex<HookState>> = OnceLock::new();

#[repr(u32)]
enum InitializeStatus {
    Success = 0,
    NullManifestPath = 1,
    InvalidManifestPath = 2,
    AlreadyInitialized = 3,
}

pub fn initialize(config: HookConfig, manifest: LutManifest) -> Result<(), HookError> {
    if config.manifest_path.as_os_str().is_empty() {
        return Err(HookError::InvalidPath);
    }

    let state = HookState { manifest, config };
    STATE
        .set(Mutex::new(state))
        .map_err(|_| HookError::AlreadyInitialized)
}

pub fn is_initialized() -> bool {
    STATE.get().is_some()
}

pub fn manifest_path() -> Option<PathBuf> {
    let state = STATE.get()?;
    let guard = state.lock().ok()?;
    Some(guard.config.manifest_path.clone())
}

pub fn build_profile() -> BuildProfile {
    BuildProfile::Windows11_25H2
}

/// # Safety
///
/// `manifest_path` must be null or point to a readable, NUL-terminated UTF-16
/// string in the address space of the current process.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn dwm_lut_initialize(manifest_path: *const u16) -> u32 {
    let manifest_path = match unsafe { wide_path_from_ptr(manifest_path) } {
        Some(path) => path,
        None => return InitializeStatus::NullManifestPath as u32,
    };

    let config = HookConfig {
        manifest_path,
        profile: build_profile(),
    };

    match initialize(config, LutManifest::empty()) {
        Ok(()) => InitializeStatus::Success as u32,
        Err(HookError::InvalidPath) => InitializeStatus::InvalidManifestPath as u32,
        Err(HookError::AlreadyInitialized) => InitializeStatus::AlreadyInitialized as u32,
    }
}

pub fn _manifest_path_ref(path: &Path) -> &Path {
    path
}

unsafe fn wide_path_from_ptr(ptr: *const u16) -> Option<PathBuf> {
    if ptr.is_null() {
        return None;
    }

    let mut len = 0usize;
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }

    if len == 0 {
        return Some(PathBuf::new());
    }

    let units = unsafe { std::slice::from_raw_parts(ptr, len) };
    Some(PathBuf::from(OsString::from_wide(units)))
}

#[cfg(test)]
mod tests {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::ptr;

    use super::{dwm_lut_initialize, is_initialized, manifest_path};

    #[test]
    fn ffi_initialize_returns_documented_status_codes() {
        let null_status = unsafe { dwm_lut_initialize(ptr::null()) };
        assert_eq!(null_status, 1);

        let empty_path = [0u16];
        let empty_status = unsafe { dwm_lut_initialize(empty_path.as_ptr()) };
        assert_eq!(empty_status, 2);

        let expected_path = PathBuf::from(r"C:\work\manifest.json");
        let wide_path: Vec<u16> = expected_path
            .as_os_str()
            .encode_wide()
            .chain(iter::once(0))
            .collect();
        let success_status = unsafe { dwm_lut_initialize(wide_path.as_ptr()) };
        assert_eq!(success_status, 0);
        assert!(is_initialized());
        assert_eq!(manifest_path(), Some(expected_path.clone()));

        let already_initialized_status = unsafe { dwm_lut_initialize(wide_path.as_ptr()) };
        assert_eq!(already_initialized_status, 3);
    }
}
