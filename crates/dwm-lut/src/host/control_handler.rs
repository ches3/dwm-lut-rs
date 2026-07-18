use std::sync::Arc;

use crate::control::protocol::{ControlCommand, ControlResponse, ControlStatus};
use crate::control::server::{ControlDispatch, ControlHandler};
use crate::error::{InjectorError, ShutdownStatus};
use crate::inject::{ApplyOutcome, ApplyReport, DisableOutcome, DisableReport};

use super::controller::{HostCommandError, HostController, HostState, MutationCompletion};

pub(crate) struct ControlCommandHandler {
    controller: Arc<HostController>,
}

impl ControlCommandHandler {
    pub(crate) fn new(controller: Arc<HostController>) -> Self {
        Self { controller }
    }
}

impl ControlHandler for ControlCommandHandler {
    fn dispatch(&self, command: ControlCommand) -> ControlDispatch {
        match command {
            ControlCommand::Apply {
                config_path,
                profile,
            } => ControlDispatch::immediate(response_from_apply(
                self.controller
                    .submit_apply(config_path, profile)
                    .and_then(MutationCompletion::wait),
            )),
            ControlCommand::Disable => ControlDispatch::immediate(response_from_disable(
                self.controller
                    .submit_disable()
                    .and_then(MutationCompletion::wait),
            )),
            ControlCommand::Status => {
                let response = match self.controller.state() {
                    HostState::Idle | HostState::Mutating => {
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
                let response = match self.controller.show_gui() {
                    Ok(()) => ControlResponse::ok("showing dwm-lut GUI", ControlStatus::Shown),
                    Err(error) => response_from_error(error),
                };
                ControlDispatch::immediate(response)
            }
            ControlCommand::Stop => match self.controller.prepare_stop() {
                Ok(permit) => ControlDispatch::after_response(
                    ControlResponse::ok("stopped dwm-lut host instance", ControlStatus::Stopped),
                    move || permit.commit(),
                ),
                Err(error) => ControlDispatch::immediate(response_from_error(error)),
            },
        }
    }
}

fn response_from_error(error: HostCommandError) -> ControlResponse {
    match error {
        HostCommandError::Busy => {
            ControlResponse::error(InjectorError::HostBusy.to_string(), ControlStatus::Busy)
        }
        HostCommandError::Stopping => {
            ControlResponse::error("dwm-lut host instance is stopping", ControlStatus::Stopping)
        }
        HostCommandError::MutationExecutorStopped => ControlResponse::error(
            "host mutation executor stopped unexpectedly",
            ControlStatus::Error,
        ),
        HostCommandError::Injector(error) => {
            ControlResponse::error(error.to_string(), ControlStatus::Error)
        }
    }
}

fn apply_message(report: &ApplyReport) -> String {
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

fn disable_message(report: &DisableReport) -> String {
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
    use std::sync::mpsc::{self, Receiver};

    use crate::control::server::ServerShutdown;
    use crate::gui::{UiCommand, UiHandle};

    use super::*;

    fn test_handler() -> (
        ControlCommandHandler,
        Arc<HostController>,
        Receiver<UiCommand>,
    ) {
        let shutdown = Arc::new(ServerShutdown::new());
        let (ui, commands) = UiHandle::new();
        let controller = Arc::new(HostController::new(None, shutdown, ui).unwrap());
        let handler = ControlCommandHandler::new(Arc::clone(&controller));
        (handler, controller, commands)
    }

    #[test]
    fn status_remains_running_and_stop_is_busy_during_mutation() {
        let (handler, controller, _commands) = test_handler();
        let (release_sender, release_receiver) = mpsc::channel();
        let mutation = controller
            .submit_test_mutation(move || {
                release_receiver.recv().unwrap();
                Ok(())
            })
            .unwrap();

        let status = handler.dispatch(ControlCommand::Status);
        let stop = handler.dispatch(ControlCommand::Stop);

        assert!(status.response().ok);
        assert_eq!(status.response().status, ControlStatus::Running);
        assert!(!stop.response().ok);
        assert_eq!(stop.response().status, ControlStatus::Busy);
        release_sender.send(()).unwrap();
        mutation.wait().unwrap();
    }

    #[test]
    fn dropping_stop_dispatch_before_completion_rolls_back_state() {
        let (handler, controller, _commands) = test_handler();

        let dispatch = handler.dispatch(ControlCommand::Stop);
        assert_eq!(controller.state(), HostState::Stopping);
        drop(dispatch);

        assert_eq!(controller.state(), HostState::Idle);
    }

    #[test]
    fn completing_stop_dispatch_commits_state_and_exits_ui() {
        let (handler, controller, commands) = test_handler();

        let dispatch = handler.dispatch(ControlCommand::Stop);
        assert_eq!(dispatch.response().status, ControlStatus::Stopped);
        dispatch.complete().unwrap();

        assert_eq!(controller.state(), HostState::Stopping);
        assert!(matches!(commands.recv().unwrap(), UiCommand::Exit));
    }
}
