use std::collections::BTreeMap;
#[cfg(not(test))]
use std::ffi::c_void;
#[cfg(not(test))]
use std::mem::size_of;

#[cfg(not(test))]
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_GUARD, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery,
};

use crate::lut_pipeline::{BackBufferFormat, DirtyRect, LutPipeline, LutRenderPlan};
use dwm_lut_payload::MonitorIdentity;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextLutState {
    pub back_buffer_format: Option<BackBufferFormat>,
    pub lut_index: Option<usize>,
    pub dirty_rect_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PresentHookOutcome {
    pub plan: Option<LutRenderPlan>,
    pub promotion_blocked: bool,
    pub overlay_test_mode_control: OverlayTestModeControl,
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
pub struct LutBypassRuntime {
    pub overlay_test_mode_control: OverlayTestModeControl,
    pub overlay_test_mode_patch: Option<OverlayTestModePatch>,
    pub disable_independent_flip_patch: Option<DisableIndependentFlipPatch>,
    pub overlays_enabled_override: Option<bool>,
    pub has_lut_assignments: bool,
    pub contexts: BTreeMap<usize, ContextLutState>,
}

impl LutBypassRuntime {
    pub fn new(
        has_lut_assignments: bool,
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
            has_lut_assignments,
            contexts: BTreeMap::new(),
        }
    }

    pub fn update_present(
        &mut self,
        lut_pipeline: &LutPipeline,
        context_address: usize,
        monitor_identity: Option<MonitorIdentity>,
        dxgi_format: u32,
        dirty_rects: &[DirtyRect],
    ) -> PresentHookOutcome {
        let plan = monitor_identity.and_then(|identity| {
            lut_pipeline.build_present_plan_for_monitor_identity(identity, dxgi_format, dirty_rects)
        });
        self.update_context(context_address, dxgi_format, dirty_rects, plan)
    }

    pub fn update_present_with_lut_index(
        &mut self,
        lut_pipeline: &LutPipeline,
        context_address: usize,
        dxgi_format: u32,
        dirty_rects: &[DirtyRect],
        lut_index: Option<usize>,
    ) -> PresentHookOutcome {
        let plan = lut_index.and_then(|lut_index| {
            lut_pipeline.build_present_plan_for_lut_index(dxgi_format, dirty_rects, lut_index)
        });
        self.update_context(context_address, dxgi_format, dirty_rects, plan)
    }

    fn update_context(
        &mut self,
        context_address: usize,
        dxgi_format: u32,
        dirty_rects: &[DirtyRect],
        plan: Option<LutRenderPlan>,
    ) -> PresentHookOutcome {
        let back_buffer_format = BackBufferFormat::from_dxgi_format(dxgi_format);
        let promotion_blocked = plan.is_some();
        let lut_index = plan.as_ref().map(|plan| plan.lut_index);

        if promotion_blocked {
            self.contexts.insert(
                context_address,
                ContextLutState {
                    back_buffer_format,
                    lut_index,
                    dirty_rect_count: dirty_rects.len(),
                },
            );
        } else {
            self.contexts.remove(&context_address);
        }

        let active = self.has_active_contexts();
        self.set_overlay_test_mode_control(self.overlay_test_mode_control());
        self.set_disable_independent_flip_enabled(active);
        self.set_overlays_enabled_override(active);

        PresentHookOutcome {
            plan,
            promotion_blocked,
            overlay_test_mode_control: self.overlay_test_mode_control,
        }
    }

    pub fn direct_flip_compatible(
        &mut self,
        context_address: usize,
        original_compatible: bool,
    ) -> bool {
        if self.contexts.contains_key(&context_address) {
            false
        } else {
            original_compatible
        }
    }

    pub fn ensure_independent_flip_state(&self) -> Option<i32> {
        if self.has_lut_assignments {
            Some(0)
        } else {
            None
        }
    }

    pub fn direct_flip_support_compatible(&self, original_compatible: bool) -> bool {
        if self.has_lut_assignments {
            false
        } else {
            original_compatible
        }
    }

    pub fn overlay_test_mode(&self, original_mode: i32) -> i32 {
        self.overlay_test_mode_control().apply(original_mode)
    }

    pub fn restore_overlay_test_mode(&mut self) {
        self.set_overlay_test_mode_control(OverlayTestModeControl::Unmodified);
        self.set_disable_independent_flip_enabled(false);
        self.set_overlays_enabled_override(false);
    }

    pub fn reload_for_new_payload(&mut self, has_lut_assignments: bool) {
        self.contexts.clear();
        self.has_lut_assignments = has_lut_assignments;
        self.restore_overlay_test_mode();
    }

    pub fn context(&self, context_address: usize) -> Option<&ContextLutState> {
        self.contexts.get(&context_address)
    }

    pub fn has_active_contexts(&self) -> bool {
        !self.contexts.is_empty()
    }

    fn overlay_test_mode_control(&self) -> OverlayTestModeControl {
        if self.has_active_contexts() {
            OverlayTestModeControl::ForceMode5
        } else {
            OverlayTestModeControl::Unmodified
        }
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

impl Default for LutBypassRuntime {
    fn default() -> Self {
        Self::new(false, None, None)
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
    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, MonitorIdentity, MonitorTarget, PayloadAssignment,
        PayloadLut,
    };

    use super::{LutBypassRuntime, OverlayTestModeControl};
    use crate::lut_pipeline::{DXGI_FORMAT_B8G8R8A8_UNORM, DirtyRect, LutPipeline};

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

    #[test]
    fn present_activation_blocks_promotion_for_same_context_only() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut runtime = LutBypassRuntime::new(true, None, None);

        let outcome = runtime.update_present(
            &pipeline,
            0x1234,
            Some(test_identity()),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
        );

        assert!(outcome.plan.is_some());
        assert!(outcome.promotion_blocked);
        assert_eq!(
            outcome.overlay_test_mode_control,
            OverlayTestModeControl::ForceMode5
        );
        assert!(!runtime.direct_flip_compatible(0x1234, true));
        assert!(!runtime.direct_flip_support_compatible(true));
        assert_eq!(runtime.ensure_independent_flip_state(), Some(0));
        assert_eq!(runtime.overlay_test_mode(0), 5);
        assert!(runtime.direct_flip_compatible(0x4321, true));
        assert_eq!(
            runtime.overlay_test_mode_control,
            OverlayTestModeControl::ForceMode5
        );

        let context = runtime.context(0x1234).expect("context should exist");
        assert_eq!(context.lut_index, Some(0));
        assert_eq!(context.dirty_rect_count, 1);
    }

    #[test]
    fn present_deactivation_clears_promotion_block_for_that_context() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut runtime = LutBypassRuntime::new(true, None, None);

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            Some(test_identity()),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[],
        );

        let outcome =
            runtime.update_present(&pipeline, 0x1234, None, DXGI_FORMAT_B8G8R8A8_UNORM, &[]);

        assert!(outcome.plan.is_none());
        assert!(!outcome.promotion_blocked);
        assert!(runtime.direct_flip_compatible(0x1234, true));
        assert!(!runtime.direct_flip_support_compatible(true));
        assert_eq!(runtime.ensure_independent_flip_state(), Some(0));
        assert_eq!(runtime.overlay_test_mode(0), 0);
        assert!(runtime.context(0x1234).is_none());
    }

    #[test]
    fn plan_keeps_bypass_state_even_when_render_misses_a_frame() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut runtime = LutBypassRuntime::new(true, None, None);

        let outcome = runtime.update_present(
            &pipeline,
            0x1234,
            Some(test_identity()),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
        );

        assert!(outcome.plan.is_some());
        assert!(outcome.promotion_blocked);
        assert!(runtime.context(0x1234).is_some());
        assert!(!runtime.direct_flip_compatible(0x1234, true));
        assert!(!runtime.direct_flip_support_compatible(true));
        assert_eq!(runtime.ensure_independent_flip_state(), Some(0));
        assert_eq!(runtime.overlay_test_mode(0), 5);
    }

    #[test]
    fn active_context_persists_until_explicit_deactivation() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut runtime = LutBypassRuntime::new(true, None, None);

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            Some(test_identity()),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
        );

        assert!(runtime.context(0x1234).is_some());
        assert!(!runtime.direct_flip_support_compatible(true));
        assert_eq!(runtime.overlay_test_mode(0), 5);
    }

    #[test]
    fn overlay_test_mode_global_is_patched_only_while_context_is_active() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut overlay_test_mode = 0i32;
        let mut runtime = LutBypassRuntime::new(
            true,
            Some((&mut overlay_test_mode as *mut i32) as usize),
            None,
        );

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            Some(test_identity()),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[],
        );

        assert_eq!(overlay_test_mode, 5);

        let _ = runtime.update_present(&pipeline, 0x1234, None, DXGI_FORMAT_B8G8R8A8_UNORM, &[]);

        assert_eq!(overlay_test_mode, 0);
    }

    #[test]
    fn ensure_independent_flip_state_blocks_only_with_lut_assignments() {
        let mut without_assignments = LutBypassRuntime::new(false, None, None);
        assert_eq!(without_assignments.ensure_independent_flip_state(), None);

        let with_assignments = LutBypassRuntime::new(true, None, None);
        assert_eq!(with_assignments.ensure_independent_flip_state(), Some(0));

        without_assignments.reload_for_new_payload(true);
        assert_eq!(without_assignments.ensure_independent_flip_state(), Some(0));
    }

    #[test]
    fn disable_independent_flip_is_patched_only_while_context_is_active() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut disable_independent_flip = 0i32;
        let mut runtime = LutBypassRuntime::new(
            true,
            None,
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        assert_eq!(disable_independent_flip, 0);

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            Some(test_identity()),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[],
        );
        assert_eq!(disable_independent_flip, 1);

        let _ = runtime.update_present(&pipeline, 0x1234, None, DXGI_FORMAT_B8G8R8A8_UNORM, &[]);
        assert!(!runtime.has_active_contexts());
        assert_eq!(disable_independent_flip, 0);
    }

    #[test]
    fn disable_independent_flip_rejects_unexpected_value() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut disable_independent_flip = 7i32;
        let mut runtime = LutBypassRuntime::new(
            true,
            None,
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            Some(test_identity()),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[],
        );

        assert_eq!(disable_independent_flip, 7);
        assert!(
            runtime
                .disable_independent_flip_patch
                .as_ref()
                .is_some_and(|patch| patch.rejected && !patch.applied)
        );
    }

    #[test]
    fn overlays_enabled_override_returns_true_when_dif_is_applied() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut disable_independent_flip = 0i32;
        let mut runtime = LutBypassRuntime::new(
            true,
            None,
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            Some(test_identity()),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[],
        );

        assert_eq!(disable_independent_flip, 1);
        assert_eq!(runtime.overlays_enabled_override, Some(true));
    }

    #[test]
    fn overlays_enabled_override_returns_false_when_dif_is_unavailable() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut runtime = LutBypassRuntime::new(true, None, None);

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            Some(test_identity()),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[],
        );

        assert_eq!(runtime.overlays_enabled_override, Some(false));
    }

    #[test]
    fn reload_for_new_payload_clears_active_contexts_and_restores_overlay_test_mode() {
        let pipeline = pipeline_for_single_sdr_monitor();
        let mut overlay_test_mode = 0i32;
        let mut disable_independent_flip = 0i32;
        let mut runtime = LutBypassRuntime::new(
            true,
            Some((&mut overlay_test_mode as *mut i32) as usize),
            Some((&mut disable_independent_flip as *mut i32) as usize),
        );

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            Some(test_identity()),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[],
        );

        assert!(runtime.has_active_contexts());
        assert_eq!(overlay_test_mode, 5);
        assert_eq!(disable_independent_flip, 1);
        assert_eq!(runtime.overlays_enabled_override, Some(true));

        runtime.reload_for_new_payload(true);

        assert!(!runtime.has_active_contexts());
        assert_eq!(overlay_test_mode, 0);
        assert_eq!(disable_independent_flip, 0);
        assert_eq!(runtime.overlays_enabled_override, None);
        assert_eq!(runtime.overlay_test_mode(0), 0);
    }
}
