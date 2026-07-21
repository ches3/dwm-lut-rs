//! Present-path and flip-gate diagnostics for route investigation (debug builds only).

#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(usize)]
pub enum FlipGateKind {
    OverlayContextDirectFlip,
    DirectFlipInfoEnsureIndependentFlip,
    IsDirectFlipSupportedOnTarget,
    LegacySwapChainCheckDirectFlip,
    IsAdvancedDirectFlipCompatible,
}

impl FlipGateKind {
    const COUNT: usize = 5;

    const fn label(self) -> &'static str {
        match self {
            Self::OverlayContextDirectFlip => "overlay_context_direct_flip",
            Self::DirectFlipInfoEnsureIndependentFlip => "direct_flip_info_ensure_independent_flip",
            Self::IsDirectFlipSupportedOnTarget => "is_direct_flip_supported_on_target",
            Self::LegacySwapChainCheckDirectFlip => "legacy_swap_chain_check_direct_flip",
            Self::IsAdvancedDirectFlipCompatible => "is_advanced_direct_flip_compatible",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

pub fn present_sample_interval() -> u64 {
    imp::present_sample_interval()
}

pub fn record_present_enter(overlay_swap_chain: usize, hardware_protected: bool) {
    imp::record_present_enter(overlay_swap_chain, hardware_protected);
}

pub fn record_present_lock_miss(overlay_swap_chain: usize) {
    imp::record_present_lock_miss(overlay_swap_chain);
}

pub fn record_present_lut_result(hardware_protected: bool, lut_applied: bool) {
    imp::record_present_lut_result(hardware_protected, lut_applied);
}

pub fn record_last_present_context(
    overlay_swap_chain: usize,
    monitor_identity: Option<dwm_lut_payload::MonitorIdentity>,
    hardware_protected: bool,
    lut_applied: Option<bool>,
    original_present_result: Option<i64>,
) -> u64 {
    imp::record_last_present_context(
        overlay_swap_chain,
        monitor_identity,
        hardware_protected,
        lut_applied,
        original_present_result,
    )
}

pub fn record_last_present_original_result(sequence: u64, original_present_result: i64) {
    imp::record_last_present_original_result(sequence, original_present_result);
}

pub fn record_flip_gate(kind: FlipGateKind, original: bool, result: bool) {
    imp::record_flip_gate(kind, original, result);
}

#[allow(clippy::too_many_arguments)]
pub fn record_protected_lut_resource_candidate(
    overlay_swap_chain: usize,
    monitor_identity: Option<dwm_lut_payload::MonitorIdentity>,
    hardware_protected: bool,
    back_buffer: usize,
    device: usize,
    context: usize,
    dxgi_format: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    lut_applied: bool,
) {
    imp::record_protected_lut_resource_candidate(
        overlay_swap_chain,
        monitor_identity,
        hardware_protected,
        back_buffer,
        device,
        context,
        dxgi_format,
        width,
        height,
        lut_applied,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn record_protected_present_resource_result_summary(
    overlay_swap_chain: usize,
    monitor_identity: Option<dwm_lut_payload::MonitorIdentity>,
    hardware_protected: bool,
    original_result: i64,
    render_result: crate::d3d11_renderer::PresentLutOutcome,
    dirty_rect_count: usize,
    first_dirty_rect: Option<crate::DirtyRect>,
    present_rect_override_enabled: bool,
    present_rect_override: Option<crate::DirtyRect>,
) {
    imp::record_protected_present_resource_result_summary(
        overlay_swap_chain,
        monitor_identity,
        hardware_protected,
        original_result,
        render_result,
        dirty_rect_count,
        first_dirty_rect,
        present_rect_override_enabled,
        present_rect_override,
    );
}

pub fn flush_summary(reason: &str) {
    imp::flush_summary(reason);
}

#[cfg(debug_assertions)]
mod imp {
    use super::FlipGateKind;
    use dwm_lut_payload::MonitorIdentity;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    macro_rules! trace_log {
        ($($arg:tt)*) => {
            crate::debug_log::write(format_args!($($arg)*))
        };
    }

    static PRESENT_ENTERS: AtomicU64 = AtomicU64::new(0);
    static PRESENT_HARDWARE_PROTECTED: AtomicU64 = AtomicU64::new(0);
    static PRESENT_LOCK_MISSES: AtomicU64 = AtomicU64::new(0);
    static PRESENT_LUT_APPLIED: AtomicU64 = AtomicU64::new(0);
    static PRESENT_LUT_NOT_APPLIED: AtomicU64 = AtomicU64::new(0);
    static FLIP_GATE_CALLS: [AtomicU64; FlipGateKind::COUNT] = [
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ];
    static FLIP_GATE_DENIED: [AtomicU64; FlipGateKind::COUNT] = [
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ];
    static LAST_PRESENT_NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    const PRESENT_SAMPLE_INTERVAL: u64 = 600;
    const FLUSH_EVERY_PRESENTS: u64 = 120;
    const IDLE_GAP_LOG_THRESHOLD: Duration = Duration::from_secs(2);
    const PROTECTED_FLIP_GATE_SEQUENCE_SUMMARY_BURST: u64 = 16;
    const PROTECTED_FLIP_GATE_SEQUENCE_SUMMARY_EVERY: u64 = 600;
    const PROTECTED_PRESENT_RESOURCE_RESULT_SUMMARY_BURST: u64 = 16;
    const PROTECTED_PRESENT_RESOURCE_RESULT_SUMMARY_EVERY: u64 = 600;
    const PROTECTED_LUT_RESOURCE_CANDIDATE_BURST: u64 = 16;
    const PROTECTED_LUT_RESOURCE_CANDIDATE_EVERY: u64 = 600;
    struct TraceState {
        last_present_at: BTreeMap<usize, Instant>,
        last_idle_gap_logged_at: BTreeMap<usize, Instant>,
        presents_since_flush: u64,
        protected_flip_gate_sequence: u64,
        protected_flip_gate_log_count: u64,
        protected_flip_gate_stats: [FlipGateSequenceGateStats; FlipGateKind::COUNT],
        protected_present_resource_result_summary_count: u64,
        protected_present_resource_result_last_original_result: Option<i64>,
        protected_lut_resource_candidate_count: u64,
        protected_lut_resource_candidate_last_key: Option<ProtectedLutResourceCandidateLogKey>,
    }

    #[derive(Clone, Copy)]
    struct FlipGateSequenceGateStats {
        calls: u64,
        original_true: u64,
        returned_true: u64,
        denied: u64,
    }

    impl FlipGateSequenceGateStats {
        const fn new() -> Self {
            Self {
                calls: 0,
                original_true: 0,
                returned_true: 0,
                denied: 0,
            }
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct ProtectedLutResourceCandidateLogKey {
        target_id: Option<u32>,
        dxgi_format: Option<u32>,
        width: Option<u32>,
        height: Option<u32>,
        lut_applied: bool,
    }

    #[derive(Debug, Clone, Copy)]
    struct ProtectedLutResourceSnapshot {
        sequence: u64,
        overlay_swap_chain: usize,
        monitor_identity: Option<MonitorIdentity>,
        target_id: Option<u32>,
        back_buffer: usize,
        device: usize,
        context: usize,
        dxgi_format: Option<u32>,
        width: Option<u32>,
        height: Option<u32>,
        lut_applied: bool,
        recorded_at: Option<Instant>,
    }

    impl ProtectedLutResourceSnapshot {
        fn matches_present(
            self,
            overlay_swap_chain: usize,
            monitor_identity: Option<MonitorIdentity>,
            render_result: crate::d3d11_renderer::PresentLutOutcome,
        ) -> bool {
            self.overlay_swap_chain == overlay_swap_chain
                && monitor_identity_matches(self.monitor_identity, monitor_identity)
                && self.dxgi_format == render_result.dxgi_format
                && self.width == render_result.width
                && self.height == render_result.height
        }
    }

    static TRACE_STATE: OnceLock<Mutex<TraceState>> = OnceLock::new();
    static PROTECTED_LUT_RESOURCE_SNAPSHOT: OnceLock<Mutex<Option<ProtectedLutResourceSnapshot>>> =
        OnceLock::new();
    static LAST_PRESENT_CONTEXT: OnceLock<Mutex<Option<LastPresentContext>>> = OnceLock::new();

    #[derive(Debug, Clone, Copy)]
    struct LastPresentContext {
        sequence: u64,
        overlay_swap_chain: usize,
        monitor_identity: Option<MonitorIdentity>,
        target_id: Option<u32>,
        hardware_protected: Option<bool>,
        lut_applied: Option<bool>,
        original_present_result: Option<i64>,
        recorded_at: Option<Instant>,
    }

    pub const fn present_sample_interval() -> u64 {
        PRESENT_SAMPLE_INTERVAL
    }

    pub fn record_present_enter(overlay_swap_chain: usize, hardware_protected: bool) {
        PRESENT_ENTERS.fetch_add(1, Ordering::Relaxed);
        if hardware_protected {
            PRESENT_HARDWARE_PROTECTED.fetch_add(1, Ordering::Relaxed);
        }

        let now = Instant::now();
        let mut state = trace_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(last) = state.last_present_at.get(&overlay_swap_chain) {
            let gap = now.duration_since(*last);
            let last_idle_logged = state
                .last_idle_gap_logged_at
                .get(&overlay_swap_chain)
                .copied();
            if gap >= IDLE_GAP_LOG_THRESHOLD
                && last_idle_logged
                    .is_none_or(|logged| now.duration_since(logged) >= IDLE_GAP_LOG_THRESHOLD)
            {
                let gap_ms = gap.as_millis();
                trace_log!(
                    "event=route_present_idle_gap overlay_swap_chain=0x{:x} gap_ms={} hardware_protected={}",
                    overlay_swap_chain,
                    gap_ms,
                    u8::from(hardware_protected)
                );
                state
                    .last_idle_gap_logged_at
                    .insert(overlay_swap_chain, now);
            }
        }
        state.last_present_at.insert(overlay_swap_chain, now);
        state.presents_since_flush = state.presents_since_flush.saturating_add(1);
        if state.presents_since_flush >= FLUSH_EVERY_PRESENTS {
            state.presents_since_flush = 0;
            drop(state);
            flush_summary("present_interval");
        }
    }

    pub fn record_present_lock_miss(overlay_swap_chain: usize) {
        let misses = PRESENT_LOCK_MISSES.fetch_add(1, Ordering::Relaxed) + 1;
        if misses == 1 || misses.is_multiple_of(PRESENT_SAMPLE_INTERVAL) {
            trace_log!(
                "event=route_present_lock_miss overlay_swap_chain=0x{:x} total_misses={}",
                overlay_swap_chain,
                misses
            );
        }
    }

    pub fn record_present_lut_result(hardware_protected: bool, lut_applied: bool) {
        let _ = hardware_protected;
        if lut_applied {
            PRESENT_LUT_APPLIED.fetch_add(1, Ordering::Relaxed);
        } else {
            PRESENT_LUT_NOT_APPLIED.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_last_present_context(
        overlay_swap_chain: usize,
        monitor_identity: Option<MonitorIdentity>,
        hardware_protected: bool,
        lut_applied: Option<bool>,
        original_present_result: Option<i64>,
    ) -> u64 {
        let sequence = LAST_PRESENT_NEXT_SEQUENCE
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let snapshot = LastPresentContext {
            sequence,
            overlay_swap_chain,
            monitor_identity,
            target_id: monitor_identity.map(|identity| identity.target_id),
            hardware_protected: Some(hardware_protected),
            lut_applied,
            original_present_result,
            recorded_at: Some(Instant::now()),
        };
        *last_present_context()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(snapshot);
        sequence
    }

    pub fn record_last_present_original_result(sequence: u64, original_present_result: i64) {
        let mut last_present = last_present_context()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(snapshot) = last_present.as_mut() else {
            return;
        };
        if snapshot.sequence != sequence {
            return;
        }
        snapshot.original_present_result = Some(original_present_result);
        snapshot.recorded_at = Some(Instant::now());
    }

    pub fn record_flip_gate(kind: FlipGateKind, original: bool, result: bool) {
        let index = kind.index();
        FLIP_GATE_CALLS[index].fetch_add(1, Ordering::Relaxed);
        if original && !result {
            let denied = FLIP_GATE_DENIED[index].fetch_add(1, Ordering::Relaxed) + 1;
            if denied == 1 || denied.is_multiple_of(PRESENT_SAMPLE_INTERVAL) {
                trace_log!(
                    "event=route_flip_gate_denied gate={} denied_total={}",
                    kind.label(),
                    denied
                );
            }
        }
        record_protected_flip_gate_sequence_summary(kind, original, result);
    }

    fn record_protected_flip_gate_sequence_summary(
        kind: FlipGateKind,
        original: bool,
        result: bool,
    ) {
        let last_present = last_present_context_snapshot();
        let now = Instant::now();
        let last_present_age_ms = last_present.as_ref().and_then(|last_present| {
            last_present
                .recorded_at
                .map(|recorded_at| now.duration_since(recorded_at).as_millis())
        });
        let last_present_sequence = last_present
            .as_ref()
            .map(|last_present| last_present.sequence);
        let last_target_id = last_present
            .as_ref()
            .and_then(|last_present| last_present.target_id);
        let last_hardware_protected = last_present
            .as_ref()
            .and_then(|last_present| last_present.hardware_protected);
        let sequence_key = last_present_sequence.unwrap_or(0);
        let is_hardware_protected_present = last_hardware_protected == Some(true);
        let should_log = {
            let mut state = trace_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.protected_flip_gate_sequence != sequence_key {
                state.protected_flip_gate_sequence = sequence_key;
                state.protected_flip_gate_stats =
                    [FlipGateSequenceGateStats::new(); FlipGateKind::COUNT];
            }

            let stats = &mut state.protected_flip_gate_stats[kind.index()];
            stats.calls = stats.calls.saturating_add(1);
            if original {
                stats.original_true = stats.original_true.saturating_add(1);
            }
            if result {
                stats.returned_true = stats.returned_true.saturating_add(1);
            }
            if original && !result {
                stats.denied = stats.denied.saturating_add(1);
            }

            let protected_count = if is_hardware_protected_present {
                state.protected_flip_gate_log_count =
                    state.protected_flip_gate_log_count.saturating_add(1);
                state.protected_flip_gate_log_count
            } else {
                0
            };
            is_hardware_protected_present
                && (protected_count <= PROTECTED_FLIP_GATE_SEQUENCE_SUMMARY_BURST
                    || protected_count.is_multiple_of(PROTECTED_FLIP_GATE_SEQUENCE_SUMMARY_EVERY))
        };
        if !should_log {
            return;
        }

        let state = trace_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let overlay =
            state.protected_flip_gate_stats[FlipGateKind::OverlayContextDirectFlip.index()];
        let ensure = state.protected_flip_gate_stats
            [FlipGateKind::DirectFlipInfoEnsureIndependentFlip.index()];
        let df_supported =
            state.protected_flip_gate_stats[FlipGateKind::IsDirectFlipSupportedOnTarget.index()];
        let legacy =
            state.protected_flip_gate_stats[FlipGateKind::LegacySwapChainCheckDirectFlip.index()];
        let advanced =
            state.protected_flip_gate_stats[FlipGateKind::IsAdvancedDirectFlipCompatible.index()];
        trace_log!(
            "event=protected_flip_gate_sequence_summary last_present_sequence={:?} last_target_id={:?} last_hardware_protected={:?} last_present_age_ms={:?} overlay_context_direct_flip_calls={} overlay_context_direct_flip_original_true={} overlay_context_direct_flip_returned_true={} overlay_context_direct_flip_denied={} ensure_independent_flip_calls={} ensure_independent_flip_original_true={} ensure_independent_flip_returned_true={} ensure_independent_flip_denied={} df_supported_calls={} df_supported_original_true={} df_supported_returned_true={} df_supported_denied={} legacy_df_calls={} legacy_df_original_true={} legacy_df_returned_true={} legacy_df_denied={} advanced_df_calls={} advanced_df_original_true={} advanced_df_returned_true={} advanced_df_denied={}",
            last_present_sequence,
            last_target_id,
            last_hardware_protected,
            last_present_age_ms,
            overlay.calls,
            overlay.original_true,
            overlay.returned_true,
            overlay.denied,
            ensure.calls,
            ensure.original_true,
            ensure.returned_true,
            ensure.denied,
            df_supported.calls,
            df_supported.original_true,
            df_supported.returned_true,
            df_supported.denied,
            legacy.calls,
            legacy.original_true,
            legacy.returned_true,
            legacy.denied,
            advanced.calls,
            advanced.original_true,
            advanced.returned_true,
            advanced.denied
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_protected_lut_resource_candidate(
        overlay_swap_chain: usize,
        monitor_identity: Option<MonitorIdentity>,
        hardware_protected: bool,
        back_buffer: usize,
        device: usize,
        context: usize,
        dxgi_format: Option<u32>,
        width: Option<u32>,
        height: Option<u32>,
        lut_applied: bool,
    ) {
        if !hardware_protected {
            return;
        }
        let sequence = {
            let mut snapshot = protected_lut_resource_snapshot_cell()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let sequence = snapshot
                .as_ref()
                .map(|snapshot| snapshot.sequence)
                .unwrap_or(0)
                .saturating_add(1);
            *snapshot = Some(ProtectedLutResourceSnapshot {
                sequence,
                overlay_swap_chain,
                monitor_identity,
                target_id: monitor_identity.map(|identity| identity.target_id),
                back_buffer,
                device,
                context,
                dxgi_format,
                width,
                height,
                lut_applied,
                recorded_at: Some(Instant::now()),
            });
            sequence
        };

        let log_key = ProtectedLutResourceCandidateLogKey {
            target_id: monitor_identity.map(|identity| identity.target_id),
            dxgi_format,
            width,
            height,
            lut_applied,
        };
        let should_log = {
            let mut state = trace_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.protected_lut_resource_candidate_count = state
                .protected_lut_resource_candidate_count
                .saturating_add(1);
            let count = state.protected_lut_resource_candidate_count;
            let changed = state.protected_lut_resource_candidate_last_key != Some(log_key);
            state.protected_lut_resource_candidate_last_key = Some(log_key);

            count <= PROTECTED_LUT_RESOURCE_CANDIDATE_BURST
                || count.is_multiple_of(PROTECTED_LUT_RESOURCE_CANDIDATE_EVERY)
                || changed
        };
        if !should_log {
            return;
        }

        trace_log!(
            "event=protected_display_resource_candidate source=renderer_lut_candidate sequence={} overlay_swap_chain=0x{:x} monitor_identity={} target_id={:?} hardware_protected={} back_buffer=0x{:x} device=0x{:x} context=0x{:x} dxgi_format={:?} width={:?} height={:?} lut_drawn={} lut_applied={}",
            sequence,
            overlay_swap_chain,
            crate::debug_log::quoted(format_monitor_identity(monitor_identity)),
            monitor_identity.map(|identity| identity.target_id),
            u8::from(hardware_protected),
            back_buffer,
            device,
            context,
            dxgi_format,
            width,
            height,
            lut_applied,
            lut_applied
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_protected_present_resource_result_summary(
        overlay_swap_chain: usize,
        monitor_identity: Option<MonitorIdentity>,
        hardware_protected: bool,
        original_result: i64,
        render_result: crate::d3d11_renderer::PresentLutOutcome,
        dirty_rect_count: usize,
        first_dirty_rect: Option<crate::DirtyRect>,
        present_rect_override_enabled: bool,
        present_rect_override: Option<crate::DirtyRect>,
    ) {
        if !hardware_protected {
            return;
        }

        let snapshot = render_result
            .width
            .zip(render_result.height)
            .and_then(|_| protected_lut_resource_snapshot())
            .filter(|snapshot| {
                snapshot.matches_present(overlay_swap_chain, monitor_identity, render_result)
            });

        let should_log = {
            let mut state = trace_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.protected_present_resource_result_summary_count = state
                .protected_present_resource_result_summary_count
                .saturating_add(1);
            let count = state.protected_present_resource_result_summary_count;
            let result_changed = state.protected_present_resource_result_last_original_result
                != Some(original_result);
            state.protected_present_resource_result_last_original_result = Some(original_result);

            count <= PROTECTED_PRESENT_RESOURCE_RESULT_SUMMARY_BURST
                || count.is_multiple_of(PROTECTED_PRESENT_RESOURCE_RESULT_SUMMARY_EVERY)
                || result_changed
        };
        if !should_log {
            return;
        }

        let last_present = last_present_context_snapshot();
        let now = Instant::now();
        let last_present_age_ms = last_present.as_ref().and_then(|last_present| {
            last_present
                .recorded_at
                .map(|recorded_at| now.duration_since(recorded_at).as_millis())
        });
        let lut_age_ms = snapshot
            .and_then(|snapshot| snapshot.recorded_at)
            .map(|recorded_at| now.duration_since(recorded_at).as_millis());
        let snapshot_back_buffer = snapshot.map(|snapshot| format!("0x{:x}", snapshot.back_buffer));
        let snapshot_device = snapshot.map(|snapshot| format!("0x{:x}", snapshot.device));
        let snapshot_context = snapshot.map(|snapshot| format!("0x{:x}", snapshot.context));

        trace_log!(
            "event=protected_present_resource_result_summary overlay_swap_chain=0x{:x} monitor_identity={} target_id={:?} hardware_protected={} original_result={} render_lut_applied={} render_dxgi_format={:?} render_width={:?} render_height={:?} render_lut_index={:?} back_buffer={:?} device={:?} context={:?} dirty_rect_count={} first_dirty_rect={:?} present_rect_override_enabled={} present_rect_override={:?} last_present_sequence={:?} last_present_age_ms={:?} lut_resource_sequence={:?} lut_resource_age_ms={:?} lut_resource_overlay_swap_chain={:?} lut_resource_monitor_identity={} lut_resource_target_id={:?} lut_resource_lut_applied={:?}",
            overlay_swap_chain,
            crate::debug_log::quoted(format_monitor_identity(monitor_identity)),
            monitor_identity.map(|identity| identity.target_id),
            u8::from(hardware_protected),
            format_original_result(original_result),
            render_result.lut_applied(),
            render_result.dxgi_format,
            render_result.width,
            render_result.height,
            render_result.lut_index,
            snapshot_back_buffer,
            snapshot_device,
            snapshot_context,
            dirty_rect_count,
            first_dirty_rect,
            present_rect_override_enabled,
            present_rect_override,
            last_present
                .as_ref()
                .map(|last_present| last_present.sequence),
            last_present_age_ms,
            snapshot.map(|snapshot| snapshot.sequence),
            lut_age_ms,
            snapshot.map(|snapshot| format!("0x{:x}", snapshot.overlay_swap_chain)),
            crate::debug_log::quoted(format_monitor_identity(
                snapshot.and_then(|snapshot| snapshot.monitor_identity)
            )),
            snapshot.and_then(|snapshot| snapshot.target_id),
            snapshot.map(|snapshot| snapshot.lut_applied)
        );
    }

    pub fn flush_summary(reason: &str) {
        trace_log!(
            "event=route_trace_summary reason={} present_enters={} present_hw_protected={} present_lock_misses={} present_lut_applied={} present_lut_not_applied={} overlay_df_calls={} overlay_df_denied={} ensure_if_calls={} ensure_if_denied={} df_supported_calls={} df_supported_denied={} legacy_df_calls={} legacy_df_denied={} advanced_df_calls={} advanced_df_denied={}",
            crate::debug_log::quoted(reason),
            PRESENT_ENTERS.load(Ordering::Relaxed),
            PRESENT_HARDWARE_PROTECTED.load(Ordering::Relaxed),
            PRESENT_LOCK_MISSES.load(Ordering::Relaxed),
            PRESENT_LUT_APPLIED.load(Ordering::Relaxed),
            PRESENT_LUT_NOT_APPLIED.load(Ordering::Relaxed),
            FLIP_GATE_CALLS[FlipGateKind::OverlayContextDirectFlip.index()].load(Ordering::Relaxed),
            FLIP_GATE_DENIED[FlipGateKind::OverlayContextDirectFlip.index()]
                .load(Ordering::Relaxed),
            FLIP_GATE_CALLS[FlipGateKind::DirectFlipInfoEnsureIndependentFlip.index()]
                .load(Ordering::Relaxed),
            FLIP_GATE_DENIED[FlipGateKind::DirectFlipInfoEnsureIndependentFlip.index()]
                .load(Ordering::Relaxed),
            FLIP_GATE_CALLS[FlipGateKind::IsDirectFlipSupportedOnTarget.index()]
                .load(Ordering::Relaxed),
            FLIP_GATE_DENIED[FlipGateKind::IsDirectFlipSupportedOnTarget.index()]
                .load(Ordering::Relaxed),
            FLIP_GATE_CALLS[FlipGateKind::LegacySwapChainCheckDirectFlip.index()]
                .load(Ordering::Relaxed),
            FLIP_GATE_DENIED[FlipGateKind::LegacySwapChainCheckDirectFlip.index()]
                .load(Ordering::Relaxed),
            FLIP_GATE_CALLS[FlipGateKind::IsAdvancedDirectFlipCompatible.index()]
                .load(Ordering::Relaxed),
            FLIP_GATE_DENIED[FlipGateKind::IsAdvancedDirectFlipCompatible.index()]
                .load(Ordering::Relaxed),
        );
    }

    fn trace_state() -> &'static Mutex<TraceState> {
        TRACE_STATE.get_or_init(|| {
            Mutex::new(TraceState {
                last_present_at: BTreeMap::new(),
                last_idle_gap_logged_at: BTreeMap::new(),
                presents_since_flush: 0,
                protected_flip_gate_sequence: 0,
                protected_flip_gate_log_count: 0,
                protected_flip_gate_stats: [FlipGateSequenceGateStats::new(); FlipGateKind::COUNT],
                protected_present_resource_result_summary_count: 0,
                protected_present_resource_result_last_original_result: None,
                protected_lut_resource_candidate_count: 0,
                protected_lut_resource_candidate_last_key: None,
            })
        })
    }

    fn protected_lut_resource_snapshot_cell() -> &'static Mutex<Option<ProtectedLutResourceSnapshot>>
    {
        PROTECTED_LUT_RESOURCE_SNAPSHOT.get_or_init(|| Mutex::new(None))
    }

    fn last_present_context() -> &'static Mutex<Option<LastPresentContext>> {
        LAST_PRESENT_CONTEXT.get_or_init(|| Mutex::new(None))
    }

    fn last_present_context_snapshot() -> Option<LastPresentContext> {
        *last_present_context()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn protected_lut_resource_snapshot() -> Option<ProtectedLutResourceSnapshot> {
        *protected_lut_resource_snapshot_cell()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn format_original_result(value: i64) -> String {
        if value == 0 {
            "0".to_owned()
        } else {
            format!("0x{:08X}", value as u32)
        }
    }

    fn format_monitor_identity(identity: Option<MonitorIdentity>) -> String {
        identity
            .map(|identity| format!("{}:{}", identity.adapter_luid, identity.target_id))
            .unwrap_or_else(|| "none".to_owned())
    }

    fn monitor_identity_matches(
        left: Option<MonitorIdentity>,
        right: Option<MonitorIdentity>,
    ) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => {
                left.adapter_luid.low_part == right.adapter_luid.low_part
                    && left.adapter_luid.high_part == right.adapter_luid.high_part
                    && left.target_id == right.target_id
            }
            (None, None) => true,
            _ => false,
        }
    }
}

#[cfg(not(debug_assertions))]
mod imp {
    use super::FlipGateKind;

    pub const fn present_sample_interval() -> u64 {
        600
    }

    pub fn record_present_enter(_: usize, _: bool) {}

    pub fn record_present_lock_miss(_: usize) {}

    pub fn record_present_lut_result(_: bool, _: bool) {}

    pub fn record_last_present_context(
        _: usize,
        _: Option<dwm_lut_payload::MonitorIdentity>,
        _: bool,
        _: Option<bool>,
        _: Option<i64>,
    ) -> u64 {
        0
    }

    pub fn record_last_present_original_result(_: u64, _: i64) {}

    pub fn record_flip_gate(_: FlipGateKind, _: bool, _: bool) {}

    #[allow(clippy::too_many_arguments)]
    pub fn record_protected_lut_resource_candidate(
        _: usize,
        _: Option<dwm_lut_payload::MonitorIdentity>,
        _: bool,
        _: usize,
        _: usize,
        _: usize,
        _: Option<u32>,
        _: Option<u32>,
        _: Option<u32>,
        _: bool,
    ) {
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_protected_present_resource_result_summary(
        _: usize,
        _: Option<dwm_lut_payload::MonitorIdentity>,
        _: bool,
        _: i64,
        _: crate::d3d11_renderer::PresentLutOutcome,
        _: usize,
        _: Option<crate::DirtyRect>,
        _: bool,
        _: Option<crate::DirtyRect>,
    ) {
    }

    pub fn flush_summary(_: &str) {}
}
