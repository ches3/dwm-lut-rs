use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::gui::{UiCommand, UiHandle};
use crate::host::HostProcessError;
use crate::inject::{self, HookStatusSnapshot, InjectError};

use super::controller::{HostState, lock_state};
use super::hook_status::{HookStatusStore, HookStatusUpdate};

const STATUS_QUERY_INTERVAL: Duration = Duration::from_secs(2);
const STATUS_POLLER_SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

type StatusQuery = dyn Fn() -> Result<HookStatusSnapshot, InjectError> + Send + Sync + 'static;

#[derive(Clone)]
pub(super) struct StatusPollerHandle {
    wake: SyncSender<()>,
}

impl StatusPollerHandle {
    pub(super) fn request_refresh(&self) {
        match self.wake.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) | Err(TrySendError::Disconnected(())) => {}
        }
    }
}

pub(super) struct StatusPoller {
    handle: StatusPollerHandle,
    stop: Arc<AtomicBool>,
    done: Option<Mutex<Receiver<()>>>,
    thread: Option<JoinHandle<()>>,
}

impl StatusPoller {
    pub(super) fn new(
        state: Arc<Mutex<HostState>>,
        hook_status: HookStatusStore,
        ui: Arc<UiHandle>,
    ) -> Result<Self, HostProcessError> {
        Self::new_with_query(
            state,
            hook_status,
            ui,
            Arc::new(inject::query_status),
            STATUS_QUERY_INTERVAL,
        )
    }

    pub(super) fn new_with_query(
        state: Arc<Mutex<HostState>>,
        hook_status: HookStatusStore,
        ui: Arc<UiHandle>,
        query: Arc<StatusQuery>,
        interval: Duration,
    ) -> Result<Self, HostProcessError> {
        let (wake, wake_receiver) = mpsc::sync_channel(1);
        let (done_sender, done) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("dwm-lut-status".to_string())
            .spawn(move || {
                run_poller(
                    &state,
                    &hook_status,
                    &ui,
                    query.as_ref(),
                    &wake_receiver,
                    &worker_stop,
                    interval,
                );
                let _ = done_sender.send(());
            })
            .map_err(|error| {
                HostProcessError::StartupFailed(format!(
                    "start status poller thread failed: {error}"
                ))
            })?;
        Ok(Self {
            handle: StatusPollerHandle { wake },
            stop,
            done: Some(Mutex::new(done)),
            thread: Some(thread),
        })
    }

    pub(super) fn handle(&self) -> StatusPollerHandle {
        self.handle.clone()
    }

    #[cfg(test)]
    pub(super) fn stopped() -> Self {
        let (wake, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        Self {
            handle: StatusPollerHandle { wake },
            stop: Arc::new(AtomicBool::new(true)),
            done: None,
            thread: None,
        }
    }
}

impl Drop for StatusPoller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.handle.request_refresh();
        let completed = self.done.take().is_none_or(|done| {
            done.into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .recv_timeout(STATUS_POLLER_SHUTDOWN_GRACE)
                .is_ok()
        });
        if completed && let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // Dropping a still-present JoinHandle detaches a worker blocked in a
        // non-cancelable Windows read without delaying host shutdown.
    }
}

fn run_poller(
    state: &Mutex<HostState>,
    hook_status: &HookStatusStore,
    ui: &UiHandle,
    query: &StatusQuery,
    wake: &Receiver<()>,
    stop: &AtomicBool,
    interval: Duration,
) {
    let mut polling = true;
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }

        if polling && let Some(generation) = capture_probe_generation(state, hook_status) {
            let (update, continue_polling) = match query() {
                Ok(snapshot) => {
                    let continue_polling = !matches!(snapshot, HookStatusSnapshot::Inactive);
                    (
                        hook_status.observe_if_generation(generation, snapshot),
                        continue_polling,
                    )
                }
                Err(_) => (
                    hook_status.record_query_failure_if_generation(generation),
                    true,
                ),
            };
            if let Some(update) = update {
                polling = continue_polling;
                if update.should_notify_ui() {
                    notify_status_changed(ui, update);
                }
            }
        }

        if polling {
            match wake.recv_timeout(interval) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match wake.recv() {
                Ok(()) => polling = true,
                Err(_) => return,
            }
        }
    }
}

fn capture_probe_generation(
    state: &Mutex<HostState>,
    hook_status: &HookStatusStore,
) -> Option<u64> {
    let state = lock_state(state);
    if *state != HostState::Idle {
        return None;
    }
    Some(hook_status.probe_generation())
}

fn notify_status_changed(ui: &UiHandle, update: HookStatusUpdate) {
    let _ = ui.send(UiCommand::HookStatusChanged {
        loss_revision: update.loss_revision,
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::host::hook_status::HookRuntimeStatus;

    use super::*;

    #[test]
    fn active_query_updates_status_and_notifies_ui() {
        let state = Arc::new(Mutex::new(HostState::Idle));
        let hook_status = HookStatusStore::default();
        let (ui, commands) = UiHandle::new();
        let _poller = StatusPoller::new_with_query(
            state,
            hook_status.clone(),
            ui,
            Arc::new(|| {
                Ok(HookStatusSnapshot::Active {
                    profile_name: "gaming".to_string(),
                })
            }),
            Duration::from_secs(60),
        )
        .unwrap();

        assert!(matches!(
            commands.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiCommand::HookStatusChanged {
                loss_revision: None
            }
        ));
        assert_eq!(
            hook_status.status(),
            HookRuntimeStatus::Active {
                profile_name: "gaming".to_string()
            }
        );
    }

    #[test]
    fn not_injected_mode_stops_periodic_queries_until_polling_resumes() {
        let state = Arc::new(Mutex::new(HostState::Idle));
        let hook_status = HookStatusStore::default();
        let (ui, commands) = UiHandle::new();
        let (call_sender, call_receiver) = mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let query_calls = Arc::clone(&calls);
        let poller = StatusPoller::new_with_query(
            state,
            hook_status.clone(),
            ui,
            Arc::new(move || {
                let call = query_calls.fetch_add(1, Ordering::SeqCst);
                call_sender.send(call).unwrap();
                if call == 0 {
                    Ok(HookStatusSnapshot::Inactive)
                } else {
                    Ok(HookStatusSnapshot::Active {
                        profile_name: "test".to_string(),
                    })
                }
            }),
            Duration::from_millis(10),
        )
        .unwrap();

        assert_eq!(
            call_receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            0
        );
        assert!(matches!(
            commands.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiCommand::HookStatusChanged {
                loss_revision: None
            }
        ));
        assert!(matches!(
            call_receiver.recv_timeout(Duration::from_millis(40)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        poller.handle().request_refresh();
        assert_eq!(
            call_receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            1
        );
        assert!(matches!(
            commands.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiCommand::HookStatusChanged {
                loss_revision: None
            }
        ));
        assert_eq!(
            hook_status.status(),
            HookRuntimeStatus::Active {
                profile_name: "test".to_string()
            }
        );
    }
}
