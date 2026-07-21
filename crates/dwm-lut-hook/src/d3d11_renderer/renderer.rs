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

use super::Vertex;
use super::context_state::{ContextState, unbind_pipeline};
use super::d3d11_api::*;

use crate::lut_pipeline::{BackBufferFormat, DirtyRect, LutPipeline, ShaderConstantsCBuffer};
use crate::profile::SwapChainPathHypothesis;
use dwm_lut_payload::MonitorIdentity;

#[cfg(debug_assertions)]
const DIAGNOSTIC_SAMPLE_INTERVAL: u64 = 600;
const BACK_BUFFER_ID_PRIVATE_DATA_GUID: GUID =
    GUID::from_u128(0x6ca95369_322a_4ee3_8515_fec2020a7416);

static RENDERER: OnceLock<Mutex<D3D11Renderer>> = OnceLock::new();

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameDiagnosticKey {
    dxgi_format: u32,
    target_id: Option<u32>,
    width: u32,
    height: u32,
}

#[cfg(debug_assertions)]
struct PerOverlayDiagnosticLogLimiter<K> {
    last_keys: BTreeMap<usize, K>,
    counts: BTreeMap<usize, u64>,
}

#[cfg(debug_assertions)]
impl<K> Default for PerOverlayDiagnosticLogLimiter<K> {
    fn default() -> Self {
        Self {
            last_keys: BTreeMap::new(),
            counts: BTreeMap::new(),
        }
    }
}

#[cfg(debug_assertions)]
impl<K: Copy + PartialEq> PerOverlayDiagnosticLogLimiter<K> {
    fn should_log(&mut self, overlay_swap_chain: usize, key: K) -> bool {
        let count = self.counts.entry(overlay_swap_chain).or_insert(0);
        *count = count.saturating_add(1);

        let changed = self
            .last_keys
            .get(&overlay_swap_chain)
            .is_none_or(|last_key| *last_key != key);
        if changed {
            self.last_keys.insert(overlay_swap_chain, key);
        }

        *count == 1 || changed || (*count).is_multiple_of(DIAGNOSTIC_SAMPLE_INTERVAL)
    }
}

struct D3D11Renderer {
    devices: BTreeMap<super::ResourceKey, DeviceResources>,
    #[cfg(debug_assertions)]
    frame_diagnostics: PerOverlayDiagnosticLogLimiter<FrameDiagnosticKey>,
    #[cfg(debug_assertions)]
    back_buffer_identity_fallbacks: BTreeSet<usize>,
}

#[derive(Clone, Copy)]
struct PresentRenderContext {
    overlay_swap_chain: usize,
    swap_chain_path: SwapChainPathHypothesis,
    monitor_identity: Option<MonitorIdentity>,
    hardware_protected: bool,
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
            frame_diagnostics: PerOverlayDiagnosticLogLimiter::default(),
            #[cfg(debug_assertions)]
            back_buffer_identity_fallbacks: BTreeSet::new(),
        }
    }

    unsafe fn render_present_lut(
        &mut self,
        present_context: PresentRenderContext,
        dirty_rects: &[DirtyRect],
        pipeline: &LutPipeline,
    ) -> super::RenderPresentLutResult {
        let Some(back_buffer) = (unsafe {
            self.overlay_swap_chain_to_back_buffer(
                present_context.overlay_swap_chain,
                present_context.swap_chain_path,
            )
        }) else {
            debug_log!(
                "event=back_buffer_get_failed overlay_swap_chain=0x{:x}",
                present_context.overlay_swap_chain
            );
            return super::RenderPresentLutResult::default();
        };

        let Ok(device) = (unsafe { back_buffer.GetDevice() }) else {
            debug_log!(
                "event=renderer_early_return reason=device_get_failed overlay_swap_chain=0x{:x} back_buffer=0x{:x}",
                present_context.overlay_swap_chain,
                back_buffer.as_raw() as usize
            );
            return super::RenderPresentLutResult::default();
        };

        let Ok(context) = (unsafe { device.GetImmediateContext() }) else {
            debug_log!(
                "event=renderer_early_return reason=context_get_failed overlay_swap_chain=0x{:x} back_buffer=0x{:x} device=0x{:x}",
                present_context.overlay_swap_chain,
                back_buffer.as_raw() as usize,
                device.as_raw() as usize
            );
            return super::RenderPresentLutResult::default();
        };

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            back_buffer.GetDesc(&mut desc);
        }
        #[cfg(debug_assertions)]
        let frame_adapter_luid = present_context
            .monitor_identity
            .map(|identity| identity.adapter_luid.to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        #[cfg(debug_assertions)]
        let frame_target_id = present_context
            .monitor_identity
            .map(|identity| identity.target_id);
        #[cfg(debug_assertions)]
        let should_log_success_frame = self.should_log_success_frame(
            present_context.overlay_swap_chain,
            desc,
            frame_target_id,
        );
        #[cfg(debug_assertions)]
        if should_log_success_frame {
            debug_log!(
                "event=back_buffer_desc overlay_swap_chain=0x{:x} back_buffer=0x{:x} dxgi_format={} width={} height={} target_id={:?}",
                present_context.overlay_swap_chain,
                back_buffer.as_raw() as usize,
                desc.Format.0,
                desc.Width,
                desc.Height,
                frame_target_id
            );
        }

        let draw_plan = match super::prepare_gpu_draw_plan(
            pipeline,
            present_context.monitor_identity,
            desc.Format.0 as u32,
            desc.Width,
            desc.Height,
            dirty_rects,
        ) {
            Ok(draw_plan) => draw_plan,
            Err(skip) => {
                #[cfg(debug_assertions)]
                debug_log!(
                    "event=lut_draw_skip reason={} overlay_swap_chain=0x{:x} back_buffer=0x{:x} adapter_luid={} target_id={:?} dxgi_format={} width={} height={} dirty_rect_count={}",
                    skip.reason.as_str(),
                    present_context.overlay_swap_chain,
                    back_buffer.as_raw() as usize,
                    frame_adapter_luid,
                    frame_target_id,
                    desc.Format.0,
                    desc.Width,
                    desc.Height,
                    dirty_rects.len()
                );
                return super::RenderPresentLutResult {
                    lut_applied: false,
                    dxgi_format: Some(desc.Format.0 as u32),
                    width: Some(desc.Width),
                    height: Some(desc.Height),
                    lut_index: skip.resolved.map(|resolved| resolved.lut_index),
                    present_dirty_rect: None,
                };
            }
        };
        #[cfg(debug_assertions)]
        if should_log_success_frame {
            debug_log!(
                "event=lut_draw_plan overlay_swap_chain=0x{:x} back_buffer=0x{:x} adapter_luid={} target_id={:?} dxgi_format={} width={} height={} lut_index={} dirty_rect_count={} draw_rect_count={} first_draw_rect={:?}",
                present_context.overlay_swap_chain,
                back_buffer.as_raw() as usize,
                frame_adapter_luid,
                frame_target_id,
                desc.Format.0,
                desc.Width,
                desc.Height,
                draw_plan.lut_index,
                dirty_rects.len(),
                draw_plan.dirty_rects.len(),
                draw_plan.dirty_rects.first()
            );
        }
        #[cfg(debug_assertions)]
        let draw_rect_count = draw_plan.dirty_rects.len();
        #[cfg(debug_assertions)]
        let first_draw_rect = draw_plan.dirty_rects.first().copied();

        let frame = RenderFrame {
            device: &device,
            context: &context,
            back_buffer: &back_buffer,
            width: desc.Width,
            height: desc.Height,
        };
        let result = self.render_with_device(
            present_context.overlay_swap_chain,
            present_context.monitor_identity,
            present_context.hardware_protected,
            frame,
            pipeline,
            draw_plan,
        );
        #[cfg(debug_assertions)]
        if should_log_success_frame {
            debug_log!(
                "event=renderer_present_result overlay_swap_chain=0x{:x} monitor_identity={} target_id={:?} back_buffer=0x{:x} device=0x{:x} context=0x{:x} dxgi_format={:?} width={} height={} lut_index={:?} lut_applied={} dirty_rect_count={} draw_rect_count={} first_draw_rect={:?} present_dirty_rect={:?}",
                present_context.overlay_swap_chain,
                crate::debug_log::quoted(format_monitor_identity(present_context.monitor_identity)),
                present_context
                    .monitor_identity
                    .map(|identity| identity.target_id),
                back_buffer.as_raw() as usize,
                device.as_raw() as usize,
                context.as_raw() as usize,
                result.dxgi_format,
                desc.Width,
                desc.Height,
                result.lut_index,
                result.lut_applied,
                dirty_rects.len(),
                draw_rect_count,
                first_draw_rect,
                result.present_dirty_rect
            );
        }
        result
    }

    #[cfg(debug_assertions)]
    fn should_log_success_frame(
        &mut self,
        overlay_swap_chain: usize,
        desc: D3D11_TEXTURE2D_DESC,
        target_id: Option<u32>,
    ) -> bool {
        self.frame_diagnostics.should_log(
            overlay_swap_chain,
            FrameDiagnosticKey {
                dxgi_format: desc.Format.0 as u32,
                target_id,
                width: desc.Width,
                height: desc.Height,
            },
        )
    }

    unsafe fn overlay_swap_chain_to_back_buffer(
        &mut self,
        overlay_swap_chain: usize,
        swap_chain_path: SwapChainPathHypothesis,
    ) -> Option<ID3D11Texture2D> {
        let texture = unsafe {
            dwm_lut_get_back_buffer(
                overlay_swap_chain as *mut c_void,
                swap_chain_path.container_vtable_index,
                swap_chain_path.resource_vtable_index,
            )
        };
        unsafe { take_owned_interface(texture) }
    }

    fn render_with_device(
        &mut self,
        overlay_swap_chain: usize,
        #[cfg_attr(not(debug_assertions), allow(unused_variables))] monitor_identity: Option<
            MonitorIdentity,
        >,
        #[cfg_attr(not(debug_assertions), allow(unused_variables))] hardware_protected: bool,
        frame: RenderFrame<'_>,
        pipeline: &LutPipeline,
        mut draw_plan: super::GpuDrawPlan,
    ) -> super::RenderPresentLutResult {
        let back_buffer_id = self.back_buffer_id(frame.back_buffer);
        let device_key = frame.device.as_raw() as usize;
        let resource_key = super::ResourceKey {
            device: device_key,
            overlay_swap_chain,
            width: frame.width,
            height: frame.height,
        };
        let recreate = self
            .devices
            .get(&resource_key)
            .is_none_or(|resources| resources.lut_count != pipeline.luts.len());
        if recreate {
            self.devices.remove(&resource_key);
            let Some(resources) =
                DeviceResources::create(frame.device, frame.width, frame.height, pipeline)
            else {
                debug_log!(
                    "event=renderer_early_return reason=device_resources_create_failed overlay_swap_chain=0x{:x} device=0x{:x} width={} height={} lut_count={}",
                    overlay_swap_chain,
                    device_key,
                    frame.width,
                    frame.height,
                    pipeline.luts.len()
                );
                return super::RenderPresentLutResult::planned(
                    draw_plan.format,
                    draw_plan.lut_index,
                );
            };
            self.devices
                .retain(|key, _| super::keeps_device_resource(*key, resource_key));
            self.devices.insert(resource_key, resources);
        }

        let Some(resources) = self.devices.get_mut(&resource_key) else {
            debug_log!(
                "event=renderer_early_return reason=device_resources_missing overlay_swap_chain=0x{:x} device=0x{:x}",
                overlay_swap_chain,
                device_key
            );
            return super::RenderPresentLutResult::planned(draw_plan.format, draw_plan.lut_index);
        };
        if draw_plan.lut_index >= resources.lut_srvs.len() {
            debug_log!(
                "event=renderer_early_return reason=lut_index_out_of_range overlay_swap_chain=0x{:x} device=0x{:x} lut_index={} srv_count={}",
                overlay_swap_chain,
                device_key,
                draw_plan.lut_index,
                resources.lut_srvs.len()
            );
            return super::RenderPresentLutResult::planned(draw_plan.format, draw_plan.lut_index);
        }

        let current_draw_state = super::DrawState {
            format: draw_plan.format,
            lut_index: draw_plan.lut_index,
        };
        let render_target_key = super::RenderTargetKey {
            overlay_swap_chain,
            back_buffer: back_buffer_id,
        };
        let previous_state = resources.draw_states.previous_state(render_target_key);
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
                frame.back_buffer.as_raw() as usize,
                frame.width,
                frame.height,
                draw_plan.format,
                draw_plan.lut_index,
                recreate,
                copy_texture_created,
                previous_state,
                draw_plan.dirty_rects.len()
            );
            draw_plan.dirty_rects = super::draw_rects_for_full_frame(frame.width, frame.height);
        }
        if draw_plan.dirty_rects.is_empty() {
            debug_log!(
                "event=renderer_early_return reason=draw_rects_empty overlay_swap_chain=0x{:x} device=0x{:x}",
                overlay_swap_chain,
                device_key
            );
            return super::RenderPresentLutResult::planned(draw_plan.format, draw_plan.lut_index);
        }
        let present_dirty_rect = super::present_dirty_rect_for_full_redraw(
            needs_full_redraw,
            previous_state,
            recreate,
            copy_texture_created,
            &draw_plan.dirty_rects,
        );

        let result = resources.draw(frame, &draw_plan);
        #[cfg(debug_assertions)]
        crate::route_trace::record_protected_lut_resource_candidate(
            overlay_swap_chain,
            monitor_identity,
            hardware_protected,
            frame.back_buffer.as_raw() as usize,
            frame.device.as_raw() as usize,
            frame.context.as_raw() as usize,
            Some(super::dxgi_format_for_copy_texture(draw_plan.format)),
            Some(frame.width),
            Some(frame.height),
            result,
        );
        if result {
            resources
                .draw_states
                .record_success(render_target_key, current_draw_state);
        }
        super::RenderPresentLutResult {
            lut_applied: result,
            dxgi_format: Some(super::dxgi_format_for_copy_texture(draw_plan.format)),
            width: Some(frame.width),
            height: Some(frame.height),
            lut_index: Some(draw_plan.lut_index),
            present_dirty_rect: result.then_some(present_dirty_rect).flatten(),
        }
    }

    fn clear_resources(&mut self) -> usize {
        let device_count = self.devices.len();
        self.devices.clear();
        #[cfg(debug_assertions)]
        {
            self.frame_diagnostics = PerOverlayDiagnosticLogLimiter::default();
            self.back_buffer_identity_fallbacks.clear();
        }
        device_count
    }

    fn back_buffer_id(&mut self, back_buffer: &ID3D11Texture2D) -> super::BackBufferId {
        match private_data_back_buffer_id(back_buffer) {
            Ok(id) => super::BackBufferId::PrivateData(id),
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
                super::BackBufferId::ComIdentity(identity)
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
    last_constants: Option<ShaderConstantsCBuffer>,
    copy_textures: CopyTextureResources,
    lut_srvs: Vec<ID3D11ShaderResourceView>,
    draw_states: super::RenderTargetStates,
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
        pipeline: &LutPipeline,
    ) -> Option<Self> {
        let vertex_shader = create_vertex_shader(device, LUT_VERTEX_SHADER_BYTECODE)?;
        let pixel_shader = create_pixel_shader(device, LUT_PIXEL_SHADER_BYTECODE)?;
        let input_layout = create_input_layout(device, LUT_VERTEX_SHADER_BYTECODE)?;
        let vertex_buffer = create_vertex_buffer(device)?;
        let sampler = create_sampler(device)?;
        let constant_buffer = create_constant_buffer(device)?;
        let mut lut_srvs = Vec::with_capacity(pipeline.luts.len());
        for lut in &pipeline.luts {
            let (_texture, srv) = create_lut_texture(device, lut)?;
            lut_srvs.push(srv);
        }

        Some(Self {
            width,
            height,
            lut_count: pipeline.luts.len(),
            vertex_shader,
            pixel_shader,
            input_layout,
            vertex_buffer,
            sampler,
            constant_buffer,
            last_constants: None,
            copy_textures: CopyTextureResources::default(),
            lut_srvs,
            draw_states: super::RenderTargetStates::default(),
        })
    }

    fn draw(&mut self, frame: RenderFrame<'_>, draw_plan: &super::GpuDrawPlan) -> bool {
        let Some((copy_texture, copy_srv)) = self
            .copy_textures
            .for_format(frame.device, frame.width, frame.height, draw_plan.format)
            .map(|resource| (resource.texture.clone(), resource.srv.clone()))
        else {
            debug_log!(
                "event=renderer_early_return reason=copy_texture_create_failed device=0x{:x} back_buffer=0x{:x} width={} height={} format={:?}",
                frame.device.as_raw() as usize,
                frame.back_buffer.as_raw() as usize,
                frame.width,
                frame.height,
                draw_plan.format
            );
            return false;
        };
        let Some(rtv) = create_render_target_view(frame.device, frame.back_buffer) else {
            debug_log!(
                "event=renderer_early_return reason=render_target_view_create_failed device=0x{:x} back_buffer=0x{:x}",
                frame.device.as_raw() as usize,
                frame.back_buffer.as_raw() as usize
            );
            return false;
        };
        let bindings =
            PipelineBindings::new(self, &rtv, &copy_srv, &self.lut_srvs[draw_plan.lut_index]);
        super::with_restored_state(
            || ContextState::capture(frame.context),
            || {
                if self.last_constants.as_ref() != Some(&draw_plan.constants) {
                    unsafe {
                        frame.context.UpdateSubresource(
                            &self.constant_buffer,
                            0,
                            None,
                            (&draw_plan.constants as *const ShaderConstantsCBuffer).cast(),
                            0,
                            0,
                        );
                    }
                    self.last_constants = Some(draw_plan.constants);
                }

                for rect in &draw_plan.dirty_rects {
                    let rect = *rect;
                    let box3d = super::copy_box_for_rect(rect);
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
                    let vertices = super::vertices_for_rect(*rect, frame.width, frame.height);
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
        )
    }
}

#[cfg(debug_assertions)]
fn format_monitor_identity(identity: Option<MonitorIdentity>) -> String {
    identity
        .map(|identity| format!("{}:{}", identity.adapter_luid, identity.target_id))
        .unwrap_or_else(|| "none".to_owned())
}

pub(crate) unsafe fn render_present_lut(
    overlay_swap_chain: usize,
    swap_chain_path: SwapChainPathHypothesis,
    monitor_identity: Option<MonitorIdentity>,
    hardware_protected: bool,
    dirty_rects: &[DirtyRect],
    pipeline: &LutPipeline,
) -> super::RenderPresentLutResult {
    let renderer = RENDERER.get_or_init(|| Mutex::new(D3D11Renderer::new()));
    let Ok(mut renderer) = renderer.lock() else {
        return super::RenderPresentLutResult::default();
    };
    unsafe {
        renderer.render_present_lut(
            PresentRenderContext {
                overlay_swap_chain,
                swap_chain_path,
                monitor_identity,
                hardware_protected,
            },
            dirty_rects,
            pipeline,
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
