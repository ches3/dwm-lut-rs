mod blue_noise;
mod bootstrap;
mod lut_pipeline;
mod minhook;
mod profile;
mod resolver;
mod state;

pub use bootstrap::{HookError, build_profile};
pub use lut_pipeline::{
    BackBufferFormat, ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
    DirtyRect, LoadedLut, LutMetadata, LutPipeline, LutPipelineError, LutPipelineSummary,
    LutRenderPlan, LutShaderProgram, ShaderConstants, ShaderConstantsCBuffer, ShaderTexture3D,
    apply_sdr_dither, cube_to_texture, tetrahedral_interpolation,
};
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
    hook_profile, initialization_trace, is_initialized, lut_pipeline_summary, manifest_path,
    signature_resolution,
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
    use std::fs;
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        BuildProfile, HookConfig, HookProfile, HookRegistrationState, HookTarget,
        InitializationStage, ManifestLoadState, SignatureLocator, SignatureResolutionReport,
        build_profile, dwm_lut_initialize, hook_profile, initialization_trace, is_initialized,
        lut_pipeline_summary, manifest_path, signature_resolution,
    };
    use crate::bootstrap::initialize_with_resolution;
    use crate::resolver::{LoadedModule, ResolvedTarget};
    use crate::state::LutPipelineState;

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

    fn test_cube_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dwm-lut-hook-test-{unique}.cube"));
        fs::write(
            &path,
            "LUT_3D_SIZE 2\n\
0.0 0.0 0.0\n\
1.0 0.0 0.0\n\
0.0 1.0 0.0\n\
1.0 1.0 0.0\n\
0.0 0.0 1.0\n\
1.0 0.0 1.0\n\
0.0 1.0 1.0\n\
1.0 1.0 1.0\n",
        )
        .expect("cube file should be written");
        path
    }

    fn write_test_manifest(cube_path: &Path) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dwm-lut-hook-test-{unique}.json"));
        let cube_path = cube_path.display().to_string().replace('\\', "\\\\");
        let manifest = format!(
            "{{\n  \"assignments\": [\n    {{\n      \"monitor_id\": \"DISPLAY1\",\n      \"desktop_left\": 0,\n      \"desktop_top\": 0,\n      \"color_mode\": \"sdr\",\n      \"lut_path\": \"{cube_path}\",\n      \"lut_size\": 2\n    }}\n  ]\n}}\n"
        );
        fs::write(&path, manifest).expect("manifest file should be written");
        path
    }

    fn synthetic_manifest_paths() -> (PathBuf, PathBuf) {
        let cube_path = test_cube_path();
        let manifest_path = write_test_manifest(&cube_path);
        (manifest_path, cube_path)
    }

    #[test]
    fn prepare_initial_state_records_bootstrap_order() {
        let (expected_manifest_path, cube_path) = synthetic_manifest_paths();
        let config = HookConfig {
            manifest_path: expected_manifest_path.clone(),
            profile: BuildProfile::Windows11_25H2,
        };
        let resolution = synthetic_resolution(&HookProfile::for_build(config.profile));
        let state = super::bootstrap::prepare_initial_state_with_resolution(
            config.clone(),
            resolution.clone(),
        )
        .expect("state should build");

        assert_eq!(
            state.runtime.initialization_trace,
            vec![
                InitializationStage::LoggerReady,
                InitializationStage::MinHookBoundaryReady,
                InitializationStage::ManifestLoaded,
                InitializationStage::LutPipelinePrepared,
                InitializationStage::ProfileSelected,
                InitializationStage::TargetModuleResolved,
                InitializationStage::SignaturesResolved,
                InitializationStage::HookRegistrationDeferred,
                InitializationStage::GlobalStateCommitted,
            ]
        );
        assert_eq!(
            state.runtime.manifest_load,
            ManifestLoadState::Loaded {
                manifest_path: expected_manifest_path.clone(),
                assignment_count: 1,
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

        match &state.runtime.lut_pipeline {
            LutPipelineState::Ready(runtime) => assert_eq!(runtime.summary().lut_count, 1),
        }

        let _ = fs::remove_file(expected_manifest_path);
        let _ = fs::remove_file(cube_path);
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
        let resolution = synthetic_resolution(&HookProfile::for_build(config.profile));

        let error = super::bootstrap::prepare_initial_state_with_resolution(config, resolution)
            .expect_err("empty path should be rejected before state installation");

        assert!(matches!(error, super::HookError::InvalidPath));
    }

    #[test]
    fn initialize_with_resolution_commits_resolved_state() {
        let (expected_manifest_path, cube_path) = synthetic_manifest_paths();
        let config = HookConfig {
            manifest_path: expected_manifest_path.clone(),
            profile: BuildProfile::Windows11_25H2,
        };
        let resolution = synthetic_resolution(&HookProfile::for_build(config.profile));

        initialize_with_resolution(config, resolution.clone())
            .expect("initialization should succeed with synthetic resolution");

        assert!(is_initialized());
        assert_eq!(manifest_path(), Some(expected_manifest_path.clone()));
        assert_eq!(
            initialization_trace(),
            Some(vec![
                InitializationStage::LoggerReady,
                InitializationStage::MinHookBoundaryReady,
                InitializationStage::ManifestLoaded,
                InitializationStage::LutPipelinePrepared,
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
        assert_eq!(
            lut_pipeline_summary().map(|summary| summary.lut_count),
            Some(1)
        );

        let wide_path: Vec<u16> = expected_manifest_path
            .as_os_str()
            .encode_wide()
            .chain(iter::once(0))
            .collect();
        let already_initialized_status = unsafe { dwm_lut_initialize(wide_path.as_ptr()) };
        assert_eq!(already_initialized_status, 3);
        let _ = fs::remove_file(expected_manifest_path);
        let _ = fs::remove_file(cube_path);
    }
}
