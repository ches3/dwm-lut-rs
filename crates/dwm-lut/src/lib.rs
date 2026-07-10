mod backend;
pub mod cli;
mod config;
mod control;
pub mod error;
mod lut;
mod monitor_list;
mod runtime;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cli::{CliCommand, ParseArgsResult, parse_args};
use control::server::{HostInstanceClaim, HostInstanceGuard, HostInstanceWaiter};
use error::InjectorError;

const HOST_INSTANCE_TRANSITION_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_INSTANCE_WAIT_SLICE: Duration = Duration::from_millis(100);

pub fn run_cli() -> Result<(), InjectorError> {
    match parse_args()? {
        ParseArgsResult::Command(CliCommand::Apply(options)) => {
            run_control_command(CliCommand::Apply(options))
        }
        ParseArgsResult::Command(CliCommand::Disable) => run_control_command(CliCommand::Disable),
        ParseArgsResult::Command(CliCommand::HostStart(options)) => {
            let host_exe = resolve_host_executable_path(options.host_path)?;
            let message =
                host_start_message(backend::start_background_host(&host_exe, options.dll_path))?;
            println!("{message}");
            Ok(())
        }
        ParseArgsResult::Command(CliCommand::HostStop) => run_control_command(CliCommand::HostStop),
        ParseArgsResult::Command(CliCommand::Monitors) => monitor_list::run_monitors(),
        ParseArgsResult::Command(CliCommand::Status) => run_control_command(CliCommand::Status),
        ParseArgsResult::Help(message) => {
            println!("{message}");
            Ok(())
        }
    }
}

pub fn run_host(
    dll_path: Option<PathBuf>,
    startup_result_pipe: Option<String>,
) -> Result<(), InjectorError> {
    let mut startup_notifier = startup_result_pipe.map(backend::StartupNotifier::new);
    let result = run_host_inner(dll_path, &mut startup_notifier);
    if let Err(error) = &result
        && let Some(notifier) = startup_notifier.take()
    {
        let _ = notifier.notify_failure(error);
    }
    result
}

pub fn report_host_startup_error(error: &InjectorError) -> i32 {
    eprintln!("{error}");
    1
}

fn run_host_inner(
    dll_path: Option<PathBuf>,
    startup_notifier: &mut Option<backend::StartupNotifier>,
) -> Result<(), InjectorError> {
    let host_guard = acquire_host_instance()?;
    backend::ensure_host_privileges()?;
    let dll_path = runtime::resolve_host_dll_path(dll_path)?;
    control::server::run_server(host_guard, dll_path, || {
        if let Some(notifier) = startup_notifier.take() {
            notifier.notify_success()?;
        }
        Ok(())
    })
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
        let wait = remaining.min(HOST_INSTANCE_WAIT_SLICE);
        let wait_ms = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    match response.status.as_str() {
        "running" => Ok(ExistingHostState::Running),
        "stopping" => Ok(ExistingHostState::Stopping),
        status => Err(InjectorError::ControlProtocol(format!(
            "unknown host status: {status}"
        ))),
    }
}

fn run_control_command(command: CliCommand) -> Result<(), InjectorError> {
    let request = runtime::request_from_cli(command)?.expect("command must map to control request");
    let response = control::client::send_request(&request)?;
    if !response.ok {
        eprintln!("{}", response.message);
        std::process::exit(1);
    }

    println!("{}", response.message);
    Ok(())
}

fn host_start_message(result: Result<(), InjectorError>) -> Result<&'static str, InjectorError> {
    match result {
        Ok(()) => Ok("started dwm-lut host instance"),
        Err(InjectorError::HostAlreadyRunning) => Ok("dwm-lut host instance is already running"),
        Err(error) => Err(error),
    }
}

fn default_host_executable_path() -> Result<PathBuf, InjectorError> {
    let cli_path = std::env::current_exe().map_err(|source| InjectorError::HostLaunchFailed {
        operation: "resolve CLI executable",
        source,
    })?;
    let directory = cli_path.parent().ok_or_else(|| {
        InjectorError::HostStartupFailed("CLI executable has no parent directory".to_string())
    })?;
    Ok(directory.join(host_executable_name()))
}

fn host_executable_name() -> &'static Path {
    Path::new("dwm-lut.exe")
}

fn resolve_host_executable_path(host_path: Option<PathBuf>) -> Result<PathBuf, InjectorError> {
    let host_path = match host_path {
        Some(path) => absolute_cli_path(path)?,
        None => default_host_executable_path()?,
    };
    if !host_path.is_file() {
        return Err(InjectorError::MissingFile {
            kind: "host executable",
            path: host_path,
        });
    }

    Ok(host_path)
}

fn absolute_cli_path(path: PathBuf) -> Result<PathBuf, InjectorError> {
    if path.is_absolute() {
        return Ok(path);
    }

    let cwd = std::env::current_dir().map_err(|source| InjectorError::ControlPipe {
        operation: "resolve current directory",
        source,
    })?;
    Ok(cwd.join(path))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::error::InjectorError;

    use super::{host_start_message, is_transient_host_state_error, resolve_host_executable_path};

    #[test]
    fn resolve_host_executable_path_uses_current_directory_for_relative_path() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let relative_path = PathBuf::from("target").join(format!("dwm-lut-host-test-{unique}.exe"));
        fs::create_dir_all("target").expect("target directory should be available");
        fs::write(&relative_path, b"host").expect("test host executable should be written");

        let resolved = resolve_host_executable_path(Some(relative_path.clone()))
            .expect("existing host executable should resolve");

        assert_eq!(
            resolved,
            std::env::current_dir().unwrap().join(&relative_path)
        );

        let _ = fs::remove_file(relative_path);
    }

    #[test]
    fn resolve_host_executable_path_rejects_missing_file() {
        let path = PathBuf::from(r"C:\missing\dwm-lut.exe");
        let error = resolve_host_executable_path(Some(path.clone()))
            .expect_err("missing host executable must be rejected");

        match error {
            InjectorError::MissingFile { kind, path: actual } => {
                assert_eq!(kind, "host executable");
                assert_eq!(actual, path);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn host_start_message_reports_started() {
        let message = host_start_message(Ok(())).expect("successful start should report message");
        assert_eq!(message, "started dwm-lut host instance");
    }

    #[test]
    fn host_start_message_treats_already_running_as_success() {
        let message = host_start_message(Err(InjectorError::HostAlreadyRunning))
            .expect("already running should be a successful start result");
        assert_eq!(message, "dwm-lut host instance is already running");
    }

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
}
