use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::DirtyRect;
use crate::flip_gate;
use crate::lifecycle;
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
    a3: u32,
    rect_vec: usize,
    a5: i32,
    a6: usize,
    a7: u8,
) -> i64 {
    type PresentOriginal =
        unsafe extern "system" fn(usize, usize, u32, usize, i32, usize, u8) -> i64;

    let original = present_original();
    if original.is_null() {
        return 0;
    }
    let original: PresentOriginal = unsafe { std::mem::transmute(original) };

    if !lifecycle::is_runtime_active() {
        return unsafe { original(this, overlay_swap_chain, a3, rect_vec, a5, a6, a7) };
    }

    let mut present_rect_storage = [DirtyRect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    }];
    let mut present_rect_vec_storage = crate::present::empty_rect_vec_storage();
    let prepared = crate::present::prepare_present(
        this,
        overlay_swap_chain,
        rect_vec,
        &mut present_rect_storage,
        &mut present_rect_vec_storage,
    );
    unsafe { original(this, overlay_swap_chain, a3, prepared.rect_vec, a5, a6, a7) }
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
    use std::ptr;
    use std::sync::atomic::Ordering;

    use crate::present::test_support::initialize_test_state;
    use crate::state::HOOK_GLOBAL_TEST_LOCK as CONTROLLED_TEST_LOCK;
    use dwm_lut_profile::HookTarget;

    fn reset_original_slots() {
        for &target in HookTarget::ALL {
            super::original_pointer_for_target(target).store(ptr::null_mut(), Ordering::Release);
        }
    }

    #[test]
    fn present_detour_forwards_apply_rect_override_to_original() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        use crate::DirtyRect;
        use crate::present::test_support::{
            FakePresentObjects, install_present_original, last_original_present_rects,
            reset_last_original_present_rects,
        };

        reset_last_original_present_rects();
        initialize_test_state();
        let fake = FakePresentObjects::new(
            vec![DirtyRect {
                left: 10,
                top: 20,
                right: 64,
                bottom: 96,
            }],
            false,
        );
        let full_rect = DirtyRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        crate::d3d11::set_fake_render_result(Ok(crate::d3d11::PresentLutOutcome {
            lut_active: true,
            present_dirty_rect: Some(full_rect),
            draw: crate::d3d11::PresentDrawStatus::Applied { full_redraw: true },
            dxgi_format: Some(crate::d3d11::DXGI_FORMAT_B8G8R8A8_UNORM),
            width: None,
            height: None,
            lut_index: Some(0),
            #[cfg(debug_assertions)]
            back_buffer_id: None,
        }));
        install_present_original();

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
        assert_eq!(last_original_present_rects(), Some(vec![full_rect]));
        reset_original_slots();
    }
}
