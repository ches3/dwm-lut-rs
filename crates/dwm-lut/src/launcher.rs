use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::control::client;
use crate::control::protocol::{
    CONTROL_PROTOCOL_VERSION, ControlCommand, ControlRequest, ControlStatus,
};
use crate::error::InjectorError;
use crate::host::launch;

const HOST_TRANSITION_TIMEOUT: Duration = Duration::from_secs(5);
const HOST_WAIT_SLICE: Duration = Duration::from_millis(100);

pub fn run_app_launcher() -> Result<(), InjectorError> {
    let host_executable = default_host_executable_path()?;
    let mut stopping_deadline = None;
    loop {
        match show_host_gui() {
            Ok(ShowGuiOutcome::Shown) => return Ok(()),
            Ok(ShowGuiOutcome::Stopping) => {
                let deadline = stopping_deadline
                    .get_or_insert_with(|| Instant::now() + HOST_TRANSITION_TIMEOUT);
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(InjectorError::HostStartupFailed(format!(
                        "existing host instance did not stop within {}ms",
                        HOST_TRANSITION_TIMEOUT.as_millis()
                    )));
                }
                std::thread::sleep(remaining.min(HOST_WAIT_SLICE));
            }
            Err(InjectorError::HostUnavailable) => {
                stopping_deadline = None;
                match launch::start_background_host(&host_executable, None) {
                    Ok(()) | Err(InjectorError::HostAlreadyRunning) => {}
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShowGuiOutcome {
    Shown,
    Stopping,
}

fn show_host_gui() -> Result<ShowGuiOutcome, InjectorError> {
    let response = client::send_request(&ControlRequest {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        command: ControlCommand::ShowGui,
    })?;
    if response.ok {
        Ok(ShowGuiOutcome::Shown)
    } else if response.status == ControlStatus::Stopping {
        Ok(ShowGuiOutcome::Stopping)
    } else {
        Err(InjectorError::ControlProtocol(response.message))
    }
}

pub(crate) fn default_host_executable_path() -> Result<PathBuf, InjectorError> {
    let executable = std::env::current_exe().map_err(|source| InjectorError::HostLaunchFailed {
        operation: "resolve application executable",
        source,
    })?;
    let directory = executable.parent().ok_or_else(|| {
        InjectorError::HostStartupFailed(
            "application executable has no parent directory".to_string(),
        )
    })?;
    Ok(directory.join(host_executable_name()))
}

pub(crate) fn resolve_host_executable_path(
    host_path: Option<PathBuf>,
) -> Result<PathBuf, InjectorError> {
    let host_path = match host_path {
        Some(path) => absolute_path(path)?,
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

fn host_executable_name() -> &'static Path {
    Path::new("dwm-lut.exe")
}

fn absolute_path(path: PathBuf) -> Result<PathBuf, InjectorError> {
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

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
