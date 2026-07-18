use crate::lut_pipeline::{
    BackBufferFormat, ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
    DirtyRect, LutPipeline, ShaderConstantsCBuffer,
};
use dwm_lut_payload::MonitorIdentity;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Vertex {
    position: [f32; 2],
    texcoord: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Box3D {
    left: u32,
    top: u32,
    front: u32,
    right: u32,
    bottom: u32,
    back: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct GpuDrawPlan {
    format: BackBufferFormat,
    lut_index: usize,
    constants: ShaderConstantsCBuffer,
    dirty_rects: Vec<DirtyRect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrawPlanSkipReason {
    ZeroSize,
    MissingMonitorIdentity,
    UnsupportedFormat,
    MissingAssignment,
    EmptyDirtyRects,
}

#[cfg(all(not(test), debug_assertions))]
impl DrawPlanSkipReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ZeroSize => "zero_size",
            Self::MissingMonitorIdentity => "missing_monitor_identity",
            Self::UnsupportedFormat => "unsupported_format",
            Self::MissingAssignment => "missing_assignment",
            Self::EmptyDirtyRects => "empty_dirty_rects",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RenderPresentLutResult {
    pub lut_applied: bool,
    pub dxgi_format: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub lut_index: Option<usize>,
    pub present_dirty_rect: Option<DirtyRect>,
}

#[cfg(not(test))]
impl RenderPresentLutResult {
    fn planned(format: BackBufferFormat, lut_index: usize) -> Self {
        Self {
            lut_applied: false,
            dxgi_format: Some(dxgi_format_for_copy_texture(format)),
            width: None,
            height: None,
            lut_index: Some(lut_index),
            present_dirty_rect: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DrawState {
    format: BackBufferFormat,
    lut_index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RenderTargetState {
    Bootstrapping,
    Stable(DrawState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RenderTargetKey {
    overlay_swap_chain: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ResourceKey {
    device: usize,
    overlay_swap_chain: usize,
    width: u32,
    height: u32,
}

fn prepare_gpu_draw_plan(
    pipeline: &LutPipeline,
    monitor_identity: Option<MonitorIdentity>,
    clip_box: ClipBox,
    dxgi_format: u32,
    width: u32,
    height: u32,
    dirty_rects: &[DirtyRect],
) -> Result<GpuDrawPlan, DrawPlanSkipReason> {
    if width == 0 || height == 0 {
        return Err(DrawPlanSkipReason::ZeroSize);
    }

    let Some(identity) = monitor_identity else {
        return Err(DrawPlanSkipReason::MissingMonitorIdentity);
    };
    let Some(format) = BackBufferFormat::from_dxgi_format(dxgi_format) else {
        return Err(DrawPlanSkipReason::UnsupportedFormat);
    };
    let Some(lut_index) = pipeline.select_lut_index_for_monitor_identity(identity, format) else {
        return Err(DrawPlanSkipReason::MissingAssignment);
    };
    let Some(plan) =
        pipeline.build_present_plan_for_lut_index(clip_box, dxgi_format, dirty_rects, lut_index)
    else {
        return Err(DrawPlanSkipReason::MissingAssignment);
    };
    let dirty_rects = draw_rects_for_frame(&plan.dirty_rects, width, height);
    if dirty_rects.is_empty() {
        return Err(DrawPlanSkipReason::EmptyDirtyRects);
    }
    Ok(GpuDrawPlan {
        format: plan.format,
        lut_index: plan.lut_index,
        constants: plan.shader_constants.to_cbuffer(),
        dirty_rects,
    })
}

const fn dxgi_format_for_copy_texture(format: BackBufferFormat) -> u32 {
    match format {
        BackBufferFormat::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
        BackBufferFormat::Rgba16Float => DXGI_FORMAT_R16G16B16A16_FLOAT,
    }
}

fn draw_rects_for_frame(dirty_rects: &[DirtyRect], width: u32, height: u32) -> Vec<DirtyRect> {
    let full_rect;
    let rects = if dirty_rects.is_empty() {
        full_rect = [DirtyRect {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        }];
        &full_rect[..]
    } else {
        dirty_rects
    };

    rects
        .iter()
        .filter_map(|rect| clamp_rect(*rect, width, height))
        .collect()
}

#[cfg_attr(test, allow(dead_code))]
fn draw_rects_for_full_frame(width: u32, height: u32) -> Vec<DirtyRect> {
    draw_rects_for_frame(&[], width, height)
}

fn with_restored_state<State, Capture, Draw, Restore>(
    capture: Capture,
    draw: Draw,
    restore: Restore,
) -> bool
where
    Capture: FnOnce() -> State,
    Draw: FnOnce() -> bool,
    Restore: FnOnce(&State),
{
    let state = capture();
    let result = draw();
    restore(&state);
    result
}

fn copy_box_for_rect(rect: DirtyRect) -> Box3D {
    Box3D {
        left: rect.left as u32,
        top: rect.top as u32,
        front: 0,
        right: rect.right as u32,
        bottom: rect.bottom as u32,
        back: 1,
    }
}

fn vertices_for_rect(rect: DirtyRect, width: u32, height: u32) -> [Vertex; 4] {
    let width = width as f32;
    let height = height as f32;
    let left = rect.left as f32;
    let top = rect.top as f32;
    let right = rect.right as f32;
    let bottom = rect.bottom as f32;
    let ndc_left = (left / width) * 2.0 - 1.0;
    let ndc_right = (right / width) * 2.0 - 1.0;
    let ndc_top = 1.0 - (top / height) * 2.0;
    let ndc_bottom = 1.0 - (bottom / height) * 2.0;

    [
        Vertex {
            position: [ndc_left, ndc_top],
            texcoord: [left / width, top / height],
        },
        Vertex {
            position: [ndc_right, ndc_top],
            texcoord: [right / width, top / height],
        },
        Vertex {
            position: [ndc_left, ndc_bottom],
            texcoord: [left / width, bottom / height],
        },
        Vertex {
            position: [ndc_right, ndc_bottom],
            texcoord: [right / width, bottom / height],
        },
    ]
}

fn clamp_rect(rect: DirtyRect, width: u32, height: u32) -> Option<DirtyRect> {
    let left = rect.left.clamp(0, width as i32);
    let top = rect.top.clamp(0, height as i32);
    let right = rect.right.clamp(0, width as i32);
    let bottom = rect.bottom.clamp(0, height as i32);
    (left < right && top < bottom).then_some(DirtyRect {
        left,
        top,
        right,
        bottom,
    })
}

fn bounding_rect(rects: &[DirtyRect]) -> Option<DirtyRect> {
    let first = *rects.first()?;
    Some(rects.iter().skip(1).fold(first, |bounds, rect| DirtyRect {
        left: bounds.left.min(rect.left),
        top: bounds.top.min(rect.top),
        right: bounds.right.max(rect.right),
        bottom: bounds.bottom.max(rect.bottom),
    }))
}

fn requires_full_redraw(
    previous: RenderTargetState,
    current: DrawState,
    resources_recreated: bool,
    copy_texture_created: bool,
) -> bool {
    match previous {
        RenderTargetState::Bootstrapping => true,
        RenderTargetState::Stable(previous) => {
            resources_recreated || copy_texture_created || previous != current
        }
    }
}

fn present_dirty_rect_for_full_redraw(
    needs_full_redraw: bool,
    previous_state: RenderTargetState,
    dirty_rects: &[DirtyRect],
) -> Option<DirtyRect> {
    (needs_full_redraw && !matches!(previous_state, RenderTargetState::Bootstrapping))
        .then(|| bounding_rect(dirty_rects).unwrap())
}

#[cfg(not(test))]
mod context_state;
#[cfg(not(test))]
mod d3d11_api;
#[cfg(not(test))]
mod renderer;
#[cfg(test)]
mod test_stub;

#[cfg(not(test))]
pub(crate) use renderer::{render_present_lut, shutdown_renderer_resources};
#[cfg(test)]
pub(crate) use test_stub::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lut_pipeline::{
        BackBufferFormat, DXGI_FORMAT_R16G16B16A16_FLOAT, LoadedLut, LutMetadata, LutShaderProgram,
        ShaderTexture3D,
    };
    use dwm_lut_payload::{AdapterLuid, ColorMode, MonitorIdentity, MonitorTarget};
    use std::cell::RefCell;

    fn test_identity() -> MonitorIdentity {
        MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4357,
        }
    }

    fn test_pipeline() -> LutPipeline {
        fn loaded_lut(color_mode: ColorMode) -> LoadedLut {
            LoadedLut {
                target: MonitorTarget {
                    identity: test_identity(),
                    color_mode,
                },
                metadata: LutMetadata {
                    size: 2,
                    domain_min: [0.0, 0.0, 0.0],
                    domain_max: [1.0, 1.0, 1.0],
                },
                texture: ShaderTexture3D {
                    width: 2,
                    height: 2,
                    depth: 2,
                    texels: vec![[0.0, 0.0, 0.0, 1.0]; 8],
                },
            }
        }

        LutPipeline {
            luts: vec![loaded_lut(ColorMode::Sdr), loaded_lut(ColorMode::Hdr)],
            shader: LutShaderProgram::embedded(),
        }
    }

    #[test]
    fn dirty_rect_copy_box_uses_absolute_source_region() {
        let rect = DirtyRect {
            left: 32,
            top: 48,
            right: 96,
            bottom: 128,
        };

        assert_eq!(
            copy_box_for_rect(rect),
            Box3D {
                left: 32,
                top: 48,
                front: 0,
                right: 96,
                bottom: 128,
                back: 1,
            }
        );
    }

    #[test]
    fn dirty_rect_vertices_target_same_absolute_region() {
        let vertices = vertices_for_rect(
            DirtyRect {
                left: 100,
                top: 50,
                right: 300,
                bottom: 250,
            },
            1000,
            500,
        );

        assert_vec2_near(vertices[0].position, [-0.8, 0.8]);
        assert_vec2_near(vertices[1].position, [-0.4, 0.8]);
        assert_vec2_near(vertices[2].position, [-0.8, 0.0]);
        assert_vec2_near(vertices[3].position, [-0.4, 0.0]);
        assert_vec2_near(vertices[0].texcoord, [0.1, 0.1]);
        assert_vec2_near(vertices[3].texcoord, [0.3, 0.5]);
    }

    #[test]
    fn dirty_rects_are_clamped_before_copy_and_draw() {
        assert_eq!(
            clamp_rect(
                DirtyRect {
                    left: -10,
                    top: 5,
                    right: 120,
                    bottom: 80,
                },
                100,
                60,
            ),
            Some(DirtyRect {
                left: 0,
                top: 5,
                right: 100,
                bottom: 60,
            })
        );
        assert_eq!(
            clamp_rect(
                DirtyRect {
                    left: 100,
                    top: 0,
                    right: 120,
                    bottom: 60,
                },
                100,
                60,
            ),
            None
        );
    }

    #[test]
    fn gpu_draw_plan_accepts_sdr_and_hdr_frames_with_size() {
        let pipeline = test_pipeline();
        let clip_box = ClipBox {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let dirty_rects = [DirtyRect {
            left: 0,
            top: 0,
            right: 64,
            bottom: 64,
        }];

        assert!(
            prepare_gpu_draw_plan(
                &pipeline,
                Some(test_identity()),
                clip_box,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                1920,
                1080,
                &dirty_rects,
            )
            .is_ok()
        );
        let hdr_plan = prepare_gpu_draw_plan(
            &pipeline,
            Some(test_identity()),
            clip_box,
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            1920,
            1080,
            &dirty_rects,
        )
        .expect("HDR frames should render when an HDR LUT matches");
        assert_eq!(hdr_plan.format, BackBufferFormat::Rgba16Float);
        assert_eq!(hdr_plan.lut_index, 1);
        assert_eq!(hdr_plan.constants.hdr, 1);
        assert!(
            prepare_gpu_draw_plan(
                &pipeline,
                Some(test_identity()),
                clip_box,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                0,
                1080,
                &dirty_rects,
            )
            .is_err()
        );
    }

    #[test]
    fn copy_texture_format_matches_back_buffer_format() {
        assert_eq!(
            dxgi_format_for_copy_texture(BackBufferFormat::Bgra8Unorm),
            DXGI_FORMAT_B8G8R8A8_UNORM
        );
        assert_eq!(
            dxgi_format_for_copy_texture(BackBufferFormat::Rgba16Float),
            DXGI_FORMAT_R16G16B16A16_FLOAT
        );
    }

    #[test]
    fn gpu_draw_plan_expands_empty_dirty_rects_to_full_frame() {
        let pipeline = test_pipeline();
        let plan = prepare_gpu_draw_plan(
            &pipeline,
            Some(test_identity()),
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            1920,
            1080,
            &[],
        )
        .expect("empty dirty rect input should render the full frame");

        assert_eq!(
            plan.dirty_rects,
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }]
        );
    }

    #[test]
    fn bootstrapping_full_redraw_does_not_expand_present_dirty_rect() {
        let full_frame_rects = vec![DirtyRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }];

        assert_eq!(
            present_dirty_rect_for_full_redraw(
                true,
                RenderTargetState::Bootstrapping,
                &full_frame_rects,
            ),
            None
        );
    }

    #[test]
    fn stable_full_redraw_expands_present_dirty_rect() {
        let sdr = DrawState {
            format: BackBufferFormat::Bgra8Unorm,
            lut_index: 0,
        };
        let full_frame_rects = vec![DirtyRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }];

        assert_eq!(
            present_dirty_rect_for_full_redraw(
                true,
                RenderTargetState::Stable(sdr),
                &full_frame_rects,
            ),
            Some(DirtyRect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            })
        );
    }

    #[test]
    fn stable_partial_update_does_not_expand_present_dirty_rect() {
        let sdr = DrawState {
            format: BackBufferFormat::Bgra8Unorm,
            lut_index: 0,
        };
        let partial_rects = vec![DirtyRect {
            left: 10,
            top: 20,
            right: 64,
            bottom: 96,
        }];

        assert_eq!(
            present_dirty_rect_for_full_redraw(
                false,
                RenderTargetState::Stable(sdr),
                &partial_rects,
            ),
            None
        );
    }

    #[test]
    fn full_redraw_is_required_until_draw_state_is_stable() {
        let sdr = DrawState {
            format: BackBufferFormat::Bgra8Unorm,
            lut_index: 0,
        };
        let hdr = DrawState {
            format: BackBufferFormat::Rgba16Float,
            lut_index: 0,
        };
        let second_lut = DrawState {
            format: BackBufferFormat::Bgra8Unorm,
            lut_index: 1,
        };

        assert!(requires_full_redraw(
            RenderTargetState::Bootstrapping,
            sdr,
            false,
            false
        ));
        assert!(requires_full_redraw(
            RenderTargetState::Stable(sdr),
            sdr,
            true,
            false
        ));
        assert!(requires_full_redraw(
            RenderTargetState::Stable(sdr),
            sdr,
            false,
            true
        ));
        assert!(requires_full_redraw(
            RenderTargetState::Stable(sdr),
            hdr,
            false,
            false
        ));
        assert!(requires_full_redraw(
            RenderTargetState::Stable(sdr),
            second_lut,
            false,
            false
        ));
        assert!(!requires_full_redraw(
            RenderTargetState::Stable(sdr),
            sdr,
            false,
            false
        ));
    }

    #[test]
    fn render_target_key_is_stable_for_one_overlay_swap_chain() {
        let first = RenderTargetKey {
            overlay_swap_chain: 0x1000,
        };
        let second = RenderTargetKey {
            overlay_swap_chain: 0x1000,
        };

        assert_eq!(first, second);
    }

    #[test]
    fn resource_key_distinguishes_overlay_swap_chain_sizes_on_one_device() {
        let first = ResourceKey {
            device: 0x1000,
            overlay_swap_chain: 0x2000,
            width: 1920,
            height: 1080,
        };
        let second = ResourceKey {
            device: 0x1000,
            overlay_swap_chain: 0x3000,
            width: 1280,
            height: 720,
        };

        assert_ne!(first, second);
    }

    #[test]
    fn gpu_draw_plan_ignores_dirty_rects_outside_the_frame() {
        let pipeline = test_pipeline();
        let plan = prepare_gpu_draw_plan(
            &pipeline,
            Some(test_identity()),
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            1920,
            1080,
            &[
                DirtyRect {
                    left: 10,
                    top: 10,
                    right: 20,
                    bottom: 20,
                },
                DirtyRect {
                    left: 1920,
                    top: 0,
                    right: 1940,
                    bottom: 50,
                },
            ],
        )
        .expect("one dirty rect intersects the frame");

        assert_eq!(
            plan.dirty_rects,
            vec![DirtyRect {
                left: 10,
                top: 10,
                right: 20,
                bottom: 20,
            }]
        );
        assert!(
            prepare_gpu_draw_plan(
                &pipeline,
                Some(test_identity()),
                ClipBox {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                DXGI_FORMAT_B8G8R8A8_UNORM,
                1920,
                1080,
                &[DirtyRect {
                    left: 1920,
                    top: 0,
                    right: 1940,
                    bottom: 50,
                }],
            )
            .is_err()
        );
    }

    #[test]
    fn gpu_draw_plan_selects_lut_by_runtime_monitor_identity() {
        fn loaded_lut(_label: &str, identity: MonitorIdentity, color_mode: ColorMode) -> LoadedLut {
            LoadedLut {
                target: MonitorTarget {
                    identity,
                    color_mode,
                },
                metadata: LutMetadata {
                    size: 2,
                    domain_min: [0.0, 0.0, 0.0],
                    domain_max: [1.0, 1.0, 1.0],
                },
                texture: ShaderTexture3D {
                    width: 2,
                    height: 2,
                    depth: 2,
                    texels: vec![[0.0, 0.0, 0.0, 1.0]; 8],
                },
            }
        }

        let primary = MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 11,
        };
        let right = MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4357,
        };
        let pipeline = LutPipeline {
            luts: vec![
                loaded_lut("PRIMARY", primary, ColorMode::Sdr),
                loaded_lut("RIGHT", right, ColorMode::Sdr),
            ],
            shader: LutShaderProgram::embedded(),
        };

        let plan = prepare_gpu_draw_plan(
            &pipeline,
            Some(right),
            ClipBox {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            1920,
            1080,
            &[],
        )
        .expect("runtime monitor identity should select a plan");

        assert_eq!(plan.lut_index, 1);
        assert!(
            prepare_gpu_draw_plan(
                &pipeline,
                None,
                ClipBox {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                },
                DXGI_FORMAT_B8G8R8A8_UNORM,
                1920,
                1080,
                &[],
            )
            .is_err()
        );
    }

    #[test]
    fn draw_lifecycle_restores_context_state_after_draw_work() {
        let events = RefCell::new(Vec::new());

        let result = with_restored_state(
            || {
                events.borrow_mut().push("capture");
                "captured-state"
            },
            || {
                events.borrow_mut().push("draw");
                true
            },
            |state| {
                assert_eq!(*state, "captured-state");
                events.borrow_mut().push("restore");
            },
        );

        assert!(result);
        assert_eq!(&*events.borrow(), &["capture", "draw", "restore"]);
    }

    fn assert_vec2_near(actual: [f32; 2], expected: [f32; 2]) {
        const EPSILON: f32 = 0.000_001;
        assert!((actual[0] - expected[0]).abs() <= EPSILON);
        assert!((actual[1] - expected[1]).abs() <= EPSILON);
    }
}
