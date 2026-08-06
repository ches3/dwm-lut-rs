use dwm_lut_profile::HookTarget;

use crate::dwmcore;
use crate::lifecycle;
use crate::state;

#[cfg(debug_assertions)]
use crate::log::SharedLimiter;

#[cfg(debug_assertions)]
static FLIP_GATE_DENIED_LIMITER: SharedLimiter<HookTarget> = SharedLimiter::new(600);

fn record_flip_gate_denied(target: HookTarget) {
    #[cfg(debug_assertions)]
    {
        let decision = FLIP_GATE_DENIED_LIMITER.sample(target);
        if decision.should_log {
            crate::log::flip_gate_denied(target.label(), decision.count);
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = target;
    }
}

pub(crate) fn should_block(target: HookTarget, overlay_context: usize) -> bool {
    if !lifecycle::is_runtime_active() {
        return false;
    }
    let Some(profile) = state::hook_profile() else {
        return false;
    };
    let Some(assignments) = state::assignments() else {
        return false;
    };
    if assignments.is_empty() {
        return false;
    }
    let blocked =
        dwmcore::resolve_overlay_swap_chain(overlay_context, profile.context_to_swap_chain_path)
            .and_then(|swap_chain| {
                dwmcore::read_monitor_identity(swap_chain, profile.monitor_identity_offsets)
            })
            .is_some_and(|identity| {
                assignments
                    .iter()
                    .any(|assignment| assignment.target.identity == identity)
            });
    if blocked {
        record_flip_gate_denied(target);
    }
    blocked
}

#[cfg(test)]
mod tests {
    use super::should_block;
    use crate::dwmcore::test_support::FakeOverlayContext;
    use crate::present::test_support::{
        initialize_test_state, test_monitor_identity, test_profile,
    };
    use crate::state::HOOK_GLOBAL_TEST_LOCK as CONTROLLED_TEST_LOCK;
    use dwm_lut_payload::{AdapterLuid, MonitorIdentity};
    use dwm_lut_profile::HookTarget;

    #[test]
    fn should_block_assigned_monitor_only() {
        let _guard = CONTROLLED_TEST_LOCK.lock().expect("test mutex should lock");
        initialize_test_state();
        let profile = test_profile();
        let assigned = FakeOverlayContext::with_identity(&profile, test_monitor_identity());
        let other = FakeOverlayContext::with_identity(
            &profile,
            MonitorIdentity {
                adapter_luid: AdapterLuid {
                    high_part: 0,
                    low_part: 0x9999,
                },
                target_id: 1,
            },
        );

        assert!(should_block(
            HookTarget::IsCandidateDirectFlipCompatible,
            assigned.address(),
        ));
        assert!(!should_block(
            HookTarget::IsCandidateDirectFlipCompatible,
            other.address(),
        ));
        assert!(!should_block(
            HookTarget::IsCandidateDirectFlipCompatible,
            0,
        ));
    }
}
