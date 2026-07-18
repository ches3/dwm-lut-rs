use std::path::PathBuf;

use crate::control::protocol::{CONTROL_PROTOCOL_VERSION, ControlCommand, ControlRequest};
use crate::error::{InjectionStep, InjectorError};

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
    match config_path {
        Some(config_path) => absolute_cli_path(config_path),
        None => crate::paths::default_config_path(),
    }
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::cli::{ApplyOptions, CliCommand};
    use crate::control::protocol::ControlCommand;

    use super::*;

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
                assert_eq!(config_path, crate::paths::default_config_path().unwrap());
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

        assert!(matches!(
            error,
            InjectorError::MissingFile {
                kind: "hook DLL",
                ..
            }
        ));
    }
}
