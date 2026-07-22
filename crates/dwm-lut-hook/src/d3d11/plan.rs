use std::collections::BTreeMap;

use crate::present::DirtyRect;
use crate::state::{LutAssignment, find_assignment};
use dwm_lut_payload::{ColorMode, MonitorIdentity};

use super::{BackBufferId, DrawPlanSkipReason};

pub(crate) const DXGI_FORMAT_R16G16B16A16_FLOAT: u32 = 10;
pub(crate) const DXGI_FORMAT_B8G8R8A8_UNORM: u32 = 87;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackBufferFormat {
    Bgra8Unorm,
    Rgba16Float,
}

impl BackBufferFormat {
    pub(crate) const fn from_dxgi_format(format: u32) -> Option<Self> {
        match format {
            DXGI_FORMAT_B8G8R8A8_UNORM => Some(Self::Bgra8Unorm),
            DXGI_FORMAT_R16G16B16A16_FLOAT => Some(Self::Rgba16Float),
            _ => None,
        }
    }

    pub(crate) const fn is_hdr(self) -> bool {
        matches!(self, Self::Rgba16Float)
    }

    pub(crate) const fn color_mode(self) -> ColorMode {
        match self {
            Self::Bgra8Unorm => ColorMode::Sdr,
            Self::Rgba16Float => ColorMode::Hdr,
        }
    }
}

const fn extend_domain(domain: [f32; 3]) -> [f32; 4] {
    [domain[0], domain[1], domain[2], 0.0]
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Vertex {
    pub(super) position: [f32; 2],
    pub(super) texcoord: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShaderConstants {
    pub lut_size: u32,
    pub hdr: u32,
    pub padding: [f32; 2],
    pub domain_min: [f32; 4],
    pub domain_max: [f32; 4],
}

impl ShaderConstants {
    fn for_assignment(assignment: &LutAssignment, format: BackBufferFormat) -> Self {
        Self {
            lut_size: assignment.metadata.size,
            hdr: u32::from(format.is_hdr()),
            padding: [0.0, 0.0],
            domain_min: extend_domain(assignment.metadata.domain_min),
            domain_max: extend_domain(assignment.metadata.domain_max),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Box3D {
    pub(super) left: u32,
    pub(super) top: u32,
    pub(super) front: u32,
    pub(super) right: u32,
    pub(super) bottom: u32,
    pub(super) back: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct GpuDrawPlan {
    pub(super) format: BackBufferFormat,
    pub(super) lut_index: usize,
    pub(super) constants: ShaderConstants,
    pub(super) dirty_rects: Vec<DirtyRect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DrawPlanSkip {
    pub(super) reason: DrawPlanSkipReason,
    pub(super) lut_active: bool,
    pub(super) lut_index: Option<usize>,
}

impl DrawPlanSkip {
    const fn inactive(reason: DrawPlanSkipReason) -> Self {
        Self {
            reason,
            lut_active: false,
            lut_index: None,
        }
    }

    const fn active(reason: DrawPlanSkipReason, lut_index: usize) -> Self {
        Self {
            reason,
            lut_active: true,
            lut_index: Some(lut_index),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DrawState {
    pub(super) format: BackBufferFormat,
    pub(super) lut_index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RenderTargetState {
    Bootstrapping,
    Stable(DrawState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RenderTargetKey {
    pub(super) overlay_swap_chain: usize,
    pub(super) back_buffer: BackBufferId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RenderTargetStateEntry {
    state: RenderTargetState,
    last_seen: u64,
}

#[derive(Default)]
pub(super) struct RenderTargetStates {
    entries: BTreeMap<RenderTargetKey, RenderTargetStateEntry>,
    sequence: u64,
}

impl RenderTargetStates {
    const MAX_ENTRIES: usize = 16;

    pub(super) fn previous_state(&mut self, key: RenderTargetKey) -> RenderTargetState {
        self.sequence = self.sequence.saturating_add(1);
        let Some(entry) = self.entries.get_mut(&key) else {
            return RenderTargetState::Bootstrapping;
        };
        entry.last_seen = self.sequence;
        entry.state
    }

    pub(super) fn record_success(&mut self, key: RenderTargetKey, state: DrawState) {
        if !self.entries.contains_key(&key) && self.entries.len() >= Self::MAX_ENTRIES {
            let oldest_key = self
                .entries
                .iter()
                .min_by_key(|(key, entry)| (entry.last_seen, **key))
                .map(|(key, _)| *key)
                .expect("a full render target state cache must have an oldest entry");
            self.entries.remove(&oldest_key);
        }
        self.entries.insert(
            key,
            RenderTargetStateEntry {
                state: RenderTargetState::Stable(state),
                last_seen: self.sequence,
            },
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ResourceKey {
    pub(super) device: usize,
    pub(super) overlay_swap_chain: usize,
    pub(super) width: u32,
    pub(super) height: u32,
}

pub(super) const fn keeps_device_resource(key: ResourceKey, current: ResourceKey) -> bool {
    key.device != current.device
        || key.overlay_swap_chain != current.overlay_swap_chain
        || (key.width == current.width && key.height == current.height)
}

pub(super) fn prepare_gpu_draw_plan(
    assignments: &[LutAssignment],
    monitor_identity: Option<MonitorIdentity>,
    dxgi_format: u32,
    width: u32,
    height: u32,
    dirty_rects: &[DirtyRect],
) -> Result<GpuDrawPlan, DrawPlanSkip> {
    let format = BackBufferFormat::from_dxgi_format(dxgi_format);
    let matched = monitor_identity.zip(format).and_then(|(identity, format)| {
        find_assignment(assignments, identity, format.color_mode())
            .map(|(lut_index, lut)| (format, lut_index, lut))
    });

    if width == 0 || height == 0 {
        return Err(match matched {
            Some((_, lut_index, _)) => {
                DrawPlanSkip::active(DrawPlanSkipReason::ZeroSize, lut_index)
            }
            None => DrawPlanSkip::inactive(DrawPlanSkipReason::ZeroSize),
        });
    }
    if monitor_identity.is_none() {
        return Err(DrawPlanSkip::inactive(
            DrawPlanSkipReason::MissingMonitorIdentity,
        ));
    }
    if format.is_none() {
        return Err(DrawPlanSkip::inactive(
            DrawPlanSkipReason::UnsupportedFormat,
        ));
    }
    let Some((format, lut_index, lut)) = matched else {
        return Err(DrawPlanSkip::inactive(
            DrawPlanSkipReason::MissingAssignment,
        ));
    };
    let dirty_rects = draw_rects_for_frame(dirty_rects, width, height);
    if dirty_rects.is_empty() {
        return Err(DrawPlanSkip::active(
            DrawPlanSkipReason::EmptyDirtyRects,
            lut_index,
        ));
    }
    Ok(GpuDrawPlan {
        format,
        lut_index,
        constants: ShaderConstants::for_assignment(lut, format),
        dirty_rects,
    })
}

pub(super) const fn dxgi_format_for_copy_texture(format: BackBufferFormat) -> u32 {
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
pub(super) fn draw_rects_for_full_frame(width: u32, height: u32) -> Vec<DirtyRect> {
    draw_rects_for_frame(&[], width, height)
}

pub(super) fn with_restored_state<State, Capture, Draw, Restore>(
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

pub(super) fn copy_box_for_rect(rect: DirtyRect) -> Box3D {
    Box3D {
        left: rect.left as u32,
        top: rect.top as u32,
        front: 0,
        right: rect.right as u32,
        bottom: rect.bottom as u32,
        back: 1,
    }
}

pub(super) fn vertices_for_rect(rect: DirtyRect, width: u32, height: u32) -> [Vertex; 4] {
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

pub(super) fn requires_full_redraw(
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

pub(super) fn present_dirty_rect_for_full_redraw(
    needs_full_redraw: bool,
    previous_state: RenderTargetState,
    resources_recreated: bool,
    copy_texture_created: bool,
    dirty_rects: &[DirtyRect],
) -> Option<DirtyRect> {
    let should_expand = needs_full_redraw
        && (resources_recreated
            || copy_texture_created
            || matches!(previous_state, RenderTargetState::Stable(_)));
    should_expand.then(|| bounding_rect(dirty_rects).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{LutAssignment, LutMetadata, ShaderTexture3D};
    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, MonitorIdentity, MonitorTarget, PayloadAssignment,
        PayloadLut,
    };
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

    fn test_assignments() -> Vec<LutAssignment> {
        fn lut_assignment(color_mode: ColorMode) -> LutAssignment {
            LutAssignment {
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

        vec![
            lut_assignment(ColorMode::Sdr),
            lut_assignment(ColorMode::Hdr),
        ]
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
        let assignments = test_assignments();
        let dirty_rects = [DirtyRect {
            left: 0,
            top: 0,
            right: 64,
            bottom: 64,
        }];

        assert!(
            prepare_gpu_draw_plan(
                &assignments,
                Some(test_identity()),
                DXGI_FORMAT_B8G8R8A8_UNORM,
                1920,
                1080,
                &dirty_rects,
            )
            .is_ok()
        );
        let hdr_plan = prepare_gpu_draw_plan(
            &assignments,
            Some(test_identity()),
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            1920,
            1080,
            &dirty_rects,
        )
        .expect("HDR frames should render when an HDR LUT matches");
        assert_eq!(hdr_plan.format, BackBufferFormat::Rgba16Float);
        assert_eq!(hdr_plan.lut_index, 1);
        assert_eq!(hdr_plan.constants.hdr, 1);
    }

    #[test]
    fn gpu_draw_plan_skip_preserves_only_matched_assignments() {
        let assignments = test_assignments();
        let zero_size_skip = prepare_gpu_draw_plan(
            &assignments,
            Some(test_identity()),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            0,
            1080,
            &[],
        )
        .expect_err("zero-sized frames should skip drawing");

        assert_eq!(zero_size_skip.reason, DrawPlanSkipReason::ZeroSize);
        assert!(zero_size_skip.lut_active);
        assert_eq!(zero_size_skip.lut_index, Some(0));

        let mut unmatched_identity = test_identity();
        unmatched_identity.target_id = unmatched_identity.target_id.saturating_add(1);
        let missing_assignment_skip = prepare_gpu_draw_plan(
            &assignments,
            Some(unmatched_identity),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            1920,
            1080,
            &[],
        )
        .expect_err("an unmatched monitor should skip drawing");

        assert_eq!(
            missing_assignment_skip.reason,
            DrawPlanSkipReason::MissingAssignment
        );
        assert!(!missing_assignment_skip.lut_active);
        assert_eq!(missing_assignment_skip.lut_index, None);
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
        let assignments = test_assignments();
        let plan = prepare_gpu_draw_plan(
            &assignments,
            Some(test_identity()),
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
                false,
                false,
                &full_frame_rects,
            ),
            None
        );
    }

    #[test]
    fn bootstrapping_full_redraw_expands_present_dirty_rect_when_resources_recreated() {
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
                true,
                false,
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
    fn bootstrapping_full_redraw_expands_present_dirty_rect_when_copy_texture_created() {
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
                false,
                true,
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
                false,
                false,
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
                false,
                false,
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
    fn render_target_states_stabilize_back_buffers_independently() {
        let state = DrawState {
            format: BackBufferFormat::Bgra8Unorm,
            lut_index: 0,
        };
        let first = RenderTargetKey {
            overlay_swap_chain: 0x1000,
            back_buffer: BackBufferId::PrivateData(0x2000),
        };
        let other_buffer = RenderTargetKey {
            overlay_swap_chain: 0x1000,
            back_buffer: BackBufferId::PrivateData(0x3000),
        };
        let fallback_buffer = RenderTargetKey {
            overlay_swap_chain: 0x1000,
            back_buffer: BackBufferId::ComIdentity(0x4000),
        };
        let mut states = RenderTargetStates::default();

        assert_eq!(
            states.previous_state(first),
            RenderTargetState::Bootstrapping
        );
        states.record_success(first, state);
        assert_eq!(
            states.previous_state(first),
            RenderTargetState::Stable(state)
        );
        assert_eq!(
            states.previous_state(other_buffer),
            RenderTargetState::Bootstrapping
        );
        states.record_success(other_buffer, state);
        assert_eq!(
            states.previous_state(other_buffer),
            RenderTargetState::Stable(state)
        );
        assert_eq!(
            states.previous_state(fallback_buffer),
            RenderTargetState::Bootstrapping
        );
        states.record_success(fallback_buffer, state);
        assert_eq!(
            states.previous_state(fallback_buffer),
            RenderTargetState::Stable(state)
        );
    }

    #[test]
    fn render_target_states_evict_the_least_recently_seen_buffer() {
        let state = DrawState {
            format: BackBufferFormat::Bgra8Unorm,
            lut_index: 0,
        };
        let key = |id| RenderTargetKey {
            overlay_swap_chain: 0x1000,
            back_buffer: BackBufferId::PrivateData(id),
        };
        let mut states = RenderTargetStates::default();
        for id in 0..RenderTargetStates::MAX_ENTRIES as u128 {
            let key = key(id);
            assert_eq!(states.previous_state(key), RenderTargetState::Bootstrapping);
            states.record_success(key, state);
        }

        assert_eq!(
            states.previous_state(key(0)),
            RenderTargetState::Stable(state)
        );
        let new_key = key(RenderTargetStates::MAX_ENTRIES as u128);
        assert_eq!(
            states.previous_state(new_key),
            RenderTargetState::Bootstrapping
        );
        states.record_success(new_key, state);

        assert_eq!(states.entries.len(), RenderTargetStates::MAX_ENTRIES);
        assert!(states.entries.contains_key(&key(0)));
        assert!(!states.entries.contains_key(&key(1)));
        assert!(states.entries.contains_key(&new_key));
    }

    #[test]
    fn device_resource_cache_drops_other_sizes_for_same_swap_chain() {
        let current = ResourceKey {
            device: 0x10,
            overlay_swap_chain: 0x20,
            width: 1280,
            height: 720,
        };
        let same_size = ResourceKey {
            device: 0x10,
            overlay_swap_chain: 0x20,
            width: 1280,
            height: 720,
        };
        let old_size = ResourceKey {
            device: 0x10,
            overlay_swap_chain: 0x20,
            width: 1920,
            height: 1080,
        };
        let other_swap_chain = ResourceKey {
            device: 0x10,
            overlay_swap_chain: 0x21,
            width: 1920,
            height: 1080,
        };
        let other_device = ResourceKey {
            device: 0x11,
            overlay_swap_chain: 0x20,
            width: 1920,
            height: 1080,
        };

        assert!(keeps_device_resource(same_size, current));
        assert!(!keeps_device_resource(old_size, current));
        assert!(keeps_device_resource(other_swap_chain, current));
        assert!(keeps_device_resource(other_device, current));
    }

    #[test]
    fn gpu_draw_plan_ignores_dirty_rects_outside_the_frame() {
        let assignments = test_assignments();
        let plan = prepare_gpu_draw_plan(
            &assignments,
            Some(test_identity()),
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
        let skip = prepare_gpu_draw_plan(
            &assignments,
            Some(test_identity()),
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
        .expect_err("dirty rects outside the frame should skip drawing");

        assert_eq!(skip.reason, DrawPlanSkipReason::EmptyDirtyRects);
        assert!(skip.lut_active);
        assert_eq!(skip.lut_index, Some(0));
    }

    #[test]
    fn gpu_draw_plan_selects_lut_by_runtime_monitor_identity() {
        fn lut_assignment(
            _label: &str,
            identity: MonitorIdentity,
            color_mode: ColorMode,
        ) -> LutAssignment {
            LutAssignment {
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
        let assignments = vec![
            lut_assignment("PRIMARY", primary, ColorMode::Sdr),
            lut_assignment("RIGHT", right, ColorMode::Sdr),
        ];

        let plan = prepare_gpu_draw_plan(
            &assignments,
            Some(right),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            1920,
            1080,
            &[],
        )
        .expect("runtime monitor identity should select a plan");

        assert_eq!(plan.lut_index, 1);
        assert!(
            prepare_gpu_draw_plan(
                &assignments,
                None,
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

    #[test]
    fn shader_constants_match_hlsl_layout() {
        use std::mem::size_of;
        use std::ptr::addr_of;

        let constants = ShaderConstants {
            lut_size: 33,
            hdr: 1,
            padding: [0.0, 0.0],
            domain_min: [-1.0, 0.0, 0.0, 0.0],
            domain_max: [1.0, 1.0, 1.0, 0.0],
        };

        let base = (&constants as *const ShaderConstants) as usize;
        assert_eq!(size_of::<ShaderConstants>(), 48);
        assert_eq!(addr_of!(constants.lut_size) as usize - base, 0);
        assert_eq!(addr_of!(constants.hdr) as usize - base, 4);
        assert_eq!(addr_of!(constants.padding) as usize - base, 8);
        assert_eq!(addr_of!(constants.domain_min) as usize - base, 16);
        assert_eq!(addr_of!(constants.domain_max) as usize - base, 32);
        assert_eq!(constants.padding, [0.0, 0.0]);
    }

    fn identity_cube() -> PayloadLut {
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

    fn payload(
        assignments: impl IntoIterator<Item = (MonitorIdentity, ColorMode, PayloadLut)>,
    ) -> HookPayload {
        HookPayload {
            assignments: assignments
                .into_iter()
                .map(|(identity, color_mode, lut)| PayloadAssignment {
                    target: MonitorTarget {
                        identity,
                        color_mode,
                    },
                    lut,
                })
                .collect(),
        }
    }

    #[test]
    fn gpu_draw_plan_builds_sdr_shader_constants() {
        let identity = test_identity();
        let assignments = crate::state::assignments_from_payload(&payload([(
            identity,
            ColorMode::Sdr,
            identity_cube(),
        )]));
        let plan = prepare_gpu_draw_plan(
            &assignments,
            Some(identity),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            1920,
            1080,
            &[],
        )
        .expect("SDR plan should exist");

        assert_eq!(plan.format, BackBufferFormat::Bgra8Unorm);
        assert_eq!(plan.constants.lut_size, 2);
        assert_eq!(plan.constants.hdr, 0);
        assert_eq!(plan.constants.domain_min, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(plan.constants.domain_max, [1.0, 1.0, 1.0, 0.0]);
        assert_eq!(plan.constants.padding, [0.0, 0.0]);
    }

    #[test]
    fn gpu_draw_plan_builds_hdr_shader_constants() {
        let identity = test_identity();
        let assignments = crate::state::assignments_from_payload(&payload([(
            identity,
            ColorMode::Hdr,
            identity_cube(),
        )]));
        let plan = prepare_gpu_draw_plan(
            &assignments,
            Some(identity),
            DXGI_FORMAT_R16G16B16A16_FLOAT,
            1920,
            1080,
            &[],
        )
        .expect("HDR plan should exist");

        assert_eq!(plan.format, BackBufferFormat::Rgba16Float);
        assert_eq!(plan.constants.hdr, 1);
    }

    #[test]
    fn gpu_draw_plan_preserves_non_default_domain_for_shader_constants() {
        let identity = test_identity();
        let mut lut = identity_cube();
        lut.domain_min = [-1.0, 0.0, 0.0];
        let assignments =
            crate::state::assignments_from_payload(&payload([(identity, ColorMode::Sdr, lut)]));
        let plan = prepare_gpu_draw_plan(
            &assignments,
            Some(identity),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            1920,
            1080,
            &[],
        )
        .expect("plan should exist");

        assert_eq!(plan.constants.domain_min, [-1.0, 0.0, 0.0, 0.0]);
        assert_eq!(plan.constants.domain_max, [1.0, 1.0, 1.0, 0.0]);
    }

    fn assert_vec2_near(actual: [f32; 2], expected: [f32; 2]) {
        const EPSILON: f32 = 0.000_001;
        assert!((actual[0] - expected[0]).abs() <= EPSILON);
        assert!((actual[1] - expected[1]).abs() <= EPSILON);
    }
}
