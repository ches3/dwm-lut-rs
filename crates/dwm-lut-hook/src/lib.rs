mod bootstrap;
mod minhook;
mod profile;
mod state;

pub use bootstrap::{HookError, build_profile, initialize};
pub use minhook::{
    MhCreateHookApi, MhDisableHookApi, MhEnableHookApi, MhInitializeApi, MhRemoveHookApi, MhStatus,
    MinHookBindings, MinHookRuntime, MinHookState,
};
pub use profile::{
    AobToken, BuildProfile, HookProfile, HookSignature, HookTarget, SignatureLocator,
    SignatureStage,
};
pub use state::{
    HookConfig, HookRegistrationPlan, HookRegistrationState, HookRuntime, HookState,
    InitializationStage, LoggerState, ManifestLoadState, hook_profile, initialization_trace,
    is_initialized, manifest_path,
};

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;

/// # Safety
///
/// `manifest_path` must be null or point to a readable, NUL-terminated UTF-16
/// string in the address space of the current process.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn dwm_lut_initialize(manifest_path: *const u16) -> u32 {
    bootstrap::ffi_initialize(manifest_path)
}

/// # Safety
///
/// This entry point is invoked by the Windows loader. It must stay minimal and
/// must not rely on facilities that are unsafe under the loader lock.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    module: HINSTANCE,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    const DLL_PROCESS_ATTACH: u32 = 1;

    if reason == DLL_PROCESS_ATTACH {
        unsafe {
            DisableThreadLibraryCalls(module);
        }
    }

    TRUE
}

#[cfg(test)]
mod tests {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::ptr;

    use super::{
        BuildProfile, HookConfig, HookProfile, HookRegistrationState, InitializationStage,
        ManifestLoadState, build_profile, dwm_lut_initialize, hook_profile, initialization_trace,
        is_initialized, manifest_path,
    };
    use dwm_lut_config::LutManifest;

    #[test]
    fn prepare_initial_state_records_phase3_bootstrap_order() {
        let config = HookConfig {
            manifest_path: PathBuf::from(r"C:\work\manifest.json"),
            profile: BuildProfile::Windows11_25H2,
        };
        let state = super::bootstrap::prepare_initial_state(config.clone(), LutManifest::empty())
            .expect("state should build");

        assert_eq!(
            state.runtime.initialization_trace,
            vec![
                InitializationStage::LoggerReady,
                InitializationStage::ManifestLoadDeferred,
                InitializationStage::MinHookBoundaryReady,
                InitializationStage::ProfileSelected,
                InitializationStage::HookRegistrationDeferred,
                InitializationStage::GlobalStateCommitted,
            ]
        );
        assert_eq!(
            state.runtime.manifest_load,
            ManifestLoadState::Deferred {
                manifest_path: config.manifest_path.clone(),
            }
        );

        match &state.runtime.hook_registration {
            HookRegistrationState::Deferred(plan) => {
                assert_eq!(plan.targets.len(), 3);
            }
        }
    }

    #[test]
    fn build_profile_exposes_required_targets() {
        let profile = HookProfile::for_build(build_profile());

        assert_eq!(profile.build, BuildProfile::Windows11_25H2);
        assert_eq!(profile.module_name, "dwmcore.dll");
        assert_eq!(profile.signatures.len(), 3);
    }

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
        assert_eq!(
            initialization_trace(),
            Some(vec![
                InitializationStage::LoggerReady,
                InitializationStage::ManifestLoadDeferred,
                InitializationStage::MinHookBoundaryReady,
                InitializationStage::ProfileSelected,
                InitializationStage::HookRegistrationDeferred,
                InitializationStage::GlobalStateCommitted,
            ])
        );
        assert_eq!(
            hook_profile().map(|profile| profile.build),
            Some(BuildProfile::Windows11_25H2)
        );

        let already_initialized_status = unsafe { dwm_lut_initialize(wide_path.as_ptr()) };
        assert_eq!(already_initialized_status, 3);
    }
}
