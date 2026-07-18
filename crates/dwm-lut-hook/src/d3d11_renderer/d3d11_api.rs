use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BIND_VERTEX_BUFFER,
    D3D11_BUFFER_DESC, D3D11_COMPARISON_NEVER, D3D11_FILTER_MIN_MAG_MIP_POINT, D3D11_FLOAT32_MAX,
    D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA, D3D11_SAMPLER_DESC,
    D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE2D_DESC,
    D3D11_TEXTURE3D_DESC, D3D11_USAGE_DEFAULT, ID3D11Buffer, ID3D11ClassLinkage, ID3D11Device,
    ID3D11InputLayout, ID3D11PixelShader, ID3D11RenderTargetView, ID3D11SamplerState,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11Texture3D, ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::core::{Interface, PCSTR};

use super::Vertex;
use crate::lut_pipeline::{BackBufferFormat, ShaderConstantsCBuffer};

pub(super) const LUT_VERTEX_SHADER_BYTECODE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/lut_pipeline_vs.cso"));
pub(super) const LUT_PIXEL_SHADER_BYTECODE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/lut_pipeline_ps.cso"));

unsafe extern "system" {
    pub(super) fn dwm_lut_get_back_buffer_25h2(
        overlay_swap_chain: *mut c_void,
        container_vtable_index: usize,
        resource_vtable_index: usize,
    ) -> *mut c_void;
}

pub(super) unsafe fn take_owned_interface<T: Interface>(raw: *mut c_void) -> Option<T> {
    if raw.is_null() {
        None
    } else {
        Some(unsafe { T::from_raw(raw) })
    }
}

pub(super) fn create_vertex_shader(
    device: &ID3D11Device,
    bytecode: &[u8],
) -> Option<ID3D11VertexShader> {
    let mut shader = None;
    unsafe {
        device
            .CreateVertexShader(bytecode, None::<&ID3D11ClassLinkage>, Some(&mut shader))
            .ok()?;
    }
    shader
}

pub(super) fn create_pixel_shader(
    device: &ID3D11Device,
    bytecode: &[u8],
) -> Option<ID3D11PixelShader> {
    let mut shader = None;
    unsafe {
        device
            .CreatePixelShader(bytecode, None::<&ID3D11ClassLinkage>, Some(&mut shader))
            .ok()?;
    }
    shader
}

pub(super) fn create_input_layout(
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

pub(super) fn create_vertex_buffer(device: &ID3D11Device) -> Option<ID3D11Buffer> {
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

pub(super) fn create_constant_buffer(device: &ID3D11Device) -> Option<ID3D11Buffer> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: size_of::<ShaderConstantsCBuffer>() as u32,
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

pub(super) fn create_sampler(device: &ID3D11Device) -> Option<ID3D11SamplerState> {
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

pub(super) fn create_copy_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    format: BackBufferFormat,
) -> Option<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT(super::dxgi_format_for_copy_texture(format) as i32),
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
    let srv = create_shader_resource_view(device, &texture)?;
    Some((texture, srv))
}

pub(super) fn create_render_target_view(
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

pub(super) fn create_lut_texture(
    device: &ID3D11Device,
    lut: &crate::lut_pipeline::LoadedLut,
) -> Option<(ID3D11Texture3D, ID3D11ShaderResourceView)> {
    let texture = &lut.texture;
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
    let texture3d = texture3d?;
    let srv = create_shader_resource_view(device, &texture3d)?;
    Some((texture3d, srv))
}

fn create_shader_resource_view<P0>(
    device: &ID3D11Device,
    resource: P0,
) -> Option<ID3D11ShaderResourceView>
where
    P0: windows::core::Param<windows::Win32::Graphics::Direct3D11::ID3D11Resource>,
{
    let mut srv = None;
    unsafe {
        device
            .CreateShaderResourceView(resource, None, Some(&mut srv))
            .ok()?;
    }
    srv
}
