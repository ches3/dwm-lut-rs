use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::app_args::BackgroundOptions;
use crate::backend;
use crate::cli::CliCommand;
use crate::control;
use crate::control::protocol::ControlStatus;
use crate::control::server::ServerShutdown;
use crate::elevation;
use crate::error::InjectorError;
use crate::gui;
use crate::host::launch::StartupNotifier;
use crate::host::{
    HostApplication, HostController, HostInstanceClaim, HostInstanceGuard, HostInstanceWaiter,
};
use crate::panic_report;
use crate::{native_dialog, runtime};

const HOST_INSTANCE_TRANSITION_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_INSTANCE_WAIT_SLICE: Duration = Duration::from_millis(100);

pub fn run_background(options: BackgroundOptions) -> Result<(), InjectorError> {
    if options.startup_result_pipe.is_none() {
        let elevated =
            elevation::is_process_elevated().map_err(|source| InjectorError::HostLaunchFailed {
                operation: "check process elevation",
                source,
            });
        let elevated = match elevated {
            Ok(elevated) => elevated,
            Err(error) => {
                show_background_error(&error);
                return Err(error);
            }
        };
        if !elevated {
            let executable =
                std::env::current_exe().map_err(|source| InjectorError::HostLaunchFailed {
                    operation: "resolve host executable",
                    source,
                });
            let executable = match executable {
                Ok(executable) => executable,
                Err(error) => {
                    show_background_error(&error);
                    return Err(error);
                }
            };
            return match crate::host::launch::start_background_host(&executable, options.dll_path) {
                Ok(()) | Err(InjectorError::HostAlreadyRunning) => Ok(()),
                Err(error) => {
                    show_background_error(&error);
                    Err(error)
                }
            };
        }
    }
    match run_host(options) {
        Ok(()) | Err(InjectorError::HostAlreadyRunning) => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn run_host(options: BackgroundOptions) -> Result<(), InjectorError> {
    let BackgroundOptions {
        dll_path,
        startup_result_pipe,
        panic_report_event,
        startup_abort_event,
    } = options;
    let startup_reporting_configured = startup_result_pipe.is_some();
    let mut startup_notifier = startup_result_pipe.map(StartupNotifier::new);
    let startup_completed = Arc::new(AtomicBool::new(false));
    let result = panic_report::configure(
        panic_report_event.as_deref(),
        startup_abort_event.as_deref(),
    )
    .and_then(|()| {
        run_host_inner(
            dll_path,
            &mut startup_notifier,
            Arc::clone(&startup_completed),
        )
    });
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
            HostErrorAction::ShowDialog => native_dialog::show_error(&error.to_string()),
            HostErrorAction::Suppress => {}
        }
    }
    result
}

fn run_host_inner(
    dll_path: Option<PathBuf>,
    startup_notifier: &mut Option<StartupNotifier>,
    startup_completed: Arc<AtomicBool>,
) -> Result<(), InjectorError> {
    let _host_guard = acquire_host_instance()?;
    backend::ensure_host_privileges()?;
    let dll_path = runtime::resolve_host_dll_path(dll_path)?;
    let controller = Arc::new(HostController::new(dll_path));
    let shutdown = Arc::new(ServerShutdown::new()?);
    let (ui_handle, ui_commands) = gui::UiHandle::new();
    let application = Arc::new(HostApplication::new(
        controller,
        Arc::clone(&shutdown),
        Arc::clone(&ui_handle),
    ));
    let (ui_ready_sender, ui_ready_receiver) = mpsc::channel();
    let (notifier_sender, notifier_receiver) = mpsc::sync_channel::<Option<StartupNotifier>>(1);

    let server_handler: Arc<dyn control::server::ControlHandler> = application.clone();
    let server_shutdown = Arc::clone(&shutdown);
    let server_ui_handle = Arc::clone(&ui_handle);
    let server_startup_completed = Arc::clone(&startup_completed);
    let server_thread = std::thread::Builder::new()
        .name("dwm-lut-control".to_string())
        .spawn(move || {
            let _ui_exit = UiExitOnDrop(Arc::clone(&server_ui_handle));
            let mut server_notifier = notifier_receiver.recv().map_err(|_| {
                InjectorError::HostStartupFailed(
                    "control server stopped before receiving startup notifier".to_string(),
                )
            })?;
            let result = match ui_ready_receiver.recv() {
                Ok(()) => control::server::run_server(
                    server_handler,
                    Arc::clone(&server_shutdown),
                    || {
                        if let Some(notifier) = server_notifier.take() {
                            notifier.notify_success()?;
                            server_startup_completed.store(true, Ordering::Release);
                        }
                        panic_report::complete_startup()?;
                        server_startup_completed.store(true, Ordering::Release);
                        Ok(())
                    },
                ),
                Err(_) => Err(InjectorError::HostStartupFailed(
                    "GUI event loop failed before initialization".to_string(),
                )),
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
        .map_err(|source| InjectorError::ControlPipe {
            operation: "start control server thread",
            source,
        })?;
    send_startup_notifier(notifier_sender, startup_notifier)?;

    let ui_result = gui::run_host_ui(application, ui_handle, ui_commands, ui_ready_sender);
    let _ = shutdown.request();
    let server_result = match server_thread.join() {
        Ok(result) => result,
        Err(_) => Err(InjectorError::HostStartupFailed(
            "control server thread panicked".to_string(),
        )),
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
    error: &InjectorError,
) -> HostErrorAction {
    if panic_reported || matches!(error, InjectorError::HostPanicAlreadyReported) {
        HostErrorAction::Suppress
    } else if startup_reporting_configured && !startup_completed {
        HostErrorAction::NotifyInitiator
    } else if matches!(error, InjectorError::HostAlreadyRunning) {
        HostErrorAction::Suppress
    } else {
        HostErrorAction::ShowDialog
    }
}

fn select_host_result(
    ui_result: Result<(), InjectorError>,
    server_result: Result<(), InjectorError>,
) -> Result<(), InjectorError> {
    match server_result {
        Err(error) => Err(error),
        Ok(()) => ui_result,
    }
}

fn show_background_error(error: &InjectorError) {
    if !matches!(error, InjectorError::HostPanicAlreadyReported) {
        native_dialog::show_error(&error.to_string());
    }
}

struct UiExitOnDrop(Arc<gui::UiHandle>);

impl Drop for UiExitOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(gui::UiCommand::Exit);
    }
}

fn send_startup_notifier(
    sender: mpsc::SyncSender<Option<StartupNotifier>>,
    startup_notifier: &mut Option<StartupNotifier>,
) -> Result<(), InjectorError> {
    if let Err(error) = sender.send(startup_notifier.take()) {
        *startup_notifier = error.0;
        return Err(InjectorError::HostStartupFailed(
            "control server stopped before receiving startup notifier".to_string(),
        ));
    }
    Ok(())
}

fn acquire_host_instance() -> Result<HostInstanceGuard, InjectorError> {
    match HostInstanceGuard::claim()? {
        HostInstanceClaim::Acquired(guard) => Ok(guard),
        HostInstanceClaim::Contended(waiter) => wait_for_host_instance_transition(waiter),
    }
}

fn wait_for_host_instance_transition(
    mut waiter: HostInstanceWaiter,
) -> Result<HostInstanceGuard, InjectorError> {
    let deadline = Instant::now() + HOST_INSTANCE_TRANSITION_TIMEOUT;
    loop {
        if let Some(guard) = waiter.wait(0)? {
            return Ok(guard);
        }
        match existing_host_state() {
            Ok(ExistingHostState::Running) => return Err(InjectorError::HostAlreadyRunning),
            Ok(ExistingHostState::Stopping) => {}
            Err(error) if is_transient_host_state_error(&error) => {}
            Err(error) => return Err(error),
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(InjectorError::HostStartupFailed(format!(
                "existing host instance did not become ready or exit within {}ms",
                HOST_INSTANCE_TRANSITION_TIMEOUT.as_millis()
            )));
        }
        let wait_ms =
            u32::try_from(remaining.min(HOST_INSTANCE_WAIT_SLICE).as_millis()).unwrap_or(u32::MAX);
        if let Some(guard) = waiter.wait(wait_ms)? {
            return Ok(guard);
        }
    }
}

fn is_transient_host_state_error(error: &InjectorError) -> bool {
    match error {
        InjectorError::HostUnavailable
        | InjectorError::HostBusy
        | InjectorError::ControlTimeout { .. } => true,
        InjectorError::ControlPipe { source, .. } => matches!(
            source.kind(),
            std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::UnexpectedEof
        ),
        _ => false,
    }
}

enum ExistingHostState {
    Running,
    Stopping,
}

fn existing_host_state() -> Result<ExistingHostState, InjectorError> {
    let request = runtime::request_from_cli(CliCommand::Status)?
        .expect("status command must map to a control request");
    let response = control::client::send_request(&request)?;
    if !response.ok {
        return Err(InjectorError::ControlProtocol(response.message));
    }
    match response.status {
        ControlStatus::Running => Ok(ExistingHostState::Running),
        ControlStatus::Stopping => Ok(ExistingHostState::Stopping),
        status => Err(InjectorError::ControlProtocol(format!(
            "unexpected host status: {status:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn host_state_wait_retries_disconnection_but_not_access_denied() {
        let disconnected = InjectorError::ControlPipe {
            operation: "query existing host",
            source: io::Error::from(io::ErrorKind::BrokenPipe),
        };
        let access_denied = InjectorError::ControlPipe {
            operation: "query existing host",
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        };

        assert!(is_transient_host_state_error(&disconnected));
        assert!(!is_transient_host_state_error(&access_denied));
    }

    #[test]
    fn notifier_transfer_failure_restores_startup_notifier() {
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        let mut notifier = Some(StartupNotifier::new(
            r"\\.\pipe\dwm-lut-test-startup".to_string(),
        ));

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
        let error = InjectorError::HostStartupFailed("GUI startup failed".to_string());

        assert_eq!(
            host_error_action(false, false, false, &error),
            HostErrorAction::ShowDialog
        );
    }

    #[test]
    fn launched_startup_errors_are_reported_to_the_initiator() {
        let error = InjectorError::HostStartupFailed("GUI startup failed".to_string());

        assert_eq!(
            host_error_action(true, false, false, &error),
            HostErrorAction::NotifyInitiator
        );
    }

    #[test]
    fn errors_after_successful_startup_are_shown_by_the_host() {
        let error = InjectorError::HostStartupFailed("GUI runtime failed".to_string());

        assert_eq!(
            host_error_action(true, true, false, &error),
            HostErrorAction::ShowDialog
        );
    }

    #[test]
    fn error_reporting_is_suppressed_after_panic_dialog() {
        let error = InjectorError::HostStartupFailed("control server panicked".to_string());

        assert_eq!(
            host_error_action(false, true, true, &error),
            HostErrorAction::Suppress
        );
        assert_eq!(
            host_error_action(
                false,
                false,
                false,
                &InjectorError::HostPanicAlreadyReported,
            ),
            HostErrorAction::Suppress
        );
    }

    #[test]
    fn already_running_is_not_reported_as_a_background_failure() {
        assert_eq!(
            host_error_action(false, false, false, &InjectorError::HostAlreadyRunning,),
            HostErrorAction::Suppress
        );
        assert_eq!(
            host_error_action(true, false, false, &InjectorError::HostAlreadyRunning,),
            HostErrorAction::NotifyInitiator
        );
    }

    #[test]
    fn control_server_error_takes_precedence_over_ui_error() {
        let result = select_host_result(
            Err(InjectorError::HostStartupFailed(
                "GUI runtime failed".to_string(),
            )),
            Err(InjectorError::HostStartupFailed(
                "control server failed".to_string(),
            )),
        );

        assert!(matches!(
            result,
            Err(InjectorError::HostStartupFailed(message)) if message == "control server failed"
        ));
    }
}
