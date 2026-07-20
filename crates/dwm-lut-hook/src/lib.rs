mod blue_noise;
#[macro_use]
mod debug_log;
mod bootstrap;
mod d3d11_renderer;
mod desktop_redraw;
mod lut_bypass;
mod lut_pipeline;
mod minhook;
mod profile;
mod resolver;
mod route_trace;
mod state;

pub use bootstrap::HookError;
pub use lut_bypass::{
    ContextLutState, DisableIndependentFlipPatch, LutBypassRuntime, OverlayTestModeControl,
    OverlayTestModePatch, PresentHookOutcome,
};
pub use lut_pipeline::{
    BackBufferFormat, ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
    DirtyRect, LoadedLut, LutMetadata, LutPipeline, LutRenderPlan, ShaderConstants,
    ShaderConstantsCBuffer, ShaderTexture3D, apply_sdr_dither, cube_to_texture, pq_to_scrgb,
    scrgb_to_pq, tetrahedral_interpolation,
};
pub use minhook::{MinHookError, MinHookRuntime, MinHookState, RegisteredHook};
pub use profile::{
    AobToken, ClipBoxPathHypothesis, HOOK_MODULE_NAME, HardwareProtectedPathHypothesis,
    HookProfile, HookSignature, HookTarget, MinBuild, MonitorIdentityPathHypothesis,
    ProfileHypotheses, ProfileSelectError, SignatureLocator, SwapChainPathHypothesis,
    VERSIONED_PROFILES, VersionedProfile, os_build_number, select_versioned_profile,
};
pub use resolver::{
    HookResolveError, LoadedModule, ResolvedTarget, SignatureResolutionReport, SkippedSignature,
    SkippedSignatureReason, resolve_profile,
};
pub use state::{
    HookRegistrationPlan, HookRegistrationTarget, HookRuntime, HookState,
    evaluate_comp_swap_chain_direct_flip_compatible,
    evaluate_comp_swap_chain_independent_flip_compatible,
    evaluate_comp_visual_candidate_for_promotion, evaluate_direct_flip_compatible,
    evaluate_direct_flip_support_compatible, evaluate_ensure_independent_flip_state,
    evaluate_overlay_test_mode, evaluate_window_context_direct_flip_compatible, hook_profile,
    is_initialized, lut_bypass_runtime,
};

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;

/// # Safety
///
/// `payload` must be null or point to a readable payload buffer in the address
/// space of the current process.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn dwm_lut_initialize(
    payload: *const dwm_lut_payload::DwmLutPayloadBuffer,
) -> u32 {
    unsafe { bootstrap::ffi_initialize(payload) }
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_shutdown() -> u32 {
    bootstrap::ffi_shutdown()
}

/// # Safety
///
/// `payload` must be null or point to a readable payload buffer in the address
/// space of the current process.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn dwm_lut_replace_assignments(
    payload: *const dwm_lut_payload::DwmLutPayloadBuffer,
) -> u32 {
    unsafe { bootstrap::ffi_replace_assignments(payload) }
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
pub extern "system" fn dwm_lut_ensure_independent_flip_state(original_status: i32) -> i32 {
    state::evaluate_ensure_independent_flip_state().unwrap_or(original_status)
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_direct_flip_support_compatible(original_compatible: i32) -> i32 {
    let original_compatible = original_compatible != 0;
    i32::from(
        state::evaluate_direct_flip_support_compatible(original_compatible)
            .unwrap_or(original_compatible),
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
        state::mark_process_detaching();
        state::restore_overlay_test_mode();
    }

    TRUE
}
