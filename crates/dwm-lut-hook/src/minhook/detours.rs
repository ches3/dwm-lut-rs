use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU8, Ordering};

use crate::profile::HookTarget;
#[cfg(debug_assertions)]
use crate::route_trace::{FlipGateKind, record_comp_direct_flip_call_summary, record_flip_gate};
use crate::state;

use super::present::present_detour;

type ForwardBool1 = unsafe extern "system" fn(usize) -> u8;
type ForwardBool3 = unsafe extern "system" fn(usize, usize, u8) -> u8;
type ForwardBoolThis2 = unsafe extern "system" fn(usize, usize, usize) -> u8;
type ForwardCheckDirectFlipSupport =
    unsafe extern "system" fn(usize, usize, u32, usize, usize) -> u8;
type ForwardHresult1 = unsafe extern "system" fn(usize) -> i32;
type ForwardOverlayDirectFlip =
    unsafe extern "system" fn(usize, usize, usize, usize, u32, u8) -> u8;
type ForwardCompVisual = unsafe extern "system" fn(usize, usize, usize) -> u8;

static PRESENT_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static DIRECT_FLIP_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static WINDOW_DIRECT_FLIP_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static COMP_SWAP_CHAIN_DIRECT_FLIP_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static COMP_SWAP_CHAIN_INDEPENDENT_FLIP_ORIGINAL: AtomicPtr<c_void> =
    AtomicPtr::new(ptr::null_mut());
static COMP_VISUAL_PROMOTION_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static ENSURE_INDEPENDENT_FLIP_STATE_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static IS_DIRECT_FLIP_SUPPORTED_ON_TARGET_ORIGINAL: AtomicPtr<c_void> =
    AtomicPtr::new(ptr::null_mut());
static LEGACY_CHECK_DIRECT_FLIP_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static IS_ADVANCED_DIRECT_FLIP_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static OVERLAYS_ENABLED_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

const OVERLAYS_ENABLED_ORIGINAL_BEHAVIOR: u8 = 0;
const OVERLAYS_ENABLED_FORCE_FALSE: u8 = 1;
const OVERLAYS_ENABLED_FORCE_TRUE: u8 = 2;

static OVERLAYS_ENABLED_OVERRIDE: AtomicU8 = AtomicU8::new(OVERLAYS_ENABLED_ORIGINAL_BEHAVIOR);

pub(super) fn present_original() -> *mut c_void {
    PRESENT_ORIGINAL.load(Ordering::Acquire)
}

#[cfg(test)]
pub(super) fn reset_test_original_slots() {
    PRESENT_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    DIRECT_FLIP_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    WINDOW_DIRECT_FLIP_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    COMP_SWAP_CHAIN_DIRECT_FLIP_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    COMP_SWAP_CHAIN_INDEPENDENT_FLIP_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    COMP_VISUAL_PROMOTION_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    ENSURE_INDEPENDENT_FLIP_STATE_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    IS_DIRECT_FLIP_SUPPORTED_ON_TARGET_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    LEGACY_CHECK_DIRECT_FLIP_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    IS_ADVANCED_DIRECT_FLIP_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    OVERLAYS_ENABLED_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    set_overlays_enabled_override(None);
}

pub(super) fn set_overlays_enabled_override(value: Option<bool>) {
    let value = match value {
        None => OVERLAYS_ENABLED_ORIGINAL_BEHAVIOR,
        Some(false) => OVERLAYS_ENABLED_FORCE_FALSE,
        Some(true) => OVERLAYS_ENABLED_FORCE_TRUE,
    };
    OVERLAYS_ENABLED_OVERRIDE.store(value, Ordering::Release);
}

pub(super) fn original_pointer_for_target(target: HookTarget) -> &'static AtomicPtr<c_void> {
    match target {
        HookTarget::Present => &PRESENT_ORIGINAL,
        HookTarget::IsCandidateDirectFlipCompatible => &DIRECT_FLIP_ORIGINAL,
        HookTarget::WindowContextIsCandidateDirectFlipCompatible => &WINDOW_DIRECT_FLIP_ORIGINAL,
        HookTarget::CompSwapChainIsCandidateDirectFlipCompatible => {
            &COMP_SWAP_CHAIN_DIRECT_FLIP_ORIGINAL
        }
        HookTarget::CompSwapChainIsCandidateIndependentFlipCompatible => {
            &COMP_SWAP_CHAIN_INDEPENDENT_FLIP_ORIGINAL
        }
        HookTarget::CompVisualIsCandidateForPromotion => &COMP_VISUAL_PROMOTION_ORIGINAL,
        HookTarget::DirectFlipInfoEnsureIndependentFlipState => {
            &ENSURE_INDEPENDENT_FLIP_STATE_ORIGINAL
        }
        HookTarget::IsDirectFlipSupportedOnTarget => &IS_DIRECT_FLIP_SUPPORTED_ON_TARGET_ORIGINAL,
        HookTarget::LegacySwapChainCheckDirectFlipSupport => &LEGACY_CHECK_DIRECT_FLIP_ORIGINAL,
        HookTarget::IsAdvancedDirectFlipCompatible => &IS_ADVANCED_DIRECT_FLIP_ORIGINAL,
        HookTarget::OverlaysEnabled => &OVERLAYS_ENABLED_ORIGINAL,
        HookTarget::OverlayTestMode | HookTarget::DisableIndependentFlip => {
            unreachable!("global patch target is not a function hook")
        }
    }
}

pub(super) fn original_slot_for_target(target: HookTarget) -> *mut *mut c_void {
    original_pointer_for_target(target).as_ptr()
}

pub(super) fn detour_for_target(target: HookTarget) -> *mut c_void {
    match target {
        HookTarget::Present => present_detour as *mut c_void,
        HookTarget::IsCandidateDirectFlipCompatible => direct_flip_detour as *mut c_void,
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
        HookTarget::DirectFlipInfoEnsureIndependentFlipState => {
            ensure_independent_flip_state_detour as *mut c_void
        }
        HookTarget::IsDirectFlipSupportedOnTarget => {
            is_direct_flip_supported_on_target_detour as *mut c_void
        }
        HookTarget::LegacySwapChainCheckDirectFlipSupport => {
            legacy_check_direct_flip_support_detour as *mut c_void
        }
        HookTarget::IsAdvancedDirectFlipCompatible => {
            is_advanced_direct_flip_compatible_detour as *mut c_void
        }
        HookTarget::OverlaysEnabled => overlays_enabled_detour as *mut c_void,
        HookTarget::OverlayTestMode | HookTarget::DisableIndependentFlip => {
            unreachable!("global patch target is not a function hook")
        }
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

fn evaluate_bool_detour(
    #[cfg(debug_assertions)] kind: FlipGateKind,
    original: u8,
    evaluate: impl FnOnce(bool) -> Option<bool>,
) -> u8 {
    if !state::is_runtime_active() {
        return original;
    }
    let original_bool = original != 0;
    let result_bool = evaluate(original_bool).unwrap_or(original_bool);
    #[cfg(debug_assertions)]
    record_flip_gate(kind, original_bool, result_bool);
    bool_to_u8(result_bool)
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
    evaluate_bool_detour(
        #[cfg(debug_assertions)]
        FlipGateKind::OverlayContextDirectFlip,
        original,
        |original| state::evaluate_direct_flip_compatible(this, original),
    )
}

unsafe extern "system" fn window_direct_flip_detour(this: usize, a2: usize, a3: u8) -> u8 {
    let original = unsafe { forward_bool3(&WINDOW_DIRECT_FLIP_ORIGINAL, this, a2, a3) };
    evaluate_bool_detour(
        #[cfg(debug_assertions)]
        FlipGateKind::WindowContextDirectFlip,
        original,
        state::evaluate_window_context_direct_flip_compatible,
    )
}

unsafe extern "system" fn comp_swap_chain_direct_flip_detour(this: usize, a2: usize, a3: u8) -> u8 {
    let original = unsafe { forward_bool3(&COMP_SWAP_CHAIN_DIRECT_FLIP_ORIGINAL, this, a2, a3) };
    let result = evaluate_bool_detour(
        #[cfg(debug_assertions)]
        FlipGateKind::CompSwapChainDirectFlip,
        original,
        state::evaluate_comp_swap_chain_direct_flip_compatible,
    );
    #[cfg(debug_assertions)]
    record_comp_direct_flip_call_summary(this, a2, a3, original != 0, result != 0);
    result
}

unsafe extern "system" fn comp_swap_chain_independent_flip_detour(this: usize) -> u8 {
    let original = unsafe { forward_bool1(&COMP_SWAP_CHAIN_INDEPENDENT_FLIP_ORIGINAL, this) };
    evaluate_bool_detour(
        #[cfg(debug_assertions)]
        FlipGateKind::CompSwapChainIndependentFlip,
        original,
        state::evaluate_comp_swap_chain_independent_flip_compatible,
    )
}

unsafe extern "system" fn comp_visual_promotion_detour(this: usize, a2: usize, a3: usize) -> u8 {
    let original = COMP_VISUAL_PROMOTION_ORIGINAL.load(Ordering::Acquire);
    if original.is_null() {
        return 0;
    }

    let original_fn: ForwardCompVisual = unsafe { std::mem::transmute(original) };
    let original = unsafe { original_fn(this, a2, a3) };
    evaluate_bool_detour(
        #[cfg(debug_assertions)]
        FlipGateKind::CompVisualPromotion,
        original,
        state::evaluate_comp_visual_candidate_for_promotion,
    )
}

unsafe extern "system" fn ensure_independent_flip_state_detour(this: usize) -> i32 {
    if state::is_runtime_active()
        && let Some(blocked_status) = state::evaluate_ensure_independent_flip_state()
    {
        #[cfg(debug_assertions)]
        record_flip_gate(
            FlipGateKind::DirectFlipInfoEnsureIndependentFlip,
            true,
            false,
        );
        return blocked_status;
    }

    let original = ENSURE_INDEPENDENT_FLIP_STATE_ORIGINAL.load(Ordering::Acquire);
    if original.is_null() {
        return 0;
    }

    let original_fn: ForwardHresult1 = unsafe { std::mem::transmute(original) };
    let status = unsafe { original_fn(this) };
    #[cfg(debug_assertions)]
    record_flip_gate(
        FlipGateKind::DirectFlipInfoEnsureIndependentFlip,
        true,
        true,
    );
    status
}

unsafe extern "system" fn is_direct_flip_supported_on_target_detour(
    this: usize,
    a2: usize,
    a3: usize,
) -> u8 {
    let original = IS_DIRECT_FLIP_SUPPORTED_ON_TARGET_ORIGINAL.load(Ordering::Acquire);
    if original.is_null() {
        return 0;
    }
    let original_fn: ForwardBoolThis2 = unsafe { std::mem::transmute(original) };
    let original = unsafe { original_fn(this, a2, a3) };
    evaluate_bool_detour(
        #[cfg(debug_assertions)]
        FlipGateKind::IsDirectFlipSupportedOnTarget,
        original,
        state::evaluate_direct_flip_support_compatible,
    )
}

unsafe extern "system" fn legacy_check_direct_flip_support_detour(
    this: usize,
    a2: usize,
    a3: u32,
    a4: usize,
    a5: usize,
) -> u8 {
    let original = LEGACY_CHECK_DIRECT_FLIP_ORIGINAL.load(Ordering::Acquire);
    if original.is_null() {
        return 0;
    }
    let original_fn: ForwardCheckDirectFlipSupport = unsafe { std::mem::transmute(original) };
    let original = unsafe { original_fn(this, a2, a3, a4, a5) };
    evaluate_bool_detour(
        #[cfg(debug_assertions)]
        FlipGateKind::LegacySwapChainCheckDirectFlip,
        original,
        state::evaluate_direct_flip_support_compatible,
    )
}

unsafe extern "system" fn is_advanced_direct_flip_compatible_detour(this: usize) -> u8 {
    let original = unsafe { forward_bool1(&IS_ADVANCED_DIRECT_FLIP_ORIGINAL, this) };
    evaluate_bool_detour(
        #[cfg(debug_assertions)]
        FlipGateKind::IsAdvancedDirectFlipCompatible,
        original,
        state::evaluate_direct_flip_support_compatible,
    )
}

// DWM callers retain volatile registers across this internal leaf function.
// Keep the detour transparent except for the original AL result and flags.
#[unsafe(naked)]
unsafe extern "system" fn overlays_enabled_detour(_this: usize) -> u8 {
    core::arch::naked_asm!(
        "cmp byte ptr [rip + {override_state}], {original_behavior}",
        "je 2f",
        "cmp byte ptr [rip + {override_state}], {force_true}",
        "sete al",
        "ret",
        "2:",
        "jmp qword ptr [rip + {original_slot}]",
        override_state = sym OVERLAYS_ENABLED_OVERRIDE,
        original_slot = sym OVERLAYS_ENABLED_ORIGINAL,
        original_behavior = const OVERLAYS_ENABLED_ORIGINAL_BEHAVIOR,
        force_true = const OVERLAYS_ENABLED_FORCE_TRUE,
    );
}

const fn bool_to_u8(value: bool) -> u8 {
    value as u8
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::atomic::Ordering;

    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, MonitorIdentity, MonitorTarget, PayloadAssignment,
        PayloadLut,
    };

    use crate::profile::VERSIONED_PROFILES;
    use crate::resolver::{LoadedModule, ResolvedTarget, SignatureResolutionReport};
    use crate::state::{self, PRESENT_RUNTIME_TEST_LOCK as CONTROLLED_TEST_LOCK};
    use crate::{ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DirtyRect, HookProfile};

    fn test_profile() -> HookProfile {
        (VERSIONED_PROFILES[0].build)()
    }

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

    unsafe extern "system" fn returns_hresult_fail(_a0: usize) -> i32 {
        -1
    }

    unsafe extern "system" fn returns_true_this2(_a0: usize, _a1: usize, _a2: usize) -> u8 {
        1
    }

    unsafe extern "system" fn returns_true_check_direct_flip(
        _a0: usize,
        _a1: usize,
        _a2: u32,
        _a3: usize,
        _a4: usize,
    ) -> u8 {
        1
    }

    #[unsafe(naked)]
    unsafe extern "system" fn returns_true_overlays_enabled_original(_this: usize) -> u8 {
        core::arch::naked_asm!("mov al, 1", "ret");
    }

    #[unsafe(naked)]
    unsafe extern "system" fn overlays_enabled_register_probe() -> u8 {
        core::arch::naked_asm!(
            "sub rsp, 40",
            "mov rax, 0x1122334455667701",
            "mov [rsp + 32], rax",
            "mov rax, 0x1122334455667700",
            "mov ecx, 0x11111111",
            "mov edx, 0x22222222",
            "mov r8d, 0x33333333",
            "mov r9d, 0x44444444",
            "mov r10d, 0x55555555",
            "mov r11d, 0x66666666",
            "pcmpeqb xmm0, xmm0",
            "movdqa xmm1, xmm0",
            "movdqa xmm2, xmm0",
            "movdqa xmm3, xmm0",
            "movdqa xmm4, xmm0",
            "movdqa xmm5, xmm0",
            "call {detour}",
            "cmp rax, [rsp + 32]",
            "jne 3f",
            "cmp ecx, 0x11111111",
            "jne 3f",
            "cmp edx, 0x22222222",
            "jne 3f",
            "cmp r8d, 0x33333333",
            "jne 3f",
            "cmp r9d, 0x44444444",
            "jne 3f",
            "cmp r10d, 0x55555555",
            "jne 3f",
            "cmp r11d, 0x66666666",
            "jne 3f",
            "pmovmskb eax, xmm0",
            "cmp eax, 0xffff",
            "jne 3f",
            "pmovmskb eax, xmm1",
            "cmp eax, 0xffff",
            "jne 3f",
            "pmovmskb eax, xmm2",
            "cmp eax, 0xffff",
            "jne 3f",
            "pmovmskb eax, xmm3",
            "cmp eax, 0xffff",
            "jne 3f",
            "pmovmskb eax, xmm4",
            "cmp eax, 0xffff",
            "jne 3f",
            "pmovmskb eax, xmm5",
            "cmp eax, 0xffff",
            "jne 3f",
            "mov eax, 1",
            "add rsp, 40",
            "ret",
            "3:",
            "xor eax, eax",
            "add rsp, 40",
            "ret",
            detour = sym super::overlays_enabled_detour,
        );
    }

    #[test]
    fn overlays_enabled_detour_forwards_or_returns_atomic_override() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        super::OVERLAYS_ENABLED_ORIGINAL.store(
            returns_true_overlays_enabled_original as *mut c_void,
            Ordering::Release,
        );
        super::set_overlays_enabled_override(None);

        assert_eq!(unsafe { super::overlays_enabled_detour(0) }, 1);

        super::set_overlays_enabled_override(Some(false));
        assert_eq!(unsafe { super::overlays_enabled_detour(0) }, 0);

        super::set_overlays_enabled_override(Some(true));
        assert_eq!(unsafe { super::overlays_enabled_detour(0) }, 1);

        super::set_overlays_enabled_override(None);
        super::OVERLAYS_ENABLED_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn overlays_enabled_override_preserves_original_volatile_registers() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        super::set_overlays_enabled_override(Some(true));

        assert_eq!(unsafe { overlays_enabled_register_probe() }, 1);

        super::OVERLAYS_ENABLED_ORIGINAL.store(
            returns_true_overlays_enabled_original as *mut c_void,
            Ordering::Release,
        );
        super::set_overlays_enabled_override(None);
        assert_eq!(unsafe { overlays_enabled_register_probe() }, 1);

        super::OVERLAYS_ENABLED_ORIGINAL.store(ptr::null_mut(), Ordering::Release);
    }

    fn test_monitor_identity() -> MonitorIdentity {
        MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4357,
        }
    }

    fn identity_lut() -> PayloadLut {
        PayloadLut {
            size: 2,
            domain_min: [0.0, 0.0, 0.0],
            domain_max: [1.0, 1.0, 1.0],
            values: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
                [1.0, 1.0, 1.0],
            ],
        }
    }

    fn test_payload(color_modes: &[ColorMode]) -> HookPayload {
        HookPayload {
            assignments: color_modes
                .iter()
                .map(|color_mode| PayloadAssignment {
                    target: MonitorTarget {
                        identity: test_monitor_identity(),
                        color_mode: *color_mode,
                    },
                    lut: identity_lut(),
                })
                .collect(),
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

    fn initialize_test_state() {
        state::reset_state_for_tests();
        let profile = test_profile();
        let resolution = synthetic_resolution(&profile);
        crate::bootstrap::initialize_with_resolution(
            profile,
            test_payload(&[ColorMode::Sdr]),
            resolution,
        )
        .expect("initialization should succeed with synthetic resolution");
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
            Some(test_monitor_identity()),
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

    #[test]
    fn context_detours_override_original_return_value_when_context_is_active() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        initialize_test_state();
        activate_context(0x1234);
        super::DIRECT_FLIP_ORIGINAL.store(
            returns_true_overlay_direct_flip as *mut c_void,
            Ordering::Release,
        );

        assert_eq!(
            unsafe { super::direct_flip_detour(0x1234, 0, 0, 0, 0, 0) },
            0
        );
        assert!(
            state::lut_bypass_runtime()
                .and_then(|runtime| runtime.context(0x1234).cloned())
                .is_some()
        );
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
        super::ENSURE_INDEPENDENT_FLIP_STATE_ORIGINAL
            .store(returns_hresult_fail as *mut c_void, Ordering::Release);
        super::IS_DIRECT_FLIP_SUPPORTED_ON_TARGET_ORIGINAL
            .store(returns_true_this2 as *mut c_void, Ordering::Release);
        super::LEGACY_CHECK_DIRECT_FLIP_ORIGINAL.store(
            returns_true_check_direct_flip as *mut c_void,
            Ordering::Release,
        );
        super::IS_ADVANCED_DIRECT_FLIP_ORIGINAL
            .store(returns_true_1 as *mut c_void, Ordering::Release);

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
        assert_eq!(
            unsafe { super::ensure_independent_flip_state_detour(0) },
            -1
        );
        assert_eq!(
            unsafe { super::is_direct_flip_supported_on_target_detour(0, 0, 0) },
            1
        );
        assert_eq!(
            unsafe { super::legacy_check_direct_flip_support_detour(0, 0, 0, 0, 0) },
            1
        );
        assert_eq!(
            unsafe { super::is_advanced_direct_flip_compatible_detour(0) },
            1
        );
    }

    #[test]
    fn global_promotion_detours_block_when_lut_assignments_exist() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        initialize_test_state();
        super::WINDOW_DIRECT_FLIP_ORIGINAL.store(returns_true_3 as *mut c_void, Ordering::Release);
        super::COMP_SWAP_CHAIN_DIRECT_FLIP_ORIGINAL
            .store(returns_true_3 as *mut c_void, Ordering::Release);
        super::COMP_SWAP_CHAIN_INDEPENDENT_FLIP_ORIGINAL
            .store(returns_true_1 as *mut c_void, Ordering::Release);
        super::COMP_VISUAL_PROMOTION_ORIGINAL
            .store(returns_true_comp_visual as *mut c_void, Ordering::Release);
        super::ENSURE_INDEPENDENT_FLIP_STATE_ORIGINAL
            .store(returns_hresult_fail as *mut c_void, Ordering::Release);
        super::IS_DIRECT_FLIP_SUPPORTED_ON_TARGET_ORIGINAL
            .store(returns_true_this2 as *mut c_void, Ordering::Release);
        super::LEGACY_CHECK_DIRECT_FLIP_ORIGINAL.store(
            returns_true_check_direct_flip as *mut c_void,
            Ordering::Release,
        );
        super::IS_ADVANCED_DIRECT_FLIP_ORIGINAL
            .store(returns_true_1 as *mut c_void, Ordering::Release);

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
        assert_eq!(unsafe { super::ensure_independent_flip_state_detour(0) }, 0);
        assert_eq!(
            unsafe { super::is_direct_flip_supported_on_target_detour(0, 0, 0) },
            0
        );
        assert_eq!(
            unsafe { super::legacy_check_direct_flip_support_detour(0, 0, 0, 0, 0) },
            0
        );
        assert_eq!(
            unsafe { super::is_advanced_direct_flip_compatible_detour(0) },
            0
        );
    }
}
