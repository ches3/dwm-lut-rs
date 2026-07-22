mod apply_lut;
mod collect;

use crate::state;

use apply_lut::apply_lut;
use collect::{RectVec, collect_present_inputs};

pub(crate) use apply_lut::empty_rect_vec_storage;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug)]
pub(crate) struct PreparedPresent {
    pub rect_vec: usize,
}

pub(crate) fn prepare_present(
    this: usize,
    overlay_swap_chain: usize,
    rect_vec: usize,
    present_rect_storage: &mut [DirtyRect; 1],
    present_rect_vec_storage: &mut RectVec,
) -> PreparedPresent {
    match unsafe { collect_present_inputs(overlay_swap_chain, rect_vec) } {
        Ok(inputs) => {
            let applied = apply_lut(
                this,
                overlay_swap_chain,
                &inputs,
                rect_vec,
                present_rect_storage,
                present_rect_vec_storage,
            );
            PreparedPresent {
                rect_vec: applied.rect_vec,
            }
        }
        Err(error) => {
            #[cfg(debug_assertions)]
            {
                debug_log!(
                    "event=present_input_collect_error this=0x{:x} overlay_swap_chain=0x{:x} rect_vec=0x{:x} error={:?}",
                    this,
                    overlay_swap_chain,
                    rect_vec,
                    error
                );
            }
            #[cfg(not(debug_assertions))]
            let _ = error;
            state::deactivate_present_context(this);
            PreparedPresent { rect_vec }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;

    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, MonitorIdentity, MonitorTarget, PayloadAssignment,
        PayloadLut,
    };

    use super::DirtyRect;
    use crate::HookProfile;
    use crate::minhook;
    use crate::profile::HookTarget;
    use crate::resolver::{LoadedModule, ResolvedTarget, SignatureResolutionReport};
    use crate::state;

    use super::collect::{RectVec, read_dirty_rects};

    pub(crate) fn test_profile() -> HookProfile {
        crate::profile::latest_registered_profile()
    }

    static LAST_ORIGINAL_PRESENT_RECTS: Mutex<Option<Vec<DirtyRect>>> = Mutex::new(None);

    pub(crate) fn last_original_present_rects() -> Option<Vec<DirtyRect>> {
        LAST_ORIGINAL_PRESENT_RECTS
            .lock()
            .ok()
            .and_then(|rects| rects.clone())
    }

    pub(crate) fn reset_last_original_present_rects() {
        if let Ok(mut rects) = LAST_ORIGINAL_PRESENT_RECTS.lock() {
            *rects = None;
        }
    }

    pub(crate) unsafe extern "system" fn returns_present_status(
        _a0: usize,
        _a1: usize,
        _a2: u32,
        a3: usize,
        _a4: i32,
        _a5: usize,
        _a6: u8,
    ) -> i64 {
        if let Ok(mut rects) = LAST_ORIGINAL_PRESENT_RECTS.lock() {
            *rects = unsafe { read_dirty_rects(a3) }.ok();
        }
        0x55
    }

    pub(crate) fn test_monitor_identity() -> MonitorIdentity {
        test_monitor_identity_for_target(4357)
    }

    fn test_monitor_identity_for_target(target_id: u32) -> MonitorIdentity {
        MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id,
        }
    }

    fn synthetic_resolution(profile: &HookProfile) -> SignatureResolutionReport {
        let base_address = 0x1800_0000usize;
        SignatureResolutionReport {
            module: LoadedModule {
                module_name: crate::profile::HOOK_MODULE_NAME,
                base_address,
                size: 0x20_0000,
            },
            targets: profile
                .signatures
                .iter()
                .enumerate()
                .map(|(index, signature)| ResolvedTarget {
                    target: signature.target,
                    address: if !signature.target.is_function_hook_target() {
                        0
                    } else {
                        base_address + 0x1000 + index * 0x100
                    },
                })
                .collect(),
            skipped_signatures: Vec::new(),
        }
    }

    fn identity_lut() -> PayloadLut {
        PayloadLut {
            size: 2,
            domain_min: [0.0, 0.0, 0.0],
            domain_max: [1.0, 1.0, 1.0],
            values: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
                [1.0, 1.0, 1.0],
            ],
        }
    }

    pub(crate) fn test_payload(color_modes: &[ColorMode]) -> HookPayload {
        HookPayload {
            assignments: color_modes
                .iter()
                .map(|color_mode| PayloadAssignment {
                    target: MonitorTarget {
                        identity: test_monitor_identity(),
                        color_mode: *color_mode,
                    },
                    lut: identity_lut(),
                })
                .collect(),
        }
    }

    pub(crate) fn initialize_test_state() {
        state::reset_state_for_tests();
        initialize_test_state_from_payload(test_payload(&[ColorMode::Sdr]));
    }

    pub(crate) fn initialize_test_state_from_payload(payload: HookPayload) {
        let profile = test_profile();
        let resolution = synthetic_resolution(&profile);
        crate::bootstrap::initialize_with_resolution(profile, payload, resolution)
            .expect("initialization should succeed with synthetic resolution");
    }

    pub(crate) fn activate_context(context_address: usize) {
        state::update_present_context(context_address, true);
    }

    pub(crate) fn install_present_original() {
        minhook::original_pointer_for_target(HookTarget::Present)
            .store(returns_present_status as *mut c_void, Ordering::Release);
    }

    pub(crate) struct FakePresentObjects {
        context: Box<usize>,
        overlay_swap_chain: Vec<usize>,
        pub(crate) dirty_rects: Vec<DirtyRect>,
        rect_vec: RectVec,
    }

    impl FakePresentObjects {
        pub(crate) fn new(dirty_rects: Vec<DirtyRect>, hardware_protected: bool) -> Self {
            let profile = test_profile();
            let context = Box::new(0usize);

            let identity = profile.monitor_identity;
            let overlay_swap_chain_len = (profile
                .hardware_protected_offset
                .max(identity.target_id_offset + size_of::<u32>())
                + 1)
            .div_ceil(size_of::<usize>());
            let mut overlay_swap_chain = vec![0usize; overlay_swap_chain_len];
            unsafe {
                (overlay_swap_chain.as_mut_ptr() as *mut u8)
                    .add(profile.hardware_protected_offset)
                    .write(u8::from(hardware_protected));
                ((overlay_swap_chain.as_mut_ptr() as *mut u8).add(identity.adapter_luid_low_offset)
                    as *mut u32)
                    .write(test_monitor_identity().adapter_luid.low_part);
                ((overlay_swap_chain.as_mut_ptr() as *mut u8)
                    .add(identity.adapter_luid_high_offset) as *mut i32)
                    .write(test_monitor_identity().adapter_luid.high_part);
                ((overlay_swap_chain.as_mut_ptr() as *mut u8).add(identity.target_id_offset)
                    as *mut u32)
                    .write(test_monitor_identity().target_id);
            }

            let rect_vec = if dirty_rects.is_empty() {
                RectVec {
                    start: std::ptr::null(),
                    end: std::ptr::null(),
                    capacity_end: std::ptr::null(),
                }
            } else {
                let start = dirty_rects.as_ptr();
                RectVec {
                    start,
                    end: unsafe { start.add(dirty_rects.len()) },
                    capacity_end: unsafe { start.add(dirty_rects.capacity()) },
                }
            };

            Self {
                context,
                overlay_swap_chain,
                dirty_rects,
                rect_vec,
            }
        }

        pub(crate) fn context_address(&self) -> usize {
            (&*self.context as *const usize) as usize
        }

        pub(crate) fn overlay_swap_chain_address(&self) -> usize {
            self.overlay_swap_chain.as_ptr() as usize
        }

        pub(crate) fn rect_vec_address(&self) -> usize {
            (&self.rect_vec as *const RectVec) as usize
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{activate_context, initialize_test_state};
    use super::{DirtyRect, empty_rect_vec_storage, prepare_present};
    use crate::state;
    use crate::state::HOOK_GLOBAL_TEST_LOCK;

    #[test]
    fn prepare_present_clears_context_when_input_acquisition_fails() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        initialize_test_state();
        activate_context(0x1234);

        let mut present_rect_storage = [DirtyRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }];
        let mut present_rect_vec_storage = empty_rect_vec_storage();
        let prepared = prepare_present(
            0x1234,
            0,
            0,
            &mut present_rect_storage,
            &mut present_rect_vec_storage,
        );

        assert_eq!(prepared.rect_vec, 0);
        assert!(!state::has_present_context(0x1234));
    }
}
