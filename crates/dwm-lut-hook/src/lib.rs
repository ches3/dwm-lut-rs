use std::fmt;
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

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_initialize(_manifest_path: *const u16) -> i32 {
    -1
}

pub fn _manifest_path_ref(path: &Path) -> &Path {
    path
}
