use std::ptr;

use crate::dwmcore::{
    self, DirtyRect, DirtyRectReadError, HardwareProtectedReadError, MonitorIdentityReadError,
    RectVec,
};
use crate::lifecycle;
use crate::lifecycle::PresentMode;
use crate::log::{self, FrameUnavailableReason};
use crate::state;
use dwm_lut_payload::MonitorIdentity;
use dwm_lut_profile::HookProfile;

#[derive(Debug, Clone, PartialEq)]
struct PresentInputs {
    monitor_identity: MonitorIdentity,
    dirty_rects: Vec<DirtyRect>,
}

#[derive(Debug, Clone, PartialEq)]
enum PresentInputOutcome {
    HardwareProtected,
    Ready(PresentInputs),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresentInputError {
    NullOverlaySwapChain,
    HardwareProtected(HardwareProtectedReadError),
    MonitorIdentity(MonitorIdentityReadError),
    DirtyRects(DirtyRectReadError),
}

pub(crate) fn present(
    this: usize,
    overlay_swap_chain: usize,
    rect_vec: usize,
    call_original: impl FnOnce(usize) -> i32,
) -> i32 {
    if lifecycle::present_mode().is_none() {
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

    let processed_frame;
    let prepared_rect_vec =
        match unsafe { collect_present_inputs(&profile, overlay_swap_chain, rect_vec) } {
            // Skip applying the LUT to prevent crashes with hardware DRM (e.g., PlayReady SL3000).
            Ok(PresentInputOutcome::HardwareProtected) => {
                log::skipped_hardware_protected(overlay_swap_chain);
                processed_frame = None;
                rect_vec
            }
            Ok(PresentInputOutcome::Ready(inputs)) => {
                processed_frame = process_frame(overlay_swap_chain, &profile, &inputs);
                match processed_frame.and_then(|outcome| outcome.present_dirty_rect) {
                    Some(rect) => full_present_rect_vec(
                        rect,
                        &mut present_rect_storage,
                        &mut present_rect_vec_storage,
                    ),
                    None => rect_vec,
                }
            }
            Err(error) => {
                log::present_input_collect_error(this, overlay_swap_chain, rect_vec, error);
                processed_frame = None;
                rect_vec
            }
        };
    let status = call_original(prepared_rect_vec);
    if status >= 0
        && let Some(token) = processed_frame.and_then(|outcome| outcome.restore_token)
    {
        crate::render::lock_renderer().commit_restore(token);
    }
    status
}

/// # Safety
///
/// `overlay_swap_chain` must point to a live object compatible with `profile`, and `rect_vec`
/// must point to a valid dirty-rect vector whose storage remains stable for this call.
unsafe fn collect_present_inputs(
    profile: &HookProfile,
    overlay_swap_chain: usize,
    rect_vec: usize,
) -> Result<PresentInputOutcome, PresentInputError> {
    if overlay_swap_chain == 0 {
        return Err(PresentInputError::NullOverlaySwapChain);
    }

    if unsafe { dwmcore::read_hardware_protected(overlay_swap_chain, profile.hardware_protected) }
        .map_err(PresentInputError::HardwareProtected)?
    {
        return Ok(PresentInputOutcome::HardwareProtected);
    }

    let monitor_identity =
        unsafe { dwmcore::read_monitor_identity(overlay_swap_chain, profile.monitor_identity) }
            .map_err(PresentInputError::MonitorIdentity)?;
    let dirty_rects =
        unsafe { dwmcore::read_dirty_rects(rect_vec) }.map_err(PresentInputError::DirtyRects)?;
    Ok(PresentInputOutcome::Ready(PresentInputs {
        monitor_identity,
        dirty_rects,
    }))
}

fn process_frame(
    overlay_swap_chain: usize,
    profile: &HookProfile,
    inputs: &PresentInputs,
) -> Option<crate::render::FrameOutcome> {
    let frame_processing_result = {
        let Some(mut renderer) = crate::render::try_lock_renderer() else {
            log::frame_unavailable(overlay_swap_chain, FrameUnavailableReason::RendererBusy);
            return None;
        };

        let mode = lifecycle::present_mode()?;
        match mode {
            PresentMode::Apply => {
                let Some(assignments) = state::assignments() else {
                    log::frame_unavailable(
                        overlay_swap_chain,
                        FrameUnavailableReason::AssignmentsUnavailable,
                    );
                    return None;
                };
                unsafe {
                    renderer.process_frame(
                        overlay_swap_chain,
                        profile.d3d11_resource,
                        inputs.monitor_identity,
                        &inputs.dirty_rects,
                        &assignments,
                    )
                }
            }
            PresentMode::Restore => unsafe {
                renderer.restore_back_buffer(
                    overlay_swap_chain,
                    profile.d3d11_resource,
                    inputs.monitor_identity,
                    &inputs.dirty_rects,
                )
            },
        }
    };

    match frame_processing_result {
        Err(error) => {
            log::frame_unavailable(overlay_swap_chain, FrameUnavailableReason::from(error));
            None
        }
        Ok(outcome) => {
            log::frame_processed(
                overlay_swap_chain,
                inputs.monitor_identity,
                &inputs.dirty_rects,
                outcome,
            );
            Some(outcome)
        }
    }
}

fn empty_rect_vec_storage() -> RectVec {
    RectVec {
        first: ptr::null(),
        last: ptr::null(),
        end: ptr::null(),
    }
}

fn full_present_rect_vec(
    rect: DirtyRect,
    rect_storage: &mut [DirtyRect; 1],
    rect_vec_storage: &mut RectVec,
) -> usize {
    rect_storage[0] = rect;
    let first = rect_storage.as_ptr();
    *rect_vec_storage = RectVec {
        first,
        last: unsafe { first.add(1) },
        end: unsafe { first.add(1) },
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
    pub(crate) use crate::test_support::representative_hook_profile as test_profile;
    use dwm_lut_profile::HookProfile;

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

    fn rect_vec_from(dirty_rects: &Vec<DirtyRect>) -> RectVec {
        if dirty_rects.is_empty() {
            RectVec {
                first: std::ptr::null(),
                last: std::ptr::null(),
                end: std::ptr::null(),
            }
        } else {
            let first = dirty_rects.as_ptr();
            RectVec {
                first,
                last: unsafe { first.add(dirty_rects.len()) },
                end: unsafe { first.add(dirty_rects.capacity()) },
            }
        }
    }

    pub(crate) struct FakePresentObjects {
        overlay_swap_chain: dwmcore::test_support::FakeOverlaySwapChain,
        pub(crate) dirty_rects: Vec<DirtyRect>,
        rect_vec: RectVec,
    }

    impl FakePresentObjects {
        pub(crate) fn new(dirty_rects: Vec<DirtyRect>, hardware_protected: bool) -> Self {
            let profile = test_profile();
            let overlay_swap_chain =
                dwmcore::test_support::fake_overlay_swap_chain_with_hardware_protected(
                    profile.monitor_identity,
                    profile.hardware_protected,
                    test_monitor_identity(),
                    hardware_protected,
                );
            let rect_vec = rect_vec_from(&dirty_rects);

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

    pub(crate) struct FakeVirtualPresentObjects {
        overlay_swap_chain: dwmcore::test_support::FakeOverlaySwapChain,
        pub(crate) dirty_rects: Vec<DirtyRect>,
        rect_vec: RectVec,
    }

    impl FakeVirtualPresentObjects {
        pub(crate) fn new(dirty_rects: Vec<DirtyRect>, hardware_protected: bool) -> Self {
            let profile = test_profile();
            let overlay_swap_chain =
                dwmcore::test_support::overlay_swap_chain_with_virtual_base_hardware_protected(
                    profile.monitor_identity,
                    test_monitor_identity(),
                    hardware_protected,
                );
            let rect_vec = rect_vec_from(&dirty_rects);

            Self {
                overlay_swap_chain,
                dirty_rects,
                rect_vec,
            }
        }

        pub(crate) fn profile(&self) -> HookProfile {
            let mut profile = test_profile();
            profile.hardware_protected =
                dwmcore::test_support::virtual_base_hardware_protected_access();
            profile
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
        FakePresentObjects, FakeVirtualPresentObjects, initialize_test_state,
        test_monitor_identity, test_profile,
    };
    use super::{
        PresentInputError, PresentInputOutcome, PresentInputs, collect_present_inputs, present,
        process_frame,
    };
    use crate::dwmcore::{self, DirtyRect};
    use crate::lifecycle;
    use crate::render::{DXGI_FORMAT_B8G8R8A8_UNORM, FrameSize, FrameStatus};
    use crate::state;
    use crate::state::HOOK_GLOBAL_TEST_LOCK;

    fn sample_outcome(
        present_dirty_rect: Option<DirtyRect>,
        status: FrameStatus,
    ) -> crate::render::FrameOutcome {
        crate::render::FrameOutcome {
            present_dirty_rect,
            status,
            dxgi_format: DXGI_FORMAT_B8G8R8A8_UNORM,
            frame_size: FrameSize {
                width: 1920,
                height: 1080,
            },
            lut_index: Some(0),
            restore_token: None,
            #[cfg(debug_assertions)]
            back_buffer_id: 1,
        }
    }

    fn sample_inputs(dirty_rects: Vec<DirtyRect>) -> PresentInputs {
        PresentInputs {
            monitor_identity: test_monitor_identity(),
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
            let outcome = unsafe {
                collect_present_inputs(
                    &test_profile(),
                    unprotected.overlay_swap_chain_address(),
                    unprotected.rect_vec_address(),
                )
            }
            .expect("present inputs should be collected");
            let PresentInputOutcome::Ready(inputs) = outcome else {
                panic!("expected Ready outcome");
            };
            assert_eq!(inputs.monitor_identity, test_monitor_identity());
            assert_eq!(inputs.dirty_rects, unprotected.dirty_rects);
        }

        #[test]
        fn returns_hardware_protected_without_reading_dirty_rects() {
            let fake = FakePresentObjects::new(
                vec![DirtyRect {
                    left: 10,
                    top: 20,
                    right: 30,
                    bottom: 40,
                }],
                true,
            );
            let outcome = unsafe {
                collect_present_inputs(
                    &test_profile(),
                    fake.overlay_swap_chain_address(),
                    0, // would fail if dirty rects were read
                )
            }
            .expect("hardware-protected outcome should not be an error");
            assert_eq!(outcome, PresentInputOutcome::HardwareProtected);
        }

        #[test]
        fn reads_virtual_base_overlay_outcomes() {
            let dirty_rects = vec![DirtyRect {
                left: 1,
                top: 2,
                right: 3,
                bottom: 4,
            }];

            let unprotected = FakeVirtualPresentObjects::new(dirty_rects.clone(), false);
            let outcome = unsafe {
                collect_present_inputs(
                    &unprotected.profile(),
                    unprotected.overlay_swap_chain_address(),
                    unprotected.rect_vec_address(),
                )
            }
            .expect("unprotected virtual-base inputs should be collected");
            let PresentInputOutcome::Ready(inputs) = outcome else {
                panic!("expected Ready outcome");
            };
            assert_eq!(inputs.monitor_identity, test_monitor_identity());
            assert_eq!(inputs.dirty_rects, unprotected.dirty_rects);

            let protected = FakeVirtualPresentObjects::new(dirty_rects, true);
            let outcome = unsafe {
                collect_present_inputs(
                    &protected.profile(),
                    protected.overlay_swap_chain_address(),
                    0,
                )
            }
            .expect("protected virtual-base outcome should not be an error");
            assert_eq!(outcome, PresentInputOutcome::HardwareProtected);
        }

        #[test]
        fn rejects_null_dirty_rect_vector() {
            let fake = FakePresentObjects::new(Vec::new(), false);
            let error = unsafe {
                collect_present_inputs(&test_profile(), fake.overlay_swap_chain_address(), 0)
            }
            .expect_err("null rectVec pointer should be rejected");

            assert_eq!(
                error,
                PresentInputError::DirtyRects(dwmcore::DirtyRectReadError::InvalidVector)
            );
        }
    }

    mod frame_processing {
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
            crate::render::set_fake_frame_result(Ok(sample_outcome(None, FrameStatus::LutApplied)));

            let outcome = process_frame(overlay_swap_chain, &test_profile(), &inputs)
                .expect("renderer should return the configured frame outcome");
            assert_eq!(outcome.present_dirty_rect, None);

            let frame_call =
                crate::render::fake_frame_call().expect("renderer should receive present inputs");
            assert_eq!(
                frame_call.operation,
                crate::render::FakeFrameOperation::Process
            );
            assert_eq!(frame_call.overlay_swap_chain, overlay_swap_chain);
            assert_eq!(frame_call.monitor_identity, test_monitor_identity());
            assert_eq!(frame_call.dirty_rects, dirty_rects);
        }

        #[test]
        fn routes_present_to_restore_processing_during_shutdown() {
            let _guard = HOOK_GLOBAL_TEST_LOCK
                .lock()
                .expect("test mutex should lock");
            initialize_test_state();
            crate::render::set_fake_frame_result(Ok(sample_outcome(
                Some(DirtyRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                }),
                FrameStatus::Restored,
            )));

            let shutdown = match lifecycle::begin_shutdown() {
                lifecycle::ShutdownStart::Started(transition) => transition,
                other => panic!("unexpected shutdown start: {other:?}"),
            };
            state::clear_assignments();

            let _ = process_frame(0x1234, &test_profile(), &sample_inputs(Vec::new()));

            assert_eq!(
                crate::render::fake_frame_call()
                    .expect("renderer should receive restore inputs")
                    .operation,
                crate::render::FakeFrameOperation::Restore
            );
            shutdown.finish_inactive();
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
        fn skips_lut_when_hardware_protected() {
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
                true,
            );
            crate::render::set_fake_frame_result(Ok(sample_outcome(
                Some(DirtyRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                }),
                FrameStatus::LutApplied,
            )));

            let original_rect_vec = fake.rect_vec_address();
            let mut forwarded_rect_vec = None;
            let status = present(
                0x1234,
                fake.overlay_swap_chain_address(),
                original_rect_vec,
                |rect_vec| {
                    forwarded_rect_vec = Some(rect_vec);
                    0x42
                },
            );

            assert_eq!(status, 0x42);
            assert_eq!(forwarded_rect_vec, Some(original_rect_vec));
            assert!(crate::render::fake_frame_call().is_none());
            crate::render::reset_fake_frame_result();
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
            crate::render::set_fake_frame_result(Ok(sample_outcome(
                Some(full_rect),
                FrameStatus::LutApplied,
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
            crate::render::reset_fake_frame_result();
        }

        #[test]
        fn commits_restore_only_after_original_present_succeeds() {
            let _guard = HOOK_GLOBAL_TEST_LOCK
                .lock()
                .expect("test mutex should lock");
            initialize_test_state();
            let fake = FakePresentObjects::new(
                vec![DirtyRect {
                    left: 0,
                    top: 0,
                    right: 64,
                    bottom: 64,
                }],
                false,
            );
            crate::render::set_fake_frame_result(Ok(sample_outcome(
                Some(DirtyRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                }),
                FrameStatus::Restored,
            )));
            crate::render::set_fake_restore_token(
                test_monitor_identity(),
                fake.overlay_swap_chain_address(),
            );
            let shutdown = match lifecycle::begin_shutdown() {
                lifecycle::ShutdownStart::Started(transition) => transition,
                other => panic!("unexpected shutdown start: {other:?}"),
            };

            assert_eq!(
                present(
                    0x1234,
                    fake.overlay_swap_chain_address(),
                    fake.rect_vec_address(),
                    |_| 0,
                ),
                0
            );
            assert!(crate::render::fake_restore_commit().is_some());

            shutdown.finish_inactive();
            state::reset_state_for_tests();
            crate::render::reset_fake_frame_result();
        }

        #[test]
        fn does_not_commit_restore_when_original_present_fails() {
            let _guard = HOOK_GLOBAL_TEST_LOCK
                .lock()
                .expect("test mutex should lock");
            initialize_test_state();
            let fake = FakePresentObjects::new(
                vec![DirtyRect {
                    left: 0,
                    top: 0,
                    right: 64,
                    bottom: 64,
                }],
                false,
            );
            crate::render::set_fake_frame_result(Ok(sample_outcome(
                Some(DirtyRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                }),
                FrameStatus::Restored,
            )));
            crate::render::set_fake_restore_token(
                test_monitor_identity(),
                fake.overlay_swap_chain_address(),
            );
            let shutdown = match lifecycle::begin_shutdown() {
                lifecycle::ShutdownStart::Started(transition) => transition,
                other => panic!("unexpected shutdown start: {other:?}"),
            };

            assert_eq!(
                present(
                    0x1234,
                    fake.overlay_swap_chain_address(),
                    fake.rect_vec_address(),
                    |_| -1,
                ),
                -1
            );
            assert!(crate::render::fake_restore_commit().is_none());

            shutdown.finish_inactive();
            state::reset_state_for_tests();
            crate::render::reset_fake_frame_result();
        }
    }
}
