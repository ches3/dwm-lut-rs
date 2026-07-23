mod error;

pub use error::{HookError, ReplaceAssignmentsError};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dwm_lut_payload::{
    DwmLutPayloadBuffer, HookPayload, InitializeStatus, ReplaceAssignmentsStatus, ShutdownStatus,
    deserialize_payload_buffer,
};

use crate::flip_gate::FlipGateEffects;
use crate::log;
use crate::minhook::{
    disable_registered_hooks, enable_registered_hooks, register_plan, unregister_registered_hooks,
};
use crate::profile::{HookProfile, dwmcore_file_version, select_versioned_profile};
use crate::resolver::{HookResolveError, SignatureResolutionReport, resolve_profile};
use crate::state::assignments_from_payload;
use crate::state::{
    HookRuntime, HookState, ReplaceAssignmentsStart, ShutdownStart, begin_replace_assignments,
    begin_shutdown, can_initialize, clear_state_after_shutdown, finish_reactivation,
    finish_replace_assignments, finish_shutdown, has_retained_state, install_state,
    lock_present_runtime, minhook_cleanup_plan, reactivate_retained_state, replace_lut_assignments,
    retain_state_after_shutdown,
};

static INITIALIZATION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct InitializationGuard;

impl Drop for InitializationGuard {
    fn drop(&mut self) {
        INITIALIZATION_IN_PROGRESS.store(false, Ordering::Release);
    }
}

struct ReplaceAssignmentsGuard;

impl Drop for ReplaceAssignmentsGuard {
    fn drop(&mut self) {
        finish_replace_assignments();
    }
}

fn enter_initialization() -> Result<InitializationGuard, HookError> {
    if !can_initialize() {
        return Err(HookError::AlreadyInitialized);
    }

    if INITIALIZATION_IN_PROGRESS
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(HookError::AlreadyInitialized);
    }

    if !can_initialize() {
        INITIALIZATION_IN_PROGRESS.store(false, Ordering::Release);
        return Err(HookError::AlreadyInitialized);
    }

    Ok(InitializationGuard)
}

fn enter_replace_assignments() -> Result<ReplaceAssignmentsGuard, ReplaceAssignmentsError> {
    match begin_replace_assignments() {
        ReplaceAssignmentsStart::Started => Ok(ReplaceAssignmentsGuard),
        ReplaceAssignmentsStart::NotInitialized => Err(ReplaceAssignmentsError::NotInitialized),
        ReplaceAssignmentsStart::AlreadyInProgress => {
            Err(ReplaceAssignmentsError::AlreadyInProgress)
        }
    }
}

#[cfg(test)]
pub(crate) fn reset_initialization_guard_for_tests() {
    INITIALIZATION_IN_PROGRESS.store(false, Ordering::Release);
}

#[cfg(test)]
pub(crate) fn initialize_with_resolution(
    profile: HookProfile,
    payload: HookPayload,
    resolution: SignatureResolutionReport,
) -> Result<(), HookError> {
    let _guard = enter_initialization()?;
    if has_retained_state() {
        return reactivate_from_payload(payload);
    }
    let state = prepare_initial_state(profile, payload, |_| Ok(resolution))?;
    install_prepared_state(state)
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
    if INITIALIZATION_IN_PROGRESS.load(Ordering::Acquire) {
        log::shutdown_finished(log::ShutdownFinished::InitializationInProgress);
        return ShutdownStatus::AlreadyInProgress as u32;
    }

    match begin_shutdown() {
        ShutdownStart::Started => {}
        ShutdownStart::NotInitialized => {
            log::shutdown_finished(log::ShutdownFinished::NotInitialized);
            return ShutdownStatus::NotInitialized as u32;
        }
        ShutdownStart::AlreadyInProgress => {
            log::shutdown_finished(log::ShutdownFinished::ShutdownInProgress);
            return ShutdownStatus::AlreadyInProgress as u32;
        }
        ShutdownStart::AlreadyShutDown => {
            log::shutdown_finished(log::ShutdownFinished::AlreadyShutDown);
            return ShutdownStatus::AlreadyShutDown as u32;
        }
    }

    let Some((minhook, hooks)) = minhook_cleanup_plan() else {
        clear_state_after_shutdown();
        log::shutdown_finished(log::ShutdownFinished::StateMissing);
        return ShutdownStatus::Success as u32;
    };

    let cleanup_failures = {
        let _present_guard = lock_present_runtime();
        let _ = crate::d3d11::shutdown_renderer_resources();
        crate::state::clear_present_session();
        crate::desktop_redraw::request_desktop_redraw();
        disable_registered_hooks(&minhook, &hooks)
    };
    for failure in &cleanup_failures {
        log::minhook_cleanup_failed(*failure);
    }

    retain_state_after_shutdown();
    finish_shutdown();
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
    if INITIALIZATION_IN_PROGRESS.load(Ordering::Acquire) {
        return Err(ReplaceAssignmentsError::AlreadyInProgress);
    }
    let _guard = enter_replace_assignments()?;

    let assignments = assignments_from_payload(&payload);

    {
        let _present_guard = lock_present_runtime();
        replace_lut_assignments(payload, assignments)?;
        let _ = crate::d3d11::shutdown_renderer_resources();
    }
    crate::desktop_redraw::request_desktop_redraw();
    Ok(())
}

fn initialize_from_payload(payload: HookPayload) -> Result<(), HookError> {
    let _guard = enter_initialization()?;

    if has_retained_state() {
        return reactivate_from_payload(payload);
    }

    let dwmcore_version = dwmcore_file_version()?;
    let entry = select_versioned_profile(dwmcore_version)?;
    log::profile_selected(entry.min_version, dwmcore_version);
    let profile = (entry.profile)();

    let state = prepare_initial_state(profile, payload, resolve_profile)?;
    install_prepared_state(state)
}

fn reactivate_from_payload(payload: HookPayload) -> Result<(), HookError> {
    let assignments = assignments_from_payload(&payload);

    let Some((minhook, hooks)) = reactivate_retained_state(payload, assignments) else {
        return Err(HookError::AlreadyInitialized);
    };
    if let Err(error) = enable_registered_hooks(&minhook) {
        retain_state_after_shutdown();
        return Err(HookError::MinHook(error));
    }
    finish_reactivation();
    log::hooks(log::HooksPhase::Reenabled, &hooks);
    Ok(())
}

fn install_prepared_state(state: HookState) -> Result<(), HookError> {
    let minhook = state.runtime.minhook;
    let hooks = state.runtime.hooks.clone();

    install_state(state).map_err(|state| {
        unregister_registered_hooks(&state.runtime.minhook, &state.runtime.hooks);
        HookError::AlreadyInitialized
    })?;

    if let Err(error) = enable_registered_hooks(&minhook) {
        finish_shutdown();
        disable_registered_hooks(&minhook, &hooks);
        retain_state_after_shutdown();
        return Err(HookError::MinHook(error));
    }

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

    let flip_gate_effects = FlipGateEffects::new(
        resolution.overlay_test_mode,
        resolution.disable_independent_flip,
    );

    Ok(HookState {
        payload,
        profile,
        assignments: Arc::new(assignments),
        contexts: Default::default(),
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
        AdapterLuid, ColorMode, HookPayload, InitializeStatus, MonitorIdentity, MonitorTarget,
        PayloadAssignment, PayloadLut, ShutdownStatus,
    };

    use crate::profile::{DwmcoreVersion, HookProfile, HookTarget, ProfileSelectError};
    use crate::resolver::{HookResolveError, SignatureResolutionReport};
    use crate::state::{self, HOOK_GLOBAL_TEST_LOCK};

    use super::HookError;

    fn test_profile() -> HookProfile {
        crate::profile::latest_registered_profile()
    }

    fn test_payload() -> HookPayload {
        HookPayload {
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
        }
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
            module_name: crate::profile::HOOK_MODULE_NAME,
            operation: "map image view",
            error_code: 5,
        });

        assert_eq!(status, InitializeStatus::DwmcoreImageAccessFailed);
    }

    #[test]
    fn profile_select_failures_have_distinct_initialize_statuses() {
        let cases = [
            (
                ProfileSelectError::UnsupportedDwmcoreVersion {
                    version: DwmcoreVersion {
                        build: 26100,
                        revision: 0,
                    },
                },
                InitializeStatus::UnsupportedDwmcoreVersion,
            ),
            (
                ProfileSelectError::DwmcoreModuleNotLoaded,
                InitializeStatus::DwmcoreModuleNotLoaded,
            ),
            (
                ProfileSelectError::DwmcoreVersionQueryFailed,
                InitializeStatus::DwmcoreVersionQueryFailed,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(InitializeStatus::from(HookError::from(error)), expected);
        }
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
        assert!(!state::is_initialized());
        assert!(state::has_retained_state());

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

        assert_eq!(super::ffi_shutdown(), ShutdownStatus::Success as u32);
        let shutdown_calls = crate::minhook::test_minhook_call_counts();
        assert!(!state::is_initialized());
        assert!(state::hook_profile().is_none());
        assert!(!state::has_active_contexts());
        assert_eq!(shutdown_calls.disable_calls, initialized_calls.create_calls);
        assert_eq!(shutdown_calls.remove_calls, 0);
        assert_eq!(shutdown_calls.uninitialize_calls, 0);

        super::initialize_with_resolution(
            profile,
            test_payload(),
            SignatureResolutionReport::synthetic_for_tests(&profile),
        )
        .expect("reinitialization should reuse registered hooks");
        let reinitialized_calls = crate::minhook::test_minhook_call_counts();
        assert!(state::is_initialized());
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

        assert_eq!(super::ffi_shutdown(), ShutdownStatus::Success as u32);
        let repeated_shutdown_calls = crate::minhook::test_minhook_call_counts();
        assert_eq!(
            repeated_shutdown_calls.create_calls,
            initialized_calls.create_calls
        );
        assert_eq!(repeated_shutdown_calls.remove_calls, 0);
        assert_eq!(repeated_shutdown_calls.uninitialize_calls, 0);

        state::reset_state_for_tests();
    }
}
