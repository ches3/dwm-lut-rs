use std::path::{Path, PathBuf};

use crate::error::{
    InitializeContext, InitializeStatus, InjectionStep, InjectorError, ReplaceAssignmentsStatus,
    ShutdownStatus,
};
use crate::win32::{
    NamedRemoteModule, OwnedHandle, RemoteAllocation, RemoteModule, find_remote_module,
    find_remote_modules_by_name, open_target_process, resolve_remote_export_address,
    resolve_remote_module_export_address, run_remote_thread, wide_null,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisableOutcome {
    ShutDown(ShutdownStatus),
    NotInjected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyOutcome {
    Replaced,
    Initialized,
    Reinitialized,
}

enum ReplaceAssignmentsOutcome {
    Replaced,
    Fallback,
    Failed(ReplaceAssignmentsStatus),
}

struct RemotePayload {
    _bytes: RemoteAllocation,
    buffer: RemoteAllocation,
}

impl RemotePayload {
    fn address(&self) -> *mut std::ffi::c_void {
        self.buffer.address()
    }
}

fn write_remote_payload(
    process: &OwnedHandle,
    payload_bytes: &[u8],
) -> Result<RemotePayload, InjectorError> {
    let remote_payload_bytes = RemoteAllocation::write_bytes(
        process,
        payload_bytes,
        windows_sys::Win32::System::Memory::PAGE_READWRITE,
        InjectionStep::AllocatePayloadBytes,
        InjectionStep::WritePayloadBytes,
    )?;
    let payload_buffer = dwm_lut_payload::DwmLutPayloadBuffer {
        data: remote_payload_bytes.address().cast(),
        len: payload_bytes.len(),
    };
    let buffer = RemoteAllocation::write_copy(
        process,
        &payload_buffer,
        windows_sys::Win32::System::Memory::PAGE_READWRITE,
        InjectionStep::AllocatePayloadBuffer,
        InjectionStep::WritePayloadBuffer,
    )?;
    Ok(RemotePayload {
        _bytes: remote_payload_bytes,
        buffer,
    })
}

fn try_remote_replace_assignments(
    process: &OwnedHandle,
    module: &NamedRemoteModule,
    payload_bytes: &[u8],
) -> Result<ReplaceAssignmentsOutcome, InjectorError> {
    let module_path = PathBuf::from(module_export_path(&module.path, &module.name));
    let remote_replace_assignments_address = match resolve_remote_module_export_address(
        process,
        module.module.base_address,
        "dwm_lut_replace_assignments",
        InjectionStep::ResolveReplaceAssignmentsExport,
        &module_path,
    ) {
        Ok(address) => address,
        Err(InjectorError::ExportNotFound { .. }) => {
            return Ok(ReplaceAssignmentsOutcome::Fallback);
        }
        Err(error) => return Err(error),
    };

    let remote_payload_buffer = write_remote_payload(process, payload_bytes)?;
    let replace_assignments_status = run_remote_thread(
        process,
        remote_replace_assignments_address,
        remote_payload_buffer.address(),
        InjectionStep::StartReplaceAssignments,
        InjectionStep::WaitReplaceAssignments,
    )?;

    match ReplaceAssignmentsStatus::from_code(replace_assignments_status) {
        Some(ReplaceAssignmentsStatus::Success) => Ok(ReplaceAssignmentsOutcome::Replaced),
        Some(status) if status.should_fallback() => Ok(ReplaceAssignmentsOutcome::Fallback),
        Some(status) => Ok(ReplaceAssignmentsOutcome::Failed(status)),
        None => Err(InjectorError::UnknownReplaceAssignmentsStatus(
            replace_assignments_status,
        )),
    }
}

fn find_matching_staged_dll<'a>(
    staged_dll_path: &Path,
    modules: &'a [NamedRemoteModule],
) -> Option<&'a NamedRemoteModule> {
    let expected = staged_dll_basename(staged_dll_path)?;
    modules
        .iter()
        .find(|module| matches_staged_dll_basename(expected, module))
}

fn staged_dll_basename(path: &Path) -> Option<&str> {
    path.file_name()?.to_str()
}

fn matches_staged_dll_basename(expected: &str, module: &NamedRemoteModule) -> bool {
    module_basename(&module.path, &module.name).eq_ignore_ascii_case(expected)
}

pub(crate) fn apply_config(
    pid: u32,
    staged_dll_path: &Path,
    payload_bytes: &[u8],
) -> Result<ApplyOutcome, InjectorError> {
    let process = open_target_process(pid)?;
    let loaded_hooks = find_remote_modules_by_name(
        pid,
        InjectionStep::ResolveShutdownExport,
        is_staged_hook_module,
    )?;

    if loaded_hooks.is_empty() {
        inject_and_initialize(
            pid,
            staged_dll_path,
            payload_bytes,
            InitializeContext::FreshInstall,
        )?;
        return Ok(ApplyOutcome::Initialized);
    }

    if let Some(module) = find_matching_staged_dll(staged_dll_path, &loaded_hooks) {
        match try_remote_replace_assignments(&process, module, payload_bytes)? {
            ReplaceAssignmentsOutcome::Replaced => return Ok(ApplyOutcome::Replaced),
            ReplaceAssignmentsOutcome::Fallback => {
                shutdown_for_reinject(pid)?;
                inject_and_initialize(
                    pid,
                    staged_dll_path,
                    payload_bytes,
                    InitializeContext::AfterReplaceFallback,
                )?;
                return Ok(ApplyOutcome::Reinitialized);
            }
            ReplaceAssignmentsOutcome::Failed(status) => {
                return Err(InjectorError::HookReplaceAssignmentsFailed(status));
            }
        }
    }

    shutdown_for_reinject(pid)?;
    inject_and_initialize(
        pid,
        staged_dll_path,
        payload_bytes,
        InitializeContext::AfterShutdown,
    )?;
    Ok(ApplyOutcome::Reinitialized)
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

pub(crate) fn inject_and_initialize(
    pid: u32,
    dll_path: &Path,
    payload_bytes: &[u8],
    context: InitializeContext,
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

    let remote_payload_buffer = write_remote_payload(&process, payload_bytes)?;
    let initialize_status = run_remote_thread(
        &process,
        remote_initialize_address,
        remote_payload_buffer.address(),
        InjectionStep::StartInitialize,
        InjectionStep::WaitInitialize,
    )?;

    match InitializeStatus::from_code(initialize_status) {
        Some(InitializeStatus::Success) => Ok(()),
        Some(status) => Err(InjectorError::HookInitializeFailed { status, context }),
        None => Err(InjectorError::UnknownInitializeStatus(initialize_status)),
    }
}

pub(crate) fn disable_injected_hook(pid: u32) -> Result<DisableOutcome, InjectorError> {
    let process = open_target_process(pid)?;
    let remote_hook_modules = find_remote_modules_by_name(
        pid,
        InjectionStep::ResolveShutdownExport,
        is_staged_hook_module,
    )?;
    if remote_hook_modules.is_empty() {
        return Ok(DisableOutcome::NotInjected);
    }

    let mut aggregation = ShutdownAggregation::default();
    for remote_hook_module in remote_hook_modules {
        let module_path = PathBuf::from(module_export_path(
            &remote_hook_module.path,
            &remote_hook_module.name,
        ));
        let remote_shutdown_address = match resolve_remote_module_export_address(
            &process,
            remote_hook_module.module.base_address,
            "dwm_lut_shutdown",
            InjectionStep::ResolveShutdownExport,
            &module_path,
        ) {
            Ok(address) => address,
            Err(error) => {
                aggregation.record_export_error(error)?;
                continue;
            }
        };

        let shutdown_status = run_remote_thread(
            &process,
            remote_shutdown_address,
            std::ptr::null_mut(),
            InjectionStep::StartShutdown,
            InjectionStep::WaitShutdown,
        )?;
        let Some(status) = ShutdownStatus::from_code(shutdown_status) else {
            return Err(InjectorError::UnknownShutdownStatus(shutdown_status));
        };

        if let Some(outcome) = aggregation.record_status(status)? {
            return Ok(outcome);
        }
    }

    aggregation.finish()
}

fn shutdown_for_reinject(pid: u32) -> Result<(), InjectorError> {
    match disable_injected_hook(pid)? {
        DisableOutcome::NotInjected => Ok(()),
        DisableOutcome::ShutDown(ShutdownStatus::Success)
        | DisableOutcome::ShutDown(ShutdownStatus::NotInitialized)
        | DisableOutcome::ShutDown(ShutdownStatus::AlreadyShutDown) => Ok(()),
        DisableOutcome::ShutDown(status) => Err(InjectorError::HookShutdownFailed(status)),
    }
}

fn is_staged_hook_module(module: &NamedRemoteModule) -> bool {
    is_staged_hook_module_name(module_basename(&module.path, &module.name))
}

fn module_export_path<'a>(module_path: &'a str, module_name: &'a str) -> &'a str {
    if module_path.is_empty() {
        module_name
    } else {
        module_path
    }
}

fn module_basename<'a>(module_path: &'a str, module_name: &'a str) -> &'a str {
    let source = module_export_path(module_path, module_name);
    Path::new(source)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(module_name)
}

fn is_staged_hook_module_name(module_name: &str) -> bool {
    let lower = module_name.to_ascii_lowercase();
    let Some(hex) = lower
        .strip_prefix("dwm_lut_hook-")
        .and_then(|value| value.strip_suffix(".dll"))
    else {
        return false;
    };

    hex.len() == 32 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownDecision {
    Done,
    Continue,
    Fail,
}

fn evaluate_shutdown_status(status: ShutdownStatus) -> ShutdownDecision {
    match status {
        ShutdownStatus::Success => ShutdownDecision::Done,
        ShutdownStatus::NotInitialized | ShutdownStatus::AlreadyShutDown => {
            ShutdownDecision::Continue
        }
        ShutdownStatus::AlreadyInProgress | ShutdownStatus::MinHookCleanupFailed => {
            ShutdownDecision::Fail
        }
    }
}

#[derive(Debug, Default)]
struct ShutdownAggregation {
    deferred_status: Option<ShutdownStatus>,
    export_not_found: Option<InjectorError>,
}

impl ShutdownAggregation {
    fn record_export_error(&mut self, error: InjectorError) -> Result<(), InjectorError> {
        match error {
            InjectorError::ExportNotFound { .. } => {
                self.export_not_found.get_or_insert(error);
                Ok(())
            }
            error => Err(error),
        }
    }

    fn record_status(
        &mut self,
        status: ShutdownStatus,
    ) -> Result<Option<DisableOutcome>, InjectorError> {
        match evaluate_shutdown_status(status) {
            ShutdownDecision::Done => Ok(Some(DisableOutcome::ShutDown(status))),
            ShutdownDecision::Fail => Err(InjectorError::HookShutdownFailed(status)),
            ShutdownDecision::Continue => {
                self.deferred_status =
                    preferred_deferred_shutdown_status(self.deferred_status, status);
                Ok(None)
            }
        }
    }

    fn finish(self) -> Result<DisableOutcome, InjectorError> {
        if let Some(status) = self.deferred_status {
            return Ok(DisableOutcome::ShutDown(status));
        }

        Err(self
            .export_not_found
            .expect("at least one staged hook module was evaluated"))
    }
}

fn preferred_deferred_shutdown_status(
    current: Option<ShutdownStatus>,
    candidate: ShutdownStatus,
) -> Option<ShutdownStatus> {
    let current_rank = current.map(deferred_shutdown_status_rank).unwrap_or(0);
    let candidate_rank = deferred_shutdown_status_rank(candidate);
    if candidate_rank > current_rank {
        Some(candidate)
    } else {
        current
    }
}

fn deferred_shutdown_status_rank(status: ShutdownStatus) -> u8 {
    match status {
        ShutdownStatus::AlreadyShutDown => 2,
        ShutdownStatus::NotInitialized => 1,
        ShutdownStatus::Success
        | ShutdownStatus::AlreadyInProgress
        | ShutdownStatus::MinHookCleanupFailed => 0,
    }
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::error::InjectorError;
    use crate::error::ShutdownStatus;

    use super::{
        DisableOutcome, ShutdownAggregation, ShutdownDecision, evaluate_shutdown_status,
        find_matching_staged_dll, is_staged_hook_module_name, matches_staged_dll_basename,
        module_basename, staged_dll_basename,
    };
    use crate::win32::NamedRemoteModule;

    #[test]
    fn staged_hook_module_match_is_limited_to_content_addressed_hook_dlls() {
        assert!(is_staged_hook_module_name(
            "dwm_lut_hook-0123456789abcdef0123456789abcdef.dll"
        ));
        assert!(is_staged_hook_module_name(
            "DWM_LUT_HOOK-0123456789ABCDEF0123456789ABCDEF.DLL"
        ));
        assert!(!is_staged_hook_module_name("dwm_lut_hook.dll"));
        assert!(!is_staged_hook_module_name(
            "dwm_lut_hook-0123456789abcdef0123456789abcdef-extra.dll"
        ));
        assert!(!is_staged_hook_module_name(
            "dwm_lut_hook-0123456789abcdef0123456789abcdeg.dll"
        ));
        assert!(!is_staged_hook_module_name(
            "dwm_lut_hook-0123456789abcdef.dll"
        ));
        assert!(!is_staged_hook_module_name("other.dll"));
    }

    #[test]
    fn staged_hook_module_match_uses_module_path_basename() {
        assert_eq!(
            module_basename(
                r"C:\ProgramData\dwm-lut-rs\hook\dwm_lut_hook-0123456789abcdef0123456789abcdef.dll",
                "ignored.dll",
            ),
            "dwm_lut_hook-0123456789abcdef0123456789abcdef.dll"
        );
        assert_eq!(module_basename("", "dwm_lut_hook.dll"), "dwm_lut_hook.dll");
    }

    #[test]
    fn shutdown_status_decision_continues_until_success_or_failure() {
        assert_eq!(
            evaluate_shutdown_status(ShutdownStatus::NotInitialized),
            ShutdownDecision::Continue
        );
        assert_eq!(
            evaluate_shutdown_status(ShutdownStatus::AlreadyShutDown),
            ShutdownDecision::Continue
        );
        assert_eq!(
            evaluate_shutdown_status(ShutdownStatus::Success),
            ShutdownDecision::Done
        );
        assert_eq!(
            evaluate_shutdown_status(ShutdownStatus::AlreadyInProgress),
            ShutdownDecision::Fail
        );
        assert_eq!(
            evaluate_shutdown_status(ShutdownStatus::MinHookCleanupFailed),
            ShutdownDecision::Fail
        );
    }

    #[test]
    fn shutdown_aggregation_continues_after_export_not_found_until_success() {
        let mut aggregation = ShutdownAggregation::default();
        aggregation
            .record_export_error(InjectorError::ExportNotFound {
                export: "dwm_lut_shutdown".to_string(),
                dll_path: PathBuf::from("old.dll"),
            })
            .expect("export mismatch should not stop candidate evaluation");

        let outcome = aggregation
            .record_status(ShutdownStatus::Success)
            .expect("success should be accepted")
            .expect("success should finish aggregation");

        assert_eq!(outcome, DisableOutcome::ShutDown(ShutdownStatus::Success));
    }

    #[test]
    fn shutdown_aggregation_returns_representative_export_error_when_none_resolve() {
        let mut aggregation = ShutdownAggregation::default();
        aggregation
            .record_export_error(InjectorError::ExportNotFound {
                export: "dwm_lut_shutdown".to_string(),
                dll_path: PathBuf::from("first.dll"),
            })
            .expect("first export mismatch should be recorded");
        aggregation
            .record_export_error(InjectorError::ExportNotFound {
                export: "dwm_lut_shutdown".to_string(),
                dll_path: PathBuf::from("second.dll"),
            })
            .expect("second export mismatch should be recorded");

        let error = aggregation
            .finish()
            .expect_err("all candidates without shutdown export should fail");

        match error {
            InjectorError::ExportNotFound { dll_path, .. } => {
                assert_eq!(dll_path, PathBuf::from("first.dll"));
            }
            error => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn shutdown_aggregation_prefers_already_shutdown_over_not_initialized() {
        for statuses in [
            [
                ShutdownStatus::NotInitialized,
                ShutdownStatus::AlreadyShutDown,
            ],
            [
                ShutdownStatus::AlreadyShutDown,
                ShutdownStatus::NotInitialized,
            ],
        ] {
            let mut aggregation = ShutdownAggregation::default();
            for status in statuses {
                assert_eq!(
                    aggregation
                        .record_status(status)
                        .expect("benign status should be accepted"),
                    None
                );
            }

            assert_eq!(
                aggregation.finish().expect("benign statuses should finish"),
                DisableOutcome::ShutDown(ShutdownStatus::AlreadyShutDown)
            );
        }
    }

    fn sample_remote_module(path: &str, name: &str) -> NamedRemoteModule {
        NamedRemoteModule {
            path: path.to_string(),
            name: name.to_string(),
            module: crate::win32::RemoteModule {
                base_address: 0x1000,
            },
        }
    }

    #[test]
    fn staged_dll_basename_matching_is_case_insensitive() {
        let staged = PathBuf::from(
            r"C:\ProgramData\dwm-lut-rs\hook\dwm_lut_hook-0123456789abcdef0123456789abcdef.dll",
        );
        let module = sample_remote_module(
            r"C:\ProgramData\dwm-lut-rs\hook\DWM_LUT_HOOK-0123456789ABCDEF0123456789ABCDEF.DLL",
            "ignored.dll",
        );

        assert_eq!(
            staged_dll_basename(&staged),
            Some("dwm_lut_hook-0123456789abcdef0123456789abcdef.dll")
        );
        assert!(matches_staged_dll_basename(
            "dwm_lut_hook-0123456789abcdef0123456789abcdef.dll",
            &module
        ));
        assert!(find_matching_staged_dll(&staged, &[module]).is_some());
    }

    #[test]
    fn staged_dll_basename_matching_rejects_different_content_hash() {
        let staged = PathBuf::from(
            r"C:\ProgramData\dwm-lut-rs\hook\dwm_lut_hook-11111111111111111111111111111111.dll",
        );
        let module = sample_remote_module(
            r"C:\ProgramData\dwm-lut-rs\hook\dwm_lut_hook-22222222222222222222222222222222.dll",
            "ignored.dll",
        );

        assert!(find_matching_staged_dll(&staged, &[module]).is_none());
    }
}
