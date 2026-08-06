use std::mem::{align_of, size_of, transmute_copy};
use std::ptr;

use dwm_lut_payload::{AdapterLuid, MonitorIdentity};
use dwm_lut_profile::{ContextToSwapChainPath, MonitorIdentityOffsets, SwapChainToResourcePath};

const MAX_DIRTY_RECTS: usize = 4096;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RectVec {
    pub(crate) start: *const DirtyRect,
    pub(crate) end: *const DirtyRect,
    pub(crate) capacity_end: *const DirtyRect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirtyRectReadError {
    InvalidVector,
    UnreadableMemory,
}

pub(crate) fn read_monitor_identity(
    overlay_swap_chain: usize,
    offsets: MonitorIdentityOffsets,
) -> Option<MonitorIdentity> {
    let low_addr = overlay_swap_chain.checked_add(offsets.adapter_luid_low_offset)?;
    let high_addr = overlay_swap_chain.checked_add(offsets.adapter_luid_high_offset)?;
    let target_addr = overlay_swap_chain.checked_add(offsets.target_id_offset)?;
    microseh::try_seh(|| unsafe {
        let low_part = read::<u32>(low_addr)?;
        let high_part = read::<i32>(high_addr)?;
        let target_id = read::<u32>(target_addr)?;
        Some(MonitorIdentity {
            adapter_luid: AdapterLuid {
                low_part,
                high_part,
            },
            target_id,
        })
    })
    .unwrap_or_default()
}

pub(crate) fn read_hardware_protected(overlay_swap_chain: usize, offset: usize) -> Option<bool> {
    let address = overlay_swap_chain.checked_add(offset)?;
    microseh::try_seh(|| unsafe { read::<u8>(address).map(|value| value != 0) }).unwrap_or_default()
}

pub(crate) fn resolve_overlay_swap_chain(
    overlay_context: usize,
    path: ContextToSwapChainPath,
) -> Option<usize> {
    if overlay_context == 0 {
        return None;
    }

    microseh::try_seh(|| unsafe { resolve_overlay_swap_chain_unguarded(overlay_context, path) })
        .unwrap_or_default()
}

pub(crate) fn resolve_swap_chain_resource(
    overlay_swap_chain: usize,
    path: SwapChainToResourcePath,
) -> Option<usize> {
    if overlay_swap_chain == 0 {
        return None;
    }

    microseh::try_seh(|| unsafe { resolve_swap_chain_resource_unguarded(overlay_swap_chain, path) })
        .unwrap_or_default()
}

pub(crate) unsafe fn read_dirty_rects(
    rect_vec: usize,
) -> Result<Vec<DirtyRect>, DirtyRectReadError> {
    if rect_vec == 0 {
        return Err(DirtyRectReadError::InvalidVector);
    }

    let rect_vec = microseh::try_seh(|| unsafe { read::<RectVec>(rect_vec) })
        .map_err(|_| DirtyRectReadError::UnreadableMemory)?
        .ok_or(DirtyRectReadError::UnreadableMemory)?;

    let start = rect_vec.start as usize;
    let end = rect_vec.end as usize;
    let capacity_end = rect_vec.capacity_end as usize;
    if start == 0 && end == 0 && capacity_end == 0 {
        return Ok(Vec::new());
    }
    if start == 0
        || end < start
        || capacity_end < end
        || !start.is_multiple_of(align_of::<DirtyRect>())
    {
        return Err(DirtyRectReadError::InvalidVector);
    }

    let byte_len = end - start;
    if !byte_len.is_multiple_of(size_of::<DirtyRect>())
        || !(capacity_end - start).is_multiple_of(size_of::<DirtyRect>())
    {
        return Err(DirtyRectReadError::InvalidVector);
    }

    let count = byte_len / size_of::<DirtyRect>();
    if count > MAX_DIRTY_RECTS {
        return Err(DirtyRectReadError::InvalidVector);
    }

    let mut rects = Vec::with_capacity(count);
    if count != 0 {
        let destination = rects.as_mut_ptr();
        microseh::try_seh(|| unsafe {
            ptr::copy_nonoverlapping(start as *const DirtyRect, destination, count);
        })
        .map_err(|_| DirtyRectReadError::UnreadableMemory)?;
        unsafe {
            rects.set_len(count);
        }
    }
    Ok(rects)
}

unsafe fn resolve_overlay_swap_chain_unguarded(
    overlay_context: usize,
    path: ContextToSwapChainPath,
) -> Option<usize> {
    let monitor =
        unsafe { read::<usize>(overlay_context.checked_add(path.monitor_target_offset)?)? };
    if monitor == 0 {
        return None;
    }

    type GetOverlaySwapChain = unsafe extern "system" fn(usize) -> usize;
    let get_overlay_swap_chain =
        unsafe { object_vtable_fn::<GetOverlaySwapChain>(monitor, path.swap_chain_vtable_index)? };
    let swap_chain = unsafe { get_overlay_swap_chain(monitor) };
    (swap_chain != 0).then_some(swap_chain)
}

unsafe fn resolve_swap_chain_resource_unguarded(
    overlay_swap_chain: usize,
    path: SwapChainToResourcePath,
) -> Option<usize> {
    type GetContainer = unsafe extern "system" fn(usize) -> usize;
    type GetResource = unsafe extern "system" fn(usize) -> usize;

    let get_container = unsafe {
        object_vtable_fn::<GetContainer>(overlay_swap_chain, path.container_vtable_index)?
    };
    let container = unsafe { get_container(overlay_swap_chain) };
    if container == 0 {
        return None;
    }

    let get_resource =
        unsafe { object_vtable_fn::<GetResource>(container, path.resource_vtable_index)? };
    let resource = unsafe { get_resource(container) };
    (resource != 0).then_some(resource)
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

unsafe fn read<T: Copy>(address: usize) -> Option<T> {
    if address == 0 {
        return None;
    }
    Some(unsafe { (address as *const T).read_unaligned() })
}

#[cfg(test)]
mod tests {
    use std::mem::{align_of, size_of};

    use dwm_lut_payload::{AdapterLuid, MonitorIdentity};

    use super::{
        DirtyRect, DirtyRectReadError, RectVec, read_dirty_rects, read_hardware_protected,
        read_monitor_identity, resolve_overlay_swap_chain, resolve_swap_chain_resource,
    };
    use crate::dwmcore::test_support::{
        FakeOverlayContext, overlay_path_profile, overlay_swap_chain_bytes,
        overlay_swap_chain_bytes_with_hardware_protected,
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

    #[test]
    fn resolve_overlay_swap_chain_rejects_null_overlay_context() {
        assert_eq!(
            resolve_overlay_swap_chain(0, overlay_path_profile().context_to_swap_chain_path),
            None
        );
    }

    #[test]
    fn resolve_overlay_swap_chain_rejects_null_method() {
        let profile = overlay_path_profile();
        let context = FakeOverlayContext::with_method(&profile, test_identity(), 0);
        assert_eq!(
            resolve_overlay_swap_chain(context.address(), profile.context_to_swap_chain_path),
            None
        );
    }

    #[test]
    fn read_monitor_identity_reads_profile_offsets() {
        let profile = overlay_path_profile();
        let bytes = overlay_swap_chain_bytes(profile.monitor_identity_offsets, test_identity());
        assert_eq!(
            read_monitor_identity(bytes.as_ptr() as usize, profile.monitor_identity_offsets),
            Some(test_identity())
        );
    }

    #[test]
    fn read_hardware_protected_reads_profile_offset() {
        let profile = overlay_path_profile();
        let bytes = overlay_swap_chain_bytes_with_hardware_protected(
            profile.monitor_identity_offsets,
            profile.hardware_protected_offset,
            test_identity(),
            true,
        );
        assert_eq!(
            read_hardware_protected(bytes.as_ptr() as usize, profile.hardware_protected_offset),
            Some(true)
        );
    }

    #[test]
    fn read_monitor_identity_rejects_offset_overflow() {
        let offsets = dwm_lut_profile::MonitorIdentityOffsets {
            adapter_luid_low_offset: usize::MAX,
            adapter_luid_high_offset: 0,
            target_id_offset: 0,
        };
        assert_eq!(read_monitor_identity(1, offsets), None);
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
            start: rects.as_ptr(),
            end: unsafe { rects.as_ptr().add(1) },
            capacity_end: rects.as_ptr(),
        };

        let error = unsafe { read_dirty_rects((&rect_vec as *const RectVec) as usize) }
            .expect_err("end past capacity should be rejected");
        assert_eq!(error, DirtyRectReadError::InvalidVector);
    }

    #[test]
    fn dirty_rects_reject_misaligned_vector() {
        let start = std::ptr::dangling::<DirtyRect>() as usize + 1;
        let rect_vec = RectVec {
            start: start as *const DirtyRect,
            end: (start + size_of::<DirtyRect>()) as *const DirtyRect,
            capacity_end: (start + size_of::<DirtyRect>()) as *const DirtyRect,
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
        let start = std::ptr::dangling::<DirtyRect>();
        let end = start.wrapping_add(1);
        let rect_vec = RectVec {
            start,
            end,
            capacity_end: end,
        };

        let error = unsafe { read_dirty_rects((&rect_vec as *const RectVec) as usize) }
            .expect_err("unreadable dirty rects should be rejected");
        assert_eq!(error, DirtyRectReadError::UnreadableMemory);
    }

    #[test]
    fn resolve_swap_chain_resource_reports_unreadable_swap_chain() {
        let invalid_address = align_of::<usize>();
        assert!(
            resolve_swap_chain_resource(
                invalid_address,
                overlay_path_profile().swap_chain_to_resource_path,
            )
            .is_none()
        );
    }
}
