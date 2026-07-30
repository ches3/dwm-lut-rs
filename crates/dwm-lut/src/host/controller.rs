use std::fmt;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::control::server::ServerShutdown;
use crate::gui::{UiCommand, UiHandle};
use crate::host::{HOST_BUSY_MESSAGE, HostProcessError};
use crate::inject::{
    self, ApplyReport, ApplyRequest, DisableOutcome, DisableReport, InjectError, ShutdownStatus,
};

use super::hook_status::{HookRuntimeStatus, HookStatusStore};
use super::status_poller::{StatusPoller, StatusPollerHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostState {
    Idle,
    Mutating,
    Stopping,
}

#[derive(Debug)]
pub(crate) enum HostCommandError {
    Busy,
    Stopping,
    MutationExecutorStopped,
    Inject(InjectError),
    UiUnavailable,
}

impl fmt::Display for HostCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => formatter.write_str(HOST_BUSY_MESSAGE),
            Self::Stopping => formatter.write_str("dwm-lut host instance is stopping"),
            Self::MutationExecutorStopped => {
                formatter.write_str("host mutation executor stopped unexpectedly")
            }
            Self::Inject(error) => error.fmt(formatter),
            Self::UiUnavailable => HostProcessError::UiUnavailable.fmt(formatter),
        }
    }
}

impl std::error::Error for HostCommandError {}

impl From<InjectError> for HostCommandError {
    fn from(value: InjectError) -> Self {
        Self::Inject(value)
    }
}

pub(crate) struct MutationCompletion<T> {
    receiver: Option<Receiver<Result<T, HostCommandError>>>,
}

impl<T> MutationCompletion<T> {
    pub(crate) fn try_take(&mut self) -> Option<Result<T, HostCommandError>> {
        match self.receiver.as_ref()?.try_recv() {
            Ok(result) => {
                self.receiver.take();
                Some(result)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.receiver.take();
                Some(Err(HostCommandError::MutationExecutorStopped))
            }
        }
    }

    pub(crate) fn wait(mut self) -> Result<T, HostCommandError> {
        let Some(receiver) = self.receiver.take() else {
            return Err(HostCommandError::MutationExecutorStopped);
        };
        receiver
            .recv()
            .unwrap_or(Err(HostCommandError::MutationExecutorStopped))
    }

    fn new(receiver: Receiver<Result<T, HostCommandError>>) -> Self {
        Self {
            receiver: Some(receiver),
        }
    }

    #[cfg(test)]
    pub(crate) fn disconnected() -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(sender);
        Self::new(receiver)
    }
}

type MutationJob = Box<dyn FnOnce() + Send + 'static>;

struct MutationExecutor {
    sender: Option<mpsc::Sender<MutationJob>>,
    thread: Option<JoinHandle<()>>,
}

impl MutationExecutor {
    fn new() -> Result<Self, HostProcessError> {
        let (sender, receiver) = mpsc::channel::<MutationJob>();
        let thread = std::thread::Builder::new()
            .name("dwm-lut-mutation".to_string())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    job();
                }
            })
            .map_err(|error| {
                HostProcessError::StartupFailed(format!(
                    "start mutation executor thread failed: {error}"
                ))
            })?;
        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
        })
    }

    fn submit(&self, job: MutationJob) -> Result<(), HostCommandError> {
        self.sender
            .as_ref()
            .ok_or(HostCommandError::MutationExecutorStopped)?
            .send(job)
            .map_err(|_| HostCommandError::MutationExecutorStopped)
    }

    #[cfg(test)]
    fn stopped() -> Self {
        Self {
            sender: None,
            thread: None,
        }
    }
}

impl Drop for MutationExecutor {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct MutationStateGuard {
    state: Arc<Mutex<HostState>>,
    ui: Arc<UiHandle>,
    status_poller: StatusPollerHandle,
    armed: bool,
}

impl MutationStateGuard {
    fn complete<T>(
        mut self,
        sender: mpsc::SyncSender<Result<T, HostCommandError>>,
        result: Result<T, HostCommandError>,
    ) {
        *lock_state(&self.state) = HostState::Idle;
        self.armed = false;
        let _ = sender.send(result);
        notify_state_changed(&self.ui);
        self.status_poller.request_refresh();
    }
}

impl Drop for MutationStateGuard {
    fn drop(&mut self) {
        if self.armed {
            *lock_state(&self.state) = HostState::Idle;
            notify_state_changed(&self.ui);
            self.status_poller.request_refresh();
        }
    }
}

pub(crate) struct HostController {
    host_dll_path: Option<PathBuf>,
    state: Arc<Mutex<HostState>>,
    hook_status: HookStatusStore,
    status_poller: StatusPoller,
    executor: MutationExecutor,
    shutdown: Arc<ServerShutdown>,
    ui: Arc<UiHandle>,
}

impl HostController {
    pub(crate) fn new(
        host_dll_path: Option<PathBuf>,
        shutdown: Arc<ServerShutdown>,
        ui: Arc<UiHandle>,
    ) -> Result<Self, HostProcessError> {
        let state = Arc::new(Mutex::new(HostState::Idle));
        let hook_status = HookStatusStore::default();
        let executor = MutationExecutor::new()?;
        let status_poller =
            StatusPoller::new(Arc::clone(&state), hook_status.clone(), Arc::clone(&ui))?;
        Ok(Self {
            host_dll_path,
            state,
            hook_status,
            status_poller,
            executor,
            shutdown,
            ui,
        })
    }

    pub(crate) fn submit_apply(
        &self,
        config_path: PathBuf,
        profile: Option<String>,
    ) -> Result<MutationCompletion<ApplyReport>, HostCommandError> {
        let dll_path = self.host_dll_path.clone();
        let hook_status = self.hook_status.clone();
        self.submit_mutation(move || {
            let report = inject::apply(ApplyRequest {
                dll_path,
                config_path,
                profile,
            })?;
            hook_status.record_apply(report.profile_name.clone());
            Ok(report)
        })
    }

    pub(crate) fn submit_disable(
        &self,
    ) -> Result<MutationCompletion<DisableReport>, HostCommandError> {
        let hook_status = self.hook_status.clone();
        self.submit_mutation(move || {
            let report = inject::disable()?;
            validate_disable_outcome(report.outcome)?;
            hook_status.record_disable();
            Ok(report)
        })
    }

    pub(crate) fn state(&self) -> HostState {
        *lock_state(&self.state)
    }

    pub(crate) fn hook_status(&self) -> HookRuntimeStatus {
        self.hook_status.status()
    }

    pub(crate) fn should_report_hook_loss(&self, revision: u64) -> bool {
        self.hook_status.should_report_loss(revision)
    }

    pub(crate) fn show_gui(&self) -> Result<(), HostCommandError> {
        let state = lock_state(&self.state);
        if *state == HostState::Stopping {
            return Err(HostCommandError::Stopping);
        }
        let result = self.ui.send(UiCommand::Show);
        drop(state);
        result.map_err(|_| HostCommandError::UiUnavailable)?;
        Ok(())
    }

    pub(crate) fn prepare_stop(&self) -> Result<PreparedStop, HostCommandError> {
        let mut state = lock_state(&self.state);
        match *state {
            HostState::Idle => *state = HostState::Stopping,
            HostState::Mutating => return Err(HostCommandError::Busy),
            HostState::Stopping => return Err(HostCommandError::Stopping),
        }
        drop(state);
        notify_state_changed(&self.ui);
        Ok(PreparedStop {
            state: Arc::clone(&self.state),
            shutdown: Arc::clone(&self.shutdown),
            ui: Arc::clone(&self.ui),
            committed: false,
        })
    }

    pub(crate) fn stop(&self) -> Result<(), HostCommandError> {
        self.prepare_stop()?
            .commit()
            .map_err(|_| HostCommandError::UiUnavailable)?;
        Ok(())
    }

    fn submit_mutation<T, F>(&self, operation: F) -> Result<MutationCompletion<T>, HostCommandError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, HostCommandError> + Send + 'static,
    {
        {
            let mut state = lock_state(&self.state);
            match *state {
                HostState::Idle => {
                    self.hook_status.begin_mutation();
                    *state = HostState::Mutating;
                }
                HostState::Mutating => return Err(HostCommandError::Busy),
                HostState::Stopping => return Err(HostCommandError::Stopping),
            }
        }
        notify_state_changed(&self.ui);

        let guard = MutationStateGuard {
            state: Arc::clone(&self.state),
            ui: Arc::clone(&self.ui),
            status_poller: self.status_poller.handle(),
            armed: true,
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        self.executor.submit(Box::new(move || {
            let result = operation();
            guard.complete(sender, result);
        }))?;
        Ok(MutationCompletion::new(receiver))
    }

    #[cfg(test)]
    pub(super) fn submit_test_mutation<T, F>(
        &self,
        operation: F,
    ) -> Result<MutationCompletion<T>, HostCommandError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, HostCommandError> + Send + 'static,
    {
        self.submit_mutation(operation)
    }
}

pub(super) fn lock_state(state: &Mutex<HostState>) -> MutexGuard<'_, HostState> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn validate_disable_outcome(outcome: DisableOutcome) -> Result<(), HostCommandError> {
    match outcome {
        DisableOutcome::NotInjected
        | DisableOutcome::ShutDown(ShutdownStatus::Success)
        | DisableOutcome::ShutDown(ShutdownStatus::NotInitialized)
        | DisableOutcome::ShutDown(ShutdownStatus::AlreadyShutDown) => Ok(()),
        DisableOutcome::ShutDown(status) => Err(HostCommandError::Inject(
            InjectError::HookShutdownFailed(status),
        )),
    }
}

fn notify_state_changed(ui: &UiHandle) {
    let _ = ui.send(UiCommand::HostStateChanged);
}

pub(crate) struct PreparedStop {
    state: Arc<Mutex<HostState>>,
    shutdown: Arc<ServerShutdown>,
    ui: Arc<UiHandle>,
    committed: bool,
}

impl PreparedStop {
    pub(crate) fn commit(mut self) -> Result<(), HostProcessError> {
        self.shutdown.request();
        self.committed = true;
        self.ui.send(UiCommand::Exit)
    }
}

impl Drop for PreparedStop {
    fn drop(&mut self) {
        if !self.committed {
            *lock_state(&self.state) = HostState::Idle;
            notify_state_changed(&self.ui);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    use super::*;

    fn test_controller() -> (Arc<HostController>, Receiver<UiCommand>) {
        let shutdown = Arc::new(ServerShutdown::new());
        let (ui, commands) = UiHandle::new();
        let state = Arc::new(Mutex::new(HostState::Idle));
        (
            Arc::new(HostController {
                host_dll_path: None,
                state,
                hook_status: HookStatusStore::default(),
                status_poller: StatusPoller::stopped(),
                executor: MutationExecutor::new().unwrap(),
                shutdown,
                ui,
            }),
            commands,
        )
    }

    fn wait_until_idle(controller: &HostController) {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while controller.state() == HostState::Mutating {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn disable_outcome_validation_accepts_benign_statuses() {
        for outcome in [
            DisableOutcome::NotInjected,
            DisableOutcome::ShutDown(ShutdownStatus::Success),
            DisableOutcome::ShutDown(ShutdownStatus::NotInitialized),
            DisableOutcome::ShutDown(ShutdownStatus::AlreadyShutDown),
        ] {
            validate_disable_outcome(outcome).unwrap();
        }
    }

    #[test]
    fn disable_outcome_validation_rejects_cleanup_failure() {
        assert!(matches!(
            validate_disable_outcome(DisableOutcome::ShutDown(
                ShutdownStatus::MinHookCleanupFailed
            )),
            Err(HostCommandError::Inject(InjectError::HookShutdownFailed(
                ShutdownStatus::MinHookCleanupFailed
            )))
        ));
    }

    #[test]
    fn submitted_mutation_runs_without_polling_completion() {
        let (controller, _commands) = test_controller();
        let (finished_sender, finished_receiver) = mpsc::channel();

        let completion = controller
            .submit_test_mutation(move || {
                finished_sender.send(()).unwrap();
                Ok(42)
            })
            .unwrap();

        finished_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        wait_until_idle(&controller);
        assert_eq!(controller.state(), HostState::Idle);
        assert_eq!(completion.wait().unwrap(), 42);
    }

    #[test]
    fn blocked_status_query_does_not_block_mutation_executor() {
        let shutdown = Arc::new(ServerShutdown::new());
        let (ui, _commands) = UiHandle::new();
        let state = Arc::new(Mutex::new(HostState::Idle));
        let hook_status = HookStatusStore::default();
        let (query_started_sender, query_started_receiver) = mpsc::channel();
        let (release_query_sender, release_query_receiver) = mpsc::channel();
        let release_query_receiver = Arc::new(Mutex::new(release_query_receiver));
        let query_release = Arc::clone(&release_query_receiver);
        let query_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&query_calls);
        let status_poller = StatusPoller::new_with_query(
            Arc::clone(&state),
            hook_status.clone(),
            Arc::clone(&ui),
            Arc::new(move || {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    query_started_sender.send(()).unwrap();
                    query_release.lock().unwrap().recv().unwrap();
                }
                Ok(crate::inject::HookStatusSnapshot::Inactive)
            }),
            Duration::from_secs(60),
        )
        .unwrap();
        let controller = HostController {
            host_dll_path: None,
            state,
            hook_status,
            status_poller,
            executor: MutationExecutor::new().unwrap(),
            shutdown,
            ui,
        };

        query_started_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        let completion = controller.submit_test_mutation(|| Ok(42)).unwrap();
        assert_eq!(completion.wait().unwrap(), 42);
        release_query_sender.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while query_calls.load(Ordering::SeqCst) < 2 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn completion_is_delivered_after_mutation_becomes_idle() {
        let (controller, _commands) = test_controller();
        let mut completion = controller.submit_test_mutation(|| Ok(42)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);

        let result = loop {
            if let Some(result) = completion.try_take() {
                break result;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };

        assert_eq!(result.unwrap(), 42);
        assert_eq!(controller.state(), HostState::Idle);
        assert!(completion.try_take().is_none());
    }

    #[test]
    fn idle_notification_follows_completion_delivery() {
        let (controller, commands) = test_controller();
        let (release_sender, release_receiver) = mpsc::channel();
        let mut completion = controller
            .submit_test_mutation(move || {
                release_receiver.recv().unwrap();
                Ok(42)
            })
            .unwrap();

        assert_eq!(commands.recv().unwrap(), UiCommand::HostStateChanged);
        release_sender.send(()).unwrap();
        assert_eq!(
            commands.recv_timeout(Duration::from_secs(1)).unwrap(),
            UiCommand::HostStateChanged
        );
        assert_eq!(completion.try_take().unwrap().unwrap(), 42);
    }

    #[test]
    fn submitted_mutation_reserves_busy_state_before_execution() {
        let (controller, _commands) = test_controller();
        let (release_sender, release_receiver) = mpsc::channel();
        let completion = controller
            .submit_test_mutation(move || {
                release_receiver.recv().unwrap();
                Ok(())
            })
            .unwrap();

        assert_eq!(controller.state(), HostState::Mutating);
        assert!(matches!(
            controller.submit_test_mutation(|| Ok(())),
            Err(HostCommandError::Busy)
        ));
        assert!(matches!(
            controller.prepare_stop(),
            Err(HostCommandError::Busy)
        ));

        release_sender.send(()).unwrap();
        completion.wait().unwrap();
        assert_eq!(controller.state(), HostState::Idle);
    }

    #[test]
    fn stopping_controller_rejects_mutations_and_show_gui() {
        let (controller, commands) = test_controller();
        controller.prepare_stop().unwrap().commit().unwrap();
        assert_eq!(commands.recv().unwrap(), UiCommand::HostStateChanged);
        assert_eq!(commands.recv().unwrap(), UiCommand::Exit);

        assert!(matches!(
            controller.submit_test_mutation(|| Ok(())),
            Err(HostCommandError::Stopping)
        ));
        assert!(matches!(
            controller.show_gui(),
            Err(HostCommandError::Stopping)
        ));
    }

    #[test]
    fn stopped_executor_rejects_mutation_and_releases_busy_state() {
        let shutdown = Arc::new(ServerShutdown::new());
        let (ui, commands) = UiHandle::new();
        let controller = HostController {
            host_dll_path: None,
            state: Arc::new(Mutex::new(HostState::Idle)),
            hook_status: HookStatusStore::default(),
            status_poller: StatusPoller::stopped(),
            executor: MutationExecutor::stopped(),
            shutdown,
            ui,
        };

        assert!(matches!(
            controller.submit_test_mutation(|| Ok(())),
            Err(HostCommandError::MutationExecutorStopped)
        ));
        assert_eq!(controller.state(), HostState::Idle);
        assert_eq!(commands.recv().unwrap(), UiCommand::HostStateChanged);
        assert_eq!(commands.recv().unwrap(), UiCommand::HostStateChanged);
    }

    #[test]
    fn disconnected_completion_reports_executor_failure() {
        let completion = MutationCompletion::<()>::disconnected();

        assert!(matches!(
            completion.wait(),
            Err(HostCommandError::MutationExecutorStopped)
        ));
    }

    #[test]
    fn dropping_prepared_stop_rolls_back_state() {
        let (controller, commands) = test_controller();

        let prepared = controller.prepare_stop().unwrap();
        assert_eq!(controller.state(), HostState::Stopping);
        assert_eq!(commands.recv().unwrap(), UiCommand::HostStateChanged);
        drop(prepared);

        assert_eq!(controller.state(), HostState::Idle);
        assert_eq!(commands.recv().unwrap(), UiCommand::HostStateChanged);
    }

    #[test]
    fn committed_prepared_stop_keeps_stopping_state_and_exits_ui() {
        let (controller, commands) = test_controller();

        controller.prepare_stop().unwrap().commit().unwrap();

        assert_eq!(controller.state(), HostState::Stopping);
        assert_eq!(commands.recv().unwrap(), UiCommand::HostStateChanged);
        assert_eq!(commands.recv().unwrap(), UiCommand::Exit);
    }

    #[test]
    fn show_gui_remains_available_during_mutation() {
        let (controller, commands) = test_controller();
        let (release_sender, release_receiver) = mpsc::channel();
        let mutation = controller
            .submit_test_mutation(move || {
                release_receiver.recv().unwrap();
                Ok(())
            })
            .unwrap();

        controller.show_gui().unwrap();

        assert_eq!(commands.recv().unwrap(), UiCommand::HostStateChanged);
        assert_eq!(commands.recv().unwrap(), UiCommand::Show);
        release_sender.send(()).unwrap();
        mutation.wait().unwrap();
        assert_eq!(commands.recv().unwrap(), UiCommand::HostStateChanged);
    }
}
