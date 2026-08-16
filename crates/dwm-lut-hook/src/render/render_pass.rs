use std::mem::size_of;

use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_VERTEX_BUFFER, D3D11_BOX, D3D11_BUFFER_DESC,
    D3D11_COMPARISON_NEVER, D3D11_FILTER_MIN_MAG_MIP_POINT, D3D11_FLOAT32_MAX,
    D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA, D3D11_SAMPLER_DESC,
    D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE_ADDRESS_CLAMP,
    D3D11_USAGE_DEFAULT, D3D11_VIEWPORT, ID3D11Buffer, ID3D11ClassLinkage, ID3D11DepthStencilView,
    ID3D11Device, ID3D11DeviceContext, ID3D11InputLayout, ID3D11PixelShader,
    ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D,
    ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32G32_FLOAT;
use windows::core::PCSTR;

use super::FrameSize;
use super::context_state::ContextState;
use super::frame_plan::{BackBufferFormat, DrawScope, FramePlan};
use crate::dwmcore::DirtyRect;
use crate::state::LutAssignment;

const LUT_VERTEX_SHADER_BYTECODE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/lut_pipeline_vs.cso"));
const LUT_PIXEL_SHADER_BYTECODE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/lut_pipeline_ps.cso"));
const RENDER_TARGET_LIMIT: usize = D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as usize;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Vertex {
    position: [f32; 2],
    texcoord: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct ShaderConstants {
    lut_size: u32,
    hdr: u32,
    padding: [f32; 2],
    domain_min: [f32; 4],
    domain_max: [f32; 4],
}

impl ShaderConstants {
    fn for_assignment(assignment: &LutAssignment, format: BackBufferFormat) -> Self {
        let extend_domain = |domain: [f32; 3]| [domain[0], domain[1], domain[2], 0.0];
        Self {
            lut_size: assignment.metadata.size,
            hdr: u32::from(format.is_hdr()),
            padding: [0.0, 0.0],
            domain_min: extend_domain(assignment.metadata.domain_min),
            domain_max: extend_domain(assignment.metadata.domain_max),
        }
    }
}

pub(super) struct LutRenderPass {
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    input_layout: ID3D11InputLayout,
    vertex_buffer: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    constant_buffer: ID3D11Buffer,
    last_constants: Option<ShaderConstants>,
}

pub(super) struct LutPassInput<'a> {
    pub(super) context: &'a ID3D11DeviceContext,
    pub(super) back_buffer: &'a ID3D11Texture2D,
    pub(super) render_target: &'a ID3D11RenderTargetView,
    pub(super) clean_texture: &'a ID3D11Texture2D,
    pub(super) clean_view: &'a ID3D11ShaderResourceView,
    pub(super) lut_view: &'a ID3D11ShaderResourceView,
    pub(super) assignment: &'a LutAssignment,
    pub(super) clean_update_rects: &'a [DirtyRect],
    pub(super) plan: &'a FramePlan,
}

impl LutRenderPass {
    pub(super) fn create(device: &ID3D11Device) -> Option<Self> {
        Some(Self {
            vertex_shader: create_vertex_shader(device, LUT_VERTEX_SHADER_BYTECODE)?,
            pixel_shader: create_pixel_shader(device, LUT_PIXEL_SHADER_BYTECODE)?,
            input_layout: create_input_layout(device, LUT_VERTEX_SHADER_BYTECODE)?,
            vertex_buffer: create_vertex_buffer(device)?,
            sampler: create_sampler(device)?,
            constant_buffer: create_constant_buffer(device)?,
            last_constants: None,
        })
    }

    pub(super) fn execute(&mut self, input: LutPassInput<'_>) {
        let saved_state = ContextState::capture(input.context);
        clear_pipeline_bindings(input.context);

        copy_back_buffer_regions_to_clean_frame(
            input.context,
            input.back_buffer,
            input.clean_texture,
            input.clean_update_rects,
        );

        let constants =
            ShaderConstants::for_assignment(input.assignment, input.plan.lut().back_buffer_format);
        if self.last_constants != Some(constants) {
            unsafe {
                input.context.UpdateSubresource(
                    &self.constant_buffer,
                    0,
                    None,
                    (&constants as *const ShaderConstants).cast(),
                    0,
                    0,
                );
            }
            self.last_constants = Some(constants);
        }

        self.bind(
            input.context,
            input.render_target,
            input.clean_view,
            input.lut_view,
            input.plan.size(),
        );
        match input.plan.scope() {
            DrawScope::DirtyRects => {
                for rect in input.plan.source_dirty_rects() {
                    self.draw_rect(input.context, *rect, input.plan.size());
                }
            }
            DrawScope::FullFrame => {
                self.draw_rect(
                    input.context,
                    input.plan.size().full_rect(),
                    input.plan.size(),
                );
            }
        }

        clear_pipeline_bindings(input.context);
        saved_state.restore(input.context);
    }

    fn bind(
        &self,
        context: &ID3D11DeviceContext,
        render_target: &ID3D11RenderTargetView,
        clean_view: &ID3D11ShaderResourceView,
        lut_view: &ID3D11ShaderResourceView,
        size: FrameSize,
    ) {
        let vertex_buffers = [Some(self.vertex_buffer.clone())];
        let shader_resources = [Some(clean_view.clone()), Some(lut_view.clone())];
        let samplers = [Some(self.sampler.clone())];
        let constant_buffers = [Some(self.constant_buffer.clone())];
        let mut render_targets: [Option<ID3D11RenderTargetView>; RENDER_TARGET_LIMIT] =
            std::array::from_fn(|_| None);
        render_targets[0] = Some(render_target.clone());
        let stride = size_of::<Vertex>() as u32;
        let offset = 0;
        let blend_factor = [0.0; 4];
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: size.width as f32,
            Height: size.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };

        unsafe {
            context.IASetInputLayout(&self.input_layout);
            context.IASetVertexBuffers(
                0,
                1,
                Some(vertex_buffers.as_ptr()),
                Some(&stride),
                Some(&offset),
            );
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            context.VSSetShader(&self.vertex_shader, None);
            context.GSSetShader(None, None);
            context.HSSetShader(None, None);
            context.DSSetShader(None, None);
            context.PSSetShader(&self.pixel_shader, None);
            context.PSSetShaderResources(0, Some(&shader_resources));
            context.PSSetSamplers(0, Some(&samplers));
            context.VSSetConstantBuffers(0, Some(&constant_buffers));
            context.PSSetConstantBuffers(0, Some(&constant_buffers));
            context.OMSetRenderTargets(Some(&render_targets), None);
            context.OMSetBlendState(None, Some(&blend_factor), u32::MAX);
            context.OMSetDepthStencilState(None, 0);
            context.RSSetState(None);
            context.RSSetViewports(Some(&[viewport]));
            context.RSSetScissorRects(Some(&[]));
        }
    }

    fn draw_rect(&self, context: &ID3D11DeviceContext, rect: DirtyRect, size: FrameSize) {
        let vertices = vertices_for_rect(rect, size);
        unsafe {
            context.UpdateSubresource(&self.vertex_buffer, 0, None, vertices.as_ptr().cast(), 0, 0);
            context.Draw(4, 0);
        }
    }
}

pub(super) fn restore_back_buffer_from_clean_frame(
    context: &ID3D11DeviceContext,
    back_buffer: &ID3D11Texture2D,
    clean_texture: &ID3D11Texture2D,
    source_dirty_rects: &[DirtyRect],
) {
    with_preserved_context(context, || {
        copy_back_buffer_regions_to_clean_frame(
            context,
            back_buffer,
            clean_texture,
            source_dirty_rects,
        );
        unsafe {
            context.CopyResource(back_buffer, clean_texture);
        }
    });
}

pub(super) fn update_clean_frame(
    context: &ID3D11DeviceContext,
    back_buffer: &ID3D11Texture2D,
    clean_texture: &ID3D11Texture2D,
    source_dirty_rects: &[DirtyRect],
) {
    with_preserved_context(context, || {
        copy_back_buffer_regions_to_clean_frame(
            context,
            back_buffer,
            clean_texture,
            source_dirty_rects,
        );
    });
}

fn with_preserved_context(context: &ID3D11DeviceContext, operation: impl FnOnce()) {
    let saved_state = ContextState::capture(context);
    clear_pipeline_bindings(context);
    operation();
    clear_pipeline_bindings(context);
    saved_state.restore(context);
}

fn copy_back_buffer_regions_to_clean_frame(
    context: &ID3D11DeviceContext,
    back_buffer: &ID3D11Texture2D,
    clean_texture: &ID3D11Texture2D,
    source_dirty_rects: &[DirtyRect],
) {
    for rect in source_dirty_rects {
        copy_back_buffer_rect_to_clean(context, back_buffer, clean_texture, *rect);
    }
}

fn copy_back_buffer_rect_to_clean(
    context: &ID3D11DeviceContext,
    back_buffer: &ID3D11Texture2D,
    clean_texture: &ID3D11Texture2D,
    rect: DirtyRect,
) {
    let source_box = D3D11_BOX {
        left: rect.left as u32,
        top: rect.top as u32,
        front: 0,
        right: rect.right as u32,
        bottom: rect.bottom as u32,
        back: 1,
    };
    unsafe {
        context.CopySubresourceRegion(
            clean_texture,
            0,
            rect.left as u32,
            rect.top as u32,
            0,
            back_buffer,
            0,
            Some(&source_box),
        );
    }
}

fn vertices_for_rect(rect: DirtyRect, size: FrameSize) -> [Vertex; 4] {
    let width = size.width as f32;
    let height = size.height as f32;
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

fn create_vertex_shader(device: &ID3D11Device, bytecode: &[u8]) -> Option<ID3D11VertexShader> {
    let mut shader = None;
    unsafe {
        device
            .CreateVertexShader(bytecode, None::<&ID3D11ClassLinkage>, Some(&mut shader))
            .ok()?;
    }
    shader
}

fn create_pixel_shader(device: &ID3D11Device, bytecode: &[u8]) -> Option<ID3D11PixelShader> {
    let mut shader = None;
    unsafe {
        device
            .CreatePixelShader(bytecode, None::<&ID3D11ClassLinkage>, Some(&mut shader))
            .ok()?;
    }
    shader
}

fn create_input_layout(
    device: &ID3D11Device,
    vertex_shader_bytecode: &[u8],
) -> Option<ID3D11InputLayout> {
    const POSITION: PCSTR = PCSTR(c"POSITION".as_ptr().cast());
    const TEXCOORD: PCSTR = PCSTR(c"TEXCOORD".as_ptr().cast());
    let elements = [
        D3D11_INPUT_ELEMENT_DESC {
            SemanticName: POSITION,
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 0,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D11_INPUT_ELEMENT_DESC {
            SemanticName: TEXCOORD,
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 8,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
    ];
    let mut layout = None;
    unsafe {
        device
            .CreateInputLayout(&elements, vertex_shader_bytecode, Some(&mut layout))
            .ok()?;
    }
    layout
}

fn create_vertex_buffer(device: &ID3D11Device) -> Option<ID3D11Buffer> {
    let vertices = [Vertex {
        position: [0.0, 0.0],
        texcoord: [0.0, 0.0],
    }; 4];
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: size_of::<[Vertex; 4]>() as u32,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let data = D3D11_SUBRESOURCE_DATA {
        pSysMem: vertices.as_ptr().cast(),
        SysMemPitch: 0,
        SysMemSlicePitch: 0,
    };
    create_buffer(device, &desc, Some(&data))
}

fn create_constant_buffer(device: &ID3D11Device) -> Option<ID3D11Buffer> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: size_of::<ShaderConstants>() as u32,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    create_buffer(device, &desc, None)
}

fn create_buffer(
    device: &ID3D11Device,
    desc: &D3D11_BUFFER_DESC,
    data: Option<&D3D11_SUBRESOURCE_DATA>,
) -> Option<ID3D11Buffer> {
    let mut buffer = None;
    unsafe {
        device
            .CreateBuffer(
                desc,
                data.map(|data| data as *const D3D11_SUBRESOURCE_DATA),
                Some(&mut buffer),
            )
            .ok()?;
    }
    buffer
}

fn create_sampler(device: &ID3D11Device) -> Option<ID3D11SamplerState> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        BorderColor: [0.0; 4],
        MinLOD: 0.0,
        MaxLOD: D3D11_FLOAT32_MAX,
    };
    let mut sampler = None;
    unsafe {
        device.CreateSamplerState(&desc, Some(&mut sampler)).ok()?;
    }
    sampler
}

fn clear_pipeline_bindings(context: &ID3D11DeviceContext) {
    let null_srvs: [Option<ID3D11ShaderResourceView>; 2] = std::array::from_fn(|_| None);
    let null_samplers: [Option<ID3D11SamplerState>; 1] = std::array::from_fn(|_| None);
    let null_buffers: [Option<ID3D11Buffer>; 1] = std::array::from_fn(|_| None);
    let null_targets: [Option<ID3D11RenderTargetView>; RENDER_TARGET_LIMIT] =
        std::array::from_fn(|_| None);
    unsafe {
        context.PSSetShaderResources(0, Some(&null_srvs));
        context.PSSetSamplers(0, Some(&null_samplers));
        context.VSSetConstantBuffers(0, Some(&null_buffers));
        context.PSSetConstantBuffers(0, Some(&null_buffers));
        context.OMSetRenderTargets(Some(&null_targets), None::<&ID3D11DepthStencilView>);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{LutMetadata, ShaderTexture3D};
    use dwm_lut_payload::{AdapterLuid, ColorMode, MonitorIdentity, MonitorTarget};

    #[test]
    fn vertices_map_a_pixel_rect_to_clip_space_and_texture_coordinates() {
        let vertices = vertices_for_rect(
            DirtyRect {
                left: 100,
                top: 50,
                right: 300,
                bottom: 250,
            },
            FrameSize {
                width: 1000,
                height: 500,
            },
        );

        assert_vec2_near(vertices[0].position, [-0.8, 0.8]);
        assert_vec2_near(vertices[3].position, [-0.4, 0.0]);
        assert_vec2_near(vertices[0].texcoord, [0.1, 0.1]);
        assert_vec2_near(vertices[3].texcoord, [0.3, 0.5]);
    }

    #[test]
    fn shader_constants_match_the_hlsl_constant_buffer_layout() {
        use std::ptr::addr_of;

        let assignment = LutAssignment {
            target: MonitorTarget {
                identity: MonitorIdentity {
                    adapter_luid: AdapterLuid {
                        low_part: 1,
                        high_part: 2,
                    },
                    target_id: 3,
                },
                color_mode: ColorMode::Hdr,
            },
            metadata: LutMetadata {
                size: 2,
                domain_min: [0.0; 3],
                domain_max: [1.0; 3],
            },
            texture: ShaderTexture3D {
                width: 2,
                height: 2,
                depth: 2,
                texels: vec![[0.0; 4]; 8],
            },
        };
        let constants = ShaderConstants::for_assignment(&assignment, BackBufferFormat::Rgba16Float);
        let base = (&constants as *const ShaderConstants) as usize;

        assert_eq!(size_of::<ShaderConstants>(), 48);
        assert_eq!(addr_of!(constants.lut_size) as usize - base, 0);
        assert_eq!(addr_of!(constants.hdr) as usize - base, 4);
        assert_eq!(addr_of!(constants.padding) as usize - base, 8);
        assert_eq!(addr_of!(constants.domain_min) as usize - base, 16);
        assert_eq!(addr_of!(constants.domain_max) as usize - base, 32);
        assert_eq!(constants.hdr, 1);
    }

    fn assert_vec2_near(actual: [f32; 2], expected: [f32; 2]) {
        const EPSILON: f32 = 0.000_001;
        assert!((actual[0] - expected[0]).abs() <= EPSILON);
        assert!((actual[1] - expected[1]).abs() <= EPSILON);
    }
}
