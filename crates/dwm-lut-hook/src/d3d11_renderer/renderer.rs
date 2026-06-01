use std::collections::BTreeMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::{Mutex, OnceLock};

use super::Vertex;
use super::context_state::{ContextState, unbind_pipeline};
use super::d3d11_api::*;

use crate::lut_pipeline::{
    BackBufferFormat, ClipBox, DirtyRect, LutPipeline, ShaderConstantsCBuffer,
};
use crate::profile::SwapChainPathHypothesis;
use dwm_lut_payload::MonitorIdentity;

#[cfg(debug_assertions)]
const DIAGNOSTIC_SAMPLE_INTERVAL: u64 = 600;
#[cfg(debug_assertions)]
const BACK_BUFFER_STAGE_SUCCESS: u32 = 9;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BackBufferHelperDiagnosticKey {
    stage: u32,
    hresult: Hresult,
    container: usize,
    resource: usize,
    texture: usize,
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
    compiler: Option<D3DCompiler>,
    devices: BTreeMap<super::ResourceKey, DeviceResources>,
    #[cfg(debug_assertions)]
    frame_diagnostics: PerOverlayDiagnosticLogLimiter<FrameDiagnosticKey>,
    #[cfg(debug_assertions)]
    back_buffer_helper_diagnostics: PerOverlayDiagnosticLogLimiter<BackBufferHelperDiagnosticKey>,
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
            #[cfg(debug_assertions)]
            frame_diagnostics: PerOverlayDiagnosticLogLimiter::default(),
            #[cfg(debug_assertions)]
            back_buffer_helper_diagnostics: PerOverlayDiagnosticLogLimiter::default(),
        }
    }

    unsafe fn render_present_lut(
        &mut self,
        overlay_swap_chain: usize,
        swap_chain_path: SwapChainPathHypothesis,
        monitor_identity: Option<MonitorIdentity>,
        clip_box: ClipBox,
        dirty_rects: &[DirtyRect],
        pipeline: &LutPipeline,
    ) -> super::RenderPresentLutResult {
        let Some(back_buffer) = (unsafe {
            self.overlay_swap_chain_to_back_buffer(overlay_swap_chain, swap_chain_path)
        }) else {
            debug_log!(
                "event=back_buffer_get_failed overlay_swap_chain=0x{:x}",
                overlay_swap_chain
            );
            return super::RenderPresentLutResult::default();
        };

        let mut device: ComPtr = ptr::null_mut();
        unsafe {
            d3d11_device_child_get_device(back_buffer, &mut device);
        }
        if device.is_null() {
            debug_log!(
                "event=renderer_early_return reason=device_null overlay_swap_chain=0x{:x} back_buffer=0x{:x}",
                overlay_swap_chain,
                back_buffer as usize
            );
            unsafe { release(back_buffer) };
            return super::RenderPresentLutResult::default();
        }

        let mut context: ComPtr = ptr::null_mut();
        unsafe {
            d3d11_device_get_immediate_context(device, &mut context);
        }
        if context.is_null() {
            debug_log!(
                "event=renderer_early_return reason=context_null overlay_swap_chain=0x{:x} back_buffer=0x{:x} device=0x{:x}",
                overlay_swap_chain,
                back_buffer as usize,
                device as usize
            );
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
        #[cfg(debug_assertions)]
        let frame_adapter_luid = monitor_identity
            .map(|identity| identity.adapter_luid.to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        #[cfg(debug_assertions)]
        let frame_target_id = monitor_identity.map(|identity| identity.target_id);
        #[cfg(debug_assertions)]
        let should_log_success_frame =
            self.should_log_success_frame(overlay_swap_chain, desc, frame_target_id);
        #[cfg(debug_assertions)]
        if should_log_success_frame {
            debug_log!(
                "event=back_buffer_desc overlay_swap_chain=0x{:x} back_buffer=0x{:x} dxgi_format={} width={} height={} target_id={:?}",
                overlay_swap_chain,
                back_buffer as usize,
                desc.format,
                desc.width,
                desc.height,
                frame_target_id
            );
        }

        let draw_plan = match super::prepare_gpu_draw_plan(
            pipeline,
            monitor_identity,
            clip_box,
            desc.format,
            desc.width,
            desc.height,
            dirty_rects,
        ) {
            Ok(draw_plan) => draw_plan,
            Err(_reason) => {
                #[cfg(debug_assertions)]
                debug_log!(
                    "event=lut_draw_skip reason={} overlay_swap_chain=0x{:x} back_buffer=0x{:x} adapter_luid={} target_id={:?} clip_left={} clip_top={} dxgi_format={} width={} height={} dirty_rect_count={}",
                    _reason.as_str(),
                    overlay_swap_chain,
                    back_buffer as usize,
                    frame_adapter_luid,
                    frame_target_id,
                    clip_box.left,
                    clip_box.top,
                    desc.format,
                    desc.width,
                    desc.height,
                    dirty_rects.len()
                );
                unsafe {
                    release(back_buffer);
                    release(context);
                    release(device);
                }
                return super::RenderPresentLutResult::default();
            }
        };
        #[cfg(debug_assertions)]
        if should_log_success_frame {
            debug_log!(
                "event=lut_draw_plan overlay_swap_chain=0x{:x} back_buffer=0x{:x} adapter_luid={} target_id={:?} clip_left={} clip_top={} dxgi_format={} width={} height={} lut_index={} dirty_rect_count={} draw_rect_count={} first_draw_rect={:?}",
                overlay_swap_chain,
                back_buffer as usize,
                frame_adapter_luid,
                frame_target_id,
                clip_box.left,
                clip_box.top,
                desc.format,
                desc.width,
                desc.height,
                draw_plan.lut_index,
                dirty_rects.len(),
                draw_plan.dirty_rects.len(),
                draw_plan.dirty_rects.first()
            );
        }

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

    #[cfg(debug_assertions)]
    fn should_log_success_frame(
        &mut self,
        overlay_swap_chain: usize,
        desc: Texture2DDesc,
        target_id: Option<u32>,
    ) -> bool {
        self.frame_diagnostics.should_log(
            overlay_swap_chain,
            FrameDiagnosticKey {
                dxgi_format: desc.format,
                target_id,
                width: desc.width,
                height: desc.height,
            },
        )
    }

    #[cfg(debug_assertions)]
    unsafe fn overlay_swap_chain_to_back_buffer(
        &mut self,
        overlay_swap_chain: usize,
        swap_chain_path: SwapChainPathHypothesis,
    ) -> Option<ComPtr> {
        let mut diagnostic = BackBuffer25H2Diagnostic::new();
        let texture = unsafe {
            dwm_lut_get_back_buffer_25h2_diagnostic(
                overlay_swap_chain as ComPtr,
                swap_chain_path.container_vtable_index,
                swap_chain_path.resource_vtable_index,
                &mut diagnostic,
            )
        };
        let diagnostic_key = BackBufferHelperDiagnosticKey {
            stage: diagnostic.stage,
            hresult: diagnostic.hresult,
            container: diagnostic.container as usize,
            resource: diagnostic.resource as usize,
            texture: diagnostic.texture as usize,
        };
        if diagnostic.stage != BACK_BUFFER_STAGE_SUCCESS
            || self
                .back_buffer_helper_diagnostics
                .should_log(overlay_swap_chain, diagnostic_key)
        {
            debug_log!(
                "event=back_buffer_helper_result overlay_swap_chain=0x{:x} stage={} hresult={} container=0x{:x} resource=0x{:x} texture=0x{:x}",
                overlay_swap_chain,
                diagnostic.stage,
                diagnostic.hresult,
                diagnostic.container as usize,
                diagnostic.resource as usize,
                diagnostic.texture as usize
            );
        }
        (!texture.is_null()).then_some(texture)
    }

    #[cfg(not(debug_assertions))]
    unsafe fn overlay_swap_chain_to_back_buffer(
        &mut self,
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
            None => {
                debug_log!(
                    "event=renderer_early_return reason=compiler_unavailable overlay_swap_chain=0x{:x} device=0x{:x}",
                    overlay_swap_chain,
                    device_key
                );
                return super::RenderPresentLutResult::planned(
                    draw_plan.format,
                    draw_plan.lut_index,
                );
            }
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
            &draw_plan.dirty_rects,
        );

        let result = unsafe { resources.draw(frame, &draw_plan) };
        if result {
            resources.draw_states.insert(
                render_target_key,
                super::RenderTargetState::Stable(current_draw_state),
            );
        }
        super::RenderPresentLutResult {
            lut_applied: result,
            dxgi_format: Some(super::dxgi_format_for_copy_texture(draw_plan.format)),
            lut_index: Some(draw_plan.lut_index),
            present_dirty_rect: result.then_some(present_dirty_rect).flatten(),
        }
    }

    fn compiler(&mut self) -> Option<&D3DCompiler> {
        if self.compiler.is_none() {
            self.compiler = unsafe { load_compiler() };
        }
        self.compiler.as_ref()
    }

    fn clear_resources(&mut self) -> usize {
        let device_count = self.devices.len();
        self.devices.clear();
        self.compiler = None;
        #[cfg(debug_assertions)]
        {
            self.frame_diagnostics = PerOverlayDiagnosticLogLimiter::default();
            self.back_buffer_helper_diagnostics = PerOverlayDiagnosticLogLimiter::default();
        }
        device_count
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
            *slot = Some(unsafe { CopyTextureResource::create(device, width, height, format) }?);
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
            debug_log!(
                "event=renderer_early_return reason=copy_texture_create_failed device=0x{:x} back_buffer=0x{:x} width={} height={} format={:?}",
                frame.device as usize,
                frame.back_buffer as usize,
                frame.width,
                frame.height,
                draw_plan.format
            );
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
            debug_log!(
                "event=renderer_early_return reason=render_target_view_create_failed device=0x{:x} back_buffer=0x{:x}",
                frame.device as usize,
                frame.back_buffer as usize
            );
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
    monitor_identity: Option<MonitorIdentity>,
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
            monitor_identity,
            clip_box,
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
        d3d11_context_ia_set_primitive_topology(context, D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
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
        d3d11_context_om_set_blend_state(context, ptr::null_mut(), blend_factor.as_ptr(), u32::MAX);
        d3d11_context_om_set_depth_stencil_state(context, ptr::null_mut(), 0);
        d3d11_context_rs_set_state(context, ptr::null_mut());
        d3d11_context_rs_set_viewports(context, &viewport);
        d3d11_context_rs_set_scissor_rects(context, empty_scissors.as_ptr(), 0);
    }
}
