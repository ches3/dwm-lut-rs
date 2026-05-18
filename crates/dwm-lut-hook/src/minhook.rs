#[cfg(test)]
use std::cell::RefCell;
use std::ffi::c_void;
#[cfg(not(test))]
use std::mem::MaybeUninit;
use std::mem::{align_of, size_of};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::profile::{HookProfile, HookTarget};
use crate::state::HookRegistrationPlan;
use crate::{ClipBox, DirtyRect, state};

pub type MhStatus = i32;

pub const MH_OK: MhStatus = 0;
pub const MH_ERROR_ALREADY_INITIALIZED: MhStatus = 1;
const MH_ALL_HOOKS: *mut c_void = ptr::null_mut();

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinHookState {
    pub module_name: &'static str,
    pub module_handle: usize,
    pub owns_initialization: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinHookRuntime {
    pub state: MinHookState,
    api_addresses: MinHookApiAddresses,
}

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
pub(crate) enum MinHookCleanupOperation {
    DisableHook,
    RemoveHook,
}

impl MinHookError {
    pub(crate) fn has_remove_hook_cleanup_failure(&self) -> bool {
        cleanup_has_remove_hook_failure(&self.cleanup_failures)
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
#[cfg_attr(test, allow(dead_code))]
pub(crate) enum MinHookOperation {
    Initialize,
    CreateHook(HookTarget),
    EnableHook,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MinHookApiAddresses {
    initialize: usize,
    uninitialize: usize,
    create_hook: usize,
    enable_hook: usize,
    disable_hook: usize,
    remove_hook: usize,
}

impl MinHookApiAddresses {
    fn from_apis(apis: MinHookApis) -> Self {
        Self {
            initialize: apis.initialize as usize,
            uninitialize: apis.uninitialize as usize,
            create_hook: apis.create_hook as usize,
            enable_hook: apis.enable_hook as usize,
            disable_hook: apis.disable_hook as usize,
            remove_hook: apis.remove_hook as usize,
        }
    }

    unsafe fn to_apis(self) -> MinHookApis {
        MinHookApis {
            initialize: unsafe { std::mem::transmute::<usize, MhInitializeApi>(self.initialize) },
            uninitialize: unsafe {
                std::mem::transmute::<usize, MhUninitializeApi>(self.uninitialize)
            },
            create_hook: unsafe { std::mem::transmute::<usize, MhCreateHookApi>(self.create_hook) },
            enable_hook: unsafe { std::mem::transmute::<usize, MhEnableHookApi>(self.enable_hook) },
            disable_hook: unsafe {
                std::mem::transmute::<usize, MhDisableHookApi>(self.disable_hook)
            },
            remove_hook: unsafe { std::mem::transmute::<usize, MhRemoveHookApi>(self.remove_hook) },
        }
    }
}

struct LoadedMinHook {
    module_name: &'static str,
    module_handle: usize,
    apis: MinHookApis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredHook {
    pub target: HookTarget,
    pub target_address: usize,
}

pub(crate) fn register_plan(
    plan: &HookRegistrationPlan,
) -> Result<(MinHookRuntime, Vec<RegisteredHook>), MinHookError> {
    let loaded = load_minhook_apis()?;
    let apis = loaded.apis;
    let status = unsafe { (apis.initialize)() };
    if status != MH_OK && status != MH_ERROR_ALREADY_INITIALIZED {
        free_minhook_module(loaded.module_handle);
        return Err(MinHookError::new(
            MinHookOperation::Initialize,
            Some(status),
        ));
    }
    let owns_minhook_initialization = status == MH_OK;

    let registered = match register_plan_with_apis(plan, apis) {
        Ok(registered) => registered,
        Err(error) => {
            if !error.has_remove_hook_cleanup_failure() {
                if owns_minhook_initialization {
                    unsafe {
                        (apis.uninitialize)();
                    }
                }
                free_minhook_module(loaded.module_handle);
            } else {
                // A remove failure means at least one hook may still reference
                // MinHook state. Keep that state initialized for the process
                // lifetime instead of tearing down data that may still be used.
            }
            return Err(error);
        }
    };
    let runtime = MinHookRuntime {
        state: MinHookState {
            module_name: loaded.module_name,
            module_handle: loaded.module_handle,
            owns_initialization: owns_minhook_initialization,
        },
        api_addresses: MinHookApiAddresses::from_apis(apis),
    };
    Ok((runtime, registered))
}

pub(crate) fn register_plan_with_apis(
    plan: &HookRegistrationPlan,
    apis: MinHookApis,
) -> Result<Vec<RegisteredHook>, MinHookError> {
    let mut created = Vec::with_capacity(plan.targets.len());

    for target in &plan.targets {
        let detour = detour_for_target(target.target);
        let original_slot = original_slot_for_target(target.target);
        let target_address = target.address as *mut c_void;
        let status = unsafe { (apis.create_hook)(target_address, detour, original_slot) };
        if status != MH_OK {
            let cleanup_failures = remove_created_hooks(&apis, &created);
            return Err(MinHookError::with_cleanup_failures(
                MinHookOperation::CreateHook(target.target),
                Some(status),
                cleanup_failures,
            ));
        }

        created.push(CreatedHook {
            target: target.target,
            target_address,
            target_address_value: target.address,
        });
    }

    let status = unsafe { (apis.enable_hook)(MH_ALL_HOOKS) };
    if status != MH_OK {
        let cleanup_failures = remove_created_hooks(&apis, &created);
        return Err(MinHookError::with_cleanup_failures(
            MinHookOperation::EnableHook,
            Some(status),
            cleanup_failures,
        ));
    }

    let mut registered = Vec::with_capacity(created.len());
    for hook in created {
        registered.push(RegisteredHook {
            target: hook.target,
            target_address: hook.target_address_value,
        });
    }

    Ok(registered)
}

pub(crate) fn unregister_registered_hooks(
    runtime: &MinHookRuntime,
    hooks: &[RegisteredHook],
) -> Vec<MinHookCleanupFailure> {
    let apis = unsafe { runtime.api_addresses.to_apis() };
    let failures = unregister_registered_hooks_with_apis(hooks, apis);
    if !cleanup_has_remove_hook_failure(&failures) {
        if runtime.state.owns_initialization {
            unsafe {
                (apis.uninitialize)();
            }
        }
        free_minhook_module(runtime.state.module_handle);
    }
    failures
}

pub(crate) fn unregister_registered_hooks_with_apis(
    hooks: &[RegisteredHook],
    apis: MinHookApis,
) -> Vec<MinHookCleanupFailure> {
    let mut failures = Vec::new();
    for hook in hooks.iter().rev() {
        let status = unsafe { (apis.disable_hook)(hook.target_address as *mut c_void) };
        if status != MH_OK {
            failures.push(MinHookCleanupFailure {
                operation: MinHookCleanupOperation::DisableHook,
                target: hook.target,
                status,
            });
        }
    }

    for hook in hooks.iter().rev() {
        let status = unsafe { (apis.remove_hook)(hook.target_address as *mut c_void) };
        if status != MH_OK {
            failures.push(MinHookCleanupFailure {
                operation: MinHookCleanupOperation::RemoveHook,
                target: hook.target,
                status,
            });
            continue;
        }
        original_pointer_for_target(hook.target).store(ptr::null_mut(), Ordering::Release);
    }

    failures
}

struct CreatedHook {
    target: HookTarget,
    target_address: *mut c_void,
    target_address_value: usize,
}

fn remove_created_hooks(apis: &MinHookApis, created: &[CreatedHook]) -> Vec<MinHookCleanupFailure> {
    let mut failures = Vec::new();
    for hook in created.iter().rev() {
        let status = unsafe { (apis.remove_hook)(hook.target_address) };
        if status != MH_OK {
            failures.push(MinHookCleanupFailure {
                operation: MinHookCleanupOperation::RemoveHook,
                target: hook.target,
                status,
            });
            continue;
        }
        original_pointer_for_target(hook.target).store(ptr::null_mut(), Ordering::Release);
    }
    failures
}

#[cfg(not(test))]
fn load_minhook_apis() -> Result<LoadedMinHook, MinHookError> {
    Ok(LoadedMinHook {
        module_name: "minhook-sys",
        module_handle: 0,
        apis: MinHookApis {
            initialize: minhook_sys::MH_Initialize,
            uninitialize: minhook_sys::MH_Uninitialize,
            create_hook: minhook_sys::MH_CreateHook,
            enable_hook: minhook_sys::MH_EnableHook,
            disable_hook: minhook_sys::MH_DisableHook,
            remove_hook: minhook_sys::MH_RemoveHook,
        },
    })
}

#[cfg(not(test))]
fn free_minhook_module(_module_handle: usize) {}

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
    disable_fail_on: Option<usize>,
    remove_fail_on: Option<usize>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TestMinHookCallCounts {
    uninitialize_calls: usize,
    create_calls: usize,
    enable_calls: usize,
    disable_calls: usize,
    remove_calls: usize,
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
pub(crate) fn reset_test_original_slots() {
    PRESENT_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    DIRECT_FLIP_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    OVERLAYS_ENABLED_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    WINDOW_DIRECT_FLIP_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    COMP_SWAP_CHAIN_DIRECT_FLIP_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    COMP_SWAP_CHAIN_INDEPENDENT_FLIP_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    COMP_VISUAL_PROMOTION_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
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
fn test_minhook_call_counts() -> TestMinHookCallCounts {
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
fn load_minhook_apis() -> Result<LoadedMinHook, MinHookError> {
    Ok(LoadedMinHook {
        module_name: "test-minhook",
        module_handle: 0,
        apis: test_minhook_apis(),
    })
}

#[cfg(test)]
fn free_minhook_module(_module_handle: usize) {}

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
unsafe extern "system" fn test_enable_hook(_target: *mut c_void) -> MhStatus {
    let status = TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        let mut behavior = behavior.borrow_mut();
        behavior.enable_calls += 1;
        if behavior.enable_fail_on == Some(behavior.enable_calls) {
            -2
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
unsafe extern "system" fn test_disable_hook(_target: *mut c_void) -> MhStatus {
    let status = TEST_MINHOOK_BEHAVIOR.with(|behavior| {
        let mut behavior = behavior.borrow_mut();
        behavior.disable_calls += 1;
        if behavior.disable_fail_on == Some(behavior.disable_calls) {
            -3
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

type PresentOriginal = unsafe extern "system" fn(usize, usize, u32, usize, i32, usize, u8) -> i64;
type ForwardBool1 = unsafe extern "system" fn(usize) -> u8;
type ForwardBool3 = unsafe extern "system" fn(usize, usize, u8) -> u8;
type ForwardOverlayDirectFlip =
    unsafe extern "system" fn(usize, usize, usize, usize, u32, u8) -> u8;
type ForwardCompVisual = unsafe extern "system" fn(usize, usize, usize) -> u8;

static PRESENT_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static DIRECT_FLIP_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static OVERLAYS_ENABLED_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static WINDOW_DIRECT_FLIP_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static COMP_SWAP_CHAIN_DIRECT_FLIP_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static COMP_SWAP_CHAIN_INDEPENDENT_FLIP_ORIGINAL: AtomicPtr<c_void> =
    AtomicPtr::new(ptr::null_mut());
static COMP_VISUAL_PROMOTION_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

const MAX_DIRTY_RECTS: usize = 4096;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RectVec {
    start: *const DirtyRect,
    end: *const DirtyRect,
    capacity_end: *const DirtyRect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PresentInputCollection {
    MissingFormat(PresentInputsWithoutFormat),
    HardwareProtected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PresentInputsWithoutFormat {
    clip_box: ClipBox,
    dirty_rects: Vec<DirtyRect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresentInputError {
    MissingProfile,
    NullOverlaySwapChain,
    NullContextState,
    InvalidDirtyRectVector,
    UnreadableMemory,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryBasicInformation {
    base_address: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protect: u32,
    partition_id: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    type_: u32,
}

#[cfg(not(test))]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn ReadProcessMemory(
        hProcess: *mut c_void,
        lpBaseAddress: *const c_void,
        lpBuffer: *mut c_void,
        nSize: usize,
        lpNumberOfBytesRead: *mut usize,
    ) -> i32;
    fn VirtualQuery(
        lpAddress: *const c_void,
        lpBuffer: *mut MemoryBasicInformation,
        dwLength: usize,
    ) -> usize;
}

const MEM_COMMIT: u32 = 0x1000;
const PAGE_READONLY: u32 = 0x02;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_WRITECOPY: u32 = 0x08;
#[cfg(test)]
const PAGE_EXECUTE: u32 = 0x10;
const PAGE_EXECUTE_READ: u32 = 0x20;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
const PAGE_GUARD: u32 = 0x100;
const PAGE_PROTECTION_MASK: u32 = 0xff;

fn original_pointer_for_target(target: HookTarget) -> &'static AtomicPtr<c_void> {
    match target {
        HookTarget::Present => &PRESENT_ORIGINAL,
        HookTarget::IsCandidateDirectFlipCompatible => &DIRECT_FLIP_ORIGINAL,
        HookTarget::OverlaysEnabled => &OVERLAYS_ENABLED_ORIGINAL,
        HookTarget::WindowContextIsCandidateDirectFlipCompatible => &WINDOW_DIRECT_FLIP_ORIGINAL,
        HookTarget::CompSwapChainIsCandidateDirectFlipCompatible => {
            &COMP_SWAP_CHAIN_DIRECT_FLIP_ORIGINAL
        }
        HookTarget::CompSwapChainIsCandidateIndependentFlipCompatible => {
            &COMP_SWAP_CHAIN_INDEPENDENT_FLIP_ORIGINAL
        }
        HookTarget::CompVisualIsCandidateForPromotion => &COMP_VISUAL_PROMOTION_ORIGINAL,
        HookTarget::OverlayTestMode => unreachable!("OverlayTestMode is not a function hook"),
    }
}

fn original_slot_for_target(target: HookTarget) -> *mut *mut c_void {
    original_pointer_for_target(target).as_ptr()
}

fn detour_for_target(target: HookTarget) -> *mut c_void {
    match target {
        HookTarget::Present => present_detour as *mut c_void,
        HookTarget::IsCandidateDirectFlipCompatible => direct_flip_detour as *mut c_void,
        HookTarget::OverlaysEnabled => overlays_enabled_detour as *mut c_void,
        HookTarget::WindowContextIsCandidateDirectFlipCompatible => {
            window_direct_flip_detour as *mut c_void
        }
        HookTarget::CompSwapChainIsCandidateDirectFlipCompatible => {
            comp_swap_chain_direct_flip_detour as *mut c_void
        }
        HookTarget::CompSwapChainIsCandidateIndependentFlipCompatible => {
            comp_swap_chain_independent_flip_detour as *mut c_void
        }
        HookTarget::CompVisualIsCandidateForPromotion => {
            comp_visual_promotion_detour as *mut c_void
        }
        HookTarget::OverlayTestMode => unreachable!("OverlayTestMode is not a function hook"),
    }
}

unsafe fn forward_overlay_direct_flip(
    slot: &AtomicPtr<c_void>,
    this: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: u32,
    a6: u8,
) -> u8 {
    let original = slot.load(Ordering::Acquire);
    if original.is_null() {
        return 0;
    }

    let original: ForwardOverlayDirectFlip = unsafe { std::mem::transmute(original) };
    unsafe { original(this, a2, a3, a4, a5, a6) }
}

unsafe fn forward_bool3(slot: &AtomicPtr<c_void>, this: usize, a2: usize, a3: u8) -> u8 {
    let original = slot.load(Ordering::Acquire);
    if original.is_null() {
        return 0;
    }

    let original: ForwardBool3 = unsafe { std::mem::transmute(original) };
    unsafe { original(this, a2, a3) }
}

unsafe fn forward_bool1(slot: &AtomicPtr<c_void>, this: usize) -> u8 {
    let original = slot.load(Ordering::Acquire);
    if original.is_null() {
        return 0;
    }

    let original: ForwardBool1 = unsafe { std::mem::transmute(original) };
    unsafe { original(this) }
}

unsafe fn collect_present_inputs(
    this: usize,
    overlay_swap_chain: usize,
    rect_vec: usize,
) -> Result<PresentInputCollection, PresentInputError> {
    let profile = state::hook_profile().ok_or(PresentInputError::MissingProfile)?;
    unsafe { collect_present_inputs_with_profile(&profile, this, overlay_swap_chain, rect_vec) }
}

unsafe fn collect_present_inputs_with_profile(
    profile: &HookProfile,
    this: usize,
    overlay_swap_chain: usize,
    rect_vec: usize,
) -> Result<PresentInputCollection, PresentInputError> {
    let hypotheses = profile.hypotheses;

    if overlay_swap_chain == 0 {
        return Err(PresentInputError::NullOverlaySwapChain);
    }

    let hardware_protected = unsafe {
        read_memory::<u8>(checked_address(
            overlay_swap_chain,
            hypotheses.hardware_protected.offset,
        )?)? != 0
    };
    if hardware_protected {
        return Ok(PresentInputCollection::HardwareProtected);
    }
    let clip_box = unsafe {
        read_clip_box(
            this,
            hypotheses.clip_box.context_state_pointer_offset,
            hypotheses.clip_box.offset,
        )?
    };
    let dirty_rects = unsafe { read_dirty_rects(rect_vec)? };
    Ok(PresentInputCollection::MissingFormat(
        PresentInputsWithoutFormat {
            clip_box,
            dirty_rects,
        },
    ))
}

unsafe fn read_clip_box(
    context_address: usize,
    context_state_pointer_offset: usize,
    clip_box_offset: usize,
) -> Result<ClipBox, PresentInputError> {
    let state_pointer_address = checked_address(context_address, context_state_pointer_offset)?;
    let state_object = unsafe { read_memory::<usize>(state_pointer_address)? };
    if state_object == 0 {
        return Err(PresentInputError::NullContextState);
    }

    let origin =
        unsafe { read_memory::<[i32; 2]>(checked_address(state_object, clip_box_offset)?)? };
    Ok(ClipBox {
        left: origin[0],
        top: origin[1],
        right: origin[0],
        bottom: origin[1],
    })
}

unsafe fn read_dirty_rects(rect_vec: usize) -> Result<Vec<DirtyRect>, PresentInputError> {
    if rect_vec == 0 {
        return Err(PresentInputError::InvalidDirtyRectVector);
    }

    let rect_vec = unsafe { read_memory::<RectVec>(rect_vec)? };
    let start = rect_vec.start as usize;
    let end = rect_vec.end as usize;
    let capacity_end = rect_vec.capacity_end as usize;
    if start == 0 && end == 0 && capacity_end == 0 {
        return Ok(Vec::new());
    }
    if start == 0
        || end < start
        || capacity_end < end
        || !start.is_multiple_of(align_of::<DirtyRect>())
    {
        return Err(PresentInputError::InvalidDirtyRectVector);
    }

    let byte_len = end - start;
    if !byte_len.is_multiple_of(size_of::<DirtyRect>()) {
        return Err(PresentInputError::InvalidDirtyRectVector);
    }
    if !(capacity_end - start).is_multiple_of(size_of::<DirtyRect>()) {
        return Err(PresentInputError::InvalidDirtyRectVector);
    }

    let count = byte_len / size_of::<DirtyRect>();
    if count > MAX_DIRTY_RECTS {
        return Err(PresentInputError::InvalidDirtyRectVector);
    }
    if count > 0 && !is_readable_range(start, byte_len) {
        return Err(PresentInputError::UnreadableMemory);
    }

    let mut dirty_rects = Vec::with_capacity(count);
    for index in 0..count {
        let offset = index * size_of::<DirtyRect>();
        dirty_rects.push(unsafe { read_memory::<DirtyRect>(checked_address(start, offset)?)? });
    }

    Ok(dirty_rects)
}

fn checked_address(base: usize, offset: usize) -> Result<usize, PresentInputError> {
    base.checked_add(offset)
        .ok_or(PresentInputError::UnreadableMemory)
}

unsafe fn read_memory<T: Copy>(address: usize) -> Result<T, PresentInputError> {
    if !is_readable_range(address, size_of::<T>()) {
        return Err(PresentInputError::UnreadableMemory);
    }

    #[cfg(test)]
    {
        Ok(unsafe { (address as *const T).read_unaligned() })
    }

    #[cfg(not(test))]
    {
        let mut value = MaybeUninit::<T>::uninit();
        let mut bytes_read = 0usize;
        let success = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                address as *const c_void,
                value.as_mut_ptr().cast(),
                size_of::<T>(),
                &mut bytes_read,
            )
        };
        if success == 0 || bytes_read != size_of::<T>() {
            return Err(PresentInputError::UnreadableMemory);
        }
        Ok(unsafe { value.assume_init() })
    }
}

fn is_readable_range(address: usize, size: usize) -> bool {
    if address == 0 || size == 0 {
        return false;
    }
    let Some(end) = address.checked_add(size - 1) else {
        return false;
    };

    #[cfg(test)]
    {
        let _ = end;
        true
    }

    #[cfg(not(test))]
    {
        is_readable_range_in_process(address, end)
    }
}

#[cfg(not(test))]
fn is_readable_range_in_process(mut address: usize, end: usize) -> bool {
    while address <= end {
        let Some(info) = query_memory(address) else {
            return false;
        };
        if !is_readable_memory_region(&info) {
            return false;
        }

        let region_start = info.base_address as usize;
        let Some(region_end) = region_start.checked_add(info.region_size.saturating_sub(1)) else {
            return false;
        };
        if region_end >= end {
            return true;
        }
        address = match region_end.checked_add(1) {
            Some(next) if next > address => next,
            _ => return false,
        };
    }

    true
}

#[cfg(not(test))]
fn query_memory(address: usize) -> Option<MemoryBasicInformation> {
    let mut info = MemoryBasicInformation {
        base_address: ptr::null_mut(),
        allocation_base: ptr::null_mut(),
        allocation_protect: 0,
        partition_id: 0,
        region_size: 0,
        state: 0,
        protect: 0,
        type_: 0,
    };
    let written = unsafe {
        VirtualQuery(
            address as *const c_void,
            &mut info,
            size_of::<MemoryBasicInformation>(),
        )
    };
    (written != 0).then_some(info)
}

fn is_readable_memory_region(info: &MemoryBasicInformation) -> bool {
    if info.state != MEM_COMMIT || (info.protect & PAGE_GUARD) != 0 {
        return false;
    }

    matches!(
        info.protect & PAGE_PROTECTION_MASK,
        PAGE_READONLY
            | PAGE_READWRITE
            | PAGE_WRITECOPY
            | PAGE_EXECUTE_READ
            | PAGE_EXECUTE_READWRITE
            | PAGE_EXECUTE_WRITECOPY
    )
}

fn deactivate_present_context(context_address: usize) {
    let _ = state::evaluate_present_hook(
        context_address,
        ClipBox {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        0,
        &[],
        false,
    );
}

unsafe extern "system" fn present_detour(
    this: usize,
    overlay_swap_chain: usize,
    a3: u32,
    rect_vec: usize,
    a5: i32,
    a6: usize,
    a7: u8,
) -> i64 {
    unsafe { present_detour_impl(this, overlay_swap_chain, a3, rect_vec, a5, a6, a7) }
}

unsafe extern "system" fn present_detour_impl(
    this: usize,
    overlay_swap_chain: usize,
    a3: u32,
    rect_vec: usize,
    a5: i32,
    a6: usize,
    a7: u8,
) -> i64 {
    let original = PRESENT_ORIGINAL.load(Ordering::Acquire);
    if original.is_null() {
        return 0;
    }

    match unsafe { collect_present_inputs(this, overlay_swap_chain, rect_vec) } {
        Ok(PresentInputCollection::MissingFormat(inputs)) => {
            let lut_applied =
                state::render_present_lut(overlay_swap_chain, inputs.clip_box, &inputs.dirty_rects);
            let _ = state::evaluate_present_hook(
                this,
                inputs.clip_box,
                crate::DXGI_FORMAT_B8G8R8A8_UNORM,
                &inputs.dirty_rects,
                lut_applied,
            );
        }
        Ok(PresentInputCollection::HardwareProtected) | Err(_) => deactivate_present_context(this),
    }

    let original: PresentOriginal = unsafe { std::mem::transmute(original) };
    unsafe { original(this, overlay_swap_chain, a3, rect_vec, a5, a6, a7) }
}

unsafe extern "system" fn direct_flip_detour(
    this: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: u32,
    a6: u8,
) -> u8 {
    let original =
        unsafe { forward_overlay_direct_flip(&DIRECT_FLIP_ORIGINAL, this, a2, a3, a4, a5, a6) };
    bool_to_u8(state::evaluate_direct_flip_compatible(this, original != 0).unwrap_or(original != 0))
}

unsafe extern "system" fn overlays_enabled_detour(this: usize) -> u8 {
    let original = unsafe { forward_bool1(&OVERLAYS_ENABLED_ORIGINAL, this) };
    bool_to_u8(state::evaluate_overlays_enabled(this, original != 0).unwrap_or(original != 0))
}

unsafe extern "system" fn window_direct_flip_detour(this: usize, a2: usize, a3: u8) -> u8 {
    let original = unsafe { forward_bool3(&WINDOW_DIRECT_FLIP_ORIGINAL, this, a2, a3) };
    bool_to_u8(
        state::evaluate_window_context_direct_flip_compatible(original != 0)
            .unwrap_or(original != 0),
    )
}

unsafe extern "system" fn comp_swap_chain_direct_flip_detour(this: usize, a2: usize, a3: u8) -> u8 {
    let original = unsafe { forward_bool3(&COMP_SWAP_CHAIN_DIRECT_FLIP_ORIGINAL, this, a2, a3) };
    bool_to_u8(
        state::evaluate_comp_swap_chain_direct_flip_compatible(original != 0)
            .unwrap_or(original != 0),
    )
}

unsafe extern "system" fn comp_swap_chain_independent_flip_detour(this: usize) -> u8 {
    let original = unsafe { forward_bool1(&COMP_SWAP_CHAIN_INDEPENDENT_FLIP_ORIGINAL, this) };
    bool_to_u8(
        state::evaluate_comp_swap_chain_independent_flip_compatible(original != 0)
            .unwrap_or(original != 0),
    )
}

unsafe extern "system" fn comp_visual_promotion_detour(this: usize, a2: usize, a3: usize) -> u8 {
    let original = COMP_VISUAL_PROMOTION_ORIGINAL.load(Ordering::Acquire);
    if original.is_null() {
        return 0;
    }

    let original: ForwardCompVisual = unsafe { std::mem::transmute(original) };
    let original = unsafe { original(this, a2, a3) };
    bool_to_u8(
        state::evaluate_comp_visual_candidate_for_promotion(original != 0).unwrap_or(original != 0),
    )
}

const fn bool_to_u8(value: bool) -> u8 {
    value as u8
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::fs;
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::profile::HookTarget;
    use crate::resolver::{LoadedModule, ResolvedTarget, SignatureResolutionReport};
    use crate::state::{self, HookConfig, HookRegistrationPlan, HookRegistrationTarget};
    use crate::{
        BuildProfile, ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DirtyRect, HookProfile, SignatureLocator,
    };

    use super::{
        MinHookApiAddresses, MinHookCleanupOperation, MinHookOperation, MinHookRuntime,
        MinHookState, register_plan_with_apis, test_minhook_apis, test_minhook_call_counts,
        unregister_registered_hooks, unregister_registered_hooks_with_apis,
    };

    unsafe extern "system" fn returns_true_overlay_direct_flip(
        _a0: usize,
        _a1: usize,
        _a2: usize,
        _a3: usize,
        _a4: u32,
        _a5: u8,
    ) -> u8 {
        1
    }

    unsafe extern "system" fn returns_true_1(_a0: usize) -> u8 {
        1
    }

    unsafe extern "system" fn returns_true_3(_a0: usize, _a1: usize, _a2: u8) -> u8 {
        1
    }

    unsafe extern "system" fn returns_true_comp_visual(_a0: usize, _a1: usize, _a2: usize) -> u8 {
        1
    }

    unsafe extern "system" fn returns_present_status(
        _a0: usize,
        _a1: usize,
        _a2: u32,
        _a3: usize,
        _a4: i32,
        _a5: usize,
        _a6: u8,
    ) -> i64 {
        0x55
    }

    static CONTROLLED_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn plan_with_targets(targets: &[(HookTarget, usize)]) -> HookRegistrationPlan {
        HookRegistrationPlan {
            module_name: "dwmcore.dll",
            module_base_address: 0x1800_0000,
            module_size: 0x20_0000,
            targets: targets
                .iter()
                .map(|(target, address)| HookRegistrationTarget {
                    target: *target,
                    capture_key: "test",
                    address: *address,
                })
                .collect(),
        }
    }

    fn reset_controlled_behavior(create_fail_on: Option<usize>, enable_fail_on: Option<usize>) {
        super::reset_test_original_slots();
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

    fn test_cube_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dwm-lut-minhook-test-{unique}.cube"));
        fs::write(
            &path,
            "LUT_3D_SIZE 2\n\
0.0 0.0 0.0\n\
1.0 0.0 0.0\n\
0.0 1.0 0.0\n\
1.0 1.0 0.0\n\
0.0 0.0 1.0\n\
1.0 0.0 1.0\n\
0.0 1.0 1.0\n\
1.0 1.0 1.0\n",
        )
        .expect("cube file should be written");
        path
    }

    fn write_test_manifest(cube_path: &Path) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dwm-lut-minhook-test-{unique}.json"));
        let cube_path = cube_path.display().to_string().replace('\\', "\\\\");
        fs::write(
            &path,
            format!(
                "{{\n  \"assignments\": [\n    {{\n      \"monitor_id\": \"DISPLAY1\",\n      \"desktop_left\": 0,\n      \"desktop_top\": 0,\n      \"color_mode\": \"sdr\",\n      \"lut_path\": \"{cube_path}\",\n      \"lut_size\": 2\n    }}\n  ]\n}}\n"
            ),
        )
        .expect("manifest file should be written");
        path
    }

    fn synthetic_resolution(profile: &HookProfile) -> SignatureResolutionReport {
        let base_address = 0x1800_0000usize;
        SignatureResolutionReport {
            module: LoadedModule {
                module_name: profile.module_name,
                base_address,
                size: 0x20_0000,
            },
            targets: profile
                .signatures
                .iter()
                .enumerate()
                .map(|(index, signature)| {
                    let capture_key = match &signature.locator {
                        SignatureLocator::Aob { capture_key, .. } => *capture_key,
                        SignatureLocator::AobExcludingFollowingBytes { capture_key, .. } => {
                            *capture_key
                        }
                        SignatureLocator::RipRelativeGlobalAob { capture_key, .. } => *capture_key,
                        SignatureLocator::FollowingAob { capture_key, .. } => *capture_key,
                    };

                    ResolvedTarget {
                        target: signature.target,
                        capture_key,
                        address: if signature.target == HookTarget::OverlayTestMode {
                            0
                        } else {
                            base_address + 0x1000 + index * 0x100
                        },
                    }
                })
                .collect(),
        }
    }

    fn initialize_test_state() -> (PathBuf, PathBuf) {
        state::reset_state_for_tests();
        let cube_path = test_cube_path();
        let manifest_path = write_test_manifest(&cube_path);
        let config = HookConfig {
            manifest_path: manifest_path.clone(),
            profile: BuildProfile::Windows11_25H2,
        };
        let resolution = synthetic_resolution(&HookProfile::for_build(config.profile));
        crate::bootstrap::initialize_with_resolution(config, resolution)
            .expect("initialization should succeed with synthetic resolution");
        (manifest_path, cube_path)
    }

    fn activate_context(context_address: usize) {
        let dirty_rects = [DirtyRect {
            left: 0,
            top: 0,
            right: 64,
            bottom: 64,
        }];
        state::evaluate_present_hook(
            context_address,
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &dirty_rects,
            true,
        )
        .expect("present evaluation should run");
    }

    struct FakePresentObjects {
        context: Box<usize>,
        _context_state: Vec<usize>,
        overlay_swap_chain: Vec<usize>,
        dirty_rects: Vec<DirtyRect>,
        rect_vec: super::RectVec,
    }

    impl FakePresentObjects {
        fn new(clip_box: ClipBox, dirty_rects: Vec<DirtyRect>, hardware_protected: bool) -> Self {
            let profile = HookProfile::for_build(BuildProfile::Windows11_25H2);
            let context_state_len = (profile.hypotheses.clip_box.offset + size_of::<ClipBox>())
                .div_ceil(size_of::<usize>());
            let mut context_state = vec![0usize; context_state_len];
            unsafe {
                ((context_state.as_mut_ptr() as *mut u8).add(profile.hypotheses.clip_box.offset)
                    as *mut ClipBox)
                    .write(clip_box);
            }

            let context = Box::new(context_state.as_ptr() as usize);

            let overlay_swap_chain_len =
                (profile.hypotheses.hardware_protected.offset + 1).div_ceil(size_of::<usize>());
            let mut overlay_swap_chain = vec![0usize; overlay_swap_chain_len];
            unsafe {
                (overlay_swap_chain.as_mut_ptr() as *mut u8)
                    .add(profile.hypotheses.hardware_protected.offset)
                    .write(u8::from(hardware_protected));
            }

            let rect_vec = if dirty_rects.is_empty() {
                super::RectVec {
                    start: std::ptr::null(),
                    end: std::ptr::null(),
                    capacity_end: std::ptr::null(),
                }
            } else {
                let start = dirty_rects.as_ptr();
                super::RectVec {
                    start,
                    end: unsafe { start.add(dirty_rects.len()) },
                    capacity_end: unsafe { start.add(dirty_rects.capacity()) },
                }
            };

            Self {
                context,
                _context_state: context_state,
                overlay_swap_chain,
                dirty_rects,
                rect_vec,
            }
        }

        fn context_address(&self) -> usize {
            (&*self.context as *const usize) as usize
        }

        fn overlay_swap_chain_address(&self) -> usize {
            self.overlay_swap_chain.as_ptr() as usize
        }

        fn rect_vec_address(&self) -> usize {
            (&self.rect_vec as *const super::RectVec) as usize
        }
    }

    #[test]
    fn registration_maps_targets_to_detours_and_original_slots() {
        let plan = plan_with_targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::OverlaysEnabled, 0x1800_2000),
        ]);

        super::reset_test_minhook_behavior(None, None, None, None);
        let registered = register_plan_with_apis(&plan, test_minhook_apis())
            .expect("registration should succeed");

        assert_eq!(registered.len(), 2);
        assert_eq!(registered[0].target, HookTarget::Present);
        assert_eq!(registered[0].target_address, 0x1800_1000);
        assert_eq!(
            super::PRESENT_ORIGINAL.load(Ordering::Acquire) as usize,
            0x1800_1000
        );
        assert_eq!(registered[1].target, HookTarget::OverlaysEnabled);
        assert_eq!(registered[1].target_address, 0x1800_2000);
        assert_eq!(
            super::OVERLAYS_ENABLED_ORIGINAL.load(Ordering::Acquire) as usize,
            0x1800_2000
        );
        super::PRESENT_ORIGINAL.store(std::ptr::null_mut(), Ordering::Release);
        super::OVERLAYS_ENABLED_ORIGINAL.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn enable_all_hooks_uses_minhook_null_sentinel() {
        assert_eq!(super::MH_ALL_HOOKS, std::ptr::null_mut());
    }

    #[test]
    fn create_failure_removes_previously_created_hooks() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(Some(3), None);
        let plan = plan_with_targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::OverlaysEnabled, 0x1800_2000),
            (HookTarget::CompVisualIsCandidateForPromotion, 0x1800_3000),
        ]);

        let error = register_plan_with_apis(&plan, test_minhook_apis())
            .expect_err("third create should fail");

        assert_eq!(
            error.operation,
            MinHookOperation::CreateHook(HookTarget::CompVisualIsCandidateForPromotion)
        );
        assert!(error.cleanup_failures.is_empty());
        let calls = test_minhook_call_counts();
        assert_eq!(calls.create_calls, 3);
        assert_eq!(calls.enable_calls, 0);
        assert_eq!(calls.disable_calls, 0);
        assert_eq!(calls.remove_calls, 2);
    }

    #[test]
    fn enable_failure_removes_created_hooks() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, Some(1));
        let plan = plan_with_targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::OverlaysEnabled, 0x1800_2000),
            (HookTarget::CompVisualIsCandidateForPromotion, 0x1800_3000),
        ]);

        let error =
            register_plan_with_apis(&plan, test_minhook_apis()).expect_err("enable should fail");

        assert_eq!(error.operation, MinHookOperation::EnableHook);
        assert!(error.cleanup_failures.is_empty());
        let calls = test_minhook_call_counts();
        assert_eq!(calls.create_calls, 3);
        assert_eq!(calls.enable_calls, 1);
        assert_eq!(calls.disable_calls, 0);
        assert_eq!(calls.remove_calls, 3);
    }

    #[test]
    fn create_failure_reports_cleanup_remove_failure() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(Some(3), None);
        set_controlled_cleanup_failures(None, Some(1));
        let plan = plan_with_targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::OverlaysEnabled, 0x1800_2000),
            (HookTarget::CompVisualIsCandidateForPromotion, 0x1800_3000),
        ]);

        let error = register_plan_with_apis(&plan, test_minhook_apis())
            .expect_err("third create should fail");

        assert_eq!(
            error.operation,
            MinHookOperation::CreateHook(HookTarget::CompVisualIsCandidateForPromotion)
        );
        assert_eq!(error.cleanup_failures.len(), 1);
        assert_eq!(
            error.cleanup_failures[0].operation,
            MinHookCleanupOperation::RemoveHook
        );
        assert_eq!(
            error.cleanup_failures[0].target,
            HookTarget::OverlaysEnabled
        );
        assert_eq!(error.cleanup_failures[0].status, -4);
        assert!(super::PRESENT_ORIGINAL.load(Ordering::Acquire).is_null());
        assert_eq!(
            super::OVERLAYS_ENABLED_ORIGINAL.load(Ordering::Acquire) as usize,
            0x1800_2000
        );
        super::OVERLAYS_ENABLED_ORIGINAL.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn enable_failure_keeps_original_slot_when_cleanup_remove_fails() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, Some(1));
        set_controlled_cleanup_failures(None, Some(1));
        let plan = plan_with_targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::OverlaysEnabled, 0x1800_2000),
            (HookTarget::CompVisualIsCandidateForPromotion, 0x1800_3000),
        ]);

        let error =
            register_plan_with_apis(&plan, test_minhook_apis()).expect_err("enable should fail");

        assert_eq!(error.operation, MinHookOperation::EnableHook);
        assert_eq!(error.cleanup_failures.len(), 1);
        assert_eq!(
            error.cleanup_failures[0].operation,
            MinHookCleanupOperation::RemoveHook
        );
        assert_eq!(
            error.cleanup_failures[0].target,
            HookTarget::CompVisualIsCandidateForPromotion
        );
        assert_eq!(error.cleanup_failures[0].status, -4);
        assert!(super::PRESENT_ORIGINAL.load(Ordering::Acquire).is_null());
        assert!(
            super::OVERLAYS_ENABLED_ORIGINAL
                .load(Ordering::Acquire)
                .is_null()
        );
        assert_eq!(
            super::COMP_VISUAL_PROMOTION_ORIGINAL.load(Ordering::Acquire) as usize,
            0x1800_3000
        );
        super::COMP_VISUAL_PROMOTION_ORIGINAL.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn unregister_disables_and_removes_registered_hooks() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, None);
        let plan = plan_with_targets(&[
            (HookTarget::Present, 0x1800_1000),
            (HookTarget::OverlaysEnabled, 0x1800_2000),
        ]);

        let registered = register_plan_with_apis(&plan, test_minhook_apis())
            .expect("registration should succeed");
        let cleanup_failures =
            unregister_registered_hooks_with_apis(&registered, test_minhook_apis());

        let calls = test_minhook_call_counts();
        assert_eq!(calls.create_calls, 2);
        assert_eq!(calls.enable_calls, 1);
        assert_eq!(calls.disable_calls, 2);
        assert_eq!(calls.remove_calls, 2);
        assert!(cleanup_failures.is_empty());
        assert!(super::PRESENT_ORIGINAL.load(Ordering::Acquire).is_null());
        assert!(
            super::OVERLAYS_ENABLED_ORIGINAL
                .load(Ordering::Acquire)
                .is_null()
        );
    }

    #[test]
    fn unregister_keeps_original_slot_when_remove_fails() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, None);
        set_controlled_cleanup_failures(None, Some(1));
        let plan = plan_with_targets(&[(HookTarget::Present, 0x1800_1000)]);

        let registered = register_plan_with_apis(&plan, test_minhook_apis())
            .expect("registration should succeed");
        let cleanup_failures =
            unregister_registered_hooks_with_apis(&registered, test_minhook_apis());

        assert_eq!(cleanup_failures.len(), 1);
        assert_eq!(
            cleanup_failures[0].operation,
            MinHookCleanupOperation::RemoveHook
        );
        assert_eq!(cleanup_failures[0].target, HookTarget::Present);
        assert_eq!(
            super::PRESENT_ORIGINAL.load(Ordering::Acquire) as usize,
            0x1800_1000
        );
        super::PRESENT_ORIGINAL.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn unregister_keeps_minhook_runtime_when_cleanup_fails() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, None);
        set_controlled_cleanup_failures(None, Some(1));
        let plan = plan_with_targets(&[(HookTarget::Present, 0x1800_1000)]);
        let apis = test_minhook_apis();
        let registered = register_plan_with_apis(&plan, apis).expect("registration should succeed");
        let runtime = MinHookRuntime {
            state: MinHookState {
                module_name: "test-minhook",
                module_handle: 0,
                owns_initialization: true,
            },
            api_addresses: MinHookApiAddresses::from_apis(apis),
        };

        let cleanup_failures = unregister_registered_hooks(&runtime, &registered);

        assert_eq!(cleanup_failures.len(), 1);
        assert_eq!(test_minhook_call_counts().uninitialize_calls, 0);
        super::PRESENT_ORIGINAL.store(std::ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn unregister_uninitializes_when_only_disable_cleanup_fails() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        reset_controlled_behavior(None, None);
        set_controlled_cleanup_failures(Some(1), None);
        let plan = plan_with_targets(&[(HookTarget::Present, 0x1800_1000)]);
        let apis = test_minhook_apis();
        let registered = register_plan_with_apis(&plan, apis).expect("registration should succeed");
        let runtime = MinHookRuntime {
            state: MinHookState {
                module_name: "test-minhook",
                module_handle: 0,
                owns_initialization: true,
            },
            api_addresses: MinHookApiAddresses::from_apis(apis),
        };

        let cleanup_failures = unregister_registered_hooks(&runtime, &registered);

        assert_eq!(cleanup_failures.len(), 1);
        assert_eq!(
            cleanup_failures[0].operation,
            MinHookCleanupOperation::DisableHook
        );
        assert_eq!(test_minhook_call_counts().uninitialize_calls, 1);
        assert!(super::PRESENT_ORIGINAL.load(Ordering::Acquire).is_null());
    }

    #[test]
    fn context_detours_override_original_return_value_when_context_is_active() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        let (manifest_path, cube_path) = initialize_test_state();
        activate_context(0x1234);
        super::DIRECT_FLIP_ORIGINAL.store(
            returns_true_overlay_direct_flip as *mut c_void,
            Ordering::Release,
        );
        super::OVERLAYS_ENABLED_ORIGINAL.store(returns_true_1 as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe { super::direct_flip_detour(0x1234, 0, 0, 0, 0, 0) },
            0
        );
        assert_eq!(unsafe { super::overlays_enabled_detour(0x1234) }, 0);
        assert!(
            state::lut_bypass_runtime()
                .and_then(|runtime| runtime.context(0x1234).cloned())
                .is_some()
        );

        let _ = fs::remove_file(manifest_path);
        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn global_promotion_detours_forward_original_return_value() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        state::reset_state_for_tests();
        super::WINDOW_DIRECT_FLIP_ORIGINAL.store(returns_true_3 as *mut c_void, Ordering::Release);
        super::COMP_SWAP_CHAIN_DIRECT_FLIP_ORIGINAL
            .store(returns_true_3 as *mut c_void, Ordering::Release);
        super::COMP_SWAP_CHAIN_INDEPENDENT_FLIP_ORIGINAL
            .store(returns_true_1 as *mut c_void, Ordering::Release);
        super::COMP_VISUAL_PROMOTION_ORIGINAL
            .store(returns_true_comp_visual as *mut c_void, Ordering::Release);

        assert_eq!(unsafe { super::window_direct_flip_detour(0, 0, 0) }, 1);
        assert_eq!(
            unsafe { super::comp_swap_chain_direct_flip_detour(0, 0, 0) },
            1
        );
        assert_eq!(
            unsafe { super::comp_swap_chain_independent_flip_detour(0) },
            1
        );
        assert_eq!(unsafe { super::comp_visual_promotion_detour(0, 0, 0) }, 1);
    }

    #[test]
    fn global_promotion_detours_block_when_lut_assignments_exist() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        let (manifest_path, cube_path) = initialize_test_state();
        super::WINDOW_DIRECT_FLIP_ORIGINAL.store(returns_true_3 as *mut c_void, Ordering::Release);
        super::COMP_SWAP_CHAIN_DIRECT_FLIP_ORIGINAL
            .store(returns_true_3 as *mut c_void, Ordering::Release);
        super::COMP_SWAP_CHAIN_INDEPENDENT_FLIP_ORIGINAL
            .store(returns_true_1 as *mut c_void, Ordering::Release);
        super::COMP_VISUAL_PROMOTION_ORIGINAL
            .store(returns_true_comp_visual as *mut c_void, Ordering::Release);

        assert_eq!(unsafe { super::window_direct_flip_detour(0, 0, 0) }, 0);
        assert_eq!(
            unsafe { super::comp_swap_chain_direct_flip_detour(0, 0, 0) },
            0
        );
        assert_eq!(
            unsafe { super::comp_swap_chain_independent_flip_detour(0) },
            0
        );
        assert_eq!(unsafe { super::comp_visual_promotion_detour(0, 0, 0) }, 0);

        let _ = fs::remove_file(manifest_path);
        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn present_input_collection_reads_confirmed_inputs_without_swap_chain_accessor() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        let (manifest_path, cube_path) = initialize_test_state();
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 120,
                top: 80,
                right: 1920,
                bottom: 1080,
            },
            vec![DirtyRect {
                left: 10,
                top: 20,
                right: 30,
                bottom: 40,
            }],
            false,
        );

        let inputs = unsafe {
            super::collect_present_inputs_with_profile(
                &HookProfile::for_build(BuildProfile::Windows11_25H2),
                fake.context_address(),
                fake.overlay_swap_chain_address(),
                fake.rect_vec_address(),
            )
        }
        .expect("present inputs should be collected");
        let super::PresentInputCollection::MissingFormat(inputs) = inputs else {
            panic!("present inputs should be collected without format");
        };

        assert_eq!(inputs.clip_box.left, 120);
        assert_eq!(inputs.clip_box.top, 80);
        assert_eq!(inputs.dirty_rects, fake.dirty_rects);

        let _ = fs::remove_file(manifest_path);
        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn present_input_collection_skips_swap_chain_when_hardware_protected() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        let (manifest_path, cube_path) = initialize_test_state();
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            Vec::new(),
            true,
        );

        let inputs = unsafe {
            super::collect_present_inputs_with_profile(
                &HookProfile::for_build(BuildProfile::Windows11_25H2),
                fake.context_address(),
                fake.overlay_swap_chain_address(),
                fake.rect_vec_address(),
            )
        }
        .expect("hardware protected state should be collected");

        assert!(matches!(
            inputs,
            super::PresentInputCollection::HardwareProtected
        ));

        let _ = fs::remove_file(manifest_path);
        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn present_input_collection_rejects_dirty_rect_vector_past_capacity() {
        let rects = [DirtyRect {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        }];
        let rect_vec = super::RectVec {
            start: rects.as_ptr(),
            end: unsafe { rects.as_ptr().add(1) },
            capacity_end: rects.as_ptr(),
        };

        let error =
            unsafe { super::read_dirty_rects((&rect_vec as *const super::RectVec) as usize) }
                .expect_err("end past capacity should be rejected");

        assert_eq!(error, super::PresentInputError::InvalidDirtyRectVector);
    }

    #[test]
    fn present_input_collection_rejects_misaligned_dirty_rect_vector() {
        let start = std::ptr::dangling::<DirtyRect>() as usize + 1;
        let rect_vec = super::RectVec {
            start: start as *const DirtyRect,
            end: (start + size_of::<DirtyRect>()) as *const DirtyRect,
            capacity_end: (start + size_of::<DirtyRect>()) as *const DirtyRect,
        };

        let error =
            unsafe { super::read_dirty_rects((&rect_vec as *const super::RectVec) as usize) }
                .expect_err("misaligned start should be rejected");

        assert_eq!(error, super::PresentInputError::InvalidDirtyRectVector);
    }

    #[test]
    fn pointer_addition_overflow_is_not_treated_as_readable_memory() {
        let error = super::checked_address(usize::MAX, 1)
            .expect_err("overflowed address should be rejected");

        assert_eq!(error, super::PresentInputError::UnreadableMemory);
    }

    #[test]
    fn null_dirty_rect_vector_is_invalid_present_input() {
        let error = unsafe { super::read_dirty_rects(0) }
            .expect_err("null rectVec pointer should be rejected");

        assert_eq!(error, super::PresentInputError::InvalidDirtyRectVector);
    }

    #[test]
    fn memory_region_readability_requires_read_protection() {
        let readable = super::MemoryBasicInformation {
            base_address: std::ptr::null_mut(),
            allocation_base: std::ptr::null_mut(),
            allocation_protect: 0,
            partition_id: 0,
            region_size: 4096,
            state: super::MEM_COMMIT,
            protect: super::PAGE_READONLY,
            type_: 0,
        };
        let execute_only = super::MemoryBasicInformation {
            protect: super::PAGE_EXECUTE,
            ..readable
        };
        let guarded = super::MemoryBasicInformation {
            protect: super::PAGE_READWRITE | super::PAGE_GUARD,
            ..readable
        };

        assert!(super::is_readable_memory_region(&readable));
        assert!(!super::is_readable_memory_region(&execute_only));
        assert!(!super::is_readable_memory_region(&guarded));
    }

    #[test]
    fn present_detour_keeps_context_active_when_render_succeeds() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        let (manifest_path, cube_path) = initialize_test_state();
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
            false,
        );
        crate::d3d11_renderer::set_test_render_present_lut_result(true);
        super::PRESENT_ORIGINAL.store(returns_present_status as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe {
                super::present_detour(
                    fake.context_address(),
                    fake.overlay_swap_chain_address(),
                    0,
                    fake.rect_vec_address(),
                    0,
                    0,
                    0,
                )
            },
            0x55
        );

        let context = state::lut_bypass_runtime()
            .and_then(|runtime| runtime.context(fake.context_address()).cloned())
            .expect("successful LUT render should keep the context active");
        assert_eq!(context.lut_index, Some(0));
        assert_eq!(context.dirty_rect_count, 1);
        let render_call = crate::d3d11_renderer::test_render_present_lut_call()
            .expect("renderer should be called with collected present inputs");
        assert_eq!(
            render_call.overlay_swap_chain,
            fake.overlay_swap_chain_address()
        );
        assert_eq!(render_call.swap_chain_path.container_vtable_index, 24);
        assert_eq!(render_call.swap_chain_path.resource_vtable_index, 19);
        assert_eq!(render_call.clip_box.left, 0);
        assert_eq!(render_call.clip_box.top, 0);
        assert_eq!(render_call.dirty_rects, fake.dirty_rects);

        let _ = fs::remove_file(manifest_path);
        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn present_detour_clears_context_when_format_is_unavailable() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        let (manifest_path, cube_path) = initialize_test_state();
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
            false,
        );
        activate_context(fake.context_address());
        super::PRESENT_ORIGINAL.store(returns_present_status as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe {
                super::present_detour(
                    fake.context_address(),
                    fake.overlay_swap_chain_address(),
                    0,
                    fake.rect_vec_address(),
                    0,
                    0,
                    0,
                )
            },
            0x55
        );
        assert!(
            state::lut_bypass_runtime()
                .and_then(|runtime| runtime.context(fake.context_address()).cloned())
                .is_none()
        );

        let _ = fs::remove_file(manifest_path);
        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn present_detour_clears_context_when_hardware_protected() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        let (manifest_path, cube_path) = initialize_test_state();
        let fake = FakePresentObjects::new(
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            Vec::new(),
            true,
        );
        activate_context(fake.context_address());
        super::PRESENT_ORIGINAL.store(returns_present_status as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe {
                super::present_detour(
                    fake.context_address(),
                    fake.overlay_swap_chain_address(),
                    0,
                    fake.rect_vec_address(),
                    0,
                    0,
                    0,
                )
            },
            0x55
        );
        assert!(
            state::lut_bypass_runtime()
                .and_then(|runtime| runtime.context(fake.context_address()).cloned())
                .is_none()
        );

        let _ = fs::remove_file(manifest_path);
        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn present_detour_clears_context_when_input_acquisition_fails() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        let (manifest_path, cube_path) = initialize_test_state();
        activate_context(0x1234);
        super::PRESENT_ORIGINAL.store(returns_present_status as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe { super::present_detour(0x1234, 0, 0, 0, 0, 0, 0) },
            0x55
        );
        assert!(
            state::lut_bypass_runtime()
                .and_then(|runtime| runtime.context(0x1234).cloned())
                .is_none()
        );

        let _ = fs::remove_file(manifest_path);
        let _ = fs::remove_file(cube_path);
    }
}
