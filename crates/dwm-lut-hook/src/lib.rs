mod blue_noise;
mod bootstrap;
mod d3d11_renderer;
mod lut_bypass;
mod lut_pipeline;
mod minhook;
mod profile;
mod resolver;
mod state;

pub use bootstrap::{HookError, build_profile};
pub use lut_bypass::{
    ContextLutState, LutBypassRuntime, OverlayTestModeControl, PresentHookOutcome,
};
pub use lut_pipeline::{
    BackBufferFormat, ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
    DirtyRect, LoadedLut, LutMetadata, LutPipeline, LutPipelineError, LutPipelineSummary,
    LutRenderPlan, LutShaderProgram, ShaderConstants, ShaderConstantsCBuffer, ShaderTexture3D,
    apply_sdr_dither, cube_to_texture, pq_to_scrgb, scrgb_to_pq, tetrahedral_interpolation,
};
pub use minhook::{MinHookError, MinHookRuntime, MinHookState, RegisteredHook};
pub use profile::{
    AobToken, BuildProfile, ClipBoxOwner, ClipBoxPathHypothesis, HardwareProtectedPathHypothesis,
    HookProfile, HookSignature, HookTarget, ProfileHypotheses, SignatureLocator,
    SwapChainPathHypothesis,
};
pub use resolver::{
    HookResolveError, LoadedModule, ResolvedTarget, SignatureResolutionReport, resolve_profile,
};
pub use state::{
    HookConfig, HookRegistrationPlan, HookRegistrationState, HookRegistrationTarget, HookRuntime,
    HookState, InitializationStage, LoggerState, LutBypassState, ManifestLoadState,
    SignatureResolutionState, evaluate_comp_swap_chain_direct_flip_compatible,
    evaluate_comp_swap_chain_independent_flip_compatible,
    evaluate_comp_visual_candidate_for_promotion, evaluate_direct_flip_compatible,
    evaluate_overlay_test_mode, evaluate_overlays_enabled, evaluate_present_hook,
    evaluate_window_context_direct_flip_compatible, hook_profile, initialization_trace,
    is_initialized, lut_bypass_runtime, lut_pipeline_summary, manifest_path, signature_resolution,
};

use std::ffi::c_void;
use std::slice;

use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;

const PRESENT_FLAG_HAS_PLAN: u32 = 1 << 0;
const PRESENT_FLAG_PROMOTION_BLOCKED: u32 = 1 << 1;
const PRESENT_FLAG_OVERLAY_TEST_MODE_FORCED: u32 = 1 << 2;

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
/// If `dirty_rect_count` is non-zero, `dirty_rects` must point to a readable
/// array of `DirtyRect` values in the current process.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn dwm_lut_update_present_state(
    context_address: usize,
    clip_box: ClipBox,
    dxgi_format: u32,
    dirty_rects: *const DirtyRect,
    dirty_rect_count: usize,
    lut_applied: i32,
) -> u32 {
    let dirty_rects = if dirty_rect_count == 0 {
        &[]
    } else if dirty_rects.is_null() {
        return 0;
    } else {
        unsafe { slice::from_raw_parts(dirty_rects, dirty_rect_count) }
    };

    state::evaluate_present_hook(
        context_address,
        clip_box,
        dxgi_format,
        dirty_rects,
        lut_applied != 0,
    )
    .map(|outcome| {
        let mut flags = 0;
        if outcome.plan.is_some() {
            flags |= PRESENT_FLAG_HAS_PLAN;
        }
        if outcome.promotion_blocked {
            flags |= PRESENT_FLAG_PROMOTION_BLOCKED;
        }
        if outcome.overlay_test_mode_control.is_force_mode_5() {
            flags |= PRESENT_FLAG_OVERLAY_TEST_MODE_FORCED;
        }
        flags
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_overlays_enabled(
    context_address: usize,
    original_enabled: i32,
) -> i32 {
    let original_enabled = original_enabled != 0;
    i32::from(
        state::evaluate_overlays_enabled(context_address, original_enabled)
            .unwrap_or(original_enabled),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_direct_flip_compatible(
    context_address: usize,
    original_compatible: i32,
) -> i32 {
    let original_compatible = original_compatible != 0;
    i32::from(
        state::evaluate_direct_flip_compatible(context_address, original_compatible)
            .unwrap_or(original_compatible),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_window_context_direct_flip_compatible(
    original_compatible: i32,
) -> i32 {
    let original_compatible = original_compatible != 0;
    i32::from(
        state::evaluate_window_context_direct_flip_compatible(original_compatible)
            .unwrap_or(original_compatible),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_comp_swap_chain_direct_flip_compatible(
    original_compatible: i32,
) -> i32 {
    let original_compatible = original_compatible != 0;
    i32::from(
        state::evaluate_comp_swap_chain_direct_flip_compatible(original_compatible)
            .unwrap_or(original_compatible),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_comp_swap_chain_independent_flip_compatible(
    original_compatible: i32,
) -> i32 {
    let original_compatible = original_compatible != 0;
    i32::from(
        state::evaluate_comp_swap_chain_independent_flip_compatible(original_compatible)
            .unwrap_or(original_compatible),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_comp_visual_candidate_for_promotion(original_candidate: i32) -> i32 {
    let original_candidate = original_candidate != 0;
    i32::from(
        state::evaluate_comp_visual_candidate_for_promotion(original_candidate)
            .unwrap_or(original_candidate),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_overlay_test_mode(original_mode: i32) -> i32 {
    state::evaluate_overlay_test_mode(original_mode).unwrap_or(original_mode)
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
    const DLL_PROCESS_DETACH: u32 = 0;

    if reason == DLL_PROCESS_ATTACH {
        unsafe {
            DisableThreadLibraryCalls(module);
        }
    } else if reason == DLL_PROCESS_DETACH {
        state::restore_overlay_test_mode();
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
        BuildProfile, ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DirtyRect, HookConfig, HookProfile,
        HookTarget, InitializationStage, LutBypassState, ManifestLoadState, PRESENT_FLAG_HAS_PLAN,
        PRESENT_FLAG_OVERLAY_TEST_MODE_FORCED, PRESENT_FLAG_PROMOTION_BLOCKED, SignatureLocator,
        SignatureResolutionReport, build_profile, dwm_lut_comp_swap_chain_direct_flip_compatible,
        dwm_lut_comp_swap_chain_independent_flip_compatible,
        dwm_lut_comp_visual_candidate_for_promotion, dwm_lut_direct_flip_compatible,
        dwm_lut_initialize, dwm_lut_overlay_test_mode, dwm_lut_overlays_enabled,
        dwm_lut_update_present_state, dwm_lut_window_context_direct_flip_compatible, hook_profile,
        initialization_trace, is_initialized, lut_bypass_runtime, lut_pipeline_summary,
        manifest_path, signature_resolution,
    };
    use crate::bootstrap::initialize_with_resolution;
    use crate::resolver::{LoadedModule, ResolvedTarget};
    use crate::state::{LutPipelineState, reset_state_for_tests};

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
                        SignatureLocator::AobExcludingFollowingBytes { capture_key, .. } => {
                            *capture_key
                        }
                        SignatureLocator::RipRelativeGlobalAob { capture_key, .. } => *capture_key,
                        SignatureLocator::FollowingAob { capture_key, .. } => *capture_key,
                    };

                    ResolvedTarget {
                        target: signature.target,
                        capture_key,
                        address: if signature.target == HookTarget::OverlayTestMode {
                            0
                        } else {
                            base_address + 0x1000 + index * 0x100
                        },
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
        reset_state_for_tests();
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
                InitializationStage::ManifestLoaded,
                InitializationStage::LutPipelinePrepared,
                InitializationStage::ProfileSelected,
                InitializationStage::TargetModuleResolved,
                InitializationStage::SignaturesResolved,
                InitializationStage::HookRegistrationEnabled,
                InitializationStage::LutBypassStatePrepared,
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
                assert_eq!(report.targets.len(), 8);
            }
        }

        let hook_registration = &state.runtime.hook_registration;
        assert_eq!(hook_registration.plan.module_name, "dwmcore.dll");
        assert_eq!(hook_registration.plan.targets.len(), 7);
        assert_eq!(
            hook_registration.plan.targets[0].target,
            HookTarget::Present
        );
        assert_eq!(hook_registration.hooks.len(), 7);
        assert_eq!(hook_registration.hooks[0].target, HookTarget::Present);
        assert!(
            hook_registration
                .plan
                .targets
                .iter()
                .all(|target| target.target != HookTarget::OverlayTestMode)
        );
        assert!(
            hook_registration
                .plan
                .targets
                .iter()
                .all(|target| target.address != 0)
        );

        match &state.runtime.lut_pipeline {
            LutPipelineState::Ready(runtime) => assert_eq!(runtime.summary().lut_count, 1),
        }
        assert!(matches!(state.runtime.lut_bypass, LutBypassState::Ready(_)));

        let _ = fs::remove_file(expected_manifest_path);
        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn build_profile_exposes_initial_hypotheses() {
        let profile = HookProfile::for_build(build_profile());

        assert_eq!(profile.build, BuildProfile::Windows11_25H2);
        assert_eq!(profile.module_name, "dwmcore.dll");
        assert_eq!(profile.signatures.len(), 8);
        assert_eq!(profile.hypotheses.swap_chain.container_vtable_index, 24);
        assert_eq!(profile.hypotheses.swap_chain.resource_vtable_index, 19);
        assert_eq!(profile.hypotheses.clip_box.context_state_pointer_offset, 0);
        assert_eq!(profile.hypotheses.clip_box.offset, 0x4D0);
        assert_eq!(profile.hypotheses.hardware_protected.offset, 0x4C);
    }

    #[test]
    fn ffi_initialize_rejects_null_and_empty_manifest_paths() {
        let null_status = unsafe { dwm_lut_initialize(ptr::null()) };
        assert_eq!(null_status, 1);
    }

    #[test]
    fn prepare_initial_state_rejects_empty_manifest_path_without_touching_global_state() {
        reset_state_for_tests();
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
        reset_state_for_tests();
        let (expected_manifest_path, cube_path) = synthetic_manifest_paths();
        let config = HookConfig {
            manifest_path: expected_manifest_path.clone(),
            profile: BuildProfile::Windows11_25H2,
        };
        let resolution = synthetic_resolution(&HookProfile::for_build(config.profile));

        initialize_with_resolution(config.clone(), resolution.clone())
            .expect("initialization should succeed with synthetic resolution");
        let second_initialize = initialize_with_resolution(config, resolution.clone());
        assert!(matches!(
            second_initialize,
            Err(super::HookError::AlreadyInitialized)
        ));

        assert!(is_initialized());
        assert_eq!(manifest_path(), Some(expected_manifest_path.clone()));
        assert_eq!(
            initialization_trace(),
            Some(vec![
                InitializationStage::LoggerReady,
                InitializationStage::ManifestLoaded,
                InitializationStage::LutPipelinePrepared,
                InitializationStage::ProfileSelected,
                InitializationStage::TargetModuleResolved,
                InitializationStage::SignaturesResolved,
                InitializationStage::HookRegistrationEnabled,
                InitializationStage::LutBypassStatePrepared,
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
        assert_eq!(
            lut_bypass_runtime().map(|runtime| runtime.contexts.len()),
            Some(0)
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

    #[test]
    fn initialize_with_resolution_rejects_in_progress_initialization() {
        reset_state_for_tests();
        let _initialization = super::bootstrap::hold_initialization_for_tests()
            .expect("test should acquire initialization guard");
        let (expected_manifest_path, cube_path) = synthetic_manifest_paths();
        let config = HookConfig {
            manifest_path: expected_manifest_path.clone(),
            profile: BuildProfile::Windows11_25H2,
        };
        let resolution = synthetic_resolution(&HookProfile::for_build(config.profile));

        let result = initialize_with_resolution(config, resolution);

        assert!(matches!(result, Err(super::HookError::AlreadyInitialized)));
        assert!(!is_initialized());
        let _ = fs::remove_file(expected_manifest_path);
        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn present_updates_context_scoped_bypass_state() {
        reset_state_for_tests();
        let (expected_manifest_path, cube_path) = synthetic_manifest_paths();
        let config = HookConfig {
            manifest_path: expected_manifest_path.clone(),
            profile: BuildProfile::Windows11_25H2,
        };
        let resolution = synthetic_resolution(&HookProfile::for_build(config.profile));

        initialize_with_resolution(config, resolution)
            .expect("initialization should succeed with synthetic resolution");

        let dirty_rects = [DirtyRect {
            left: 0,
            top: 0,
            right: 64,
            bottom: 64,
        }];
        let flags = unsafe {
            dwm_lut_update_present_state(
                0x1234,
                ClipBox {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                DXGI_FORMAT_B8G8R8A8_UNORM,
                dirty_rects.as_ptr(),
                dirty_rects.len(),
                1,
            )
        };

        assert_ne!(flags & PRESENT_FLAG_HAS_PLAN, 0);
        assert_ne!(flags & PRESENT_FLAG_PROMOTION_BLOCKED, 0);
        assert_ne!(flags & PRESENT_FLAG_OVERLAY_TEST_MODE_FORCED, 0);
        assert_eq!(dwm_lut_overlays_enabled(0x1234, 1), 0);
        assert_eq!(dwm_lut_direct_flip_compatible(0x1234, 1), 0);
        assert_eq!(dwm_lut_window_context_direct_flip_compatible(1), 0);
        assert_eq!(dwm_lut_comp_swap_chain_direct_flip_compatible(1), 0);
        assert_eq!(dwm_lut_comp_swap_chain_independent_flip_compatible(1), 0);
        assert_eq!(dwm_lut_comp_visual_candidate_for_promotion(1), 0);
        assert_eq!(dwm_lut_overlay_test_mode(0), 5);
        assert_eq!(dwm_lut_overlays_enabled(0x4321, 1), 1);
        assert_eq!(dwm_lut_direct_flip_compatible(0x4321, 1), 1);
        assert_eq!(
            unsafe {
                dwm_lut_update_present_state(
                    0x9999,
                    ClipBox {
                        left: 0,
                        top: 0,
                        right: 1920,
                        bottom: 1080,
                    },
                    DXGI_FORMAT_B8G8R8A8_UNORM,
                    std::ptr::null(),
                    1,
                    1,
                )
            },
            0
        );

        assert_eq!(
            lut_bypass_runtime()
                .and_then(|runtime| runtime.context(0x1234).map(|context| context.lut_index)),
            Some(Some(0))
        );

        let _ = fs::remove_file(expected_manifest_path);
        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn present_export_uses_empty_slice_for_null_zero_length_dirty_rects() {
        reset_state_for_tests();
        let (expected_manifest_path, cube_path) = synthetic_manifest_paths();
        let config = HookConfig {
            manifest_path: expected_manifest_path.clone(),
            profile: BuildProfile::Windows11_25H2,
        };
        let resolution = synthetic_resolution(&HookProfile::for_build(config.profile));

        initialize_with_resolution(config, resolution)
            .expect("initialization should succeed with synthetic resolution");

        let flags = unsafe {
            dwm_lut_update_present_state(
                0x1234,
                ClipBox {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                DXGI_FORMAT_B8G8R8A8_UNORM,
                std::ptr::null(),
                0,
                0,
            )
        };

        assert_ne!(flags & PRESENT_FLAG_HAS_PLAN, 0);
        assert_eq!(flags & PRESENT_FLAG_PROMOTION_BLOCKED, 0);
        assert_eq!(dwm_lut_overlays_enabled(0x1234, 1), 1);

        let _ = fs::remove_file(expected_manifest_path);
        let _ = fs::remove_file(cube_path);
    }
}
