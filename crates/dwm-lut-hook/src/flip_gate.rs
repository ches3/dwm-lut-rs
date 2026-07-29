use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

#[cfg(not(test))]
use std::mem::size_of;

#[cfg(not(test))]
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};

use crate::lifecycle;
#[cfg(debug_assertions)]
use crate::log::SharedLimiter;

const OVERLAY_TEST_MODE_FORCE: i32 = 5;

#[cfg(debug_assertions)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FlipGateKind {
    OverlayContextDirectFlip,
    DirectFlipInfoEnsureIndependentFlip,
    IsDirectFlipSupportedOnTarget,
    LegacySwapChainCheckDirectFlip,
    IsAdvancedDirectFlipCompatible,
}

#[cfg(debug_assertions)]
impl FlipGateKind {
    const fn label(self) -> &'static str {
        match self {
            Self::OverlayContextDirectFlip => "overlay_context_direct_flip",
            Self::DirectFlipInfoEnsureIndependentFlip => "direct_flip_info_ensure_independent_flip",
            Self::IsDirectFlipSupportedOnTarget => "is_direct_flip_supported_on_target",
            Self::LegacySwapChainCheckDirectFlip => "legacy_swap_chain_check_direct_flip",
            Self::IsAdvancedDirectFlipCompatible => "is_advanced_direct_flip_compatible",
        }
    }
}

#[cfg(debug_assertions)]
static FLIP_GATE_DENIED_LIMITER: SharedLimiter<FlipGateKind> = SharedLimiter::new(600);

#[cfg(debug_assertions)]
fn record_flip_gate_denied(kind: FlipGateKind) {
    let decision = FLIP_GATE_DENIED_LIMITER.sample(kind);
    if decision.should_log {
        crate::log::flip_gate_denied(kind.label(), decision.count);
    }
}

pub(crate) fn apply_flip_gate<T: Default>(
    original_slot: &AtomicPtr<c_void>,
    #[cfg(debug_assertions)] kind: FlipGateKind,
    call_original: impl FnOnce(*mut c_void) -> T,
) -> T {
    if lifecycle::is_runtime_active() {
        #[cfg(debug_assertions)]
        record_flip_gate_denied(kind);
        return T::default();
    }
    let original = original_slot.load(Ordering::Acquire);
    if original.is_null() {
        return T::default();
    }
    call_original(original)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GlobalI32Patch {
    address: usize,
    original: Option<i32>,
    applied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DisableIndependentFlipPatch {
    address: usize,
    original: Option<i32>,
    applied: bool,
    rejected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlipGateEffects {
    overlay_test_mode: Option<GlobalI32Patch>,
    disable_independent_flip: Option<DisableIndependentFlipPatch>,
    overlays_enabled_override: Option<bool>,
}

impl FlipGateEffects {
    pub fn new(
        overlay_test_mode_address: Option<usize>,
        disable_independent_flip_address: Option<usize>,
    ) -> Self {
        Self {
            overlay_test_mode: overlay_test_mode_address
                .filter(|address| *address != 0)
                .map(|address| GlobalI32Patch {
                    address,
                    original: None,
                    applied: false,
                }),
            disable_independent_flip: disable_independent_flip_address
                .filter(|address| *address != 0)
                .map(|address| DisableIndependentFlipPatch {
                    address,
                    original: None,
                    applied: false,
                    rejected: false,
                }),
            overlays_enabled_override: None,
        }
    }

    pub fn apply(&mut self) {
        self.apply_overlay_test_mode();
        self.apply_disable_independent_flip();
        self.set_overlays_enabled_override(true);
    }

    pub fn restore(&mut self) {
        self.restore_overlay_test_mode();
        self.restore_disable_independent_flip();
        self.set_overlays_enabled_override(false);
    }

    fn apply_overlay_test_mode(&mut self) {
        let Some(patch) = &mut self.overlay_test_mode else {
            return;
        };
        if patch.applied {
            return;
        }
        let original = unsafe { read_i32(patch.address) };
        patch.original = Some(original);
        unsafe { write_i32(patch.address, OVERLAY_TEST_MODE_FORCE) };
        patch.applied = true;
    }

    fn restore_overlay_test_mode(&mut self) {
        let Some(patch) = &mut self.overlay_test_mode else {
            return;
        };
        if !patch.applied {
            return;
        }
        unsafe { write_i32(patch.address, patch.original.unwrap_or(0)) };
        patch.applied = false;
    }

    fn apply_disable_independent_flip(&mut self) {
        let Some(patch) = &mut self.disable_independent_flip else {
            return;
        };
        if patch.rejected || patch.applied {
            return;
        }

        if !is_writable_i32(patch.address) {
            patch.rejected = true;
            crate::log::independent_flip(crate::log::IndependentFlipOutcome::Rejected(
                crate::log::IndependentFlipRejectReason::PageNotWritable,
            ));
            return;
        }

        let original = unsafe { read_i32(patch.address) };
        if original != 0 && original != 1 {
            patch.rejected = true;
            crate::log::independent_flip(crate::log::IndependentFlipOutcome::Rejected(
                crate::log::IndependentFlipRejectReason::UnexpectedValue(original),
            ));
            return;
        }

        patch.original = Some(original);
        unsafe { write_i32(patch.address, 1) };
        patch.applied = true;
        crate::log::independent_flip(crate::log::IndependentFlipOutcome::Applied);
    }

    fn restore_disable_independent_flip(&mut self) {
        let Some(patch) = &mut self.disable_independent_flip else {
            return;
        };
        if !patch.applied {
            return;
        }
        unsafe { write_i32(patch.address, patch.original.unwrap_or(0)) };
        patch.applied = false;
        crate::log::independent_flip(crate::log::IndependentFlipOutcome::Restored);
    }

    fn set_overlays_enabled_override(&mut self, enabled: bool) {
        let value = enabled.then(|| {
            self.disable_independent_flip
                .as_ref()
                .is_some_and(|dif| dif.applied)
        });
        if self.overlays_enabled_override == value {
            return;
        }

        self.overlays_enabled_override = value;
        #[cfg(not(test))]
        crate::minhook::set_overlays_enabled_override(value);
        crate::log::overlays_enabled_override(value);
    }
}

impl Default for FlipGateEffects {
    fn default() -> Self {
        Self::new(None, None)
    }
}

unsafe fn read_i32(address: usize) -> i32 {
    unsafe { (address as *const i32).read_volatile() }
}

unsafe fn write_i32(address: usize, value: i32) {
    unsafe { (address as *mut i32).write_volatile(value) };
}

fn is_writable_i32(address: usize) -> bool {
    #[cfg(test)]
    {
        let _ = address;
        true
    }
    #[cfg(not(test))]
    {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let written = unsafe {
            VirtualQuery(
                Some(address as *const c_void),
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if written == 0 || info.State != MEM_COMMIT || (info.Protect.0 & PAGE_GUARD.0) != 0 {
            return false;
        }
        matches!(
            info.Protect.0
                & (PAGE_READWRITE.0
                    | PAGE_WRITECOPY.0
                    | PAGE_EXECUTE_READWRITE.0
                    | PAGE_EXECUTE_WRITECOPY.0),
            value if value != 0
        )
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

    #[cfg(debug_assertions)]
    use super::FlipGateKind;
    use super::{FlipGateEffects, apply_flip_gate};
    use crate::lifecycle;
    use crate::state::{self, HOOK_GLOBAL_TEST_LOCK};

    unsafe extern "system" fn unused_original(_this: usize) -> u8 {
        1
    }

    #[test]
    fn apply_flip_gate_returns_default_when_runtime_is_active() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        lifecycle::begin_initialization()
            .expect("initialization transition should start")
            .commit_active("test");

        let slot = AtomicPtr::new(unused_original as *mut c_void);
        let called = AtomicBool::new(false);
        let result = apply_flip_gate(
            &slot,
            #[cfg(debug_assertions)]
            FlipGateKind::IsAdvancedDirectFlipCompatible,
            |_| {
                called.store(true, Ordering::Relaxed);
                7u8
            },
        );

        assert_eq!(result, 0);
        assert!(!called.load(Ordering::Relaxed));
        state::reset_state_for_tests();
    }

    #[test]
    fn apply_flip_gate_calls_original_when_inactive() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();

        let expected = unused_original as *mut c_void;
        let slot = AtomicPtr::new(expected);
        let seen = AtomicPtr::new(std::ptr::null_mut());
        let result = apply_flip_gate(
            &slot,
            #[cfg(debug_assertions)]
            FlipGateKind::IsAdvancedDirectFlipCompatible,
            |original| {
                seen.store(original, Ordering::Relaxed);
                7u8
            },
        );

        assert_eq!(result, 7);
        assert_eq!(seen.load(Ordering::Relaxed), expected);
    }

    #[test]
    fn apply_flip_gate_returns_default_when_original_is_null() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();

        let slot = AtomicPtr::new(std::ptr::null_mut());
        let called = AtomicBool::new(false);
        let result = apply_flip_gate(
            &slot,
            #[cfg(debug_assertions)]
            FlipGateKind::IsAdvancedDirectFlipCompatible,
            |_| {
                called.store(true, Ordering::Relaxed);
                7u8
            },
        );

        assert_eq!(result, 0);
        assert!(!called.load(Ordering::Relaxed));
    }

    #[test]
    fn overlay_test_mode_is_patched_while_applied() {
        let mut overlay_mode = 0i32;
        let mut effects =
            FlipGateEffects::new(Some((&mut overlay_mode as *mut i32) as usize), None);

        effects.apply();
        assert_eq!(overlay_mode, 5);

        effects.restore();
        assert_eq!(overlay_mode, 0);
    }

    #[test]
    fn disable_independent_flip_is_patched_while_applied() {
        let mut disable_independent_flip = 0i32;
        let mut effects = FlipGateEffects::new(
            None,
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        effects.apply();
        assert_eq!(disable_independent_flip, 1);

        effects.restore();
        assert_eq!(disable_independent_flip, 0);
    }

    #[test]
    fn disable_independent_flip_rejects_unexpected_value() {
        let mut disable_independent_flip = 7i32;
        let mut effects = FlipGateEffects::new(
            None,
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        effects.apply();

        assert_eq!(disable_independent_flip, 7);
        assert!(
            effects
                .disable_independent_flip
                .as_ref()
                .is_some_and(|patch| patch.rejected && !patch.applied)
        );
        assert_eq!(effects.overlays_enabled_override, Some(false));
    }

    #[test]
    fn overlays_enabled_override_is_true_when_dif_is_applied() {
        let mut disable_independent_flip = 0i32;
        let mut effects = FlipGateEffects::new(
            None,
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        effects.apply();

        assert_eq!(effects.overlays_enabled_override, Some(true));
    }

    #[test]
    fn overlays_enabled_override_is_false_when_dif_is_unavailable() {
        let mut effects = FlipGateEffects::new(None, None);

        effects.apply();

        assert_eq!(effects.overlays_enabled_override, Some(false));
    }

    #[test]
    fn restore_clears_all_effects() {
        let mut overlay_mode = 0i32;
        let mut disable_independent_flip = 0i32;
        let mut effects = FlipGateEffects::new(
            Some((&mut overlay_mode as *mut i32) as usize),
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        effects.apply();
        assert_eq!(overlay_mode, 5);
        assert_eq!(disable_independent_flip, 1);
        assert_eq!(effects.overlays_enabled_override, Some(true));

        effects.restore();
        assert_eq!(overlay_mode, 0);
        assert_eq!(disable_independent_flip, 0);
        assert_eq!(effects.overlays_enabled_override, None);
    }

    #[test]
    fn apply_is_idempotent() {
        let mut overlay_mode = 0i32;
        let mut effects =
            FlipGateEffects::new(Some((&mut overlay_mode as *mut i32) as usize), None);

        effects.apply();
        overlay_mode = 9;
        effects.apply();
        assert_eq!(overlay_mode, 9);
    }
}
