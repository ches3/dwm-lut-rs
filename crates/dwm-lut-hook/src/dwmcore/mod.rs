mod interop;
mod version;

pub use interop::DirtyRect;
#[cfg(not(test))]
pub(crate) use interop::resolve_d3d11_resource;
pub(crate) use interop::{
    DirtyRectReadError, RectVec, read_dirty_rects, read_hardware_protected, read_monitor_identity,
    resolve_overlay_swap_chain,
};
pub use version::{DwmcoreVersionError, dwmcore_file_version};

#[cfg(test)]
pub(crate) mod test_support {
    use std::mem::size_of;

    use dwm_lut_payload::MonitorIdentity;
    use dwm_lut_profile::{
        FieldOffsets, HookProfile, OverlaySwapChainIdentityOffsets, VtableIndices,
    };

    const CHAIN_OFFSET_IN_TARGET: usize = size_of::<usize>();

    static OVERLAY_PATH_VTABLE_INDICES: VtableIndices = VtableIndices {
        get_physical_back_buffer: 0,
        get_d3d11_resource: 0,
        get_overlay_swap_chain: 3,
    };
    static OVERLAY_PATH_FIELD_OFFSETS: FieldOffsets = FieldOffsets {
        hardware_protected: 0x10,
        adapter_luid_low_part: 0x20,
        adapter_luid_high_part: 0x24,
        vid_pn_target_id: 0x28,
        overlay_monitor_target_pointer: 0,
    };

    pub(crate) fn overlay_path_profile() -> HookProfile {
        HookProfile {
            signatures: &[],
            vtable_indices: &OVERLAY_PATH_VTABLE_INDICES,
            field_offsets: &OVERLAY_PATH_FIELD_OFFSETS,
        }
    }

    pub(crate) fn overlay_swap_chain_bytes(
        offsets: OverlaySwapChainIdentityOffsets,
        identity: MonitorIdentity,
    ) -> Vec<u8> {
        overlay_swap_chain_bytes_with_hardware_protected(offsets, 0, identity, false)
    }

    pub(crate) fn overlay_swap_chain_bytes_with_hardware_protected(
        offsets: OverlaySwapChainIdentityOffsets,
        hardware_protected_offset: usize,
        identity: MonitorIdentity,
        hardware_protected: bool,
    ) -> Vec<u8> {
        let len = chain_buffer_len(offsets, hardware_protected_offset);
        let mut bytes = vec![0u8; len];
        write_at(
            &mut bytes,
            offsets.adapter_luid_low_part_offset,
            identity.adapter_luid.low_part,
        );
        write_at(
            &mut bytes,
            offsets.adapter_luid_high_part_offset,
            identity.adapter_luid.high_part,
        );
        write_at(
            &mut bytes,
            offsets.vid_pn_target_id_offset,
            identity.target_id,
        );
        write_at(
            &mut bytes,
            hardware_protected_offset,
            u8::from(hardware_protected),
        );
        bytes
    }

    pub(crate) struct FakeOverlayContext {
        _vtable: Box<[usize]>,
        _monitor: Vec<u8>,
        _context_storage: Vec<u8>,
        context_address: usize,
    }

    unsafe extern "system" fn fake_get_overlay_swap_chain(this: usize) -> usize {
        this + CHAIN_OFFSET_IN_TARGET
    }

    impl FakeOverlayContext {
        pub(crate) fn with_identity(profile: &HookProfile, identity: MonitorIdentity) -> Self {
            Self::with_method(
                profile,
                identity,
                fake_get_overlay_swap_chain as *const () as usize,
            )
        }

        pub(crate) fn with_method(
            profile: &HookProfile,
            identity: MonitorIdentity,
            method: usize,
        ) -> Self {
            let path = profile.overlay_swap_chain_path();
            let mut vtable =
                vec![0usize; path.get_overlay_swap_chain_vtable_index + 1].into_boxed_slice();
            vtable[path.get_overlay_swap_chain_vtable_index] = method;

            let chain =
                overlay_swap_chain_bytes(profile.overlay_swap_chain_identity_offsets(), identity);
            let mut monitor = vec![0u8; CHAIN_OFFSET_IN_TARGET + chain.len()];
            let vtable_ptr = vtable.as_ptr() as usize;
            monitor[..size_of::<usize>()].copy_from_slice(&vtable_ptr.to_ne_bytes());
            monitor[CHAIN_OFFSET_IN_TARGET..].copy_from_slice(&chain);

            let monitor_addr = monitor.as_ptr() as usize;
            let mut context_storage =
                vec![0u8; path.overlay_monitor_target_pointer_offset + size_of::<usize>()];
            context_storage[path.overlay_monitor_target_pointer_offset
                ..path.overlay_monitor_target_pointer_offset + size_of::<usize>()]
                .copy_from_slice(&monitor_addr.to_ne_bytes());
            let context_address = context_storage.as_ptr() as usize;

            Self {
                _vtable: vtable,
                _monitor: monitor,
                _context_storage: context_storage,
                context_address,
            }
        }

        pub(crate) fn address(&self) -> usize {
            self.context_address
        }
    }

    fn chain_buffer_len(
        offsets: OverlaySwapChainIdentityOffsets,
        hardware_protected_offset: usize,
    ) -> usize {
        offsets
            .adapter_luid_low_part_offset
            .max(offsets.adapter_luid_high_part_offset)
            .max(offsets.vid_pn_target_id_offset)
            .max(hardware_protected_offset)
            + size_of::<u32>()
    }

    fn write_at<T: Copy>(bytes: &mut [u8], offset: usize, value: T) {
        let size = size_of::<T>();
        assert!(offset + size <= bytes.len());
        unsafe {
            (bytes.as_mut_ptr().add(offset) as *mut T).write_unaligned(value);
        }
    }
}
