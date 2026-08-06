use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::flip_gate;
use crate::present;
use dwm_lut_profile::HookTarget;

static PRESENT_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static IS_CANDIDATE_DIRECT_FLIP_COMPATIBLE_ORIGINAL: AtomicPtr<c_void> =
    AtomicPtr::new(ptr::null_mut());
static IS_CANDIDATE_OVERLAY_COMPATIBLE_ORIGINAL: AtomicPtr<c_void> =
    AtomicPtr::new(ptr::null_mut());

pub(crate) fn present_original() -> *mut c_void {
    PRESENT_ORIGINAL.load(Ordering::Acquire)
}

pub(crate) fn original_pointer_for_target(target: HookTarget) -> &'static AtomicPtr<c_void> {
    match target {
        HookTarget::Present => &PRESENT_ORIGINAL,
        HookTarget::IsCandidateDirectFlipCompatible => {
            &IS_CANDIDATE_DIRECT_FLIP_COMPATIBLE_ORIGINAL
        }
        HookTarget::IsCandidateOverlayCompatible => &IS_CANDIDATE_OVERLAY_COMPATIBLE_ORIGINAL,
    }
}

pub(super) fn original_slot_for_target(target: HookTarget) -> *mut *mut c_void {
    original_pointer_for_target(target).as_ptr()
}

pub(super) fn detour_for_target(target: HookTarget) -> *mut c_void {
    match target {
        HookTarget::Present => present_detour as *mut c_void,
        HookTarget::IsCandidateDirectFlipCompatible => {
            is_candidate_direct_flip_compatible_detour as *mut c_void
        }
        HookTarget::IsCandidateOverlayCompatible => {
            is_candidate_overlay_compatible_detour as *mut c_void
        }
    }
}

unsafe extern "system" fn present_detour(
    this: usize,
    overlay_swap_chain: usize,
    a2: usize,
    rect_vec: usize,
    a4: usize,
    a5: usize,
    a6: usize,
) -> i64 {
    type PresentOriginal =
        unsafe extern "system" fn(usize, usize, usize, usize, usize, usize, usize) -> i64;

    let original = present_original();
    if original.is_null() {
        return 0;
    }
    let original: PresentOriginal = unsafe { std::mem::transmute(original) };

    present::present(this, overlay_swap_chain, rect_vec, |rect_vec| unsafe {
        original(this, overlay_swap_chain, a2, rect_vec, a4, a5, a6)
    })
}

fn apply_flip_gate<T: Default>(
    target: HookTarget,
    overlay_context: usize,
    original_slot: &AtomicPtr<c_void>,
    call_original: impl FnOnce(*mut c_void) -> T,
) -> T {
    if flip_gate::should_block(target, overlay_context) {
        return T::default();
    }
    let original = original_slot.load(Ordering::Acquire);
    if original.is_null() {
        return T::default();
    }
    call_original(original)
}

unsafe extern "system" fn is_candidate_direct_flip_compatible_detour(
    this: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
) -> u8 {
    type IsCandidateDirectFlipCompatibleOriginal =
        unsafe extern "system" fn(usize, usize, usize, usize, usize, usize) -> u8;

    apply_flip_gate(
        HookTarget::IsCandidateDirectFlipCompatible,
        this,
        &IS_CANDIDATE_DIRECT_FLIP_COMPATIBLE_ORIGINAL,
        |original| {
            let original_fn: IsCandidateDirectFlipCompatibleOriginal =
                unsafe { std::mem::transmute(original) };
            unsafe { original_fn(this, a1, a2, a3, a4, a5) }
        },
    )
}

unsafe extern "system" fn is_candidate_overlay_compatible_detour(
    this: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
    a7: usize,
    a8: usize,
) -> u8 {
    type IsCandidateOverlayCompatibleOriginal = unsafe extern "system" fn(
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
    ) -> u8;

    apply_flip_gate(
        HookTarget::IsCandidateOverlayCompatible,
        this,
        &IS_CANDIDATE_OVERLAY_COMPATIBLE_ORIGINAL,
        |original| {
            let original_fn: IsCandidateOverlayCompatibleOriginal =
                unsafe { std::mem::transmute(original) };
            unsafe { original_fn(this, a1, a2, a3, a4, a5, a6, a7, a8) }
        },
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::atomic::Ordering;

    use crate::state::HOOK_GLOBAL_TEST_LOCK as CONTROLLED_TEST_LOCK;
    use dwm_lut_profile::HookTarget;

    fn reset_original_slots() {
        for &target in HookTarget::ALL {
            super::original_pointer_for_target(target).store(ptr::null_mut(), Ordering::Release);
        }
    }

    unsafe extern "system" fn returns_fixed_return_value(
        _this: usize,
        _overlay_swap_chain: usize,
        _a2: usize,
        _rect_vec: usize,
        _a4: usize,
        _a5: usize,
        _a6: usize,
    ) -> i64 {
        0x55
    }

    #[test]
    fn present_detour_forwards_original_return_value() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        super::original_pointer_for_target(HookTarget::Present)
            .store(returns_fixed_return_value as *mut c_void, Ordering::Release);

        assert_eq!(unsafe { super::present_detour(0, 0, 0, 0, 0, 0, 0) }, 0x55);
        reset_original_slots();
    }
}
