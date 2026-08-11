use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

use crate::flip_gate;
use crate::present;
use dwm_lut_profile::{AbiArgument, AbiClass, HookEntry, HookTarget};

const UNRESOLVED_ARGUMENT_INDEX: usize = usize::MAX;

static PRESENT_ORIGINAL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());
static IS_CANDIDATE_DIRECT_FLIP_COMPATIBLE_ORIGINAL: AtomicPtr<c_void> =
    AtomicPtr::new(ptr::null_mut());
static IS_CANDIDATE_OVERLAY_COMPATIBLE_ORIGINAL: AtomicPtr<c_void> =
    AtomicPtr::new(ptr::null_mut());
static PRESENT_OVERLAY_SWAP_CHAIN_INDEX: AtomicUsize = AtomicUsize::new(UNRESOLVED_ARGUMENT_INDEX);
static PRESENT_DIRTY_RECTS_INDEX: AtomicUsize = AtomicUsize::new(UNRESOLVED_ARGUMENT_INDEX);

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

fn has_integer_arguments(entry: HookEntry) -> bool {
    entry
        .arguments
        .iter()
        .all(|argument| argument.class() == AbiClass::Integer)
}

fn resolve_present_bindings(entry: HookEntry) -> Option<(usize, usize)> {
    let mut overlay_swap_chain = None;
    let mut dirty_rects = None;

    for (index, argument) in entry.arguments.iter().enumerate() {
        let slot = match argument {
            AbiArgument::OverlaySwapChain => &mut overlay_swap_chain,
            AbiArgument::DirtyRects => &mut dirty_rects,
            AbiArgument::Wildcard(_) => continue,
        };
        if slot.replace(index).is_some() {
            return None;
        }
    }

    Some((overlay_swap_chain?, dirty_rects?))
}

pub(super) fn detour_for_entry(entry: HookEntry) -> Option<*mut c_void> {
    if !has_integer_arguments(entry) {
        return None;
    }

    match (entry.target, entry.arguments.len()) {
        (HookTarget::Present, 5) => Some(present_with_5_arguments_detour as *mut c_void),
        (HookTarget::Present, 6) => Some(present_with_6_arguments_detour as *mut c_void),
        (HookTarget::IsCandidateDirectFlipCompatible, 5) => {
            Some(is_candidate_direct_flip_compatible_with_5_arguments_detour as *mut c_void)
        }
        (HookTarget::IsCandidateDirectFlipCompatible, 7) => {
            Some(is_candidate_direct_flip_compatible_with_7_arguments_detour as *mut c_void)
        }
        (HookTarget::IsCandidateOverlayCompatible, 7) => {
            Some(is_candidate_overlay_compatible_with_7_arguments_detour as *mut c_void)
        }
        (HookTarget::IsCandidateOverlayCompatible, 8) => {
            Some(is_candidate_overlay_compatible_with_8_arguments_detour as *mut c_void)
        }
        _ => None,
    }
}

pub(super) fn configure_present_bindings(entry: HookEntry) -> Option<()> {
    if entry.target != HookTarget::Present {
        return Some(());
    }
    let (overlay_swap_chain, dirty_rects) = resolve_present_bindings(entry)?;
    PRESENT_OVERLAY_SWAP_CHAIN_INDEX.store(overlay_swap_chain, Ordering::Release);
    PRESENT_DIRTY_RECTS_INDEX.store(dirty_rects, Ordering::Release);
    Some(())
}

fn present_bindings(argument_count: usize) -> Option<(usize, usize)> {
    let overlay_swap_chain = PRESENT_OVERLAY_SWAP_CHAIN_INDEX.load(Ordering::Acquire);
    let dirty_rects = PRESENT_DIRTY_RECTS_INDEX.load(Ordering::Acquire);
    if overlay_swap_chain < argument_count && dirty_rects < argument_count {
        Some((overlay_swap_chain, dirty_rects))
    } else {
        None
    }
}

macro_rules! argument_type {
    ($argument:ident) => {
        usize
    };
}

macro_rules! define_present_detour {
    ($name:ident, [$($argument:ident => $index:expr),+ $(,)?] $(,)?) => {
        unsafe extern "system" fn $name(
            this: usize,
            $($argument: usize),+
        ) -> i32 {
            type Original = unsafe extern "system" fn(
                usize,
                $(argument_type!($argument)),+
            ) -> i32;

            let original = present_original();
            if original.is_null() {
                return 0;
            }
            let original: Original = unsafe { std::mem::transmute(original) };
            let mut arguments = [$($argument),+];
            let Some((overlay_swap_chain, dirty_rects)) = present_bindings(arguments.len()) else {
                return unsafe { original(this, $($argument),+) };
            };

            present::present(
                this,
                arguments[overlay_swap_chain],
                arguments[dirty_rects],
                |replacement_dirty_rects| {
                    arguments[dirty_rects] = replacement_dirty_rects;
                    unsafe { original(this, $(arguments[$index]),+) }
                },
            )
        }
    };
}

define_present_detour!(
    present_with_5_arguments_detour,
    [arg0 => 0, arg1 => 1, arg2 => 2, arg3 => 3, arg4 => 4],
);
define_present_detour!(
    present_with_6_arguments_detour,
    [arg0 => 0, arg1 => 1, arg2 => 2, arg3 => 3, arg4 => 4, arg5 => 5],
);

fn apply_flip_gate(
    target: HookTarget,
    overlay_context: usize,
    original_slot: &AtomicPtr<c_void>,
    call_original: impl FnOnce(*mut c_void) -> u8,
) -> u8 {
    if flip_gate::should_block(target, overlay_context) {
        return 0;
    }
    let original = original_slot.load(Ordering::Acquire);
    if original.is_null() {
        return 0;
    }
    call_original(original)
}

macro_rules! define_flip_gate_detour {
    ($name:ident, $target:expr, $original_slot:ident, [$($argument:ident),+ $(,)?] $(,)?) => {
        unsafe extern "system" fn $name(
            this: usize,
            $($argument: usize),+
        ) -> u8 {
            type Original = unsafe extern "system" fn(
                usize,
                $(argument_type!($argument)),+
            ) -> u8;

            apply_flip_gate($target, this, &$original_slot, |original| {
                let original: Original = unsafe { std::mem::transmute(original) };
                unsafe { original(this, $($argument),+) }
            })
        }
    };
}

define_flip_gate_detour!(
    is_candidate_direct_flip_compatible_with_5_arguments_detour,
    HookTarget::IsCandidateDirectFlipCompatible,
    IS_CANDIDATE_DIRECT_FLIP_COMPATIBLE_ORIGINAL,
    [arg0, arg1, arg2, arg3, arg4],
);
define_flip_gate_detour!(
    is_candidate_direct_flip_compatible_with_7_arguments_detour,
    HookTarget::IsCandidateDirectFlipCompatible,
    IS_CANDIDATE_DIRECT_FLIP_COMPATIBLE_ORIGINAL,
    [arg0, arg1, arg2, arg3, arg4, arg5, arg6],
);
define_flip_gate_detour!(
    is_candidate_overlay_compatible_with_7_arguments_detour,
    HookTarget::IsCandidateOverlayCompatible,
    IS_CANDIDATE_OVERLAY_COMPATIBLE_ORIGINAL,
    [arg0, arg1, arg2, arg3, arg4, arg5, arg6],
);
define_flip_gate_detour!(
    is_candidate_overlay_compatible_with_8_arguments_detour,
    HookTarget::IsCandidateOverlayCompatible,
    IS_CANDIDATE_OVERLAY_COMPATIBLE_ORIGINAL,
    [arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7],
);

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
        super::PRESENT_OVERLAY_SWAP_CHAIN_INDEX
            .store(super::UNRESOLVED_ARGUMENT_INDEX, Ordering::Release);
        super::PRESENT_DIRTY_RECTS_INDEX.store(super::UNRESOLVED_ARGUMENT_INDEX, Ordering::Release);
    }

    unsafe extern "system" fn returns_negative_hresult(
        _this: usize,
        _arg0: usize,
        _arg1: usize,
        _arg2: usize,
        _arg3: usize,
        _arg4: usize,
        _arg5: usize,
    ) -> i32 {
        -1
    }

    #[test]
    fn every_compiled_profile_entry_has_a_detour() {
        for supported in dwm_lut_profile::SUPPORTED_BUILDS {
            for revision in supported.profiles {
                let profile = (revision.profile)();
                for &entry in profile.hooks {
                    assert!(
                        super::detour_for_entry(entry).is_some(),
                        "{}.{} {}/{} has no detour",
                        supported.build,
                        revision.min_revision,
                        entry.target.label(),
                        entry.arguments.len(),
                    );
                    if entry.target == HookTarget::Present {
                        assert!(
                            super::resolve_present_bindings(entry).is_some(),
                            "{}.{} Present is missing OverlaySwapChain/DirtyRects bindings",
                            supported.build,
                            revision.min_revision,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn present_detour_preserves_negative_hresult() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        let entry = dwm_lut_profile::SUPPORTED_BUILDS
            .iter()
            .flat_map(|supported| supported.profiles)
            .flat_map(|revision| (revision.profile)().hooks)
            .copied()
            .find(|entry| entry.target == HookTarget::Present && entry.arguments.len() == 6)
            .expect("compiled profiles should include six-argument Present");
        assert!(
            super::detour_for_entry(entry).is_some(),
            "entry should select a detour"
        );
        super::configure_present_bindings(entry).expect("entry should configure Present bindings");
        super::original_pointer_for_target(HookTarget::Present)
            .store(returns_negative_hresult as *mut c_void, Ordering::Release);

        assert_eq!(
            unsafe { super::present_with_6_arguments_detour(0, 0, 0, 0, 0, 0, 0) },
            -1
        );
        reset_original_slots();
    }
}
