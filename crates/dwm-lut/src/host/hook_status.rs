use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::inject::HookStatusSnapshot;

const QUERY_FAILURES_BEFORE_UNKNOWN: u8 = 3;
const TRANSITIONING_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum HookRuntimeStatus {
    #[default]
    Checking,
    Active {
        profile_name: String,
    },
    Inactive,
    Unknown,
}

impl HookRuntimeStatus {
    pub(crate) const fn can_disable(&self) -> bool {
        !matches!(self, Self::Inactive)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HookStatusUpdate {
    pub(super) status_changed: bool,
    pub(super) loss_revision: Option<u64>,
}

impl HookStatusUpdate {
    const fn unchanged() -> Self {
        Self {
            status_changed: false,
            loss_revision: None,
        }
    }

    pub(super) const fn should_notify_ui(self) -> bool {
        self.status_changed || self.loss_revision.is_some()
    }
}

#[derive(Debug, Default)]
struct HookRuntimeState {
    status: HookRuntimeStatus,
    loss_armed: bool,
    consecutive_query_failures: u8,
    transitioning_since: Option<Instant>,
    mutation_generation: u64,
    revision: u64,
}

#[derive(Debug, Clone, Default)]
pub(super) struct HookStatusStore {
    inner: Arc<Mutex<HookRuntimeState>>,
}

impl HookStatusStore {
    pub(super) fn status(&self) -> HookRuntimeStatus {
        self.lock().status.clone()
    }

    pub(super) fn begin_mutation(&self) {
        let mut state = self.lock();
        state.mutation_generation = state.mutation_generation.wrapping_add(1);
        state.revision = state.revision.wrapping_add(1);
        state.loss_armed = false;
        state.reset_observation_tracking();
    }

    pub(super) fn probe_generation(&self) -> u64 {
        self.lock().mutation_generation
    }

    pub(super) fn record_apply(&self, profile_name: String) {
        let mut state = self.lock();
        state.status = HookRuntimeStatus::Active { profile_name };
        state.loss_armed = true;
        state.reset_observation_tracking();
        state.revision = state.revision.wrapping_add(1);
    }

    pub(super) fn record_disable(&self) {
        let mut state = self.lock();
        state.status = HookRuntimeStatus::Inactive;
        state.loss_armed = false;
        state.reset_observation_tracking();
        state.revision = state.revision.wrapping_add(1);
    }

    pub(super) fn observe_if_generation(
        &self,
        generation: u64,
        snapshot: HookStatusSnapshot,
    ) -> Option<HookStatusUpdate> {
        self.observe_at_if_generation(generation, snapshot, Instant::now())
    }

    pub(super) fn record_query_failure_if_generation(
        &self,
        generation: u64,
    ) -> Option<HookStatusUpdate> {
        let mut state = self.lock();
        if state.mutation_generation != generation {
            return None;
        }
        state.transitioning_since = None;
        state.consecutive_query_failures = state.consecutive_query_failures.saturating_add(1);
        let status_changed = if state.consecutive_query_failures >= QUERY_FAILURES_BEFORE_UNKNOWN {
            state.set_status(HookRuntimeStatus::Unknown)
        } else {
            false
        };
        Some(HookStatusUpdate {
            status_changed,
            loss_revision: None,
        })
    }

    pub(super) fn should_report_loss(&self, revision: u64) -> bool {
        self.lock().revision == revision
    }

    fn observe_at_if_generation(
        &self,
        generation: u64,
        snapshot: HookStatusSnapshot,
        observed_at: Instant,
    ) -> Option<HookStatusUpdate> {
        let mut state = self.lock();
        if state.mutation_generation != generation {
            return None;
        }

        state.consecutive_query_failures = 0;
        let mut update = HookStatusUpdate::unchanged();
        match snapshot {
            HookStatusSnapshot::Active { profile_name } => {
                state.transitioning_since = None;
                state.loss_armed = true;
                update.status_changed =
                    state.set_status(HookRuntimeStatus::Active { profile_name });
            }
            HookStatusSnapshot::Inactive => {
                state.transitioning_since = None;
                let unexpected_loss = state.loss_armed;
                state.loss_armed = false;
                update.status_changed = state.set_status(HookRuntimeStatus::Inactive);
                if unexpected_loss {
                    update.loss_revision = Some(state.revision);
                }
            }
            HookStatusSnapshot::Transitioning => {
                let started_at = state.transitioning_since.get_or_insert(observed_at);
                if observed_at.saturating_duration_since(*started_at) >= TRANSITIONING_TIMEOUT {
                    update.status_changed = state.set_status(HookRuntimeStatus::Unknown);
                }
            }
        }
        Some(update)
    }

    fn lock(&self) -> MutexGuard<'_, HookRuntimeState> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl HookRuntimeState {
    fn set_status(&mut self, status: HookRuntimeStatus) -> bool {
        if self.status == status {
            return false;
        }
        self.status = status;
        self.revision = self.revision.wrapping_add(1);
        true
    }

    fn reset_observation_tracking(&mut self) {
        self.consecutive_query_failures = 0;
        self.transitioning_since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generation(status: &HookStatusStore) -> u64 {
        status.probe_generation()
    }

    #[test]
    fn disable_is_available_unless_inactive_is_confirmed() {
        assert!(HookRuntimeStatus::Checking.can_disable());
        assert!(
            HookRuntimeStatus::Active {
                profile_name: "gaming".to_string()
            }
            .can_disable()
        );
        assert!(!HookRuntimeStatus::Inactive.can_disable());
        assert!(HookRuntimeStatus::Unknown.can_disable());
    }

    #[test]
    fn query_failures_only_become_unknown_after_three_consecutive_failures() {
        let status = HookStatusStore::default();
        status.record_apply("editing".to_string());
        let generation = generation(&status);

        assert!(
            !status
                .record_query_failure_if_generation(generation)
                .unwrap()
                .status_changed
        );
        assert!(
            !status
                .record_query_failure_if_generation(generation)
                .unwrap()
                .status_changed
        );
        assert!(
            status
                .record_query_failure_if_generation(generation)
                .unwrap()
                .status_changed
        );
        assert_eq!(status.status(), HookRuntimeStatus::Unknown);
    }

    #[test]
    fn valid_inactive_observation_updates_public_status() {
        let status = HookStatusStore::default();
        let generation = generation(&status);
        status
            .observe_if_generation(generation, HookStatusSnapshot::Inactive)
            .unwrap();

        assert_eq!(status.status(), HookRuntimeStatus::Inactive);
    }

    #[test]
    fn stable_observation_resets_failure_count_and_recovers_unknown() {
        let status = HookStatusStore::default();
        status.record_apply("editing".to_string());
        let generation = generation(&status);
        for _ in 0..3 {
            status
                .record_query_failure_if_generation(generation)
                .unwrap();
        }
        assert_eq!(status.status(), HookRuntimeStatus::Unknown);

        status
            .observe_if_generation(
                generation,
                HookStatusSnapshot::Active {
                    profile_name: "gaming".to_string(),
                },
            )
            .unwrap();
        assert_eq!(
            status.status(),
            HookRuntimeStatus::Active {
                profile_name: "gaming".to_string()
            }
        );

        status
            .record_query_failure_if_generation(generation)
            .unwrap();
        status
            .record_query_failure_if_generation(generation)
            .unwrap();
        assert!(matches!(status.status(), HookRuntimeStatus::Active { .. }));
    }

    #[test]
    fn transitioning_preserves_status_until_timeout() {
        let status = HookStatusStore::default();
        let started_at = Instant::now();
        status.record_apply("editing".to_string());
        let generation = generation(&status);

        status
            .observe_at_if_generation(generation, HookStatusSnapshot::Transitioning, started_at)
            .unwrap();
        assert_eq!(
            status.status(),
            HookRuntimeStatus::Active {
                profile_name: "editing".to_string()
            }
        );
        status
            .observe_at_if_generation(
                generation,
                HookStatusSnapshot::Transitioning,
                started_at + TRANSITIONING_TIMEOUT - Duration::from_nanos(1),
            )
            .unwrap();
        assert_eq!(
            status.status(),
            HookRuntimeStatus::Active {
                profile_name: "editing".to_string()
            }
        );
        let update = status
            .observe_at_if_generation(
                generation,
                HookStatusSnapshot::Transitioning,
                started_at + TRANSITIONING_TIMEOUT,
            )
            .unwrap();
        assert!(update.status_changed);
        assert_eq!(status.status(), HookRuntimeStatus::Unknown);
    }

    #[test]
    fn stale_success_and_failure_do_not_change_status() {
        let status = HookStatusStore::default();
        status.record_apply("editing".to_string());
        let stale_generation = generation(&status);
        status.begin_mutation();

        assert!(
            status
                .observe_if_generation(stale_generation, HookStatusSnapshot::Inactive)
                .is_none()
        );
        assert!(
            status
                .record_query_failure_if_generation(stale_generation)
                .is_none()
        );
        assert_eq!(
            status.status(),
            HookRuntimeStatus::Active {
                profile_name: "editing".to_string()
            }
        );

        let current_generation = generation(&status);
        status
            .record_query_failure_if_generation(current_generation)
            .unwrap();
        status
            .record_query_failure_if_generation(current_generation)
            .unwrap();
        assert!(matches!(status.status(), HookRuntimeStatus::Active { .. }));
    }

    #[test]
    fn mutation_invalidates_pending_loss_notification() {
        let status = HookStatusStore::default();
        status.record_apply("editing".to_string());
        let update = status
            .observe_if_generation(generation(&status), HookStatusSnapshot::Inactive)
            .unwrap();
        let revision = update.loss_revision.unwrap();
        assert!(status.should_report_loss(revision));

        status.begin_mutation();
        assert!(!status.should_report_loss(revision));
    }

    #[test]
    fn mutation_suppresses_loss_until_active_is_observed_again() {
        let status = HookStatusStore::default();
        status.record_apply("editing".to_string());
        status.begin_mutation();
        let generation = generation(&status);

        let update = status
            .observe_if_generation(generation, HookStatusSnapshot::Inactive)
            .unwrap();
        assert!(update.loss_revision.is_none());

        status
            .observe_if_generation(
                generation,
                HookStatusSnapshot::Active {
                    profile_name: "editing".to_string(),
                },
            )
            .unwrap();
        let update = status
            .observe_if_generation(generation, HookStatusSnapshot::Inactive)
            .unwrap();
        assert!(update.loss_revision.is_some());
    }

    #[test]
    fn mutation_resets_query_failure_tracking() {
        let status = HookStatusStore::default();
        status.record_apply("editing".to_string());
        let initial_generation = generation(&status);
        status
            .record_query_failure_if_generation(initial_generation)
            .unwrap();
        status
            .record_query_failure_if_generation(initial_generation)
            .unwrap();

        status.begin_mutation();
        let current_generation = generation(&status);
        status
            .record_query_failure_if_generation(current_generation)
            .unwrap();
        status
            .record_query_failure_if_generation(current_generation)
            .unwrap();

        assert!(matches!(status.status(), HookRuntimeStatus::Active { .. }));
    }
}
