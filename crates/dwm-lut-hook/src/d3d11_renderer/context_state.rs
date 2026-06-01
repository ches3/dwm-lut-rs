use std::ptr;

use super::d3d11_api::*;
use crate::lut_pipeline::DirtyRect;

pub(super) struct ShaderStageState {
    pub(super) shader: ComPtr,
    pub(super) class_instances: [ComPtr; D3D11_SHADER_CLASS_INSTANCE_LIMIT],
    pub(super) class_instance_count: u32,
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

pub(super) struct ContextState {
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
    pub(super) unsafe fn capture(context: ComPtr) -> Self {
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

    pub(super) unsafe fn restore(&self, context: ComPtr) {
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

pub(super) unsafe fn unbind_pipeline(context: ComPtr) {
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
