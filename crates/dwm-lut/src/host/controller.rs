use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::{InjectorError, ShutdownStatus};
use crate::inject::{self, ApplyReport, ApplyRequest, DisableOutcome, DisableReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostState {
    Running,
    Stopping,
}

#[derive(Debug)]
pub(crate) enum HostCommandError {
    Busy,
    Stopping,
    Injector(InjectorError),
}

impl fmt::Display for HostCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => InjectorError::HostBusy.fmt(formatter),
            Self::Stopping => formatter.write_str("dwm-lut host instance is stopping"),
            Self::Injector(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for HostCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Injector(error) => Some(error),
            Self::Busy | Self::Stopping => None,
        }
    }
}

impl From<InjectorError> for HostCommandError {
    fn from(value: InjectorError) -> Self {
        Self::Injector(value)
    }
}

pub(crate) struct HostController {
    host_dll_path: Option<PathBuf>,
    mutation_lock: Mutex<()>,
    lifecycle: Mutex<HostState>,
}

impl HostController {
    pub(crate) fn new(host_dll_path: Option<PathBuf>) -> Self {
        Self {
            host_dll_path,
            mutation_lock: Mutex::new(()),
            lifecycle: Mutex::new(HostState::Running),
        }
    }

    pub(crate) fn apply(
        &self,
        config_path: PathBuf,
        profile: Option<String>,
    ) -> Result<ApplyReport, HostCommandError> {
        let _mutation = self.try_lock_mutation()?;
        self.ensure_running()?;
        Ok(inject::apply(ApplyRequest {
            dll_path: self.host_dll_path.clone(),
            config_path,
            profile,
        })?)
    }

    pub(crate) fn disable(&self) -> Result<DisableReport, HostCommandError> {
        let _mutation = self.try_lock_mutation()?;
        self.ensure_running()?;
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
    }

    pub(crate) fn state(&self) -> HostState {
        *self.lock_lifecycle()
    }

    pub(crate) fn is_busy(&self) -> bool {
        matches!(
            self.mutation_lock.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        )
    }

    pub(crate) fn perform_while_running<T>(
        &self,
        operation: impl FnOnce() -> Result<T, InjectorError>,
    ) -> Result<T, HostCommandError> {
        let lifecycle = self.lock_lifecycle();
        if *lifecycle == HostState::Stopping {
            return Err(HostCommandError::Stopping);
        }
        Ok(operation()?)
    }

    pub(crate) fn prepare_stop(self: &Arc<Self>) -> Result<StopPermit, HostCommandError> {
        let _mutation = self.try_lock_mutation()?;
        let mut lifecycle = self.lock_lifecycle();
        if *lifecycle == HostState::Stopping {
            return Err(HostCommandError::Stopping);
        }
        *lifecycle = HostState::Stopping;
        drop(lifecycle);
        Ok(StopPermit {
            controller: Arc::clone(self),
            committed: false,
        })
    }

    fn ensure_running(&self) -> Result<(), HostCommandError> {
        if self.state() == HostState::Stopping {
            Err(HostCommandError::Stopping)
        } else {
            Ok(())
        }
    }

    fn try_lock_mutation(&self) -> Result<MutexGuard<'_, ()>, HostCommandError> {
        match self.mutation_lock.try_lock() {
            Ok(guard) => Ok(guard),
            Err(std::sync::TryLockError::WouldBlock) => Err(HostCommandError::Busy),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => Ok(poisoned.into_inner()),
        }
    }

    fn lock_lifecycle(&self) -> MutexGuard<'_, HostState> {
        match self.lifecycle.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn cancel_stop(&self) {
        *self.lock_lifecycle() = HostState::Running;
    }

    #[cfg(test)]
    pub(crate) fn hold_command_lock(&self) -> MutexGuard<'_, ()> {
        self.mutation_lock
            .lock()
            .expect("host mutation lock should be available")
    }
}

pub(crate) struct StopPermit {
    controller: Arc<HostController>,
    committed: bool,
}

impl StopPermit {
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for StopPermit {
    fn drop(&mut self) {
        if !self.committed {
            self.controller.cancel_stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use super::*;

    #[test]
    fn stop_is_busy_while_mutation_lock_is_held() {
        let controller = Arc::new(HostController::new(None));
        let _guard = controller.hold_command_lock();

        let error = match controller.prepare_stop() {
            Ok(_) => panic!("stop must not begin while a mutation is running"),
            Err(error) => error,
        };

        assert!(matches!(error, HostCommandError::Busy));
        assert_eq!(controller.state(), HostState::Running);
    }

    #[test]
    fn dropping_uncommitted_stop_permit_rolls_back_state() {
        let controller = Arc::new(HostController::new(None));

        let permit = controller.prepare_stop().unwrap();
        assert_eq!(controller.state(), HostState::Stopping);
        drop(permit);

        assert_eq!(controller.state(), HostState::Running);
    }

    #[test]
    fn committed_stop_permit_keeps_stopping_state() {
        let controller = Arc::new(HostController::new(None));

        controller.prepare_stop().unwrap().commit();

        assert_eq!(controller.state(), HostState::Stopping);
    }

    #[test]
    fn running_operation_finishes_before_stop_transition() {
        let controller = Arc::new(HostController::new(None));
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let operation_controller = Arc::clone(&controller);
        let operation = std::thread::spawn(move || {
            operation_controller.perform_while_running(|| {
                entered_sender.send(()).unwrap();
                release_receiver.recv().unwrap();
                Ok(())
            })
        });
        entered_receiver.recv().unwrap();

        let (stop_sender, stop_receiver) = mpsc::channel();
        let stop_controller = Arc::clone(&controller);
        let stop = std::thread::spawn(move || {
            stop_sender.send(stop_controller.prepare_stop()).unwrap();
        });

        assert!(matches!(
            stop_receiver.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        release_sender.send(()).unwrap();
        operation.join().unwrap().unwrap();
        let permit = stop_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        permit.commit();
        stop.join().unwrap();
        assert_eq!(controller.state(), HostState::Stopping);
    }
}
