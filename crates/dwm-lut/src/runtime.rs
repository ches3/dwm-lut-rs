use crate::backend::{
    self, ApplyOutcome, ApplyReport, ApplyRequest, DisableOutcome, DisableReport,
};
use crate::control::protocol::{
    CONTROL_PROTOCOL_VERSION, ControlCommand, ControlRequest, ControlResponse,
};
use crate::error::{InjectionStep, InjectorError, ShutdownStatus};
use std::path::PathBuf;

pub(crate) fn handle_command(
    command: ControlCommand,
    host_dll_path: Option<PathBuf>,
) -> ControlResponse {
    match command {
        ControlCommand::Apply {
            config_path,
            profile,
        } => response_from_result(apply(ApplyRequest {
            dll_path: host_dll_path,
            config_path,
            profile,
        })),
        ControlCommand::Disable => response_from_result(disable()),
        ControlCommand::Status => ControlResponse::ok("host instance is running", "running"),
        ControlCommand::Stop => ControlResponse::ok("stopped dwm-lut host instance", "stopped"),
    }
}

pub(crate) fn apply(request: ApplyRequest) -> Result<ControlResponse, InjectorError> {
    backend::apply(request).map(|report| match report.outcome {
        ApplyOutcome::Replaced => ControlResponse::ok(apply_message(&report), "replaced"),
        ApplyOutcome::Initialized => ControlResponse::ok(apply_message(&report), "initialized"),
        ApplyOutcome::Reinitialized => ControlResponse::ok(apply_message(&report), "reinitialized"),
    })
}

pub(crate) fn disable() -> Result<ControlResponse, InjectorError> {
    backend::disable().and_then(|report| match report.outcome {
        DisableOutcome::NotInjected => Ok(ControlResponse::ok(
            disable_message(&report),
            "not_injected",
        )),
        DisableOutcome::ShutDown(ShutdownStatus::Success) => {
            Ok(ControlResponse::ok(disable_message(&report), "disabled"))
        }
        DisableOutcome::ShutDown(ShutdownStatus::NotInitialized) => Ok(ControlResponse::ok(
            disable_message(&report),
            "not_initialized",
        )),
        DisableOutcome::ShutDown(ShutdownStatus::AlreadyShutDown) => Ok(ControlResponse::ok(
            disable_message(&report),
            "already_shutdown",
        )),
        DisableOutcome::ShutDown(status) => Err(InjectorError::HookShutdownFailed(status)),
    })
}

pub(crate) fn response_from_result(
    result: Result<ControlResponse, InjectorError>,
) -> ControlResponse {
    match result {
        Ok(response) => response,
        Err(InjectorError::HostBusy) => {
            ControlResponse::error(InjectorError::HostBusy.to_string(), "busy")
        }
        Err(error) => ControlResponse::error(error.to_string(), "error"),
    }
}

pub(crate) fn request_from_cli(
    command: crate::cli::CliCommand,
) -> Result<Option<ControlRequest>, InjectorError> {
    let Some(command) = (match command {
        crate::cli::CliCommand::Apply(options) => Some(ControlCommand::Apply {
            config_path: resolve_apply_config_path(options.config_path)?,
            profile: options.profile,
        }),
        crate::cli::CliCommand::Disable => Some(ControlCommand::Disable),
        crate::cli::CliCommand::HostStop => Some(ControlCommand::Stop),
        crate::cli::CliCommand::Status => Some(ControlCommand::Status),
        crate::cli::CliCommand::HostStart(_)
        | crate::cli::CliCommand::Install
        | crate::cli::CliCommand::Monitors
        | crate::cli::CliCommand::Uninstall => None,
    }) else {
        return Ok(None);
    };
    Ok(Some(ControlRequest {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        command,
    }))
}

fn resolve_apply_config_path(config_path: Option<PathBuf>) -> Result<PathBuf, InjectorError> {
    if let Some(config_path) = config_path {
        return absolute_cli_path(config_path);
    }

    default_config_path()
}

fn default_config_path() -> Result<PathBuf, InjectorError> {
    Ok(
        crate::paths::program_data_directory(InjectionStep::ResolveConfigPath)?
            .join("dwm-lut-rs")
            .join("config.json"),
    )
}

pub(crate) fn resolve_host_dll_path(
    dll_path: Option<PathBuf>,
) -> Result<Option<PathBuf>, InjectorError> {
    let Some(dll_path) = dll_path else {
        return Ok(None);
    };

    let dll_path = absolute_cli_path(dll_path)?;
    if !dll_path.is_file() {
        return Err(InjectorError::MissingFile {
            kind: "hook DLL",
            path: dll_path,
        });
    }

    dll_path
        .canonicalize()
        .map(Some)
        .map_err(|source| InjectorError::StepFailed {
            step: InjectionStep::ResolveLocalHookDll,
            source,
        })
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::cli::{ApplyOptions, CliCommand};
    use crate::control::protocol::ControlCommand;

    use super::*;

    fn apply_report(outcome: ApplyOutcome) -> ApplyReport {
        ApplyReport {
            outcome,
            pid: 4242,
            input_dll_path: PathBuf::from(r"C:\tools\dwm_lut_hook.dll"),
            staged_dll_path: PathBuf::from(
                r"C:\ProgramData\dwm-lut-rs\hook\dwm_lut_hook-11111111111111111111111111111111.dll",
            ),
            config_path: PathBuf::from(r"C:\profiles\config.json"),
            profile_name: "desktop".to_string(),
        }
    }

    #[test]
    fn apply_report_message_describes_replace_outcome() {
        let message = apply_message(&apply_report(ApplyOutcome::Replaced));

        assert_eq!(
            message,
            r"replaced assignments in dwm.exe (pid=4242) from C:\profiles\config.json (profile=desktop)"
        );
    }

    #[test]
    fn disable_report_message_describes_not_initialized() {
        let message = disable_message(&DisableReport {
            outcome: DisableOutcome::ShutDown(ShutdownStatus::NotInitialized),
            pid: 4242,
        });

        assert_eq!(
            message,
            "disable skipped: hook DLL is loaded but not initialized in dwm.exe (pid=4242)"
        );
    }

    #[test]
    fn response_from_error_keeps_display_message() {
        let response = response_from_result(Err(InjectorError::HostUnavailable));

        assert!(!response.ok);
        assert_eq!(response.protocol_version, CONTROL_PROTOCOL_VERSION);
        assert_eq!(response.status, "error");
        assert!(response.message.contains("host instance is not running"));
    }

    #[test]
    fn response_from_host_busy_uses_busy_status() {
        let response = response_from_result(Err(InjectorError::HostBusy));

        assert!(!response.ok);
        assert_eq!(response.protocol_version, CONTROL_PROTOCOL_VERSION);
        assert_eq!(response.status, "busy");
        assert!(response.message.contains("host instance is busy"));
    }

    #[test]
    fn request_from_cli_attaches_protocol_version() {
        let request = request_from_cli(CliCommand::Status)
            .expect("request should resolve")
            .expect("status should map to control request");

        assert_eq!(request.command, ControlCommand::Status);
        assert_eq!(request.protocol_version, CONTROL_PROTOCOL_VERSION);
    }

    #[test]
    fn request_from_host_stop_cli_maps_to_stop_command() {
        let request = request_from_cli(CliCommand::HostStop)
            .expect("request should resolve")
            .expect("host stop should map to control request");

        assert_eq!(request.command, ControlCommand::Stop);
        assert_eq!(request.protocol_version, CONTROL_PROTOCOL_VERSION);
    }

    #[test]
    fn request_from_cli_resolves_relative_config_path_before_ipc() {
        let request = request_from_cli(CliCommand::Apply(ApplyOptions {
            config_path: Some(PathBuf::from("config.json")),
            profile: None,
        }))
        .expect("request should resolve")
        .expect("apply should map to control request");

        match request.command {
            ControlCommand::Apply { config_path, .. } => {
                assert!(config_path.is_absolute());
                assert_eq!(
                    config_path,
                    std::env::current_dir().unwrap().join("config.json")
                );
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn request_from_cli_uses_default_config() {
        let request = request_from_cli(CliCommand::Apply(ApplyOptions {
            config_path: None,
            profile: None,
        }))
        .expect("apply should resolve the default config")
        .expect("apply should map to control request");

        match request.command {
            ControlCommand::Apply { config_path, .. } => {
                assert_eq!(config_path, default_config_path().unwrap());
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn resolve_host_dll_path_canonicalizes_existing_relative_path() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let relative_path =
            PathBuf::from("target").join(format!("dwm-lut-run-dll-test-{unique}.dll"));
        fs::create_dir_all("target").expect("target directory should be available");
        fs::write(&relative_path, b"hook").expect("test hook DLL should be written");

        let resolved = resolve_host_dll_path(Some(relative_path.clone()))
            .expect("existing hook DLL should resolve")
            .expect("explicit hook DLL should remain configured");

        assert_eq!(resolved, relative_path.canonicalize().unwrap());

        let _ = fs::remove_file(relative_path);
    }

    #[test]
    fn resolve_host_dll_path_rejects_missing_file_at_startup() {
        let error = resolve_host_dll_path(Some(PathBuf::from(r"C:\missing\hook.dll")))
            .expect_err("missing hook DLL must be rejected");

        match error {
            InjectorError::MissingFile { kind, path } => {
                assert_eq!(kind, "hook DLL");
                assert_eq!(path, PathBuf::from(r"C:\missing\hook.dll"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
