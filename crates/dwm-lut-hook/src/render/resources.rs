use std::collections::BTreeMap;
use std::mem::size_of;

use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_TEXTURE3D_DESC,
    D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView,
    ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC,
};

use super::frame_plan::{BackBufferFormat, FrameInput, LutSelection, build_frame_plan};
use super::render_pass::{LutPassInput, LutRenderPass};
use super::{FrameSize, LutRenderFailure};
use crate::dwmcore::DirtyRect;
use crate::state::LutAssignment;

pub(super) struct FrameResources<'a> {
    pub(super) device: &'a ID3D11Device,
    pub(super) context: &'a ID3D11DeviceContext,
    pub(super) back_buffer: &'a ID3D11Texture2D,
    pub(super) size: FrameSize,
}

pub(super) struct DeviceResources {
    lut_pass: LutRenderPass,
    lut_views: Vec<ID3D11ShaderResourceView>,
    swap_chains: BTreeMap<usize, SwapChainResources>,
}

impl DeviceResources {
    pub(super) fn create(device: &ID3D11Device, assignments: &[LutAssignment]) -> Option<Self> {
        let lut_pass = LutRenderPass::create(device)?;
        let lut_views = assignments
            .iter()
            .map(|assignment| create_lut_view(device, assignment))
            .collect::<Option<Vec<_>>>()?;

        Some(Self {
            lut_pass,
            lut_views,
            swap_chains: BTreeMap::new(),
        })
    }

    pub(super) fn remove_swap_chain(&mut self, swap_chain: usize) {
        self.swap_chains.remove(&swap_chain);
    }

    pub(super) fn render_frame(
        &mut self,
        swap_chain: usize,
        back_buffer_id: u128,
        frame: FrameResources<'_>,
        assignments: &[LutAssignment],
        input: FrameInput,
    ) -> Result<Option<DirtyRect>, LutRenderFailure> {
        let lut_index = input.lut.lut_index;
        let assignment = assignments
            .get(lut_index)
            .ok_or(LutRenderFailure::LutIndexOutOfRange)?;
        let lut_view = self
            .lut_views
            .get(lut_index)
            .cloned()
            .ok_or(LutRenderFailure::LutIndexOutOfRange)?;
        let render_target = create_render_target_view(frame.device, frame.back_buffer)
            .ok_or(LutRenderFailure::RenderTargetViewCreateFailed)?;

        let swap_chain_resources = self
            .swap_chains
            .entry(swap_chain)
            .or_insert_with(|| SwapChainResources::new(frame.size));
        if swap_chain_resources.size != frame.size {
            *swap_chain_resources = SwapChainResources::new(frame.size);
        }

        let previous = swap_chain_resources
            .back_buffer_states
            .last_applied_lut(back_buffer_id);
        let plan = build_frame_plan(input, previous);
        let (source, source_created) = swap_chain_resources
            .sources
            .get_or_create(frame.device, frame.size, plan.lut().back_buffer_format)
            .ok_or(LutRenderFailure::SourceTextureCreateFailed)?;
        let full_source_update = [frame.size.full_rect()];
        let source_update_rects = if source_created {
            &full_source_update[..]
        } else {
            plan.source_dirty_rects()
        };

        self.lut_pass.execute(LutPassInput {
            context: frame.context,
            back_buffer: frame.back_buffer,
            render_target: &render_target,
            source_texture: &source.texture,
            source_view: &source.view,
            lut_view: &lut_view,
            assignment,
            source_update_rects,
            plan: &plan,
        });
        swap_chain_resources
            .back_buffer_states
            .record_applied_lut(back_buffer_id, plan.lut());

        Ok(plan.present_dirty_rect())
    }
}

struct SwapChainResources {
    size: FrameSize,
    sources: SourceTextures,
    back_buffer_states: BackBufferStates,
}

impl SwapChainResources {
    fn new(size: FrameSize) -> Self {
        Self {
            size,
            sources: SourceTextures::default(),
            back_buffer_states: BackBufferStates::default(),
        }
    }
}

#[derive(Default)]
struct SourceTextures {
    sdr: Option<SourceTexture>,
    hdr: Option<SourceTexture>,
}

impl SourceTextures {
    fn get_or_create(
        &mut self,
        device: &ID3D11Device,
        size: FrameSize,
        format: BackBufferFormat,
    ) -> Option<(SourceTexture, bool)> {
        let slot = match format {
            BackBufferFormat::Bgra8Unorm => &mut self.sdr,
            BackBufferFormat::Rgba16Float => &mut self.hdr,
        };
        let created = slot.is_none();
        if created {
            *slot = Some(create_source_texture(device, size, format)?);
        }
        slot.clone().map(|source| (source, created))
    }
}

#[derive(Clone)]
struct SourceTexture {
    texture: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
}

#[derive(Clone, Copy)]
struct BackBufferState {
    lut: LutSelection,
    last_seen: u64,
}

#[derive(Default)]
struct BackBufferStates {
    entries: BTreeMap<u128, BackBufferState>,
    sequence: u64,
}

impl BackBufferStates {
    const MAX_ENTRIES: usize = 16;

    fn last_applied_lut(&mut self, back_buffer: u128) -> Option<LutSelection> {
        self.sequence = self.sequence.saturating_add(1);
        let entry = self.entries.get_mut(&back_buffer)?;
        entry.last_seen = self.sequence;
        Some(entry.lut)
    }

    fn record_applied_lut(&mut self, back_buffer: u128, lut: LutSelection) {
        if !self.entries.contains_key(&back_buffer) && self.entries.len() >= Self::MAX_ENTRIES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(id, entry)| (entry.last_seen, **id))
                .map(|(id, _)| *id)
                .expect("a full back buffer state cache must contain an oldest entry");
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            back_buffer,
            BackBufferState {
                lut,
                last_seen: self.sequence,
            },
        );
    }
}

fn create_source_texture(
    device: &ID3D11Device,
    size: FrameSize,
    format: BackBufferFormat,
) -> Option<SourceTexture> {
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
    Some(SourceTexture { texture, view })
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

    fn lut(lut_index: usize) -> LutSelection {
        LutSelection {
            back_buffer_format: BackBufferFormat::Bgra8Unorm,
            lut_index,
        }
    }

    #[test]
    fn back_buffers_are_tracked_independently() {
        let first = 1;
        let second = 2;
        let mut states = BackBufferStates::default();

        assert_eq!(states.last_applied_lut(first), None);
        states.record_applied_lut(first, lut(0));

        assert_eq!(states.last_applied_lut(first), Some(lut(0)));
        assert_eq!(states.last_applied_lut(second), None);
    }

    #[test]
    fn back_buffer_states_evict_the_least_recently_seen_entry() {
        let mut states = BackBufferStates::default();
        for value in 0..BackBufferStates::MAX_ENTRIES as u128 {
            states.last_applied_lut(value);
            states.record_applied_lut(value, lut(0));
        }
        states.last_applied_lut(0);

        let new_id = BackBufferStates::MAX_ENTRIES as u128;
        states.last_applied_lut(new_id);
        states.record_applied_lut(new_id, lut(1));

        assert!(states.entries.contains_key(&0));
        assert!(!states.entries.contains_key(&1));
        assert_eq!(states.last_applied_lut(new_id), Some(lut(1)));
    }
}
