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
    MinHookRuntime, RegisteredHook, disable_registered_hooks, enable_hooks_for_cleanup,
    enable_registered_hooks, flip_gate_hooks, non_flip_gate_hooks, register_plan,
    set_flip_gate_hooks_enabled, unregister_registered_hooks,
};
use crate::resolver::{HookResolveError, SignatureResolutionReport, resolve_profile};
use crate::state::assignments_from_payload;
use crate::state::{
    HookRuntime, LutConfig, clear_hook_runtime, clear_lut_config, clone_hook_runtime,
    has_hook_runtime, lock_present_runtime, lut_config, store_flip_gate_enabled,
    store_hook_profile, store_hook_runtime, store_lut_config,
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
        let (profile, config, runtime) =
            prepare_cold_install(payload, || Ok(profile), |_| Ok(resolution))?;
        install_prepared(profile, config, runtime, flip_gate_enabled)
    };
    if result.is_ok() {
        transition.commit_active(&profile_name);
    } else if has_hook_runtime() {
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

    let Some(runtime) = clone_hook_runtime() else {
        clear_lut_config();
        store_flip_gate_enabled(false);
        clear_hook_runtime();
        transition.finish_idle();
        log::shutdown_finished(log::ShutdownFinished::HookRuntimeMissing);
        return ShutdownStatus::Success as u32;
    };

    let cleanup_failures = {
        let _present_guard = lock_present_runtime();
        let _ = crate::d3d11::shutdown_renderer_resources();
        crate::desktop_redraw::request_desktop_redraw();
        disable_registered_hooks(&runtime.minhook, &runtime.hooks)
    };
    for failure in &cleanup_failures {
        log::minhook_cleanup_failed(*failure);
    }

    clear_lut_config();
    store_flip_gate_enabled(false);
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
    let next_flip_gate_enabled = payload.flip_gate_enabled;
    let transition = match begin_replace_assignments() {
        ReplaceAssignmentsStart::Started(transition) => transition,
        ReplaceAssignmentsStart::NotInitialized => {
            return Err(ReplaceAssignmentsError::NotInitialized);
        }
        ReplaceAssignmentsStart::AlreadyInProgress => {
            return Err(ReplaceAssignmentsError::AlreadyInProgress);
        }
    };

    let config = LutConfig {
        profile_name: profile_name.clone(),
        assignments: Arc::new(assignments_from_payload(&payload)),
    };
    let previous_flip_gate_enabled = crate::state::flip_gate_enabled();

    {
        let _present_guard = lock_present_runtime();
        let Some(runtime) = clone_hook_runtime() else {
            return Err(ReplaceAssignmentsError::NotInitialized);
        };
        let Some(previous) = lut_config() else {
            return Err(ReplaceAssignmentsError::NotInitialized);
        };

        store_lut_config(config);
        if let Err(error) =
            set_flip_gate_hooks_enabled(&runtime.minhook, &runtime.hooks, next_flip_gate_enabled)
        {
            store_lut_config(Arc::unwrap_or_clone(previous));
            return Err(flip_gate_hook_failure(
                &runtime.minhook,
                &runtime.hooks,
                previous_flip_gate_enabled,
                error,
            ));
        }
        store_flip_gate_enabled(next_flip_gate_enabled);
        let _ = crate::d3d11::shutdown_renderer_resources();
    }
    crate::desktop_redraw::request_desktop_redraw();
    transition.commit_active(&profile_name);
    Ok(())
}

enum FlipGateReconcileOutcome {
    Restored,
    FailSafeDisabled,
}

fn reconcile_or_fail_safe_flip_gate(
    minhook: &MinHookRuntime,
    hooks: &[RegisteredHook],
    previous_enabled: bool,
) -> FlipGateReconcileOutcome {
    let flip_hooks = flip_gate_hooks(hooks);
    let reconcile_failures = if previous_enabled {
        enable_hooks_for_cleanup(minhook, &flip_hooks)
    } else {
        disable_registered_hooks(minhook, &flip_hooks)
    };
    if reconcile_failures.is_empty() {
        return FlipGateReconcileOutcome::Restored;
    }
    for failure in &reconcile_failures {
        log::minhook_cleanup_failed(*failure);
    }
    let fail_safe_failures = disable_registered_hooks(minhook, &flip_hooks);
    for failure in &fail_safe_failures {
        log::minhook_cleanup_failed(*failure);
    }
    store_flip_gate_enabled(false);
    FlipGateReconcileOutcome::FailSafeDisabled
}

fn flip_gate_hook_failure(
    minhook: &MinHookRuntime,
    hooks: &[RegisteredHook],
    previous_flip_gate_enabled: bool,
    error: crate::minhook::MinHookError,
) -> ReplaceAssignmentsError {
    if !error.has_cleanup_failures() {
        return ReplaceAssignmentsError::MinHook(error);
    }
    match reconcile_or_fail_safe_flip_gate(minhook, hooks, previous_flip_gate_enabled) {
        FlipGateReconcileOutcome::Restored => ReplaceAssignmentsError::MinHook(error),
        FlipGateReconcileOutcome::FailSafeDisabled => ReplaceAssignmentsError::MinHookCleanupFailed,
    }
}

fn initialize_from_payload(payload: HookPayload) -> Result<(), HookError> {
    let profile_name = payload.profile_name.clone();
    let transition = begin_initialization().ok_or(HookError::AlreadyInitialized)?;
    let result = if has_hook_runtime() {
        reactivate_from_payload(payload)
    } else {
        let flip_gate_enabled = payload.flip_gate_enabled;
        let (profile, config, runtime) = prepare_cold_install(
            payload,
            || {
                let dwmcore_version = dwmcore_file_version()?;
                let selected = select_profile(dwmcore_version)?;
                log::profile_selected(selected.min_version, dwmcore_version);
                Ok((selected.profile)())
            },
            resolve_profile,
        )?;
        install_prepared(profile, config, runtime, flip_gate_enabled)
    };
    if result.is_ok() {
        transition.commit_active(&profile_name);
    } else if has_hook_runtime() {
        transition.finish_shut_down();
    } else {
        drop(transition);
    }
    result
}

fn prepare_cold_install<S, R>(
    payload: HookPayload,
    select_profile_for_process: S,
    resolver: R,
) -> Result<(HookProfile, LutConfig, HookRuntime), HookError>
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
    let assignments = Arc::new(assignments_from_payload(&payload));
    let config = LutConfig {
        profile_name: payload.profile_name,
        assignments,
    };

    let Some(runtime) = clone_hook_runtime() else {
        return Err(HookError::AlreadyInitialized);
    };
    store_flip_gate_enabled(false);
    store_lut_config(config);
    enable_hooks_for_runtime(
        &runtime.minhook,
        &runtime.hooks,
        flip_gate_enabled,
        log::HooksPhase::Reenabled,
    )
}

fn install_prepared(
    profile: HookProfile,
    config: LutConfig,
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
        unregister_registered_hooks(&runtime.minhook, &runtime.hooks);
        HookError::AlreadyInitialized
    })?;
    let _ = store_hook_profile(profile);
    store_lut_config(config);
    store_flip_gate_enabled(false);

    enable_hooks_for_runtime(
        &minhook,
        &hooks,
        flip_gate_enabled,
        log::HooksPhase::Enabled,
    )
}

fn enable_hooks_for_runtime(
    minhook: &MinHookRuntime,
    hooks: &[RegisteredHook],
    flip_gate_enabled: bool,
    phase: log::HooksPhase,
) -> Result<(), HookError> {
    let non_flip_hooks = non_flip_gate_hooks(hooks);
    if let Err(error) = enable_registered_hooks(minhook, &non_flip_hooks) {
        disable_registered_hooks(minhook, hooks);
        clear_lut_config();
        return Err(HookError::MinHook(error));
    }
    if let Err(error) = set_flip_gate_hooks_enabled(minhook, hooks, flip_gate_enabled) {
        disable_registered_hooks(minhook, hooks);
        clear_lut_config();
        return Err(HookError::MinHook(error));
    }
    store_flip_gate_enabled(flip_gate_enabled);
    if flip_gate_enabled {
        log::hooks(phase, hooks);
    } else {
        log::hooks(phase, &non_flip_hooks);
    }
    Ok(())
}

fn prepare_initial_install<F>(
    profile: HookProfile,
    payload: HookPayload,
    resolver: F,
) -> Result<(HookProfile, LutConfig, HookRuntime), HookError>
where
    F: FnOnce(&HookProfile) -> Result<SignatureResolutionReport, HookResolveError>,
{
    let assignments = Arc::new(assignments_from_payload(&payload));
    let config = LutConfig {
        profile_name: payload.profile_name,
        assignments,
    };

    let resolution = resolver(&profile)?;
    log::signatures(&resolution);

    let (minhook, registered_hooks) = register_plan(&resolution.function_targets)?;
    log::hooks(log::HooksPhase::Created, &registered_hooks);

    Ok((
        profile,
        config,
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
    fn enable_failure_disables_hooks_and_keeps_runtime() {
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
        assert!(state::has_hook_runtime());
        assert!(state::assignments().is_none());
        assert!(state::lut_profile_name().is_none());
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
    fn replace_assignments_does_not_store_when_config_missing() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        let profile = test_profile();
        let mut initial = test_payload();
        initial.profile_name = "original".to_string();
        super::initialize_with_resolution(
            profile,
            initial,
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect("initialization should succeed");
        state::clear_lut_config();

        let mut replacement = test_payload();
        replacement.profile_name = "updated".to_string();
        let error = super::replace_assignments(replacement)
            .expect_err("missing config should abort before store");

        assert_eq!(
            ReplaceAssignmentsStatus::from(&error),
            ReplaceAssignmentsStatus::NotInitialized
        );
        assert!(state::assignments().is_none());
        assert!(state::lut_profile_name().is_none());
        assert_exported_status(HookStatus::Inactive, None);

        state::reset_state_for_tests();
    }

    #[test]
    fn replace_assignments_rolls_back_when_flip_gate_enable_fails() {
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
        let original_assignments = state::assignments().expect("assignments should be installed");
        assert_exported_status(HookStatus::Active, Some("original"));

        let next_enable = crate::minhook::test_minhook_call_counts().enable_calls + 1;
        crate::minhook::set_test_minhook_enable_fail_on(Some(next_enable));

        let mut replacement = test_payload();
        replacement.profile_name = "updated".to_string();
        replacement.flip_gate_enabled = true;
        replacement.assignments[0].target.identity.target_id = 99;
        let error = super::replace_assignments(replacement)
            .expect_err("flip-gate enable failure should abort replacement");

        assert_eq!(
            ReplaceAssignmentsStatus::from(&error),
            ReplaceAssignmentsStatus::MinHookFailed
        );
        assert!(lifecycle::is_runtime_active());
        assert_eq!(state::lut_profile_name().as_deref(), Some("original"));
        assert_eq!(state::assignments(), Some(original_assignments));
        assert!(!state::flip_gate_enabled());
        assert_eq!(crate::minhook::test_enabled_target_count(), 1);
        assert_exported_status(HookStatus::Active, Some("original"));

        state::reset_state_for_tests();
    }

    #[test]
    fn replace_assignments_rolls_back_when_flip_gate_disable_fails() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        let profile = test_profile();
        let mut initial = test_payload();
        initial.profile_name = "original".to_string();
        initial.flip_gate_enabled = true;
        super::initialize_with_resolution(
            profile,
            initial,
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect("initialization should succeed");
        let original_assignments = state::assignments().expect("assignments should be installed");
        let enabled_before = crate::minhook::test_enabled_target_count();
        assert!(enabled_before > 1);
        assert!(state::flip_gate_enabled());
        assert_exported_status(HookStatus::Active, Some("original"));

        let next_disable = crate::minhook::test_minhook_call_counts().disable_calls + 1;
        crate::minhook::set_test_minhook_disable_fail_on(Some(next_disable));

        let mut replacement = test_payload();
        replacement.profile_name = "updated".to_string();
        replacement.flip_gate_enabled = false;
        replacement.assignments[0].target.identity.target_id = 99;
        let error = super::replace_assignments(replacement)
            .expect_err("flip-gate disable failure should abort replacement");

        assert_eq!(
            ReplaceAssignmentsStatus::from(&error),
            ReplaceAssignmentsStatus::MinHookFailed
        );
        assert!(lifecycle::is_runtime_active());
        assert_eq!(state::lut_profile_name().as_deref(), Some("original"));
        assert_eq!(state::assignments(), Some(original_assignments));
        assert!(state::flip_gate_enabled());
        assert_eq!(crate::minhook::test_enabled_target_count(), enabled_before);
        assert_exported_status(HookStatus::Active, Some("original"));

        state::reset_state_for_tests();
    }

    #[test]
    fn replace_assignments_reconciles_when_flip_gate_enable_cleanup_fails() {
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
        let original_assignments = state::assignments().expect("assignments should be installed");
        assert_eq!(crate::minhook::test_enabled_target_count(), 1);

        let counts = crate::minhook::test_minhook_call_counts();
        crate::minhook::set_test_minhook_enable_fail_on(Some(counts.enable_calls + 2));
        crate::minhook::set_test_minhook_disable_fail_on(Some(counts.disable_calls + 1));

        let mut replacement = test_payload();
        replacement.profile_name = "updated".to_string();
        replacement.flip_gate_enabled = true;
        replacement.assignments[0].target.identity.target_id = 99;
        let error = super::replace_assignments(replacement)
            .expect_err("flip-gate enable cleanup failure should abort replacement");

        assert_eq!(
            ReplaceAssignmentsStatus::from(&error),
            ReplaceAssignmentsStatus::MinHookFailed
        );
        assert!(matches!(
            error,
            super::ReplaceAssignmentsError::MinHook(ref minhook)
            if minhook.has_cleanup_failures()
        ));
        assert_eq!(state::lut_profile_name().as_deref(), Some("original"));
        assert_eq!(state::assignments(), Some(original_assignments));
        assert!(!state::flip_gate_enabled());
        assert_eq!(crate::minhook::test_enabled_target_count(), 1);
        assert_exported_status(HookStatus::Active, Some("original"));

        state::reset_state_for_tests();
    }

    #[test]
    fn replace_assignments_disables_flip_gate_when_reconcile_fails() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        let profile = test_profile();
        let mut initial = test_payload();
        initial.profile_name = "original".to_string();
        initial.flip_gate_enabled = true;
        super::initialize_with_resolution(
            profile,
            initial,
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect("initialization should succeed");
        let original_assignments = state::assignments().expect("assignments should be installed");
        let enabled_before = crate::minhook::test_enabled_target_count();
        assert!(enabled_before > 1);

        let counts = crate::minhook::test_minhook_call_counts();
        crate::minhook::set_test_minhook_disable_fail_on(Some(counts.disable_calls + 2));
        crate::minhook::set_test_minhook_enable_fail_from(Some(counts.enable_calls + 1));

        let mut replacement = test_payload();
        replacement.profile_name = "updated".to_string();
        replacement.flip_gate_enabled = false;
        replacement.assignments[0].target.identity.target_id = 99;
        let error = super::replace_assignments(replacement)
            .expect_err("flip-gate reconcile failure should disable gates");

        assert_eq!(
            ReplaceAssignmentsStatus::from(&error),
            ReplaceAssignmentsStatus::MinHookCleanupFailed
        );
        assert!(matches!(
            error,
            super::ReplaceAssignmentsError::MinHookCleanupFailed
        ));
        assert_eq!(state::lut_profile_name().as_deref(), Some("original"));
        assert_eq!(state::assignments(), Some(original_assignments));
        assert!(!state::flip_gate_enabled());
        assert_eq!(crate::minhook::test_enabled_target_count(), 1);
        assert_exported_status(HookStatus::Active, Some("original"));

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
        assert!(state::hook_profile().is_some());
        assert!(state::assignments().is_none());
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
            initialized_calls.enable_calls * 2
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
