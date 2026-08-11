mod interop;
mod version;

pub use interop::DirtyRect;
#[cfg(not(test))]
pub(crate) use interop::resolve_d3d11_texture2d;
pub(crate) use interop::{
    D3D11TextureResolveError, DirtyRectReadError, HardwareProtectedReadError,
    MonitorIdentityReadError, RectVec, read_dirty_rects, read_hardware_protected,
    read_monitor_identity, resolve_overlay_swap_chain,
};
pub use version::{DwmcoreVersionError, dwmcore_file_version};

#[cfg(test)]
pub(crate) mod test_support {
    use std::mem::size_of;

    use dwm_lut_payload::MonitorIdentity;
    use dwm_lut_profile::{
        D3D11ResourceAccess, DeviceResourceVirtualBaseAccess, HardwareProtectedAccess, HookProfile,
        MonitorIdentityAccess, OverlaySwapChainFromContext,
    };
    use windows::Win32::Foundation::LUID;

    const CHAIN_OFFSET_IN_TARGET: usize = size_of::<usize>();
    const TEST_ADAPTER_LUID_LOW_PART_OFFSET: usize = 0x20;
    const TEST_ADAPTER_LUID_HIGH_PART_OFFSET: usize = 0x24;
    const TEST_VID_PN_TARGET_ID_OFFSET: usize = 0x28;
    const TEST_DEVICE_RESOURCE_OFFSET: usize = 0x40;
    const TEST_IS_HARDWARE_PROTECTED_VTABLE_INDEX: usize = 3;
    const TEST_VBTABLE_ENTRY_INDEX: usize = 3;

    pub(crate) fn overlay_path_profile() -> HookProfile {
        HookProfile {
            hooks: &[],
            overlay_swap_chain_from_context: OverlaySwapChainFromContext {
                overlay_monitor_target_pointer_offset: 0,
                get_overlay_swap_chain_vtable_index: 3,
            },
            d3d11_resource: D3D11ResourceAccess::PhysicalBackBuffer {
                get_physical_back_buffer_vtable_index: 0,
                get_d3d11_resource_vtable_index: 0,
            },
            monitor_identity: MonitorIdentityAccess {
                get_display_adapter_luid_vtable_index: 6,
                get_vid_pn_target_id_vtable_index: 8,
            },
            hardware_protected: HardwareProtectedAccess::DirectField { offset: 0x10 },
        }
    }

    pub(crate) fn virtual_base_hardware_protected_access() -> HardwareProtectedAccess {
        HardwareProtectedAccess::VirtualBaseDeviceResource {
            virtual_base: DeviceResourceVirtualBaseAccess {
                vbptr_offset: size_of::<usize>() as i32,
                vbtable_entry_offset: TEST_VBTABLE_ENTRY_INDEX * size_of::<i32>(),
            },
            is_hardware_protected_vtable_index: TEST_IS_HARDWARE_PROTECTED_VTABLE_INDEX,
        }
    }

    pub(crate) fn fake_overlay_swap_chain(
        access: MonitorIdentityAccess,
        identity: MonitorIdentity,
    ) -> FakeOverlaySwapChain {
        fake_overlay_swap_chain_with_hardware_protected(
            access,
            HardwareProtectedAccess::DirectField { offset: 0x10 },
            identity,
            false,
        )
    }

    pub(crate) fn fake_overlay_swap_chain_with_hardware_protected(
        identity_access: MonitorIdentityAccess,
        hardware_protected_access: HardwareProtectedAccess,
        identity: MonitorIdentity,
        hardware_protected: bool,
    ) -> FakeOverlaySwapChain {
        let hardware_protected_offset = match hardware_protected_access {
            HardwareProtectedAccess::DirectField { offset } => offset,
            HardwareProtectedAccess::VirtualBaseDeviceResource { .. } => {
                panic!("byte fixture requires a direct hardware-protected field")
            }
        };
        let hardware_protected_offset = positive_offset(hardware_protected_offset);
        let vtable = identity_vtable(identity_access);

        let len = chain_buffer_len(hardware_protected_offset);
        let mut bytes = vec![0u8; len];
        write_at(&mut bytes, 0, vtable.as_ptr() as usize);
        write_identity(&mut bytes, identity);
        write_at(
            &mut bytes,
            hardware_protected_offset,
            u8::from(hardware_protected),
        );
        FakeOverlaySwapChain {
            _vtable: vtable,
            _device_resource_vtable: None,
            _vbtable: None,
            bytes,
        }
    }

    pub(crate) fn overlay_swap_chain_with_virtual_base_hardware_protected(
        identity_access: MonitorIdentityAccess,
        identity: MonitorIdentity,
        hardware_protected: bool,
    ) -> FakeOverlaySwapChain {
        let mut vtable = identity_vtable(identity_access);
        vtable[0] = fake_get_current_back_buffer as *const () as usize;

        let mut device_resource_vtable =
            vec![0usize; TEST_IS_HARDWARE_PROTECTED_VTABLE_INDEX + 1].into_boxed_slice();
        device_resource_vtable[TEST_IS_HARDWARE_PROTECTED_VTABLE_INDEX] =
            fake_is_hardware_protected as *const () as usize;

        let mut vbtable = vec![0i32; TEST_VBTABLE_ENTRY_INDEX + 1].into_boxed_slice();
        vbtable[TEST_VBTABLE_ENTRY_INDEX] =
            (TEST_DEVICE_RESOURCE_OFFSET - size_of::<usize>()) as i32;

        let len = TEST_DEVICE_RESOURCE_OFFSET + size_of::<usize>() + size_of::<u8>();
        let mut bytes = vec![0u8; len];
        write_at(&mut bytes, 0, vtable.as_ptr() as usize);
        write_at(&mut bytes, size_of::<usize>(), vbtable.as_ptr() as usize);
        write_identity(&mut bytes, identity);
        write_at(
            &mut bytes,
            TEST_DEVICE_RESOURCE_OFFSET,
            device_resource_vtable.as_ptr() as usize,
        );
        write_at(
            &mut bytes,
            TEST_DEVICE_RESOURCE_OFFSET + size_of::<usize>(),
            u8::from(hardware_protected),
        );

        FakeOverlaySwapChain {
            _vtable: vtable,
            _device_resource_vtable: Some(device_resource_vtable),
            _vbtable: Some(vbtable),
            bytes,
        }
    }

    pub(crate) struct FakeOverlaySwapChain {
        _vtable: Box<[usize]>,
        _device_resource_vtable: Option<Box<[usize]>>,
        _vbtable: Option<Box<[i32]>>,
        bytes: Vec<u8>,
    }

    impl FakeOverlaySwapChain {
        pub(crate) fn as_ptr(&self) -> *const u8 {
            self.bytes.as_ptr()
        }
    }

    fn identity_vtable(identity_access: MonitorIdentityAccess) -> Box<[usize]> {
        let mut vtable = vec![
            0usize;
            identity_access
                .get_display_adapter_luid_vtable_index
                .max(identity_access.get_vid_pn_target_id_vtable_index)
                + 1
        ]
        .into_boxed_slice();
        vtable[identity_access.get_display_adapter_luid_vtable_index] =
            fake_get_display_adapter_luid as *const () as usize;
        vtable[identity_access.get_vid_pn_target_id_vtable_index] =
            fake_get_vid_pn_target_id as *const () as usize;
        vtable
    }

    fn write_identity(bytes: &mut [u8], identity: MonitorIdentity) {
        write_at(
            bytes,
            TEST_ADAPTER_LUID_LOW_PART_OFFSET,
            identity.adapter_luid.low_part,
        );
        write_at(
            bytes,
            TEST_ADAPTER_LUID_HIGH_PART_OFFSET,
            identity.adapter_luid.high_part,
        );
        write_at(bytes, TEST_VID_PN_TARGET_ID_OFFSET, identity.target_id);
    }

    unsafe extern "system" fn fake_get_current_back_buffer(_this: usize) -> usize {
        0
    }

    unsafe extern "system" fn fake_get_display_adapter_luid(
        this: usize,
        out: *mut LUID,
    ) -> *mut LUID {
        let low_part = unsafe {
            (this.wrapping_add(TEST_ADAPTER_LUID_LOW_PART_OFFSET) as *const u32).read_unaligned()
        };
        let high_part = unsafe {
            (this.wrapping_add(TEST_ADAPTER_LUID_HIGH_PART_OFFSET) as *const i32).read_unaligned()
        };
        unsafe {
            out.write(LUID {
                LowPart: low_part,
                HighPart: high_part,
            });
            out
        }
    }

    unsafe extern "system" fn fake_get_vid_pn_target_id(this: usize) -> u32 {
        unsafe { (this.wrapping_add(TEST_VID_PN_TARGET_ID_OFFSET) as *const u32).read_unaligned() }
    }

    unsafe extern "system" fn fake_is_hardware_protected(this: usize) -> u8 {
        unsafe { (this.wrapping_add(size_of::<usize>()) as *const u8).read_unaligned() }
    }

    pub(crate) struct FakeOverlayContext {
        _vtable: Box<[usize]>,
        _chain: FakeOverlaySwapChain,
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
            let path = profile.overlay_swap_chain_from_context;
            let mut vtable =
                vec![0usize; path.get_overlay_swap_chain_vtable_index + 1].into_boxed_slice();
            vtable[path.get_overlay_swap_chain_vtable_index] = method;

            let chain = fake_overlay_swap_chain(profile.monitor_identity, identity);
            let mut monitor = vec![0u8; CHAIN_OFFSET_IN_TARGET + chain.bytes.len()];
            let vtable_ptr = vtable.as_ptr() as usize;
            monitor[..size_of::<usize>()].copy_from_slice(&vtable_ptr.to_ne_bytes());
            monitor[CHAIN_OFFSET_IN_TARGET..].copy_from_slice(&chain.bytes);

            let monitor_addr = monitor.as_ptr() as usize;
            let monitor_offset = positive_offset(path.overlay_monitor_target_pointer_offset);
            let mut context_storage = vec![0u8; monitor_offset + size_of::<usize>()];
            context_storage[monitor_offset..monitor_offset + size_of::<usize>()]
                .copy_from_slice(&monitor_addr.to_ne_bytes());
            let context_address = context_storage.as_ptr() as usize;

            Self {
                _vtable: vtable,
                _chain: chain,
                _monitor: monitor,
                _context_storage: context_storage,
                context_address,
            }
        }

        pub(crate) fn address(&self) -> usize {
            self.context_address
        }
    }

    fn chain_buffer_len(hardware_protected_offset: usize) -> usize {
        TEST_ADAPTER_LUID_LOW_PART_OFFSET
            .max(TEST_ADAPTER_LUID_HIGH_PART_OFFSET)
            .max(TEST_VID_PN_TARGET_ID_OFFSET)
            .max(hardware_protected_offset)
            + size_of::<u32>()
    }

    fn positive_offset(offset: i32) -> usize {
        usize::try_from(offset).expect("test fixture offsets must be non-negative")
    }

    fn write_at<T: Copy>(bytes: &mut [u8], offset: usize, value: T) {
        let size = size_of::<T>();
        assert!(offset + size <= bytes.len());
        unsafe {
            (bytes.as_mut_ptr().add(offset) as *mut T).write_unaligned(value);
        }
    }
}
