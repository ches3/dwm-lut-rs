use std::ptr;

#[cfg(debug_assertions)]
use crate::d3d11::{BackBufferId, PresentDrawStatus};
use crate::dwmcore::{self, DirtyRect, DirtyRectReadError, RectVec};
use crate::lifecycle;
#[cfg(debug_assertions)]
use crate::log::SharedLimiter;
use crate::log::{self, PresentLutAcquireFailReason};
use crate::state;
use dwm_lut_payload::MonitorIdentity;
use dwm_lut_profile::HookProfile;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PresentInputs {
    monitor_identity: Option<MonitorIdentity>,
    dirty_rects: Vec<DirtyRect>,
    hardware_protected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresentInputError {
    NullOverlaySwapChain,
    InvalidDirtyRectVector,
    UnreadableMemory,
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PresentLutLogKey {
    overlay_swap_chain: usize,
    back_buffer: Option<BackBufferId>,
}

#[cfg(debug_assertions)]
static PRESENT_LUT_LOG_LIMITER: SharedLimiter<PresentLutLogKey> = SharedLimiter::new(300);

pub(crate) fn present(
    this: usize,
    overlay_swap_chain: usize,
    rect_vec: usize,
    call_original: impl FnOnce(usize) -> i64,
) -> i64 {
    if !lifecycle::is_runtime_active() {
        return call_original(rect_vec);
    }
    let Some(profile) = state::hook_profile() else {
        return call_original(rect_vec);
    };

    let mut present_rect_storage = [DirtyRect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    }];
    let mut present_rect_vec_storage = empty_rect_vec_storage();

    let prepared_rect_vec =
        match unsafe { collect_present_inputs(&profile, overlay_swap_chain, rect_vec) } {
            Ok(inputs) => match apply_lut(overlay_swap_chain, &profile, &inputs) {
                Some(rect) => full_present_rect_vec(
                    rect,
                    &mut present_rect_storage,
                    &mut present_rect_vec_storage,
                ),
                None => rect_vec,
            },
            Err(error) => {
                log::present_input_collect_error(this, overlay_swap_chain, rect_vec, error);
                rect_vec
            }
        };
    call_original(prepared_rect_vec)
}

unsafe fn collect_present_inputs(
    profile: &HookProfile,
    overlay_swap_chain: usize,
    rect_vec: usize,
) -> Result<PresentInputs, PresentInputError> {
    if overlay_swap_chain == 0 {
        return Err(PresentInputError::NullOverlaySwapChain);
    }

    let hardware_protected =
        dwmcore::read_hardware_protected(overlay_swap_chain, profile.hardware_protected_offset)
            .ok_or(PresentInputError::UnreadableMemory)?;
    let monitor_identity =
        dwmcore::read_monitor_identity(overlay_swap_chain, profile.monitor_identity_offsets);
    let dirty_rects =
        unsafe { dwmcore::read_dirty_rects(rect_vec) }.map_err(|error| match error {
            DirtyRectReadError::InvalidVector => PresentInputError::InvalidDirtyRectVector,
            DirtyRectReadError::UnreadableMemory => PresentInputError::UnreadableMemory,
        })?;
    Ok(PresentInputs {
        monitor_identity,
        dirty_rects,
        hardware_protected,
    })
}

fn apply_lut(
    overlay_swap_chain: usize,
    profile: &HookProfile,
    inputs: &PresentInputs,
) -> Option<DirtyRect> {
    let Some(_present_guard) = state::try_lock_present_runtime() else {
        log::present_lut_acquire_failed(overlay_swap_chain, PresentLutAcquireFailReason::LockMiss);
        return None;
    };

    if !lifecycle::is_runtime_active() {
        return None;
    }

    let Some(assignments) = state::assignments() else {
        log::present_lut_acquire_failed(
            overlay_swap_chain,
            PresentLutAcquireFailReason::from(crate::d3d11::RenderAcquireError::Unavailable),
        );
        return None;
    };

    match unsafe {
        crate::d3d11::render_present_lut(
            overlay_swap_chain,
            profile.swap_chain_to_resource_path,
            inputs.monitor_identity,
            &inputs.dirty_rects,
            &assignments,
        )
    } {
        Err(error) => {
            log::present_lut_acquire_failed(
                overlay_swap_chain,
                PresentLutAcquireFailReason::from(error),
            );
            None
        }
        Ok(render_outcome) => {
            log_present_lut_frame(
                overlay_swap_chain,
                inputs.hardware_protected,
                inputs.monitor_identity,
                &inputs.dirty_rects,
                render_outcome,
            );
            render_outcome.present_dirty_rect
        }
    }
}

fn log_present_lut_frame(
    overlay_swap_chain: usize,
    hardware_protected: bool,
    monitor_identity: Option<MonitorIdentity>,
    dirty_rects: &[DirtyRect],
    outcome: crate::d3d11::PresentLutOutcome,
) {
    #[cfg(debug_assertions)]
    {
        let is_err = matches!(outcome.draw, PresentDrawStatus::Failed(_));
        if !is_err {
            let key = PresentLutLogKey {
                overlay_swap_chain,
                back_buffer: outcome.back_buffer_id,
            };
            if !PRESENT_LUT_LOG_LIMITER.sample(key).should_log {
                return;
            }
        }
    }
    log::present_lut_frame(
        overlay_swap_chain,
        hardware_protected,
        monitor_identity,
        dirty_rects,
        outcome,
    );
}

fn empty_rect_vec_storage() -> RectVec {
    RectVec {
        start: ptr::null(),
        end: ptr::null(),
        capacity_end: ptr::null(),
    }
}

fn full_present_rect_vec(
    rect: DirtyRect,
    rect_storage: &mut [DirtyRect; 1],
    rect_vec_storage: &mut RectVec,
) -> usize {
    rect_storage[0] = rect;
    let start = rect_storage.as_ptr();
    *rect_vec_storage = RectVec {
        start,
        end: unsafe { start.add(1) },
        capacity_end: unsafe { start.add(1) },
    };
    (rect_vec_storage as *const RectVec) as usize
}

#[cfg(test)]
pub(crate) mod test_support {
    use dwm_lut_payload::{
        AdapterLuid, ColorMode, HookPayload, MonitorIdentity, MonitorTarget, PayloadAssignment,
        PayloadLut,
    };

    use crate::dwmcore::{self, DirtyRect, RectVec};
    use crate::resolver::SignatureResolutionReport;
    use crate::state;
    use dwm_lut_profile::{HookProfile, SUPPORTED_BUILDS};

    pub(crate) fn test_profile() -> HookProfile {
        (SUPPORTED_BUILDS
            .first()
            .expect("SUPPORTED_BUILDS is non-empty")
            .profiles
            .last()
            .expect("supported build must include profiles")
            .profile)()
    }

    pub(crate) fn test_monitor_identity() -> MonitorIdentity {
        MonitorIdentity {
            adapter_luid: AdapterLuid {
                high_part: 0,
                low_part: 0x14e02,
            },
            target_id: 4357,
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
            profile_name: "test".to_string(),
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
            flip_gate_enabled: true,
        }
    }

    pub(crate) fn initialize_test_state() {
        state::reset_state_for_tests();
        let profile = test_profile();
        let payload = test_payload(&[ColorMode::Sdr]);
        let resolution = SignatureResolutionReport::synthetic_for_tests(&profile);
        crate::bootstrap::initialize_with_resolution(profile, payload, resolution)
            .expect("initialization should succeed with synthetic resolution");
    }

    pub(crate) struct FakePresentObjects {
        overlay_swap_chain: Vec<u8>,
        pub(crate) dirty_rects: Vec<DirtyRect>,
        rect_vec: RectVec,
    }

    impl FakePresentObjects {
        pub(crate) fn new(dirty_rects: Vec<DirtyRect>, hardware_protected: bool) -> Self {
            let profile = test_profile();
            let overlay_swap_chain =
                dwmcore::test_support::overlay_swap_chain_bytes_with_hardware_protected(
                    profile.monitor_identity_offsets,
                    profile.hardware_protected_offset,
                    test_monitor_identity(),
                    hardware_protected,
                );

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
                overlay_swap_chain,
                dirty_rects,
                rect_vec,
            }
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
    use super::test_support::{
        FakePresentObjects, initialize_test_state, test_monitor_identity, test_profile,
    };
    use super::{PresentInputError, PresentInputs, apply_lut, collect_present_inputs, present};
    use crate::d3d11::{DXGI_FORMAT_B8G8R8A8_UNORM, PresentDrawStatus};
    use crate::dwmcore::{self, DirtyRect};
    use crate::lifecycle;
    use crate::state;
    use crate::state::HOOK_GLOBAL_TEST_LOCK;

    fn sample_outcome(
        present_dirty_rect: Option<DirtyRect>,
        draw: PresentDrawStatus,
    ) -> crate::d3d11::PresentLutOutcome {
        crate::d3d11::PresentLutOutcome {
            lut_active: present_dirty_rect.is_some()
                || matches!(draw, PresentDrawStatus::Applied { .. }),
            present_dirty_rect,
            draw,
            dxgi_format: Some(DXGI_FORMAT_B8G8R8A8_UNORM),
            width: None,
            height: None,
            lut_index: Some(0),
            #[cfg(debug_assertions)]
            back_buffer_id: None,
        }
    }

    fn sample_inputs(dirty_rects: Vec<DirtyRect>) -> PresentInputs {
        PresentInputs {
            monitor_identity: Some(test_monitor_identity()),
            hardware_protected: false,
            dirty_rects,
        }
    }

    mod collect {
        use super::*;

        #[test]
        fn reads_confirmed_inputs() {
            let unprotected = FakePresentObjects::new(
                vec![DirtyRect {
                    left: 10,
                    top: 20,
                    right: 30,
                    bottom: 40,
                }],
                false,
            );
            let inputs = unsafe {
                collect_present_inputs(
                    &test_profile(),
                    unprotected.overlay_swap_chain_address(),
                    unprotected.rect_vec_address(),
                )
            }
            .expect("present inputs should be collected");
            assert_eq!(inputs.monitor_identity, Some(test_monitor_identity()));
            assert_eq!(inputs.dirty_rects, unprotected.dirty_rects);
            assert!(!inputs.hardware_protected);

            let protected = FakePresentObjects::new(unprotected.dirty_rects.clone(), true);
            let inputs = unsafe {
                collect_present_inputs(
                    &test_profile(),
                    protected.overlay_swap_chain_address(),
                    protected.rect_vec_address(),
                )
            }
            .expect("hardware protected state should be collected");
            assert!(inputs.hardware_protected);
        }

        #[test]
        fn rejects_null_dirty_rect_vector() {
            let fake = FakePresentObjects::new(Vec::new(), false);
            let error = unsafe {
                collect_present_inputs(&test_profile(), fake.overlay_swap_chain_address(), 0)
            }
            .expect_err("null rectVec pointer should be rejected");

            assert_eq!(error, PresentInputError::InvalidDirtyRectVector);
        }
    }

    mod apply {
        use super::*;

        #[test]
        fn forwards_present_inputs_to_renderer() {
            let _guard = HOOK_GLOBAL_TEST_LOCK
                .lock()
                .expect("test mutex should lock");
            initialize_test_state();
            let overlay_swap_chain = 0x2222;
            let dirty_rects = vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }];
            let inputs = sample_inputs(dirty_rects.clone());
            crate::d3d11::set_fake_render_result(Ok(sample_outcome(
                None,
                PresentDrawStatus::Applied { full_redraw: false },
            )));

            assert_eq!(
                apply_lut(overlay_swap_chain, &test_profile(), &inputs),
                None
            );

            let render_call = crate::d3d11::fake_render_present_lut_call()
                .expect("renderer should receive present inputs");
            assert_eq!(render_call.overlay_swap_chain, overlay_swap_chain);
            assert_eq!(render_call.monitor_identity, Some(test_monitor_identity()));
            assert_eq!(render_call.dirty_rects, dirty_rects);
        }

        #[test]
        fn skips_render_when_shutdown_starts_after_entry_check() {
            let _guard = HOOK_GLOBAL_TEST_LOCK
                .lock()
                .expect("test mutex should lock");
            initialize_test_state();
            crate::d3d11::set_fake_render_result(Ok(sample_outcome(
                None,
                PresentDrawStatus::Applied { full_redraw: false },
            )));

            let shutdown = match lifecycle::begin_shutdown() {
                lifecycle::ShutdownStart::Started(transition) => transition,
                other => panic!("unexpected shutdown start: {other:?}"),
            };

            let _ = apply_lut(0x1234, &test_profile(), &sample_inputs(Vec::new()));

            assert!(crate::d3d11::fake_render_present_lut_call().is_none());
            shutdown.finish_shut_down();
            state::reset_state_for_tests();
        }
    }

    mod entry {
        use super::*;

        #[test]
        fn keeps_rect_vec_when_input_acquisition_fails() {
            let _guard = HOOK_GLOBAL_TEST_LOCK
                .lock()
                .expect("test mutex should lock");
            initialize_test_state();

            let forwarded = present(0x1234, 0, 0, |rect_vec| {
                assert_eq!(rect_vec, 0);
                0x42
            });
            assert_eq!(forwarded, 0x42);
        }

        #[test]
        fn forwards_expanded_rect_vec_when_apply_returns_dirty_rect() {
            let _guard = HOOK_GLOBAL_TEST_LOCK
                .lock()
                .expect("test mutex should lock");
            initialize_test_state();
            let fake = FakePresentObjects::new(
                vec![DirtyRect {
                    left: 10,
                    top: 20,
                    right: 64,
                    bottom: 96,
                }],
                false,
            );
            let full_rect = DirtyRect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            };
            crate::d3d11::set_fake_render_result(Ok(sample_outcome(
                Some(full_rect),
                PresentDrawStatus::Applied { full_redraw: true },
            )));

            let mut forwarded_rects = None;
            let status = present(
                0x1234,
                fake.overlay_swap_chain_address(),
                fake.rect_vec_address(),
                |rect_vec| {
                    forwarded_rects = unsafe { dwmcore::read_dirty_rects(rect_vec) }.ok();
                    0x42
                },
            );

            assert_eq!(status, 0x42);
            assert_eq!(forwarded_rects, Some(vec![full_rect]));
            crate::d3d11::reset_fake_render_result();
        }
    }
}
