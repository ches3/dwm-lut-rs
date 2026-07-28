use std::ptr;

use super::DirtyRect;
#[cfg(debug_assertions)]
use crate::d3d11::{BackBufferId, PresentDrawStatus};
#[cfg(debug_assertions)]
use crate::log::SharedLimiter;
use crate::log::{self, PresentLutAcquireFailReason};
use crate::state;
use dwm_lut_payload::MonitorIdentity;

use super::collect::{PresentInputs, RectVec};

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PresentLutLogKey {
    overlay_swap_chain: usize,
    back_buffer: Option<BackBufferId>,
}

#[cfg(debug_assertions)]
static PRESENT_LUT_LOG_LIMITER: SharedLimiter<PresentLutLogKey> = SharedLimiter::new(300);

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

#[derive(Debug)]
pub(crate) struct ApplyOutcome {
    pub(crate) rect_vec: usize,
}

pub(crate) fn apply_lut(
    overlay_swap_chain: usize,
    inputs: &PresentInputs,
    rect_vec: usize,
    present_rect_storage: &mut [DirtyRect; 1],
    present_rect_vec_storage: &mut RectVec,
) -> ApplyOutcome {
    let mut outcome = ApplyOutcome { rect_vec };

    let Some(_present_guard) = state::try_lock_present_runtime() else {
        log::present_lut_acquire_failed(overlay_swap_chain, PresentLutAcquireFailReason::LockMiss);
        return outcome;
    };

    if !state::is_runtime_active() {
        return outcome;
    }

    let Some(assignments) = state::assignments() else {
        log::present_lut_acquire_failed(
            overlay_swap_chain,
            PresentLutAcquireFailReason::from(crate::d3d11::RenderAcquireError::Unavailable),
        );
        return outcome;
    };
    let Some(profile) = state::hook_profile() else {
        log::present_lut_acquire_failed(
            overlay_swap_chain,
            PresentLutAcquireFailReason::from(crate::d3d11::RenderAcquireError::Unavailable),
        );
        return outcome;
    };

    match unsafe {
        crate::d3d11::render_present_lut(
            overlay_swap_chain,
            profile.swap_chain,
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
        }
        Ok(render_outcome) => {
            log_present_lut_frame(
                overlay_swap_chain,
                inputs.hardware_protected,
                inputs.monitor_identity,
                &inputs.dirty_rects,
                render_outcome,
            );
            if let Some(rect) = render_outcome.present_dirty_rect {
                outcome.rect_vec =
                    full_present_rect_vec(rect, present_rect_storage, present_rect_vec_storage);
            }
        }
    }

    outcome
}

pub(crate) fn empty_rect_vec_storage() -> RectVec {
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
mod tests {
    use dwm_lut_payload::ColorMode;

    use super::super::collect::{PresentInputs, read_dirty_rects};
    use super::super::test_support::{
        initialize_test_state, initialize_test_state_from_payload, test_monitor_identity,
        test_payload,
    };
    use super::DirtyRect;
    use super::{ApplyOutcome, apply_lut, empty_rect_vec_storage};
    use crate::d3d11::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT};
    use crate::state;
    use crate::state::HOOK_GLOBAL_TEST_LOCK;

    fn sample_outcome(
        lut_active: bool,
        lut_index: Option<usize>,
        dxgi_format: Option<u32>,
        draw: crate::d3d11::PresentDrawStatus,
        present_dirty_rect: Option<DirtyRect>,
    ) -> crate::d3d11::PresentLutOutcome {
        crate::d3d11::PresentLutOutcome {
            lut_active,
            present_dirty_rect,
            draw,
            dxgi_format,
            width: None,
            height: None,
            lut_index,
            #[cfg(debug_assertions)]
            back_buffer_id: None,
        }
    }

    fn sample_inputs(hardware_protected: bool, dirty_rects: Vec<DirtyRect>) -> PresentInputs {
        PresentInputs {
            monitor_identity: Some(test_monitor_identity()),
            hardware_protected,
            dirty_rects,
        }
    }

    fn run_apply(overlay_swap_chain: usize, inputs: &PresentInputs) -> ApplyOutcome {
        let mut present_rect_storage = [DirtyRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }];
        let mut present_rect_vec_storage = empty_rect_vec_storage();
        apply_lut(
            overlay_swap_chain,
            inputs,
            0xdead,
            &mut present_rect_storage,
            &mut present_rect_vec_storage,
        )
    }

    #[test]
    fn apply_lut_forwards_present_inputs_when_render_succeeds() {
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
        let inputs = sample_inputs(false, dirty_rects.clone());
        crate::d3d11::set_fake_render_result(Ok(sample_outcome(
            true,
            Some(0),
            Some(DXGI_FORMAT_B8G8R8A8_UNORM),
            crate::d3d11::PresentDrawStatus::Applied { full_redraw: false },
            None,
        )));

        let _ = run_apply(overlay_swap_chain, &inputs);

        let render_call = crate::d3d11::fake_render_present_lut_call()
            .expect("renderer should receive present inputs");
        assert_eq!(render_call.overlay_swap_chain, overlay_swap_chain);
        assert_eq!(render_call.monitor_identity, Some(test_monitor_identity()));
        assert_eq!(render_call.dirty_rects, dirty_rects);
    }

    #[test]
    fn apply_lut_renders_hdr_assignment() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        initialize_test_state_from_payload(test_payload(&[ColorMode::Sdr, ColorMode::Hdr]));
        let inputs = sample_inputs(
            false,
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
        );
        crate::d3d11::set_fake_render_result(Ok(sample_outcome(
            true,
            Some(1),
            Some(DXGI_FORMAT_R16G16B16A16_FLOAT),
            crate::d3d11::PresentDrawStatus::Applied { full_redraw: false },
            None,
        )));

        let _ = run_apply(0x2222, &inputs);

        assert!(crate::d3d11::fake_render_present_lut_call().is_some());
        crate::d3d11::reset_fake_render_result();
    }

    #[test]
    fn apply_lut_expands_rect_vec_for_full_redraw() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        initialize_test_state();
        let dirty_rects = vec![DirtyRect {
            left: 10,
            top: 20,
            right: 64,
            bottom: 96,
        }];
        let inputs = sample_inputs(false, dirty_rects.clone());
        let full_rect = DirtyRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        crate::d3d11::set_fake_render_result(Ok(sample_outcome(
            true,
            Some(0),
            Some(DXGI_FORMAT_B8G8R8A8_UNORM),
            crate::d3d11::PresentDrawStatus::Applied { full_redraw: true },
            Some(full_rect),
        )));

        let mut present_rect_storage = [DirtyRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }];
        let mut present_rect_vec_storage = empty_rect_vec_storage();
        let outcome = apply_lut(
            0x2222,
            &inputs,
            0xdead,
            &mut present_rect_storage,
            &mut present_rect_vec_storage,
        );

        assert_ne!(outcome.rect_vec, 0xdead);
        assert_eq!(
            unsafe { read_dirty_rects(outcome.rect_vec) }.expect("expanded rect vec"),
            vec![full_rect]
        );
        let render_call = crate::d3d11::fake_render_present_lut_call()
            .expect("renderer should still receive original dirty rects");
        assert_eq!(render_call.dirty_rects, dirty_rects);
    }

    #[test]
    fn apply_lut_accepts_skipped_and_failed_draw_outcomes() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        initialize_test_state();
        let inputs = sample_inputs(
            false,
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
        );

        crate::d3d11::set_fake_render_result(Ok(sample_outcome(
            false,
            None,
            Some(DXGI_FORMAT_R16G16B16A16_FLOAT),
            crate::d3d11::PresentDrawStatus::Skipped(
                crate::d3d11::DrawPlanSkipReason::MissingAssignment,
            ),
            None,
        )));
        let _ = run_apply(0x2222, &inputs);
        assert!(crate::d3d11::fake_render_present_lut_call().is_some());

        crate::d3d11::set_fake_render_result(Ok(sample_outcome(
            true,
            Some(0),
            Some(DXGI_FORMAT_B8G8R8A8_UNORM),
            crate::d3d11::PresentDrawStatus::Failed(
                crate::d3d11::PresentDrawFailReason::DrawFailed,
            ),
            None,
        )));
        let _ = run_apply(0x2222, &inputs);
        assert!(crate::d3d11::fake_render_present_lut_call().is_some());
        crate::d3d11::reset_fake_render_result();
    }

    #[test]
    fn apply_lut_skips_render_when_shutdown_starts_after_entry_check() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        initialize_test_state();
        crate::d3d11::set_fake_render_result(Ok(sample_outcome(
            true,
            Some(0),
            Some(DXGI_FORMAT_B8G8R8A8_UNORM),
            crate::d3d11::PresentDrawStatus::Applied { full_redraw: false },
            None,
        )));

        assert_eq!(state::begin_shutdown(), state::ShutdownStart::Started);

        let _ = run_apply(0x1234, &sample_inputs(false, Vec::new()));

        assert!(crate::d3d11::fake_render_present_lut_call().is_none());
        state::reset_state_for_tests();
    }
}
