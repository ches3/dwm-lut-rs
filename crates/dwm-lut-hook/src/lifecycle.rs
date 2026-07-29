use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};

use dwm_lut_payload::{DwmLutStatusSnapshot, HookStatus, MAX_PROFILE_NAME_BYTES};

use crate::DWM_LUT_STATUS;
use crate::state::active_profile_name;

static LIFECYCLE_TRANSITION_LOCK: Mutex<()> = Mutex::new(());
static LIFECYCLE: AtomicU8 = AtomicU8::new(LIFECYCLE_IDLE);

const LIFECYCLE_IDLE: u8 = 0;
const LIFECYCLE_INITIALIZING: u8 = 1;
const LIFECYCLE_RUNNING: u8 = 2;
const LIFECYCLE_REPLACING_ASSIGNMENTS: u8 = 3;
const LIFECYCLE_SHUTTING_DOWN: u8 = 4;
const LIFECYCLE_SHUT_DOWN: u8 = 5;

static STATUS_PUBLISH_LOCK: Mutex<()> = Mutex::new(());

#[repr(transparent)]
pub(crate) struct ExportedStatusSnapshot {
    snapshot: UnsafeCell<DwmLutStatusSnapshot>,
}

// SAFETY: The inner value is only mutated by publish, which serializes writers.
// The sequence field is accessed atomically, full in-process reads take the
// same lock, and no reference to the inner value escapes.
unsafe impl Sync for ExportedStatusSnapshot {}

impl ExportedStatusSnapshot {
    pub(crate) const fn inactive() -> Self {
        Self {
            snapshot: UnsafeCell::new(DwmLutStatusSnapshot::inactive()),
        }
    }

    fn sequence(&self) -> &AtomicU32 {
        // SAFETY: sequence is the first u32 field of this live snapshot.
        // AtomicU32 has compatible size and alignment on the supported Windows
        // targets, and the field is never accessed non-atomically after initialization.
        unsafe {
            let sequence = std::ptr::addr_of_mut!((*self.snapshot.get()).sequence);
            &*sequence.cast::<AtomicU32>()
        }
    }

    fn publish(&self, snapshot: &DwmLutStatusSnapshot) {
        let _writer = STATUS_PUBLISH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.sequence().fetch_add(1, Ordering::AcqRel);
        let source = std::ptr::addr_of!(snapshot.abi_version);
        let content_size = std::mem::size_of::<DwmLutStatusSnapshot>() - std::mem::size_of::<u32>();
        // SAFETY: The writer lock excludes concurrent access, and the source
        // and destination are disjoint regions valid for content_size bytes.
        unsafe {
            let destination = std::ptr::addr_of_mut!((*self.snapshot.get()).abi_version);
            std::ptr::copy_nonoverlapping(
                source.cast::<u8>(),
                destination.cast::<u8>(),
                content_size,
            );
        }
        self.sequence().fetch_add(1, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn load_for_test(&self) -> DwmLutStatusSnapshot {
        let _reader = STATUS_PUBLISH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // SAFETY: The writer lock prevents concurrent mutation, and the
        // snapshot was fully initialized before it became shared.
        unsafe { *self.snapshot.get() }
    }
}

#[derive(Debug)]
pub(crate) struct InitializationTransition {
    rollback_lifecycle: u8,
    completed: bool,
}

impl InitializationTransition {
    pub(crate) fn commit_active(mut self, profile_name: &str) {
        self.complete(LIFECYCLE_RUNNING, HookStatus::Active, Some(profile_name));
    }

    pub(crate) fn finish_shut_down(mut self) {
        self.complete(LIFECYCLE_SHUT_DOWN, HookStatus::Inactive, None);
    }

    fn complete(&mut self, lifecycle: u8, status: HookStatus, profile_name: Option<&str>) {
        set_lifecycle_and_status(lifecycle, status, profile_name);
        self.completed = true;
    }
}

impl Drop for InitializationTransition {
    fn drop(&mut self) {
        if !self.completed {
            set_lifecycle_and_status(self.rollback_lifecycle, HookStatus::Inactive, None);
        }
    }
}

#[derive(Debug)]
pub(crate) struct ReplaceAssignmentsTransition {
    completed: bool,
}

impl ReplaceAssignmentsTransition {
    pub(crate) fn commit_active(mut self, profile_name: &str) {
        self.complete(LIFECYCLE_RUNNING, HookStatus::Active, Some(profile_name));
    }

    fn restore_current_status(&mut self) {
        let profile_name = active_profile_name();
        match profile_name {
            Some(profile_name) => {
                self.complete(LIFECYCLE_RUNNING, HookStatus::Active, Some(&profile_name));
            }
            None => self.complete(LIFECYCLE_IDLE, HookStatus::Inactive, None),
        }
    }

    fn complete(&mut self, lifecycle: u8, status: HookStatus, profile_name: Option<&str>) {
        set_lifecycle_and_status(lifecycle, status, profile_name);
        self.completed = true;
    }
}

impl Drop for ReplaceAssignmentsTransition {
    fn drop(&mut self) {
        if !self.completed {
            self.restore_current_status();
        }
    }
}

#[derive(Debug)]
pub(crate) struct ShutdownTransition {
    completed: bool,
}

impl ShutdownTransition {
    pub(crate) fn finish_shut_down(mut self) {
        self.complete(LIFECYCLE_SHUT_DOWN, HookStatus::Inactive, None);
    }

    pub(crate) fn finish_idle(mut self) {
        self.complete(LIFECYCLE_IDLE, HookStatus::Inactive, None);
    }

    fn restore_current_status(&mut self) {
        let profile_name = active_profile_name();
        match profile_name {
            Some(profile_name) => {
                self.complete(LIFECYCLE_RUNNING, HookStatus::Active, Some(&profile_name));
            }
            None => self.complete(LIFECYCLE_IDLE, HookStatus::Inactive, None),
        }
    }

    fn complete(&mut self, lifecycle: u8, status: HookStatus, profile_name: Option<&str>) {
        set_lifecycle_and_status(lifecycle, status, profile_name);
        self.completed = true;
    }
}

impl Drop for ShutdownTransition {
    fn drop(&mut self) {
        if !self.completed {
            self.restore_current_status();
        }
    }
}

#[derive(Debug)]
pub(crate) enum ShutdownStart {
    Started(ShutdownTransition),
    NotInitialized,
    InitializationInProgress,
    AssignmentReplacementInProgress,
    ShutdownInProgress,
    AlreadyShutDown,
}

#[derive(Debug)]
pub(crate) enum ReplaceAssignmentsStart {
    Started(ReplaceAssignmentsTransition),
    NotInitialized,
    AlreadyInProgress,
}

fn publish_status(status: HookStatus, profile_name: Option<&str>) {
    let mut snapshot = DwmLutStatusSnapshot::inactive();
    snapshot.hook_status = status as u32;
    if let Some(profile_name) = profile_name {
        let bytes = profile_name.as_bytes();
        debug_assert!(!bytes.is_empty());
        debug_assert!(bytes.len() <= MAX_PROFILE_NAME_BYTES);
        if bytes.is_empty() || bytes.len() > MAX_PROFILE_NAME_BYTES {
            snapshot.hook_status = HookStatus::Inactive as u32;
        } else {
            snapshot.profile_name_len = bytes.len() as u32;
            snapshot.profile_name[..bytes.len()].copy_from_slice(bytes);
        }
    }
    DWM_LUT_STATUS.publish(&snapshot);
}

fn lifecycle_transition_lock() -> MutexGuard<'static, ()> {
    LIFECYCLE_TRANSITION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn set_lifecycle_and_status(lifecycle: u8, status: HookStatus, profile_name: Option<&str>) {
    let _transition = lifecycle_transition_lock();
    LIFECYCLE.store(lifecycle, Ordering::Release);
    publish_status(status, profile_name);
}

fn transition_lifecycle(
    from: u8,
    to: u8,
    status: HookStatus,
    profile_name: Option<&str>,
) -> Result<(), u8> {
    let _transition = lifecycle_transition_lock();
    let current = LIFECYCLE.load(Ordering::Acquire);
    if current != from {
        return Err(current);
    }
    LIFECYCLE.store(to, Ordering::Release);
    publish_status(status, profile_name);
    Ok(())
}

pub(crate) fn begin_initialization() -> Option<InitializationTransition> {
    let _transition = lifecycle_transition_lock();
    let rollback_lifecycle = LIFECYCLE.load(Ordering::Acquire);
    if !matches!(rollback_lifecycle, LIFECYCLE_IDLE | LIFECYCLE_SHUT_DOWN) {
        return None;
    }
    LIFECYCLE.store(LIFECYCLE_INITIALIZING, Ordering::Release);
    publish_status(HookStatus::Transitioning, None);
    Some(InitializationTransition {
        rollback_lifecycle,
        completed: false,
    })
}

pub(crate) fn is_runtime_active() -> bool {
    matches!(
        LIFECYCLE.load(Ordering::Acquire),
        LIFECYCLE_RUNNING | LIFECYCLE_REPLACING_ASSIGNMENTS
    )
}

pub(crate) fn begin_replace_assignments() -> ReplaceAssignmentsStart {
    match transition_lifecycle(
        LIFECYCLE_RUNNING,
        LIFECYCLE_REPLACING_ASSIGNMENTS,
        HookStatus::Transitioning,
        None,
    ) {
        Ok(()) => {
            ReplaceAssignmentsStart::Started(ReplaceAssignmentsTransition { completed: false })
        }
        Err(LIFECYCLE_IDLE | LIFECYCLE_SHUT_DOWN) => ReplaceAssignmentsStart::NotInitialized,
        Err(LIFECYCLE_INITIALIZING | LIFECYCLE_REPLACING_ASSIGNMENTS | LIFECYCLE_SHUTTING_DOWN) => {
            ReplaceAssignmentsStart::AlreadyInProgress
        }
        Err(_) => ReplaceAssignmentsStart::NotInitialized,
    }
}

pub(crate) fn begin_shutdown() -> ShutdownStart {
    match transition_lifecycle(
        LIFECYCLE_RUNNING,
        LIFECYCLE_SHUTTING_DOWN,
        HookStatus::Transitioning,
        None,
    ) {
        Ok(()) => ShutdownStart::Started(ShutdownTransition { completed: false }),
        Err(LIFECYCLE_IDLE) => ShutdownStart::NotInitialized,
        Err(LIFECYCLE_INITIALIZING) => ShutdownStart::InitializationInProgress,
        Err(LIFECYCLE_REPLACING_ASSIGNMENTS) => ShutdownStart::AssignmentReplacementInProgress,
        Err(LIFECYCLE_SHUTTING_DOWN) => ShutdownStart::ShutdownInProgress,
        Err(LIFECYCLE_SHUT_DOWN) => ShutdownStart::AlreadyShutDown,
        Err(_) => ShutdownStart::NotInitialized,
    }
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    set_lifecycle_and_status(LIFECYCLE_IDLE, HookStatus::Inactive, None);
}

#[cfg(test)]
mod tests {
    use dwm_lut_payload::HookStatus;

    use super::*;
    use crate::state::{HOOK_GLOBAL_TEST_LOCK, reset_state_for_tests};

    #[test]
    fn exported_status_publishers_use_even_sequence_and_fixed_profile_storage() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        reset_state_for_tests();
        let before = DWM_LUT_STATUS.load_for_test().sequence;

        publish_status(HookStatus::Active, Some("gaming"));
        let active = DWM_LUT_STATUS.load_for_test();
        assert_eq!(active.sequence, before.wrapping_add(2));
        assert_eq!(active.sequence % 2, 0);
        assert_eq!(active.hook_status, HookStatus::Active as u32);
        assert_eq!(active.profile_name_len, 6);
        assert_eq!(&active.profile_name[..6], b"gaming");

        publish_status(HookStatus::Inactive, None);
        let inactive = DWM_LUT_STATUS.load_for_test();
        assert_eq!(inactive.sequence, active.sequence.wrapping_add(2));
        assert_eq!(inactive.hook_status, HookStatus::Inactive as u32);
        assert_eq!(inactive.profile_name_len, 0);

        let initialization =
            begin_initialization().expect("initialization transition should start");
        let transitioning = DWM_LUT_STATUS.load_for_test();
        assert_eq!(transitioning.sequence, inactive.sequence.wrapping_add(2));
        assert_eq!(transitioning.hook_status, HookStatus::Transitioning as u32);
        assert_eq!(transitioning.profile_name_len, 0);

        drop(initialization);
        let rolled_back = DWM_LUT_STATUS.load_for_test();
        assert_eq!(rolled_back.sequence, transitioning.sequence.wrapping_add(2));
        assert_eq!(rolled_back.hook_status, HookStatus::Inactive as u32);
        assert_eq!(rolled_back.profile_name_len, 0);

        reset_state_for_tests();
    }

    #[test]
    fn initialization_transition_blocks_other_mutations_and_restores_idle() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        reset_state_for_tests();

        let initialization =
            begin_initialization().expect("initialization transition should start");
        assert!(matches!(
            begin_shutdown(),
            ShutdownStart::InitializationInProgress
        ));
        assert!(matches!(
            begin_replace_assignments(),
            ReplaceAssignmentsStart::AlreadyInProgress
        ));

        drop(initialization);
        assert!(matches!(begin_shutdown(), ShutdownStart::NotInitialized));
        let snapshot = DWM_LUT_STATUS.load_for_test();
        assert_eq!(snapshot.hook_status, HookStatus::Inactive as u32);

        reset_state_for_tests();
    }

    #[test]
    fn transition_tokens_publish_ordered_terminal_states() {
        let _guard = HOOK_GLOBAL_TEST_LOCK
            .lock()
            .expect("test mutex should lock");
        reset_state_for_tests();

        begin_initialization()
            .expect("initialization transition should start")
            .commit_active("gaming");
        assert!(is_runtime_active());
        let active = DWM_LUT_STATUS.load_for_test();
        assert_eq!(active.hook_status, HookStatus::Active as u32);
        assert_eq!(&active.profile_name[..6], b"gaming");

        let replacement = match begin_replace_assignments() {
            ReplaceAssignmentsStart::Started(transition) => transition,
            other => panic!("unexpected replace start: {other:?}"),
        };
        assert!(matches!(
            begin_shutdown(),
            ShutdownStart::AssignmentReplacementInProgress
        ));
        replacement.commit_active("updated");
        let replaced = DWM_LUT_STATUS.load_for_test();
        assert_eq!(replaced.hook_status, HookStatus::Active as u32);
        assert_eq!(&replaced.profile_name[..7], b"updated");

        let shutdown = match begin_shutdown() {
            ShutdownStart::Started(transition) => transition,
            other => panic!("unexpected shutdown start: {other:?}"),
        };
        assert!(matches!(
            begin_shutdown(),
            ShutdownStart::ShutdownInProgress
        ));
        assert!(begin_initialization().is_none());
        shutdown.finish_shut_down();
        assert!(!is_runtime_active());
        let inactive = DWM_LUT_STATUS.load_for_test();
        assert_eq!(inactive.hook_status, HookStatus::Inactive as u32);
        assert_eq!(inactive.profile_name_len, 0);

        let reinitialization =
            begin_initialization().expect("reinitialization transition should start");
        drop(reinitialization);
        assert!(matches!(begin_shutdown(), ShutdownStart::AlreadyShutDown));

        reset_state_for_tests();
    }
}
