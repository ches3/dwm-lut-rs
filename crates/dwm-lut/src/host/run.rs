use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::control;
use crate::control::ControlError;
use crate::control::protocol::{ControlCommand, ControlRequest, ControlStatus};
use crate::control::server::ServerShutdown;
use crate::entry::BackgroundOptions;
use crate::gui;
use crate::host::launch::StartupNotifier;
use crate::host::{
    ControlCommandHandler, HostController, HostInstanceClaim, HostInstanceGuard,
    HostInstanceWaiter, HostProcessError, HostRunError,
};
use crate::inject;
use crate::panic_report;
use crate::paths::PathError;
use crate::platform::dialog;
use crate::platform::elevation;
use crate::platform::security::{SecurityDescriptor, UserSid};

const HOST_INSTANCE_TRANSITION_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_INSTANCE_WAIT_SLICE: Duration = Duration::from_millis(100);

pub fn run_background(options: BackgroundOptions) -> Result<(), HostRunError> {
    if options.startup_result_pipe.is_none() {
        let elevated = match elevation::is_process_elevated() {
            Ok(elevated) => elevated,
            Err(source) => {
                let error = HostRunError::from(HostProcessError::LaunchFailed {
                    operation: "check process elevation",
                    source,
                });
                show_background_error(&error);
                return Err(error);
            }
        };
        if !elevated {
            let executable = match std::env::current_exe() {
                Ok(executable) => executable,
                Err(source) => {
                    let error = HostRunError::from(PathError::Io {
                        operation: "resolve host executable",
                        source,
                    });
                    show_background_error(&error);
                    return Err(error);
                }
            };
            return match crate::host::launch::start_background_host(&executable, options.dll_path) {
                Ok(()) | Err(HostProcessError::AlreadyRunning) => Ok(()),
                Err(error) => {
                    let error = HostRunError::from(error);
                    show_background_error(&error);
                    Err(error)
                }
            };
        }
    }
    match run_host(options) {
        Ok(()) | Err(HostRunError::Process(HostProcessError::AlreadyRunning)) => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn run_host(options: BackgroundOptions) -> Result<(), HostRunError> {
    let BackgroundOptions {
        dll_path,
        startup_result_pipe,
        panic_report_event,
    } = options;
    let startup_reporting_configured = startup_result_pipe.is_some();
    let mut startup_notifier = None;
    let startup_completed = Arc::new(AtomicBool::new(false));
    let result = (|| -> Result<(), HostRunError> {
        panic_report::configure(panic_report_event.as_deref())?;
        startup_notifier = startup_result_pipe
            .map(StartupNotifier::connect)
            .transpose()?;
        run_host_session(
            dll_path,
            &mut startup_notifier,
            Arc::clone(&startup_completed),
        )
    })();
    if let Err(error) = &result {
        match host_error_action(
            startup_reporting_configured,
            startup_completed.load(Ordering::Acquire),
            panic_report::was_reported(),
            error,
        ) {
            HostErrorAction::NotifyInitiator => {
                if let Some(notifier) = startup_notifier.take()
                    && panic_report::claim_startup_failure()
                {
                    let _ = notifier.notify_failure(error);
                }
            }
            HostErrorAction::ShowDialog => dialog::show_error(&error.to_string()),
            HostErrorAction::Suppress => {}
        }
    }
    result
}

fn run_host_session(
    dll_path: Option<PathBuf>,
    startup_notifier: &mut Option<StartupNotifier>,
    startup_completed: Arc<AtomicBool>,
) -> Result<(), HostRunError> {
    let _host_guard = acquire_host_instance()?;
    inject::ensure_host_privileges()?;
    let dll_path = crate::entry::launcher::resolve_host_dll_path(dll_path)?;
    let shutdown = Arc::new(ServerShutdown::new());
    let (ui_handle, ui_commands) = gui::UiHandle::new();
    let controller = Arc::new(HostController::new(
        dll_path,
        Arc::clone(&shutdown),
        Arc::clone(&ui_handle),
    )?);
    let (ui_ready_sender, ui_ready_receiver) = mpsc::channel();
    let (notifier_sender, notifier_receiver) = mpsc::sync_channel::<Option<StartupNotifier>>(1);

    let command_handler: Arc<dyn control::server::ControlHandler> =
        Arc::new(ControlCommandHandler::new(Arc::clone(&controller)));
    let server_shutdown = Arc::clone(&shutdown);
    let server_ui_handle = Arc::clone(&ui_handle);
    let server_startup_completed = Arc::clone(&startup_completed);
    let server_thread = std::thread::Builder::new()
        .name("dwm-lut-control".to_string())
        .spawn(move || {
            let _ui_exit = UiExitOnDrop(Arc::clone(&server_ui_handle));
            let mut server_notifier = notifier_receiver.recv().map_err(|_| {
                HostProcessError::StartupFailed(
                    "control server stopped before receiving startup notifier".to_string(),
                )
            })?;
            let result = match ui_ready_receiver.recv() {
                Ok(()) => {
                    let host_user_sid = UserSid::current_process()?;
                    let pipe_security = SecurityDescriptor::read_write_for_user(&host_user_sid)?;
                    control::server::run_server(
                        command_handler,
                        Arc::clone(&server_shutdown),
                        pipe_security,
                        || {
                            if let Some(notifier) = server_notifier.take() {
                                notifier.notify_success()?;
                                server_startup_completed.store(true, Ordering::Release);
                            }
                            panic_report::complete_startup()?;
                            server_startup_completed.store(true, Ordering::Release);
                            Ok(())
                        },
                    )
                }
                Err(_) => Err(HostRunError::from(HostProcessError::StartupFailed(
                    "GUI event loop failed before initialization".to_string(),
                ))),
            };
            if let Err(error) = &result {
                if server_startup_completed.load(Ordering::Acquire) {
                    let _ = server_ui_handle.send(gui::UiCommand::Exit);
                } else {
                    if let Some(notifier) = server_notifier.take()
                        && panic_report::claim_startup_failure()
                    {
                        let _ = notifier.notify_failure(error);
                    }
                    let _ = server_ui_handle.send(gui::UiCommand::Exit);
                }
            }
            result
        })
        .map_err(|source| {
            HostProcessError::StartupFailed(format!("start control server thread failed: {source}"))
        })?;
    send_startup_notifier(notifier_sender, startup_notifier)?;

    let ui_result = gui::run_host_ui(controller, ui_commands, ui_ready_sender).map_err(Into::into);
    shutdown.request();
    let server_result = match server_thread.join() {
        Ok(result) => result,
        Err(_) => Err(HostRunError::from(HostProcessError::StartupFailed(
            "control server thread panicked".to_string(),
        ))),
    };
    select_host_result(ui_result, server_result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostErrorAction {
    NotifyInitiator,
    ShowDialog,
    Suppress,
}

fn host_error_action(
    startup_reporting_configured: bool,
    startup_completed: bool,
    panic_reported: bool,
    error: &HostRunError,
) -> HostErrorAction {
    if panic_reported
        || matches!(
            error,
            HostRunError::Process(HostProcessError::PanicAlreadyReported)
        )
    {
        HostErrorAction::Suppress
    } else if startup_reporting_configured && !startup_completed {
        HostErrorAction::NotifyInitiator
    } else if matches!(
        error,
        HostRunError::Process(HostProcessError::AlreadyRunning)
    ) {
        HostErrorAction::Suppress
    } else {
        HostErrorAction::ShowDialog
    }
}

fn select_host_result(
    ui_result: Result<(), HostRunError>,
    server_result: Result<(), HostRunError>,
) -> Result<(), HostRunError> {
    match server_result {
        Err(error) => Err(error),
        Ok(()) => ui_result,
    }
}

fn show_background_error(error: &HostRunError) {
    if !matches!(
        error,
        HostRunError::Process(HostProcessError::PanicAlreadyReported)
    ) {
        dialog::show_error(&error.to_string());
    }
}

struct UiExitOnDrop(Arc<gui::UiHandle>);

impl Drop for UiExitOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(gui::UiCommand::Exit);
    }
}

fn send_startup_notifier<T>(
    sender: mpsc::SyncSender<Option<T>>,
    startup_notifier: &mut Option<T>,
) -> Result<(), HostProcessError> {
    if let Err(error) = sender.send(startup_notifier.take()) {
        *startup_notifier = error.0;
        return Err(HostProcessError::StartupFailed(
            "control server stopped before receiving startup notifier".to_string(),
        ));
    }
    Ok(())
}

fn acquire_host_instance() -> Result<HostInstanceGuard, HostRunError> {
    match HostInstanceGuard::claim()? {
        HostInstanceClaim::Acquired(guard) => Ok(guard),
        HostInstanceClaim::Contended(waiter) => wait_for_host_instance_transition(waiter),
    }
}

fn wait_for_host_instance_transition(
    mut waiter: HostInstanceWaiter,
) -> Result<HostInstanceGuard, HostRunError> {
    let deadline = Instant::now() + HOST_INSTANCE_TRANSITION_TIMEOUT;
    loop {
        if let Some(guard) = waiter.wait(0)? {
            return Ok(guard);
        }
        match query_existing_host_status() {
            Ok(ExistingHostStatus::Running) => {
                return Err(HostProcessError::AlreadyRunning.into());
            }
            Ok(ExistingHostStatus::Stopping) => {}
            Err(error) if is_transient_host_state_error(&error) => {}
            Err(error) => return Err(error),
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(HostProcessError::StartupFailed(format!(
                "existing host instance did not become ready or exit within {}ms",
                HOST_INSTANCE_TRANSITION_TIMEOUT.as_millis()
            ))
            .into());
        }
        let wait_ms =
            u32::try_from(remaining.min(HOST_INSTANCE_WAIT_SLICE).as_millis()).unwrap_or(u32::MAX);
        if let Some(guard) = waiter.wait(wait_ms)? {
            return Ok(guard);
        }
    }
}

fn is_transient_host_state_error(error: &HostRunError) -> bool {
    match error {
        HostRunError::Control(control) => control.is_transient_host_probe(),
        _ => false,
    }
}

enum ExistingHostStatus {
    Running,
    Stopping,
}

fn query_existing_host_status() -> Result<ExistingHostStatus, HostRunError> {
    let response = control::client::send_request(&ControlRequest::new(ControlCommand::Status))?;
    if !response.ok {
        return Err(ControlError::Protocol(response.message).into());
    }
    match response.status {
        ControlStatus::Running => Ok(ExistingHostStatus::Running),
        ControlStatus::Stopping => Ok(ExistingHostStatus::Stopping),
        status => Err(ControlError::Protocol(format!("unexpected host status: {status:?}")).into()),
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn host_state_wait_retries_disconnection_but_not_access_denied() {
        let disconnected = HostRunError::Control(ControlError::Io {
            operation: "query existing host",
            source: io::Error::from(io::ErrorKind::BrokenPipe),
        });
        let access_denied = HostRunError::Control(ControlError::Io {
            operation: "query existing host",
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        });

        assert!(is_transient_host_state_error(&disconnected));
        assert!(!is_transient_host_state_error(&access_denied));
    }

    #[test]
    fn notifier_transfer_failure_restores_startup_notifier() {
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        let mut notifier = Some("startup notifier");

        let error = send_startup_notifier(sender, &mut notifier)
            .expect_err("disconnected transfer must fail");

        assert!(notifier.is_some());
        assert!(error.to_string().contains("startup notifier"));
    }

    #[test]
    fn ui_exit_guard_notifies_ui_when_control_thread_ends() {
        let (ui, commands) = gui::UiHandle::new();

        drop(UiExitOnDrop(ui));

        assert!(matches!(commands.recv().unwrap(), gui::UiCommand::Exit));
    }

    #[test]
    fn standalone_startup_errors_are_shown_by_the_host() {
        let error = HostRunError::from(HostProcessError::StartupFailed(
            "GUI startup failed".to_string(),
        ));

        assert_eq!(
            host_error_action(false, false, false, &error),
            HostErrorAction::ShowDialog
        );
    }

    #[test]
    fn launched_startup_errors_are_reported_to_the_initiator() {
        let error = HostRunError::from(HostProcessError::StartupFailed(
            "GUI startup failed".to_string(),
        ));

        assert_eq!(
            host_error_action(true, false, false, &error),
            HostErrorAction::NotifyInitiator
        );
    }

    #[test]
    fn errors_after_successful_startup_are_shown_by_the_host() {
        let error = HostRunError::from(HostProcessError::StartupFailed(
            "GUI runtime failed".to_string(),
        ));

        assert_eq!(
            host_error_action(true, true, false, &error),
            HostErrorAction::ShowDialog
        );
    }

    #[test]
    fn error_reporting_is_suppressed_after_panic_dialog() {
        let error = HostRunError::from(HostProcessError::StartupFailed(
            "control server panicked".to_string(),
        ));

        assert_eq!(
            host_error_action(false, true, true, &error),
            HostErrorAction::Suppress
        );
        assert_eq!(
            host_error_action(
                false,
                false,
                false,
                &HostRunError::from(HostProcessError::PanicAlreadyReported),
            ),
            HostErrorAction::Suppress
        );
    }

    #[test]
    fn already_running_is_not_reported_as_a_background_failure() {
        assert_eq!(
            host_error_action(
                false,
                false,
                false,
                &HostRunError::from(HostProcessError::AlreadyRunning),
            ),
            HostErrorAction::Suppress
        );
        assert_eq!(
            host_error_action(
                true,
                false,
                false,
                &HostRunError::from(HostProcessError::AlreadyRunning),
            ),
            HostErrorAction::NotifyInitiator
        );
    }

    #[test]
    fn control_server_error_takes_precedence_over_ui_error() {
        let result = select_host_result(
            Err(HostRunError::from(HostProcessError::StartupFailed(
                "GUI runtime failed".to_string(),
            ))),
            Err(HostRunError::from(HostProcessError::StartupFailed(
                "control server failed".to_string(),
            ))),
        );

        assert!(matches!(
            result,
            Err(HostRunError::Process(HostProcessError::StartupFailed(message)))
                if message == "control server failed"
        ));
    }
}
