use std::collections::BTreeMap;
#[cfg(debug_assertions)]
use std::collections::BTreeSet;
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT, D3D11_TEXTURE2D_DESC, D3D11_VIEWPORT,
    ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11InputLayout, ID3D11PixelShader,
    ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D,
    ID3D11VertexShader,
};
use windows::core::{GUID, IUnknown, Interface};

use super::context_state::{ContextState, unbind_pipeline};
use super::d3d11_api::*;
use super::plan::{
    BackBufferFormat, DrawPlanSkip, DrawState, GpuDrawPlan, RenderTargetKey, RenderTargetStates,
    ResourceKey, ShaderConstants, Vertex, copy_box_for_rect, draw_rects_for_full_frame,
    dxgi_format_for_copy_texture, keeps_device_resource, prepare_gpu_draw_plan,
    present_dirty_rect_for_full_redraw, requires_full_redraw, vertices_for_rect,
    with_restored_state,
};
use super::{
    BackBufferId, PresentDrawFailReason, PresentDrawStatus, PresentLutOutcome, RenderAcquireError,
};

use crate::present::DirtyRect;
use crate::profile::SwapChainVtablePath;
use crate::state::LutAssignment;
use dwm_lut_payload::MonitorIdentity;

static RENDERER: OnceLock<Mutex<D3D11Renderer>> = OnceLock::new();

const BACK_BUFFER_ID_PRIVATE_DATA_GUID: GUID =
    GUID::from_u128(0x6ca95369_322a_4ee3_8515_fec2020a7416);

struct D3D11Renderer {
    devices: BTreeMap<ResourceKey, DeviceResources>,
    #[cfg(debug_assertions)]
    back_buffer_identity_fallbacks: BTreeSet<usize>,
}

#[derive(Clone, Copy)]
struct PresentRenderContext {
    overlay_swap_chain: usize,
    swap_chain_path: SwapChainVtablePath,
    monitor_identity: Option<MonitorIdentity>,
}

#[derive(Clone, Copy)]
struct RenderFrame<'a> {
    device: &'a ID3D11Device,
    context: &'a ID3D11DeviceContext,
    back_buffer: &'a ID3D11Texture2D,
    width: u32,
    height: u32,
}

impl D3D11Renderer {
    fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
            #[cfg(debug_assertions)]
            back_buffer_identity_fallbacks: BTreeSet::new(),
        }
    }

    unsafe fn render_present_lut(
        &mut self,
        present_context: PresentRenderContext,
        dirty_rects: &[DirtyRect],
        assignments: &[LutAssignment],
    ) -> Result<PresentLutOutcome, RenderAcquireError> {
        let back_buffer = (unsafe {
            self.overlay_swap_chain_to_back_buffer(
                present_context.overlay_swap_chain,
                present_context.swap_chain_path,
            )
        })
        .ok_or(RenderAcquireError::BackBuffer)?;

        let device =
            (unsafe { back_buffer.GetDevice() }).map_err(|_| RenderAcquireError::Device)?;
        let context =
            (unsafe { device.GetImmediateContext() }).map_err(|_| RenderAcquireError::Context)?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            back_buffer.GetDesc(&mut desc);
        }

        let back_buffer_id = self.back_buffer_id(&back_buffer);
        let draw_plan = match prepare_gpu_draw_plan(
            assignments,
            present_context.monitor_identity,
            desc.Format.0 as u32,
            desc.Width,
            desc.Height,
            dirty_rects,
        ) {
            Ok(draw_plan) => draw_plan,
            Err(skip) => {
                return Ok(outcome_from_skip(
                    skip,
                    desc.Format.0 as u32,
                    desc.Width,
                    desc.Height,
                    #[cfg(debug_assertions)]
                    Some(back_buffer_id),
                ));
            }
        };

        let frame = RenderFrame {
            device: &device,
            context: &context,
            back_buffer: &back_buffer,
            width: desc.Width,
            height: desc.Height,
        };
        Ok(self.render_with_device(
            present_context.overlay_swap_chain,
            frame,
            assignments,
            draw_plan,
            back_buffer_id,
        ))
    }

    unsafe fn overlay_swap_chain_to_back_buffer(
        &mut self,
        overlay_swap_chain: usize,
        swap_chain_path: SwapChainVtablePath,
    ) -> Option<ID3D11Texture2D> {
        let texture = unsafe {
            super::back_buffer::get_back_buffer(
                overlay_swap_chain as *mut c_void,
                swap_chain_path.container_vtable_index,
                swap_chain_path.resource_vtable_index,
            )
        }?;
        unsafe { take_owned_interface(texture) }
    }

    fn render_with_device(
        &mut self,
        overlay_swap_chain: usize,
        frame: RenderFrame<'_>,
        assignments: &[LutAssignment],
        mut draw_plan: GpuDrawPlan,
        back_buffer_id: BackBufferId,
    ) -> PresentLutOutcome {
        let device_key = frame.device.as_raw() as usize;
        let resource_key = ResourceKey {
            device: device_key,
            overlay_swap_chain,
            width: frame.width,
            height: frame.height,
        };
        let recreate = self
            .devices
            .get(&resource_key)
            .is_none_or(|resources| resources.lut_count != assignments.len());
        if recreate {
            self.devices.remove(&resource_key);
            let Some(resources) =
                DeviceResources::create(frame.device, frame.width, frame.height, assignments)
            else {
                return outcome_planned(
                    draw_plan.lut_index,
                    draw_plan.format,
                    PresentDrawFailReason::ResourcesCreateFailed,
                    #[cfg(debug_assertions)]
                    Some(back_buffer_id),
                );
            };
            self.devices
                .retain(|key, _| keeps_device_resource(*key, resource_key));
            self.devices.insert(resource_key, resources);
        }

        let Some(resources) = self.devices.get_mut(&resource_key) else {
            return outcome_planned(
                draw_plan.lut_index,
                draw_plan.format,
                PresentDrawFailReason::ResourcesMissing,
                #[cfg(debug_assertions)]
                Some(back_buffer_id),
            );
        };
        if draw_plan.lut_index >= resources.lut_srvs.len() {
            return outcome_planned(
                draw_plan.lut_index,
                draw_plan.format,
                PresentDrawFailReason::LutIndexOutOfRange,
                #[cfg(debug_assertions)]
                Some(back_buffer_id),
            );
        }

        let current_draw_state = DrawState {
            format: draw_plan.format,
            lut_index: draw_plan.lut_index,
        };
        let render_target_key = RenderTargetKey {
            overlay_swap_chain,
            back_buffer: back_buffer_id,
        };
        let previous_state = resources.draw_states.previous_state(render_target_key);
        let copy_texture_created = !resources.copy_textures.has_format(draw_plan.format);
        let needs_full_redraw = requires_full_redraw(
            previous_state,
            current_draw_state,
            recreate,
            copy_texture_created,
        );
        if needs_full_redraw {
            draw_plan.dirty_rects = draw_rects_for_full_frame(frame.width, frame.height);
        }
        if draw_plan.dirty_rects.is_empty() {
            return outcome_planned(
                draw_plan.lut_index,
                draw_plan.format,
                PresentDrawFailReason::DrawRectsEmpty,
                #[cfg(debug_assertions)]
                Some(back_buffer_id),
            );
        }
        let present_dirty_rect = present_dirty_rect_for_full_redraw(
            needs_full_redraw,
            previous_state,
            recreate,
            copy_texture_created,
            &draw_plan.dirty_rects,
        );

        let draw_result = resources.draw(frame, &draw_plan);
        match draw_result {
            Ok(()) => {
                resources
                    .draw_states
                    .record_success(render_target_key, current_draw_state);
                outcome_applied(
                    draw_plan.lut_index,
                    draw_plan.format,
                    frame.width,
                    frame.height,
                    present_dirty_rect,
                    needs_full_redraw,
                    #[cfg(debug_assertions)]
                    Some(back_buffer_id),
                )
            }
            Err(fail) => outcome_draw_failed(
                draw_plan.lut_index,
                draw_plan.format,
                frame.width,
                frame.height,
                fail,
                #[cfg(debug_assertions)]
                Some(back_buffer_id),
            ),
        }
    }

    fn clear_resources(&mut self) -> usize {
        let device_count = self.devices.len();
        self.devices.clear();
        #[cfg(debug_assertions)]
        {
            self.back_buffer_identity_fallbacks.clear();
        }
        device_count
    }

    fn back_buffer_id(&mut self, back_buffer: &ID3D11Texture2D) -> BackBufferId {
        match private_data_back_buffer_id(back_buffer) {
            Ok(id) => BackBufferId::PrivateData(id),
            Err(_reason) => {
                let identity = back_buffer
                    .cast::<IUnknown>()
                    .map(|unknown| unknown.as_raw() as usize)
                    .unwrap_or_else(|_| back_buffer.as_raw() as usize);
                #[cfg(debug_assertions)]
                if self.back_buffer_identity_fallbacks.insert(identity) {
                    debug_log!(
                        "event=back_buffer_identity_fallback reason={} back_buffer=0x{:x} identity=0x{:x}",
                        _reason,
                        back_buffer.as_raw() as usize,
                        identity
                    );
                }
                BackBufferId::ComIdentity(identity)
            }
        }
    }
}

fn private_data_back_buffer_id(back_buffer: &ID3D11Texture2D) -> Result<u128, &'static str> {
    let mut id = GUID::zeroed();
    let mut size = size_of::<GUID>() as u32;
    let get_result = unsafe {
        back_buffer.GetPrivateData(
            &BACK_BUFFER_ID_PRIVATE_DATA_GUID,
            &mut size,
            Some((&mut id as *mut GUID).cast()),
        )
    };
    if get_result.is_ok() && size == size_of::<GUID>() as u32 && id != GUID::zeroed() {
        return Ok(id.to_u128());
    }

    let id = GUID::new().map_err(|_| "guid_create_failed")?;
    unsafe {
        back_buffer.SetPrivateData(
            &BACK_BUFFER_ID_PRIVATE_DATA_GUID,
            size_of::<GUID>() as u32,
            Some((&id as *const GUID).cast()),
        )
    }
    .map_err(|_| "private_data_set_failed")?;
    Ok(id.to_u128())
}

struct DeviceResources {
    width: u32,
    height: u32,
    lut_count: usize,
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    input_layout: ID3D11InputLayout,
    vertex_buffer: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    constant_buffer: ID3D11Buffer,
    last_constants: Option<ShaderConstants>,
    copy_textures: CopyTextureResources,
    lut_srvs: Vec<ID3D11ShaderResourceView>,
    draw_states: RenderTargetStates,
}

#[derive(Default)]
struct CopyTextureResources {
    sdr: Option<CopyTextureResource>,
    hdr: Option<CopyTextureResource>,
}

struct CopyTextureResource {
    texture: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
}

impl CopyTextureResources {
    fn has_format(&self, format: BackBufferFormat) -> bool {
        match format {
            BackBufferFormat::Bgra8Unorm => self.sdr.is_some(),
            BackBufferFormat::Rgba16Float => self.hdr.is_some(),
        }
    }

    fn for_format(
        &mut self,
        device: &ID3D11Device,
        width: u32,
        height: u32,
        format: BackBufferFormat,
    ) -> Option<&CopyTextureResource> {
        let slot = match format {
            BackBufferFormat::Bgra8Unorm => &mut self.sdr,
            BackBufferFormat::Rgba16Float => &mut self.hdr,
        };
        if slot.is_none() {
            *slot = Some(CopyTextureResource::create(device, width, height, format)?);
        }
        slot.as_ref()
    }
}

impl CopyTextureResource {
    fn create(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        format: BackBufferFormat,
    ) -> Option<Self> {
        let (texture, srv) = create_copy_texture(device, width, height, format)?;
        Some(Self { texture, srv })
    }
}

impl DeviceResources {
    fn create(
        device: &ID3D11Device,
        width: u32,
        height: u32,
        assignments: &[LutAssignment],
    ) -> Option<Self> {
        let vertex_shader = create_vertex_shader(device, LUT_VERTEX_SHADER_BYTECODE)?;
        let pixel_shader = create_pixel_shader(device, LUT_PIXEL_SHADER_BYTECODE)?;
        let input_layout = create_input_layout(device, LUT_VERTEX_SHADER_BYTECODE)?;
        let vertex_buffer = create_vertex_buffer(device)?;
        let sampler = create_sampler(device)?;
        let constant_buffer = create_constant_buffer(device)?;
        let mut lut_srvs = Vec::with_capacity(assignments.len());
        for assignment in assignments {
            let (_texture, srv) = create_lut_texture(device, assignment)?;
            lut_srvs.push(srv);
        }

        Some(Self {
            width,
            height,
            lut_count: assignments.len(),
            vertex_shader,
            pixel_shader,
            input_layout,
            vertex_buffer,
            sampler,
            constant_buffer,
            last_constants: None,
            copy_textures: CopyTextureResources::default(),
            lut_srvs,
            draw_states: RenderTargetStates::default(),
        })
    }

    fn draw(
        &mut self,
        frame: RenderFrame<'_>,
        draw_plan: &GpuDrawPlan,
    ) -> Result<(), PresentDrawFailReason> {
        let Some((copy_texture, copy_srv)) = self
            .copy_textures
            .for_format(frame.device, frame.width, frame.height, draw_plan.format)
            .map(|resource| (resource.texture.clone(), resource.srv.clone()))
        else {
            return Err(PresentDrawFailReason::CopyTextureCreateFailed);
        };
        let Some(rtv) = create_render_target_view(frame.device, frame.back_buffer) else {
            return Err(PresentDrawFailReason::RenderTargetViewCreateFailed);
        };
        let bindings =
            PipelineBindings::new(self, &rtv, &copy_srv, &self.lut_srvs[draw_plan.lut_index]);
        let drawn = with_restored_state(
            || ContextState::capture(frame.context),
            || {
                if self.last_constants.as_ref() != Some(&draw_plan.constants) {
                    unsafe {
                        frame.context.UpdateSubresource(
                            &self.constant_buffer,
                            0,
                            None,
                            (&draw_plan.constants as *const ShaderConstants).cast(),
                            0,
                            0,
                        );
                    }
                    self.last_constants = Some(draw_plan.constants);
                }

                for rect in &draw_plan.dirty_rects {
                    let rect = *rect;
                    let box3d = copy_box_for_rect(rect);
                    let source_box = D3D11_BOX {
                        left: box3d.left,
                        top: box3d.top,
                        front: box3d.front,
                        right: box3d.right,
                        bottom: box3d.bottom,
                        back: box3d.back,
                    };
                    unsafe {
                        frame.context.CopySubresourceRegion(
                            &copy_texture,
                            0,
                            rect.left as u32,
                            rect.top as u32,
                            0,
                            frame.back_buffer,
                            0,
                            Some(&source_box),
                        );
                    }
                }

                if draw_plan.dirty_rects.is_empty() {
                    return false;
                }

                bind_pipeline(frame.context, self, &bindings);
                for rect in &draw_plan.dirty_rects {
                    let vertices = vertices_for_rect(*rect, frame.width, frame.height);
                    unsafe {
                        frame.context.UpdateSubresource(
                            &self.vertex_buffer,
                            0,
                            None,
                            vertices.as_ptr().cast(),
                            0,
                            0,
                        );
                        frame.context.Draw(4, 0);
                    }
                }
                unbind_pipeline(frame.context);
                true
            },
            |saved_state| saved_state.restore(frame.context),
        );
        if drawn {
            Ok(())
        } else {
            Err(PresentDrawFailReason::DrawFailed)
        }
    }
}

fn outcome_from_skip(
    skip: DrawPlanSkip,
    dxgi_format: u32,
    width: u32,
    height: u32,
    #[cfg(debug_assertions)] back_buffer_id: Option<BackBufferId>,
) -> PresentLutOutcome {
    PresentLutOutcome {
        lut_active: skip.lut_active,
        present_dirty_rect: None,
        draw: PresentDrawStatus::Skipped(skip.reason),
        dxgi_format: Some(dxgi_format),
        width: Some(width),
        height: Some(height),
        lut_index: skip.lut_index,
        #[cfg(debug_assertions)]
        back_buffer_id,
    }
}

fn outcome_planned(
    lut_index: usize,
    format: BackBufferFormat,
    fail: PresentDrawFailReason,
    #[cfg(debug_assertions)] back_buffer_id: Option<BackBufferId>,
) -> PresentLutOutcome {
    PresentLutOutcome {
        lut_active: true,
        lut_index: Some(lut_index),
        present_dirty_rect: None,
        draw: PresentDrawStatus::Failed(fail),
        dxgi_format: Some(dxgi_format_for_copy_texture(format)),
        width: None,
        height: None,
        #[cfg(debug_assertions)]
        back_buffer_id,
    }
}

fn outcome_applied(
    lut_index: usize,
    format: BackBufferFormat,
    width: u32,
    height: u32,
    present_dirty_rect: Option<DirtyRect>,
    full_redraw: bool,
    #[cfg(debug_assertions)] back_buffer_id: Option<BackBufferId>,
) -> PresentLutOutcome {
    PresentLutOutcome {
        lut_active: true,
        lut_index: Some(lut_index),
        present_dirty_rect,
        draw: PresentDrawStatus::Applied { full_redraw },
        dxgi_format: Some(dxgi_format_for_copy_texture(format)),
        width: Some(width),
        height: Some(height),
        #[cfg(debug_assertions)]
        back_buffer_id,
    }
}

fn outcome_draw_failed(
    lut_index: usize,
    format: BackBufferFormat,
    width: u32,
    height: u32,
    fail: PresentDrawFailReason,
    #[cfg(debug_assertions)] back_buffer_id: Option<BackBufferId>,
) -> PresentLutOutcome {
    PresentLutOutcome {
        lut_active: true,
        lut_index: Some(lut_index),
        present_dirty_rect: None,
        draw: PresentDrawStatus::Failed(fail),
        dxgi_format: Some(dxgi_format_for_copy_texture(format)),
        width: Some(width),
        height: Some(height),
        #[cfg(debug_assertions)]
        back_buffer_id,
    }
}

pub(crate) unsafe fn render_present_lut(
    overlay_swap_chain: usize,
    swap_chain_path: SwapChainVtablePath,
    monitor_identity: Option<MonitorIdentity>,
    dirty_rects: &[DirtyRect],
    assignments: &[LutAssignment],
) -> Result<PresentLutOutcome, RenderAcquireError> {
    let renderer = RENDERER.get_or_init(|| Mutex::new(D3D11Renderer::new()));
    let Ok(mut renderer) = renderer.lock() else {
        return Err(RenderAcquireError::Unavailable);
    };
    unsafe {
        renderer.render_present_lut(
            PresentRenderContext {
                overlay_swap_chain,
                swap_chain_path,
                monitor_identity,
            },
            dirty_rects,
            assignments,
        )
    }
}

pub(crate) fn shutdown_renderer_resources() -> usize {
    let Some(renderer) = RENDERER.get() else {
        return 0;
    };
    let Ok(mut renderer) = renderer.lock() else {
        return 0;
    };
    renderer.clear_resources()
}

struct PipelineBindings {
    vertex_buffers: [Option<ID3D11Buffer>; 1],
    srvs: [Option<ID3D11ShaderResourceView>; 2],
    samplers: [Option<ID3D11SamplerState>; 1],
    constant_buffers: [Option<ID3D11Buffer>; 1],
    render_targets:
        [Option<ID3D11RenderTargetView>; D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as usize],
}

impl PipelineBindings {
    fn new(
        resources: &DeviceResources,
        rtv: &ID3D11RenderTargetView,
        copy_srv: &ID3D11ShaderResourceView,
        lut_srv: &ID3D11ShaderResourceView,
    ) -> Self {
        let mut render_targets = std::array::from_fn(|_| None);
        render_targets[0] = Some(rtv.clone());
        Self {
            vertex_buffers: [Some(resources.vertex_buffer.clone())],
            srvs: [Some(copy_srv.clone()), Some(lut_srv.clone())],
            samplers: [Some(resources.sampler.clone())],
            constant_buffers: [Some(resources.constant_buffer.clone())],
            render_targets,
        }
    }
}

fn bind_pipeline(
    context: &ID3D11DeviceContext,
    resources: &DeviceResources,
    bindings: &PipelineBindings,
) {
    let stride = size_of::<Vertex>() as u32;
    let offset = 0u32;
    let blend_factor = [0.0; 4];
    let empty_scissors: [RECT; 0] = [];
    let viewport = D3D11_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: resources.width as f32,
        Height: resources.height as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    };

    unsafe {
        context.IASetInputLayout(&resources.input_layout);
        context.IASetVertexBuffers(
            0,
            bindings.vertex_buffers.len() as u32,
            Some(bindings.vertex_buffers.as_ptr()),
            Some(&stride),
            Some(&offset),
        );
        context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
        context.VSSetShader(&resources.vertex_shader, None);
        context.GSSetShader(None, None);
        context.HSSetShader(None, None);
        context.DSSetShader(None, None);
        context.PSSetShader(&resources.pixel_shader, None);
        context.PSSetShaderResources(0, Some(&bindings.srvs));
        context.PSSetSamplers(0, Some(&bindings.samplers));
        context.VSSetConstantBuffers(0, Some(&bindings.constant_buffers));
        context.PSSetConstantBuffers(0, Some(&bindings.constant_buffers));
        context.OMSetRenderTargets(Some(&bindings.render_targets), None);
        context.OMSetBlendState(None, Some(&blend_factor), u32::MAX);
        context.OMSetDepthStencilState(None, 0);
        context.RSSetState(None);
        context.RSSetViewports(Some(&[viewport]));
        context.RSSetScissorRects(Some(&empty_scissors));
    }
}
