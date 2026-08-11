use std::ffi::c_void;
use std::mem::{align_of, size_of, transmute_copy};
use std::ptr;

use dwm_lut_payload::{AdapterLuid, MonitorIdentity};
use dwm_lut_profile::{
    D3D11ResourceAccess, HardwareProtectedAccess, MonitorIdentityAccess,
    OverlaySwapChainFromContext,
};
use windows::Win32::Foundation::{LUID, RECT};
use windows::Win32::Graphics::Direct3D11::{ID3D11Resource, ID3D11Texture2D, ID3D11View};
use windows::core::{GUID, IUnknown, Interface};

const MAX_DIRTY_RECTS: usize = 4096;

pub type DirtyRect = RECT;

/// # Safety
///
/// `overlay_context` must point to a live object compatible with `path`.
pub(crate) unsafe fn resolve_overlay_swap_chain(
    overlay_context: usize,
    path: OverlaySwapChainFromContext,
) -> Option<usize> {
    if overlay_context == 0 {
        return None;
    }

    microseh::try_seh(|| unsafe { resolve_overlay_swap_chain_unguarded(overlay_context, path) })
        .unwrap_or_default()
}

unsafe fn resolve_overlay_swap_chain_unguarded(
    overlay_context: usize,
    path: OverlaySwapChainFromContext,
) -> Option<usize> {
    let monitor = unsafe {
        read::<usize>(address_with_offset(
            overlay_context,
            path.overlay_monitor_target_pointer_offset,
        )?)?
    };
    if monitor == 0 {
        return None;
    }

    type GetOverlaySwapChain = unsafe extern "system" fn(usize) -> usize;
    let get_overlay_swap_chain = unsafe {
        object_vtable_fn::<GetOverlaySwapChain>(monitor, path.get_overlay_swap_chain_vtable_index)?
    };
    let swap_chain = unsafe { get_overlay_swap_chain(monitor) };
    (swap_chain != 0).then_some(swap_chain)
}

/// # Safety
///
/// `overlay_swap_chain` must point to a live object compatible with `access`.
pub(crate) unsafe fn resolve_d3d11_texture2d(
    overlay_swap_chain: usize,
    access: D3D11ResourceAccess,
) -> Result<ID3D11Texture2D, D3D11TextureResolveError> {
    if overlay_swap_chain == 0 {
        return Err(D3D11TextureResolveError::UnreadableMemory);
    }

    match access {
        D3D11ResourceAccess::PhysicalBackBuffer {
            get_physical_back_buffer_vtable_index,
            get_d3d11_resource_vtable_index,
        } => {
            let texture = match microseh::try_seh(|| unsafe {
                resolve_physical_back_buffer_texture2d(
                    overlay_swap_chain,
                    get_physical_back_buffer_vtable_index,
                    get_d3d11_resource_vtable_index,
                )
            }) {
                Ok(Ok(texture)) => texture,
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err(D3D11TextureResolveError::UnreadableMemory),
            };
            Ok(unsafe { ID3D11Texture2D::from_raw(texture) })
        }
        D3D11ResourceAccess::CurrentBackBufferRenderTargetView {
            get_current_back_buffer_vtable_index,
            get_render_target_view_vtable_index,
        } => {
            let resource = match microseh::try_seh(|| unsafe {
                resolve_current_back_buffer_d3d11_resource(
                    overlay_swap_chain,
                    get_current_back_buffer_vtable_index,
                    get_render_target_view_vtable_index,
                )
            }) {
                Ok(Ok(resource)) => resource,
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err(D3D11TextureResolveError::UnreadableMemory),
            };
            let resource = unsafe { ID3D11Resource::from_raw(resource) };
            query_texture2d(resource.as_raw())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum D3D11TextureResolveError {
    UnreadableMemory,
    MissingObject,
    QueryInterface,
}

unsafe fn resolve_physical_back_buffer_texture2d(
    overlay_swap_chain: usize,
    get_physical_back_buffer_vtable_index: usize,
    get_d3d11_resource_vtable_index: usize,
) -> Result<*mut c_void, D3D11TextureResolveError> {
    type GetPhysicalBackBuffer = unsafe extern "system" fn(usize) -> usize;
    type GetD3D11Resource = unsafe extern "system" fn(usize) -> usize;

    let get_physical_back_buffer = unsafe {
        object_vtable_fn::<GetPhysicalBackBuffer>(
            overlay_swap_chain,
            get_physical_back_buffer_vtable_index,
        )
    }
    .ok_or(D3D11TextureResolveError::UnreadableMemory)?;
    let physical_back_buffer = unsafe { get_physical_back_buffer(overlay_swap_chain) };
    if physical_back_buffer == 0 {
        return Err(D3D11TextureResolveError::MissingObject);
    }

    let get_d3d11_resource = unsafe {
        object_vtable_fn::<GetD3D11Resource>(physical_back_buffer, get_d3d11_resource_vtable_index)
    }
    .ok_or(D3D11TextureResolveError::UnreadableMemory)?;
    let d3d11_resource = unsafe { get_d3d11_resource(physical_back_buffer) };
    if d3d11_resource == 0 {
        return Err(D3D11TextureResolveError::MissingObject);
    }
    unsafe { query_interface_raw(d3d11_resource as *mut c_void, &ID3D11Texture2D::IID) }
        .ok_or(D3D11TextureResolveError::QueryInterface)
}

unsafe fn resolve_current_back_buffer_d3d11_resource(
    overlay_swap_chain: usize,
    get_current_back_buffer_vtable_index: usize,
    get_render_target_view_vtable_index: usize,
) -> Result<*mut c_void, D3D11TextureResolveError> {
    type GetCurrentBackBuffer = unsafe extern "system" fn(usize) -> usize;
    type GetRenderTargetView = unsafe extern "system" fn(usize) -> usize;

    let get_current_back_buffer = unsafe {
        object_vtable_fn::<GetCurrentBackBuffer>(
            overlay_swap_chain,
            get_current_back_buffer_vtable_index,
        )
    }
    .ok_or(D3D11TextureResolveError::UnreadableMemory)?;
    let device_target = unsafe { get_current_back_buffer(overlay_swap_chain) };
    if device_target == 0 {
        return Err(D3D11TextureResolveError::MissingObject);
    }

    let get_render_target_view = unsafe {
        object_vtable_fn::<GetRenderTargetView>(device_target, get_render_target_view_vtable_index)
    }
    .ok_or(D3D11TextureResolveError::UnreadableMemory)?;
    let render_target_view = unsafe { get_render_target_view(device_target) };
    if render_target_view == 0 {
        return Err(D3D11TextureResolveError::MissingObject);
    }

    let raw_view = render_target_view as *mut c_void;
    let view = unsafe { ID3D11View::from_raw_borrowed(&raw_view) }
        .ok_or(D3D11TextureResolveError::UnreadableMemory)?;
    let mut resource = ptr::null_mut();
    unsafe {
        (Interface::vtable(view).GetResource)(Interface::as_raw(view), &mut resource);
    }
    if resource.is_null() {
        return Err(D3D11TextureResolveError::MissingObject);
    }
    Ok(resource)
}

fn query_texture2d(resource: *mut c_void) -> Result<ID3D11Texture2D, D3D11TextureResolveError> {
    if resource.is_null() {
        return Err(D3D11TextureResolveError::MissingObject);
    }
    let texture =
        match microseh::try_seh(|| unsafe { query_interface_raw(resource, &ID3D11Texture2D::IID) })
        {
            Ok(Some(texture)) => texture,
            Ok(None) => return Err(D3D11TextureResolveError::QueryInterface),
            Err(_) => return Err(D3D11TextureResolveError::UnreadableMemory),
        };
    Ok(unsafe { ID3D11Texture2D::from_raw(texture) })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MonitorIdentityReadError {
    UnreadableMemory,
}

/// # Safety
///
/// `overlay_swap_chain` must point to a live object compatible with `access`.
pub(crate) unsafe fn read_monitor_identity(
    overlay_swap_chain: usize,
    access: MonitorIdentityAccess,
) -> Result<MonitorIdentity, MonitorIdentityReadError> {
    if overlay_swap_chain == 0 {
        return Err(MonitorIdentityReadError::UnreadableMemory);
    }
    match microseh::try_seh(|| unsafe {
        read_monitor_identity_virtual(
            overlay_swap_chain,
            access.get_display_adapter_luid_vtable_index,
            access.get_vid_pn_target_id_vtable_index,
        )
    }) {
        Ok(Some(identity)) => Ok(identity),
        Ok(None) | Err(_) => Err(MonitorIdentityReadError::UnreadableMemory),
    }
}

unsafe fn read_monitor_identity_virtual(
    overlay_swap_chain: usize,
    get_display_adapter_luid_vtable_index: usize,
    get_vid_pn_target_id_vtable_index: usize,
) -> Option<MonitorIdentity> {
    type GetDisplayAdapterLuid = unsafe extern "system" fn(usize, *mut LUID) -> *mut LUID;
    type GetVidPnTargetId = unsafe extern "system" fn(usize) -> u32;

    let get_display_adapter_luid = unsafe {
        object_vtable_fn::<GetDisplayAdapterLuid>(
            overlay_swap_chain,
            get_display_adapter_luid_vtable_index,
        )?
    };
    let get_vid_pn_target_id = unsafe {
        object_vtable_fn::<GetVidPnTargetId>(overlay_swap_chain, get_vid_pn_target_id_vtable_index)?
    };
    let mut luid = LUID::default();
    let returned = unsafe { get_display_adapter_luid(overlay_swap_chain, &mut luid) };
    if returned.is_null() {
        return None;
    }
    let target_id = unsafe { get_vid_pn_target_id(overlay_swap_chain) };
    Some(MonitorIdentity {
        adapter_luid: AdapterLuid {
            high_part: luid.HighPart,
            low_part: luid.LowPart,
        },
        target_id,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HardwareProtectedReadError {
    UnreadableMemory,
}

/// # Safety
///
/// `overlay_swap_chain` must point to a live object compatible with `access`.
pub(crate) unsafe fn read_hardware_protected(
    overlay_swap_chain: usize,
    access: HardwareProtectedAccess,
) -> Result<bool, HardwareProtectedReadError> {
    if overlay_swap_chain == 0 {
        return Err(HardwareProtectedReadError::UnreadableMemory);
    }
    match microseh::try_seh(|| match access {
        HardwareProtectedAccess::DirectField { offset } => {
            let address = address_with_offset(overlay_swap_chain, offset)?;
            unsafe { read::<u8>(address).map(|value| value != 0) }
        }
        HardwareProtectedAccess::VirtualBaseDeviceResource {
            virtual_base,
            is_hardware_protected_vtable_index,
        } => unsafe {
            read_hardware_protected_via_device_resource(
                overlay_swap_chain,
                virtual_base.vbptr_offset,
                virtual_base.vbtable_entry_offset,
                is_hardware_protected_vtable_index,
            )
        },
    }) {
        Ok(Some(value)) => Ok(value),
        Ok(None) | Err(_) => Err(HardwareProtectedReadError::UnreadableMemory),
    }
}

unsafe fn read_hardware_protected_via_device_resource(
    overlay_swap_chain: usize,
    vbptr_offset: i32,
    vbtable_entry_offset: usize,
    is_hardware_protected_vtable_index: usize,
) -> Option<bool> {
    type IsHardwareProtected = unsafe extern "system" fn(usize) -> u8;

    let vbptr_address = address_with_offset(overlay_swap_chain, vbptr_offset)?;
    let vbtable = unsafe { read::<usize>(vbptr_address)? };
    let displacement_address = vbtable.checked_add(vbtable_entry_offset)?;
    let device_resource_displacement = unsafe { read::<i32>(displacement_address)? };
    let device_resource =
        vbptr_address.checked_add_signed(device_resource_displacement as isize)?;
    let is_hardware_protected = unsafe {
        object_vtable_fn::<IsHardwareProtected>(
            device_resource,
            is_hardware_protected_vtable_index,
        )?
    };
    Some(unsafe { is_hardware_protected(device_resource) } != 0)
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StdVector<T> {
    pub(crate) first: *const T,
    pub(crate) last: *const T,
    pub(crate) end: *const T,
}

pub(crate) type RectVec = StdVector<DirtyRect>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StdVectorReadError {
    InvalidVector,
    UnreadableMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirtyRectReadError {
    InvalidVector,
    UnreadableMemory,
}

impl From<StdVectorReadError> for DirtyRectReadError {
    fn from(error: StdVectorReadError) -> Self {
        match error {
            StdVectorReadError::InvalidVector => Self::InvalidVector,
            StdVectorReadError::UnreadableMemory => Self::UnreadableMemory,
        }
    }
}

/// # Safety
///
/// `address` must point to a readable MSVC `std::vector<T>` header, and the
/// element range `[first, last)` must remain stable for this call.
pub(crate) unsafe fn read_std_vector<T: Copy>(
    address: usize,
    max_len: usize,
) -> Result<Vec<T>, StdVectorReadError> {
    if address == 0 {
        return Err(StdVectorReadError::InvalidVector);
    }

    let vector = microseh::try_seh(|| unsafe { read::<StdVector<T>>(address) })
        .map_err(|_| StdVectorReadError::UnreadableMemory)?
        .ok_or(StdVectorReadError::UnreadableMemory)?;

    let first = vector.first as usize;
    let last = vector.last as usize;
    let end = vector.end as usize;
    if first == 0 && last == 0 && end == 0 {
        return Ok(Vec::new());
    }
    if first == 0 || last < first || end < last || !first.is_multiple_of(align_of::<T>()) {
        return Err(StdVectorReadError::InvalidVector);
    }

    let byte_len = last - first;
    if !byte_len.is_multiple_of(size_of::<T>()) || !(end - first).is_multiple_of(size_of::<T>()) {
        return Err(StdVectorReadError::InvalidVector);
    }

    let count = byte_len / size_of::<T>();
    if count > max_len {
        return Err(StdVectorReadError::InvalidVector);
    }

    let mut values = Vec::with_capacity(count);
    if count != 0 {
        let destination = values.as_mut_ptr();
        microseh::try_seh(|| unsafe {
            ptr::copy_nonoverlapping(first as *const T, destination, count);
        })
        .map_err(|_| StdVectorReadError::UnreadableMemory)?;
        unsafe {
            values.set_len(count);
        }
    }
    Ok(values)
}

/// # Safety
///
/// `rect_vec` must point to a valid dirty-rect vector whose storage remains stable for this call.
pub(crate) unsafe fn read_dirty_rects(
    rect_vec: usize,
) -> Result<Vec<DirtyRect>, DirtyRectReadError> {
    unsafe { read_std_vector::<DirtyRect>(rect_vec, MAX_DIRTY_RECTS) }.map_err(Into::into)
}

unsafe fn query_interface_raw(object: *mut c_void, iid: &GUID) -> Option<*mut c_void> {
    let unknown = unsafe { IUnknown::from_raw_borrowed(&object)? };
    let mut result_object = ptr::null_mut();
    let result = unsafe {
        (Interface::vtable(unknown).QueryInterface)(
            Interface::as_raw(unknown),
            iid,
            &mut result_object,
        )
    };
    (result.is_ok() && !result_object.is_null()).then_some(result_object)
}

unsafe fn object_vtable_fn<T>(object: usize, index: usize) -> Option<T> {
    debug_assert_eq!(size_of::<T>(), size_of::<usize>());
    let vtable = unsafe { read::<usize>(object)? };
    if vtable == 0 {
        return None;
    }
    let method_offset = index.checked_mul(size_of::<usize>())?;
    let method = unsafe { read::<usize>(vtable.checked_add(method_offset)?)? };
    if method == 0 {
        return None;
    }
    Some(unsafe { transmute_copy(&method) })
}

fn address_with_offset(address: usize, offset: i32) -> Option<usize> {
    address.checked_add_signed(offset as isize)
}

unsafe fn read<T: Copy>(address: usize) -> Option<T> {
    if address == 0 {
        return None;
    }
    Some(unsafe { (address as *const T).read_unaligned() })
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::mem::{align_of, offset_of, size_of};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use dwm_lut_payload::{AdapterLuid, MonitorIdentity};
    use dwm_lut_profile::{
        D3D11ResourceAccess, DeviceResourceVirtualBaseAccess, HardwareProtectedAccess,
        MonitorIdentityAccess,
    };
    use windows::Win32::Foundation::LUID;
    use windows::Win32::Graphics::Direct3D11::ID3D11View_Vtbl;
    use windows::core::{GUID, HRESULT, Interface};

    use super::{
        D3D11TextureResolveError, DirtyRect, DirtyRectReadError, HardwareProtectedReadError,
        MonitorIdentityReadError, RectVec, read_dirty_rects, read_hardware_protected,
        read_monitor_identity, resolve_d3d11_texture2d, resolve_overlay_swap_chain,
    };
    use crate::dwmcore::test_support::{
        FakeOverlayContext, fake_overlay_swap_chain_with_hardware_protected, overlay_path_profile,
    };

    fn test_identity() -> MonitorIdentity {
        MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x1234,
            },
            target_id: 7,
        }
    }

    #[repr(C)]
    struct FakeComObject {
        vtable: *const usize,
        next: usize,
    }

    fn fake_com_object(methods: &[(usize, usize)], next: usize) -> (Box<[usize]>, FakeComObject) {
        let vtable_len = methods
            .iter()
            .map(|(index, _)| index + 1)
            .max()
            .unwrap_or(1);
        let mut vtable = vec![0usize; vtable_len].into_boxed_slice();
        for &(index, method) in methods {
            vtable[index] = method;
        }
        let object = FakeComObject {
            vtable: vtable.as_ptr(),
            next,
        };
        (vtable, object)
    }

    #[repr(C)]
    struct FakeIdentityObject {
        vtable: *const usize,
        luid: LUID,
        target_id: u32,
    }

    #[repr(C)]
    struct FakeVirtualBaseObject {
        overlay_vtable: *const usize,
        vbtable: *const i32,
        padding: usize,
        device_resource: FakeComObject,
    }

    #[repr(C)]
    struct FakeNegativeVirtualBaseObject {
        overlay_vtable: *const usize,
        device_resource: FakeComObject,
        vbtable: *const i32,
    }

    static RELEASE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static TEXTURE_RELEASE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static PHYSICAL_RESOURCE_RELEASE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static PHYSICAL_TEXTURE_RELEASE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static CURRENT_BACK_BUFFER_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "system" fn fake_get_next(this: usize) -> usize {
        unsafe { (*(this as *const FakeComObject)).next }
    }

    unsafe extern "system" fn fake_get_current_back_buffer(_this: usize) -> usize {
        CURRENT_BACK_BUFFER_CALLS.fetch_add(1, Ordering::Relaxed);
        0
    }

    unsafe extern "system" fn fake_get_resource(this: usize, output: *mut usize) {
        unsafe {
            output.write((*(this as *const FakeComObject)).next);
        }
    }

    unsafe extern "system" fn fake_query_interface(
        this: usize,
        _iid: *const GUID,
        output: *mut *mut c_void,
    ) -> HRESULT {
        unsafe {
            output.write((*(this as *const FakeComObject)).next as *mut c_void);
        }
        HRESULT(0)
    }

    unsafe extern "system" fn fake_get_display_adapter_luid(
        this: usize,
        out: *mut LUID,
    ) -> *mut LUID {
        unsafe {
            out.write((*(this as *const FakeIdentityObject)).luid);
            out
        }
    }

    unsafe extern "system" fn fake_get_vid_pn_target_id(this: usize) -> u32 {
        unsafe { (*(this as *const FakeIdentityObject)).target_id }
    }

    unsafe extern "system" fn fake_is_hardware_protected(this: usize) -> u8 {
        unsafe { (*(this as *const FakeComObject)).next as u8 }
    }

    unsafe extern "system" fn fake_release(_this: usize) -> u32 {
        RELEASE_CALLS.fetch_add(1, Ordering::Relaxed);
        1
    }

    unsafe extern "system" fn fake_texture_release(_this: usize) -> u32 {
        TEXTURE_RELEASE_CALLS.fetch_add(1, Ordering::Relaxed);
        1
    }

    unsafe extern "system" fn fake_physical_resource_release(_this: usize) -> u32 {
        PHYSICAL_RESOURCE_RELEASE_CALLS.fetch_add(1, Ordering::Relaxed);
        1
    }

    unsafe extern "system" fn fake_physical_texture_release(_this: usize) -> u32 {
        PHYSICAL_TEXTURE_RELEASE_CALLS.fetch_add(1, Ordering::Relaxed);
        1
    }

    fn virtual_base_access() -> HardwareProtectedAccess {
        const IS_HARDWARE_PROTECTED_INDEX: usize = 3;

        HardwareProtectedAccess::VirtualBaseDeviceResource {
            virtual_base: DeviceResourceVirtualBaseAccess {
                vbptr_offset: size_of::<usize>() as i32,
                vbtable_entry_offset: 3 * size_of::<i32>(),
            },
            is_hardware_protected_vtable_index: IS_HARDWARE_PROTECTED_INDEX,
        }
    }

    #[test]
    fn resolve_overlay_swap_chain_rejects_null_overlay_context() {
        assert_eq!(
            unsafe {
                resolve_overlay_swap_chain(
                    0,
                    overlay_path_profile().overlay_swap_chain_from_context,
                )
            },
            None
        );
    }

    #[test]
    fn resolve_overlay_swap_chain_rejects_null_method() {
        let profile = overlay_path_profile();
        let context = FakeOverlayContext::with_method(&profile, test_identity(), 0);
        assert_eq!(
            unsafe {
                resolve_overlay_swap_chain(
                    context.address(),
                    profile.overlay_swap_chain_from_context,
                )
            },
            None
        );
    }

    #[test]
    fn physical_back_buffer_resolves_texture_and_owns_query_interface_reference() {
        const GET_PHYSICAL_BACK_BUFFER_INDEX: usize = 2;
        const GET_D3D11_RESOURCE_INDEX: usize = 4;

        PHYSICAL_RESOURCE_RELEASE_CALLS.store(0, Ordering::Relaxed);
        PHYSICAL_TEXTURE_RELEASE_CALLS.store(0, Ordering::Relaxed);

        let get_next = fake_get_next as *const () as usize;
        let query_interface = fake_query_interface as *const () as usize;
        let resource_release = fake_physical_resource_release as *const () as usize;
        let texture_release = fake_physical_texture_release as *const () as usize;

        let (_texture_methods, texture) = fake_com_object(&[(2, texture_release)], 0);
        let (_resource_methods, resource) = fake_com_object(
            &[(0, query_interface), (2, resource_release)],
            std::ptr::from_ref(&texture) as usize,
        );
        let (_buffer_methods, physical_back_buffer) = fake_com_object(
            &[(GET_D3D11_RESOURCE_INDEX, get_next)],
            std::ptr::from_ref(&resource) as usize,
        );
        let (_swap_chain_methods, swap_chain) = fake_com_object(
            &[(GET_PHYSICAL_BACK_BUFFER_INDEX, get_next)],
            std::ptr::from_ref(&physical_back_buffer) as usize,
        );

        let resolved_texture = unsafe {
            resolve_d3d11_texture2d(
                std::ptr::from_ref(&swap_chain) as usize,
                D3D11ResourceAccess::PhysicalBackBuffer {
                    get_physical_back_buffer_vtable_index: GET_PHYSICAL_BACK_BUFFER_INDEX,
                    get_d3d11_resource_vtable_index: GET_D3D11_RESOURCE_INDEX,
                },
            )
        }
        .expect("texture should resolve");

        assert_eq!(
            resolved_texture.as_raw() as usize,
            std::ptr::from_ref(&texture) as usize
        );
        assert_eq!(PHYSICAL_RESOURCE_RELEASE_CALLS.load(Ordering::Relaxed), 0);
        assert_eq!(PHYSICAL_TEXTURE_RELEASE_CALLS.load(Ordering::Relaxed), 0);
        drop(resolved_texture);
        assert_eq!(PHYSICAL_TEXTURE_RELEASE_CALLS.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn current_back_buffer_rtv_resolves_texture_and_releases_resource_reference() {
        const GET_CURRENT_BACK_BUFFER_INDEX: usize = 2;
        const GET_RENDER_TARGET_VIEW_INDEX: usize = 4;
        const GET_RESOURCE_INDEX: usize =
            offset_of!(ID3D11View_Vtbl, GetResource) / size_of::<usize>();

        RELEASE_CALLS.store(0, Ordering::Relaxed);
        TEXTURE_RELEASE_CALLS.store(0, Ordering::Relaxed);

        let get_next = fake_get_next as *const () as usize;
        let get_resource = fake_get_resource as *const () as usize;
        let query_interface = fake_query_interface as *const () as usize;
        let release = fake_release as *const () as usize;
        let texture_release = fake_texture_release as *const () as usize;

        let (_texture_methods, texture) = fake_com_object(&[(2, texture_release)], 0);
        let (_resource_methods, resource) = fake_com_object(
            &[(0, query_interface), (2, release)],
            std::ptr::from_ref(&texture) as usize,
        );
        let (_rtv_methods, rtv) = fake_com_object(
            &[(GET_RESOURCE_INDEX, get_resource)],
            std::ptr::from_ref(&resource) as usize,
        );
        let (_target_methods, target) = fake_com_object(
            &[(GET_RENDER_TARGET_VIEW_INDEX, get_next)],
            std::ptr::from_ref(&rtv) as usize,
        );
        let (_swap_chain_methods, swap_chain) = fake_com_object(
            &[(GET_CURRENT_BACK_BUFFER_INDEX, get_next)],
            std::ptr::from_ref(&target) as usize,
        );

        let resolved_texture = unsafe {
            resolve_d3d11_texture2d(
                std::ptr::from_ref(&swap_chain) as usize,
                D3D11ResourceAccess::CurrentBackBufferRenderTargetView {
                    get_current_back_buffer_vtable_index: GET_CURRENT_BACK_BUFFER_INDEX,
                    get_render_target_view_vtable_index: GET_RENDER_TARGET_VIEW_INDEX,
                },
            )
        }
        .expect("texture should resolve");

        assert_eq!(
            resolved_texture.as_raw() as usize,
            std::ptr::from_ref(&texture) as usize
        );
        assert_eq!(RELEASE_CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(TEXTURE_RELEASE_CALLS.load(Ordering::Relaxed), 0);
        drop(resolved_texture);
        assert_eq!(TEXTURE_RELEASE_CALLS.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn resolve_d3d11_texture2d_reports_unreadable_swap_chain() {
        let invalid_address = align_of::<usize>();
        assert_eq!(
            unsafe {
                resolve_d3d11_texture2d(invalid_address, overlay_path_profile().d3d11_resource)
            },
            Err(D3D11TextureResolveError::UnreadableMemory)
        );
    }

    #[test]
    fn read_monitor_identity_calls_profile_virtual_methods() {
        const GET_LUID_INDEX: usize = 6;
        const GET_TARGET_ID_INDEX: usize = 8;

        let mut vtable = [0usize; GET_TARGET_ID_INDEX + 1];
        vtable[GET_LUID_INDEX] = fake_get_display_adapter_luid as *const () as usize;
        vtable[GET_TARGET_ID_INDEX] = fake_get_vid_pn_target_id as *const () as usize;
        let object = FakeIdentityObject {
            vtable: vtable.as_ptr(),
            luid: LUID {
                LowPart: 0x1234,
                HighPart: -2,
            },
            target_id: 7,
        };

        assert_eq!(
            unsafe {
                read_monitor_identity(
                    (&object as *const FakeIdentityObject) as usize,
                    MonitorIdentityAccess {
                        get_display_adapter_luid_vtable_index: GET_LUID_INDEX,
                        get_vid_pn_target_id_vtable_index: GET_TARGET_ID_INDEX,
                    },
                )
            },
            Ok(MonitorIdentity {
                adapter_luid: AdapterLuid {
                    high_part: -2,
                    low_part: 0x1234,
                },
                target_id: 7,
            })
        );
    }

    #[test]
    fn read_monitor_identity_rejects_null_overlay_pointer() {
        assert_eq!(
            unsafe {
                read_monitor_identity(
                    0,
                    MonitorIdentityAccess {
                        get_display_adapter_luid_vtable_index: 0,
                        get_vid_pn_target_id_vtable_index: 0,
                    },
                )
            },
            Err(MonitorIdentityReadError::UnreadableMemory)
        );
    }

    #[test]
    fn read_hardware_protected_reads_profile_offset() {
        let profile = overlay_path_profile();
        let bytes = fake_overlay_swap_chain_with_hardware_protected(
            profile.monitor_identity,
            profile.hardware_protected,
            test_identity(),
            true,
        );
        assert_eq!(
            unsafe { read_hardware_protected(bytes.as_ptr() as usize, profile.hardware_protected) },
            Ok(true)
        );
    }

    #[test]
    fn read_hardware_protected_uses_virtual_base_without_overlay_vtable_or_release() {
        const IS_HARDWARE_PROTECTED_INDEX: usize = 3;

        RELEASE_CALLS.store(0, Ordering::Relaxed);
        CURRENT_BACK_BUFFER_CALLS.store(0, Ordering::Relaxed);
        let mut device_resource_vtable = [0usize; IS_HARDWARE_PROTECTED_INDEX + 1];
        device_resource_vtable[2] = fake_release as *const () as usize;
        device_resource_vtable[IS_HARDWARE_PROTECTED_INDEX] =
            fake_is_hardware_protected as *const () as usize;
        let mut swap_chain_vtable = [0usize; 1];
        swap_chain_vtable[0] = fake_get_current_back_buffer as *const () as usize;
        let mut vbtable = [0i32; 4];
        vbtable[3] = (3 * size_of::<usize>() - size_of::<usize>()) as i32;

        for expected in [false, true] {
            let object = FakeVirtualBaseObject {
                overlay_vtable: swap_chain_vtable.as_ptr(),
                vbtable: vbtable.as_ptr(),
                padding: 0,
                device_resource: FakeComObject {
                    vtable: device_resource_vtable.as_ptr(),
                    next: usize::from(expected),
                },
            };

            assert_eq!(
                unsafe {
                    read_hardware_protected(
                        (&object as *const FakeVirtualBaseObject) as usize,
                        virtual_base_access(),
                    )
                },
                Ok(expected)
            );
        }
        assert_eq!(CURRENT_BACK_BUFFER_CALLS.load(Ordering::Relaxed), 0);
        assert_eq!(RELEASE_CALLS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn read_hardware_protected_honors_negative_virtual_base_displacement() {
        const IS_HARDWARE_PROTECTED_INDEX: usize = 3;

        let mut device_resource_vtable = [0usize; IS_HARDWARE_PROTECTED_INDEX + 1];
        device_resource_vtable[IS_HARDWARE_PROTECTED_INDEX] =
            fake_is_hardware_protected as *const () as usize;
        let overlay_vtable = [fake_get_current_back_buffer as *const () as usize];
        let mut vbtable = [0i32; 4];
        vbtable[3] = -(2 * size_of::<usize>() as i32);
        let object = FakeNegativeVirtualBaseObject {
            overlay_vtable: overlay_vtable.as_ptr(),
            device_resource: FakeComObject {
                vtable: device_resource_vtable.as_ptr(),
                next: 1,
            },
            vbtable: vbtable.as_ptr(),
        };

        assert_eq!(
            unsafe {
                read_hardware_protected(
                    (&object as *const FakeNegativeVirtualBaseObject) as usize,
                    HardwareProtectedAccess::VirtualBaseDeviceResource {
                        virtual_base: DeviceResourceVirtualBaseAccess {
                            vbptr_offset: (3 * size_of::<usize>()) as i32,
                            vbtable_entry_offset: 3 * size_of::<i32>(),
                        },
                        is_hardware_protected_vtable_index: IS_HARDWARE_PROTECTED_INDEX,
                    },
                )
            },
            Ok(true)
        );
    }

    #[test]
    fn read_hardware_protected_rejects_null_vbtable() {
        let object = FakeComObject {
            vtable: std::ptr::null(),
            next: 0,
        };
        assert_eq!(
            unsafe {
                read_hardware_protected(
                    (&object as *const FakeComObject) as usize,
                    virtual_base_access(),
                )
            },
            Err(HardwareProtectedReadError::UnreadableMemory)
        );
    }

    #[test]
    fn read_hardware_protected_rejects_null_overlay_pointer() {
        assert_eq!(
            unsafe { read_hardware_protected(0, virtual_base_access()) },
            Err(HardwareProtectedReadError::UnreadableMemory)
        );
    }

    #[test]
    fn read_hardware_protected_rejects_unresolvable_signed_displacement() {
        let overlay_vtable = [fake_get_current_back_buffer as *const () as usize];
        let mut vbtable = [0i32; 4];
        vbtable[3] = i32::MAX;
        let object = FakeVirtualBaseObject {
            overlay_vtable: overlay_vtable.as_ptr(),
            vbtable: vbtable.as_ptr(),
            padding: 0,
            device_resource: FakeComObject {
                vtable: std::ptr::null(),
                next: 0,
            },
        };

        assert_eq!(
            unsafe {
                read_hardware_protected(
                    (&object as *const FakeVirtualBaseObject) as usize,
                    virtual_base_access(),
                )
            },
            Err(HardwareProtectedReadError::UnreadableMemory)
        );
    }

    #[test]
    fn read_hardware_protected_rejects_null_device_resource_vtable() {
        let overlay_vtable = [fake_get_current_back_buffer as *const () as usize];
        let mut vbtable = [0i32; 4];
        vbtable[3] = (2 * size_of::<usize>()) as i32;
        let object = FakeVirtualBaseObject {
            overlay_vtable: overlay_vtable.as_ptr(),
            vbtable: vbtable.as_ptr(),
            padding: 0,
            device_resource: FakeComObject {
                vtable: std::ptr::null(),
                next: 0,
            },
        };

        assert_eq!(
            unsafe {
                read_hardware_protected(
                    (&object as *const FakeVirtualBaseObject) as usize,
                    virtual_base_access(),
                )
            },
            Err(HardwareProtectedReadError::UnreadableMemory)
        );
    }

    #[test]
    fn read_hardware_protected_rejects_null_device_resource_method() {
        let device_resource_vtable = [0usize; 4];
        let overlay_vtable = [fake_get_current_back_buffer as *const () as usize];
        let mut vbtable = [0i32; 4];
        vbtable[3] = (2 * size_of::<usize>()) as i32;
        let object = FakeVirtualBaseObject {
            overlay_vtable: overlay_vtable.as_ptr(),
            vbtable: vbtable.as_ptr(),
            padding: 0,
            device_resource: FakeComObject {
                vtable: device_resource_vtable.as_ptr(),
                next: 0,
            },
        };
        assert_eq!(
            unsafe {
                read_hardware_protected(
                    (&object as *const FakeVirtualBaseObject) as usize,
                    virtual_base_access(),
                )
            },
            Err(HardwareProtectedReadError::UnreadableMemory)
        );
    }

    #[test]
    fn read_hardware_protected_rejects_vbptr_address_overflow() {
        assert_eq!(
            unsafe { read_hardware_protected(usize::MAX, virtual_base_access()) },
            Err(HardwareProtectedReadError::UnreadableMemory)
        );
    }

    #[test]
    fn read_hardware_protected_reports_unreadable_virtual_base_memory() {
        assert_eq!(
            unsafe { read_hardware_protected(align_of::<usize>(), virtual_base_access()) },
            Err(HardwareProtectedReadError::UnreadableMemory)
        );
    }

    #[test]
    fn dirty_rects_reject_vector_past_capacity() {
        let rects = [DirtyRect {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        }];
        let rect_vec = RectVec {
            first: rects.as_ptr(),
            last: unsafe { rects.as_ptr().add(1) },
            end: rects.as_ptr(),
        };

        let error = unsafe { read_dirty_rects((&rect_vec as *const RectVec) as usize) }
            .expect_err("end past capacity should be rejected");
        assert_eq!(error, DirtyRectReadError::InvalidVector);
    }

    #[test]
    fn dirty_rects_reject_misaligned_vector() {
        let first = std::ptr::dangling::<DirtyRect>() as usize + 1;
        let rect_vec = RectVec {
            first: first as *const DirtyRect,
            last: (first + size_of::<DirtyRect>()) as *const DirtyRect,
            end: (first + size_of::<DirtyRect>()) as *const DirtyRect,
        };

        let error = unsafe { read_dirty_rects((&rect_vec as *const RectVec) as usize) }
            .expect_err("misaligned start should be rejected");
        assert_eq!(error, DirtyRectReadError::InvalidVector);
    }

    #[test]
    fn dirty_rects_report_unreadable_vector_memory() {
        let invalid_address = align_of::<RectVec>();
        let error = unsafe { read_dirty_rects(invalid_address) }
            .expect_err("unreadable rect vector should be rejected");
        assert_eq!(error, DirtyRectReadError::UnreadableMemory);
    }

    #[test]
    fn dirty_rects_report_unreadable_element_memory() {
        let first = std::ptr::dangling::<DirtyRect>();
        let last = first.wrapping_add(1);
        let rect_vec = RectVec {
            first,
            last,
            end: last,
        };

        let error = unsafe { read_dirty_rects((&rect_vec as *const RectVec) as usize) }
            .expect_err("unreadable dirty rects should be rejected");
        assert_eq!(error, DirtyRectReadError::UnreadableMemory);
    }
}
