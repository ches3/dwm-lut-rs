use std::ffi::c_void;
use std::mem::{size_of, transmute_copy};
use std::ptr;

use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::core::{GUID, Interface};

type GetContainerFn = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
type GetResourceFn = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
type QueryInterfaceFn =
    unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32;

pub(super) unsafe fn get_back_buffer(
    overlay_swap_chain: *mut c_void,
    container_vtable_index: usize,
    resource_vtable_index: usize,
) -> Option<*mut c_void> {
    microseh::try_seh(|| unsafe {
        get_back_buffer_unguarded(
            overlay_swap_chain,
            container_vtable_index,
            resource_vtable_index,
        )
    })
    .unwrap_or_default()
}

unsafe fn get_back_buffer_unguarded(
    overlay_swap_chain: *mut c_void,
    container_vtable_index: usize,
    resource_vtable_index: usize,
) -> Option<*mut c_void> {
    if overlay_swap_chain.is_null() {
        return None;
    }

    let overlay_vtbl = unsafe { read_vtable(overlay_swap_chain)? };
    let get_container =
        unsafe { vtable_fn::<GetContainerFn>(overlay_vtbl, container_vtable_index)? };
    let container = unsafe { get_container(overlay_swap_chain) };
    if container.is_null() {
        return None;
    }

    let container_vtbl = unsafe { read_vtable(container)? };
    let get_resource =
        unsafe { vtable_fn::<GetResourceFn>(container_vtbl, resource_vtable_index)? };
    let resource = unsafe { get_resource(container) };
    if resource.is_null() {
        return None;
    }

    let resource_vtbl = unsafe { read_vtable(resource)? };
    let query_interface = unsafe { vtable_fn::<QueryInterfaceFn>(resource_vtbl, 0)? };
    let mut texture = ptr::null_mut();
    let hr = unsafe { query_interface(resource, &ID3D11Texture2D::IID, &mut texture) };
    if hr < 0 || texture.is_null() {
        return None;
    }

    Some(texture)
}

unsafe fn read_vtable(object: *mut c_void) -> Option<*const *mut c_void> {
    let vtbl = unsafe { *(object as *const *const *mut c_void) };
    if vtbl.is_null() { None } else { Some(vtbl) }
}

unsafe fn vtable_fn<T>(vtbl: *const *mut c_void, index: usize) -> Option<T> {
    debug_assert_eq!(size_of::<T>(), size_of::<*mut c_void>());
    let entry = unsafe { *vtbl.add(index) };
    if entry.is_null() {
        None
    } else {
        Some(unsafe { transmute_copy(&entry) })
    }
}
