use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use dwm_lut_payload::{
    DwmLutPayloadBuffer, HookPayload, InitializeStatus, PayloadError, ReplaceAssignmentsStatus,
    ResolveFailureKind, ShutdownStatus, deserialize_payload_buffer,
};

use crate::flip_gate::FlipGateEffects;
use crate::log;
use std::sync::Arc;

use crate::minhook::{
    MinHookError, disable_registered_hooks, enable_registered_hooks, register_plan,
    unregister_registered_hooks,
};
use crate::profile::{
    HookProfile, ProfileSelectError, dwmcore_file_version, select_versioned_profile,
};
use crate::state::{LutAssignment, assignments_from_payload};

use crate::resolver::{HookResolveError, SignatureResolutionReport, resolve_profile};
use crate::state::{
    HookRegistrationPlan, HookRuntime, HookState, ReplaceAssignmentsStart,
    ReplaceLutAssignmentsError, ShutdownStart, begin_replace_assignments, begin_shutdown,
    can_initialize, clear_state_after_shutdown, finish_reactivation, finish_replace_assignments,
    finish_shutdown, has_retained_state, install_state, lock_present_runtime, minhook_cleanup_plan,
    reactivate_retained_state, replace_lut_assignments, retain_state_after_shutdown,
};

static INITIALIZATION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct InitializationGuard;

impl Drop for InitializationGuard {
    fn drop(&mut self) {
        clear_initialization_in_progress();
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

    if !mark_initialization_in_progress() {
        return Err(HookError::AlreadyInitialized);
    }

    if !can_initialize() {
        clear_initialization_in_progress();
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

fn is_initialization_in_progress() -> bool {
    INITIALIZATION_IN_PROGRESS.load(Ordering::Acquire)
}

fn mark_initialization_in_progress() -> bool {
    INITIALIZATION_IN_PROGRESS
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn clear_initialization_in_progress() {
    INITIALIZATION_IN_PROGRESS.store(false, Ordering::Release);
}

#[cfg(test)]
pub(crate) fn reset_initialization_guard_for_tests() {
    clear_initialization_in_progress();
}

#[derive(Debug)]
pub enum HookError {
    AlreadyInitialized,
    ProfileSelect(ProfileSelectError),
    Payload(PayloadError),
    MinHook(MinHookError),
    Resolve(HookResolveError),
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => write!(f, "hook is already initialized"),
            Self::ProfileSelect(error) => write!(f, "{error}"),
            Self::Payload(error) => write!(f, "{error}"),
            Self::MinHook(error) => write!(f, "{error}"),
            Self::Resolve(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HookError {}

impl From<HookResolveError> for HookError {
    fn from(value: HookResolveError) -> Self {
        Self::Resolve(value)
    }
}

impl From<PayloadError> for HookError {
    fn from(value: PayloadError) -> Self {
        Self::Payload(value)
    }
}

impl From<MinHookError> for HookError {
    fn from(value: MinHookError) -> Self {
        Self::MinHook(value)
    }
}

impl From<ProfileSelectError> for HookError {
    fn from(value: ProfileSelectError) -> Self {
        Self::ProfileSelect(value)
    }
}

#[derive(Debug)]
pub enum ReplaceAssignmentsError {
    NotInitialized,
    AlreadyInProgress,
    Payload(PayloadError),
    State(ReplaceLutAssignmentsError),
}

impl fmt::Display for ReplaceAssignmentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "hook is not initialized"),
            Self::AlreadyInProgress => write!(f, "hook initialization or shutdown is in progress"),
            Self::Payload(error) => write!(f, "{error}"),
            Self::State(ReplaceLutAssignmentsError::NotInitialized) => {
                write!(f, "hook is not initialized")
            }
        }
    }
}

impl std::error::Error for ReplaceAssignmentsError {}

impl From<PayloadError> for ReplaceAssignmentsError {
    fn from(value: PayloadError) -> Self {
        Self::Payload(value)
    }
}

impl From<ReplaceLutAssignmentsError> for ReplaceAssignmentsError {
    fn from(value: ReplaceLutAssignmentsError) -> Self {
        Self::State(value)
    }
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
    let state = prepare_initial_state_with_resolution(profile, payload, resolution)?;
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
    if is_initialization_in_progress() {
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
            let status = map_replace_assignments_error(&error);
            log::replace_assignments_failed(status, &error);
            status as u32
        }
    }
}

fn replace_assignments(payload: HookPayload) -> Result<(), ReplaceAssignmentsError> {
    if is_initialization_in_progress() {
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

fn map_replace_assignments_error(error: &ReplaceAssignmentsError) -> ReplaceAssignmentsStatus {
    match error {
        ReplaceAssignmentsError::NotInitialized
        | ReplaceAssignmentsError::State(ReplaceLutAssignmentsError::NotInitialized) => {
            ReplaceAssignmentsStatus::NotInitialized
        }
        ReplaceAssignmentsError::AlreadyInProgress => ReplaceAssignmentsStatus::AlreadyInProgress,
        ReplaceAssignmentsError::Payload(error) => ReplaceAssignmentsStatus::from(error),
    }
}

fn initialize_from_payload(payload: HookPayload) -> Result<(), HookError> {
    let _guard = enter_initialization()?;

    if has_retained_state() {
        return reactivate_from_payload(payload);
    }

    let state = prepare_initial_state_from_payload(payload)?;
    install_prepared_state(state)
}

fn selected_profile() -> Result<HookProfile, HookError> {
    let dwmcore_version = dwmcore_file_version()?;
    let entry = select_versioned_profile(dwmcore_version)?;
    log::profile_selected(entry.min_version, dwmcore_version);
    Ok((entry.profile)())
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
        rollback_registered_state_hooks(&state);
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

fn rollback_registered_state_hooks(state: &HookState) {
    unregister_registered_hooks(&state.runtime.minhook, &state.runtime.hooks);
}

fn prepare_initial_state_from_payload(payload: HookPayload) -> Result<HookState, HookError> {
    let profile = selected_profile()?;
    prepare_initial_state_from_payload_with_profile_resolver(profile, payload, resolve_profile)
}

fn prepare_initial_state_from_payload_with_profile_resolver<F>(
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

    finalize_initial_state(payload, profile, resolution, assignments)
}

#[cfg(test)]
pub(crate) fn prepare_initial_state_with_resolution(
    profile: HookProfile,
    payload: HookPayload,
    resolution: SignatureResolutionReport,
) -> Result<HookState, HookError> {
    prepare_initial_state_from_payload_with_profile_resolver(profile, payload, |_| Ok(resolution))
}

fn finalize_initial_state(
    payload: HookPayload,
    profile: HookProfile,
    resolution: SignatureResolutionReport,
    assignments: Vec<LutAssignment>,
) -> Result<HookState, HookError> {
    let registration_plan = HookRegistrationPlan::from_resolution(&resolution);
    let (minhook, registered_hooks) = register_plan(&registration_plan)?;
    log::hooks(log::HooksPhase::Created, &registered_hooks);

    let overlay_test_mode_address = resolution
        .targets
        .iter()
        .find(|target| target.target == crate::profile::HookTarget::OverlayTestMode)
        .map(|target| target.address)
        .filter(|address| *address != 0);
    let disable_independent_flip_address = resolution
        .targets
        .iter()
        .find(|target| target.target == crate::profile::HookTarget::DisableIndependentFlip)
        .map(|target| target.address)
        .filter(|address| *address != 0);
    let flip_gate_effects =
        FlipGateEffects::new(overlay_test_mode_address, disable_independent_flip_address);

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

impl From<HookResolveError> for InitializeStatus {
    fn from(error: HookResolveError) -> Self {
        match error {
            HookResolveError::ModuleNotLoaded { .. } => Self::DwmcoreModuleNotLoaded,
            HookResolveError::InvalidModuleImage { .. } => Self::DwmcoreImageInvalid,
            HookResolveError::ModuleAccessFailed { .. } => Self::DwmcoreImageAccessFailed,
            HookResolveError::ModuleImageMismatch { .. } => Self::DwmcoreImageMismatch,
            HookResolveError::SignatureNotFound { target } => Self::Resolve {
                kind: ResolveFailureKind::NotFound,
                target: target.into(),
            },
            HookResolveError::SignatureAmbiguous { target, .. } => Self::Resolve {
                kind: ResolveFailureKind::Ambiguous,
                target: target.into(),
            },
            HookResolveError::ConflictingPrologue { target, .. } => Self::Resolve {
                kind: ResolveFailureKind::PrologueConflict,
                target: target.into(),
            },
        }
    }
}

impl From<HookError> for InitializeStatus {
    fn from(error: HookError) -> Self {
        match error {
            HookError::AlreadyInitialized => Self::AlreadyInitialized,
            HookError::ProfileSelect(error) => match error {
                ProfileSelectError::UnsupportedDwmcoreVersion { .. } => {
                    Self::UnsupportedDwmcoreVersion
                }
                ProfileSelectError::DwmcoreModuleNotLoaded => Self::DwmcoreModuleNotLoaded,
                ProfileSelectError::DwmcoreVersionQueryFailed => Self::DwmcoreVersionQueryFailed,
            },
            HookError::Resolve(error) => error.into(),
            HookError::Payload(error) => InitializeStatus::from(&error),
            HookError::MinHook(error) => match error.operation {
                crate::minhook::MinHookOperation::Initialize => Self::MinHookInitializeFailed,
                crate::minhook::MinHookOperation::CreateHook(_) => Self::MinHookCreateHookFailed,
                crate::minhook::MinHookOperation::EnableHook => Self::MinHookEnableHookFailed,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, InitializeStatus, MonitorIdentity, MonitorTarget,
        PayloadAssignment, PayloadLut, ShutdownStatus,
    };

    use crate::profile::{DwmcoreVersion, HookProfile, HookTarget, ProfileSelectError};
    use crate::resolver::{
        HookResolveError, LoadedModule, ResolvedTarget, SignatureResolutionReport,
    };
    use crate::state::{self, HOOK_GLOBAL_TEST_LOCK};

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

    fn synthetic_resolution(profile: &HookProfile) -> SignatureResolutionReport {
        let base_address = 0x1800_0000usize;
        SignatureResolutionReport {
            module: LoadedModule {
                module_name: crate::profile::HOOK_MODULE_NAME,
                base_address,
                size: 0x20_0000,
            },
            targets: profile
                .signatures
                .iter()
                .enumerate()
                .map(|(index, signature)| ResolvedTarget {
                    target: signature.target,
                    address: if signature.target.is_function_hook_target() {
                        base_address + 0x1000 + index * 0x100
                    } else {
                        0
                    },
                })
                .collect(),
            skipped_signatures: Vec::new(),
        }
    }

    #[test]
    fn prologue_conflict_stops_before_minhook_registration() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        crate::minhook::reset_test_minhook_behavior(None, None, None, None);

        let error = super::prepare_initial_state_from_payload_with_profile_resolver(
            test_profile(),
            test_payload(),
            |_| {
                Err(HookResolveError::ConflictingPrologue {
                    target: HookTarget::Present,
                    rva: 0x1000,
                    mismatch_offset: 0,
                    expected: 0x40,
                    actual: 0xE9,
                })
            },
        )
        .expect_err("prologue conflict should stop initialization");

        assert!(matches!(
            error,
            super::HookError::Resolve(HookResolveError::ConflictingPrologue {
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
            assert_eq!(
                InitializeStatus::from(super::HookError::from(error)),
                expected
            );
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
            synthetic_resolution(&profile),
        )
        .expect_err("enable failure should abort initialization");

        assert!(matches!(error, super::HookError::MinHook(_)));
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

        super::initialize_with_resolution(profile, test_payload(), synthetic_resolution(&profile))
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

        super::initialize_with_resolution(profile, test_payload(), synthetic_resolution(&profile))
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
