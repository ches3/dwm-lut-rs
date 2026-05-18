use std::collections::BTreeMap;

use crate::lut_pipeline::{BackBufferFormat, ClipBox, DirtyRect, LutPipeline, LutRenderPlan};

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
    pub clip_box: ClipBox,
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
pub struct LutBypassRuntime {
    pub overlay_test_mode_control: OverlayTestModeControl,
    pub overlay_test_mode_patch: Option<OverlayTestModePatch>,
    pub has_lut_assignments: bool,
    pub contexts: BTreeMap<usize, ContextLutState>,
}

impl LutBypassRuntime {
    pub fn new(has_lut_assignments: bool, overlay_test_mode_address: Option<usize>) -> Self {
        Self {
            overlay_test_mode_control: OverlayTestModeControl::Unmodified,
            overlay_test_mode_patch: overlay_test_mode_address
                .filter(|address| *address != 0)
                .map(|address| OverlayTestModePatch {
                    address,
                    original_mode: None,
                    applied: false,
                }),
            has_lut_assignments,
            contexts: BTreeMap::new(),
        }
    }

    pub fn update_present(
        &mut self,
        lut_pipeline: &LutPipeline,
        context_address: usize,
        clip_box: ClipBox,
        dxgi_format: u32,
        dirty_rects: &[DirtyRect],
        lut_applied: bool,
    ) -> PresentHookOutcome {
        let plan = lut_pipeline.build_present_plan(clip_box, dxgi_format, dirty_rects);
        let back_buffer_format = BackBufferFormat::from_dxgi_format(dxgi_format);
        let promotion_blocked = plan.is_some() && lut_applied;
        let lut_index = plan.as_ref().map(|plan| plan.lut_index);

        if promotion_blocked {
            self.contexts.insert(
                context_address,
                ContextLutState {
                    clip_box,
                    back_buffer_format,
                    lut_index,
                    dirty_rect_count: dirty_rects.len(),
                },
            );
        } else {
            self.contexts.remove(&context_address);
        }

        self.set_overlay_test_mode_control(self.overlay_test_mode_control());

        PresentHookOutcome {
            plan,
            promotion_blocked,
            overlay_test_mode_control: self.overlay_test_mode_control,
        }
    }

    pub fn overlays_enabled(&mut self, context_address: usize, original_enabled: bool) -> bool {
        if self.contexts.contains_key(&context_address) {
            false
        } else {
            original_enabled
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

    pub fn window_context_direct_flip_compatible(&self, original_compatible: bool) -> bool {
        if self.has_lut_assignments {
            false
        } else {
            original_compatible
        }
    }

    pub fn comp_swap_chain_direct_flip_compatible(&self, original_compatible: bool) -> bool {
        if self.has_lut_assignments {
            false
        } else {
            original_compatible
        }
    }

    pub fn comp_visual_candidate_for_promotion(&self, original_candidate: bool) -> bool {
        if self.has_lut_assignments {
            false
        } else {
            original_candidate
        }
    }

    pub fn comp_swap_chain_independent_flip_compatible(&self, original_compatible: bool) -> bool {
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
                let original_mode = unsafe { read_overlay_test_mode(patch.address) };
                patch.original_mode = Some(original_mode);
                unsafe { write_overlay_test_mode(patch.address, 5) };
                patch.applied = true;
            }
            OverlayTestModeControl::Unmodified if patch.applied => {
                unsafe { write_overlay_test_mode(patch.address, patch.original_mode.unwrap_or(0)) };
                patch.applied = false;
            }
            _ => {}
        }
    }
}

impl Default for LutBypassRuntime {
    fn default() -> Self {
        Self::new(false, None)
    }
}

unsafe fn read_overlay_test_mode(address: usize) -> i32 {
    unsafe { (address as *const i32).read_volatile() }
}

unsafe fn write_overlay_test_mode(address: usize, mode: i32) {
    unsafe { (address as *mut i32).write_volatile(mode) };
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use dwm_lut_config::{ColorMode, LutAssignment, LutManifest, MonitorTarget};

    use super::{LutBypassRuntime, OverlayTestModeControl};
    use crate::lut_pipeline::{ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DirtyRect, LutPipeline};

    fn write_test_cube() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dwm-lut-phase6-{unique}.cube"));
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

    fn pipeline_for_single_sdr_monitor() -> (LutPipeline, PathBuf) {
        let cube_path = write_test_cube();
        let mut manifest = LutManifest::empty();
        manifest.add(LutAssignment {
            target: MonitorTarget {
                monitor_id: "DISPLAY1".into(),
                desktop_left: 0,
                desktop_top: 0,
                desktop_right: None,
                desktop_bottom: None,
                color_mode: ColorMode::Sdr,
            },
            lut_path: cube_path.clone(),
            lut_size: 2,
        });

        (
            LutPipeline::load(&manifest).expect("pipeline should load"),
            cube_path,
        )
    }

    #[test]
    fn present_activation_blocks_promotion_for_same_context_only() {
        let (pipeline, cube_path) = pipeline_for_single_sdr_monitor();
        let mut runtime = LutBypassRuntime::new(true, None);

        let outcome = runtime.update_present(
            &pipeline,
            0x1234,
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
            true,
        );

        assert!(outcome.plan.is_some());
        assert!(outcome.promotion_blocked);
        assert_eq!(
            outcome.overlay_test_mode_control,
            OverlayTestModeControl::ForceMode5
        );
        assert!(!runtime.overlays_enabled(0x1234, true));
        assert!(!runtime.direct_flip_compatible(0x1234, true));
        assert!(!runtime.window_context_direct_flip_compatible(true));
        assert!(!runtime.comp_swap_chain_direct_flip_compatible(true));
        assert!(!runtime.comp_visual_candidate_for_promotion(true));
        assert_eq!(runtime.overlay_test_mode(0), 5);
        assert!(runtime.overlays_enabled(0x4321, true));
        assert!(runtime.direct_flip_compatible(0x4321, true));
        assert_eq!(
            runtime.overlay_test_mode_control,
            OverlayTestModeControl::ForceMode5
        );

        let context = runtime.context(0x1234).expect("context should exist");
        assert_eq!(context.lut_index, Some(0));
        assert_eq!(context.dirty_rect_count, 1);

        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn present_deactivation_clears_promotion_block_for_that_context() {
        let (pipeline, cube_path) = pipeline_for_single_sdr_monitor();
        let mut runtime = LutBypassRuntime::new(true, None);

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[],
            true,
        );

        let outcome = runtime.update_present(
            &pipeline,
            0x1234,
            ClipBox {
                left: 100,
                top: 100,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[],
            false,
        );

        assert!(outcome.plan.is_none());
        assert!(!outcome.promotion_blocked);
        assert!(runtime.overlays_enabled(0x1234, true));
        assert!(runtime.direct_flip_compatible(0x1234, true));
        assert!(!runtime.window_context_direct_flip_compatible(true));
        assert!(!runtime.comp_swap_chain_direct_flip_compatible(true));
        assert!(!runtime.comp_visual_candidate_for_promotion(true));
        assert_eq!(runtime.overlay_test_mode(0), 0);
        assert!(runtime.context(0x1234).is_none());

        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn plan_without_success_does_not_activate_bypass_state() {
        let (pipeline, cube_path) = pipeline_for_single_sdr_monitor();
        let mut runtime = LutBypassRuntime::new(true, None);

        let outcome = runtime.update_present(
            &pipeline,
            0x1234,
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
            false,
        );

        assert!(outcome.plan.is_some());
        assert!(!outcome.promotion_blocked);
        assert!(runtime.context(0x1234).is_none());
        assert!(runtime.overlays_enabled(0x1234, true));
        assert!(runtime.direct_flip_compatible(0x1234, true));
        assert!(!runtime.window_context_direct_flip_compatible(true));
        assert!(!runtime.comp_swap_chain_direct_flip_compatible(true));
        assert!(!runtime.comp_visual_candidate_for_promotion(true));
        assert_eq!(runtime.overlay_test_mode(0), 0);

        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn active_context_persists_until_explicit_deactivation() {
        let (pipeline, cube_path) = pipeline_for_single_sdr_monitor();
        let mut runtime = LutBypassRuntime::new(true, None);

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
            true,
        );

        assert!(runtime.context(0x1234).is_some());
        assert!(!runtime.window_context_direct_flip_compatible(true));
        assert_eq!(runtime.overlay_test_mode(0), 5);

        let _ = fs::remove_file(cube_path);
    }

    #[test]
    fn overlay_test_mode_global_is_patched_only_while_context_is_active() {
        let (pipeline, cube_path) = pipeline_for_single_sdr_monitor();
        let mut overlay_test_mode = 0i32;
        let mut runtime =
            LutBypassRuntime::new(true, Some((&mut overlay_test_mode as *mut i32) as usize));

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[],
            true,
        );

        assert_eq!(overlay_test_mode, 5);

        let _ = runtime.update_present(
            &pipeline,
            0x1234,
            ClipBox {
                left: 100,
                top: 100,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &[],
            false,
        );

        assert_eq!(overlay_test_mode, 0);

        let _ = fs::remove_file(cube_path);
    }
}
