use crate::lut_pipeline::{
    BackBufferFormat, ClipBox, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
    DirtyRect, LutPipeline, ShaderConstantsCBuffer,
};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

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

struct D3D11VtableIndex;

impl D3D11VtableIndex {
    const DEVICE_CHILD_GET_DEVICE: usize = 3;
    const TEXTURE2D_GET_DESC: usize = 10;
    const DEVICE_CREATE_RENDER_TARGET_VIEW: usize = 9;
    const DEVICE_GET_IMMEDIATE_CONTEXT: usize = 40;
    const CONTEXT_COPY_SUBRESOURCE_REGION: usize = 46;
    const CONTEXT_UPDATE_SUBRESOURCE: usize = 48;
}

#[derive(Clone, Debug, PartialEq)]
struct GpuDrawPlan {
    format: BackBufferFormat,
    lut_index: usize,
    constants: ShaderConstantsCBuffer,
    dirty_rects: Vec<DirtyRect>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RenderPresentLutResult {
    pub lut_applied: bool,
    pub present_dirty_rect: Option<DirtyRect>,
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
    clip_box: ClipBox,
    dxgi_format: u32,
    width: u32,
    height: u32,
    dirty_rects: &[DirtyRect],
) -> Option<GpuDrawPlan> {
    if width == 0 || height == 0 {
        return None;
    }

    let plan = pipeline.build_present_plan(clip_box, dxgi_format, dirty_rects)?;
    let dirty_rects = draw_rects_for_frame(&plan.dirty_rects, width, height, false);
    (!dirty_rects.is_empty()).then_some(GpuDrawPlan {
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

fn draw_rects_for_frame(
    dirty_rects: &[DirtyRect],
    width: u32,
    height: u32,
    force_full_frame: bool,
) -> Vec<DirtyRect> {
    let full_rect;
    let rects = if force_full_frame || dirty_rects.is_empty() {
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

#[cfg(test)]
static TEST_RENDER_PRESENT_LUT_RESULT: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static TEST_RENDER_PRESENT_DIRTY_RECT: OnceLock<Mutex<Option<DirtyRect>>> = OnceLock::new();

#[cfg(test)]
static TEST_RENDER_CONTEXT_ACTIVE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TestRenderPresentLutCall {
    pub overlay_swap_chain: usize,
    pub swap_chain_path: crate::profile::SwapChainPathHypothesis,
    pub clip_box: ClipBox,
    pub dirty_rects: Vec<DirtyRect>,
}

#[cfg(test)]
static TEST_RENDER_PRESENT_LUT_CALL: OnceLock<Mutex<Option<TestRenderPresentLutCall>>> =
    OnceLock::new();

#[cfg(test)]
pub(crate) fn set_test_render_present_lut_result(result: bool) {
    TEST_RENDER_PRESENT_LUT_RESULT.store(result, Ordering::Release);
    set_test_render_present_dirty_rect(None);
}

#[cfg(test)]
pub(crate) fn set_test_render_present_lut_result_with_present_rect(
    result: bool,
    rect: Option<DirtyRect>,
) {
    TEST_RENDER_PRESENT_LUT_RESULT.store(result, Ordering::Release);
    set_test_render_present_dirty_rect(rect);
}

#[cfg(test)]
fn set_test_render_present_dirty_rect(rect: Option<DirtyRect>) {
    let result = TEST_RENDER_PRESENT_DIRTY_RECT.get_or_init(|| Mutex::new(None));
    if let Ok(mut result) = result.lock() {
        *result = rect;
    }
}

#[cfg(test)]
pub(crate) fn reset_test_render_present_lut_result() {
    set_test_render_present_lut_result(false);
    let calls = TEST_RENDER_PRESENT_LUT_CALL.get_or_init(|| Mutex::new(None));
    if let Ok(mut calls) = calls.lock() {
        *calls = None;
    }
    set_test_render_present_dirty_rect(None);
    let context_active = TEST_RENDER_CONTEXT_ACTIVE.get_or_init(|| Mutex::new(None));
    if let Ok(mut context_active) = context_active.lock() {
        *context_active = None;
    }
}

#[cfg(test)]
pub(crate) fn test_render_present_lut_call() -> Option<TestRenderPresentLutCall> {
    TEST_RENDER_PRESENT_LUT_CALL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|call| call.clone())
}

#[cfg(test)]
pub(crate) fn test_render_context_active() -> Option<bool> {
    TEST_RENDER_CONTEXT_ACTIVE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|active| *active)
}

#[cfg(test)]
pub(crate) unsafe fn render_present_lut(
    overlay_swap_chain: usize,
    swap_chain_path: crate::profile::SwapChainPathHypothesis,
    clip_box: ClipBox,
    dirty_rects: &[DirtyRect],
    _pipeline: &LutPipeline,
) -> RenderPresentLutResult {
    let calls = TEST_RENDER_PRESENT_LUT_CALL.get_or_init(|| Mutex::new(None));
    if let Ok(mut calls) = calls.lock() {
        *calls = Some(TestRenderPresentLutCall {
            overlay_swap_chain,
            swap_chain_path,
            clip_box,
            dirty_rects: dirty_rects.to_vec(),
        });
    }
    let context_active = TEST_RENDER_CONTEXT_ACTIVE.get_or_init(|| Mutex::new(None));
    if let Ok(mut context_active) = context_active.lock() {
        *context_active = Some(
            crate::state::lut_bypass_runtime().is_some_and(|runtime| runtime.has_active_contexts()),
        );
    }
    let present_dirty_rect = TEST_RENDER_PRESENT_DIRTY_RECT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|rect| *rect);
    RenderPresentLutResult {
        lut_applied: TEST_RENDER_PRESENT_LUT_RESULT.load(Ordering::Acquire),
        present_dirty_rect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lut_pipeline::{
        BackBufferFormat, DXGI_FORMAT_R16G16B16A16_FLOAT, LoadedLut, LutMetadata, LutShaderProgram,
        ShaderTexture3D,
    };
    use dwm_lut_config::{ColorMode, LutAssignment, MonitorTarget};
    use std::cell::RefCell;

    fn test_pipeline() -> LutPipeline {
        fn loaded_lut(color_mode: ColorMode) -> LoadedLut {
            LoadedLut {
                assignment: LutAssignment {
                    target: MonitorTarget {
                        monitor_id: "DISPLAY1".into(),
                        desktop_left: 0,
                        desktop_top: 0,
                        desktop_right: Some(1920),
                        desktop_bottom: Some(1080),
                        color_mode,
                    },
                    lut_path: match color_mode {
                        ColorMode::Sdr => "identity-sdr.cube".into(),
                        ColorMode::Hdr => "identity-hdr.cube".into(),
                    },
                    lut_size: 2,
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
    fn d3d11_abi_indices_cover_render_path_external_calls() {
        assert_eq!(D3D11VtableIndex::DEVICE_CHILD_GET_DEVICE, 3);
        assert_eq!(D3D11VtableIndex::TEXTURE2D_GET_DESC, 10);
        assert_eq!(D3D11VtableIndex::DEVICE_CREATE_RENDER_TARGET_VIEW, 9);
        assert_eq!(D3D11VtableIndex::DEVICE_GET_IMMEDIATE_CONTEXT, 40);
        assert_eq!(D3D11VtableIndex::CONTEXT_COPY_SUBRESOURCE_REGION, 46);
        assert_eq!(D3D11VtableIndex::CONTEXT_UPDATE_SUBRESOURCE, 48);
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
                clip_box,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                1920,
                1080,
                &dirty_rects,
            )
            .is_some()
        );
        let hdr_plan = prepare_gpu_draw_plan(
            &pipeline,
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
                clip_box,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                0,
                1080,
                &dirty_rects,
            )
            .is_none()
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
    fn draw_rects_can_force_full_frame_despite_dirty_rects() {
        let rects = draw_rects_for_frame(
            &[DirtyRect {
                left: 10,
                top: 20,
                right: 30,
                bottom: 40,
            }],
            1920,
            1080,
            true,
        );

        assert_eq!(
            rects,
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }]
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
            ClipBox {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            DXGI_FORMAT_B8G8R8A8_UNORM,
            100,
            100,
            &[
                DirtyRect {
                    left: 10,
                    top: 10,
                    right: 20,
                    bottom: 20,
                },
                DirtyRect {
                    left: 100,
                    top: 0,
                    right: 120,
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
                ClipBox {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                DXGI_FORMAT_B8G8R8A8_UNORM,
                100,
                100,
                &[DirtyRect {
                    left: 100,
                    top: 0,
                    right: 120,
                    bottom: 50,
                }],
            )
            .is_none()
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

#[cfg(not(test))]
mod imp {
    use std::collections::BTreeMap;
    use std::ffi::c_void;
    use std::mem::{size_of, transmute};
    use std::ptr;
    use std::sync::{Mutex, OnceLock};

    use super::{Box3D, Vertex};
    use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    use crate::lut_pipeline::{
        BackBufferFormat, ClipBox, DirtyRect, LutPipeline, ShaderConstantsCBuffer,
    };
    use crate::profile::SwapChainPathHypothesis;

    type Hresult = i32;
    type ComPtr = *mut c_void;

    const S_OK: Hresult = 0;
    const DXGI_FORMAT_R32G32B32A32_FLOAT: u32 = 2;
    const D3D11_USAGE_DEFAULT: u32 = 0;
    const D3D11_BIND_VERTEX_BUFFER: u32 = 0x1;
    const D3D11_BIND_SHADER_RESOURCE: u32 = 0x8;
    const D3D11_BIND_CONSTANT_BUFFER: u32 = 0x4;
    const D3D11_RESOURCE_MISC_NONE: u32 = 0;
    const D3D11_INPUT_PER_VERTEX_DATA: u32 = 0;
    const D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP: u32 = 5;
    const D3D11_FILTER_MIN_MAG_MIP_POINT: u32 = 0;
    const D3D11_TEXTURE_ADDRESS_CLAMP: u32 = 3;
    const D3D11_COMPARISON_NEVER: u32 = 1;
    const D3D11_FLOAT32_MAX: f32 = f32::MAX;
    const D3D11_SHADER_CLASS_INSTANCE_LIMIT: usize = 256;
    const D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT: usize = 8;

    static RENDERER: OnceLock<Mutex<D3D11Renderer>> = OnceLock::new();

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Texture2DDesc {
        width: u32,
        height: u32,
        mip_levels: u32,
        array_size: u32,
        format: u32,
        sample_count: u32,
        sample_quality: u32,
        usage: u32,
        bind_flags: u32,
        cpu_access_flags: u32,
        misc_flags: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Texture3DDesc {
        width: u32,
        height: u32,
        depth: u32,
        mip_levels: u32,
        format: u32,
        usage: u32,
        bind_flags: u32,
        cpu_access_flags: u32,
        misc_flags: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct BufferDesc {
        byte_width: u32,
        usage: u32,
        bind_flags: u32,
        cpu_access_flags: u32,
        misc_flags: u32,
        structure_byte_stride: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SubresourceData {
        sys_mem: *const c_void,
        sys_mem_pitch: u32,
        sys_mem_slice_pitch: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InputElementDesc {
        semantic_name: *const u8,
        semantic_index: u32,
        format: u32,
        input_slot: u32,
        aligned_byte_offset: u32,
        input_slot_class: u32,
        instance_data_step_rate: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SamplerDesc {
        filter: u32,
        address_u: u32,
        address_v: u32,
        address_w: u32,
        mip_lod_bias: f32,
        max_anisotropy: u32,
        comparison_func: u32,
        border_color: [f32; 4],
        min_lod: f32,
        max_lod: f32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Viewport {
        top_left_x: f32,
        top_left_y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    }

    unsafe extern "system" {
        fn dwm_lut_get_back_buffer_25h2(
            overlay_swap_chain: *mut c_void,
            container_vtable_index: usize,
            resource_vtable_index: usize,
        ) -> ComPtr;
    }

    type D3DCompileApi = unsafe extern "system" fn(
        src_data: *const c_void,
        src_data_size: usize,
        source_name: *const u8,
        defines: *const c_void,
        include: *mut c_void,
        entrypoint: *const u8,
        target: *const u8,
        flags1: u32,
        flags2: u32,
        code: *mut ComPtr,
        error_msgs: *mut ComPtr,
    ) -> Hresult;

    struct D3DCompiler {
        module: HMODULE,
        compile: D3DCompileApi,
    }

    impl Drop for D3DCompiler {
        fn drop(&mut self) {
            unsafe {
                FreeLibrary(self.module);
            }
        }
    }

    struct D3D11Renderer {
        compiler: Option<D3DCompiler>,
        devices: BTreeMap<super::ResourceKey, DeviceResources>,
    }

    unsafe impl Send for D3D11Renderer {}

    #[derive(Clone, Copy)]
    struct RenderFrame {
        device: ComPtr,
        context: ComPtr,
        back_buffer: ComPtr,
        width: u32,
        height: u32,
    }

    impl D3D11Renderer {
        fn new() -> Self {
            Self {
                compiler: None,
                devices: BTreeMap::new(),
            }
        }

        unsafe fn render_present_lut(
            &mut self,
            overlay_swap_chain: usize,
            swap_chain_path: SwapChainPathHypothesis,
            clip_box: ClipBox,
            dirty_rects: &[DirtyRect],
            pipeline: &LutPipeline,
        ) -> super::RenderPresentLutResult {
            let Some(back_buffer) =
                (unsafe { overlay_swap_chain_to_back_buffer(overlay_swap_chain, swap_chain_path) })
            else {
                return super::RenderPresentLutResult::default();
            };

            let mut device: ComPtr = ptr::null_mut();
            unsafe {
                d3d11_device_child_get_device(back_buffer, &mut device);
            }
            if device.is_null() {
                unsafe { release(back_buffer) };
                return super::RenderPresentLutResult::default();
            }

            let mut context: ComPtr = ptr::null_mut();
            unsafe {
                d3d11_device_get_immediate_context(device, &mut context);
            }
            if context.is_null() {
                unsafe {
                    release(device);
                    release(back_buffer);
                }
                return super::RenderPresentLutResult::default();
            }

            let mut desc = Texture2DDesc {
                width: 0,
                height: 0,
                mip_levels: 0,
                array_size: 0,
                format: 0,
                sample_count: 0,
                sample_quality: 0,
                usage: 0,
                bind_flags: 0,
                cpu_access_flags: 0,
                misc_flags: 0,
            };
            unsafe {
                d3d11_texture2d_get_desc(back_buffer, &mut desc);
            }

            let Some(draw_plan) = super::prepare_gpu_draw_plan(
                pipeline,
                clip_box,
                desc.format,
                desc.width,
                desc.height,
                dirty_rects,
            ) else {
                unsafe {
                    release(back_buffer);
                    release(context);
                    release(device);
                }
                return super::RenderPresentLutResult::default();
            };

            let frame = RenderFrame {
                device,
                context,
                back_buffer,
                width: desc.width,
                height: desc.height,
            };
            let result =
                unsafe { self.render_with_device(overlay_swap_chain, frame, pipeline, draw_plan) };

            unsafe {
                release(back_buffer);
                release(context);
                release(device);
            }
            result
        }

        unsafe fn render_with_device(
            &mut self,
            overlay_swap_chain: usize,
            frame: RenderFrame,
            pipeline: &LutPipeline,
            mut draw_plan: super::GpuDrawPlan,
        ) -> super::RenderPresentLutResult {
            let device_key = frame.device as usize;
            let resource_key = super::ResourceKey {
                device: device_key,
                overlay_swap_chain,
                width: frame.width,
                height: frame.height,
            };
            let compile = match self.compiler() {
                Some(compiler) => compiler.compile,
                None => return super::RenderPresentLutResult::default(),
            };

            let recreate = self
                .devices
                .get(&resource_key)
                .is_none_or(|resources| resources.lut_count != pipeline.luts.len());
            if recreate {
                self.devices.remove(&resource_key);
                let Some(resources) = (unsafe {
                    DeviceResources::create(
                        frame.device,
                        frame.context,
                        frame.width,
                        frame.height,
                        pipeline,
                        compile,
                    )
                }) else {
                    return super::RenderPresentLutResult::default();
                };
                self.devices.insert(resource_key, resources);
            }

            let Some(resources) = self.devices.get_mut(&resource_key) else {
                return super::RenderPresentLutResult::default();
            };
            if draw_plan.lut_index >= resources.lut_srvs.len() {
                return super::RenderPresentLutResult::default();
            }

            let current_draw_state = super::DrawState {
                format: draw_plan.format,
                lut_index: draw_plan.lut_index,
            };
            let render_target_key = super::RenderTargetKey { overlay_swap_chain };
            let previous_state = resources
                .draw_states
                .get(&render_target_key)
                .copied()
                .unwrap_or(super::RenderTargetState::Bootstrapping);
            let copy_texture_created = !resources.copy_textures.has_format(draw_plan.format);
            let needs_full_redraw = super::requires_full_redraw(
                previous_state,
                current_draw_state,
                recreate,
                copy_texture_created,
            );
            if needs_full_redraw {
                debug_log!(
                    "event=lut_full_redraw device=0x{:x} overlay_swap_chain=0x{:x} back_buffer=0x{:x} width={} height={} format={:?} lut_index={} resources_recreated={} copy_texture_created={} previous_state={:?} dirty_rect_count={}",
                    device_key,
                    overlay_swap_chain,
                    frame.back_buffer as usize,
                    frame.width,
                    frame.height,
                    draw_plan.format,
                    draw_plan.lut_index,
                    recreate,
                    copy_texture_created,
                    previous_state,
                    draw_plan.dirty_rects.len()
                );
                draw_plan.dirty_rects = super::draw_rects_for_frame(
                    &draw_plan.dirty_rects,
                    frame.width,
                    frame.height,
                    true,
                );
            }
            if draw_plan.dirty_rects.is_empty() {
                return super::RenderPresentLutResult::default();
            }
            let present_dirty_rect = needs_full_redraw.then_some(DirtyRect {
                left: 0,
                top: 0,
                right: frame.width as i32,
                bottom: frame.height as i32,
            });

            let result = unsafe { resources.draw(frame, &draw_plan) };
            if result {
                resources.draw_states.insert(
                    render_target_key,
                    super::RenderTargetState::Stable(current_draw_state),
                );
            }
            super::RenderPresentLutResult {
                lut_applied: result,
                present_dirty_rect: result.then_some(present_dirty_rect).flatten(),
            }
        }

        fn compiler(&mut self) -> Option<&D3DCompiler> {
            if self.compiler.is_none() {
                self.compiler = unsafe { load_compiler() };
            }
            self.compiler.as_ref()
        }
    }

    struct DeviceResources {
        width: u32,
        height: u32,
        lut_count: usize,
        vertex_shader: ComPtr,
        pixel_shader: ComPtr,
        input_layout: ComPtr,
        vertex_buffer: ComPtr,
        sampler: ComPtr,
        constant_buffer: ComPtr,
        copy_textures: CopyTextureResources,
        lut_textures: Vec<ComPtr>,
        lut_srvs: Vec<ComPtr>,
        draw_states: BTreeMap<super::RenderTargetKey, super::RenderTargetState>,
    }

    unsafe impl Send for DeviceResources {}

    #[derive(Default)]
    struct CopyTextureResources {
        sdr: Option<CopyTextureResource>,
        hdr: Option<CopyTextureResource>,
    }

    struct CopyTextureResource {
        texture: ComPtr,
        srv: ComPtr,
    }

    unsafe impl Send for CopyTextureResources {}

    impl CopyTextureResources {
        fn has_format(&self, format: BackBufferFormat) -> bool {
            match format {
                BackBufferFormat::Bgra8Unorm => self.sdr.is_some(),
                BackBufferFormat::Rgba16Float => self.hdr.is_some(),
            }
        }

        unsafe fn for_format(
            &mut self,
            device: ComPtr,
            width: u32,
            height: u32,
            format: BackBufferFormat,
        ) -> Option<&CopyTextureResource> {
            let slot = match format {
                BackBufferFormat::Bgra8Unorm => &mut self.sdr,
                BackBufferFormat::Rgba16Float => &mut self.hdr,
            };
            if slot.is_none() {
                *slot =
                    Some(unsafe { CopyTextureResource::create(device, width, height, format) }?);
            }
            slot.as_ref()
        }
    }

    impl CopyTextureResource {
        unsafe fn create(
            device: ComPtr,
            width: u32,
            height: u32,
            format: BackBufferFormat,
        ) -> Option<Self> {
            let (texture, srv) = unsafe { create_copy_texture(device, width, height, format) }?;
            Some(Self {
                texture: texture.into_raw(),
                srv: srv.into_raw(),
            })
        }
    }

    impl Drop for CopyTextureResource {
        fn drop(&mut self) {
            unsafe {
                release(self.srv);
                release(self.texture);
            }
        }
    }

    impl DeviceResources {
        unsafe fn create(
            device: ComPtr,
            context: ComPtr,
            width: u32,
            height: u32,
            pipeline: &LutPipeline,
            compile: D3DCompileApi,
        ) -> Option<Self> {
            let shader = &pipeline.shader;
            let vertex_blob = unsafe {
                compile_shader(
                    compile,
                    shader.source,
                    shader.vertex_entry,
                    shader.vertex_profile,
                )
            }?;
            let pixel_blob = unsafe {
                compile_shader(
                    compile,
                    shader.source,
                    shader.pixel_entry,
                    shader.pixel_profile,
                )
            }?;

            let vertex_shader = unsafe { create_vertex_shader(device, &vertex_blob) }?;
            let vertex_shader = OwnedCom::new(vertex_shader);
            let pixel_shader = OwnedCom::new(unsafe { create_pixel_shader(device, &pixel_blob) }?);
            let input_layout = OwnedCom::new(unsafe { create_input_layout(device, &vertex_blob) }?);
            let vertex_buffer = OwnedCom::new(unsafe { create_vertex_buffer(device) }?);
            let sampler = OwnedCom::new(unsafe { create_sampler(device) }?);
            let constant_buffer = OwnedCom::new(unsafe { create_constant_buffer(device) }?);
            let mut lut_textures = Vec::with_capacity(pipeline.luts.len());
            let mut lut_srvs = Vec::with_capacity(pipeline.luts.len());
            for lut in &pipeline.luts {
                let (texture, srv) = (unsafe { create_lut_texture(device, lut) })?;
                lut_textures.push(texture);
                lut_srvs.push(srv);
            }

            let _ = context;
            Some(Self {
                width,
                height,
                lut_count: pipeline.luts.len(),
                vertex_shader: vertex_shader.into_raw(),
                pixel_shader: pixel_shader.into_raw(),
                input_layout: input_layout.into_raw(),
                vertex_buffer: vertex_buffer.into_raw(),
                sampler: sampler.into_raw(),
                constant_buffer: constant_buffer.into_raw(),
                copy_textures: CopyTextureResources::default(),
                lut_textures: lut_textures.into_iter().map(OwnedCom::into_raw).collect(),
                lut_srvs: lut_srvs.into_iter().map(OwnedCom::into_raw).collect(),
                draw_states: BTreeMap::new(),
            })
        }

        unsafe fn draw(&mut self, frame: RenderFrame, draw_plan: &super::GpuDrawPlan) -> bool {
            let Some((copy_texture, copy_srv)) = (unsafe {
                self.copy_textures
                    .for_format(frame.device, frame.width, frame.height, draw_plan.format)
                    .map(|resource| (resource.texture, resource.srv))
            }) else {
                return false;
            };
            let mut rtv: ComPtr = ptr::null_mut();
            if unsafe {
                d3d11_device_create_render_target_view_from_context(
                    frame.context,
                    frame.back_buffer,
                    &mut rtv,
                )
            } < S_OK
                || rtv.is_null()
            {
                return false;
            }
            let drew_any = super::with_restored_state(
                || unsafe { ContextState::capture(frame.context) },
                || {
                    unsafe {
                        d3d11_context_update_subresource(
                            frame.context,
                            self.constant_buffer,
                            &draw_plan.constants as *const ShaderConstantsCBuffer as *const c_void,
                            0,
                            0,
                        );
                    }

                    let mut drew_any = false;
                    for rect in &draw_plan.dirty_rects {
                        let rect = *rect;
                        let box3d = super::copy_box_for_rect(rect);
                        unsafe {
                            d3d11_context_copy_subresource_region(
                                frame.context,
                                copy_texture,
                                rect.left as u32,
                                rect.top as u32,
                                frame.back_buffer,
                                &box3d,
                            );
                        }

                        let vertices = super::vertices_for_rect(rect, frame.width, frame.height);
                        unsafe {
                            bind_pipeline(
                                frame.context,
                                self,
                                rtv,
                                copy_srv,
                                self.lut_srvs[draw_plan.lut_index],
                            );
                            d3d11_context_update_subresource(
                                frame.context,
                                self.vertex_buffer,
                                vertices.as_ptr().cast(),
                                0,
                                0,
                            );
                            d3d11_context_draw(frame.context, 4, 0);
                            unbind_pipeline(frame.context);
                        }
                        drew_any = true;
                    }
                    drew_any
                },
                |saved_state| unsafe { saved_state.restore(frame.context) },
            );

            unsafe {
                release(rtv);
            }
            drew_any
        }
    }

    impl Drop for DeviceResources {
        fn drop(&mut self) {
            unsafe {
                for srv in self.lut_srvs.drain(..) {
                    release(srv);
                }
                for texture in self.lut_textures.drain(..) {
                    release(texture);
                }
                release(self.constant_buffer);
                release(self.sampler);
                release(self.vertex_buffer);
                release(self.input_layout);
                release(self.pixel_shader);
                release(self.vertex_shader);
            }
        }
    }

    pub(crate) unsafe fn render_present_lut(
        overlay_swap_chain: usize,
        swap_chain_path: SwapChainPathHypothesis,
        clip_box: ClipBox,
        dirty_rects: &[DirtyRect],
        pipeline: &LutPipeline,
    ) -> super::RenderPresentLutResult {
        let renderer = RENDERER.get_or_init(|| Mutex::new(D3D11Renderer::new()));
        let Ok(mut renderer) = renderer.lock() else {
            return super::RenderPresentLutResult::default();
        };
        unsafe {
            renderer.render_present_lut(
                overlay_swap_chain,
                swap_chain_path,
                clip_box,
                dirty_rects,
                pipeline,
            )
        }
    }

    unsafe fn overlay_swap_chain_to_back_buffer(
        overlay_swap_chain: usize,
        swap_chain_path: SwapChainPathHypothesis,
    ) -> Option<ComPtr> {
        let texture = unsafe {
            dwm_lut_get_back_buffer_25h2(
                overlay_swap_chain as ComPtr,
                swap_chain_path.container_vtable_index,
                swap_chain_path.resource_vtable_index,
            )
        };
        (!texture.is_null()).then_some(texture)
    }

    unsafe fn d3d11_device_get_immediate_context(device: ComPtr, context: *mut ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr);
        let api: Api = unsafe {
            vtbl_fn(
                device,
                super::D3D11VtableIndex::DEVICE_GET_IMMEDIATE_CONTEXT,
            )
        };
        unsafe { api(device, context) };
    }

    unsafe fn d3d11_device_child_get_device(child: ComPtr, device: *mut ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr);
        let api: Api = unsafe { vtbl_fn(child, super::D3D11VtableIndex::DEVICE_CHILD_GET_DEVICE) };
        unsafe { api(child, device) };
    }

    unsafe fn d3d11_texture2d_get_desc(texture: ComPtr, desc: *mut Texture2DDesc) {
        type Api = unsafe extern "system" fn(ComPtr, *mut Texture2DDesc);
        let api: Api = unsafe { vtbl_fn(texture, super::D3D11VtableIndex::TEXTURE2D_GET_DESC) };
        unsafe { api(texture, desc) };
    }

    unsafe fn create_vertex_shader(device: ComPtr, blob: &Blob) -> Option<ComPtr> {
        type Api =
            unsafe extern "system" fn(ComPtr, *const c_void, usize, ComPtr, *mut ComPtr) -> Hresult;
        let api: Api = unsafe { vtbl_fn(device, 12) };
        let mut shader = ptr::null_mut();
        let hr = unsafe {
            api(
                device,
                blob.buffer_pointer(),
                blob.buffer_size(),
                ptr::null_mut(),
                &mut shader,
            )
        };
        (hr >= S_OK && !shader.is_null()).then_some(shader)
    }

    unsafe fn create_pixel_shader(device: ComPtr, blob: &Blob) -> Option<ComPtr> {
        type Api =
            unsafe extern "system" fn(ComPtr, *const c_void, usize, ComPtr, *mut ComPtr) -> Hresult;
        let api: Api = unsafe { vtbl_fn(device, 15) };
        let mut shader = ptr::null_mut();
        let hr = unsafe {
            api(
                device,
                blob.buffer_pointer(),
                blob.buffer_size(),
                ptr::null_mut(),
                &mut shader,
            )
        };
        (hr >= S_OK && !shader.is_null()).then_some(shader)
    }

    unsafe fn create_input_layout(device: ComPtr, blob: &Blob) -> Option<ComPtr> {
        type Api = unsafe extern "system" fn(
            ComPtr,
            *const InputElementDesc,
            u32,
            *const c_void,
            usize,
            *mut ComPtr,
        ) -> Hresult;
        const DXGI_FORMAT_R32G32_FLOAT: u32 = 16;
        const POSITION: &[u8] = b"POSITION\0";
        const TEXCOORD: &[u8] = b"TEXCOORD\0";
        let elements = [
            InputElementDesc {
                semantic_name: POSITION.as_ptr(),
                semantic_index: 0,
                format: DXGI_FORMAT_R32G32_FLOAT,
                input_slot: 0,
                aligned_byte_offset: 0,
                input_slot_class: D3D11_INPUT_PER_VERTEX_DATA,
                instance_data_step_rate: 0,
            },
            InputElementDesc {
                semantic_name: TEXCOORD.as_ptr(),
                semantic_index: 0,
                format: DXGI_FORMAT_R32G32_FLOAT,
                input_slot: 0,
                aligned_byte_offset: 8,
                input_slot_class: D3D11_INPUT_PER_VERTEX_DATA,
                instance_data_step_rate: 0,
            },
        ];
        let api: Api = unsafe { vtbl_fn(device, 11) };
        let mut layout = ptr::null_mut();
        let hr = unsafe {
            api(
                device,
                elements.as_ptr(),
                elements.len() as u32,
                blob.buffer_pointer(),
                blob.buffer_size(),
                &mut layout,
            )
        };
        (hr >= S_OK && !layout.is_null()).then_some(layout)
    }

    unsafe fn create_vertex_buffer(device: ComPtr) -> Option<ComPtr> {
        let vertices = [Vertex {
            position: [0.0, 0.0],
            texcoord: [0.0, 0.0],
        }; 4];
        let desc = BufferDesc {
            byte_width: size_of::<[Vertex; 4]>() as u32,
            usage: D3D11_USAGE_DEFAULT,
            bind_flags: D3D11_BIND_VERTEX_BUFFER,
            cpu_access_flags: 0,
            misc_flags: D3D11_RESOURCE_MISC_NONE,
            structure_byte_stride: 0,
        };
        let data = SubresourceData {
            sys_mem: vertices.as_ptr().cast(),
            sys_mem_pitch: 0,
            sys_mem_slice_pitch: 0,
        };
        unsafe { create_buffer(device, &desc, Some(&data)) }
    }

    unsafe fn create_constant_buffer(device: ComPtr) -> Option<ComPtr> {
        let desc = BufferDesc {
            byte_width: size_of::<ShaderConstantsCBuffer>() as u32,
            usage: D3D11_USAGE_DEFAULT,
            bind_flags: D3D11_BIND_CONSTANT_BUFFER,
            cpu_access_flags: 0,
            misc_flags: D3D11_RESOURCE_MISC_NONE,
            structure_byte_stride: 0,
        };
        unsafe { create_buffer(device, &desc, None) }
    }

    unsafe fn create_buffer(
        device: ComPtr,
        desc: &BufferDesc,
        data: Option<&SubresourceData>,
    ) -> Option<ComPtr> {
        type Api = unsafe extern "system" fn(
            ComPtr,
            *const BufferDesc,
            *const SubresourceData,
            *mut ComPtr,
        ) -> Hresult;
        let api: Api = unsafe { vtbl_fn(device, 3) };
        let mut buffer = ptr::null_mut();
        let data = data.map_or(ptr::null(), |data| data as *const SubresourceData);
        let hr = unsafe { api(device, desc, data, &mut buffer) };
        (hr >= S_OK && !buffer.is_null()).then_some(buffer)
    }

    unsafe fn create_sampler(device: ComPtr) -> Option<ComPtr> {
        type Api = unsafe extern "system" fn(ComPtr, *const SamplerDesc, *mut ComPtr) -> Hresult;
        let desc = SamplerDesc {
            filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
            address_u: D3D11_TEXTURE_ADDRESS_CLAMP,
            address_v: D3D11_TEXTURE_ADDRESS_CLAMP,
            address_w: D3D11_TEXTURE_ADDRESS_CLAMP,
            mip_lod_bias: 0.0,
            max_anisotropy: 1,
            comparison_func: D3D11_COMPARISON_NEVER,
            border_color: [0.0; 4],
            min_lod: 0.0,
            max_lod: D3D11_FLOAT32_MAX,
        };
        let api: Api = unsafe { vtbl_fn(device, 23) };
        let mut sampler = ptr::null_mut();
        let hr = unsafe { api(device, &desc, &mut sampler) };
        (hr >= S_OK && !sampler.is_null()).then_some(sampler)
    }

    unsafe fn create_copy_texture(
        device: ComPtr,
        width: u32,
        height: u32,
        format: BackBufferFormat,
    ) -> Option<(OwnedCom, OwnedCom)> {
        let desc = Texture2DDesc {
            width,
            height,
            mip_levels: 1,
            array_size: 1,
            format: super::dxgi_format_for_copy_texture(format),
            sample_count: 1,
            sample_quality: 0,
            usage: D3D11_USAGE_DEFAULT,
            bind_flags: D3D11_BIND_SHADER_RESOURCE,
            cpu_access_flags: 0,
            misc_flags: 0,
        };
        let texture = OwnedCom::new(unsafe { create_texture2d(device, &desc, None) }?);
        let srv = OwnedCom::new(unsafe { create_shader_resource_view(device, texture.as_ptr()) }?);
        Some((texture, srv))
    }

    unsafe fn create_lut_texture(
        device: ComPtr,
        lut: &crate::lut_pipeline::LoadedLut,
    ) -> Option<(OwnedCom, OwnedCom)> {
        let texture = &lut.texture;
        let desc = Texture3DDesc {
            width: texture.width,
            height: texture.height,
            depth: texture.depth,
            mip_levels: 1,
            format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            usage: D3D11_USAGE_DEFAULT,
            bind_flags: D3D11_BIND_SHADER_RESOURCE,
            cpu_access_flags: 0,
            misc_flags: 0,
        };
        let row_pitch = texture.width * size_of::<[f32; 4]>() as u32;
        let data = SubresourceData {
            sys_mem: texture.texels.as_ptr().cast(),
            sys_mem_pitch: row_pitch,
            sys_mem_slice_pitch: row_pitch * texture.height,
        };
        let tex = OwnedCom::new(unsafe { create_texture3d(device, &desc, &data) }?);
        let srv = OwnedCom::new(unsafe { create_shader_resource_view(device, tex.as_ptr()) }?);
        Some((tex, srv))
    }

    unsafe fn create_texture2d(
        device: ComPtr,
        desc: &Texture2DDesc,
        data: Option<&SubresourceData>,
    ) -> Option<ComPtr> {
        type Api = unsafe extern "system" fn(
            ComPtr,
            *const Texture2DDesc,
            *const SubresourceData,
            *mut ComPtr,
        ) -> Hresult;
        let api: Api = unsafe { vtbl_fn(device, 5) };
        let mut texture = ptr::null_mut();
        let data = data.map_or(ptr::null(), |data| data as *const SubresourceData);
        let hr = unsafe { api(device, desc, data, &mut texture) };
        (hr >= S_OK && !texture.is_null()).then_some(texture)
    }

    unsafe fn create_texture3d(
        device: ComPtr,
        desc: &Texture3DDesc,
        data: &SubresourceData,
    ) -> Option<ComPtr> {
        type Api = unsafe extern "system" fn(
            ComPtr,
            *const Texture3DDesc,
            *const SubresourceData,
            *mut ComPtr,
        ) -> Hresult;
        let api: Api = unsafe { vtbl_fn(device, 6) };
        let mut texture = ptr::null_mut();
        let hr = unsafe { api(device, desc, data, &mut texture) };
        (hr >= S_OK && !texture.is_null()).then_some(texture)
    }

    unsafe fn create_shader_resource_view(device: ComPtr, resource: ComPtr) -> Option<ComPtr> {
        type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const c_void, *mut ComPtr) -> Hresult;
        let api: Api = unsafe { vtbl_fn(device, 7) };
        let mut srv = ptr::null_mut();
        let hr = unsafe { api(device, resource, ptr::null(), &mut srv) };
        (hr >= S_OK && !srv.is_null()).then_some(srv)
    }

    unsafe fn d3d11_device_create_render_target_view_from_context(
        context: ComPtr,
        resource: ComPtr,
        rtv: *mut ComPtr,
    ) -> Hresult {
        let mut device = ptr::null_mut();
        unsafe { d3d11_context_get_device(context, &mut device) };
        if device.is_null() {
            return -1;
        }
        type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const c_void, *mut ComPtr) -> Hresult;
        let api: Api = unsafe {
            vtbl_fn(
                device,
                super::D3D11VtableIndex::DEVICE_CREATE_RENDER_TARGET_VIEW,
            )
        };
        let hr = unsafe { api(device, resource, ptr::null(), rtv) };
        unsafe { release(device) };
        hr
    }

    unsafe fn d3d11_context_get_device(context: ComPtr, device: *mut ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 3) };
        unsafe { api(context, device) };
    }

    unsafe fn bind_pipeline(
        context: ComPtr,
        resources: &DeviceResources,
        rtv: ComPtr,
        copy_srv: ComPtr,
        lut_srv: ComPtr,
    ) {
        let stride = size_of::<Vertex>() as u32;
        let offset = 0u32;
        let vertex_buffers = [resources.vertex_buffer];
        let srvs = [copy_srv, lut_srv];
        let samplers = [resources.sampler];
        let constant_buffers = [resources.constant_buffer];
        let mut rtvs = [ptr::null_mut(); D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT];
        rtvs[0] = rtv;
        let blend_factor = [0.0; 4];
        let empty_scissors: [DirtyRect; 0] = [];
        let viewport = Viewport {
            top_left_x: 0.0,
            top_left_y: 0.0,
            width: resources.width as f32,
            height: resources.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };

        unsafe {
            d3d11_context_ia_set_input_layout(context, resources.input_layout);
            d3d11_context_ia_set_vertex_buffers(context, vertex_buffers.as_ptr(), &stride, &offset);
            d3d11_context_ia_set_primitive_topology(
                context,
                D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
            );
            d3d11_context_vs_set_shader(context, resources.vertex_shader, ptr::null(), 0);
            d3d11_context_gs_set_shader(context, ptr::null_mut(), ptr::null(), 0);
            d3d11_context_hs_set_shader(context, ptr::null_mut(), ptr::null(), 0);
            d3d11_context_ds_set_shader(context, ptr::null_mut(), ptr::null(), 0);
            d3d11_context_ps_set_shader(context, resources.pixel_shader, ptr::null(), 0);
            d3d11_context_ps_set_shader_resources(context, srvs.as_ptr(), srvs.len() as u32);
            d3d11_context_ps_set_samplers(context, samplers.as_ptr());
            d3d11_context_vs_set_constant_buffers(context, constant_buffers.as_ptr());
            d3d11_context_ps_set_constant_buffers(context, constant_buffers.as_ptr());
            d3d11_context_om_set_render_targets(
                context,
                D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as u32,
                rtvs.as_ptr(),
            );
            d3d11_context_om_set_blend_state(
                context,
                ptr::null_mut(),
                blend_factor.as_ptr(),
                u32::MAX,
            );
            d3d11_context_om_set_depth_stencil_state(context, ptr::null_mut(), 0);
            d3d11_context_rs_set_state(context, ptr::null_mut());
            d3d11_context_rs_set_viewports(context, &viewport);
            d3d11_context_rs_set_scissor_rects(context, empty_scissors.as_ptr(), 0);
        }
    }

    struct ShaderStageState {
        shader: ComPtr,
        class_instances: [ComPtr; D3D11_SHADER_CLASS_INSTANCE_LIMIT],
        class_instance_count: u32,
    }

    impl ShaderStageState {
        fn empty() -> Self {
            Self {
                shader: ptr::null_mut(),
                class_instances: [ptr::null_mut(); D3D11_SHADER_CLASS_INSTANCE_LIMIT],
                class_instance_count: D3D11_SHADER_CLASS_INSTANCE_LIMIT as u32,
            }
        }

        fn class_instances_ptr(&self) -> *const ComPtr {
            if self.class_instance_count == 0 {
                ptr::null()
            } else {
                self.class_instances.as_ptr()
            }
        }
    }

    impl Drop for ShaderStageState {
        fn drop(&mut self) {
            unsafe {
                release(self.shader);
                for instance in self
                    .class_instances
                    .iter_mut()
                    .take(self.class_instance_count as usize)
                {
                    release(*instance);
                }
            }
        }
    }

    struct ContextState {
        input_layout: ComPtr,
        vertex_buffer: ComPtr,
        vertex_stride: u32,
        vertex_offset: u32,
        primitive_topology: u32,
        vertex_shader: ShaderStageState,
        geometry_shader: ShaderStageState,
        hull_shader: ShaderStageState,
        domain_shader: ShaderStageState,
        pixel_shader: ShaderStageState,
        pixel_srvs: [ComPtr; 2],
        pixel_sampler: ComPtr,
        vertex_constant_buffer: ComPtr,
        pixel_constant_buffer: ComPtr,
        render_targets: [ComPtr; D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT],
        depth_stencil: ComPtr,
        blend_state: ComPtr,
        blend_factor: [f32; 4],
        sample_mask: u32,
        depth_stencil_state: ComPtr,
        stencil_ref: u32,
        rasterizer_state: ComPtr,
        viewport_count: u32,
        viewports: [Viewport; 16],
        scissor_count: u32,
        scissor_rects: [DirtyRect; 16],
    }

    impl ContextState {
        unsafe fn capture(context: ComPtr) -> Self {
            let mut state = Self {
                input_layout: ptr::null_mut(),
                vertex_buffer: ptr::null_mut(),
                vertex_stride: 0,
                vertex_offset: 0,
                primitive_topology: 0,
                vertex_shader: ShaderStageState::empty(),
                geometry_shader: ShaderStageState::empty(),
                hull_shader: ShaderStageState::empty(),
                domain_shader: ShaderStageState::empty(),
                pixel_shader: ShaderStageState::empty(),
                pixel_srvs: [ptr::null_mut(), ptr::null_mut()],
                pixel_sampler: ptr::null_mut(),
                vertex_constant_buffer: ptr::null_mut(),
                pixel_constant_buffer: ptr::null_mut(),
                render_targets: [ptr::null_mut(); D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT],
                depth_stencil: ptr::null_mut(),
                blend_state: ptr::null_mut(),
                blend_factor: [0.0; 4],
                sample_mask: 0,
                depth_stencil_state: ptr::null_mut(),
                stencil_ref: 0,
                rasterizer_state: ptr::null_mut(),
                viewport_count: 16,
                viewports: [Viewport {
                    top_left_x: 0.0,
                    top_left_y: 0.0,
                    width: 0.0,
                    height: 0.0,
                    min_depth: 0.0,
                    max_depth: 0.0,
                }; 16],
                scissor_count: 16,
                scissor_rects: [DirtyRect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                }; 16],
            };

            unsafe {
                d3d11_context_ia_get_input_layout(context, &mut state.input_layout);
                d3d11_context_ia_get_vertex_buffers(
                    context,
                    &mut state.vertex_buffer,
                    &mut state.vertex_stride,
                    &mut state.vertex_offset,
                );
                d3d11_context_ia_get_primitive_topology(context, &mut state.primitive_topology);
                d3d11_context_vs_get_shader(context, &mut state.vertex_shader);
                d3d11_context_gs_get_shader(context, &mut state.geometry_shader);
                d3d11_context_hs_get_shader(context, &mut state.hull_shader);
                d3d11_context_ds_get_shader(context, &mut state.domain_shader);
                d3d11_context_ps_get_shader(context, &mut state.pixel_shader);
                d3d11_context_ps_get_shader_resources(context, state.pixel_srvs.as_mut_ptr(), 2);
                d3d11_context_ps_get_samplers(context, &mut state.pixel_sampler);
                d3d11_context_vs_get_constant_buffers(context, &mut state.vertex_constant_buffer);
                d3d11_context_ps_get_constant_buffers(context, &mut state.pixel_constant_buffer);
                d3d11_context_om_get_render_targets(
                    context,
                    state.render_targets.as_mut_ptr(),
                    &mut state.depth_stencil,
                );
                d3d11_context_om_get_blend_state(
                    context,
                    &mut state.blend_state,
                    state.blend_factor.as_mut_ptr(),
                    &mut state.sample_mask,
                );
                d3d11_context_om_get_depth_stencil_state(
                    context,
                    &mut state.depth_stencil_state,
                    &mut state.stencil_ref,
                );
                d3d11_context_rs_get_state(context, &mut state.rasterizer_state);
                d3d11_context_rs_get_viewports(
                    context,
                    &mut state.viewport_count,
                    state.viewports.as_mut_ptr(),
                );
                d3d11_context_rs_get_scissor_rects(
                    context,
                    &mut state.scissor_count,
                    state.scissor_rects.as_mut_ptr(),
                );
            }

            state
        }

        unsafe fn restore(&self, context: ComPtr) {
            unsafe {
                d3d11_context_ia_set_input_layout(context, self.input_layout);
                d3d11_context_ia_set_vertex_buffers(
                    context,
                    &self.vertex_buffer,
                    &self.vertex_stride,
                    &self.vertex_offset,
                );
                d3d11_context_ia_set_primitive_topology(context, self.primitive_topology);
                d3d11_context_vs_set_shader(
                    context,
                    self.vertex_shader.shader,
                    self.vertex_shader.class_instances_ptr(),
                    self.vertex_shader.class_instance_count,
                );
                d3d11_context_gs_set_shader(
                    context,
                    self.geometry_shader.shader,
                    self.geometry_shader.class_instances_ptr(),
                    self.geometry_shader.class_instance_count,
                );
                d3d11_context_hs_set_shader(
                    context,
                    self.hull_shader.shader,
                    self.hull_shader.class_instances_ptr(),
                    self.hull_shader.class_instance_count,
                );
                d3d11_context_ds_set_shader(
                    context,
                    self.domain_shader.shader,
                    self.domain_shader.class_instances_ptr(),
                    self.domain_shader.class_instance_count,
                );
                d3d11_context_ps_set_shader(
                    context,
                    self.pixel_shader.shader,
                    self.pixel_shader.class_instances_ptr(),
                    self.pixel_shader.class_instance_count,
                );
                d3d11_context_ps_set_shader_resources(
                    context,
                    self.pixel_srvs.as_ptr(),
                    self.pixel_srvs.len() as u32,
                );
                d3d11_context_ps_set_samplers(context, &self.pixel_sampler);
                d3d11_context_vs_set_constant_buffers(context, &self.vertex_constant_buffer);
                d3d11_context_ps_set_constant_buffers(context, &self.pixel_constant_buffer);
                d3d11_context_om_set_render_targets_with_depth(
                    context,
                    D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as u32,
                    self.render_targets.as_ptr(),
                    self.depth_stencil,
                );
                d3d11_context_om_set_blend_state(
                    context,
                    self.blend_state,
                    self.blend_factor.as_ptr(),
                    self.sample_mask,
                );
                d3d11_context_om_set_depth_stencil_state(
                    context,
                    self.depth_stencil_state,
                    self.stencil_ref,
                );
                d3d11_context_rs_set_state(context, self.rasterizer_state);
                d3d11_context_rs_set_viewports_count(
                    context,
                    self.viewport_count,
                    self.viewports.as_ptr(),
                );
                d3d11_context_rs_set_scissor_rects(
                    context,
                    self.scissor_rects.as_ptr(),
                    self.scissor_count,
                );
            }
        }
    }

    impl Drop for ContextState {
        fn drop(&mut self) {
            unsafe {
                release(self.input_layout);
                release(self.vertex_buffer);
                for srv in self.pixel_srvs {
                    release(srv);
                }
                release(self.pixel_sampler);
                release(self.vertex_constant_buffer);
                release(self.pixel_constant_buffer);
                for render_target in self.render_targets {
                    release(render_target);
                }
                release(self.depth_stencil);
                release(self.blend_state);
                release(self.depth_stencil_state);
                release(self.rasterizer_state);
            }
        }
    }

    unsafe fn unbind_pipeline(context: ComPtr) {
        let null_srvs = [ptr::null_mut(), ptr::null_mut()];
        let null_samplers = [ptr::null_mut()];
        let null_buffers = [ptr::null_mut()];
        let null_rtvs = [ptr::null_mut(); D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT];
        unsafe {
            d3d11_context_ps_set_shader_resources(context, null_srvs.as_ptr(), 2);
            d3d11_context_ps_set_samplers(context, null_samplers.as_ptr());
            d3d11_context_vs_set_constant_buffers(context, null_buffers.as_ptr());
            d3d11_context_ps_set_constant_buffers(context, null_buffers.as_ptr());
            d3d11_context_om_set_render_targets(
                context,
                D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as u32,
                null_rtvs.as_ptr(),
            );
        }
    }

    unsafe fn d3d11_context_ia_set_input_layout(context: ComPtr, layout: ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 17) };
        unsafe { api(context, layout) };
    }

    unsafe fn d3d11_context_ia_get_input_layout(context: ComPtr, layout: *mut ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 78) };
        unsafe { api(context, layout) };
    }

    unsafe fn d3d11_context_ia_set_vertex_buffers(
        context: ComPtr,
        buffers: *const ComPtr,
        stride: *const u32,
        offset: *const u32,
    ) {
        type Api =
            unsafe extern "system" fn(ComPtr, u32, u32, *const ComPtr, *const u32, *const u32);
        let api: Api = unsafe { vtbl_fn(context, 18) };
        unsafe { api(context, 0, 1, buffers, stride, offset) };
    }

    unsafe fn d3d11_context_ia_get_vertex_buffers(
        context: ComPtr,
        buffer: *mut ComPtr,
        stride: *mut u32,
        offset: *mut u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, u32, u32, *mut ComPtr, *mut u32, *mut u32);
        let api: Api = unsafe { vtbl_fn(context, 79) };
        unsafe { api(context, 0, 1, buffer, stride, offset) };
    }

    unsafe fn d3d11_context_ia_set_primitive_topology(context: ComPtr, topology: u32) {
        type Api = unsafe extern "system" fn(ComPtr, u32);
        let api: Api = unsafe { vtbl_fn(context, 24) };
        unsafe { api(context, topology) };
    }

    unsafe fn d3d11_context_ia_get_primitive_topology(context: ComPtr, topology: *mut u32) {
        type Api = unsafe extern "system" fn(ComPtr, *mut u32);
        let api: Api = unsafe { vtbl_fn(context, 83) };
        unsafe { api(context, topology) };
    }

    unsafe fn d3d11_context_vs_set_shader(
        context: ComPtr,
        shader: ComPtr,
        class_instances: *const ComPtr,
        class_instance_count: u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const ComPtr, u32);
        let api: Api = unsafe { vtbl_fn(context, 11) };
        unsafe { api(context, shader, class_instances, class_instance_count) };
    }

    unsafe fn d3d11_context_vs_get_shader(context: ComPtr, state: &mut ShaderStageState) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut ComPtr, *mut u32);
        let api: Api = unsafe { vtbl_fn(context, 76) };
        unsafe {
            api(
                context,
                &mut state.shader,
                state.class_instances.as_mut_ptr(),
                &mut state.class_instance_count,
            )
        };
    }

    unsafe fn d3d11_context_gs_set_shader(
        context: ComPtr,
        shader: ComPtr,
        class_instances: *const ComPtr,
        class_instance_count: u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const ComPtr, u32);
        let api: Api = unsafe { vtbl_fn(context, 23) };
        unsafe { api(context, shader, class_instances, class_instance_count) };
    }

    unsafe fn d3d11_context_gs_get_shader(context: ComPtr, state: &mut ShaderStageState) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut ComPtr, *mut u32);
        let api: Api = unsafe { vtbl_fn(context, 82) };
        unsafe {
            api(
                context,
                &mut state.shader,
                state.class_instances.as_mut_ptr(),
                &mut state.class_instance_count,
            )
        };
    }

    unsafe fn d3d11_context_hs_set_shader(
        context: ComPtr,
        shader: ComPtr,
        class_instances: *const ComPtr,
        class_instance_count: u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const ComPtr, u32);
        let api: Api = unsafe { vtbl_fn(context, 60) };
        unsafe { api(context, shader, class_instances, class_instance_count) };
    }

    unsafe fn d3d11_context_hs_get_shader(context: ComPtr, state: &mut ShaderStageState) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut ComPtr, *mut u32);
        let api: Api = unsafe { vtbl_fn(context, 98) };
        unsafe {
            api(
                context,
                &mut state.shader,
                state.class_instances.as_mut_ptr(),
                &mut state.class_instance_count,
            )
        };
    }

    unsafe fn d3d11_context_ds_set_shader(
        context: ComPtr,
        shader: ComPtr,
        class_instances: *const ComPtr,
        class_instance_count: u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const ComPtr, u32);
        let api: Api = unsafe { vtbl_fn(context, 64) };
        unsafe { api(context, shader, class_instances, class_instance_count) };
    }

    unsafe fn d3d11_context_ds_get_shader(context: ComPtr, state: &mut ShaderStageState) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut ComPtr, *mut u32);
        let api: Api = unsafe { vtbl_fn(context, 102) };
        unsafe {
            api(
                context,
                &mut state.shader,
                state.class_instances.as_mut_ptr(),
                &mut state.class_instance_count,
            )
        };
    }

    unsafe fn d3d11_context_ps_set_shader(
        context: ComPtr,
        shader: ComPtr,
        class_instances: *const ComPtr,
        class_instance_count: u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const ComPtr, u32);
        let api: Api = unsafe { vtbl_fn(context, 9) };
        unsafe { api(context, shader, class_instances, class_instance_count) };
    }

    unsafe fn d3d11_context_ps_get_shader(context: ComPtr, state: &mut ShaderStageState) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut ComPtr, *mut u32);
        let api: Api = unsafe { vtbl_fn(context, 74) };
        unsafe {
            api(
                context,
                &mut state.shader,
                state.class_instances.as_mut_ptr(),
                &mut state.class_instance_count,
            )
        };
    }

    unsafe fn d3d11_context_ps_set_shader_resources(
        context: ComPtr,
        srvs: *const ComPtr,
        count: u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, u32, u32, *const ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 8) };
        unsafe { api(context, 0, count, srvs) };
    }

    unsafe fn d3d11_context_ps_get_shader_resources(
        context: ComPtr,
        srvs: *mut ComPtr,
        count: u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, u32, u32, *mut ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 73) };
        unsafe { api(context, 0, count, srvs) };
    }

    unsafe fn d3d11_context_ps_set_samplers(context: ComPtr, samplers: *const ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, u32, u32, *const ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 10) };
        unsafe { api(context, 0, 1, samplers) };
    }

    unsafe fn d3d11_context_ps_get_samplers(context: ComPtr, samplers: *mut ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, u32, u32, *mut ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 75) };
        unsafe { api(context, 0, 1, samplers) };
    }

    unsafe fn d3d11_context_vs_set_constant_buffers(context: ComPtr, buffers: *const ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, u32, u32, *const ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 7) };
        unsafe { api(context, 0, 1, buffers) };
    }

    unsafe fn d3d11_context_vs_get_constant_buffers(context: ComPtr, buffers: *mut ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, u32, u32, *mut ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 72) };
        unsafe { api(context, 0, 1, buffers) };
    }

    unsafe fn d3d11_context_ps_set_constant_buffers(context: ComPtr, buffers: *const ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, u32, u32, *const ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 16) };
        unsafe { api(context, 0, 1, buffers) };
    }

    unsafe fn d3d11_context_ps_get_constant_buffers(context: ComPtr, buffers: *mut ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, u32, u32, *mut ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 77) };
        unsafe { api(context, 0, 1, buffers) };
    }

    unsafe fn d3d11_context_om_set_render_targets(
        context: ComPtr,
        count: u32,
        rtvs: *const ComPtr,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, u32, *const ComPtr, ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 33) };
        unsafe { api(context, count, rtvs, ptr::null_mut()) };
    }

    unsafe fn d3d11_context_om_set_render_targets_with_depth(
        context: ComPtr,
        count: u32,
        rtvs: *const ComPtr,
        dsv: ComPtr,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, u32, *const ComPtr, ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 33) };
        unsafe { api(context, count, rtvs, dsv) };
    }

    unsafe fn d3d11_context_om_get_render_targets(
        context: ComPtr,
        rtv: *mut ComPtr,
        dsv: *mut ComPtr,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, u32, *mut ComPtr, *mut ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 89) };
        unsafe {
            api(
                context,
                D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as u32,
                rtv,
                dsv,
            )
        };
    }

    unsafe fn d3d11_context_om_set_blend_state(
        context: ComPtr,
        blend_state: ComPtr,
        blend_factor: *const f32,
        sample_mask: u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const f32, u32);
        let api: Api = unsafe { vtbl_fn(context, 35) };
        unsafe { api(context, blend_state, blend_factor, sample_mask) };
    }

    unsafe fn d3d11_context_om_get_blend_state(
        context: ComPtr,
        blend_state: *mut ComPtr,
        blend_factor: *mut f32,
        sample_mask: *mut u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut f32, *mut u32);
        let api: Api = unsafe { vtbl_fn(context, 91) };
        unsafe { api(context, blend_state, blend_factor, sample_mask) };
    }

    unsafe fn d3d11_context_om_set_depth_stencil_state(
        context: ComPtr,
        depth_stencil_state: ComPtr,
        stencil_ref: u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, ComPtr, u32);
        let api: Api = unsafe { vtbl_fn(context, 36) };
        unsafe { api(context, depth_stencil_state, stencil_ref) };
    }

    unsafe fn d3d11_context_om_get_depth_stencil_state(
        context: ComPtr,
        depth_stencil_state: *mut ComPtr,
        stencil_ref: *mut u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut u32);
        let api: Api = unsafe { vtbl_fn(context, 92) };
        unsafe { api(context, depth_stencil_state, stencil_ref) };
    }

    unsafe fn d3d11_context_rs_set_state(context: ComPtr, rasterizer_state: ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 43) };
        unsafe { api(context, rasterizer_state) };
    }

    unsafe fn d3d11_context_rs_get_state(context: ComPtr, rasterizer_state: *mut ComPtr) {
        type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr);
        let api: Api = unsafe { vtbl_fn(context, 94) };
        unsafe { api(context, rasterizer_state) };
    }

    unsafe fn d3d11_context_rs_set_viewports(context: ComPtr, viewport: *const Viewport) {
        type Api = unsafe extern "system" fn(ComPtr, u32, *const Viewport);
        let api: Api = unsafe { vtbl_fn(context, 44) };
        unsafe { api(context, 1, viewport) };
    }

    unsafe fn d3d11_context_rs_set_viewports_count(
        context: ComPtr,
        count: u32,
        viewports: *const Viewport,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, u32, *const Viewport);
        let api: Api = unsafe { vtbl_fn(context, 44) };
        unsafe { api(context, count, viewports) };
    }

    unsafe fn d3d11_context_rs_get_viewports(
        context: ComPtr,
        count: *mut u32,
        viewports: *mut Viewport,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, *mut u32, *mut Viewport);
        let api: Api = unsafe { vtbl_fn(context, 95) };
        unsafe { api(context, count, viewports) };
    }

    unsafe fn d3d11_context_rs_set_scissor_rects(
        context: ComPtr,
        rects: *const DirtyRect,
        count: u32,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, u32, *const DirtyRect);
        let api: Api = unsafe { vtbl_fn(context, 45) };
        unsafe { api(context, count, rects) };
    }

    unsafe fn d3d11_context_rs_get_scissor_rects(
        context: ComPtr,
        count: *mut u32,
        rects: *mut DirtyRect,
    ) {
        type Api = unsafe extern "system" fn(ComPtr, *mut u32, *mut DirtyRect);
        let api: Api = unsafe { vtbl_fn(context, 96) };
        unsafe { api(context, count, rects) };
    }

    unsafe fn d3d11_context_copy_subresource_region(
        context: ComPtr,
        dst: ComPtr,
        dst_x: u32,
        dst_y: u32,
        src: ComPtr,
        src_box: *const Box3D,
    ) {
        type Api = unsafe extern "system" fn(
            ComPtr,
            ComPtr,
            u32,
            u32,
            u32,
            u32,
            ComPtr,
            u32,
            *const Box3D,
        );
        let api: Api = unsafe {
            vtbl_fn(
                context,
                super::D3D11VtableIndex::CONTEXT_COPY_SUBRESOURCE_REGION,
            )
        };
        unsafe { api(context, dst, 0, dst_x, dst_y, 0, src, 0, src_box) };
    }

    unsafe fn d3d11_context_update_subresource(
        context: ComPtr,
        resource: ComPtr,
        data: *const c_void,
        row_pitch: u32,
        depth_pitch: u32,
    ) {
        type Api =
            unsafe extern "system" fn(ComPtr, ComPtr, u32, *const c_void, *const c_void, u32, u32);
        let api: Api =
            unsafe { vtbl_fn(context, super::D3D11VtableIndex::CONTEXT_UPDATE_SUBRESOURCE) };
        unsafe {
            api(
                context,
                resource,
                0,
                ptr::null(),
                data,
                row_pitch,
                depth_pitch,
            )
        };
    }

    unsafe fn d3d11_context_draw(context: ComPtr, vertex_count: u32, start_vertex: u32) {
        type Api = unsafe extern "system" fn(ComPtr, u32, u32);
        let api: Api = unsafe { vtbl_fn(context, 13) };
        unsafe { api(context, vertex_count, start_vertex) };
    }

    unsafe fn load_compiler() -> Option<D3DCompiler> {
        let wide: Vec<u16> = "d3dcompiler_47.dll"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let module = unsafe { LoadLibraryW(wide.as_ptr()) };
        if module.is_null() {
            return None;
        }
        let proc = unsafe { GetProcAddress(module, c"D3DCompile".as_ptr().cast()) };
        let Some(proc) = proc else {
            unsafe { FreeLibrary(module) };
            return None;
        };
        Some(D3DCompiler {
            module,
            compile: unsafe {
                transmute::<unsafe extern "system" fn() -> isize, D3DCompileApi>(proc)
            },
        })
    }

    unsafe fn compile_shader(
        compile: D3DCompileApi,
        source: &str,
        entry: &str,
        profile: &str,
    ) -> Option<Blob> {
        let entry = nul_bytes(entry);
        let profile = nul_bytes(profile);
        let mut blob = ptr::null_mut();
        let mut errors = ptr::null_mut();
        let hr = unsafe {
            compile(
                source.as_ptr().cast(),
                source.len(),
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
                entry.as_ptr(),
                profile.as_ptr(),
                0,
                0,
                &mut blob,
                &mut errors,
            )
        };
        if !errors.is_null() {
            unsafe { release(errors) };
        }
        (hr >= S_OK && !blob.is_null()).then_some(Blob(blob))
    }

    fn nul_bytes(value: &str) -> Vec<u8> {
        value.bytes().chain(std::iter::once(0)).collect()
    }

    struct Blob(ComPtr);

    impl Blob {
        unsafe fn buffer_pointer(&self) -> *const c_void {
            type Api = unsafe extern "system" fn(ComPtr) -> *const c_void;
            let api: Api = unsafe { vtbl_fn(self.0, 3) };
            unsafe { api(self.0) }
        }

        unsafe fn buffer_size(&self) -> usize {
            type Api = unsafe extern "system" fn(ComPtr) -> usize;
            let api: Api = unsafe { vtbl_fn(self.0, 4) };
            unsafe { api(self.0) }
        }
    }

    impl Drop for Blob {
        fn drop(&mut self) {
            unsafe { release(self.0) };
        }
    }

    struct OwnedCom(ComPtr);

    impl OwnedCom {
        fn new(object: ComPtr) -> Self {
            Self(object)
        }

        fn as_ptr(&self) -> ComPtr {
            self.0
        }

        fn into_raw(mut self) -> ComPtr {
            let object = self.0;
            self.0 = ptr::null_mut();
            object
        }
    }

    impl Drop for OwnedCom {
        fn drop(&mut self) {
            unsafe { release(self.0) };
        }
    }

    unsafe fn vtbl_fn<T>(object: ComPtr, index: usize) -> T {
        let vtbl = unsafe { *(object as *const *const usize) };
        let slot = unsafe { *vtbl.add(index) };
        unsafe { transmute_copy_usize(slot) }
    }

    unsafe fn transmute_copy_usize<T>(value: usize) -> T {
        unsafe { std::mem::transmute_copy(&value) }
    }

    unsafe fn release(object: ComPtr) {
        if object.is_null() {
            return;
        }
        type Release = unsafe extern "system" fn(ComPtr) -> u32;
        let release: Release = unsafe { vtbl_fn(object, 2) };
        unsafe {
            release(object);
        }
    }
}

#[cfg(not(test))]
pub(crate) use imp::render_present_lut;
