use std::collections::{BTreeMap, HashSet};
use std::mem::size_of;

use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_TEXTURE3D_DESC,
    D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView,
    ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC,
};

use super::frame_plan::{
    BackBufferFormat, FrameOperation, LutSelection, PreparedFrame, build_frame_plan,
};
use super::render_pass::{
    LutPassInput, LutRenderPass, restore_back_buffer_from_clean_frame, update_clean_frame,
};
use super::{FrameProcessingError, FrameSize};
use crate::dwmcore::DirtyRect;
use crate::state::LutAssignment;
use dwm_lut_payload::MonitorIdentity;

pub(super) struct FrameResources<'a> {
    pub(super) swap_chain: usize,
    pub(super) back_buffer_id: usize,
    pub(super) monitor_identity: MonitorIdentity,
    pub(super) device: &'a ID3D11Device,
    pub(super) context: &'a ID3D11DeviceContext,
    pub(super) back_buffer: &'a ID3D11Texture2D,
    pub(super) size: FrameSize,
}

pub(super) struct DeviceResources {
    lut_pass: LutRenderPass,
    lut_views: BTreeMap<usize, ID3D11ShaderResourceView>,
    swap_chains: BTreeMap<usize, SwapChainResources>,
}

pub(super) enum FrameEffect {
    Unchanged,
    LutApplied {
        present_dirty_rect: Option<DirtyRect>,
    },
    Restored {
        generation: u64,
    },
}

impl DeviceResources {
    pub(super) fn create(device: &ID3D11Device) -> Option<Self> {
        let lut_pass = LutRenderPass::create(device)?;

        Some(Self {
            lut_pass,
            lut_views: BTreeMap::new(),
            swap_chains: BTreeMap::new(),
        })
    }

    pub(super) fn invalidate_luts(&mut self) {
        self.lut_views.clear();
        for resources in self.swap_chains.values_mut() {
            resources.back_buffer_states.require_full_application();
        }
    }

    pub(super) fn restore_targets<'a>(
        &'a self,
        active_monitors: Option<&'a HashSet<MonitorIdentity>>,
    ) -> impl Iterator<Item = usize> + 'a {
        self.swap_chains
            .iter()
            .filter(move |(_, resources)| resources.is_restore_target(active_monitors))
            .map(|(swap_chain, _)| *swap_chain)
    }

    pub(super) fn update_monitor_identity(
        &mut self,
        swap_chain: usize,
        monitor_identity: MonitorIdentity,
    ) {
        if let Some(resources) = self.swap_chains.get_mut(&swap_chain) {
            resources.monitor_identity = monitor_identity;
        }
    }

    pub(super) fn prune_unused_swap_chains(&mut self) {
        self.swap_chains
            .retain(|_, resources| !resources.back_buffer_states.is_empty());
    }

    pub(super) fn is_empty(&self) -> bool {
        self.swap_chains.is_empty()
    }

    pub(super) fn process_frame(
        &mut self,
        frame: FrameResources<'_>,
        frame_input: PreparedFrame,
        operation: FrameOperation<'_>,
    ) -> Result<FrameEffect, FrameProcessingError> {
        let (lut, assignment) = match operation {
            FrameOperation::Apply { lut, assignment } => (lut, assignment),
            FrameOperation::Restore => return self.restore_frame(frame, frame_input),
        };
        let lut_index = lut.lut_index;
        let draw_resources = (|| {
            let lut_view = match self.lut_views.get(&lut_index) {
                Some(view) => view.clone(),
                None => {
                    let view = create_lut_view(frame.device, assignment)
                        .ok_or(FrameProcessingError::LutTexture)?;
                    self.lut_views.insert(lut_index, view.clone());
                    view
                }
            };
            let render_target = create_render_target_view(frame.device, frame.back_buffer)
                .ok_or(FrameProcessingError::RenderTargetView)?;
            Ok((lut_view, render_target))
        })();

        let swap_chain_resources = self
            .swap_chains
            .entry(frame.swap_chain)
            .or_insert_with(|| SwapChainResources::new(frame.size, frame.monitor_identity));
        if swap_chain_resources.size != frame.size {
            *swap_chain_resources = SwapChainResources::new(frame.size, frame.monitor_identity);
        } else {
            swap_chain_resources.monitor_identity = frame.monitor_identity;
        }

        let reusable_lut = swap_chain_resources
            .back_buffer_states
            .reusable_lut(frame.back_buffer_id);
        let plan = build_frame_plan(frame_input, lut, reusable_lut);
        let (clean, clean_created) = swap_chain_resources
            .clean_frames
            .get_or_create(frame.device, frame.size, plan.lut().back_buffer_format)
            .ok_or(FrameProcessingError::CleanFrameCreate)?;
        let full_clean_update = [frame.size.full_rect()];
        let clean_update_rects = if clean_created {
            &full_clean_update[..]
        } else {
            plan.source_dirty_rects()
        };

        let (lut_view, render_target) = match draw_resources {
            Ok(resources) => resources,
            Err(error) => {
                update_clean_frame(
                    frame.context,
                    frame.back_buffer,
                    &clean.texture,
                    clean_update_rects,
                );
                return Err(error);
            }
        };

        self.lut_pass.execute(LutPassInput {
            context: frame.context,
            back_buffer: frame.back_buffer,
            render_target: &render_target,
            clean_texture: &clean.texture,
            clean_view: &clean.view,
            lut_view: &lut_view,
            assignment,
            clean_update_rects,
            plan: &plan,
        });
        swap_chain_resources
            .back_buffer_states
            .record_applied_lut(frame.back_buffer_id, plan.lut());

        Ok(FrameEffect::LutApplied {
            present_dirty_rect: plan.present_dirty_rect(),
        })
    }

    pub(super) fn restore_frame(
        &mut self,
        frame: FrameResources<'_>,
        frame_input: PreparedFrame,
    ) -> Result<FrameEffect, FrameProcessingError> {
        let Some(resources) = self.swap_chains.get(&frame.swap_chain) else {
            return Ok(FrameEffect::Unchanged);
        };
        if resources.size != frame.size {
            self.swap_chains.remove(&frame.swap_chain);
            return Ok(FrameEffect::Unchanged);
        }
        let resources = self
            .swap_chains
            .get_mut(&frame.swap_chain)
            .expect("validated swap chain resources must remain available");
        resources.monitor_identity = frame.monitor_identity;
        let applied_state = resources
            .back_buffer_states
            .applied_state(frame.back_buffer_id);
        let Some(applied_state) = applied_state else {
            if let Some(clean) = resources.clean_frames.get(frame_input.back_buffer_format) {
                update_clean_frame(
                    frame.context,
                    frame.back_buffer,
                    &clean.texture,
                    &frame_input.source_dirty_rects,
                );
            }
            return Ok(FrameEffect::Unchanged);
        };
        if frame_input.back_buffer_format != applied_state.selection.back_buffer_format {
            return Ok(FrameEffect::Unchanged);
        }
        let Some(clean) = resources
            .clean_frames
            .get(frame_input.back_buffer_format)
            .cloned()
        else {
            return Err(FrameProcessingError::CleanFrameMissing);
        };

        restore_back_buffer_from_clean_frame(
            frame.context,
            frame.back_buffer,
            &clean.texture,
            &frame_input.source_dirty_rects,
        );
        Ok(FrameEffect::Restored {
            generation: applied_state.generation,
        })
    }

    pub(super) fn lut_applied_back_buffer_count(&self, swap_chain: usize) -> usize {
        self.swap_chains
            .get(&swap_chain)
            .map_or(0, |resources| resources.back_buffer_states.len())
    }

    pub(super) fn record_restored(
        &mut self,
        swap_chain: usize,
        back_buffer_id: usize,
        generation: u64,
    ) -> bool {
        let Some(resources) = self.swap_chains.get_mut(&swap_chain) else {
            return false;
        };
        let Some(applied_lut) = resources
            .back_buffer_states
            .record_clean(back_buffer_id, generation)
        else {
            return false;
        };
        if !resources
            .back_buffer_states
            .has_applied_format(applied_lut.back_buffer_format)
        {
            resources
                .clean_frames
                .remove(applied_lut.back_buffer_format);
        }
        true
    }

    pub(super) fn remove_back_buffer(&mut self, back_buffer_id: usize) {
        for resources in self.swap_chains.values_mut() {
            resources.back_buffer_states.remove(back_buffer_id);
        }
    }
}

struct SwapChainResources {
    size: FrameSize,
    monitor_identity: MonitorIdentity,
    clean_frames: CleanFrames,
    back_buffer_states: BackBufferStates,
}

impl SwapChainResources {
    fn new(size: FrameSize, monitor_identity: MonitorIdentity) -> Self {
        Self {
            size,
            monitor_identity,
            clean_frames: CleanFrames::default(),
            back_buffer_states: BackBufferStates::default(),
        }
    }

    fn is_restore_target(&self, active_monitors: Option<&HashSet<MonitorIdentity>>) -> bool {
        !self.back_buffer_states.is_empty()
            && active_monitors.is_none_or(|active| active.contains(&self.monitor_identity))
    }
}

#[derive(Default)]
struct CleanFrames {
    sdr: Option<CleanFrame>,
    hdr: Option<CleanFrame>,
}

impl CleanFrames {
    fn get_or_create(
        &mut self,
        device: &ID3D11Device,
        size: FrameSize,
        format: BackBufferFormat,
    ) -> Option<(CleanFrame, bool)> {
        let slot = match format {
            BackBufferFormat::Bgra8Unorm => &mut self.sdr,
            BackBufferFormat::Rgba16Float => &mut self.hdr,
        };
        let created = slot.is_none();
        if created {
            *slot = Some(create_clean_frame(device, size, format)?);
        }
        slot.clone().map(|source| (source, created))
    }

    fn get(&self, format: BackBufferFormat) -> Option<&CleanFrame> {
        match format {
            BackBufferFormat::Bgra8Unorm => self.sdr.as_ref(),
            BackBufferFormat::Rgba16Float => self.hdr.as_ref(),
        }
    }

    fn remove(&mut self, format: BackBufferFormat) {
        match format {
            BackBufferFormat::Bgra8Unorm => self.sdr = None,
            BackBufferFormat::Rgba16Float => self.hdr = None,
        }
    }
}

#[derive(Clone)]
struct CleanFrame {
    texture: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
}

#[derive(Clone, Copy)]
struct BackBufferState {
    selection: LutSelection,
    requires_full_application: bool,
    generation: u64,
}

#[derive(Default)]
struct BackBufferStates {
    entries: BTreeMap<usize, BackBufferState>,
    next_generation: u64,
}

impl BackBufferStates {
    fn reusable_lut(&self, back_buffer: usize) -> Option<LutSelection> {
        let entry = self.entries.get(&back_buffer)?;
        (!entry.requires_full_application).then_some(entry.selection)
    }

    fn record_applied_lut(&mut self, back_buffer: usize, lut: LutSelection) {
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let generation = self.next_generation;
        self.entries.insert(
            back_buffer,
            BackBufferState {
                selection: lut,
                requires_full_application: false,
                generation,
            },
        );
    }

    fn applied_state(&self, back_buffer: usize) -> Option<BackBufferState> {
        self.entries.get(&back_buffer).copied()
    }

    fn has_applied_format(&self, format: BackBufferFormat) -> bool {
        self.entries
            .values()
            .any(|entry| entry.selection.back_buffer_format == format)
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn record_clean(&mut self, back_buffer: usize, generation: u64) -> Option<LutSelection> {
        let entry = self.entries.get(&back_buffer)?;
        if entry.generation != generation {
            return None;
        }
        self.remove(back_buffer)
    }

    fn remove(&mut self, back_buffer: usize) -> Option<LutSelection> {
        self.entries
            .remove(&back_buffer)
            .map(|entry| entry.selection)
    }

    fn require_full_application(&mut self) {
        for entry in self.entries.values_mut() {
            entry.requires_full_application = true;
        }
    }
}

fn create_clean_frame(
    device: &ID3D11Device,
    size: FrameSize,
    format: BackBufferFormat,
) -> Option<CleanFrame> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: size.width,
        Height: size.height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT(format.dxgi_format() as i32),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut texture))
            .ok()?;
    }
    let texture = texture?;
    let view = create_shader_resource_view(device, &texture)?;
    Some(CleanFrame { texture, view })
}

fn create_render_target_view(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
) -> Option<ID3D11RenderTargetView> {
    let mut view = None;
    unsafe {
        device
            .CreateRenderTargetView(texture, None, Some(&mut view))
            .ok()?;
    }
    view
}

fn create_lut_view(
    device: &ID3D11Device,
    assignment: &LutAssignment,
) -> Option<ID3D11ShaderResourceView> {
    let texture = &assignment.texture;
    let desc = D3D11_TEXTURE3D_DESC {
        Width: texture.width,
        Height: texture.height,
        Depth: texture.depth,
        MipLevels: 1,
        Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let row_pitch = texture.width * size_of::<[f32; 4]>() as u32;
    let data = D3D11_SUBRESOURCE_DATA {
        pSysMem: texture.texels.as_ptr().cast(),
        SysMemPitch: row_pitch,
        SysMemSlicePitch: row_pitch * texture.height,
    };
    let mut texture3d = None;
    unsafe {
        device
            .CreateTexture3D(&desc, Some(&data), Some(&mut texture3d))
            .ok()?;
    }
    create_shader_resource_view(device, &texture3d?)
}

fn create_shader_resource_view<P0>(
    device: &ID3D11Device,
    resource: P0,
) -> Option<ID3D11ShaderResourceView>
where
    P0: windows::core::Param<windows::Win32::Graphics::Direct3D11::ID3D11Resource>,
{
    let mut view = None;
    unsafe {
        device
            .CreateShaderResourceView(resource, None, Some(&mut view))
            .ok()?;
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use dwm_lut_payload::AdapterLuid;

    fn monitor(target_id: u32) -> MonitorIdentity {
        MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 1,
                low_part: 2,
            },
            target_id,
        }
    }

    fn lut(lut_index: usize) -> LutSelection {
        LutSelection {
            back_buffer_format: BackBufferFormat::Bgra8Unorm,
            lut_index,
        }
    }

    #[test]
    fn restore_target_follows_resource_monitor_and_applied_state() {
        let first_monitor = monitor(1);
        let second_monitor = monitor(2);
        let mut resources = SwapChainResources::new(
            FrameSize {
                width: 1920,
                height: 1080,
            },
            first_monitor,
        );

        assert!(!resources.is_restore_target(None));

        resources.back_buffer_states.record_applied_lut(1, lut(0));
        assert!(resources.is_restore_target(None));
        assert!(resources.is_restore_target(Some(&HashSet::from([first_monitor]))));
        assert!(!resources.is_restore_target(Some(&HashSet::from([second_monitor]))));

        resources.monitor_identity = second_monitor;
        assert!(!resources.is_restore_target(Some(&HashSet::from([first_monitor]))));
        assert!(resources.is_restore_target(Some(&HashSet::from([second_monitor]))));
    }

    #[test]
    fn back_buffers_are_tracked_independently() {
        let first = 1;
        let second = 2;
        let mut states = BackBufferStates::default();

        assert_eq!(states.reusable_lut(first), None);
        states.record_applied_lut(first, lut(0));

        assert_eq!(states.reusable_lut(first), Some(lut(0)));
        assert_eq!(states.reusable_lut(second), None);
    }

    #[test]
    fn full_application_does_not_forget_that_a_buffer_still_contains_a_lut() {
        let mut states = BackBufferStates::default();
        states.record_applied_lut(1, lut(0));

        states.require_full_application();

        assert_eq!(states.reusable_lut(1), None);
        assert_eq!(
            states.applied_state(1).map(|state| state.selection),
            Some(lut(0))
        );
        assert_eq!(states.len(), 1);
    }

    #[test]
    fn clean_buffer_is_not_a_restore_target_and_requires_full_lut_application() {
        let mut states = BackBufferStates::default();
        states.record_applied_lut(1, lut(0));
        let generation = states.applied_state(1).unwrap().generation;

        assert_eq!(states.record_clean(1, generation), Some(lut(0)));

        assert_eq!(states.reusable_lut(1), None);
        assert!(states.applied_state(1).is_none());
        assert_eq!(states.len(), 0);
    }

    #[test]
    fn applied_lut_presence_tracks_cleaned_buffers() {
        let mut states = BackBufferStates::default();
        states.record_applied_lut(1, lut(0));
        states.record_applied_lut(2, lut(0));
        let first_generation = states.applied_state(1).unwrap().generation;
        let second_generation = states.applied_state(2).unwrap().generation;

        assert_eq!(states.len(), 2);
        states.record_clean(1, first_generation);
        assert_eq!(states.len(), 1);
        states.record_clean(2, second_generation);
        assert_eq!(states.len(), 0);
    }

    #[test]
    fn applied_buffers_are_retained_until_they_are_cleaned_or_destroyed() {
        let mut states = BackBufferStates::default();
        for value in 1..=32 {
            states.record_applied_lut(value, lut(0));
        }

        assert_eq!(states.len(), 32);
        assert_eq!(
            states.applied_state(1).map(|state| state.selection),
            Some(lut(0))
        );
        assert_eq!(
            states.applied_state(32).map(|state| state.selection),
            Some(lut(0))
        );
    }

    #[test]
    fn stale_restore_does_not_clear_a_newer_lut_application() {
        let mut states = BackBufferStates::default();
        states.record_applied_lut(1, lut(0));
        let stale_generation = states.applied_state(1).unwrap().generation;
        states.record_applied_lut(1, lut(1));

        assert_eq!(states.record_clean(1, stale_generation), None);
        assert_eq!(
            states.applied_state(1).map(|state| state.selection),
            Some(lut(1))
        );
    }
}
