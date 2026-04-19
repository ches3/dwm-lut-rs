mod bootstrap;
mod minhook;
mod profile;
mod resolver;
mod state;

pub use bootstrap::{HookError, build_profile, initialize};
pub use minhook::{
    MhCreateHookApi, MhDisableHookApi, MhEnableHookApi, MhInitializeApi, MhRemoveHookApi, MhStatus,
    MinHookBindings, MinHookRuntime, MinHookState,
};
pub use profile::{
    AobToken, BuildProfile, ClipBoxOwner, ClipBoxPathHypothesis, HookProfile, HookSignature,
    HookTarget, ProfileHypotheses, SignatureLocator, SwapChainPathHypothesis,
};
pub use resolver::{
    HookResolveError, LoadedModule, ResolvedTarget, SignatureResolutionReport, resolve_profile,
};
pub use state::{
    HookConfig, HookRegistrationPlan, HookRegistrationState, HookRegistrationTarget, HookRuntime,
    HookState, InitializationStage, LoggerState, ManifestLoadState, SignatureResolutionState,
    hook_profile, initialization_trace, is_initialized, manifest_path, signature_resolution,
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

    use dwm_lut_config::LutManifest;

    use super::{
        BuildProfile, HookConfig, HookProfile, HookRegistrationState, HookTarget,
        InitializationStage, ManifestLoadState, SignatureLocator, SignatureResolutionReport,
        build_profile, dwm_lut_initialize, hook_profile, initialization_trace, is_initialized,
        manifest_path, signature_resolution,
    };
    use crate::bootstrap::initialize_with_resolution;
    use crate::resolver::{LoadedModule, ResolvedTarget};

    fn synthetic_resolution(profile: &HookProfile) -> SignatureResolutionReport {
        let base_address = 0x1800_0000usize;
        SignatureResolutionReport {
            module: LoadedModule {
                module_name: profile.module_name,
                base_address,
                size: 0x20_0000,
            },
            targets: profile
                .signatures
                .iter()
                .enumerate()
                .map(|(index, signature)| {
                    let capture_key = match &signature.locator {
                        SignatureLocator::Aob { capture_key, .. } => *capture_key,
                    };

                    ResolvedTarget {
                        target: signature.target,
                        capture_key,
                        address: base_address + 0x1000 + index * 0x100,
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn prepare_initial_state_records_bootstrap_order() {
        let config = HookConfig {
            manifest_path: PathBuf::from(r"C:\work\manifest.json"),
            profile: BuildProfile::Windows11_25H2,
        };
        let profile = HookProfile::for_build(config.profile);
        let resolution = synthetic_resolution(&profile);
        let state = super::bootstrap::prepare_initial_state_with_resolution(
            config.clone(),
            LutManifest::empty(),
            profile,
            resolution.clone(),
        )
        .expect("state should build");

        assert_eq!(
            state.runtime.initialization_trace,
            vec![
                InitializationStage::LoggerReady,
                InitializationStage::ManifestLoadDeferred,
                InitializationStage::MinHookBoundaryReady,
                InitializationStage::ProfileSelected,
                InitializationStage::TargetModuleResolved,
                InitializationStage::SignaturesResolved,
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

        match &state.runtime.resolution {
            super::SignatureResolutionState::Resolved(report) => {
                assert_eq!(report.module, resolution.module);
                assert_eq!(report.targets.len(), 3);
            }
        }

        match &state.runtime.hook_registration {
            HookRegistrationState::Deferred(plan) => {
                assert_eq!(plan.module_name, "dwmcore.dll");
                assert_eq!(plan.targets.len(), 3);
                assert_eq!(plan.targets[0].target, HookTarget::Present);
                assert!(plan.targets.iter().all(|target| target.address != 0));
            }
        }
    }

    #[test]
    fn build_profile_exposes_initial_hypotheses() {
        let profile = HookProfile::for_build(build_profile());

        assert_eq!(profile.build, BuildProfile::Windows11_25H2);
        assert_eq!(profile.module_name, "dwmcore.dll");
        assert_eq!(profile.signatures.len(), 3);
        assert_eq!(profile.hypotheses.swap_chain.vtable_offset, 0x108);
        assert_eq!(profile.hypotheses.clip_box.offset, 0x7698);
    }

    #[test]
    fn ffi_initialize_rejects_null_and_empty_manifest_paths() {
        let null_status = unsafe { dwm_lut_initialize(ptr::null()) };
        assert_eq!(null_status, 1);
    }

    #[test]
    fn prepare_initial_state_rejects_empty_manifest_path_without_touching_global_state() {
        let config = HookConfig {
            manifest_path: PathBuf::new(),
            profile: BuildProfile::Windows11_25H2,
        };
        let profile = HookProfile::for_build(config.profile);
        let resolution = synthetic_resolution(&profile);

        let error = super::bootstrap::prepare_initial_state_with_resolution(
            config,
            LutManifest::empty(),
            profile,
            resolution,
        )
        .expect_err("empty path should be rejected before state installation");

        assert!(matches!(error, super::HookError::InvalidPath));
    }

    #[test]
    fn initialize_with_resolution_commits_resolved_state() {
        let expected_path = PathBuf::from(r"C:\work\manifest.json");
        let config = HookConfig {
            manifest_path: expected_path.clone(),
            profile: BuildProfile::Windows11_25H2,
        };
        let profile = HookProfile::for_build(config.profile);
        let resolution = synthetic_resolution(&profile);

        initialize_with_resolution(config, LutManifest::empty(), profile, resolution.clone())
            .expect("initialization should succeed with synthetic resolution");

        assert!(is_initialized());
        assert_eq!(manifest_path(), Some(expected_path));
        assert_eq!(
            initialization_trace(),
            Some(vec![
                InitializationStage::LoggerReady,
                InitializationStage::ManifestLoadDeferred,
                InitializationStage::MinHookBoundaryReady,
                InitializationStage::ProfileSelected,
                InitializationStage::TargetModuleResolved,
                InitializationStage::SignaturesResolved,
                InitializationStage::HookRegistrationDeferred,
                InitializationStage::GlobalStateCommitted,
            ])
        );
        assert_eq!(
            hook_profile().map(|profile| profile.build),
            Some(BuildProfile::Windows11_25H2)
        );
        assert_eq!(
            signature_resolution().map(|report| report.module),
            Some(resolution.module)
        );

        let wide_path: Vec<u16> = PathBuf::from(r"C:\work\manifest.json")
            .as_os_str()
            .encode_wide()
            .chain(iter::once(0))
            .collect();
        let already_initialized_status = unsafe { dwm_lut_initialize(wide_path.as_ptr()) };
        assert_eq!(already_initialized_status, 3);
    }
}
