#[cfg(not(test))]
use std::ffi::c_void;
#[cfg(not(test))]
use std::mem::size_of;

#[cfg(not(test))]
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};

use crate::lut_pipeline::BackBufferFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayTestModeControl {
    Unmodified,
    ForceMode5,
}

impl OverlayTestModeControl {
    pub const fn is_unmodified(self) -> bool {
        matches!(self, Self::Unmodified)
    }

    pub const fn is_force_mode_5(self) -> bool {
        matches!(self, Self::ForceMode5)
    }

    pub const fn apply(self, original_mode: i32) -> i32 {
        match self {
            Self::Unmodified => original_mode,
            Self::ForceMode5 => 5,
        }
    }

    pub const fn from_active_contexts(has_active_contexts: bool) -> Self {
        if has_active_contexts {
            Self::ForceMode5
        } else {
            Self::Unmodified
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextLutState {
    pub back_buffer_format: BackBufferFormat,
    pub lut_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayTestModePatch {
    pub address: usize,
    pub original_mode: Option<i32>,
    pub applied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisableIndependentFlipPatch {
    pub address: usize,
    pub original_value: Option<i32>,
    pub applied: bool,
    pub rejected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlipGateEffects {
    pub overlay_test_mode_control: OverlayTestModeControl,
    pub overlay_test_mode_patch: Option<OverlayTestModePatch>,
    pub disable_independent_flip_patch: Option<DisableIndependentFlipPatch>,
    pub overlays_enabled_override: Option<bool>,
}

impl FlipGateEffects {
    pub fn new(
        overlay_test_mode_address: Option<usize>,
        disable_independent_flip_address: Option<usize>,
    ) -> Self {
        Self {
            overlay_test_mode_control: OverlayTestModeControl::Unmodified,
            overlay_test_mode_patch: overlay_test_mode_address
                .filter(|address| *address != 0)
                .map(|address| OverlayTestModePatch {
                    address,
                    original_mode: None,
                    applied: false,
                }),
            disable_independent_flip_patch: disable_independent_flip_address
                .filter(|address| *address != 0)
                .map(|address| DisableIndependentFlipPatch {
                    address,
                    original_value: None,
                    applied: false,
                    rejected: false,
                }),
            overlays_enabled_override: None,
        }
    }

    pub fn sync_active(&mut self, active: bool) {
        self.set_overlay_test_mode_control(OverlayTestModeControl::from_active_contexts(active));
        self.set_disable_independent_flip_enabled(active);
        self.set_overlays_enabled_override(active);
    }

    pub fn restore(&mut self) {
        self.set_overlay_test_mode_control(OverlayTestModeControl::Unmodified);
        self.set_disable_independent_flip_enabled(false);
        self.set_overlays_enabled_override(false);
    }

    fn set_overlay_test_mode_control(&mut self, control: OverlayTestModeControl) {
        self.overlay_test_mode_control = control;
        let Some(patch) = &mut self.overlay_test_mode_patch else {
            return;
        };

        match control {
            OverlayTestModeControl::ForceMode5 if !patch.applied => {
                let original_mode = unsafe { read_i32(patch.address) };
                patch.original_mode = Some(original_mode);
                unsafe { write_i32(patch.address, 5) };
                patch.applied = true;
            }
            OverlayTestModeControl::Unmodified if patch.applied => {
                unsafe { write_i32(patch.address, patch.original_mode.unwrap_or(0)) };
                patch.applied = false;
            }
            _ => {}
        }
    }

    fn set_disable_independent_flip_enabled(&mut self, enabled: bool) {
        let Some(patch) = &mut self.disable_independent_flip_patch else {
            return;
        };
        if patch.rejected {
            return;
        }

        if enabled && !patch.applied {
            if !is_writable_i32(patch.address) {
                patch.rejected = true;
                debug_log!(
                    "event=disable_independent_flip_rejected reason={}",
                    crate::debug_log::quoted("page_not_writable")
                );
                return;
            }
            let original_value = unsafe { read_i32(patch.address) };
            if original_value != 0 && original_value != 1 {
                patch.rejected = true;
                debug_log!(
                    "event=disable_independent_flip_rejected reason={} value={}",
                    crate::debug_log::quoted("unexpected_value"),
                    original_value
                );
                return;
            }
            patch.original_value = Some(original_value);
            unsafe { write_i32(patch.address, 1) };
            patch.applied = true;
            debug_log!("event=disable_independent_flip_applied value=1");
        } else if !enabled && patch.applied {
            unsafe { write_i32(patch.address, patch.original_value.unwrap_or(0)) };
            patch.applied = false;
            debug_log!("event=disable_independent_flip_restored");
        }
    }

    fn set_overlays_enabled_override(&mut self, enabled: bool) {
        let value = enabled.then(|| {
            self.disable_independent_flip_patch
                .as_ref()
                .is_some_and(|dif| dif.applied)
        });
        if self.overlays_enabled_override == value {
            return;
        }

        self.overlays_enabled_override = value;
        #[cfg(not(test))]
        crate::minhook::set_overlays_enabled_override(value);
        debug_log!("event=overlays_enabled_override value={value:?}");
    }
}

impl Default for FlipGateEffects {
    fn default() -> Self {
        Self::new(None, None)
    }
}

pub fn direct_flip_compatible(has_present_context: bool, original_compatible: bool) -> bool {
    !has_present_context && original_compatible
}

pub fn ensure_independent_flip_state(has_lut_assignments: bool) -> Option<i32> {
    if has_lut_assignments { Some(0) } else { None }
}

pub fn direct_flip_support_compatible(
    has_lut_assignments: bool,
    original_compatible: bool,
) -> bool {
    if has_lut_assignments {
        false
    } else {
        original_compatible
    }
}

pub fn overlay_test_mode(has_active_contexts: bool, original_mode: i32) -> i32 {
    OverlayTestModeControl::from_active_contexts(has_active_contexts).apply(original_mode)
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
    use std::collections::BTreeMap;

    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, MonitorIdentity, MonitorTarget, PayloadAssignment,
        PayloadLut,
    };

    use super::{
        ContextLutState, FlipGateEffects, OverlayTestModeControl, direct_flip_compatible,
        direct_flip_support_compatible, ensure_independent_flip_state, overlay_test_mode,
    };
    use crate::lut_pipeline::{BackBufferFormat, LutDecision, LutPipeline};

    fn test_identity() -> MonitorIdentity {
        MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4357,
        }
    }

    fn pipeline_for_single_sdr_monitor() -> LutPipeline {
        LutPipeline::from_payload(&HookPayload {
            assignments: vec![PayloadAssignment {
                target: MonitorTarget {
                    identity: test_identity(),
                    color_mode: ColorMode::Sdr,
                },
                lut: PayloadLut {
                    size: 2,
                    domain_min: [0.0, 0.0, 0.0],
                    domain_max: [1.0, 1.0, 1.0],
                    values: vec![[0.0, 0.0, 0.0]; 8],
                },
            }],
        })
    }

    fn apply_decision(
        contexts: &mut BTreeMap<usize, ContextLutState>,
        effects: &mut FlipGateEffects,
        pipeline: &LutPipeline,
        context: usize,
    ) {
        let decision = pipeline.decide(test_identity(), BackBufferFormat::Bgra8Unorm);
        match decision {
            LutDecision::Apply { format, lut_index } => {
                contexts.insert(
                    context,
                    ContextLutState {
                        back_buffer_format: format,
                        lut_index,
                    },
                );
            }
            LutDecision::NotApplicable => {
                contexts.remove(&context);
            }
        }
        effects.sync_active(!contexts.is_empty());
    }

    fn deactivate(
        contexts: &mut BTreeMap<usize, ContextLutState>,
        effects: &mut FlipGateEffects,
        context: usize,
    ) {
        contexts.remove(&context);
        effects.sync_active(!contexts.is_empty());
    }

    #[test]
    fn present_activation_blocks_promotion_for_same_context_only() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut contexts = BTreeMap::new();
        let mut effects = FlipGateEffects::new(None, None);
        let has_luts = !pipeline.luts.is_empty();

        apply_decision(&mut contexts, &mut effects, &pipeline, 0x1234);

        assert_eq!(
            effects.overlay_test_mode_control,
            OverlayTestModeControl::ForceMode5
        );
        assert!(!direct_flip_compatible(
            contexts.contains_key(&0x1234),
            true
        ));
        assert!(!direct_flip_support_compatible(has_luts, true));
        assert_eq!(ensure_independent_flip_state(has_luts), Some(0));
        assert_eq!(overlay_test_mode(!contexts.is_empty(), 0), 5);
        assert!(direct_flip_compatible(contexts.contains_key(&0x4321), true));
        assert_eq!(
            effects.overlay_test_mode_control,
            OverlayTestModeControl::ForceMode5
        );

        let context = contexts.get(&0x1234).expect("context should exist");
        assert_eq!(context.lut_index, 0);
        assert_eq!(context.back_buffer_format, BackBufferFormat::Bgra8Unorm);
    }

    #[test]
    fn present_deactivation_clears_promotion_block_for_that_context() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut contexts = BTreeMap::new();
        let mut effects = FlipGateEffects::new(None, None);
        let has_luts = !pipeline.luts.is_empty();

        apply_decision(&mut contexts, &mut effects, &pipeline, 0x1234);
        deactivate(&mut contexts, &mut effects, 0x1234);

        assert!(direct_flip_compatible(contexts.contains_key(&0x1234), true));
        assert!(!direct_flip_support_compatible(has_luts, true));
        assert_eq!(ensure_independent_flip_state(has_luts), Some(0));
        assert_eq!(overlay_test_mode(!contexts.is_empty(), 0), 0);
        assert!(!contexts.contains_key(&0x1234));
    }

    #[test]
    fn overlay_test_mode_global_is_patched_only_while_context_is_active() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut overlay_mode = 0i32;
        let mut contexts = BTreeMap::new();
        let mut effects =
            FlipGateEffects::new(Some((&mut overlay_mode as *mut i32) as usize), None);

        apply_decision(&mut contexts, &mut effects, &pipeline, 0x1234);
        assert_eq!(overlay_mode, 5);

        deactivate(&mut contexts, &mut effects, 0x1234);
        assert_eq!(overlay_mode, 0);
    }

    #[test]
    fn ensure_independent_flip_state_blocks_only_with_lut_assignments() {
        assert_eq!(ensure_independent_flip_state(false), None);
        assert_eq!(ensure_independent_flip_state(true), Some(0));
    }

    #[test]
    fn disable_independent_flip_is_patched_only_while_context_is_active() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut disable_independent_flip = 0i32;
        let mut contexts = BTreeMap::new();
        let mut effects = FlipGateEffects::new(
            None,
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        assert_eq!(disable_independent_flip, 0);

        apply_decision(&mut contexts, &mut effects, &pipeline, 0x1234);
        assert_eq!(disable_independent_flip, 1);

        deactivate(&mut contexts, &mut effects, 0x1234);
        assert!(contexts.is_empty());
        assert_eq!(disable_independent_flip, 0);
    }

    #[test]
    fn disable_independent_flip_rejects_unexpected_value() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut disable_independent_flip = 7i32;
        let mut contexts = BTreeMap::new();
        let mut effects = FlipGateEffects::new(
            None,
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        apply_decision(&mut contexts, &mut effects, &pipeline, 0x1234);

        assert_eq!(disable_independent_flip, 7);
        assert!(
            effects
                .disable_independent_flip_patch
                .as_ref()
                .is_some_and(|patch| patch.rejected && !patch.applied)
        );
    }

    #[test]
    fn overlays_enabled_override_returns_true_when_dif_is_applied() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut disable_independent_flip = 0i32;
        let mut contexts = BTreeMap::new();
        let mut effects = FlipGateEffects::new(
            None,
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        apply_decision(&mut contexts, &mut effects, &pipeline, 0x1234);

        assert_eq!(disable_independent_flip, 1);
        assert_eq!(effects.overlays_enabled_override, Some(true));
    }

    #[test]
    fn overlays_enabled_override_returns_false_when_dif_is_unavailable() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut contexts = BTreeMap::new();
        let mut effects = FlipGateEffects::new(None, None);

        apply_decision(&mut contexts, &mut effects, &pipeline, 0x1234);

        assert_eq!(effects.overlays_enabled_override, Some(false));
    }

    #[test]
    fn restore_clears_active_effects() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut overlay_mode = 0i32;
        let mut disable_independent_flip = 0i32;
        let mut contexts = BTreeMap::new();
        let mut effects = FlipGateEffects::new(
            Some((&mut overlay_mode as *mut i32) as usize),
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        apply_decision(&mut contexts, &mut effects, &pipeline, 0x1234);

        assert!(!contexts.is_empty());
        assert_eq!(overlay_mode, 5);
        assert_eq!(disable_independent_flip, 1);
        assert_eq!(effects.overlays_enabled_override, Some(true));

        contexts.clear();
        effects.restore();

        assert!(contexts.is_empty());
        assert_eq!(overlay_mode, 0);
        assert_eq!(disable_independent_flip, 0);
        assert_eq!(effects.overlays_enabled_override, None);
        assert_eq!(overlay_test_mode(false, 0), 0);
    }
}
