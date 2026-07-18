use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT, D3D11_VIEWPORT,
    D3D11_VIEWPORT_AND_SCISSORRECT_OBJECT_COUNT_PER_PIPELINE, ID3D11BlendState, ID3D11Buffer,
    ID3D11ClassInstance, ID3D11DepthStencilState, ID3D11DepthStencilView, ID3D11DeviceContext,
    ID3D11DomainShader, ID3D11GeometryShader, ID3D11HullShader, ID3D11InputLayout,
    ID3D11PixelShader, ID3D11RasterizerState, ID3D11RenderTargetView, ID3D11SamplerState,
    ID3D11ShaderResourceView, ID3D11VertexShader,
};

const SHADER_CLASS_INSTANCE_LIMIT: usize = 256;
const VIEWPORT_LIMIT: usize = D3D11_VIEWPORT_AND_SCISSORRECT_OBJECT_COUNT_PER_PIPELINE as usize;
const RENDER_TARGET_LIMIT: usize = D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as usize;

pub(super) struct ShaderStageState<T> {
    shader: Option<T>,
    class_instances: [Option<ID3D11ClassInstance>; SHADER_CLASS_INSTANCE_LIMIT],
    class_instance_count: u32,
}

impl<T> ShaderStageState<T> {
    fn capture(
        get: impl FnOnce(*mut Option<T>, Option<*mut Option<ID3D11ClassInstance>>, Option<*mut u32>),
    ) -> Self {
        let mut state = Self {
            shader: None,
            class_instances: std::array::from_fn(|_| None),
            class_instance_count: SHADER_CLASS_INSTANCE_LIMIT as u32,
        };
        get(
            &mut state.shader,
            Some(state.class_instances.as_mut_ptr()),
            Some(&mut state.class_instance_count),
        );
        state.class_instance_count = state
            .class_instance_count
            .min(SHADER_CLASS_INSTANCE_LIMIT as u32);
        state
    }

    fn class_instances(&self) -> Option<&[Option<ID3D11ClassInstance>]> {
        (self.class_instance_count != 0)
            .then(|| &self.class_instances[..self.class_instance_count as usize])
    }
}

pub(super) struct ContextState {
    input_layout: Option<ID3D11InputLayout>,
    vertex_buffer: Option<ID3D11Buffer>,
    vertex_stride: u32,
    vertex_offset: u32,
    primitive_topology: D3D_PRIMITIVE_TOPOLOGY,
    vertex_shader: ShaderStageState<ID3D11VertexShader>,
    geometry_shader: ShaderStageState<ID3D11GeometryShader>,
    hull_shader: ShaderStageState<ID3D11HullShader>,
    domain_shader: ShaderStageState<ID3D11DomainShader>,
    pixel_shader: ShaderStageState<ID3D11PixelShader>,
    pixel_srvs: [Option<ID3D11ShaderResourceView>; 2],
    pixel_sampler: Option<ID3D11SamplerState>,
    vertex_constant_buffer: Option<ID3D11Buffer>,
    pixel_constant_buffer: Option<ID3D11Buffer>,
    render_targets: [Option<ID3D11RenderTargetView>; RENDER_TARGET_LIMIT],
    depth_stencil: Option<ID3D11DepthStencilView>,
    blend_state: Option<ID3D11BlendState>,
    blend_factor: [f32; 4],
    sample_mask: u32,
    depth_stencil_state: Option<ID3D11DepthStencilState>,
    stencil_ref: u32,
    rasterizer_state: Option<ID3D11RasterizerState>,
    viewport_count: u32,
    viewports: [D3D11_VIEWPORT; VIEWPORT_LIMIT],
    scissor_count: u32,
    scissor_rects: [RECT; VIEWPORT_LIMIT],
}

impl ContextState {
    pub(super) fn capture(context: &ID3D11DeviceContext) -> Self {
        let input_layout = unsafe { context.IAGetInputLayout().ok() };
        let mut vertex_buffer = None;
        let mut vertex_stride = 0;
        let mut vertex_offset = 0;
        unsafe {
            context.IAGetVertexBuffers(
                0,
                1,
                Some(&mut vertex_buffer),
                Some(&mut vertex_stride),
                Some(&mut vertex_offset),
            );
        }
        let primitive_topology = unsafe { context.IAGetPrimitiveTopology() };
        let vertex_shader = ShaderStageState::capture(|shader, instances, count| unsafe {
            context.VSGetShader(shader, instances, count);
        });
        let geometry_shader = ShaderStageState::capture(|shader, instances, count| unsafe {
            context.GSGetShader(shader, instances, count);
        });
        let hull_shader = ShaderStageState::capture(|shader, instances, count| unsafe {
            context.HSGetShader(shader, instances, count);
        });
        let domain_shader = ShaderStageState::capture(|shader, instances, count| unsafe {
            context.DSGetShader(shader, instances, count);
        });
        let pixel_shader = ShaderStageState::capture(|shader, instances, count| unsafe {
            context.PSGetShader(shader, instances, count);
        });

        let mut pixel_srvs = std::array::from_fn(|_| None);
        let mut pixel_sampler = None;
        let mut vertex_constant_buffer = None;
        let mut pixel_constant_buffer = None;
        let mut render_targets = std::array::from_fn(|_| None);
        let mut depth_stencil = None;
        let mut blend_state = None;
        let mut blend_factor = [0.0; 4];
        let mut sample_mask = 0;
        let mut depth_stencil_state = None;
        let mut stencil_ref = 0;
        let rasterizer_state = unsafe { context.RSGetState().ok() };
        let mut viewport_count = VIEWPORT_LIMIT as u32;
        let mut viewports = [D3D11_VIEWPORT::default(); VIEWPORT_LIMIT];
        let mut scissor_count = VIEWPORT_LIMIT as u32;
        let mut scissor_rects = [RECT::default(); VIEWPORT_LIMIT];

        unsafe {
            context.PSGetShaderResources(0, Some(&mut pixel_srvs));
            context.PSGetSamplers(0, Some(std::slice::from_mut(&mut pixel_sampler)));
            context
                .VSGetConstantBuffers(0, Some(std::slice::from_mut(&mut vertex_constant_buffer)));
            context.PSGetConstantBuffers(0, Some(std::slice::from_mut(&mut pixel_constant_buffer)));
            context.OMGetRenderTargets(Some(&mut render_targets), Some(&mut depth_stencil));
            context.OMGetBlendState(
                Some(&mut blend_state),
                Some(&mut blend_factor),
                Some(&mut sample_mask),
            );
            context.OMGetDepthStencilState(Some(&mut depth_stencil_state), Some(&mut stencil_ref));
            context.RSGetViewports(&mut viewport_count, Some(viewports.as_mut_ptr()));
            context.RSGetScissorRects(&mut scissor_count, Some(scissor_rects.as_mut_ptr()));
        }
        viewport_count = viewport_count.min(VIEWPORT_LIMIT as u32);
        scissor_count = scissor_count.min(VIEWPORT_LIMIT as u32);

        Self {
            input_layout,
            vertex_buffer,
            vertex_stride,
            vertex_offset,
            primitive_topology,
            vertex_shader,
            geometry_shader,
            hull_shader,
            domain_shader,
            pixel_shader,
            pixel_srvs,
            pixel_sampler,
            vertex_constant_buffer,
            pixel_constant_buffer,
            render_targets,
            depth_stencil,
            blend_state,
            blend_factor,
            sample_mask,
            depth_stencil_state,
            stencil_ref,
            rasterizer_state,
            viewport_count,
            viewports,
            scissor_count,
            scissor_rects,
        }
    }

    pub(super) fn restore(&self, context: &ID3D11DeviceContext) {
        unsafe {
            context.IASetInputLayout(self.input_layout.as_ref());
            context.IASetVertexBuffers(
                0,
                1,
                Some(&self.vertex_buffer),
                Some(&self.vertex_stride),
                Some(&self.vertex_offset),
            );
            context.IASetPrimitiveTopology(self.primitive_topology);
            context.VSSetShader(
                self.vertex_shader.shader.as_ref(),
                self.vertex_shader.class_instances(),
            );
            context.GSSetShader(
                self.geometry_shader.shader.as_ref(),
                self.geometry_shader.class_instances(),
            );
            context.HSSetShader(
                self.hull_shader.shader.as_ref(),
                self.hull_shader.class_instances(),
            );
            context.DSSetShader(
                self.domain_shader.shader.as_ref(),
                self.domain_shader.class_instances(),
            );
            context.PSSetShader(
                self.pixel_shader.shader.as_ref(),
                self.pixel_shader.class_instances(),
            );
            context.PSSetShaderResources(0, Some(&self.pixel_srvs));
            context.PSSetSamplers(0, Some(std::slice::from_ref(&self.pixel_sampler)));
            context
                .VSSetConstantBuffers(0, Some(std::slice::from_ref(&self.vertex_constant_buffer)));
            context
                .PSSetConstantBuffers(0, Some(std::slice::from_ref(&self.pixel_constant_buffer)));
            context.OMSetRenderTargets(Some(&self.render_targets), self.depth_stencil.as_ref());
            context.OMSetBlendState(
                self.blend_state.as_ref(),
                Some(&self.blend_factor),
                self.sample_mask,
            );
            context.OMSetDepthStencilState(self.depth_stencil_state.as_ref(), self.stencil_ref);
            context.RSSetState(self.rasterizer_state.as_ref());
            context.RSSetViewports(Some(&self.viewports[..self.viewport_count as usize]));
            context.RSSetScissorRects(Some(&self.scissor_rects[..self.scissor_count as usize]));
        }
    }
}

pub(super) fn unbind_pipeline(context: &ID3D11DeviceContext) {
    let null_srvs: [Option<ID3D11ShaderResourceView>; 2] = std::array::from_fn(|_| None);
    let null_samplers: [Option<ID3D11SamplerState>; 1] = std::array::from_fn(|_| None);
    let null_buffers: [Option<ID3D11Buffer>; 1] = std::array::from_fn(|_| None);
    let null_rtvs: [Option<ID3D11RenderTargetView>; RENDER_TARGET_LIMIT] =
        std::array::from_fn(|_| None);
    unsafe {
        context.PSSetShaderResources(0, Some(&null_srvs));
        context.PSSetSamplers(0, Some(&null_samplers));
        context.VSSetConstantBuffers(0, Some(&null_buffers));
        context.PSSetConstantBuffers(0, Some(&null_buffers));
        context.OMSetRenderTargets(Some(&null_rtvs), None::<&ID3D11DepthStencilView>);
    }
}
