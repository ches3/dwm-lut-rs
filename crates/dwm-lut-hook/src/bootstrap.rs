#[cfg(test)]
use std::cell::Cell;
use std::fmt;
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};

use dwm_lut_payload::{
    DwmLutPayloadBuffer, HookPayload, InitializeStatus, PayloadError, ReplaceAssignmentsStatus,
    ShutdownStatus, deserialize_payload_buffer,
};

use crate::LutBypassRuntime;
use std::sync::Arc;

use crate::lut_pipeline::LutPipeline;
use crate::minhook::{
    MinHookCleanupOperation, MinHookError, enable_registered_hooks, register_plan,
    unregister_registered_hooks,
};
use crate::profile::{BuildProfile, HookProfile};

use crate::resolver::{HookResolveError, SignatureResolutionReport, resolve_profile};
use crate::state::{
    HookRegistrationPlan, HookRuntime, HookState, ReplaceAssignmentsStart,
    ReplacePayloadPipelineError, ShutdownStart, begin_replace_assignments, begin_shutdown,
    clear_state_after_shutdown, finish_failed_shutdown, finish_replace_assignments, install_state,
    is_initialized, lock_present_runtime, minhook_cleanup_plan, replace_payload_pipeline,
};

#[cfg(not(test))]
static INITIALIZATION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
thread_local! {
    static INITIALIZATION_IN_PROGRESS: Cell<bool> = const { Cell::new(false) };
}

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
    if is_initialized() {
        return Err(HookError::AlreadyInitialized);
    }

    if !mark_initialization_in_progress() {
        return Err(HookError::AlreadyInitialized);
    }

    if is_initialized() {
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
    #[cfg(not(test))]
    {
        INITIALIZATION_IN_PROGRESS.load(Ordering::Acquire)
    }

    #[cfg(test)]
    {
        INITIALIZATION_IN_PROGRESS.with(Cell::get)
    }
}

#[cfg(not(test))]
fn mark_initialization_in_progress() -> bool {
    INITIALIZATION_IN_PROGRESS
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

#[cfg(test)]
fn mark_initialization_in_progress() -> bool {
    INITIALIZATION_IN_PROGRESS.with(|slot| {
        if slot.get() {
            false
        } else {
            slot.set(true);
            true
        }
    })
}

#[cfg(not(test))]
fn clear_initialization_in_progress() {
    INITIALIZATION_IN_PROGRESS.store(false, Ordering::Release);
}

#[cfg(test)]
fn clear_initialization_in_progress() {
    INITIALIZATION_IN_PROGRESS.with(|slot| slot.set(false));
}

#[cfg(test)]
pub(crate) fn reset_initialization_guard_for_tests() {
    clear_initialization_in_progress();
}

#[derive(Debug)]
pub enum HookError {
    AlreadyInitialized,
    Payload(PayloadError),
    MinHook(MinHookError),
    Resolve(HookResolveError),
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => write!(f, "hook is already initialized"),
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

#[derive(Debug)]
pub enum ReplaceAssignmentsError {
    NotInitialized,
    AlreadyInProgress,
    Payload(PayloadError),
    State(ReplacePayloadPipelineError),
}

impl fmt::Display for ReplaceAssignmentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "hook is not initialized"),
            Self::AlreadyInProgress => write!(f, "hook initialization or shutdown is in progress"),
            Self::Payload(error) => write!(f, "{error}"),
            Self::State(ReplacePayloadPipelineError::NotInitialized) => {
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

impl From<ReplacePayloadPipelineError> for ReplaceAssignmentsError {
    fn from(value: ReplacePayloadPipelineError) -> Self {
        Self::State(value)
    }
}

pub fn build_profile() -> BuildProfile {
    BuildProfile::Windows11_25H2
}

#[cfg(test)]
pub(crate) fn initialize_with_resolution(
    build_profile: BuildProfile,
    payload: HookPayload,
    resolution: SignatureResolutionReport,
) -> Result<(), HookError> {
    let _guard = enter_initialization()?;
    let state = prepare_initial_state_with_resolution(build_profile, payload, resolution)?;
    install_prepared_state(state)
}

pub(crate) unsafe fn ffi_initialize(payload_buffer: *const DwmLutPayloadBuffer) -> u32 {
    debug_log!("event=initialize_start profile={:?}", build_profile());

    if payload_buffer.is_null() {
        return InitializeStatus::NullPayload as u32;
    }

    let payload = match unsafe { deserialize_payload_buffer(payload_buffer) } {
        Ok(payload) => payload,
        Err(error) => {
            let status = map_payload_error_to_initialize_status(&error);
            debug_log!(
                "event=initialize_failed status={} error={}",
                status as u32,
                crate::debug_log::quoted(error.to_string())
            );
            return status as u32;
        }
    };

    match initialize_from_payload(build_profile(), payload) {
        Ok(()) => {
            crate::desktop_redraw::request_desktop_redraw();
            debug_log!("event=initialize_success");
            InitializeStatus::Success as u32
        }
        Err(error) => finish_initialize_error(error),
    }
}

pub(crate) fn ffi_shutdown() -> u32 {
    debug_log!("event=shutdown_start");
    if is_initialization_in_progress() {
        debug_log!(
            "event=shutdown_finished status={} reason={}",
            ShutdownStatus::AlreadyInProgress as u32,
            crate::debug_log::quoted("initialization_in_progress")
        );
        return ShutdownStatus::AlreadyInProgress as u32;
    }

    match begin_shutdown() {
        ShutdownStart::Started => {}
        ShutdownStart::NotInitialized => {
            debug_log!(
                "event=shutdown_finished status={} reason={}",
                ShutdownStatus::NotInitialized as u32,
                crate::debug_log::quoted("not_initialized")
            );
            return ShutdownStatus::NotInitialized as u32;
        }
        ShutdownStart::AlreadyInProgress => {
            debug_log!(
                "event=shutdown_finished status={} reason={}",
                ShutdownStatus::AlreadyInProgress as u32,
                crate::debug_log::quoted("already_in_progress")
            );
            return ShutdownStatus::AlreadyInProgress as u32;
        }
        ShutdownStart::AlreadyShutDown => {
            debug_log!(
                "event=shutdown_finished status={} reason={}",
                ShutdownStatus::AlreadyShutDown as u32,
                crate::debug_log::quoted("already_shutdown")
            );
            return ShutdownStatus::AlreadyShutDown as u32;
        }
    }

    let Some((minhook, hooks)) = minhook_cleanup_plan() else {
        clear_state_after_shutdown();
        debug_log!(
            "event=shutdown_finished status={} reason={}",
            ShutdownStatus::Success as u32,
            crate::debug_log::quoted("state_missing")
        );
        return ShutdownStatus::Success as u32;
    };

    let cleanup_failures = {
        let _present_guard = lock_present_runtime();
        #[cfg_attr(not(debug_assertions), allow(unused_variables))]
        let renderer_device_count = crate::d3d11_renderer::shutdown_renderer_resources();
        crate::state::restore_overlay_test_mode();
        debug_log!(
            "event=renderer_resources_released device_resource_count={}",
            renderer_device_count
        );
        crate::desktop_redraw::request_desktop_redraw();
        unregister_registered_hooks(&minhook, &hooks)
    };
    #[cfg(debug_assertions)]
    {
        for failure in &cleanup_failures {
            debug_log!(
                "event=minhook_cleanup_failed operation={:?} target={} status={}",
                failure.operation,
                crate::debug_log::quoted(failure.target.label()),
                failure.status
            );
        }
    }

    if cleanup_failures
        .iter()
        .any(|failure| failure.operation == MinHookCleanupOperation::RemoveHook)
    {
        finish_failed_shutdown();
        debug_log!(
            "event=shutdown_finished status={} cleanup_failure_count={}",
            ShutdownStatus::MinHookCleanupFailed as u32,
            cleanup_failures.len()
        );
        ShutdownStatus::MinHookCleanupFailed as u32
    } else {
        clear_state_after_shutdown();
        debug_log!(
            "event=shutdown_finished status={} cleanup_failure_count={}",
            ShutdownStatus::Success as u32,
            cleanup_failures.len()
        );
        ShutdownStatus::Success as u32
    }
}

pub(crate) unsafe fn ffi_replace_assignments(payload_buffer: *const DwmLutPayloadBuffer) -> u32 {
    debug_log!("event=replace_assignments_start");

    if payload_buffer.is_null() {
        return ReplaceAssignmentsStatus::NullPayload as u32;
    }

    let payload = match unsafe { deserialize_payload_buffer(payload_buffer) } {
        Ok(payload) => payload,
        Err(error) => {
            let status = map_payload_error_to_replace_assignments_status(&error);
            debug_log!(
                "event=replace_assignments_failed status={} error={}",
                status as u32,
                crate::debug_log::quoted(error.to_string())
            );
            return status as u32;
        }
    };

    match replace_assignments(payload) {
        Ok(()) => {
            debug_log!("event=replace_assignments_success");
            ReplaceAssignmentsStatus::Success as u32
        }
        Err(error) => finish_replace_assignments_error(error),
    }
}

fn replace_assignments(payload: HookPayload) -> Result<(), ReplaceAssignmentsError> {
    if is_initialization_in_progress() {
        return Err(ReplaceAssignmentsError::AlreadyInProgress);
    }
    let _guard = enter_replace_assignments()?;

    debug_log!(
        "event=replace_assignments_decoded assignment_count={}",
        payload.assignments.len()
    );

    let lut_pipeline = LutPipeline::from_payload(&payload);
    debug_log!(
        "event=replace_assignments_pipeline_prepared lut_count={}",
        lut_pipeline.luts.len()
    );

    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
    let renderer_device_count = {
        let _present_guard = lock_present_runtime();
        replace_payload_pipeline(payload, lut_pipeline)?;
        crate::d3d11_renderer::shutdown_renderer_resources()
    };
    debug_log!(
        "event=replace_assignments_renderer_resources_released device_resource_count={}",
        renderer_device_count
    );
    crate::desktop_redraw::request_desktop_redraw();
    Ok(())
}

#[cfg(debug_assertions)]
fn finish_replace_assignments_error(error: ReplaceAssignmentsError) -> u32 {
    let error_message = error.to_string();
    let status = map_replace_assignments_error(&error);
    debug_log!(
        "event=replace_assignments_failed status={} error={}",
        status as u32,
        crate::debug_log::quoted(error_message)
    );
    status as u32
}

#[cfg(not(debug_assertions))]
fn finish_replace_assignments_error(error: ReplaceAssignmentsError) -> u32 {
    map_replace_assignments_error(&error) as u32
}

fn map_replace_assignments_error(error: &ReplaceAssignmentsError) -> ReplaceAssignmentsStatus {
    match error {
        ReplaceAssignmentsError::NotInitialized
        | ReplaceAssignmentsError::State(ReplacePayloadPipelineError::NotInitialized) => {
            ReplaceAssignmentsStatus::NotInitialized
        }
        ReplaceAssignmentsError::AlreadyInProgress => ReplaceAssignmentsStatus::AlreadyInProgress,
        ReplaceAssignmentsError::Payload(error) => {
            map_payload_error_to_replace_assignments_status(error)
        }
    }
}

#[cfg(debug_assertions)]
fn finish_initialize_error(error: HookError) -> u32 {
    let error_message = error.to_string();
    let status = map_hook_error(error);
    debug_log!(
        "event=initialize_failed status={} error={}",
        status as u32,
        crate::debug_log::quoted(error_message)
    );
    status as u32
}

#[cfg(not(debug_assertions))]
fn finish_initialize_error(error: HookError) -> u32 {
    map_hook_error(error) as u32
}

fn initialize_from_payload(
    build_profile: BuildProfile,
    payload: HookPayload,
) -> Result<(), HookError> {
    let _guard = enter_initialization()?;

    let state = prepare_initial_state_from_payload(build_profile, payload)?;
    install_prepared_state(state)
}

fn install_prepared_state(state: HookState) -> Result<(), HookError> {
    let minhook = state.runtime.minhook;
    let hooks = state.runtime.hooks.clone();
    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
    let hook_count = hooks.len();

    install_state(state).map_err(|state| {
        rollback_registered_state_hooks(&state);
        HookError::AlreadyInitialized
    })?;

    if let Err(error) = enable_registered_hooks(&minhook) {
        clear_state_after_shutdown();
        unregister_registered_hooks(&minhook, &hooks);
        return Err(HookError::MinHook(error));
    }

    debug_log!("event=hooks_enabled hook_count={hook_count}");
    Ok(())
}

fn rollback_registered_state_hooks(state: &HookState) {
    unregister_registered_hooks(&state.runtime.minhook, &state.runtime.hooks);
}

fn prepare_initial_state_from_payload(
    build_profile: BuildProfile,
    payload: HookPayload,
) -> Result<HookState, HookError> {
    prepare_initial_state_from_payload_with_profile_resolver(
        build_profile,
        payload,
        resolve_profile,
    )
}

fn prepare_initial_state_from_payload_with_profile_resolver<F>(
    build_profile: BuildProfile,
    payload: HookPayload,
    resolver: F,
) -> Result<HookState, HookError>
where
    F: FnOnce(&HookProfile) -> Result<SignatureResolutionReport, HookResolveError>,
{
    debug_log!(
        "event=payload_decoded assignment_count={}",
        payload.assignments.len()
    );

    let lut_pipeline = LutPipeline::from_payload(&payload);
    debug_log!(
        "event=lut_pipeline_prepared lut_count={}",
        lut_pipeline.luts.len()
    );

    let profile = HookProfile::for_build(build_profile);

    let resolution = resolver(&profile)?;
    debug_log!(
        "event=signatures_resolved module={} module_base=0x{:x} module_size=0x{:x} target_count={} skipped_count={}",
        crate::debug_log::quoted(resolution.module.module_name),
        resolution.module.base_address,
        resolution.module.size,
        resolution.targets.len(),
        resolution.skipped_signatures.len()
    );
    #[cfg(debug_assertions)]
    {
        for target in &resolution.targets {
            debug_log!(
                "event=signature_resolved target={} capture_key={} address=0x{:x}",
                crate::debug_log::quoted(target.target.label()),
                crate::debug_log::quoted(target.capture_key),
                target.address
            );
        }
        for skipped in &resolution.skipped_signatures {
            debug_log!(
                "event=signature_skipped target={} capture_key={} reason={:?}",
                crate::debug_log::quoted(skipped.target.label()),
                crate::debug_log::quoted(skipped.capture_key),
                skipped.reason
            );
        }
    }

    finalize_initial_state(payload, profile, resolution, lut_pipeline)
}

#[cfg(test)]
pub(crate) fn prepare_initial_state_with_resolution(
    build_profile: BuildProfile,
    payload: HookPayload,
    resolution: SignatureResolutionReport,
) -> Result<HookState, HookError> {
    prepare_initial_state_from_payload_with_profile_resolver(build_profile, payload, |_| {
        Ok(resolution)
    })
}

fn finalize_initial_state(
    payload: HookPayload,
    profile: HookProfile,
    resolution: SignatureResolutionReport,
    lut_pipeline: LutPipeline,
) -> Result<HookState, HookError> {
    let registration_plan = HookRegistrationPlan::from_resolution(&resolution);
    let (minhook, registered_hooks) = register_plan(&registration_plan)?;
    debug_log!("event=hooks_created hook_count={}", registered_hooks.len());

    let overlay_test_mode_address = resolution
        .targets
        .iter()
        .find(|target| target.target == crate::profile::HookTarget::OverlayTestMode)
        .map(|target| target.address);
    let lut_bypass =
        LutBypassRuntime::new(!payload.assignments.is_empty(), overlay_test_mode_address);

    Ok(HookState {
        payload,
        profile,
        runtime: HookRuntime {
            minhook,
            lut_pipeline: Arc::new(lut_pipeline),
            hooks: registered_hooks,
            lut_bypass,
        },
    })
}

fn map_resolve_status(error: HookResolveError) -> InitializeStatus {
    match error {
        HookResolveError::ModuleNotLoaded { .. } => InitializeStatus::DwmcoreModuleNotLoaded,
        HookResolveError::InvalidModuleImage { .. } => InitializeStatus::DwmcoreImageInvalid,
        HookResolveError::SignatureNotFound { target, .. } => match target {
            crate::profile::HookTarget::Present => InitializeStatus::PresentSignatureNotFound,
            crate::profile::HookTarget::IsCandidateDirectFlipCompatible => {
                InitializeStatus::DirectFlipSignatureNotFound
            }
            crate::profile::HookTarget::WindowContextIsCandidateDirectFlipCompatible => {
                InitializeStatus::WindowDirectFlipSignatureNotFound
            }
            crate::profile::HookTarget::CompSwapChainIsCandidateDirectFlipCompatible => {
                InitializeStatus::CompSwapChainDirectFlipSignatureNotFound
            }
            crate::profile::HookTarget::CompVisualIsCandidateForPromotion => {
                InitializeStatus::CompVisualPromotionSignatureNotFound
            }
            crate::profile::HookTarget::CompSwapChainIsCandidateIndependentFlipCompatible => {
                InitializeStatus::CompSwapChainIndependentFlipSignatureNotFound
            }
            crate::profile::HookTarget::OverlayTestMode => {
                InitializeStatus::OverlayTestModeNotFound
            }
        },
        HookResolveError::SignatureAmbiguous { target, .. } => match target {
            crate::profile::HookTarget::Present => InitializeStatus::PresentSignatureAmbiguous,
            crate::profile::HookTarget::IsCandidateDirectFlipCompatible => {
                InitializeStatus::DirectFlipSignatureAmbiguous
            }
            crate::profile::HookTarget::WindowContextIsCandidateDirectFlipCompatible => {
                InitializeStatus::WindowDirectFlipSignatureAmbiguous
            }
            crate::profile::HookTarget::CompSwapChainIsCandidateDirectFlipCompatible => {
                InitializeStatus::CompSwapChainDirectFlipSignatureAmbiguous
            }
            crate::profile::HookTarget::CompVisualIsCandidateForPromotion => {
                InitializeStatus::CompVisualPromotionSignatureAmbiguous
            }
            crate::profile::HookTarget::CompSwapChainIsCandidateIndependentFlipCompatible => {
                InitializeStatus::CompSwapChainIndependentFlipSignatureAmbiguous
            }
            crate::profile::HookTarget::OverlayTestMode => {
                InitializeStatus::OverlayTestModeAmbiguous
            }
        },
    }
}

fn map_hook_error(error: HookError) -> InitializeStatus {
    match error {
        HookError::AlreadyInitialized => InitializeStatus::AlreadyInitialized,
        HookError::Resolve(error) => map_resolve_status(error),
        HookError::Payload(error) => map_payload_error_to_initialize_status(&error),
        HookError::MinHook(error) => match error.operation {
            crate::minhook::MinHookOperation::Initialize => {
                InitializeStatus::MinHookInitializeFailed
            }
            crate::minhook::MinHookOperation::CreateHook(_) => {
                InitializeStatus::MinHookCreateHookFailed
            }
            crate::minhook::MinHookOperation::EnableHook => {
                InitializeStatus::MinHookEnableHookFailed
            }
        },
    }
}

fn map_payload_error_to_initialize_status(error: &PayloadError) -> InitializeStatus {
    match error {
        PayloadError::EmptyBuffer | PayloadError::TooLarge { .. } => {
            InitializeStatus::InvalidPayload
        }
        PayloadError::NoAssignments => InitializeStatus::PayloadHasNoAssignments,
        _ => InitializeStatus::PayloadDecodeFailed,
    }
}

fn map_payload_error_to_replace_assignments_status(
    error: &PayloadError,
) -> ReplaceAssignmentsStatus {
    match error {
        PayloadError::EmptyBuffer | PayloadError::TooLarge { .. } => {
            ReplaceAssignmentsStatus::InvalidPayload
        }
        PayloadError::NoAssignments => ReplaceAssignmentsStatus::PayloadHasNoAssignments,
        _ => ReplaceAssignmentsStatus::PayloadDecodeFailed,
    }
}
