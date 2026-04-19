use std::path::{Path, PathBuf};

use crate::error::{HookInitializeStatus, InjectionStep, InjectorError};
use crate::win32::{
    OwnedHandle, RemoteAllocation, RemoteModule, find_remote_module, open_target_process,
    resolve_remote_export_address, resolve_remote_module_export_address, run_remote_thread,
    wide_null,
};

const REMOTE_LOAD_LIBRARY_X64_STUB: [u8; 60] = [
    0x48, 0x83, 0xEC, 0x28, 0x48, 0x89, 0x4C, 0x24, 0x20, 0x48, 0x8B, 0x41, 0x08, 0x48, 0x8B, 0x09,
    0xFF, 0xD0, 0x48, 0x85, 0xC0, 0x75, 0x0E, 0x48, 0x8B, 0x4C, 0x24, 0x20, 0x48, 0x8B, 0x41, 0x10,
    0x48, 0x8B, 0x09, 0xFF, 0xD0, 0x48, 0x8B, 0x4C, 0x24, 0x20, 0x48, 0x89, 0x41, 0x18, 0x48, 0x85,
    0xC0, 0x0F, 0x95, 0xC0, 0x0F, 0xB6, 0xC0, 0x48, 0x83, 0xC4, 0x28, 0xC3,
];

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RemoteDllLoadContext {
    dll_path: usize,
    get_module_handle_w: usize,
    load_library_w: usize,
    module_handle: usize,
}

pub(crate) fn inject_and_initialize(
    pid: u32,
    dll_path: &Path,
    manifest_path: &Path,
) -> Result<(), InjectorError> {
    let process = open_target_process(pid)?;
    let remote_kernel32 = find_remote_module(pid, "kernel32.dll", InjectionStep::ResolveKernel32)?;
    let get_module_handle_address = resolve_remote_export_address(
        remote_kernel32.base_address,
        "kernel32.dll",
        "GetModuleHandleW",
        InjectionStep::ResolveKernel32,
        InjectionStep::ResolveGetModuleHandleW,
    )?;
    let load_library_address = resolve_remote_export_address(
        remote_kernel32.base_address,
        "kernel32.dll",
        "LoadLibraryW",
        InjectionStep::ResolveKernel32,
        InjectionStep::ResolveLoadLibraryW,
    )?;

    let remote_hook_module = load_remote_module(
        &process,
        dll_path,
        get_module_handle_address,
        load_library_address,
    )?;

    let remote_initialize_address = resolve_remote_module_export_address(
        &process,
        remote_hook_module.base_address,
        "dwm_lut_initialize",
        InjectionStep::ResolveInitializeExport,
        dll_path,
    )?;

    let manifest_path_wide = wide_null(manifest_path.as_os_str());
    let remote_manifest_path = RemoteAllocation::write_utf16(
        &process,
        &manifest_path_wide,
        InjectionStep::AllocateManifestPath,
        InjectionStep::WriteManifestPath,
    )?;
    let initialize_status = run_remote_thread(
        &process,
        remote_initialize_address,
        remote_manifest_path.address(),
        InjectionStep::StartInitialize,
        InjectionStep::WaitInitialize,
    )?;

    match HookInitializeStatus::from_code(initialize_status) {
        Some(HookInitializeStatus::Success) => Ok(()),
        Some(status) => Err(InjectorError::HookInitializeFailed(status)),
        None => Err(InjectorError::UnknownHookInitializeStatus(
            initialize_status,
        )),
    }
}

pub(crate) fn canonicalize_existing_file(
    path: &Path,
    step: InjectionStep,
    kind: &'static str,
) -> Result<PathBuf, InjectorError> {
    if !path.is_file() {
        return Err(InjectorError::MissingFile {
            kind,
            path: path.to_path_buf(),
        });
    }

    path.canonicalize()
        .map_err(|source| InjectorError::StepFailed { step, source })
}

fn load_remote_module(
    process: &OwnedHandle,
    dll_path: &Path,
    get_module_handle_address: usize,
    load_library_address: usize,
) -> Result<RemoteModule, InjectorError> {
    let dll_path_wide = wide_null(dll_path.as_os_str());
    let remote_dll_path = RemoteAllocation::write_utf16(
        process,
        &dll_path_wide,
        InjectionStep::AllocateDllPath,
        InjectionStep::WriteDllPath,
    )?;
    let dll_load_context = RemoteDllLoadContext {
        dll_path: remote_dll_path.address() as usize,
        get_module_handle_w: get_module_handle_address,
        load_library_w: load_library_address,
        module_handle: 0,
    };
    let remote_context = RemoteAllocation::write_copy(
        process,
        &dll_load_context,
        windows_sys::Win32::System::Memory::PAGE_READWRITE,
        InjectionStep::AllocateDllLoadContext,
        InjectionStep::WriteDllLoadContext,
    )?;
    let remote_stub = RemoteAllocation::write_bytes(
        process,
        &REMOTE_LOAD_LIBRARY_X64_STUB,
        windows_sys::Win32::System::Memory::PAGE_EXECUTE_READWRITE,
        InjectionStep::AllocateDllLoadStub,
        InjectionStep::WriteDllLoadStub,
    )?;
    let exit_code = run_remote_thread(
        process,
        remote_stub.address() as usize,
        remote_context.address(),
        InjectionStep::StartDllLoad,
        InjectionStep::WaitDllLoad,
    )?;
    if exit_code == 0 {
        return Err(InjectorError::RemoteCallFailed {
            step: InjectionStep::WaitDllLoad,
            exit_code,
        });
    }

    let result =
        remote_context.read_copy::<RemoteDllLoadContext>(InjectionStep::ReadDllLoadResult)?;
    if result.module_handle == 0 {
        return Err(InjectorError::RemoteModuleNotFound {
            module: dll_path.display().to_string(),
        });
    }

    Ok(RemoteModule {
        base_address: result.module_handle,
    })
}
