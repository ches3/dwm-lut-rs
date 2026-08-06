#[cfg(not(all(
    target_arch = "x86_64",
    target_vendor = "pc",
    target_os = "windows",
    target_env = "msvc",
)))]
compile_error!("dwm-lut-hook supports only x86_64-pc-windows-msvc");

mod bootstrap;
mod d3d11;
mod desktop_redraw;
mod dwmcore;
mod flip_gate;
mod lifecycle;
mod log;
mod minhook;
mod present;
mod resolver;
mod state;

pub use bootstrap::HookError;
pub use dwmcore::DirtyRect;
pub use minhook::{MinHookError, MinHookRuntime, MinHookState, RegisteredHook};
pub use resolver::{
    HookResolveError, LoadedModule, ResolvedFunctionVa, SignatureResolutionReport, Va,
    resolve_profile,
};
pub use state::{
    HookRuntime, LutAssignment, LutConfig, LutMetadata, ShaderTexture3D, assignments_from_payload,
    cube_to_texture, hook_profile,
};

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;

// SAFETY: No other symbol in this cdylib uses `dwm_lut_status`.
#[unsafe(export_name = "dwm_lut_status")]
pub(crate) static DWM_LUT_STATUS: lifecycle::ExportedStatusSnapshot =
    lifecycle::ExportedStatusSnapshot::inactive();

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
