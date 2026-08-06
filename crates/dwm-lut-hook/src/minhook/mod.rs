#[cfg(test)]
use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
#[cfg(test)]
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering;

mod detours;

use detours::{detour_for_target, original_slot_for_target};

#[cfg(test)]
pub(crate) fn original_pointer_for_target(
    target: dwm_lut_profile::HookTarget,
) -> &'static AtomicPtr<c_void> {
    detours::original_pointer_for_target(target)
}

use crate::resolver::{ResolvedFunctionVa, Va};
use dwm_lut_profile::HookTarget;

pub type MhStatus = i32;

pub const MH_OK: MhStatus = 0;
pub const MH_ERROR_ALREADY_INITIALIZED: MhStatus = 1;
pub const MH_ERROR_ENABLED: MhStatus = 5;
pub const MH_ERROR_DISABLED: MhStatus = 6;

pub type MhInitializeApi = unsafe extern "system" fn() -> MhStatus;
pub type MhUninitializeApi = unsafe extern "system" fn() -> MhStatus;
pub type MhCreateHookApi = unsafe extern "system" fn(
    target: *mut c_void,
    detour: *mut c_void,
    original: *mut *mut c_void,
) -> MhStatus;
pub type MhEnableHookApi = unsafe extern "system" fn(target: *mut c_void) -> MhStatus;
pub type MhDisableHookApi = unsafe extern "system" fn(target: *mut c_void) -> MhStatus;
pub type MhRemoveHookApi = unsafe extern "system" fn(target: *mut c_void) -> MhStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinHookState {
    pub owns_initialization: bool,
}

#[derive(Clone, Copy)]
pub struct MinHookRuntime {
    pub state: MinHookState,
    apis: MinHookApis,
}

impl std::fmt::Debug for MinHookRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MinHookRuntime")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl PartialEq for MinHookRuntime {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state && apis_eq(self.apis, other.apis)
    }
}

impl Eq for MinHookRuntime {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinHookError {
    pub(crate) operation: MinHookOperation,
    status: Option<MhStatus>,
    cleanup_failures: Vec<MinHookCleanupFailure>,
}

impl std::fmt::Display for MinHookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.status, self.cleanup_failures.is_empty()) {
            (Some(status), true) => write!(
                f,
                "{:?} failed with MinHook status {status}",
                self.operation
            ),
            (Some(status), false) => write!(
                f,
                "{:?} failed with MinHook status {status}; cleanup also failed for {} hook operation(s)",
                self.operation,
                self.cleanup_failures.len()
            ),
            (None, true) => write!(f, "{:?} failed", self.operation),
            (None, false) => write!(
                f,
                "{:?} failed; cleanup also failed for {} hook operation(s)",
                self.operation,
                self.cleanup_failures.len()
            ),
        }
    }
}

impl std::error::Error for MinHookError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MinHookCleanupFailure {
    pub(crate) operation: MinHookCleanupOperation,
    pub(crate) target: HookTarget,
    pub(crate) status: MhStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum MinHookCleanupOperation {
    DisableHook,
    EnableHook,
    RemoveHook,
}

impl MinHookError {
    pub(crate) fn has_remove_hook_cleanup_failure(&self) -> bool {
        cleanup_has_remove_hook_failure(&self.cleanup_failures)
    }

    pub(crate) fn has_cleanup_failures(&self) -> bool {
        !self.cleanup_failures.is_empty()
    }

    fn new(operation: MinHookOperation, status: Option<MhStatus>) -> Self {
        Self {
            operation,
            status,
            cleanup_failures: Vec::new(),
        }
    }

    fn with_cleanup_failures(
        operation: MinHookOperation,
        status: Option<MhStatus>,
        cleanup_failures: Vec<MinHookCleanupFailure>,
    ) -> Self {
        Self {
            operation,
            status,
            cleanup_failures,
        }
    }
}

fn cleanup_has_remove_hook_failure(failures: &[MinHookCleanupFailure]) -> bool {
    failures
        .iter()
        .any(|failure| failure.operation == MinHookCleanupOperation::RemoveHook)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MinHookOperation {
    Initialize,
    CreateHook(HookTarget),
    EnableHook(HookTarget),
    DisableHook(HookTarget),
}

#[derive(Clone, Copy)]
pub(crate) struct MinHookApis {
    pub initialize: MhInitializeApi,
    pub uninitialize: MhUninitializeApi,
    pub create_hook: MhCreateHookApi,
    pub enable_hook: MhEnableHookApi,
    pub disable_hook: MhDisableHookApi,
    pub remove_hook: MhRemoveHookApi,
}

fn apis_eq(left: MinHookApis, right: MinHookApis) -> bool {
    left.initialize as usize == right.initialize as usize
        && left.uninitialize as usize == right.uninitialize as usize
        && left.create_hook as usize == right.create_hook as usize
        && left.enable_hook as usize == right.enable_hook as usize
        && left.disable_hook as usize == right.disable_hook as usize
        && left.remove_hook as usize == right.remove_hook as usize
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredHook {
    pub target: HookTarget,
    pub target_va: Va,
}

pub(crate) fn register_plan(
    targets: &[ResolvedFunctionVa],
) -> Result<(MinHookRuntime, Vec<RegisteredHook>), MinHookError> {
    let apis = minhook_apis();
    let status = unsafe { (apis.initialize)() };
    if status != MH_OK && status != MH_ERROR_ALREADY_INITIALIZED {
        return Err(MinHookError::new(
            MinHookOperation::Initialize,
            Some(status),
        ));
    }
    let owns_initialization = status == MH_OK;

    let registered = match create_plan_hooks_with_apis(targets, apis) {
        Ok(registered) => registered,
        Err(error) => {
            if !error.has_remove_hook_cleanup_failure() && owns_initialization {
                unsafe {
                    (apis.uninitialize)();
                }
            }
            // A remove failure means at least one hook may still reference
            // MinHook state. Keep that state initialized for the process
            // lifetime instead of tearing down data that may still be used.
            return Err(error);
        }
    };
    Ok((
        MinHookRuntime {
            state: MinHookState {
                owns_initialization,
            },
            apis,
        },
        registered,
    ))
}

pub(crate) fn enable_registered_hooks(
    runtime: &MinHookRuntime,
    hooks: &[RegisteredHook],
) -> Result<(), MinHookError> {
    enable_registered_hooks_with_apis(hooks, runtime.apis)
}

pub(crate) fn disable_registered_hooks(
    runtime: &MinHookRuntime,
    hooks: &[RegisteredHook],
) -> Vec<MinHookCleanupFailure> {
    disable_registered_hooks_with_apis(hooks, runtime.apis)
}

pub(crate) fn non_flip_gate_hooks(hooks: &[RegisteredHook]) -> Vec<RegisteredHook> {
    hooks
        .iter()
        .copied()
        .filter(|hook| !hook.target.is_flip_gate())
        .collect()
}

pub(crate) fn flip_gate_hooks(hooks: &[RegisteredHook]) -> Vec<RegisteredHook> {
    hooks
        .iter()
        .copied()
        .filter(|hook| hook.target.is_flip_gate())
        .collect()
}

pub(crate) fn set_flip_gate_hooks_enabled(
    runtime: &MinHookRuntime,
    hooks: &[RegisteredHook],
    enabled: bool,
) -> Result<(), MinHookError> {
    let flip_hooks = flip_gate_hooks(hooks);
    if enabled {
        enable_registered_hooks(runtime, &flip_hooks)
    } else {
        disable_flip_gate_hooks_transactionally(runtime.apis, &flip_hooks)
    }
}

pub(crate) fn enable_hooks_for_cleanup(
    runtime: &MinHookRuntime,
    hooks: &[RegisteredHook],
) -> Vec<MinHookCleanupFailure> {
    enable_hooks_for_cleanup_with_apis(runtime.apis, hooks)
}

pub(crate) fn create_plan_hooks_with_apis(
    targets: &[ResolvedFunctionVa],
    apis: MinHookApis,
) -> Result<Vec<RegisteredHook>, MinHookError> {
    create_hooks_for_targets(targets, apis).map(registered_hooks_from_created)
}

fn enable_registered_hooks_with_apis(
    hooks: &[RegisteredHook],
    apis: MinHookApis,
) -> Result<(), MinHookError> {
    let mut newly_enabled = Vec::new();
    for hook in hooks {
        let status = unsafe { (apis.enable_hook)(hook.target_va.0 as *mut c_void) };
        if status == MH_OK {
            newly_enabled.push(*hook);
        } else if status != MH_ERROR_ENABLED {
            let cleanup_failures = disable_registered_hooks_with_apis(&newly_enabled, apis);
            return Err(MinHookError::with_cleanup_failures(
                MinHookOperation::EnableHook(hook.target),
                Some(status),
                cleanup_failures,
            ));
        }
    }
    Ok(())
}

fn disable_flip_gate_hooks_transactionally(
    apis: MinHookApis,
    hooks: &[RegisteredHook],
) -> Result<(), MinHookError> {
    let mut newly_disabled = Vec::new();
    for hook in hooks.iter().rev() {
        let status = unsafe { (apis.disable_hook)(hook.target_va.0 as *mut c_void) };
        if status == MH_OK {
            newly_disabled.push(*hook);
        } else if status != MH_ERROR_DISABLED {
            let cleanup_failures = enable_hooks_for_cleanup_with_apis(apis, &newly_disabled);
            return Err(MinHookError::with_cleanup_failures(
                MinHookOperation::DisableHook(hook.target),
                Some(status),
                cleanup_failures,
            ));
        }
    }
    Ok(())
}

fn enable_hooks_for_cleanup_with_apis(
    apis: MinHookApis,
    hooks: &[RegisteredHook],
) -> Vec<MinHookCleanupFailure> {
    let mut failures = Vec::new();
    for hook in hooks.iter().rev() {
        let status = unsafe { (apis.enable_hook)(hook.target_va.0 as *mut c_void) };
        if status != MH_OK && status != MH_ERROR_ENABLED {
            failures.push(MinHookCleanupFailure {
                operation: MinHookCleanupOperation::EnableHook,
                target: hook.target,
                status,
            });
        }
    }
    failures
}

fn create_hooks_for_targets(
    targets: &[ResolvedFunctionVa],
    apis: MinHookApis,
) -> Result<Vec<CreatedHook>, MinHookError> {
    let mut created = Vec::with_capacity(targets.len());

    for target in targets {
        let detour = detour_for_target(target.target());
        let original_slot = original_slot_for_target(target.target());
        let target_ptr = target.va().0 as *mut c_void;
        let status = unsafe { (apis.create_hook)(target_ptr, detour, original_slot) };
        if status != MH_OK {
            let cleanup_failures = remove_created_hooks(&apis, &created);
            return Err(MinHookError::with_cleanup_failures(
                MinHookOperation::CreateHook(target.target()),
                Some(status),
                cleanup_failures,
            ));
        }

        created.push(CreatedHook {
            target: target.target(),
            target_va: target.va(),
        });
    }

    Ok(created)
}

fn registered_hooks_from_created(created: Vec<CreatedHook>) -> Vec<RegisteredHook> {
    created
        .into_iter()
        .map(|hook| RegisteredHook {
            target: hook.target,
            target_va: hook.target_va,
        })
        .collect()
}

pub(crate) fn unregister_registered_hooks(
    runtime: &MinHookRuntime,
    hooks: &[RegisteredHook],
) -> Vec<MinHookCleanupFailure> {
    let failures = unregister_registered_hooks_with_apis(hooks, runtime.apis);
    if !cleanup_has_remove_hook_failure(&failures) && runtime.state.owns_initialization {
        unsafe {
            (runtime.apis.uninitialize)();
        }
    }
    failures
}

pub(crate) fn unregister_registered_hooks_with_apis(
    hooks: &[RegisteredHook],
    apis: MinHookApis,
) -> Vec<MinHookCleanupFailure> {
    let mut failures = disable_registered_hooks_with_apis(hooks, apis);

    for hook in hooks.iter().rev() {
        let status = unsafe { (apis.remove_hook)(hook.target_va.0 as *mut c_void) };
        if status != MH_OK {
            failures.push(MinHookCleanupFailure {
                operation: MinHookCleanupOperation::RemoveHook,
                target: hook.target,
                status,
            });
            continue;
        }
        detours::original_pointer_for_target(hook.target).store(ptr::null_mut(), Ordering::Release);
    }

    failures
}

fn disable_registered_hooks_with_apis(
    hooks: &[RegisteredHook],
    apis: MinHookApis,
) -> Vec<MinHookCleanupFailure> {
    let mut failures = Vec::new();
    for hook in hooks.iter().rev() {
        let status = unsafe { (apis.disable_hook)(hook.target_va.0 as *mut c_void) };
        if status != MH_OK && status != MH_ERROR_DISABLED {
            failures.push(MinHookCleanupFailure {
                operation: MinHookCleanupOperation::DisableHook,
                target: hook.target,
                status,
            });
        }
    }
    failures
}

struct CreatedHook {
    target: HookTarget,
    target_va: Va,
}

fn remove_created_hooks(apis: &MinHookApis, created: &[CreatedHook]) -> Vec<MinHookCleanupFailure> {
    let mut failures = Vec::new();
    for hook in created.iter().rev() {
        let status = unsafe { (apis.remove_hook)(hook.target_va.0 as *mut c_void) };
        if status != MH_OK {
            failures.push(MinHookCleanupFailure {
                operation: MinHookCleanupOperation::RemoveHook,
                target: hook.target,
                status,
            });
            continue;
        }
        detours::original_pointer_for_target(hook.target).store(ptr::null_mut(), Ordering::Release);
    }
    failures
}

fn minhook_apis() -> MinHookApis {
    #[cfg(not(test))]
    {
        MinHookApis {
            initialize: minhook_sys::MH_Initialize,
            uninitialize: minhook_sys::MH_Uninitialize,
            create_hook: minhook_sys::MH_CreateHook,
            enable_hook: minhook_sys::MH_EnableHook,
            disable_hook: minhook_sys::MH_DisableHook,
            remove_hook: minhook_sys::MH_RemoveHook,
        }
    }

    #[cfg(test)]
    {
        test_minhook_apis()
    }
}

#[cfg(test)]
#[derive(Default)]
struct TestMinHookBehavior {
    initialize_calls: usize,
    uninitialize_calls: usize,
    create_calls: usize,
    enable_calls: usize,
    disable_calls: usize,
    remove_calls: usize,
    create_fail_on: Option<usize>,
    enable_fail_on: Option<usize>,
    enable_fail_from: Option<usize>,
    disable_fail_on: Option<usize>,
    remove_fail_on: Option<usize>,
    enabled_targets: Vec<usize>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TestMinHookCallCounts {
    pub(crate) uninitialize_calls: usize,
    pub(crate) create_calls: usize,
    pub(crate) enable_calls: usize,
    pub(crate) disable_calls: usize,
    pub(crate) remove_calls: usize,
}

#[cfg(test)]
thread_local! {
    static TEST_MINHOOK_BEHAVIOR: RefCell<TestMinHookBehavior> =
        RefCell::new(TestMinHookBehavior::default());
}

#[cfg(test)]
pub(crate) fn reset_test_minhook_behavior(
    create_fail_on: Option<usize>,
    enable_fail_on: Option<usize>,
    disable_fail_on: Option<usize>,
    remove_fail_on: Option<usize>,
) {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        *behavior.borrow_mut() = TestMinHookBehavior {
            create_fail_on,
            enable_fail_on,
            disable_fail_on,
            remove_fail_on,
            ..TestMinHookBehavior::default()
        };
    });
}

#[cfg(test)]
fn set_test_minhook_cleanup_failures(
    disable_fail_on: Option<usize>,
    remove_fail_on: Option<usize>,
) {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        let mut behavior = behavior.borrow_mut();
        behavior.disable_fail_on = disable_fail_on;
        behavior.remove_fail_on = remove_fail_on;
    });
}

#[cfg(test)]
pub(crate) fn set_test_minhook_enable_fail_on(enable_fail_on: Option<usize>) {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        behavior.borrow_mut().enable_fail_on = enable_fail_on;
    });
}

#[cfg(test)]
pub(crate) fn set_test_minhook_enable_fail_from(enable_fail_from: Option<usize>) {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        behavior.borrow_mut().enable_fail_from = enable_fail_from;
    });
}

#[cfg(test)]
pub(crate) fn set_test_minhook_disable_fail_on(disable_fail_on: Option<usize>) {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        behavior.borrow_mut().disable_fail_on = disable_fail_on;
    });
}

#[cfg(test)]
pub(crate) fn test_minhook_call_counts() -> TestMinHookCallCounts {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        let behavior = behavior.borrow();
        TestMinHookCallCounts {
            uninitialize_calls: behavior.uninitialize_calls,
            create_calls: behavior.create_calls,
            enable_calls: behavior.enable_calls,
            disable_calls: behavior.disable_calls,
            remove_calls: behavior.remove_calls,
        }
    })
}

#[cfg(test)]
pub(crate) fn test_enabled_target_count() -> usize {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| behavior.borrow().enabled_targets.len())
}

#[cfg(test)]
fn test_minhook_apis() -> MinHookApis {
    MinHookApis {
        initialize: test_initialize,
        uninitialize: test_uninitialize,
        create_hook: test_create_hook,
        enable_hook: test_enable_hook,
        disable_hook: test_disable_hook,
        remove_hook: test_remove_hook,
    }
}

#[cfg(test)]
unsafe extern "system" fn test_initialize() -> MhStatus {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        behavior.borrow_mut().initialize_calls += 1;
    });
    MH_OK
}

#[cfg(test)]
unsafe extern "system" fn test_uninitialize() -> MhStatus {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        behavior.borrow_mut().uninitialize_calls += 1;
    });
    MH_OK
}

#[cfg(test)]
unsafe extern "system" fn test_create_hook(
    target: *mut c_void,
    _detour: *mut c_void,
    original: *mut *mut c_void,
) -> MhStatus {
    let status = TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        let mut behavior = behavior.borrow_mut();
        behavior.create_calls += 1;
        if behavior.create_fail_on == Some(behavior.create_calls) {
            -1
        } else {
            MH_OK
        }
    });
    if status != MH_OK {
        return status;
    }
    unsafe {
        *original = target;
    }
    MH_OK
}

#[cfg(test)]
unsafe extern "system" fn test_enable_hook(target: *mut c_void) -> MhStatus {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        let mut behavior = behavior.borrow_mut();
        behavior.enable_calls += 1;
        if behavior.enable_fail_on == Some(behavior.enable_calls)
            || behavior
                .enable_fail_from
                .is_some_and(|from| behavior.enable_calls >= from)
        {
            return -2;
        }
        let target = target as usize;
        if behavior.enabled_targets.contains(&target) {
            return MH_ERROR_ENABLED;
        }
        behavior.enabled_targets.push(target);
        MH_OK
    })
}

#[cfg(test)]
unsafe extern "system" fn test_disable_hook(target: *mut c_void) -> MhStatus {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        let mut behavior = behavior.borrow_mut();
        behavior.disable_calls += 1;
        if behavior.disable_fail_on == Some(behavior.disable_calls) {
            return -3;
        }
        let target = target as usize;
        if let Some(index) = behavior
            .enabled_targets
            .iter()
            .position(|enabled| *enabled == target)
        {
            behavior.enabled_targets.swap_remove(index);
            MH_OK
        } else {
            MH_ERROR_DISABLED
        }
    })
}

#[cfg(test)]
unsafe extern "system" fn test_remove_hook(_target: *mut c_void) -> MhStatus {
    let status = TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        let mut behavior = behavior.borrow_mut();
        behavior.remove_calls += 1;
        if behavior.remove_fail_on == Some(behavior.remove_calls) {
            -4
        } else {
            MH_OK
        }
    });
    if status != MH_OK {
        return status;
    }
    MH_OK
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use crate::resolver::ResolvedFunctionVa;
    use crate::resolver::Va;
    use crate::state::HOOK_GLOBAL_TEST_LOCK as CONTROLLED_TEST_LOCK;
    use dwm_lut_profile::HookTarget;

    use super::{
        MinHookCleanupOperation, MinHookOperation, detours, disable_registered_hooks,
        enable_registered_hooks, register_plan, set_flip_gate_hooks_enabled,
        test_enabled_target_count, test_minhook_call_counts, unregister_registered_hooks,
        unregister_registered_hooks_with_apis,
    };

    fn targets(entries: &[(HookTarget, usize)]) -> Vec<ResolvedFunctionVa> {
        entries
            .iter()
            .map(|(target, va)| ResolvedFunctionVa::new(*target, Va(*va)))
            .collect()
    }

    fn reset_original_slots() {
        for &target in HookTarget::ALL {
            detours::original_pointer_for_target(target)
                .store(std::ptr::null_mut(), Ordering::Release);
        }
    }

    fn reset_controlled_behavior(create_fail_on: Option<usize>, enable_fail_on: Option<usize>) {
        super::reset_test_minhook_behavior(create_fail_on, enable_fail_on, None, None);
    }

    fn set_controlled_cleanup_failures(
        disable_fail_on: Option<usize>,
        remove_fail_on: Option<usize>,
    ) {
        let counts = test_minhook_call_counts();
        super::set_test_minhook_cleanup_failures(
            disable_fail_on.map(|call| counts.disable_calls + call),
            remove_fail_on.map(|call| counts.remove_calls + call),
        );
    }

    #[test]
    fn registration_maps_targets_to_detours_and_original_slots() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
        ]);

        super::reset_test_minhook_behavior(None, None, None, None);
        let (_runtime, registered) = register_plan(&targets).expect("registration should succeed");

        assert_eq!(registered.len(), 2);
        assert_eq!(registered[0].target, HookTarget::Present);
        assert_eq!(registered[0].target_va, Va(0x1800_1000));
        assert_eq!(
            detours::original_pointer_for_target(HookTarget::Present).load(Ordering::Acquire)
                as usize,
            0x1800_1000
        );
        assert_eq!(
            registered[1].target,
            HookTarget::IsCandidateDirectFlipCompatible
        );
        assert_eq!(registered[1].target_va, Va(0x1800_2000));
        assert_eq!(
            detours::original_pointer_for_target(HookTarget::IsCandidateDirectFlipCompatible)
                .load(Ordering::Acquire) as usize,
            0x1800_2000
        );
        assert_eq!(test_minhook_call_counts().enable_calls, 0);
        reset_original_slots();
    }

    #[test]
    fn register_plan_defers_hook_enablement() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        super::reset_test_minhook_behavior(None, None, None, None);
        let targets = targets(&[(HookTarget::Present, 0x1800_1000)]);

        let (runtime, registered) =
            register_plan(&targets).expect("register should create hooks without enabling them");
        assert_eq!(registered.len(), 1);
        assert_eq!(test_minhook_call_counts().enable_calls, 0);

        enable_registered_hooks(&runtime, &registered)
            .expect("hooks should enable after state is ready");
        assert_eq!(test_minhook_call_counts().enable_calls, 1);
        reset_original_slots();
    }

    #[test]
    fn set_flip_gate_hooks_enabled_is_idempotent() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        super::reset_test_minhook_behavior(None, None, None, None);
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
        ]);
        let (runtime, registered) = register_plan(&targets).expect("registration should succeed");
        enable_registered_hooks(&runtime, &registered).expect("initial enable should succeed");

        set_flip_gate_hooks_enabled(&runtime, &registered, true)
            .expect("re-enabling already enabled flip hooks should succeed");
        set_flip_gate_hooks_enabled(&runtime, &registered, false)
            .expect("disabling flip hooks should succeed");
        set_flip_gate_hooks_enabled(&runtime, &registered, false)
            .expect("re-disabling already disabled flip hooks should succeed");
        set_flip_gate_hooks_enabled(&runtime, &registered, true)
            .expect("enabling flip hooks after disable should succeed");
        reset_original_slots();
    }

    #[test]
    fn enable_failure_disables_newly_enabled_hooks() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, Some(2));
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
        ]);
        let (runtime, registered) = register_plan(&targets).expect("registration should succeed");

        let error =
            enable_registered_hooks(&runtime, &registered).expect_err("second enable should fail");
        assert_eq!(
            error.operation,
            MinHookOperation::EnableHook(HookTarget::IsCandidateDirectFlipCompatible)
        );
        assert!(error.cleanup_failures.is_empty());
        assert_eq!(test_minhook_call_counts().enable_calls, 2);
        assert_eq!(test_minhook_call_counts().disable_calls, 1);
        assert_eq!(test_enabled_target_count(), 0);
        reset_original_slots();
    }

    #[test]
    fn set_flip_gate_disable_failure_re_enables_newly_disabled_hooks() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        super::reset_test_minhook_behavior(None, None, Some(2), None);
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
            (HookTarget::IsCandidateOverlayCompatible, 0x1800_3000),
        ]);
        let (runtime, registered) = register_plan(&targets).expect("registration should succeed");
        enable_registered_hooks(&runtime, &registered).expect("initial enable should succeed");
        assert_eq!(test_enabled_target_count(), 3);

        let error = set_flip_gate_hooks_enabled(&runtime, &registered, false)
            .expect_err("second flip-gate disable should fail");
        assert_eq!(
            error.operation,
            MinHookOperation::DisableHook(HookTarget::IsCandidateDirectFlipCompatible)
        );
        assert!(error.cleanup_failures.is_empty());
        assert_eq!(test_enabled_target_count(), 3);
        reset_original_slots();
    }

    #[test]
    fn create_failure_removes_previously_created_hooks() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(Some(3), None);
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
            (HookTarget::IsCandidateOverlayCompatible, 0x1800_3000),
        ]);

        let error = register_plan(&targets).expect_err("third create should fail");

        assert_eq!(
            error.operation,
            MinHookOperation::CreateHook(HookTarget::IsCandidateOverlayCompatible)
        );
        assert!(error.cleanup_failures.is_empty());
        let calls = test_minhook_call_counts();
        assert_eq!(calls.create_calls, 3);
        assert_eq!(calls.enable_calls, 0);
        assert_eq!(calls.disable_calls, 0);
        assert_eq!(calls.remove_calls, 2);
    }

    #[test]
    fn create_failure_reports_cleanup_remove_failure() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(Some(3), None);
        set_controlled_cleanup_failures(None, Some(1));
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
            (HookTarget::IsCandidateOverlayCompatible, 0x1800_3000),
        ]);

        let error = register_plan(&targets).expect_err("third create should fail");

        assert_eq!(
            error.operation,
            MinHookOperation::CreateHook(HookTarget::IsCandidateOverlayCompatible)
        );
        assert_eq!(error.cleanup_failures.len(), 1);
        assert_eq!(
            error.cleanup_failures[0].operation,
            MinHookCleanupOperation::RemoveHook
        );
        assert_eq!(
            error.cleanup_failures[0].target,
            HookTarget::IsCandidateDirectFlipCompatible
        );
        assert_eq!(error.cleanup_failures[0].status, -4);
        assert!(
            detours::original_pointer_for_target(HookTarget::Present)
                .load(Ordering::Acquire)
                .is_null()
        );
        assert_eq!(
            detours::original_pointer_for_target(HookTarget::IsCandidateDirectFlipCompatible)
                .load(Ordering::Acquire) as usize,
            0x1800_2000
        );
        reset_original_slots();
    }

    #[test]
    fn unregister_disables_and_removes_registered_hooks() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, None);
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
        ]);

        let (runtime, registered) = register_plan(&targets).expect("registration should succeed");
        let cleanup_failures = unregister_registered_hooks_with_apis(&registered, runtime.apis);

        let calls = test_minhook_call_counts();
        assert_eq!(calls.create_calls, 2);
        assert_eq!(calls.enable_calls, 0);
        assert_eq!(calls.disable_calls, 2);
        assert_eq!(calls.remove_calls, 2);
        assert!(cleanup_failures.is_empty());
        assert!(
            detours::original_pointer_for_target(HookTarget::Present)
                .load(Ordering::Acquire)
                .is_null()
        );
        assert!(
            detours::original_pointer_for_target(HookTarget::IsCandidateDirectFlipCompatible)
                .load(Ordering::Acquire)
                .is_null()
        );
    }

    #[test]
    fn disable_keeps_registered_hooks_and_original_slots() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, None);
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
        ]);
        let (runtime, registered) = register_plan(&targets).expect("registration should succeed");

        let cleanup_failures = disable_registered_hooks(&runtime, &registered);

        let calls = test_minhook_call_counts();
        assert!(cleanup_failures.is_empty());
        assert_eq!(calls.disable_calls, 2);
        assert_eq!(calls.remove_calls, 0);
        assert_eq!(calls.uninitialize_calls, 0);
        assert_eq!(
            detours::original_pointer_for_target(HookTarget::Present).load(Ordering::Acquire)
                as usize,
            0x1800_1000
        );
        assert_eq!(
            detours::original_pointer_for_target(HookTarget::IsCandidateDirectFlipCompatible)
                .load(Ordering::Acquire) as usize,
            0x1800_2000
        );
        reset_original_slots();
    }

    #[test]
    fn unregister_keeps_original_slot_when_remove_fails() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, None);
        set_controlled_cleanup_failures(None, Some(1));
        let targets = targets(&[(HookTarget::Present, 0x1800_1000)]);

        let (runtime, registered) = register_plan(&targets).expect("registration should succeed");
        let cleanup_failures = unregister_registered_hooks_with_apis(&registered, runtime.apis);

        assert_eq!(cleanup_failures.len(), 1);
        assert_eq!(
            cleanup_failures[0].operation,
            MinHookCleanupOperation::RemoveHook
        );
        assert_eq!(cleanup_failures[0].target, HookTarget::Present);
        assert_eq!(
            detours::original_pointer_for_target(HookTarget::Present).load(Ordering::Acquire)
                as usize,
            0x1800_1000
        );
        reset_original_slots();
    }

    #[test]
    fn unregister_keeps_minhook_runtime_when_cleanup_fails() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, None);
        set_controlled_cleanup_failures(None, Some(1));
        let targets = targets(&[(HookTarget::Present, 0x1800_1000)]);
        let (runtime, registered) = register_plan(&targets).expect("registration should succeed");

        let cleanup_failures = unregister_registered_hooks(&runtime, &registered);

        assert_eq!(cleanup_failures.len(), 1);
        assert_eq!(test_minhook_call_counts().uninitialize_calls, 0);
        reset_original_slots();
    }

    #[test]
    fn unregister_uninitializes_when_only_disable_cleanup_fails() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, None);
        set_controlled_cleanup_failures(Some(1), None);
        let targets = targets(&[(HookTarget::Present, 0x1800_1000)]);
        let (runtime, registered) = register_plan(&targets).expect("registration should succeed");

        let cleanup_failures = unregister_registered_hooks(&runtime, &registered);

        assert_eq!(cleanup_failures.len(), 1);
        assert_eq!(
            cleanup_failures[0].operation,
            MinHookCleanupOperation::DisableHook
        );
        assert_eq!(test_minhook_call_counts().uninitialize_calls, 1);
        assert!(
            detours::original_pointer_for_target(HookTarget::Present)
                .load(Ordering::Acquire)
                .is_null()
        );
    }
}
