use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use dwm_lut_payload::{
    DwmLutPayloadBuffer, HookPayload, InitializeStatus, PayloadError, ReplaceAssignmentsStatus,
    ShutdownStatus, deserialize_payload_buffer,
};

use crate::LutBypassRuntime;
use std::sync::Arc;

use crate::lut_pipeline::LutPipeline;
use crate::minhook::{
    MinHookError, disable_registered_hooks, enable_registered_hooks, register_plan,
    unregister_registered_hooks,
};
use crate::profile::{
    HookProfile, ProfileSelectError, dwmcore_file_version, select_versioned_profile,
};

use crate::resolver::{HookResolveError, SignatureResolutionReport, resolve_profile};
use crate::state::{
    HookRegistrationPlan, HookRuntime, HookState, ReplaceAssignmentsStart,
    ReplacePayloadPipelineError, ShutdownStart, begin_replace_assignments, begin_shutdown,
    can_initialize, clear_state_after_shutdown, finish_reactivation, finish_replace_assignments,
    finish_shutdown, has_retained_state, install_state, lock_present_runtime, minhook_cleanup_plan,
    reactivate_retained_state, replace_payload_pipeline, retain_state_after_shutdown,
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
    debug_log!("event=initialize_start");

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

    match initialize_from_payload(payload) {
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
        disable_registered_hooks(&minhook, &hooks)
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

    retain_state_after_shutdown();
    finish_shutdown();
    if !cleanup_failures.is_empty() {
        debug_log!(
            "event=shutdown_finished status={} cleanup_failure_count={}",
            ShutdownStatus::MinHookCleanupFailed as u32,
            cleanup_failures.len()
        );
        ShutdownStatus::MinHookCleanupFailed as u32
    } else {
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
    debug_log!(
        "event=profile_selected min_version={} dwmcore_version={}",
        entry.min_version,
        dwmcore_version
    );
    Ok((entry.profile)())
}

fn reactivate_from_payload(payload: HookPayload) -> Result<(), HookError> {
    debug_log!(
        "event=payload_decoded assignment_count={}",
        payload.assignments.len()
    );

    let lut_pipeline = LutPipeline::from_payload(&payload);
    debug_log!(
        "event=lut_pipeline_prepared lut_count={}",
        lut_pipeline.luts.len()
    );

    let Some((minhook, _hooks)) = reactivate_retained_state(payload, lut_pipeline) else {
        return Err(HookError::AlreadyInitialized);
    };
    if let Err(error) = enable_registered_hooks(&minhook) {
        retain_state_after_shutdown();
        return Err(HookError::MinHook(error));
    }
    finish_reactivation();
    debug_log!("event=hooks_reenabled");
    Ok(())
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
        finish_shutdown();
        disable_registered_hooks(&minhook, &hooks);
        retain_state_after_shutdown();
        return Err(HookError::MinHook(error));
    }

    debug_log!("event=hooks_enabled hook_count={hook_count}");
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
    debug_log!(
        "event=payload_decoded assignment_count={}",
        payload.assignments.len()
    );

    let lut_pipeline = LutPipeline::from_payload(&payload);
    debug_log!(
        "event=lut_pipeline_prepared lut_count={}",
        lut_pipeline.luts.len()
    );

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
                "event=signature_resolved target={} address=0x{:x}",
                crate::debug_log::quoted(target.target.label()),
                target.address
            );
        }
        for skipped in &resolution.skipped_signatures {
            debug_log!(
                "event=signature_skipped target={} reason={:?}",
                crate::debug_log::quoted(skipped.target.label()),
                skipped.reason
            );
        }
    }

    finalize_initial_state(payload, profile, resolution, lut_pipeline)
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
    lut_pipeline: LutPipeline,
) -> Result<HookState, HookError> {
    let registration_plan = HookRegistrationPlan::from_resolution(&resolution);
    let (minhook, registered_hooks) = register_plan(&registration_plan)?;
    debug_log!("event=hooks_created hook_count={}", registered_hooks.len());

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
    debug_log!(
        "event=disable_independent_flip_address present={} address=0x{:x}",
        disable_independent_flip_address.is_some(),
        disable_independent_flip_address.unwrap_or(0)
    );
    let lut_bypass = LutBypassRuntime::new(
        !payload.assignments.is_empty(),
        overlay_test_mode_address,
        disable_independent_flip_address,
    );

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
        HookResolveError::ModuleAccessFailed { .. } => InitializeStatus::DwmcoreImageAccessFailed,
        HookResolveError::ModuleImageMismatch { .. } => InitializeStatus::DwmcoreImageMismatch,
        HookResolveError::SignatureNotFound { target, .. } => match target {
            crate::profile::HookTarget::Present => InitializeStatus::PresentSignatureNotFound,
            crate::profile::HookTarget::IsCandidateDirectFlipCompatible => {
                InitializeStatus::DirectFlipSignatureNotFound
            }
            crate::profile::HookTarget::DirectFlipInfoEnsureIndependentFlipState => {
                InitializeStatus::DirectFlipInfoEnsureIndependentFlipSignatureNotFound
            }
            crate::profile::HookTarget::IsDirectFlipSupportedOnTarget => {
                InitializeStatus::IsDirectFlipSupportedOnTargetSignatureNotFound
            }
            crate::profile::HookTarget::LegacySwapChainCheckDirectFlipSupport => {
                InitializeStatus::LegacySwapChainCheckDirectFlipSignatureNotFound
            }
            crate::profile::HookTarget::IsAdvancedDirectFlipCompatible => {
                InitializeStatus::IsAdvancedDirectFlipCompatibleSignatureNotFound
            }
            crate::profile::HookTarget::OverlayTestMode => {
                InitializeStatus::OverlayTestModeNotFound
            }
            crate::profile::HookTarget::DisableIndependentFlip => {
                InitializeStatus::DisableIndependentFlipNotFound
            }
            crate::profile::HookTarget::OverlaysEnabled => {
                unreachable!("optional OverlaysEnabled resolution errors are skipped")
            }
        },
        HookResolveError::SignatureAmbiguous { target, .. } => match target {
            crate::profile::HookTarget::Present => InitializeStatus::PresentSignatureAmbiguous,
            crate::profile::HookTarget::IsCandidateDirectFlipCompatible => {
                InitializeStatus::DirectFlipSignatureAmbiguous
            }
            crate::profile::HookTarget::DirectFlipInfoEnsureIndependentFlipState => {
                InitializeStatus::DirectFlipInfoEnsureIndependentFlipSignatureAmbiguous
            }
            crate::profile::HookTarget::IsDirectFlipSupportedOnTarget => {
                InitializeStatus::IsDirectFlipSupportedOnTargetSignatureAmbiguous
            }
            crate::profile::HookTarget::LegacySwapChainCheckDirectFlipSupport => {
                InitializeStatus::LegacySwapChainCheckDirectFlipSignatureAmbiguous
            }
            crate::profile::HookTarget::IsAdvancedDirectFlipCompatible => {
                InitializeStatus::IsAdvancedDirectFlipCompatibleSignatureAmbiguous
            }
            crate::profile::HookTarget::OverlayTestMode => {
                InitializeStatus::OverlayTestModeAmbiguous
            }
            crate::profile::HookTarget::DisableIndependentFlip => {
                InitializeStatus::DisableIndependentFlipAmbiguous
            }
            crate::profile::HookTarget::OverlaysEnabled => {
                unreachable!("optional OverlaysEnabled resolution errors are skipped")
            }
        },
        HookResolveError::ConflictingPrologue { target, .. } => match target {
            crate::profile::HookTarget::Present => InitializeStatus::PresentPrologueConflict,
            crate::profile::HookTarget::IsCandidateDirectFlipCompatible => {
                InitializeStatus::DirectFlipPrologueConflict
            }
            crate::profile::HookTarget::DirectFlipInfoEnsureIndependentFlipState => {
                InitializeStatus::DirectFlipInfoEnsureIndependentFlipPrologueConflict
            }
            crate::profile::HookTarget::IsDirectFlipSupportedOnTarget => {
                InitializeStatus::IsDirectFlipSupportedOnTargetPrologueConflict
            }
            crate::profile::HookTarget::LegacySwapChainCheckDirectFlipSupport => {
                InitializeStatus::LegacySwapChainCheckDirectFlipPrologueConflict
            }
            crate::profile::HookTarget::IsAdvancedDirectFlipCompatible => {
                InitializeStatus::IsAdvancedDirectFlipCompatiblePrologueConflict
            }
            crate::profile::HookTarget::OverlaysEnabled => {
                InitializeStatus::OverlaysEnabledPrologueConflict
            }
            crate::profile::HookTarget::OverlayTestMode
            | crate::profile::HookTarget::DisableIndependentFlip => {
                InitializeStatus::DwmcoreImageInvalid
            }
        },
    }
}

fn map_hook_error(error: HookError) -> InitializeStatus {
    match error {
        HookError::AlreadyInitialized => InitializeStatus::AlreadyInitialized,
        HookError::ProfileSelect(error) => match error {
            ProfileSelectError::UnsupportedDwmcoreVersion { .. } => {
                InitializeStatus::UnsupportedDwmcoreVersion
            }
            ProfileSelectError::DwmcoreModuleNotLoaded => InitializeStatus::DwmcoreModuleNotLoaded,
            ProfileSelectError::DwmcoreVersionQueryFailed => {
                InitializeStatus::DwmcoreVersionQueryFailed
            }
        },
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

#[cfg(test)]
mod tests {
    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, MonitorIdentity, MonitorTarget, PayloadAssignment,
        PayloadLut, ShutdownStatus,
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
        let status = super::map_resolve_status(HookResolveError::ModuleAccessFailed {
            module_name: crate::profile::HOOK_MODULE_NAME,
            operation: "map image view",
            error_code: 5,
        });

        assert_eq!(
            status,
            dwm_lut_payload::InitializeStatus::DwmcoreImageAccessFailed
        );
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
                dwm_lut_payload::InitializeStatus::UnsupportedDwmcoreVersion,
            ),
            (
                ProfileSelectError::DwmcoreModuleNotLoaded,
                dwm_lut_payload::InitializeStatus::DwmcoreModuleNotLoaded,
            ),
            (
                ProfileSelectError::DwmcoreVersionQueryFailed,
                dwm_lut_payload::InitializeStatus::DwmcoreVersionQueryFailed,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(super::map_hook_error(error.into()), expected);
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
        assert!(state::lut_bypass_runtime().is_none());
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
