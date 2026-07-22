#[macro_use]
mod debug_log;
mod bootstrap;
mod d3d11;
mod desktop_redraw;
mod flip_gate;
mod minhook;
mod present;
mod profile;
mod resolver;
mod state;

pub use bootstrap::HookError;
pub use flip_gate::{
    DisableIndependentFlipPatch, FlipGateEffects, OverlayTestModeControl, OverlayTestModePatch,
};
pub use minhook::{MinHookError, MinHookRuntime, MinHookState, RegisteredHook};
pub use present::DirtyRect;
pub use profile::{
    AobToken, DwmcoreVersion, HOOK_MODULE_NAME, HookProfile, HookSignature, HookTarget,
    MonitorIdentityOffsets, ProfileSelectError, SignatureLocator, SwapChainVtablePath,
    VERSIONED_PROFILES, VersionedProfile, dwmcore_file_version, select_versioned_profile,
};
pub use resolver::{
    HookResolveError, LoadedModule, ResolvedTarget, SignatureResolutionReport, SkippedSignature,
    SkippedSignatureReason, resolve_profile,
};
pub use state::{
    HookRegistrationPlan, HookRegistrationTarget, HookRuntime, HookState, LutAssignment,
    LutMetadata, ShaderTexture3D, assignments_from_payload, cube_to_texture, has_active_contexts,
    has_lut_assignments, has_present_context, hook_profile, is_initialized,
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
    let result = flip_gate::direct_flip_compatible(
        state::has_present_context(context_address),
        original_compatible,
    );
    i32::from(result)
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_ensure_independent_flip_state(original_status: i32) -> i32 {
    flip_gate::ensure_independent_flip_state(state::has_lut_assignments())
        .unwrap_or(original_status)
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_direct_flip_support_compatible(original_compatible: i32) -> i32 {
    let original_compatible = original_compatible != 0;
    i32::from(flip_gate::direct_flip_support_compatible(
        state::has_lut_assignments(),
        original_compatible,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn dwm_lut_overlay_test_mode(original_mode: i32) -> i32 {
    flip_gate::overlay_test_mode(state::has_active_contexts(), original_mode)
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
        state::clear_present_session();
    }

    TRUE
}
