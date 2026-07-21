mod apply_lut;
mod collect;

use crate::DirtyRect;
#[cfg(debug_assertions)]
use crate::route_trace;
use crate::state;
#[cfg(debug_assertions)]
use dwm_lut_payload::MonitorIdentity;

use apply_lut::apply_lut;
use collect::{RectVec, collect_present_inputs};

pub(crate) use apply_lut::empty_rect_vec_storage;

#[derive(Debug)]
pub(crate) struct PreparedPresent {
    pub rect_vec: usize,
    #[cfg(debug_assertions)]
    last_present_context: Option<(bool, Option<MonitorIdentity>, Option<bool>)>,
    #[cfg(debug_assertions)]
    protected_resource_result_detail: Option<apply_lut::PresentOriginalCallDetail>,
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
                #[cfg(debug_assertions)]
                last_present_context: applied.last_present_context,
                #[cfg(debug_assertions)]
                protected_resource_result_detail: applied.protected_resource_result_detail,
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
            PreparedPresent {
                rect_vec,
                #[cfg(debug_assertions)]
                last_present_context: None,
                #[cfg(debug_assertions)]
                protected_resource_result_detail: None,
            }
        }
    }
}

pub(crate) fn finish_present(
    overlay_swap_chain: usize,
    prepared: &PreparedPresent,
    original_result: i64,
) {
    #[cfg(debug_assertions)]
    {
        let Some((hardware_protected, monitor_identity, lut_applied)) =
            prepared.last_present_context
        else {
            return;
        };
        let last_present_sequence = route_trace::record_last_present_context(
            overlay_swap_chain,
            monitor_identity,
            hardware_protected,
            lut_applied,
            None,
        );
        route_trace::record_last_present_original_result(last_present_sequence, original_result);
        if let Some(detail) = &prepared.protected_resource_result_detail {
            route_trace::record_protected_present_resource_result_summary(
                overlay_swap_chain,
                detail.monitor_identity,
                detail.hardware_protected,
                original_result,
                detail.render_outcome,
                detail.dirty_rect_count,
                detail.first_dirty_rect,
                detail.present_dirty_rect_source == "expanded",
                detail.render_outcome.present_dirty_rect,
            );
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = (overlay_swap_chain, prepared, original_result);
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

    use crate::minhook;
    use crate::profile::HookTarget;
    use crate::resolver::{LoadedModule, ResolvedTarget, SignatureResolutionReport};
    use crate::state;
    use crate::{BackBufferFormat, DirtyRect, HookProfile};

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
        state::update_present_context(
            context_address,
            crate::lut_pipeline::LutDecision::Apply {
                format: BackBufferFormat::Bgra8Unorm,
                lut_index: 0,
            },
        );
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

            let identity = profile.hypotheses.monitor_identity;
            let overlay_swap_chain_len = (profile
                .hypotheses
                .hardware_protected
                .offset
                .max(identity.target_id_offset + size_of::<u32>())
                + 1)
            .div_ceil(size_of::<usize>());
            let mut overlay_swap_chain = vec![0usize; overlay_swap_chain_len];
            unsafe {
                (overlay_swap_chain.as_mut_ptr() as *mut u8)
                    .add(profile.hypotheses.hardware_protected.offset)
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
    use super::{empty_rect_vec_storage, prepare_present};
    use crate::DirtyRect;
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
        assert!(
            state::lut_bypass_runtime()
                .and_then(|runtime| runtime.context(0x1234).cloned())
                .is_none()
        );
    }
}
