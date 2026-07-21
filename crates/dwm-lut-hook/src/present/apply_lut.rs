#[cfg(debug_assertions)]
use std::collections::BTreeMap;
use std::ptr;
#[cfg(debug_assertions)]
use std::sync::{Mutex, OnceLock};

use crate::DirtyRect;
use crate::route_trace;
use crate::state;
use dwm_lut_payload::MonitorIdentity;

use super::collect::{PresentInputs, RectVec};

#[cfg(debug_assertions)]
const PRESENT_DIAGNOSTIC_SAMPLE_INTERVAL: u64 = 600;

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PresentDetourLogKey {
    overlay_swap_chain: usize,
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct PresentOriginalCallDetail {
    pub(crate) hardware_protected: bool,
    pub(crate) monitor_identity: Option<MonitorIdentity>,
    pub(crate) dirty_rect_count: usize,
    pub(crate) first_dirty_rect: Option<DirtyRect>,
    pub(crate) render_outcome: crate::d3d11_renderer::PresentLutOutcome,
    pub(crate) present_dirty_rect_source: &'static str,
}

#[cfg(debug_assertions)]
struct DiagnosticLogLimiter<K> {
    counts: BTreeMap<K, u64>,
}

#[cfg(debug_assertions)]
impl<K> Default for DiagnosticLogLimiter<K> {
    fn default() -> Self {
        Self {
            counts: BTreeMap::new(),
        }
    }
}

#[cfg(debug_assertions)]
impl<K: Ord> DiagnosticLogLimiter<K> {
    fn should_log(&mut self, key: K) -> bool {
        self.should_log_interval(key, PRESENT_DIAGNOSTIC_SAMPLE_INTERVAL)
    }

    fn should_log_interval(&mut self, key: K, interval: u64) -> bool {
        let count = self.counts.entry(key).or_insert(0);
        *count = count.saturating_add(1);
        *count == 1 || *count <= 8 || (*count).is_multiple_of(interval)
    }
}

#[cfg(debug_assertions)]
fn should_log_diagnostic<K: Ord>(
    limiter: &OnceLock<Mutex<DiagnosticLogLimiter<K>>>,
    key: K,
) -> bool {
    limiter
        .get_or_init(|| Mutex::new(DiagnosticLogLimiter::default()))
        .lock()
        .map(|mut limiter| limiter.should_log(key))
        .unwrap_or(true)
}

#[cfg(debug_assertions)]
static PRESENT_DETOUR_LOG_LIMITER: OnceLock<Mutex<DiagnosticLogLimiter<PresentDetourLogKey>>> =
    OnceLock::new();

#[cfg(debug_assertions)]
static HW_PRESENT_DETOUR_LOG_LIMITER: OnceLock<Mutex<DiagnosticLogLimiter<PresentDetourLogKey>>> =
    OnceLock::new();

#[cfg(debug_assertions)]
const HW_PRESENT_DETOUR_LOG_INTERVAL: u64 = 32;

#[cfg(debug_assertions)]
fn should_log_hw_present_detour_enter(overlay_swap_chain: usize) -> bool {
    HW_PRESENT_DETOUR_LOG_LIMITER
        .get_or_init(|| Mutex::new(DiagnosticLogLimiter::default()))
        .lock()
        .map(|mut limiter| {
            limiter.should_log_interval(
                PresentDetourLogKey { overlay_swap_chain },
                HW_PRESENT_DETOUR_LOG_INTERVAL,
            )
        })
        .unwrap_or(true)
}

fn should_log_present_detour_enter(overlay_swap_chain: usize, hardware_protected: bool) -> bool {
    #[cfg(debug_assertions)]
    {
        if hardware_protected {
            return should_log_hw_present_detour_enter(overlay_swap_chain);
        }
        should_log_diagnostic(
            &PRESENT_DETOUR_LOG_LIMITER,
            PresentDetourLogKey { overlay_swap_chain },
        )
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (overlay_swap_chain, hardware_protected);
        false
    }
}

fn emit_present_lut_acquire_error(
    overlay_swap_chain: usize,
    error: crate::d3d11_renderer::RenderAcquireError,
    should_log_frame: bool,
) {
    #[cfg(debug_assertions)]
    {
        if should_log_frame {
            debug_log!(
                "event=present_lut_frame overlay_swap_chain=0x{:x} acquired=0 reason={}",
                overlay_swap_chain,
                error.as_str()
            );
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = (overlay_swap_chain, error, should_log_frame);
}

fn emit_present_lut_outcome(
    overlay_swap_chain: usize,
    hardware_protected: bool,
    monitor_identity: Option<MonitorIdentity>,
    outcome: crate::d3d11_renderer::PresentLutOutcome,
    should_log_frame: bool,
) {
    #[cfg(debug_assertions)]
    {
        if should_log_frame {
            debug_log!(
                "event=present_lut_frame overlay_swap_chain=0x{:x} acquired=1 applied={} draw={} decision={:?} dxgi_format={:?} width={:?} height={:?} lut_index={:?} present_dirty_rect={:?} monitor_identity={} hardware_protected={}",
                overlay_swap_chain,
                u8::from(outcome.lut_applied()),
                outcome.draw.as_str(),
                outcome.decision,
                outcome.dxgi_format,
                outcome.width,
                outcome.height,
                outcome.lut_index,
                outcome.present_dirty_rect,
                crate::debug_log::quoted(format_monitor_identity_for_log(monitor_identity)),
                u8::from(hardware_protected)
            );
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = (
        overlay_swap_chain,
        hardware_protected,
        monitor_identity,
        outcome,
        should_log_frame,
    );
}

#[cfg(debug_assertions)]
fn format_monitor_identity_for_log(identity: Option<MonitorIdentity>) -> String {
    identity
        .map(|identity| format!("{}:{}", identity.adapter_luid, identity.target_id))
        .unwrap_or_else(|| "none".to_owned())
}

#[derive(Debug)]
pub(crate) struct ApplyOutcome {
    pub(crate) rect_vec: usize,
    #[cfg(debug_assertions)]
    pub(crate) last_present_context: Option<(bool, Option<MonitorIdentity>, Option<bool>)>,
    #[cfg(debug_assertions)]
    pub(crate) protected_resource_result_detail: Option<PresentOriginalCallDetail>,
}

pub(crate) fn apply_lut(
    this: usize,
    overlay_swap_chain: usize,
    inputs: &PresentInputs,
    rect_vec: usize,
    present_rect_storage: &mut [DirtyRect; 1],
    present_rect_vec_storage: &mut RectVec,
) -> ApplyOutcome {
    let mut outcome = ApplyOutcome {
        rect_vec,
        #[cfg(debug_assertions)]
        last_present_context: Some((inputs.hardware_protected, inputs.monitor_identity, None)),
        #[cfg(debug_assertions)]
        protected_resource_result_detail: None,
    };

    let should_log_frame =
        should_log_present_detour_enter(overlay_swap_chain, inputs.hardware_protected);
    if should_log_frame {
        debug_log!(
            "event=present_detour_enter this=0x{:x} overlay_swap_chain=0x{:x} rect_vec=0x{:x}",
            this,
            overlay_swap_chain,
            rect_vec
        );
    }
    route_trace::record_present_enter(overlay_swap_chain, inputs.hardware_protected);

    let Some(_present_guard) = state::try_lock_present_runtime() else {
        route_trace::record_present_lock_miss(overlay_swap_chain);
        return outcome;
    };

    match render_present_lut_if_active(
        overlay_swap_chain,
        inputs.monitor_identity,
        inputs.hardware_protected,
        &inputs.dirty_rects,
    ) {
        None => {}
        Some(Err(error)) => {
            emit_present_lut_acquire_error(overlay_swap_chain, error, should_log_frame);
        }
        Some(Ok(render_outcome)) => {
            emit_present_lut_outcome(
                overlay_swap_chain,
                inputs.hardware_protected,
                inputs.monitor_identity,
                render_outcome,
                should_log_frame,
            );
            #[cfg(debug_assertions)]
            {
                outcome.last_present_context = Some((
                    inputs.hardware_protected,
                    inputs.monitor_identity,
                    Some(render_outcome.lut_applied()),
                ));
                if inputs.hardware_protected {
                    outcome.protected_resource_result_detail = Some(PresentOriginalCallDetail {
                        hardware_protected: inputs.hardware_protected,
                        monitor_identity: inputs.monitor_identity,
                        dirty_rect_count: inputs.dirty_rects.len(),
                        first_dirty_rect: inputs.dirty_rects.first().copied(),
                        render_outcome,
                        present_dirty_rect_source: "original",
                    });
                }
            }
            route_trace::record_present_lut_result(
                inputs.hardware_protected,
                render_outcome.lut_applied(),
            );
            let present_dirty_rect_source = if let Some(rect) = render_outcome.present_dirty_rect {
                outcome.rect_vec =
                    full_present_rect_vec(rect, present_rect_storage, present_rect_vec_storage);
                "expanded"
            } else {
                "original"
            };
            #[cfg(debug_assertions)]
            {
                if inputs.hardware_protected {
                    outcome.protected_resource_result_detail = Some(PresentOriginalCallDetail {
                        hardware_protected: inputs.hardware_protected,
                        monitor_identity: inputs.monitor_identity,
                        dirty_rect_count: inputs.dirty_rects.len(),
                        first_dirty_rect: inputs.dirty_rects.first().copied(),
                        render_outcome,
                        present_dirty_rect_source,
                    });
                }
            }
            #[cfg(not(debug_assertions))]
            let _ = present_dirty_rect_source;
            state::update_present_context(this, render_outcome.decision);
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

pub(crate) fn render_present_lut_if_active(
    overlay_swap_chain: usize,
    monitor_identity: Option<MonitorIdentity>,
    hardware_protected: bool,
    dirty_rects: &[DirtyRect],
) -> Option<
    Result<crate::d3d11_renderer::PresentLutOutcome, crate::d3d11_renderer::RenderAcquireError>,
> {
    if !state::is_runtime_active() {
        return None;
    }
    Some(state::render_present_lut(
        overlay_swap_chain,
        monitor_identity,
        hardware_protected,
        dirty_rects,
    ))
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
        activate_context, initialize_test_state, initialize_test_state_from_payload,
        test_monitor_identity, test_payload,
    };
    use super::{apply_lut, empty_rect_vec_storage, render_present_lut_if_active};
    use crate::state;
    use crate::state::HOOK_GLOBAL_TEST_LOCK;
    use crate::{BackBufferFormat, DXGI_FORMAT_R16G16B16A16_FLOAT, DirtyRect};

    fn sample_inputs(hardware_protected: bool, dirty_rects: Vec<DirtyRect>) -> PresentInputs {
        PresentInputs {
            monitor_identity: Some(test_monitor_identity()),
            dirty_rects,
            hardware_protected,
        }
    }

    fn run_apply(
        this: usize,
        overlay_swap_chain: usize,
        inputs: &PresentInputs,
    ) -> super::ApplyOutcome {
        let mut present_rect_storage = [DirtyRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }];
        let mut present_rect_vec_storage = empty_rect_vec_storage();
        apply_lut(
            this,
            overlay_swap_chain,
            inputs,
            0,
            &mut present_rect_storage,
            &mut present_rect_vec_storage,
        )
    }

    #[test]
    fn apply_lut_updates_context_when_render_succeeds() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        initialize_test_state();
        let this = 0x1111;
        let overlay_swap_chain = 0x2222;
        let dirty_rects = vec![DirtyRect {
            left: 0,
            top: 0,
            right: 64,
            bottom: 64,
        }];
        let inputs = sample_inputs(false, dirty_rects.clone());
        crate::d3d11_renderer::set_fake_render_result(Ok(
            crate::d3d11_renderer::PresentLutOutcome {
                decision: crate::lut_pipeline::LutDecision::Apply {
                    format: BackBufferFormat::Bgra8Unorm,
                    lut_index: 0,
                },
                present_dirty_rect: None,
                draw: crate::d3d11_renderer::PresentDrawStatus::Applied { full_redraw: false },
                dxgi_format: Some(crate::lut_pipeline::DXGI_FORMAT_B8G8R8A8_UNORM),
                width: None,
                height: None,
                lut_index: Some(0),
            },
        ));

        let _ = run_apply(this, overlay_swap_chain, &inputs);

        let context = state::lut_bypass_runtime()
            .and_then(|runtime| runtime.context(this).cloned())
            .expect("successful LUT render should keep the context active");
        assert_eq!(context.lut_index, 0);
        assert_eq!(context.back_buffer_format, BackBufferFormat::Bgra8Unorm);
        let render_call = crate::d3d11_renderer::fake_render_present_lut_call()
            .expect("renderer should receive present inputs");
        assert_eq!(render_call.overlay_swap_chain, overlay_swap_chain);
        assert_eq!(render_call.monitor_identity, Some(test_monitor_identity()));
        assert_eq!(render_call.dirty_rects, dirty_rects);
        assert_eq!(
            crate::d3d11_renderer::fake_render_context_active(),
            Some(false)
        );
    }

    #[test]
    fn apply_lut_records_decision_format_into_bypass_state() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        initialize_test_state_from_payload(test_payload(&[ColorMode::Sdr, ColorMode::Hdr]));
        let this = 0x1111;
        let inputs = sample_inputs(
            false,
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
        );
        crate::d3d11_renderer::set_fake_render_result(Ok(
            crate::d3d11_renderer::PresentLutOutcome {
                decision: crate::lut_pipeline::LutDecision::Apply {
                    format: BackBufferFormat::Rgba16Float,
                    lut_index: 1,
                },
                present_dirty_rect: None,
                draw: crate::d3d11_renderer::PresentDrawStatus::Applied { full_redraw: false },
                dxgi_format: Some(DXGI_FORMAT_R16G16B16A16_FLOAT),
                width: None,
                height: None,
                lut_index: Some(1),
            },
        ));

        let _ = run_apply(this, 0x2222, &inputs);

        let context = state::lut_bypass_runtime()
            .and_then(|runtime| runtime.context(this).cloned())
            .expect("HDR render plan should keep the context active");
        assert_eq!(context.back_buffer_format, BackBufferFormat::Rgba16Float);
        assert_eq!(context.lut_index, 1);
        crate::d3d11_renderer::reset_fake_render_result();
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
        crate::d3d11_renderer::set_fake_render_result(Ok(
            crate::d3d11_renderer::PresentLutOutcome {
                decision: crate::lut_pipeline::LutDecision::Apply {
                    format: BackBufferFormat::Bgra8Unorm,
                    lut_index: 0,
                },
                present_dirty_rect: Some(full_rect),
                draw: crate::d3d11_renderer::PresentDrawStatus::Applied { full_redraw: true },
                dxgi_format: Some(crate::lut_pipeline::DXGI_FORMAT_B8G8R8A8_UNORM),
                width: None,
                height: None,
                lut_index: Some(0),
            },
        ));

        let mut present_rect_storage = [DirtyRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }];
        let mut present_rect_vec_storage = empty_rect_vec_storage();
        let outcome = apply_lut(
            0x1111,
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
        let render_call = crate::d3d11_renderer::fake_render_present_lut_call()
            .expect("renderer should still receive original dirty rects");
        assert_eq!(render_call.dirty_rects, dirty_rects);
    }

    #[test]
    fn apply_lut_keeps_context_when_draw_fails_but_decision_applies() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        initialize_test_state();
        let this = 0x1111;
        activate_context(this);
        let inputs = sample_inputs(
            false,
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
        );
        crate::d3d11_renderer::set_fake_render_result(Ok(
            crate::d3d11_renderer::PresentLutOutcome {
                decision: crate::lut_pipeline::LutDecision::Apply {
                    format: BackBufferFormat::Bgra8Unorm,
                    lut_index: 0,
                },
                present_dirty_rect: None,
                draw: crate::d3d11_renderer::PresentDrawStatus::Failed(
                    crate::d3d11_renderer::PresentDrawFailReason::DrawFailed,
                ),
                dxgi_format: Some(crate::lut_pipeline::DXGI_FORMAT_B8G8R8A8_UNORM),
                width: None,
                height: None,
                lut_index: Some(0),
            },
        ));

        let _ = run_apply(this, 0x2222, &inputs);

        let context = state::lut_bypass_runtime()
            .and_then(|runtime| runtime.context(this).cloned())
            .expect("apply decision should keep the context active across a missed render");
        assert_eq!(context.lut_index, 0);
        assert_eq!(context.back_buffer_format, BackBufferFormat::Bgra8Unorm);
        crate::d3d11_renderer::reset_fake_render_result();
    }

    #[test]
    fn apply_lut_clears_context_when_decision_is_not_applicable() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        initialize_test_state();
        let this = 0x1111;
        activate_context(this);
        let inputs = sample_inputs(
            false,
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
        );
        crate::d3d11_renderer::set_fake_render_result(Ok(
            crate::d3d11_renderer::PresentLutOutcome {
                decision: crate::lut_pipeline::LutDecision::NotApplicable,
                present_dirty_rect: None,
                draw: crate::d3d11_renderer::PresentDrawStatus::Skipped(
                    crate::d3d11_renderer::DrawPlanSkipReason::MissingAssignment,
                ),
                dxgi_format: Some(DXGI_FORMAT_R16G16B16A16_FLOAT),
                width: None,
                height: None,
                lut_index: None,
            },
        ));

        let _ = run_apply(this, 0x2222, &inputs);

        assert!(
            state::lut_bypass_runtime()
                .and_then(|runtime| runtime.context(this).cloned())
                .is_none()
        );
        crate::d3d11_renderer::reset_fake_render_result();
    }

    #[test]
    fn apply_lut_leaves_context_unchanged_when_acquire_fails() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        state::reset_state_for_tests();
        initialize_test_state_from_payload(test_payload(&[ColorMode::Hdr]));
        let this = 0x1111;
        state::update_present_context(
            this,
            crate::lut_pipeline::LutDecision::Apply {
                format: BackBufferFormat::Rgba16Float,
                lut_index: 0,
            },
        );
        crate::d3d11_renderer::reset_fake_render_result();
        let inputs = sample_inputs(
            false,
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 64,
                bottom: 64,
            }],
        );

        let _ = run_apply(this, 0x2222, &inputs);

        let context = state::lut_bypass_runtime()
            .and_then(|runtime| runtime.context(this).cloned())
            .expect("acquire failure must not clear an active HDR context");
        assert_eq!(context.back_buffer_format, BackBufferFormat::Rgba16Float);
        assert_eq!(context.lut_index, 0);
    }

    #[test]
    fn apply_lut_forwards_hardware_protected_flag_to_renderer() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        initialize_test_state();
        let overlay_swap_chain = 0x2222;
        let inputs = sample_inputs(true, Vec::new());
        crate::d3d11_renderer::set_fake_render_result(Ok(
            crate::d3d11_renderer::PresentLutOutcome {
                decision: crate::lut_pipeline::LutDecision::Apply {
                    format: BackBufferFormat::Bgra8Unorm,
                    lut_index: 0,
                },
                present_dirty_rect: None,
                draw: crate::d3d11_renderer::PresentDrawStatus::Applied { full_redraw: false },
                dxgi_format: Some(crate::lut_pipeline::DXGI_FORMAT_B8G8R8A8_UNORM),
                width: None,
                height: None,
                lut_index: Some(0),
            },
        ));

        let _ = run_apply(0x1111, overlay_swap_chain, &inputs);

        let render_call = crate::d3d11_renderer::fake_render_present_lut_call()
            .expect("hardware protected present should reach renderer");
        assert_eq!(render_call.overlay_swap_chain, overlay_swap_chain);
        assert_eq!(render_call.monitor_identity, Some(test_monitor_identity()));
        assert!(render_call.hardware_protected);
        assert!(render_call.dirty_rects.is_empty());
        crate::d3d11_renderer::reset_fake_render_result();
    }

    #[test]
    fn render_present_lut_is_skipped_when_shutdown_starts_after_entry_check() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        initialize_test_state();
        crate::d3d11_renderer::set_fake_render_result(Ok(
            crate::d3d11_renderer::PresentLutOutcome {
                decision: crate::lut_pipeline::LutDecision::Apply {
                    format: BackBufferFormat::Bgra8Unorm,
                    lut_index: 0,
                },
                present_dirty_rect: None,
                draw: crate::d3d11_renderer::PresentDrawStatus::Applied { full_redraw: false },
                dxgi_format: Some(crate::lut_pipeline::DXGI_FORMAT_B8G8R8A8_UNORM),
                width: None,
                height: None,
                lut_index: Some(0),
            },
        ));

        assert_eq!(state::begin_shutdown(), state::ShutdownStart::Started);

        let render_result =
            render_present_lut_if_active(0x1234, Some(test_monitor_identity()), false, &[]);

        assert!(render_result.is_none());
        assert!(crate::d3d11_renderer::fake_render_present_lut_call().is_none());
        state::reset_state_for_tests();
    }
}
