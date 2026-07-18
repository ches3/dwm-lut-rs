use std::path::PathBuf;
use std::sync::Arc;

use crate::control::protocol::{ControlCommand, ControlResponse, ControlStatus};
use crate::control::server::{ControlDispatch, ControlHandler, ServerShutdown};
use crate::error::{InjectorError, ShutdownStatus};
use crate::gui::{UiCommand, UiHandle};
use crate::inject::{ApplyOutcome, ApplyReport, DisableOutcome, DisableReport};

use super::controller::{HostCommandError, HostController, HostState};

pub(crate) struct HostApplication {
    controller: Arc<HostController>,
    shutdown: Arc<ServerShutdown>,
    ui: Arc<UiHandle>,
}

impl HostApplication {
    pub(crate) fn new(
        controller: Arc<HostController>,
        shutdown: Arc<ServerShutdown>,
        ui: Arc<UiHandle>,
    ) -> Self {
        Self {
            controller,
            shutdown,
            ui,
        }
    }

    pub(crate) fn apply(
        &self,
        config_path: PathBuf,
        profile: Option<String>,
    ) -> Result<ApplyReport, HostCommandError> {
        self.controller.apply(config_path, profile)
    }

    pub(crate) fn disable(&self) -> Result<DisableReport, HostCommandError> {
        self.controller.disable()
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.controller.is_busy()
    }

    pub(crate) fn state(&self) -> HostState {
        self.controller.state()
    }

    pub(crate) fn request_exit(self: &Arc<Self>) -> Result<(), HostCommandError> {
        let permit = self.controller.prepare_stop()?;
        self.shutdown.request();
        permit.commit();
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn test_instance() -> Arc<Self> {
        let controller = Arc::new(HostController::new(None));
        let shutdown = Arc::new(ServerShutdown::new());
        let (ui, _commands) = UiHandle::new();
        Arc::new(Self::new(controller, shutdown, ui))
    }

    fn dispatch_command(&self, command: ControlCommand) -> ControlDispatch {
        match command {
            ControlCommand::Apply {
                config_path,
                profile,
            } => ControlDispatch::immediate(response_from_apply(
                self.controller.apply(config_path, profile),
            )),
            ControlCommand::Disable => {
                ControlDispatch::immediate(response_from_disable(self.controller.disable()))
            }
            ControlCommand::Status => {
                let response = match self.controller.state() {
                    HostState::Running => {
                        ControlResponse::ok("host instance is running", ControlStatus::Running)
                    }
                    HostState::Stopping => ControlResponse::ok(
                        "dwm-lut host instance is stopping",
                        ControlStatus::Stopping,
                    ),
                };
                ControlDispatch::immediate(response)
            }
            ControlCommand::ShowGui => {
                let response = match self
                    .controller
                    .perform_while_running(|| self.ui.send(UiCommand::Show))
                {
                    Ok(()) => ControlResponse::ok("showing dwm-lut GUI", ControlStatus::Shown),
                    Err(error) => response_from_error(error),
                };
                ControlDispatch::immediate(response)
            }
            ControlCommand::Stop => match self.controller.prepare_stop() {
                Ok(permit) => {
                    let shutdown = Arc::clone(&self.shutdown);
                    let ui = Arc::clone(&self.ui);
                    ControlDispatch::after_response(
                        ControlResponse::ok(
                            "stopped dwm-lut host instance",
                            ControlStatus::Stopped,
                        ),
                        move || {
                            shutdown.request();
                            permit.commit();
                            ui.send(UiCommand::Exit)
                        },
                    )
                }
                Err(error) => ControlDispatch::immediate(response_from_error(error)),
            },
        }
    }
}

impl ControlHandler for HostApplication {
    fn dispatch(&self, command: ControlCommand) -> ControlDispatch {
        self.dispatch_command(command)
    }
}

pub(crate) fn response_from_error(error: HostCommandError) -> ControlResponse {
    match error {
        HostCommandError::Busy => {
            ControlResponse::error(InjectorError::HostBusy.to_string(), ControlStatus::Busy)
        }
        HostCommandError::Stopping => {
            ControlResponse::error("dwm-lut host instance is stopping", ControlStatus::Stopping)
        }
        HostCommandError::Injector(error) => {
            ControlResponse::error(error.to_string(), ControlStatus::Error)
        }
    }
}

pub(crate) fn response_from_injector_error(error: InjectorError) -> ControlResponse {
    ControlResponse::error(error.to_string(), ControlStatus::Error)
}

pub(crate) fn apply_message(report: &ApplyReport) -> String {
    match report.outcome {
        ApplyOutcome::Replaced => format!(
            "replaced assignments in dwm.exe (pid={pid}) from {} (profile={})",
            report.config_path.display(),
            report.profile_name,
            pid = report.pid,
        ),
        ApplyOutcome::Initialized => format!(
            "initialized dwm.exe (pid={pid}) with {} staged from {} and {} (profile={})",
            report.staged_dll_path.display(),
            report.input_dll_path.display(),
            report.config_path.display(),
            report.profile_name,
            pid = report.pid,
        ),
        ApplyOutcome::Reinitialized => format!(
            "reinitialized dwm.exe (pid={pid}) with {} staged from {} and {} (profile={})",
            report.staged_dll_path.display(),
            report.input_dll_path.display(),
            report.config_path.display(),
            report.profile_name,
            pid = report.pid,
        ),
    }
}

pub(crate) fn disable_message(report: &DisableReport) -> String {
    match report.outcome {
        DisableOutcome::NotInjected => format!(
            "disable skipped: hook DLL is not injected into dwm.exe (pid={})",
            report.pid
        ),
        DisableOutcome::ShutDown(ShutdownStatus::Success) => {
            format!("disabled dwm.exe hook (pid={})", report.pid)
        }
        DisableOutcome::ShutDown(ShutdownStatus::NotInitialized) => format!(
            "disable skipped: hook DLL is loaded but not initialized in dwm.exe (pid={})",
            report.pid
        ),
        DisableOutcome::ShutDown(ShutdownStatus::AlreadyShutDown) => format!(
            "disable skipped: hook DLL is already shut down in dwm.exe (pid={})",
            report.pid
        ),
        DisableOutcome::ShutDown(status) => format!("hook shutdown failed: {status}"),
    }
}

fn response_from_apply(result: Result<ApplyReport, HostCommandError>) -> ControlResponse {
    match result {
        Ok(report) => {
            let status = match report.outcome {
                ApplyOutcome::Replaced => ControlStatus::Replaced,
                ApplyOutcome::Initialized => ControlStatus::Initialized,
                ApplyOutcome::Reinitialized => ControlStatus::Reinitialized,
            };
            ControlResponse::ok(apply_message(&report), status)
        }
        Err(error) => response_from_error(error),
    }
}

fn response_from_disable(result: Result<DisableReport, HostCommandError>) -> ControlResponse {
    match result {
        Ok(report) => {
            let status = match report.outcome {
                DisableOutcome::NotInjected => ControlStatus::NotInjected,
                DisableOutcome::ShutDown(ShutdownStatus::Success) => ControlStatus::Disabled,
                DisableOutcome::ShutDown(ShutdownStatus::NotInitialized) => {
                    ControlStatus::NotInitialized
                }
                DisableOutcome::ShutDown(ShutdownStatus::AlreadyShutDown) => {
                    ControlStatus::AlreadyShutdown
                }
                DisableOutcome::ShutDown(_) => ControlStatus::Error,
            };
            ControlResponse::ok(disable_message(&report), status)
        }
        Err(error) => response_from_error(error),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;

    use super::*;

    fn test_application() -> (
        Arc<HostApplication>,
        Arc<HostController>,
        Receiver<UiCommand>,
    ) {
        let controller = Arc::new(HostController::new(None));
        let shutdown = Arc::new(ServerShutdown::new());
        let (ui, commands) = UiHandle::new();
        (
            Arc::new(HostApplication::new(Arc::clone(&controller), shutdown, ui)),
            controller,
            commands,
        )
    }

    #[test]
    fn show_gui_does_not_wait_for_mutation_lock() {
        let (application, controller, commands) = test_application();
        let _mutation = controller.hold_command_lock();

        let dispatch = application.dispatch(ControlCommand::ShowGui);

        assert!(dispatch.response().ok);
        assert_eq!(dispatch.response().status, ControlStatus::Shown);
        assert!(matches!(commands.recv().unwrap(), UiCommand::Show));
    }

    #[test]
    fn dropping_stop_dispatch_before_completion_rolls_back_state() {
        let (application, controller, _commands) = test_application();

        let dispatch = application.dispatch(ControlCommand::Stop);
        assert_eq!(controller.state(), HostState::Stopping);
        drop(dispatch);

        assert_eq!(controller.state(), HostState::Running);
    }

    #[test]
    fn completing_stop_dispatch_commits_state_and_exits_ui() {
        let (application, controller, commands) = test_application();

        let dispatch = application.dispatch(ControlCommand::Stop);
        assert_eq!(dispatch.response().status, ControlStatus::Stopped);
        dispatch.complete().unwrap();

        assert_eq!(controller.state(), HostState::Stopping);
        assert!(matches!(commands.recv().unwrap(), UiCommand::Exit));
    }
}
