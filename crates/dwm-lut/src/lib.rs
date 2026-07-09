mod backend;
pub mod cli;
mod config;
mod control;
pub mod error;
mod lut;
mod monitor_list;
mod runtime;

use std::path::{Path, PathBuf};

use cli::{CliCommand, ParseArgsResult, parse_args};
use error::InjectorError;

pub fn run_cli() -> Result<(), InjectorError> {
    match parse_args()? {
        ParseArgsResult::Run(CliCommand::Apply(options)) => {
            run_control_command(CliCommand::Apply(options))
        }
        ParseArgsResult::Run(CliCommand::Disable) => run_control_command(CliCommand::Disable),
        ParseArgsResult::Run(CliCommand::Monitors) => monitor_list::run_monitors(),
        ParseArgsResult::Run(CliCommand::Run(options)) => {
            let host_exe = resolve_host_executable_path(options.host_path)?;
            backend::start_background_host(&host_exe, options.dll_path)
        }
        ParseArgsResult::Run(CliCommand::Status) => run_control_command(CliCommand::Status),
        ParseArgsResult::Help(message) => {
            println!("{message}");
            Ok(())
        }
    }
}

pub fn run_host(
    dll_path: Option<PathBuf>,
    startup_result_handle: Option<usize>,
) -> Result<(), InjectorError> {
    let mut startup_notifier = startup_result_handle.map(backend::StartupNotifier::from_raw_handle);
    let result = run_host_inner(dll_path, &mut startup_notifier);
    if let Err(error) = &result
        && let Some(notifier) = startup_notifier.take()
    {
        let _ = notifier.notify_failure(&error.to_string());
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
    backend::ensure_host_privileges()?;
    let dll_path = runtime::resolve_host_dll_path(dll_path)?;
    control::server::run_server(dll_path, || {
        if let Some(notifier) = startup_notifier.take() {
            notifier.notify_success()?;
        }
        Ok(())
    })
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
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::error::InjectorError;

    use super::resolve_host_executable_path;

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
}
