use std::ffi::c_void;
use std::mem::{size_of, transmute};
use std::ptr;

use super::context_state::ShaderStageState;
use super::{Box3D, Vertex};
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::lut_pipeline::{BackBufferFormat, DirtyRect, ShaderConstantsCBuffer};

pub(super) type Hresult = i32;
pub(super) type ComPtr = *mut c_void;

pub(super) const S_OK: Hresult = 0;
pub(super) const DXGI_FORMAT_R32G32B32A32_FLOAT: u32 = 2;
pub(super) const D3D11_USAGE_DEFAULT: u32 = 0;
pub(super) const D3D11_BIND_VERTEX_BUFFER: u32 = 0x1;
pub(super) const D3D11_BIND_SHADER_RESOURCE: u32 = 0x8;
pub(super) const D3D11_BIND_CONSTANT_BUFFER: u32 = 0x4;
pub(super) const D3D11_RESOURCE_MISC_NONE: u32 = 0;
pub(super) const D3D11_INPUT_PER_VERTEX_DATA: u32 = 0;
pub(super) const D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP: u32 = 5;
pub(super) const D3D11_FILTER_MIN_MAG_MIP_POINT: u32 = 0;
pub(super) const D3D11_TEXTURE_ADDRESS_CLAMP: u32 = 3;
pub(super) const D3D11_COMPARISON_NEVER: u32 = 1;
pub(super) const D3D11_FLOAT32_MAX: f32 = f32::MAX;
pub(super) const D3D11_SHADER_CLASS_INSTANCE_LIMIT: usize = 256;
pub(super) const D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT: usize = 8;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Texture2DDesc {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) mip_levels: u32,
    pub(super) array_size: u32,
    pub(super) format: u32,
    pub(super) sample_count: u32,
    pub(super) sample_quality: u32,
    pub(super) usage: u32,
    pub(super) bind_flags: u32,
    pub(super) cpu_access_flags: u32,
    pub(super) misc_flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Texture3DDesc {
    pub(super) width: u32,
    pub(super) height: u32,
    depth: u32,
    pub(super) mip_levels: u32,
    pub(super) format: u32,
    pub(super) usage: u32,
    pub(super) bind_flags: u32,
    pub(super) cpu_access_flags: u32,
    pub(super) misc_flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct BufferDesc {
    byte_width: u32,
    pub(super) usage: u32,
    pub(super) bind_flags: u32,
    pub(super) cpu_access_flags: u32,
    pub(super) misc_flags: u32,
    structure_byte_stride: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct SubresourceData {
    sys_mem: *const c_void,
    sys_mem_pitch: u32,
    sys_mem_slice_pitch: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct InputElementDesc {
    semantic_name: *const u8,
    semantic_index: u32,
    pub(super) format: u32,
    input_slot: u32,
    aligned_byte_offset: u32,
    input_slot_class: u32,
    instance_data_step_rate: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct SamplerDesc {
    filter: u32,
    address_u: u32,
    address_v: u32,
    address_w: u32,
    mip_lod_bias: f32,
    max_anisotropy: u32,
    comparison_func: u32,
    border_color: [f32; 4],
    min_lod: f32,
    max_lod: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Viewport {
    pub(super) top_left_x: f32,
    pub(super) top_left_y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) min_depth: f32,
    pub(super) max_depth: f32,
}

#[cfg(debug_assertions)]
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct BackBuffer25H2Diagnostic {
    pub(super) stage: u32,
    pub(super) hresult: Hresult,
    pub(super) container: ComPtr,
    pub(super) resource: ComPtr,
    pub(super) texture: ComPtr,
}

#[cfg(debug_assertions)]
impl BackBuffer25H2Diagnostic {
    pub(super) const fn new() -> Self {
        Self {
            stage: 0,
            hresult: 0,
            container: ptr::null_mut(),
            resource: ptr::null_mut(),
            texture: ptr::null_mut(),
        }
    }
}

unsafe extern "system" {
    #[cfg(debug_assertions)]
    pub(super) fn dwm_lut_get_back_buffer_25h2_diagnostic(
        overlay_swap_chain: *mut c_void,
        container_vtable_index: usize,
        resource_vtable_index: usize,
        diagnostic: *mut BackBuffer25H2Diagnostic,
    ) -> ComPtr;
    #[cfg(not(debug_assertions))]
    pub(super) fn dwm_lut_get_back_buffer_25h2(
        overlay_swap_chain: *mut c_void,
        container_vtable_index: usize,
        resource_vtable_index: usize,
    ) -> ComPtr;
}

pub(super) type D3DCompileApi = unsafe extern "system" fn(
    src_data: *const c_void,
    src_data_size: usize,
    source_name: *const u8,
    defines: *const c_void,
    include: *mut c_void,
    entrypoint: *const u8,
    target: *const u8,
    flags1: u32,
    flags2: u32,
    code: *mut ComPtr,
    error_msgs: *mut ComPtr,
) -> Hresult;

pub(super) struct D3DCompiler {
    module: HMODULE,
    pub(super) compile: D3DCompileApi,
}

impl Drop for D3DCompiler {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.module);
        }
    }
}

pub(super) unsafe fn d3d11_device_get_immediate_context(device: ComPtr, context: *mut ComPtr) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr);
    let api: Api = unsafe {
        vtbl_fn(
            device,
            super::D3D11VtableIndex::DEVICE_GET_IMMEDIATE_CONTEXT,
        )
    };
    unsafe { api(device, context) };
}

pub(super) unsafe fn d3d11_device_child_get_device(child: ComPtr, device: *mut ComPtr) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr);
    let api: Api = unsafe { vtbl_fn(child, super::D3D11VtableIndex::DEVICE_CHILD_GET_DEVICE) };
    unsafe { api(child, device) };
}

pub(super) unsafe fn d3d11_texture2d_get_desc(texture: ComPtr, desc: *mut Texture2DDesc) {
    type Api = unsafe extern "system" fn(ComPtr, *mut Texture2DDesc);
    let api: Api = unsafe { vtbl_fn(texture, super::D3D11VtableIndex::TEXTURE2D_GET_DESC) };
    unsafe { api(texture, desc) };
}

pub(super) unsafe fn create_vertex_shader(device: ComPtr, blob: &Blob) -> Option<ComPtr> {
    type Api =
        unsafe extern "system" fn(ComPtr, *const c_void, usize, ComPtr, *mut ComPtr) -> Hresult;
    let api: Api = unsafe { vtbl_fn(device, 12) };
    let mut shader = ptr::null_mut();
    let hr = unsafe {
        api(
            device,
            blob.buffer_pointer(),
            blob.buffer_size(),
            ptr::null_mut(),
            &mut shader,
        )
    };
    (hr >= S_OK && !shader.is_null()).then_some(shader)
}

pub(super) unsafe fn create_pixel_shader(device: ComPtr, blob: &Blob) -> Option<ComPtr> {
    type Api =
        unsafe extern "system" fn(ComPtr, *const c_void, usize, ComPtr, *mut ComPtr) -> Hresult;
    let api: Api = unsafe { vtbl_fn(device, 15) };
    let mut shader = ptr::null_mut();
    let hr = unsafe {
        api(
            device,
            blob.buffer_pointer(),
            blob.buffer_size(),
            ptr::null_mut(),
            &mut shader,
        )
    };
    (hr >= S_OK && !shader.is_null()).then_some(shader)
}

pub(super) unsafe fn create_input_layout(device: ComPtr, blob: &Blob) -> Option<ComPtr> {
    type Api = unsafe extern "system" fn(
        ComPtr,
        *const InputElementDesc,
        u32,
        *const c_void,
        usize,
        *mut ComPtr,
    ) -> Hresult;
    const DXGI_FORMAT_R32G32_FLOAT: u32 = 16;
    const POSITION: &[u8] = b"POSITION\0";
    const TEXCOORD: &[u8] = b"TEXCOORD\0";
    let elements = [
        InputElementDesc {
            semantic_name: POSITION.as_ptr(),
            semantic_index: 0,
            format: DXGI_FORMAT_R32G32_FLOAT,
            input_slot: 0,
            aligned_byte_offset: 0,
            input_slot_class: D3D11_INPUT_PER_VERTEX_DATA,
            instance_data_step_rate: 0,
        },
        InputElementDesc {
            semantic_name: TEXCOORD.as_ptr(),
            semantic_index: 0,
            format: DXGI_FORMAT_R32G32_FLOAT,
            input_slot: 0,
            aligned_byte_offset: 8,
            input_slot_class: D3D11_INPUT_PER_VERTEX_DATA,
            instance_data_step_rate: 0,
        },
    ];
    let api: Api = unsafe { vtbl_fn(device, 11) };
    let mut layout = ptr::null_mut();
    let hr = unsafe {
        api(
            device,
            elements.as_ptr(),
            elements.len() as u32,
            blob.buffer_pointer(),
            blob.buffer_size(),
            &mut layout,
        )
    };
    (hr >= S_OK && !layout.is_null()).then_some(layout)
}

pub(super) unsafe fn create_vertex_buffer(device: ComPtr) -> Option<ComPtr> {
    let vertices = [Vertex {
        position: [0.0, 0.0],
        texcoord: [0.0, 0.0],
    }; 4];
    let desc = BufferDesc {
        byte_width: size_of::<[Vertex; 4]>() as u32,
        usage: D3D11_USAGE_DEFAULT,
        bind_flags: D3D11_BIND_VERTEX_BUFFER,
        cpu_access_flags: 0,
        misc_flags: D3D11_RESOURCE_MISC_NONE,
        structure_byte_stride: 0,
    };
    let data = SubresourceData {
        sys_mem: vertices.as_ptr().cast(),
        sys_mem_pitch: 0,
        sys_mem_slice_pitch: 0,
    };
    unsafe { create_buffer(device, &desc, Some(&data)) }
}

pub(super) unsafe fn create_constant_buffer(device: ComPtr) -> Option<ComPtr> {
    let desc = BufferDesc {
        byte_width: size_of::<ShaderConstantsCBuffer>() as u32,
        usage: D3D11_USAGE_DEFAULT,
        bind_flags: D3D11_BIND_CONSTANT_BUFFER,
        cpu_access_flags: 0,
        misc_flags: D3D11_RESOURCE_MISC_NONE,
        structure_byte_stride: 0,
    };
    unsafe { create_buffer(device, &desc, None) }
}

pub(super) unsafe fn create_buffer(
    device: ComPtr,
    desc: &BufferDesc,
    data: Option<&SubresourceData>,
) -> Option<ComPtr> {
    type Api = unsafe extern "system" fn(
        ComPtr,
        *const BufferDesc,
        *const SubresourceData,
        *mut ComPtr,
    ) -> Hresult;
    let api: Api = unsafe { vtbl_fn(device, 3) };
    let mut buffer = ptr::null_mut();
    let data = data.map_or(ptr::null(), |data| data as *const SubresourceData);
    let hr = unsafe { api(device, desc, data, &mut buffer) };
    (hr >= S_OK && !buffer.is_null()).then_some(buffer)
}

pub(super) unsafe fn create_sampler(device: ComPtr) -> Option<ComPtr> {
    type Api = unsafe extern "system" fn(ComPtr, *const SamplerDesc, *mut ComPtr) -> Hresult;
    let desc = SamplerDesc {
        filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
        address_u: D3D11_TEXTURE_ADDRESS_CLAMP,
        address_v: D3D11_TEXTURE_ADDRESS_CLAMP,
        address_w: D3D11_TEXTURE_ADDRESS_CLAMP,
        mip_lod_bias: 0.0,
        max_anisotropy: 1,
        comparison_func: D3D11_COMPARISON_NEVER,
        border_color: [0.0; 4],
        min_lod: 0.0,
        max_lod: D3D11_FLOAT32_MAX,
    };
    let api: Api = unsafe { vtbl_fn(device, 23) };
    let mut sampler = ptr::null_mut();
    let hr = unsafe { api(device, &desc, &mut sampler) };
    (hr >= S_OK && !sampler.is_null()).then_some(sampler)
}

pub(super) unsafe fn create_copy_texture(
    device: ComPtr,
    width: u32,
    height: u32,
    format: BackBufferFormat,
) -> Option<(OwnedCom, OwnedCom)> {
    let desc = Texture2DDesc {
        width,
        height,
        mip_levels: 1,
        array_size: 1,
        format: super::dxgi_format_for_copy_texture(format),
        sample_count: 1,
        sample_quality: 0,
        usage: D3D11_USAGE_DEFAULT,
        bind_flags: D3D11_BIND_SHADER_RESOURCE,
        cpu_access_flags: 0,
        misc_flags: 0,
    };
    let texture = OwnedCom::new(unsafe { create_texture2d(device, &desc, None) }?);
    let srv = OwnedCom::new(unsafe { create_shader_resource_view(device, texture.as_ptr()) }?);
    Some((texture, srv))
}

pub(super) unsafe fn create_lut_texture(
    device: ComPtr,
    lut: &crate::lut_pipeline::LoadedLut,
) -> Option<(OwnedCom, OwnedCom)> {
    let texture = &lut.texture;
    let desc = Texture3DDesc {
        width: texture.width,
        height: texture.height,
        depth: texture.depth,
        mip_levels: 1,
        format: DXGI_FORMAT_R32G32B32A32_FLOAT,
        usage: D3D11_USAGE_DEFAULT,
        bind_flags: D3D11_BIND_SHADER_RESOURCE,
        cpu_access_flags: 0,
        misc_flags: 0,
    };
    let row_pitch = texture.width * size_of::<[f32; 4]>() as u32;
    let data = SubresourceData {
        sys_mem: texture.texels.as_ptr().cast(),
        sys_mem_pitch: row_pitch,
        sys_mem_slice_pitch: row_pitch * texture.height,
    };
    let tex = OwnedCom::new(unsafe { create_texture3d(device, &desc, &data) }?);
    let srv = OwnedCom::new(unsafe { create_shader_resource_view(device, tex.as_ptr()) }?);
    Some((tex, srv))
}

pub(super) unsafe fn create_texture2d(
    device: ComPtr,
    desc: &Texture2DDesc,
    data: Option<&SubresourceData>,
) -> Option<ComPtr> {
    type Api = unsafe extern "system" fn(
        ComPtr,
        *const Texture2DDesc,
        *const SubresourceData,
        *mut ComPtr,
    ) -> Hresult;
    let api: Api = unsafe { vtbl_fn(device, 5) };
    let mut texture = ptr::null_mut();
    let data = data.map_or(ptr::null(), |data| data as *const SubresourceData);
    let hr = unsafe { api(device, desc, data, &mut texture) };
    (hr >= S_OK && !texture.is_null()).then_some(texture)
}

pub(super) unsafe fn create_texture3d(
    device: ComPtr,
    desc: &Texture3DDesc,
    data: &SubresourceData,
) -> Option<ComPtr> {
    type Api = unsafe extern "system" fn(
        ComPtr,
        *const Texture3DDesc,
        *const SubresourceData,
        *mut ComPtr,
    ) -> Hresult;
    let api: Api = unsafe { vtbl_fn(device, 6) };
    let mut texture = ptr::null_mut();
    let hr = unsafe { api(device, desc, data, &mut texture) };
    (hr >= S_OK && !texture.is_null()).then_some(texture)
}

pub(super) unsafe fn create_shader_resource_view(
    device: ComPtr,
    resource: ComPtr,
) -> Option<ComPtr> {
    type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const c_void, *mut ComPtr) -> Hresult;
    let api: Api = unsafe { vtbl_fn(device, 7) };
    let mut srv = ptr::null_mut();
    let hr = unsafe { api(device, resource, ptr::null(), &mut srv) };
    (hr >= S_OK && !srv.is_null()).then_some(srv)
}

pub(super) unsafe fn d3d11_device_create_render_target_view_from_context(
    context: ComPtr,
    resource: ComPtr,
    rtv: *mut ComPtr,
) -> Hresult {
    let mut device = ptr::null_mut();
    unsafe { d3d11_context_get_device(context, &mut device) };
    if device.is_null() {
        return -1;
    }
    type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const c_void, *mut ComPtr) -> Hresult;
    let api: Api = unsafe {
        vtbl_fn(
            device,
            super::D3D11VtableIndex::DEVICE_CREATE_RENDER_TARGET_VIEW,
        )
    };
    let hr = unsafe { api(device, resource, ptr::null(), rtv) };
    unsafe { release(device) };
    hr
}

pub(super) unsafe fn d3d11_context_get_device(context: ComPtr, device: *mut ComPtr) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 3) };
    unsafe { api(context, device) };
}

pub(super) unsafe fn d3d11_context_ia_set_input_layout(context: ComPtr, layout: ComPtr) {
    type Api = unsafe extern "system" fn(ComPtr, ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 17) };
    unsafe { api(context, layout) };
}

pub(super) unsafe fn d3d11_context_ia_get_input_layout(context: ComPtr, layout: *mut ComPtr) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 78) };
    unsafe { api(context, layout) };
}

pub(super) unsafe fn d3d11_context_ia_set_vertex_buffers(
    context: ComPtr,
    buffers: *const ComPtr,
    stride: *const u32,
    offset: *const u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, u32, u32, *const ComPtr, *const u32, *const u32);
    let api: Api = unsafe { vtbl_fn(context, 18) };
    unsafe { api(context, 0, 1, buffers, stride, offset) };
}

pub(super) unsafe fn d3d11_context_ia_get_vertex_buffers(
    context: ComPtr,
    buffer: *mut ComPtr,
    stride: *mut u32,
    offset: *mut u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, u32, u32, *mut ComPtr, *mut u32, *mut u32);
    let api: Api = unsafe { vtbl_fn(context, 79) };
    unsafe { api(context, 0, 1, buffer, stride, offset) };
}

pub(super) unsafe fn d3d11_context_ia_set_primitive_topology(context: ComPtr, topology: u32) {
    type Api = unsafe extern "system" fn(ComPtr, u32);
    let api: Api = unsafe { vtbl_fn(context, 24) };
    unsafe { api(context, topology) };
}

pub(super) unsafe fn d3d11_context_ia_get_primitive_topology(context: ComPtr, topology: *mut u32) {
    type Api = unsafe extern "system" fn(ComPtr, *mut u32);
    let api: Api = unsafe { vtbl_fn(context, 83) };
    unsafe { api(context, topology) };
}

pub(super) unsafe fn d3d11_context_vs_set_shader(
    context: ComPtr,
    shader: ComPtr,
    class_instances: *const ComPtr,
    class_instance_count: u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const ComPtr, u32);
    let api: Api = unsafe { vtbl_fn(context, 11) };
    unsafe { api(context, shader, class_instances, class_instance_count) };
}

pub(super) unsafe fn d3d11_context_vs_get_shader(context: ComPtr, state: &mut ShaderStageState) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut ComPtr, *mut u32);
    let api: Api = unsafe { vtbl_fn(context, 76) };
    unsafe {
        api(
            context,
            &mut state.shader,
            state.class_instances.as_mut_ptr(),
            &mut state.class_instance_count,
        )
    };
}

pub(super) unsafe fn d3d11_context_gs_set_shader(
    context: ComPtr,
    shader: ComPtr,
    class_instances: *const ComPtr,
    class_instance_count: u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const ComPtr, u32);
    let api: Api = unsafe { vtbl_fn(context, 23) };
    unsafe { api(context, shader, class_instances, class_instance_count) };
}

pub(super) unsafe fn d3d11_context_gs_get_shader(context: ComPtr, state: &mut ShaderStageState) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut ComPtr, *mut u32);
    let api: Api = unsafe { vtbl_fn(context, 82) };
    unsafe {
        api(
            context,
            &mut state.shader,
            state.class_instances.as_mut_ptr(),
            &mut state.class_instance_count,
        )
    };
}

pub(super) unsafe fn d3d11_context_hs_set_shader(
    context: ComPtr,
    shader: ComPtr,
    class_instances: *const ComPtr,
    class_instance_count: u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const ComPtr, u32);
    let api: Api = unsafe { vtbl_fn(context, 60) };
    unsafe { api(context, shader, class_instances, class_instance_count) };
}

pub(super) unsafe fn d3d11_context_hs_get_shader(context: ComPtr, state: &mut ShaderStageState) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut ComPtr, *mut u32);
    let api: Api = unsafe { vtbl_fn(context, 98) };
    unsafe {
        api(
            context,
            &mut state.shader,
            state.class_instances.as_mut_ptr(),
            &mut state.class_instance_count,
        )
    };
}

pub(super) unsafe fn d3d11_context_ds_set_shader(
    context: ComPtr,
    shader: ComPtr,
    class_instances: *const ComPtr,
    class_instance_count: u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const ComPtr, u32);
    let api: Api = unsafe { vtbl_fn(context, 64) };
    unsafe { api(context, shader, class_instances, class_instance_count) };
}

pub(super) unsafe fn d3d11_context_ds_get_shader(context: ComPtr, state: &mut ShaderStageState) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut ComPtr, *mut u32);
    let api: Api = unsafe { vtbl_fn(context, 102) };
    unsafe {
        api(
            context,
            &mut state.shader,
            state.class_instances.as_mut_ptr(),
            &mut state.class_instance_count,
        )
    };
}

pub(super) unsafe fn d3d11_context_ps_set_shader(
    context: ComPtr,
    shader: ComPtr,
    class_instances: *const ComPtr,
    class_instance_count: u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const ComPtr, u32);
    let api: Api = unsafe { vtbl_fn(context, 9) };
    unsafe { api(context, shader, class_instances, class_instance_count) };
}

pub(super) unsafe fn d3d11_context_ps_get_shader(context: ComPtr, state: &mut ShaderStageState) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut ComPtr, *mut u32);
    let api: Api = unsafe { vtbl_fn(context, 74) };
    unsafe {
        api(
            context,
            &mut state.shader,
            state.class_instances.as_mut_ptr(),
            &mut state.class_instance_count,
        )
    };
}

pub(super) unsafe fn d3d11_context_ps_set_shader_resources(
    context: ComPtr,
    srvs: *const ComPtr,
    count: u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, u32, u32, *const ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 8) };
    unsafe { api(context, 0, count, srvs) };
}

pub(super) unsafe fn d3d11_context_ps_get_shader_resources(
    context: ComPtr,
    srvs: *mut ComPtr,
    count: u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, u32, u32, *mut ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 73) };
    unsafe { api(context, 0, count, srvs) };
}

pub(super) unsafe fn d3d11_context_ps_set_samplers(context: ComPtr, samplers: *const ComPtr) {
    type Api = unsafe extern "system" fn(ComPtr, u32, u32, *const ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 10) };
    unsafe { api(context, 0, 1, samplers) };
}

pub(super) unsafe fn d3d11_context_ps_get_samplers(context: ComPtr, samplers: *mut ComPtr) {
    type Api = unsafe extern "system" fn(ComPtr, u32, u32, *mut ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 75) };
    unsafe { api(context, 0, 1, samplers) };
}

pub(super) unsafe fn d3d11_context_vs_set_constant_buffers(
    context: ComPtr,
    buffers: *const ComPtr,
) {
    type Api = unsafe extern "system" fn(ComPtr, u32, u32, *const ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 7) };
    unsafe { api(context, 0, 1, buffers) };
}

pub(super) unsafe fn d3d11_context_vs_get_constant_buffers(context: ComPtr, buffers: *mut ComPtr) {
    type Api = unsafe extern "system" fn(ComPtr, u32, u32, *mut ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 72) };
    unsafe { api(context, 0, 1, buffers) };
}

pub(super) unsafe fn d3d11_context_ps_set_constant_buffers(
    context: ComPtr,
    buffers: *const ComPtr,
) {
    type Api = unsafe extern "system" fn(ComPtr, u32, u32, *const ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 16) };
    unsafe { api(context, 0, 1, buffers) };
}

pub(super) unsafe fn d3d11_context_ps_get_constant_buffers(context: ComPtr, buffers: *mut ComPtr) {
    type Api = unsafe extern "system" fn(ComPtr, u32, u32, *mut ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 77) };
    unsafe { api(context, 0, 1, buffers) };
}

pub(super) unsafe fn d3d11_context_om_set_render_targets(
    context: ComPtr,
    count: u32,
    rtvs: *const ComPtr,
) {
    type Api = unsafe extern "system" fn(ComPtr, u32, *const ComPtr, ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 33) };
    unsafe { api(context, count, rtvs, ptr::null_mut()) };
}

pub(super) unsafe fn d3d11_context_om_set_render_targets_with_depth(
    context: ComPtr,
    count: u32,
    rtvs: *const ComPtr,
    dsv: ComPtr,
) {
    type Api = unsafe extern "system" fn(ComPtr, u32, *const ComPtr, ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 33) };
    unsafe { api(context, count, rtvs, dsv) };
}

pub(super) unsafe fn d3d11_context_om_get_render_targets(
    context: ComPtr,
    rtv: *mut ComPtr,
    dsv: *mut ComPtr,
) {
    type Api = unsafe extern "system" fn(ComPtr, u32, *mut ComPtr, *mut ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 89) };
    unsafe {
        api(
            context,
            D3D11_SIMULTANEOUS_RENDER_TARGET_COUNT as u32,
            rtv,
            dsv,
        )
    };
}

pub(super) unsafe fn d3d11_context_om_set_blend_state(
    context: ComPtr,
    blend_state: ComPtr,
    blend_factor: *const f32,
    sample_mask: u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, ComPtr, *const f32, u32);
    let api: Api = unsafe { vtbl_fn(context, 35) };
    unsafe { api(context, blend_state, blend_factor, sample_mask) };
}

pub(super) unsafe fn d3d11_context_om_get_blend_state(
    context: ComPtr,
    blend_state: *mut ComPtr,
    blend_factor: *mut f32,
    sample_mask: *mut u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut f32, *mut u32);
    let api: Api = unsafe { vtbl_fn(context, 91) };
    unsafe { api(context, blend_state, blend_factor, sample_mask) };
}

pub(super) unsafe fn d3d11_context_om_set_depth_stencil_state(
    context: ComPtr,
    depth_stencil_state: ComPtr,
    stencil_ref: u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, ComPtr, u32);
    let api: Api = unsafe { vtbl_fn(context, 36) };
    unsafe { api(context, depth_stencil_state, stencil_ref) };
}

pub(super) unsafe fn d3d11_context_om_get_depth_stencil_state(
    context: ComPtr,
    depth_stencil_state: *mut ComPtr,
    stencil_ref: *mut u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr, *mut u32);
    let api: Api = unsafe { vtbl_fn(context, 92) };
    unsafe { api(context, depth_stencil_state, stencil_ref) };
}

pub(super) unsafe fn d3d11_context_rs_set_state(context: ComPtr, rasterizer_state: ComPtr) {
    type Api = unsafe extern "system" fn(ComPtr, ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 43) };
    unsafe { api(context, rasterizer_state) };
}

pub(super) unsafe fn d3d11_context_rs_get_state(context: ComPtr, rasterizer_state: *mut ComPtr) {
    type Api = unsafe extern "system" fn(ComPtr, *mut ComPtr);
    let api: Api = unsafe { vtbl_fn(context, 94) };
    unsafe { api(context, rasterizer_state) };
}

pub(super) unsafe fn d3d11_context_rs_set_viewports(context: ComPtr, viewport: *const Viewport) {
    type Api = unsafe extern "system" fn(ComPtr, u32, *const Viewport);
    let api: Api = unsafe { vtbl_fn(context, 44) };
    unsafe { api(context, 1, viewport) };
}

pub(super) unsafe fn d3d11_context_rs_set_viewports_count(
    context: ComPtr,
    count: u32,
    viewports: *const Viewport,
) {
    type Api = unsafe extern "system" fn(ComPtr, u32, *const Viewport);
    let api: Api = unsafe { vtbl_fn(context, 44) };
    unsafe { api(context, count, viewports) };
}

pub(super) unsafe fn d3d11_context_rs_get_viewports(
    context: ComPtr,
    count: *mut u32,
    viewports: *mut Viewport,
) {
    type Api = unsafe extern "system" fn(ComPtr, *mut u32, *mut Viewport);
    let api: Api = unsafe { vtbl_fn(context, 95) };
    unsafe { api(context, count, viewports) };
}

pub(super) unsafe fn d3d11_context_rs_set_scissor_rects(
    context: ComPtr,
    rects: *const DirtyRect,
    count: u32,
) {
    type Api = unsafe extern "system" fn(ComPtr, u32, *const DirtyRect);
    let api: Api = unsafe { vtbl_fn(context, 45) };
    unsafe { api(context, count, rects) };
}

pub(super) unsafe fn d3d11_context_rs_get_scissor_rects(
    context: ComPtr,
    count: *mut u32,
    rects: *mut DirtyRect,
) {
    type Api = unsafe extern "system" fn(ComPtr, *mut u32, *mut DirtyRect);
    let api: Api = unsafe { vtbl_fn(context, 96) };
    unsafe { api(context, count, rects) };
}

pub(super) unsafe fn d3d11_context_copy_subresource_region(
    context: ComPtr,
    dst: ComPtr,
    dst_x: u32,
    dst_y: u32,
    src: ComPtr,
    src_box: *const Box3D,
) {
    type Api =
        unsafe extern "system" fn(ComPtr, ComPtr, u32, u32, u32, u32, ComPtr, u32, *const Box3D);
    let api: Api = unsafe {
        vtbl_fn(
            context,
            super::D3D11VtableIndex::CONTEXT_COPY_SUBRESOURCE_REGION,
        )
    };
    unsafe { api(context, dst, 0, dst_x, dst_y, 0, src, 0, src_box) };
}

pub(super) unsafe fn d3d11_context_update_subresource(
    context: ComPtr,
    resource: ComPtr,
    data: *const c_void,
    row_pitch: u32,
    depth_pitch: u32,
) {
    type Api =
        unsafe extern "system" fn(ComPtr, ComPtr, u32, *const c_void, *const c_void, u32, u32);
    let api: Api = unsafe { vtbl_fn(context, super::D3D11VtableIndex::CONTEXT_UPDATE_SUBRESOURCE) };
    unsafe {
        api(
            context,
            resource,
            0,
            ptr::null(),
            data,
            row_pitch,
            depth_pitch,
        )
    };
}

pub(super) unsafe fn d3d11_context_draw(context: ComPtr, vertex_count: u32, start_vertex: u32) {
    type Api = unsafe extern "system" fn(ComPtr, u32, u32);
    let api: Api = unsafe { vtbl_fn(context, 13) };
    unsafe { api(context, vertex_count, start_vertex) };
}

pub(super) unsafe fn load_compiler() -> Option<D3DCompiler> {
    let wide: Vec<u16> = "d3dcompiler_47.dll"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let module = unsafe { LoadLibraryW(wide.as_ptr()) };
    if module.is_null() {
        return None;
    }
    let proc = unsafe { GetProcAddress(module, c"D3DCompile".as_ptr().cast()) };
    let Some(proc) = proc else {
        unsafe { FreeLibrary(module) };
        return None;
    };
    Some(D3DCompiler {
        module,
        compile: unsafe { transmute::<unsafe extern "system" fn() -> isize, D3DCompileApi>(proc) },
    })
}

pub(super) unsafe fn compile_shader(
    compile: D3DCompileApi,
    source: &str,
    entry: &str,
    profile: &str,
) -> Option<Blob> {
    let entry = nul_bytes(entry);
    let profile = nul_bytes(profile);
    let mut blob = ptr::null_mut();
    let mut errors = ptr::null_mut();
    let hr = unsafe {
        compile(
            source.as_ptr().cast(),
            source.len(),
            ptr::null(),
            ptr::null(),
            ptr::null_mut(),
            entry.as_ptr(),
            profile.as_ptr(),
            0,
            0,
            &mut blob,
            &mut errors,
        )
    };
    if !errors.is_null() {
        unsafe { release(errors) };
    }
    (hr >= S_OK && !blob.is_null()).then_some(Blob(blob))
}

fn nul_bytes(value: &str) -> Vec<u8> {
    value.bytes().chain(std::iter::once(0)).collect()
}

pub(super) struct Blob(ComPtr);

impl Blob {
    unsafe fn buffer_pointer(&self) -> *const c_void {
        type Api = unsafe extern "system" fn(ComPtr) -> *const c_void;
        let api: Api = unsafe { vtbl_fn(self.0, 3) };
        unsafe { api(self.0) }
    }

    unsafe fn buffer_size(&self) -> usize {
        type Api = unsafe extern "system" fn(ComPtr) -> usize;
        let api: Api = unsafe { vtbl_fn(self.0, 4) };
        unsafe { api(self.0) }
    }
}

impl Drop for Blob {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

pub(super) struct OwnedCom(ComPtr);

impl OwnedCom {
    pub(super) fn new(object: ComPtr) -> Self {
        Self(object)
    }

    pub(super) fn as_ptr(&self) -> ComPtr {
        self.0
    }

    pub(super) fn into_raw(mut self) -> ComPtr {
        let object = self.0;
        self.0 = ptr::null_mut();
        object
    }
}

impl Drop for OwnedCom {
    fn drop(&mut self) {
        unsafe { release(self.0) };
    }
}

pub(super) unsafe fn vtbl_fn<T>(object: ComPtr, index: usize) -> T {
    let vtbl = unsafe { *(object as *const *const usize) };
    let slot = unsafe { *vtbl.add(index) };
    unsafe { transmute_copy_usize(slot) }
}

pub(super) unsafe fn transmute_copy_usize<T>(value: usize) -> T {
    unsafe { std::mem::transmute_copy(&value) }
}

pub(super) unsafe fn release(object: ComPtr) {
    if object.is_null() {
        return;
    }
    type Release = unsafe extern "system" fn(ComPtr) -> u32;
    let release: Release = unsafe { vtbl_fn(object, 2) };
    unsafe {
        release(object);
    }
}
