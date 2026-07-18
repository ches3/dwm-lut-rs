use std::fmt;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::control::server::ServerShutdown;
use crate::error::{InjectorError, ShutdownStatus};
use crate::gui::{UiCommand, UiHandle};
use crate::inject::{self, ApplyReport, ApplyRequest, DisableOutcome, DisableReport};

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
    Injector(InjectorError),
}

impl fmt::Display for HostCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => InjectorError::HostBusy.fmt(formatter),
            Self::Stopping => formatter.write_str("dwm-lut host instance is stopping"),
            Self::MutationExecutorStopped => {
                formatter.write_str("host mutation executor stopped unexpectedly")
            }
            Self::Injector(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for HostCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Injector(error) => Some(error),
            Self::Busy | Self::Stopping | Self::MutationExecutorStopped => None,
        }
    }
}

impl From<InjectorError> for HostCommandError {
    fn from(value: InjectorError) -> Self {
        Self::Injector(value)
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
    fn new() -> Result<Self, InjectorError> {
        let (sender, receiver) = mpsc::channel::<MutationJob>();
        let thread = std::thread::Builder::new()
            .name("dwm-lut-mutation".to_string())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    job();
                }
            })
            .map_err(|error| {
                InjectorError::HostStartupFailed(format!(
                    "host mutation executor startup failed: {error}"
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
}

impl Drop for MutationStateGuard {
    fn drop(&mut self) {
        *lock_state(&self.state) = HostState::Idle;
    }
}

pub(crate) struct HostController {
    host_dll_path: Option<PathBuf>,
    state: Arc<Mutex<HostState>>,
    executor: MutationExecutor,
    shutdown: Arc<ServerShutdown>,
    ui: Arc<UiHandle>,
}

impl HostController {
    pub(crate) fn new(
        host_dll_path: Option<PathBuf>,
        shutdown: Arc<ServerShutdown>,
        ui: Arc<UiHandle>,
    ) -> Result<Self, InjectorError> {
        Ok(Self {
            host_dll_path,
            state: Arc::new(Mutex::new(HostState::Idle)),
            executor: MutationExecutor::new()?,
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
        self.submit_mutation(move || {
            Ok(inject::apply(ApplyRequest {
                dll_path,
                config_path,
                profile,
            })?)
        })
    }

    pub(crate) fn submit_disable(
        &self,
    ) -> Result<MutationCompletion<DisableReport>, HostCommandError> {
        self.submit_mutation(move || {
            let report = inject::disable()?;
            match report.outcome {
                DisableOutcome::NotInjected
                | DisableOutcome::ShutDown(ShutdownStatus::Success)
                | DisableOutcome::ShutDown(ShutdownStatus::NotInitialized)
                | DisableOutcome::ShutDown(ShutdownStatus::AlreadyShutDown) => Ok(report),
                DisableOutcome::ShutDown(status) => Err(HostCommandError::Injector(
                    InjectorError::HookShutdownFailed(status),
                )),
            }
        })
    }

    pub(crate) fn state(&self) -> HostState {
        *lock_state(&self.state)
    }

    pub(crate) fn show_gui(&self) -> Result<(), HostCommandError> {
        let state = lock_state(&self.state);
        if *state == HostState::Stopping {
            return Err(HostCommandError::Stopping);
        }
        let result = self.ui.send(UiCommand::Show);
        drop(state);
        result?;
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
        Ok(PreparedStop {
            state: Arc::clone(&self.state),
            shutdown: Arc::clone(&self.shutdown),
            ui: Arc::clone(&self.ui),
            committed: false,
        })
    }

    pub(crate) fn stop(&self) -> Result<(), HostCommandError> {
        self.prepare_stop()?.commit()?;
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
                HostState::Idle => *state = HostState::Mutating,
                HostState::Mutating => return Err(HostCommandError::Busy),
                HostState::Stopping => return Err(HostCommandError::Stopping),
            }
        }

        let guard = MutationStateGuard {
            state: Arc::clone(&self.state),
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        self.executor.submit(Box::new(move || {
            let result = operation();
            drop(guard);
            let _ = sender.send(result);
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

fn lock_state(state: &Mutex<HostState>) -> MutexGuard<'_, HostState> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) struct PreparedStop {
    state: Arc<Mutex<HostState>>,
    shutdown: Arc<ServerShutdown>,
    ui: Arc<UiHandle>,
    committed: bool,
}

impl PreparedStop {
    pub(crate) fn commit(mut self) -> Result<(), InjectorError> {
        self.shutdown.request();
        self.committed = true;
        self.ui.send(UiCommand::Exit)
    }
}

impl Drop for PreparedStop {
    fn drop(&mut self) {
        if !self.committed {
            *lock_state(&self.state) = HostState::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    use super::*;

    fn test_controller() -> (Arc<HostController>, Receiver<UiCommand>) {
        let shutdown = Arc::new(ServerShutdown::new());
        let (ui, commands) = UiHandle::new();
        (
            Arc::new(HostController::new(None, shutdown, ui).unwrap()),
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
        assert!(matches!(commands.recv().unwrap(), UiCommand::Exit));

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
        let (ui, _commands) = UiHandle::new();
        let controller = HostController {
            host_dll_path: None,
            state: Arc::new(Mutex::new(HostState::Idle)),
            executor: MutationExecutor::stopped(),
            shutdown,
            ui,
        };

        assert!(matches!(
            controller.submit_test_mutation(|| Ok(())),
            Err(HostCommandError::MutationExecutorStopped)
        ));
        assert_eq!(controller.state(), HostState::Idle);
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
        let (controller, _commands) = test_controller();

        let prepared = controller.prepare_stop().unwrap();
        assert_eq!(controller.state(), HostState::Stopping);
        drop(prepared);

        assert_eq!(controller.state(), HostState::Idle);
    }

    #[test]
    fn committed_prepared_stop_keeps_stopping_state_and_exits_ui() {
        let (controller, commands) = test_controller();

        controller.prepare_stop().unwrap().commit().unwrap();

        assert_eq!(controller.state(), HostState::Stopping);
        assert!(matches!(commands.recv().unwrap(), UiCommand::Exit));
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

        assert!(matches!(commands.recv().unwrap(), UiCommand::Show));
        release_sender.send(()).unwrap();
        mutation.wait().unwrap();
    }
}
