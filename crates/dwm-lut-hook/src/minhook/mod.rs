#[cfg(test)]
use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::Ordering;

mod detours;

use detours::{detour_for_target, original_slot_for_target};

use crate::resolver::{ResolvedFunctionVa, Va};
use dwm_lut_profile::HookTarget;

pub type MhStatus = i32;

pub const MH_OK: MhStatus = 0;
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

#[derive(Clone, Copy)]
pub struct MinHookRuntime {
    apis: MinHookApis,
}

impl std::fmt::Debug for MinHookRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MinHookRuntime").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinHookError {
    pub(crate) operation: MinHookOperation,
    status: MhStatus,
    pub(crate) fail_safe_succeeded: bool,
}

impl std::fmt::Display for MinHookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?} failed with MinHook status {}",
            self.operation, self.status
        )
    }
}

impl std::error::Error for MinHookError {}

impl MinHookError {
    fn new(operation: MinHookOperation, status: MhStatus) -> Self {
        Self {
            operation,
            status,
            fail_safe_succeeded: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MinHookOperation {
    Initialize,
    CreateHook(HookTarget),
    EnableHook(HookTarget),
    DisableHook(HookTarget),
    DisableAll,
}

#[derive(Clone, Copy)]
struct MinHookApis {
    initialize: MhInitializeApi,
    uninitialize: MhUninitializeApi,
    create_hook: MhCreateHookApi,
    enable_hook: MhEnableHookApi,
    disable_hook: MhDisableHookApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredHook {
    pub target: HookTarget,
    pub target_va: Va,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegisteredHooks {
    pub non_flip_gate: Vec<RegisteredHook>,
    pub flip_gate: Vec<RegisteredHook>,
}

impl RegisteredHooks {
    pub fn iter(&self) -> impl Iterator<Item = &RegisteredHook> {
        self.non_flip_gate.iter().chain(self.flip_gate.iter())
    }
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
        }
    }

    #[cfg(test)]
    {
        test_minhook_apis()
    }
}

fn create_hook(
    target: &ResolvedFunctionVa,
    apis: MinHookApis,
) -> Result<RegisteredHook, MinHookError> {
    let detour = detour_for_target(target.target());
    let original_slot = original_slot_for_target(target.target());
    let target_ptr = target.va().0 as *mut c_void;
    let status = unsafe { (apis.create_hook)(target_ptr, detour, original_slot) };
    if status != MH_OK {
        return Err(MinHookError::new(
            MinHookOperation::CreateHook(target.target()),
            status,
        ));
    }
    Ok(RegisteredHook {
        target: target.target(),
        target_va: target.va(),
    })
}

pub(crate) fn register_hooks(
    targets: &[ResolvedFunctionVa],
) -> Result<(MinHookRuntime, RegisteredHooks), MinHookError> {
    let apis = minhook_apis();
    let status = unsafe { (apis.initialize)() };
    if status != MH_OK {
        return Err(MinHookError::new(MinHookOperation::Initialize, status));
    }

    let mut registered = RegisteredHooks::default();
    for target in targets {
        let hook = match create_hook(target, apis) {
            Ok(hook) => hook,
            Err(error) => {
                clear_original_slots(registered.iter());
                unsafe {
                    (apis.uninitialize)();
                }
                return Err(error);
            }
        };
        if hook.target.is_flip_gate() {
            registered.flip_gate.push(hook);
        } else {
            registered.non_flip_gate.push(hook);
        }
    }

    Ok((MinHookRuntime { apis }, registered))
}

pub(crate) fn enable_hooks(
    runtime: &MinHookRuntime,
    hooks: &[RegisteredHook],
) -> Result<(), MinHookError> {
    for hook in hooks {
        let status = unsafe { (runtime.apis.enable_hook)(hook.target_va.0 as *mut c_void) };
        if status != MH_OK && status != MH_ERROR_ENABLED {
            let mut error = MinHookError::new(MinHookOperation::EnableHook(hook.target), status);
            error.fail_safe_succeeded = disable_all_hooks(runtime).is_ok();
            return Err(error);
        }
    }
    Ok(())
}

fn disable_hooks(runtime: &MinHookRuntime, hooks: &[RegisteredHook]) -> Result<(), MinHookError> {
    for hook in hooks {
        let status = unsafe { (runtime.apis.disable_hook)(hook.target_va.0 as *mut c_void) };
        if status != MH_OK && status != MH_ERROR_DISABLED {
            return Err(MinHookError::new(
                MinHookOperation::DisableHook(hook.target),
                status,
            ));
        }
    }
    Ok(())
}

pub(crate) fn set_flip_gate_enabled(
    runtime: &MinHookRuntime,
    hooks: &RegisteredHooks,
    enabled: bool,
) -> Result<(), MinHookError> {
    if enabled {
        enable_hooks(runtime, &hooks.flip_gate)
    } else {
        disable_hooks(runtime, &hooks.flip_gate).map_err(|mut error| {
            error.fail_safe_succeeded = disable_all_hooks(runtime).is_ok();
            error
        })
    }
}

pub(crate) fn disable_all_hooks(runtime: &MinHookRuntime) -> Result<(), MinHookError> {
    let status = unsafe { (runtime.apis.disable_hook)(ptr::null_mut()) };
    if status == MH_OK {
        Ok(())
    } else {
        Err(MinHookError::new(MinHookOperation::DisableAll, status))
    }
}

fn clear_original_slots<'a>(hooks: impl IntoIterator<Item = &'a RegisteredHook>) {
    for hook in hooks {
        detours::original_pointer_for_target(hook.target).store(ptr::null_mut(), Ordering::Release);
    }
}

pub(crate) fn uninitialize(runtime: &MinHookRuntime, hooks: &RegisteredHooks) {
    clear_original_slots(hooks.iter());
    unsafe {
        (runtime.apis.uninitialize)();
    }
}

#[cfg(test)]
#[derive(Default)]
struct TestMinHookBehavior {
    uninitialize_calls: usize,
    create_calls: usize,
    enable_calls: usize,
    create_fail_on: Option<usize>,
    fail_next_enable: bool,
    fail_next_disable_hook: bool,
    fail_next_disable_all: bool,
    enabled_targets: Vec<usize>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TestMinHookCallCounts {
    pub(crate) uninitialize_calls: usize,
    pub(crate) create_calls: usize,
    pub(crate) enable_calls: usize,
}

#[cfg(test)]
thread_local! {
    static TEST_MINHOOK_BEHAVIOR: RefCell<TestMinHookBehavior> =
        RefCell::new(TestMinHookBehavior::default());
}

#[cfg(test)]
pub(crate) fn reset_test_minhook_behavior(create_fail_on: Option<usize>) {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        *behavior.borrow_mut() = TestMinHookBehavior {
            create_fail_on,
            ..TestMinHookBehavior::default()
        };
    });
}

#[cfg(test)]
pub(crate) fn fail_next_test_minhook_enable() {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        behavior.borrow_mut().fail_next_enable = true;
    });
}

#[cfg(test)]
fn fail_next_test_minhook_disable_hook() {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        behavior.borrow_mut().fail_next_disable_hook = true;
    });
}

#[cfg(test)]
pub(crate) fn fail_next_test_minhook_disable_all() {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        behavior.borrow_mut().fail_next_disable_all = true;
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
        }
    })
}

#[cfg(test)]
fn test_minhook_apis() -> MinHookApis {
    MinHookApis {
        initialize: test_initialize,
        uninitialize: test_uninitialize,
        create_hook: test_create_hook,
        enable_hook: test_enable_hook,
        disable_hook: test_disable_hook,
    }
}

#[cfg(test)]
unsafe extern "system" fn test_initialize() -> MhStatus {
    MH_OK
}

#[cfg(test)]
unsafe extern "system" fn test_uninitialize() -> MhStatus {
    TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        let mut behavior = behavior.borrow_mut();
        behavior.uninitialize_calls += 1;
        behavior.enabled_targets.clear();
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
        if behavior.fail_next_enable {
            behavior.fail_next_enable = false;
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
        if target.is_null() {
            if behavior.fail_next_disable_all {
                behavior.fail_next_disable_all = false;
                return -3;
            }
            behavior.enabled_targets.clear();
            return MH_OK;
        }
        if behavior.fail_next_disable_hook {
            behavior.fail_next_disable_hook = false;
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
mod tests {
    use std::sync::atomic::Ordering;

    use crate::resolver::ResolvedFunctionVa;
    use crate::resolver::Va;
    use crate::state::HOOK_GLOBAL_TEST_LOCK as CONTROLLED_TEST_LOCK;
    use dwm_lut_profile::HookTarget;

    use super::{
        MinHookOperation, MinHookRuntime, RegisteredHooks, detours, disable_all_hooks,
        enable_hooks, register_hooks, set_flip_gate_enabled, test_minhook_call_counts,
        uninitialize,
    };

    fn targets(entries: &[(HookTarget, usize)]) -> Vec<ResolvedFunctionVa> {
        entries
            .iter()
            .map(|(target, va)| ResolvedFunctionVa::new(*target, Va(*va)))
            .collect()
    }

    fn enable_all(
        runtime: &MinHookRuntime,
        hooks: &RegisteredHooks,
    ) -> Result<(), super::MinHookError> {
        enable_hooks(runtime, &hooks.non_flip_gate)?;
        enable_hooks(runtime, &hooks.flip_gate)
    }

    fn reset_original_slots() {
        for &target in HookTarget::ALL {
            detours::original_pointer_for_target(target)
                .store(std::ptr::null_mut(), Ordering::Release);
        }
    }

    fn reset_controlled_behavior(create_fail_on: Option<usize>) {
        super::reset_test_minhook_behavior(create_fail_on);
    }

    #[test]
    fn register_hooks_classifies_targets_and_creates_without_enabling() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
        ]);

        super::reset_test_minhook_behavior(None);
        let (_runtime, registered) = register_hooks(&targets).expect("registration should succeed");

        assert_eq!(registered.non_flip_gate.len(), 1);
        assert_eq!(registered.flip_gate.len(), 1);
        assert_eq!(registered.non_flip_gate[0].target, HookTarget::Present);
        assert_eq!(registered.non_flip_gate[0].target_va, Va(0x1800_1000));
        assert!(
            !detours::original_pointer_for_target(HookTarget::Present)
                .load(Ordering::Acquire)
                .is_null()
        );
        assert_eq!(
            registered.flip_gate[0].target,
            HookTarget::IsCandidateDirectFlipCompatible
        );
        assert_eq!(registered.flip_gate[0].target_va, Va(0x1800_2000));
        assert!(
            !detours::original_pointer_for_target(HookTarget::IsCandidateDirectFlipCompatible)
                .load(Ordering::Acquire)
                .is_null()
        );
        assert_eq!(test_minhook_call_counts().enable_calls, 0);
        reset_original_slots();
    }

    #[test]
    fn set_flip_gate_enabled_is_idempotent() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        super::reset_test_minhook_behavior(None);
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
        ]);
        let (runtime, registered) = register_hooks(&targets).expect("registration should succeed");
        enable_all(&runtime, &registered).expect("initial enable should succeed");

        set_flip_gate_enabled(&runtime, &registered, true)
            .expect("re-enabling already enabled flip hooks should succeed");
        set_flip_gate_enabled(&runtime, &registered, false)
            .expect("disabling flip hooks should succeed");
        set_flip_gate_enabled(&runtime, &registered, false)
            .expect("re-disabling already disabled flip hooks should succeed");
        set_flip_gate_enabled(&runtime, &registered, true)
            .expect("enabling flip hooks after disable should succeed");
        reset_original_slots();
    }

    #[test]
    fn enable_failure_reports_successful_fail_safe() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None);
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
        ]);
        let (runtime, registered) = register_hooks(&targets).expect("registration should succeed");
        super::fail_next_test_minhook_enable();

        let error = enable_all(&runtime, &registered).expect_err("enable should fail");
        assert_eq!(
            error.operation,
            MinHookOperation::EnableHook(HookTarget::Present)
        );
        assert!(error.fail_safe_succeeded);
        reset_original_slots();
    }

    #[test]
    fn flip_gate_disable_failure_reports_successful_fail_safe() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        super::reset_test_minhook_behavior(None);
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
            (HookTarget::IsCandidateOverlayCompatible, 0x1800_3000),
        ]);
        let (runtime, registered) = register_hooks(&targets).expect("registration should succeed");
        enable_all(&runtime, &registered).expect("initial enable should succeed");
        super::fail_next_test_minhook_disable_hook();

        let error = set_flip_gate_enabled(&runtime, &registered, false)
            .expect_err("flip-gate disable should fail");
        assert_eq!(
            error.operation,
            MinHookOperation::DisableHook(HookTarget::IsCandidateDirectFlipCompatible)
        );
        assert!(error.fail_safe_succeeded);
        reset_original_slots();
    }

    #[test]
    fn enable_failure_preserves_operation_when_fail_safe_disable_fails() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        super::reset_test_minhook_behavior(None);
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
        ]);
        let (runtime, registered) = register_hooks(&targets).expect("registration should succeed");
        super::fail_next_test_minhook_enable();
        super::fail_next_test_minhook_disable_all();

        let error = enable_all(&runtime, &registered)
            .expect_err("enable fail-safe should preserve the original enable failure");
        assert_eq!(
            error.operation,
            MinHookOperation::EnableHook(HookTarget::Present)
        );
        assert!(!error.fail_safe_succeeded);
        reset_original_slots();
    }

    #[test]
    fn create_failure_uninitializes_minhook() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(Some(3));
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
            (HookTarget::IsCandidateOverlayCompatible, 0x1800_3000),
        ]);

        let error = register_hooks(&targets).expect_err("third create should fail");

        assert_eq!(
            error.operation,
            MinHookOperation::CreateHook(HookTarget::IsCandidateOverlayCompatible)
        );
        assert_eq!(test_minhook_call_counts().uninitialize_calls, 1);
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
    fn uninitialize_clears_original_slots() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None);
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
        ]);

        let (runtime, registered) = register_hooks(&targets).expect("registration should succeed");
        uninitialize(&runtime, &registered);

        assert_eq!(test_minhook_call_counts().uninitialize_calls, 1);
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
    fn disable_all_keeps_registered_hooks_and_original_slots() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None);
        let targets = targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::IsCandidateDirectFlipCompatible, 0x1800_2000),
        ]);
        let (runtime, registered) = register_hooks(&targets).expect("registration should succeed");
        enable_all(&runtime, &registered).expect("enable should succeed");

        disable_all_hooks(&runtime).expect("disable all should succeed");

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
}
