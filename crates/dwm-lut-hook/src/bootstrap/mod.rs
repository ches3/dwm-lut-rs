mod error;

pub use error::{HookError, ReplaceAssignmentsError};

use std::sync::Arc;

use dwm_lut_payload::{
    DwmLutPayloadBuffer, HookPayload, InitializeStatus, ReplaceAssignmentsStatus, ShutdownStatus,
    deserialize_payload_buffer,
};

use crate::dwmcore::dwmcore_file_version;
use crate::lifecycle::{
    ReplaceAssignmentsStart, ShutdownStart, begin_initialization, begin_replace_assignments,
    begin_shutdown,
};
use crate::log;
use crate::minhook::{
    MinHookError, MinHookRuntime, RegisteredHooks, disable_all_hooks, enable_hooks, register_hooks,
    set_flip_gate_enabled, uninitialize,
};
use crate::resolver::{HookResolveError, SignatureResolutionReport, resolve_profile};
use crate::state::assignments_from_payload;
use crate::state::{
    HookRuntime, LutAssignment, assignments, clear_assignments, clear_hook_runtime,
    clone_hook_runtime, has_hook_runtime, lock_present_runtime, store_assignments,
    store_hook_profile, store_hook_runtime,
};
use dwm_lut_profile::{HookProfile, select_profile};

#[cfg(test)]
pub(crate) fn initialize_with_resolution(
    profile: HookProfile,
    payload: HookPayload,
    resolution: SignatureResolutionReport,
) -> Result<(), HookError> {
    assert_eq!(
        crate::state::hook_profile(),
        Some(profile),
        "test initialization must use the process-wide hook profile",
    );
    let profile_name = payload.profile_name.clone();
    let transition = begin_initialization().ok_or(HookError::AlreadyInitialized)?;
    let result = if has_hook_runtime() {
        reactivate_from_payload(payload)
    } else {
        let flip_gate_enabled = payload.flip_gate_enabled;
        let (profile, assignments, runtime) =
            prepare_cold_install(payload, || Ok(profile), |_| Ok(resolution))?;
        install_prepared(profile, assignments, runtime, flip_gate_enabled)
    };
    finish_initialization_transition(transition, &profile_name, result.is_ok());
    result
}

pub(crate) unsafe fn ffi_initialize(payload_buffer: *const DwmLutPayloadBuffer) -> u32 {
    log::initialize_start();

    if payload_buffer.is_null() {
        return InitializeStatus::NullPayload.to_code();
    }

    let payload = match unsafe { deserialize_payload_buffer(payload_buffer) } {
        Ok(payload) => payload,
        Err(error) => {
            let status = InitializeStatus::from(&error);
            log::initialize_failed(status, &error);
            return status.to_code();
        }
    };

    match initialize_from_payload(payload) {
        Ok(()) => {
            crate::desktop_redraw::request_desktop_redraw();
            log::initialize_success();
            InitializeStatus::Success.to_code()
        }
        Err(error) => {
            let message = error.to_string();
            let status = InitializeStatus::from(error);
            log::initialize_failed(status, message);
            status.to_code()
        }
    }
}

pub(crate) fn ffi_shutdown() -> u32 {
    log::shutdown_start();

    let transition = match begin_shutdown() {
        ShutdownStart::Started(transition) => transition,
        ShutdownStart::InitializationInProgress => {
            log::shutdown_finished(log::ShutdownFinished::InitializationInProgress);
            return ShutdownStatus::AlreadyInProgress as u32;
        }
        ShutdownStart::AssignmentReplacementInProgress => {
            log::shutdown_finished(log::ShutdownFinished::AssignmentReplacementInProgress);
            return ShutdownStatus::AlreadyInProgress as u32;
        }
        ShutdownStart::ShutdownInProgress => {
            log::shutdown_finished(log::ShutdownFinished::ShutdownInProgress);
            return ShutdownStatus::AlreadyInProgress as u32;
        }
        ShutdownStart::AlreadyInactive => {
            log::shutdown_finished(log::ShutdownFinished::AlreadyShutDown);
            return ShutdownStatus::AlreadyShutDown as u32;
        }
    };

    let Some(runtime) = clone_hook_runtime() else {
        clear_assignments();
        clear_hook_runtime();
        transition.finish_inactive();
        log::shutdown_finished(log::ShutdownFinished::HookRuntimeMissing);
        return ShutdownStatus::Success as u32;
    };

    let deactivate_result = deactivate_hook_runtime(&runtime);
    clear_assignments();
    transition.finish_inactive();
    if deactivate_result.is_err() {
        log::shutdown_finished(log::ShutdownFinished::MinHookDisableAllFailed);
        ShutdownStatus::MinHookDisableAllFailed as u32
    } else {
        log::shutdown_finished(log::ShutdownFinished::Success);
        ShutdownStatus::Success as u32
    }
}

pub(crate) unsafe fn ffi_replace_assignments(payload_buffer: *const DwmLutPayloadBuffer) -> u32 {
    log::replace_assignments_start();

    if payload_buffer.is_null() {
        return ReplaceAssignmentsStatus::NullPayload as u32;
    }

    let payload = match unsafe { deserialize_payload_buffer(payload_buffer) } {
        Ok(payload) => payload,
        Err(error) => {
            let status = ReplaceAssignmentsStatus::from(&error);
            log::replace_assignments_failed(status, &error);
            return status as u32;
        }
    };

    match replace_assignments(payload) {
        Ok(()) => {
            log::replace_assignments_success();
            ReplaceAssignmentsStatus::Success as u32
        }
        Err(error) => {
            let status = ReplaceAssignmentsStatus::from(&error);
            log::replace_assignments_failed(status, &error);
            status as u32
        }
    }
}

fn replace_assignments(payload: HookPayload) -> Result<(), ReplaceAssignmentsError> {
    let profile_name = payload.profile_name.clone();
    let next_flip_gate_enabled = payload.flip_gate_enabled;
    let transition = match begin_replace_assignments() {
        ReplaceAssignmentsStart::Started(transition) => transition,
        ReplaceAssignmentsStart::RuntimeInactive => {
            return Err(ReplaceAssignmentsError::RuntimeInactive);
        }
        ReplaceAssignmentsStart::AlreadyInProgress => {
            return Err(ReplaceAssignmentsError::AlreadyInProgress);
        }
    };

    let next_assignments = Arc::new(assignments_from_payload(&payload));
    let Some(runtime) = clone_hook_runtime() else {
        clear_assignments();
        transition.finish_inactive();
        return Err(ReplaceAssignmentsError::RuntimeInactive);
    };
    if assignments().is_none() {
        transition.finish_inactive();
        return Err(ReplaceAssignmentsError::RuntimeInactive);
    }

    let flip_error = {
        let _present_guard = lock_present_runtime();
        match set_flip_gate_enabled(&runtime.minhook, &runtime.hooks, next_flip_gate_enabled) {
            Ok(()) => {
                store_assignments(next_assignments);
                let _ = crate::d3d11::shutdown_renderer_resources();
                None
            }
            Err(error) => Some(error),
        }
    };
    if let Some(error) = flip_error {
        {
            let _present_guard = lock_present_runtime();
            let _ = crate::d3d11::shutdown_renderer_resources();
            crate::desktop_redraw::request_desktop_redraw();
        }
        clear_assignments();
        transition.finish_inactive();
        Err(ReplaceAssignmentsError::Inactive(error))
    } else {
        crate::desktop_redraw::request_desktop_redraw();
        transition.commit_active(&profile_name);
        Ok(())
    }
}

fn deactivate_hook_runtime(runtime: &HookRuntime) -> Result<(), MinHookError> {
    let _present_guard = lock_present_runtime();
    let _ = crate::d3d11::shutdown_renderer_resources();
    crate::desktop_redraw::request_desktop_redraw();
    disable_all_hooks(&runtime.minhook)
}

fn initialize_from_payload(payload: HookPayload) -> Result<(), HookError> {
    let profile_name = payload.profile_name.clone();
    let transition = begin_initialization().ok_or(HookError::AlreadyInitialized)?;
    let result = if has_hook_runtime() {
        reactivate_from_payload(payload)
    } else {
        let flip_gate_enabled = payload.flip_gate_enabled;
        let (profile, assignments, runtime) = prepare_cold_install(
            payload,
            || {
                let dwmcore_version = dwmcore_file_version()?;
                let selected = select_profile(dwmcore_version)?;
                log::profile_selected(selected.min_version, dwmcore_version);
                Ok((selected.profile)())
            },
            resolve_profile,
        )?;
        install_prepared(profile, assignments, runtime, flip_gate_enabled)
    };
    finish_initialization_transition(transition, &profile_name, result.is_ok());
    result
}

fn finish_initialization_transition(
    transition: crate::lifecycle::InitializationTransition,
    profile_name: &str,
    succeeded: bool,
) {
    if succeeded {
        transition.commit_active(profile_name);
        return;
    }
    clear_assignments();
    transition.finish_inactive();
}

fn prepare_cold_install<S, R>(
    payload: HookPayload,
    select_profile_for_process: S,
    resolver: R,
) -> Result<(HookProfile, Arc<Vec<LutAssignment>>, HookRuntime), HookError>
where
    S: FnOnce() -> Result<HookProfile, HookError>,
    R: FnOnce(&HookProfile) -> Result<SignatureResolutionReport, HookResolveError>,
{
    let profile = match crate::state::hook_profile() {
        Some(profile) => profile,
        None => select_profile_for_process()?,
    };
    prepare_initial_install(profile, payload, resolver)
}

fn reactivate_from_payload(payload: HookPayload) -> Result<(), HookError> {
    let flip_gate_enabled = payload.flip_gate_enabled;
    let next_assignments = Arc::new(assignments_from_payload(&payload));

    let Some(runtime) = clone_hook_runtime() else {
        return Err(HookError::AlreadyInitialized);
    };
    store_assignments(next_assignments);
    enable_hooks_for_runtime(
        &runtime.minhook,
        &runtime.hooks,
        flip_gate_enabled,
        log::HooksPhase::Reenabled,
    )
}

fn install_prepared(
    profile: HookProfile,
    assignments: Arc<Vec<LutAssignment>>,
    runtime: HookRuntime,
    flip_gate_enabled: bool,
) -> Result<(), HookError> {
    let minhook = runtime.minhook;
    let hooks = runtime.hooks.clone();

    store_hook_runtime(HookRuntime {
        minhook,
        hooks: hooks.clone(),
    })
    .map_err(|runtime| {
        uninitialize(&runtime.minhook, &runtime.hooks);
        HookError::AlreadyInitialized
    })?;
    let _ = store_hook_profile(profile);
    store_assignments(assignments);

    enable_hooks_for_runtime(
        &minhook,
        &hooks,
        flip_gate_enabled,
        log::HooksPhase::Enabled,
    )
}

fn enable_hooks_for_runtime(
    minhook: &MinHookRuntime,
    hooks: &RegisteredHooks,
    flip_gate_enabled: bool,
    phase: log::HooksPhase,
) -> Result<(), HookError> {
    if let Err(error) = enable_hooks(minhook, &hooks.non_flip_gate) {
        return Err(HookError::MinHook(error));
    }
    if let Err(error) = set_flip_gate_enabled(minhook, hooks, flip_gate_enabled) {
        return Err(HookError::MinHook(error));
    }
    if flip_gate_enabled {
        log::hooks(phase, hooks.iter());
    } else {
        log::hooks(phase, hooks.non_flip_gate.iter());
    }
    Ok(())
}

fn prepare_initial_install<F>(
    profile: HookProfile,
    payload: HookPayload,
    resolver: F,
) -> Result<(HookProfile, Arc<Vec<LutAssignment>>, HookRuntime), HookError>
where
    F: FnOnce(&HookProfile) -> Result<SignatureResolutionReport, HookResolveError>,
{
    let assignments = Arc::new(assignments_from_payload(&payload));

    let resolution = resolver(&profile)?;
    log::signatures(&resolution);

    let (minhook, registered_hooks) = register_hooks(&resolution.function_targets)?;
    log::hooks(log::HooksPhase::Created, registered_hooks.iter());

    Ok((
        profile,
        assignments,
        HookRuntime {
            minhook,
            hooks: registered_hooks,
        },
    ))
}

#[cfg(test)]
mod tests {
    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, HookStatus, InitializeStatus, MonitorIdentity,
        MonitorTarget, PayloadAssignment, PayloadLut, ReplaceAssignmentsStatus, ShutdownStatus,
    };

    use crate::DWM_LUT_STATUS;
    use crate::dwmcore::DwmcoreVersionError;
    use crate::resolver::{HookResolveError, SignatureResolutionReport};
    use crate::state::{self, HOOK_GLOBAL_TEST_LOCK};
    use dwm_lut_profile::{
        DwmcoreVersion, HookProfile, HookTarget, ProfileSelectError, SUPPORTED_BUILDS,
    };

    use super::HookError;

    fn test_profile() -> HookProfile {
        (SUPPORTED_BUILDS
            .first()
            .expect("SUPPORTED_BUILDS is non-empty")
            .profiles
            .last()
            .expect("supported build must include profiles")
            .profile)()
    }

    fn test_payload() -> HookPayload {
        HookPayload {
            profile_name: "test".to_string(),
            assignments: vec![PayloadAssignment {
                target: MonitorTarget {
                    identity: MonitorIdentity {
                        adapter_luid: AdapterLuid {
                            high_part: 0,
                            low_part: 1,
                        },
                        target_id: 2,
                    },
                    color_mode: ColorMode::Sdr,
                },
                lut: PayloadLut {
                    size: 2,
                    domain_min: [0.0, 0.0, 0.0],
                    domain_max: [1.0, 1.0, 1.0],
                    values: vec![[0.0, 0.0, 0.0]; 8],
                },
            }],
            flip_gate_enabled: true,
        }
    }

    fn assert_exported_status(expected: HookStatus, profile_name: Option<&str>) {
        let snapshot = DWM_LUT_STATUS.load_for_test();
        assert_eq!(snapshot.sequence % 2, 0);
        assert_eq!(snapshot.hook_status, expected as u32);
        let name_len = snapshot.profile_name_len as usize;
        assert_eq!(
            std::str::from_utf8(&snapshot.profile_name[..name_len]).unwrap(),
            profile_name.unwrap_or_default()
        );
    }

    #[test]
    fn prologue_conflict_stops_before_minhook_registration() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        crate::minhook::reset_test_minhook_behavior(None);

        let error = super::prepare_initial_install(test_profile(), test_payload(), |_| {
            Err(HookResolveError::ConflictingPrologue {
                target: HookTarget::Present,
                rva: 0x1000,
                mismatch_offset: 0,
                expected: 0x40,
                actual: 0xE9,
            })
        })
        .expect_err("prologue conflict should stop initialization");

        assert!(matches!(
            error,
            HookError::Resolve(HookResolveError::ConflictingPrologue {
                target: HookTarget::Present,
                ..
            })
        ));
        let calls = crate::minhook::test_minhook_call_counts();
        assert_eq!(calls.create_calls, 0);
        assert_eq!(calls.enable_calls, 0);
    }

    #[test]
    fn prepare_cold_install_reuses_stored_profile_without_selection() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        let stored_profile = test_profile();
        assert_eq!(state::hook_profile(), Some(stored_profile));

        let error = super::prepare_cold_install(
            test_payload(),
            || -> Result<HookProfile, HookError> {
                panic!("stored profile should bypass profile selection")
            },
            |profile| {
                assert_eq!(*profile, stored_profile);
                Err(HookResolveError::ConflictingPrologue {
                    target: HookTarget::Present,
                    rva: 0x1000,
                    mismatch_offset: 0,
                    expected: 0x40,
                    actual: 0xE9,
                })
            },
        )
        .expect_err("resolver failure should stop the cold install");

        assert!(matches!(
            error,
            HookError::Resolve(HookResolveError::ConflictingPrologue {
                target: HookTarget::Present,
                ..
            })
        ));
        assert_eq!(state::hook_profile(), Some(stored_profile));
        state::reset_state_for_tests();
    }

    #[test]
    fn module_access_failure_has_distinct_initialize_status() {
        let status = InitializeStatus::from(HookResolveError::ModuleAccessFailed {
            module_name: dwm_lut_profile::DWMCORE_MODULE_NAME,
            operation: "map image view",
            error_code: 5,
        });

        assert_eq!(status, InitializeStatus::DwmcoreImageAccessFailed);
    }

    #[test]
    fn profile_select_failures_have_distinct_initialize_statuses() {
        assert_eq!(
            InitializeStatus::from(HookError::from(
                ProfileSelectError::UnsupportedDwmcoreVersion {
                    version: DwmcoreVersion {
                        build: 26100,
                        revision: 0,
                    },
                }
            )),
            InitializeStatus::UnsupportedDwmcoreVersion
        );
        assert_eq!(
            InitializeStatus::from(HookError::from(DwmcoreVersionError::ModuleNotLoaded)),
            InitializeStatus::DwmcoreModuleNotLoaded
        );
        assert_eq!(
            InitializeStatus::from(HookError::from(DwmcoreVersionError::QueryFailed)),
            InitializeStatus::DwmcoreVersionQueryFailed
        );
    }

    #[test]
    fn initialization_failure_is_inactive_and_allows_reinitialization() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        let profile = test_profile();
        crate::minhook::fail_next_test_minhook_enable();
        crate::minhook::fail_next_test_minhook_disable_all();

        super::initialize_with_resolution(
            profile,
            test_payload(),
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect_err("enable failure should abort initialization");

        assert_exported_status(HookStatus::Inactive, None);

        super::initialize_with_resolution(
            profile,
            test_payload(),
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect("reinitialization should succeed");

        assert_exported_status(HookStatus::Active, Some("test"));

        assert_eq!(super::ffi_shutdown(), ShutdownStatus::Success as u32);
        state::reset_state_for_tests();
    }

    #[test]
    fn profile_status_follows_replace_and_shutdown_lifecycle() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        assert_eq!(
            super::ffi_shutdown(),
            ShutdownStatus::AlreadyShutDown as u32
        );
        let profile = test_profile();
        super::initialize_with_resolution(
            profile,
            test_payload(),
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .unwrap();

        let mut replacement = test_payload();
        replacement.profile_name = "updated".to_string();
        super::replace_assignments(replacement).unwrap();
        assert_exported_status(HookStatus::Active, Some("updated"));

        assert_eq!(super::ffi_shutdown(), ShutdownStatus::Success as u32);
        let error = super::replace_assignments(test_payload()).unwrap_err();
        assert_eq!(
            ReplaceAssignmentsStatus::from(&error),
            ReplaceAssignmentsStatus::RuntimeInactive
        );
        assert_exported_status(HookStatus::Inactive, None);
        state::reset_state_for_tests();
    }

    #[test]
    fn replace_failure_with_successful_fail_safe_is_inactive_and_allows_reinitialization() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        let profile = test_profile();
        let mut initial = test_payload();
        initial.profile_name = "original".to_string();
        initial.flip_gate_enabled = false;
        super::initialize_with_resolution(
            profile,
            initial,
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect("initialization should succeed");

        crate::minhook::fail_next_test_minhook_enable();

        let mut replacement = test_payload();
        replacement.profile_name = "updated".to_string();
        replacement.flip_gate_enabled = true;
        replacement.assignments[0].target.identity.target_id = 99;
        let error = super::replace_assignments(replacement.clone()).expect_err(
            "flip-gate failure with successful fail-safe should make the runtime inactive",
        );

        assert_eq!(
            ReplaceAssignmentsStatus::from(&error),
            ReplaceAssignmentsStatus::RuntimeInactive
        );
        let super::ReplaceAssignmentsError::Inactive(error) = error else {
            panic!("expected Inactive");
        };
        assert!(error.fail_safe_succeeded);
        assert_exported_status(HookStatus::Inactive, None);

        super::initialize_with_resolution(
            profile,
            replacement,
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect("reinitialization should succeed");

        assert_exported_status(HookStatus::Active, Some("updated"));

        assert_eq!(super::ffi_shutdown(), ShutdownStatus::Success as u32);

        state::reset_state_for_tests();
    }

    #[test]
    fn replace_failure_with_failed_fail_safe_is_inactive_and_allows_reinitialization() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        let profile = test_profile();
        let mut initial = test_payload();
        initial.profile_name = "original".to_string();
        initial.flip_gate_enabled = false;
        super::initialize_with_resolution(
            profile,
            initial,
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect("initialization should succeed");

        crate::minhook::fail_next_test_minhook_enable();
        crate::minhook::fail_next_test_minhook_disable_all();

        let mut replacement = test_payload();
        replacement.profile_name = "updated".to_string();
        replacement.flip_gate_enabled = true;
        replacement.assignments[0].target.identity.target_id = 99;
        let error = super::replace_assignments(replacement.clone())
            .expect_err("flip-gate and fail-safe failure should make the runtime inactive");

        assert_eq!(
            ReplaceAssignmentsStatus::from(&error),
            ReplaceAssignmentsStatus::RuntimeInactive
        );
        let super::ReplaceAssignmentsError::Inactive(error) = error else {
            panic!("expected Inactive");
        };
        assert!(!error.fail_safe_succeeded);
        assert_exported_status(HookStatus::Inactive, None);

        super::initialize_with_resolution(
            profile,
            replacement,
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect("reinitialization should succeed");

        assert_exported_status(HookStatus::Active, Some("updated"));

        assert_eq!(super::ffi_shutdown(), ShutdownStatus::Success as u32);

        state::reset_state_for_tests();
    }

    #[test]
    fn shutdown_failure_is_inactive_and_allows_reinitialization() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        let profile = test_profile();
        super::initialize_with_resolution(
            profile,
            test_payload(),
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .unwrap();
        assert_exported_status(HookStatus::Active, Some("test"));

        crate::minhook::fail_next_test_minhook_disable_all();
        assert_eq!(
            super::ffi_shutdown(),
            ShutdownStatus::MinHookDisableAllFailed as u32
        );
        assert_exported_status(HookStatus::Inactive, None);

        super::initialize_with_resolution(
            profile,
            test_payload(),
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect("reinitialization should succeed");
        assert_exported_status(HookStatus::Active, Some("test"));

        assert_eq!(super::ffi_shutdown(), ShutdownStatus::Success as u32);
        state::reset_state_for_tests();
    }
}
