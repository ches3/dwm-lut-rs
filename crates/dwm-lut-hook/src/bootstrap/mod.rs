mod error;

pub use error::{HookError, ReplaceAssignmentsError};

use std::sync::Arc;

use dwm_lut_payload::{
    DwmLutPayloadBuffer, HookPayload, InitializeStatus, ReplaceAssignmentsStatus, ShutdownStatus,
    deserialize_payload_buffer,
};

use crate::dwmcore_version::dwmcore_file_version;
use crate::lifecycle::{
    ReplaceAssignmentsStart, ShutdownStart, begin_initialization, begin_replace_assignments,
    begin_shutdown,
};
use crate::log;
use crate::minhook::{
    disable_registered_hooks, enable_registered_hooks, register_plan, unregister_registered_hooks,
};
use crate::resolver::{HookResolveError, SignatureResolutionReport, resolve_profile};
use crate::state::assignments_from_payload;
use crate::state::{
    HookRuntime, HookState, clear_state_after_shutdown, has_retained_state, install_state,
    lock_present_runtime, minhook_cleanup_plan, reactivate_retained_state, replace_lut_assignments,
    retain_state_after_shutdown,
};
use dwm_lut_profile::{HookProfile, select_profile};

#[cfg(test)]
pub(crate) fn initialize_with_resolution(
    profile: HookProfile,
    payload: HookPayload,
    resolution: SignatureResolutionReport,
) -> Result<(), HookError> {
    let profile_name = payload.profile_name.clone();
    let transition = begin_initialization().ok_or(HookError::AlreadyInitialized)?;
    let result = if has_retained_state() {
        reactivate_from_payload(payload)
    } else {
        let flip_gate_enabled = payload.flip_gate_enabled;
        let state = prepare_initial_state(profile, payload, |_| Ok(resolution))?;
        install_prepared_state(state, flip_gate_enabled)
    };
    if result.is_ok() {
        transition.commit_active(&profile_name);
    } else if has_retained_state() {
        transition.finish_shut_down();
    } else {
        drop(transition);
    }
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
        ShutdownStart::NotInitialized => {
            log::shutdown_finished(log::ShutdownFinished::NotInitialized);
            return ShutdownStatus::NotInitialized as u32;
        }
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
        ShutdownStart::AlreadyShutDown => {
            log::shutdown_finished(log::ShutdownFinished::AlreadyShutDown);
            return ShutdownStatus::AlreadyShutDown as u32;
        }
    };

    let Some((minhook, hooks)) = minhook_cleanup_plan() else {
        clear_state_after_shutdown();
        transition.finish_idle();
        log::shutdown_finished(log::ShutdownFinished::StateMissing);
        return ShutdownStatus::Success as u32;
    };

    let cleanup_failures = {
        let _present_guard = lock_present_runtime();
        let _ = crate::d3d11::shutdown_renderer_resources();
        crate::state::set_flip_gate(false);
        crate::desktop_redraw::request_desktop_redraw();
        disable_registered_hooks(&minhook, &hooks)
    };
    for failure in &cleanup_failures {
        log::minhook_cleanup_failed(*failure);
    }

    retain_state_after_shutdown();
    transition.finish_shut_down();
    if !cleanup_failures.is_empty() {
        log::shutdown_finished(log::ShutdownFinished::MinHookCleanupFailed);
        ShutdownStatus::MinHookCleanupFailed as u32
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
    let flip_gate_enabled = payload.flip_gate_enabled;
    let transition = match begin_replace_assignments() {
        ReplaceAssignmentsStart::Started(transition) => transition,
        ReplaceAssignmentsStart::NotInitialized => {
            return Err(ReplaceAssignmentsError::NotInitialized);
        }
        ReplaceAssignmentsStart::AlreadyInProgress => {
            return Err(ReplaceAssignmentsError::AlreadyInProgress);
        }
    };

    let assignments = assignments_from_payload(&payload);

    {
        let _present_guard = lock_present_runtime();
        replace_lut_assignments(profile_name.clone(), assignments)?;
        crate::state::set_flip_gate(flip_gate_enabled);
        let _ = crate::d3d11::shutdown_renderer_resources();
    }
    crate::desktop_redraw::request_desktop_redraw();
    transition.commit_active(&profile_name);
    Ok(())
}

fn initialize_from_payload(payload: HookPayload) -> Result<(), HookError> {
    let profile_name = payload.profile_name.clone();
    let transition = begin_initialization().ok_or(HookError::AlreadyInitialized)?;
    let result = if has_retained_state() {
        reactivate_from_payload(payload)
    } else {
        let dwmcore_version = dwmcore_file_version()?;
        let selected = select_profile(dwmcore_version)?;
        log::profile_selected(selected.min_version, dwmcore_version);
        let profile = (selected.profile)();

        let flip_gate_enabled = payload.flip_gate_enabled;
        let state = prepare_initial_state(profile, payload, resolve_profile)?;
        install_prepared_state(state, flip_gate_enabled)
    };
    if result.is_ok() {
        transition.commit_active(&profile_name);
    } else if has_retained_state() {
        transition.finish_shut_down();
    } else {
        drop(transition);
    }
    result
}

fn reactivate_from_payload(payload: HookPayload) -> Result<(), HookError> {
    let flip_gate_enabled = payload.flip_gate_enabled;
    let assignments = assignments_from_payload(&payload);

    let Some((minhook, hooks)) = reactivate_retained_state(payload.profile_name, assignments)
    else {
        return Err(HookError::AlreadyInitialized);
    };
    if let Err(error) = enable_registered_hooks(&minhook) {
        retain_state_after_shutdown();
        return Err(HookError::MinHook(error));
    }
    crate::state::set_flip_gate(flip_gate_enabled);
    log::hooks(log::HooksPhase::Reenabled, &hooks);
    Ok(())
}

fn install_prepared_state(state: HookState, flip_gate_enabled: bool) -> Result<(), HookError> {
    let minhook = state.runtime.minhook;
    let hooks = state.runtime.hooks.clone();

    install_state(state).map_err(|state| {
        unregister_registered_hooks(&state.runtime.minhook, &state.runtime.hooks);
        HookError::AlreadyInitialized
    })?;

    if let Err(error) = enable_registered_hooks(&minhook) {
        disable_registered_hooks(&minhook, &hooks);
        retain_state_after_shutdown();
        return Err(HookError::MinHook(error));
    }

    crate::state::set_flip_gate(flip_gate_enabled);
    log::hooks(log::HooksPhase::Enabled, &hooks);
    Ok(())
}

fn prepare_initial_state<F>(
    profile: HookProfile,
    payload: HookPayload,
    resolver: F,
) -> Result<HookState, HookError>
where
    F: FnOnce(&HookProfile) -> Result<SignatureResolutionReport, HookResolveError>,
{
    let assignments = assignments_from_payload(&payload);

    let resolution = resolver(&profile)?;
    log::signatures(&resolution);

    let (minhook, registered_hooks) = register_plan(&resolution.function_targets)?;
    log::hooks(log::HooksPhase::Created, &registered_hooks);

    let flip_gate_effects = crate::flip_gate::FlipGateEffects::new(
        resolution.overlay_test_mode,
        resolution.disable_independent_flip,
    );

    Ok(HookState {
        profile_name: payload.profile_name,
        profile,
        assignments: Arc::new(assignments),
        runtime: HookRuntime {
            minhook,
            hooks: registered_hooks,
            flip_gate_effects,
        },
    })
}

#[cfg(test)]
mod tests {
    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, HookStatus, InitializeStatus, MonitorIdentity,
        MonitorTarget, PayloadAssignment, PayloadLut, ReplaceAssignmentsStatus, ShutdownStatus,
    };

    use crate::DWM_LUT_STATUS;
    use crate::dwmcore_version::DwmcoreVersionError;
    use crate::lifecycle;
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
        crate::minhook::reset_test_minhook_behavior(None, None, None, None);

        let error = super::prepare_initial_state(test_profile(), test_payload(), |_| {
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
    fn enable_failure_disables_hooks_and_retains_state() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        crate::minhook::reset_test_minhook_behavior(None, Some(1), None, None);
        let profile = test_profile();

        let error = super::initialize_with_resolution(
            profile,
            test_payload(),
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect_err("enable failure should abort initialization");

        assert!(matches!(error, HookError::MinHook(_)));
        let calls = crate::minhook::test_minhook_call_counts();
        assert!(calls.create_calls > 0);
        assert_eq!(calls.enable_calls, 1);
        assert_eq!(calls.disable_calls, calls.create_calls);
        assert_eq!(calls.remove_calls, 0);
        assert!(!lifecycle::is_runtime_active());
        assert!(state::has_retained_state());
        assert_exported_status(HookStatus::Inactive, None);

        state::reset_state_for_tests();
    }

    #[test]
    fn profile_status_follows_replace_and_shutdown_lifecycle() {
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

        let mut replacement = test_payload();
        replacement.profile_name = "updated".to_string();
        super::replace_assignments(replacement).unwrap();
        assert_exported_status(HookStatus::Active, Some("updated"));

        assert_eq!(super::ffi_shutdown(), ShutdownStatus::Success as u32);
        let error = super::replace_assignments(test_payload()).unwrap_err();
        assert_eq!(
            ReplaceAssignmentsStatus::from(&error),
            ReplaceAssignmentsStatus::NotInitialized
        );
        assert_exported_status(HookStatus::Inactive, None);
        state::reset_state_for_tests();
    }

    #[test]
    fn shutdown_cleanup_failure_still_publishes_inactive() {
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

        crate::minhook::reset_test_minhook_behavior(None, None, Some(1), None);
        assert_eq!(
            super::ffi_shutdown(),
            ShutdownStatus::MinHookCleanupFailed as u32
        );
        assert_exported_status(HookStatus::Inactive, None);
        state::reset_state_for_tests();
    }

    #[test]
    fn shutdown_disables_hooks_and_reinitialization_reuses_registration() {
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
        .expect("initial initialization should succeed");
        let initialized_calls = crate::minhook::test_minhook_call_counts();
        assert_exported_status(HookStatus::Active, Some("test"));

        assert_eq!(super::ffi_shutdown(), ShutdownStatus::Success as u32);
        let shutdown_calls = crate::minhook::test_minhook_call_counts();
        assert!(!lifecycle::is_runtime_active());
        assert!(state::hook_profile().is_none());
        assert_eq!(shutdown_calls.disable_calls, initialized_calls.create_calls);
        assert_eq!(shutdown_calls.remove_calls, 0);
        assert_eq!(shutdown_calls.uninitialize_calls, 0);
        assert_exported_status(HookStatus::Inactive, None);

        super::initialize_with_resolution(
            profile,
            test_payload(),
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect("reinitialization should reuse registered hooks");
        let reinitialized_calls = crate::minhook::test_minhook_call_counts();
        assert!(lifecycle::is_runtime_active());
        assert_eq!(
            reinitialized_calls.create_calls,
            initialized_calls.create_calls
        );
        assert_eq!(
            reinitialized_calls.enable_calls,
            initialized_calls.enable_calls + 1
        );
        assert_eq!(reinitialized_calls.remove_calls, 0);
        assert_eq!(reinitialized_calls.uninitialize_calls, 0);
        assert_exported_status(HookStatus::Active, Some("test"));

        assert_eq!(super::ffi_shutdown(), ShutdownStatus::Success as u32);
        let repeated_shutdown_calls = crate::minhook::test_minhook_call_counts();
        assert_eq!(
            repeated_shutdown_calls.create_calls,
            initialized_calls.create_calls
        );
        assert_eq!(repeated_shutdown_calls.remove_calls, 0);
        assert_eq!(repeated_shutdown_calls.uninitialize_calls, 0);
        assert_exported_status(HookStatus::Inactive, None);

        state::reset_state_for_tests();
    }
}
