use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use crate::error::InjectorError;

#[derive(Debug, PartialEq, Eq)]
pub struct ApplyOptions {
    pub config_path: PathBuf,
    pub profile: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct HostStartOptions {
    pub host_path: Option<PathBuf>,
    pub dll_path: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CliCommand {
    Apply(ApplyOptions),
    Disable,
    HostStart(HostStartOptions),
    HostStop,
    Monitors,
    Status,
}

#[derive(Debug)]
pub enum ParseArgsResult {
    Command(CliCommand),
    Help(String),
}

pub fn parse_args() -> Result<ParseArgsResult, InjectorError> {
    parse_args_from(env::args_os())
}

fn parse_args_from<I, T>(args: I) -> Result<ParseArgsResult, InjectorError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next();

    let first = args.next();
    let Some(first) = first else {
        return Err(InjectorError::Usage(usage_message("missing command")));
    };
    let first_string = first.to_string_lossy();
    if first_string == "--help" || first_string == "-h" {
        return Ok(ParseArgsResult::Help(usage_message("")));
    }
    match first_string.as_ref() {
        "apply" => parse_apply_args(args),
        "disable" => parse_disable_args(args),
        "host" => parse_host_args(args),
        "monitors" => parse_monitors_args(args),
        "status" => parse_no_arg_command(args, "status", CliCommand::Status),
        other => Err(InjectorError::Usage(usage_message(&format!(
            "unknown command: {other}"
        )))),
    }
}

fn parse_apply_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<ParseArgsResult, InjectorError> {
    let mut config_path = None;
    let mut profile = None;
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--dll" => {
                return Err(InjectorError::Usage(usage_message(
                    "apply --dll is not supported through the host instance; start the host with `dwm-lut-cli host start --dll <hook-dll-path>`",
                )));
            }
            "--config" => {
                let value = args.next().ok_or_else(|| {
                    InjectorError::Usage(usage_message("--config requires a value"))
                })?;
                config_path = Some(PathBuf::from(value));
            }
            "--profile" => {
                let value = args.next().ok_or_else(|| {
                    InjectorError::Usage(usage_message("--profile requires a value"))
                })?;
                let value = value.to_string_lossy();
                if value.trim().is_empty() {
                    return Err(InjectorError::Usage(usage_message(
                        "--profile must not be empty",
                    )));
                }
                profile = Some(value.trim().to_owned());
            }
            "--help" | "-h" => {
                return Ok(ParseArgsResult::Help(usage_message("")));
            }
            other => {
                return Err(InjectorError::Usage(usage_message(&format!(
                    "unknown argument: {other}"
                ))));
            }
        }
    }

    let config_path =
        config_path.ok_or_else(|| InjectorError::Usage(usage_message("missing --config")))?;

    Ok(ParseArgsResult::Command(CliCommand::Apply(ApplyOptions {
        config_path,
        profile,
    })))
}

fn parse_host_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<ParseArgsResult, InjectorError> {
    let Some(subcommand) = args.next() else {
        return Err(InjectorError::Usage(usage_message("missing host command")));
    };

    match subcommand.to_string_lossy().as_ref() {
        "start" => parse_host_start_args(args),
        "stop" => parse_no_arg_command(args, "host stop", CliCommand::HostStop),
        "--help" | "-h" => Ok(ParseArgsResult::Help(usage_message(""))),
        other => Err(InjectorError::Usage(usage_message(&format!(
            "unknown host command: {other}"
        )))),
    }
}

fn parse_host_start_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<ParseArgsResult, InjectorError> {
    let mut host_path = None;
    let mut dll_path = None;
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--host" => {
                let value = args.next().ok_or_else(|| {
                    InjectorError::Usage(usage_message("--host requires a value"))
                })?;
                host_path = Some(PathBuf::from(value));
            }
            "--dll" => {
                let value = args
                    .next()
                    .ok_or_else(|| InjectorError::Usage(usage_message("--dll requires a value")))?;
                dll_path = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                return Ok(ParseArgsResult::Help(usage_message("")));
            }
            other => {
                return Err(InjectorError::Usage(usage_message(&format!(
                    "unknown argument for host start: {other}"
                ))));
            }
        }
    }

    Ok(ParseArgsResult::Command(CliCommand::HostStart(
        HostStartOptions {
            host_path,
            dll_path,
        },
    )))
}

fn parse_disable_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<ParseArgsResult, InjectorError> {
    if let Some(arg) = args.next() {
        return Err(InjectorError::Usage(usage_message(&format!(
            "unknown argument for disable: {}",
            arg.to_string_lossy()
        ))));
    }

    Ok(ParseArgsResult::Command(CliCommand::Disable))
}

fn parse_monitors_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<ParseArgsResult, InjectorError> {
    if let Some(arg) = args.next() {
        return Err(InjectorError::Usage(usage_message(&format!(
            "unknown argument for monitors: {}",
            arg.to_string_lossy()
        ))));
    }

    Ok(ParseArgsResult::Command(CliCommand::Monitors))
}

fn parse_no_arg_command(
    mut args: impl Iterator<Item = OsString>,
    command: &str,
    parsed: CliCommand,
) -> Result<ParseArgsResult, InjectorError> {
    if let Some(arg) = args.next() {
        return Err(InjectorError::Usage(usage_message(&format!(
            "unknown argument for {command}: {}",
            arg.to_string_lossy()
        ))));
    }

    Ok(ParseArgsResult::Command(parsed))
}

fn usage_message(problem: &str) -> String {
    let usage = "usage: dwm-lut-cli apply --config <config-path> [--profile <profile-name>]\n       dwm-lut-cli disable\n       dwm-lut-cli status\n       dwm-lut-cli monitors\n       dwm-lut-cli host start [--host <host-exe-path>] [--dll <hook-dll-path>]\n       dwm-lut-cli host stop";
    if problem.is_empty() {
        usage.to_string()
    } else {
        format!("{problem}\n{usage}")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::error::InjectorError;

    use super::{ApplyOptions, CliCommand, HostStartOptions, ParseArgsResult, parse_args_from};

    #[test]
    fn reports_help_without_treating_it_as_invalid_usage() {
        let parsed = parse_args_from(["dwm-lut-cli", "--help"]).expect("help should parse");

        match parsed {
            ParseArgsResult::Help(message) => {
                assert!(message.starts_with("usage: dwm-lut-cli"));
            }
            ParseArgsResult::Command(_) => panic!("help must not continue to normal execution"),
        }
    }

    #[test]
    fn requires_config_path() {
        let error =
            parse_args_from(["dwm-lut-cli", "apply"]).expect_err("missing config must be rejected");

        match error {
            InjectorError::Usage(message) => {
                assert!(message.contains("missing --config"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rejects_config_without_command() {
        let error = parse_args_from(["dwm-lut-cli", "--config", "config.json"])
            .expect_err("config without command must be rejected");

        match error {
            InjectorError::Usage(message) => {
                assert!(message.contains("unknown command: --config"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn accepts_explicit_apply_command() {
        let parsed = parse_args_from(["dwm-lut-cli", "apply", "--config", "config.json"])
            .expect("explicit apply command should parse");

        assert_eq!(
            apply_options(parsed),
            ApplyOptions {
                config_path: PathBuf::from("config.json"),
                profile: None,
            }
        );
    }

    #[test]
    fn accepts_profile_argument() {
        let parsed = parse_args_from([
            "dwm-lut-cli",
            "apply",
            "--config",
            "config.json",
            "--profile",
            "gaming",
        ])
        .expect("profile argument should parse");

        assert_eq!(
            apply_options(parsed),
            ApplyOptions {
                config_path: PathBuf::from("config.json"),
                profile: Some("gaming".to_string()),
            }
        );
    }

    #[test]
    fn accepts_profile_argument_with_mixed_case() {
        let parsed = parse_args_from([
            "dwm-lut-cli",
            "apply",
            "--config",
            "config.json",
            "--profile",
            "GAMING",
        ])
        .expect("mixed-case profile argument should parse");

        assert_eq!(
            apply_options(parsed),
            ApplyOptions {
                config_path: PathBuf::from("config.json"),
                profile: Some("GAMING".to_string()),
            }
        );
    }

    #[test]
    fn accepts_profile_argument_with_surrounding_whitespace() {
        let parsed = parse_args_from([
            "dwm-lut-cli",
            "apply",
            "--config",
            "config.json",
            "--profile",
            "  gaming  ",
        ])
        .expect("profile argument should parse");

        assert_eq!(
            apply_options(parsed),
            ApplyOptions {
                config_path: PathBuf::from("config.json"),
                profile: Some("gaming".to_string()),
            }
        );
    }

    #[test]
    fn rejects_empty_profile_argument() {
        let error = parse_args_from([
            "dwm-lut-cli",
            "apply",
            "--config",
            "config.json",
            "--profile",
            "",
        ])
        .expect_err("empty profile must be rejected");

        match error {
            InjectorError::Usage(message) => {
                assert!(message.contains("--profile must not be empty"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rejects_whitespace_profile_argument() {
        let error = parse_args_from([
            "dwm-lut-cli",
            "apply",
            "--config",
            "config.json",
            "--profile",
            "   ",
        ])
        .expect_err("whitespace profile must be rejected");

        match error {
            InjectorError::Usage(message) => {
                assert!(message.contains("--profile must not be empty"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rejects_profile_without_value() {
        let error = parse_args_from([
            "dwm-lut-cli",
            "apply",
            "--config",
            "config.json",
            "--profile",
        ])
        .expect_err("profile without value must be rejected");

        match error {
            InjectorError::Usage(message) => {
                assert!(message.contains("--profile requires a value"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn accepts_disable_command_without_config() {
        let parsed =
            parse_args_from(["dwm-lut-cli", "disable"]).expect("disable command should parse");

        match parsed {
            ParseArgsResult::Command(CliCommand::Disable) => {}
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn accepts_status_command_without_arguments() {
        let parsed =
            parse_args_from(["dwm-lut-cli", "status"]).expect("status command should parse");

        match parsed {
            ParseArgsResult::Command(CliCommand::Status) => {}
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn accepts_host_start_command_without_arguments() {
        let parsed = parse_args_from(["dwm-lut-cli", "host", "start"])
            .expect("host start command should parse");

        match parsed {
            ParseArgsResult::Command(CliCommand::HostStart(HostStartOptions {
                host_path: None,
                dll_path: None,
            })) => {}
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn accepts_host_start_dll_argument() {
        let parsed = parse_args_from(["dwm-lut-cli", "host", "start", "--dll", "hook.dll"])
            .expect("host start should parse");

        match parsed {
            ParseArgsResult::Command(CliCommand::HostStart(HostStartOptions {
                host_path: None,
                dll_path: Some(path),
            })) => assert_eq!(path, PathBuf::from("hook.dll")),
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn accepts_host_start_host_argument() {
        let parsed = parse_args_from(["dwm-lut-cli", "host", "start", "--host", "dwm-lut.exe"])
            .expect("host start should parse");

        match parsed {
            ParseArgsResult::Command(CliCommand::HostStart(HostStartOptions {
                host_path: Some(path),
                dll_path: None,
            })) => assert_eq!(path, PathBuf::from("dwm-lut.exe")),
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn accepts_host_start_host_and_dll_arguments() {
        let parsed = parse_args_from([
            "dwm-lut-cli",
            "host",
            "start",
            "--host",
            "dwm-lut.exe",
            "--dll",
            "hook.dll",
        ])
        .expect("host start should parse");

        match parsed {
            ParseArgsResult::Command(CliCommand::HostStart(HostStartOptions {
                host_path: Some(host_path),
                dll_path: Some(dll_path),
            })) => {
                assert_eq!(host_path, PathBuf::from("dwm-lut.exe"));
                assert_eq!(dll_path, PathBuf::from("hook.dll"));
            }
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn rejects_host_start_host_without_value() {
        let error = parse_args_from(["dwm-lut-cli", "host", "start", "--host"])
            .expect_err("host without value must be rejected");

        assert!(
            matches!(error, InjectorError::Usage(message) if message.contains("--host requires a value"))
        );
    }

    #[test]
    fn rejects_unknown_host_start_arguments() {
        let error = parse_args_from(["dwm-lut-cli", "host", "start", "--config", "config.json"])
            .expect_err("host start must reject arguments");

        assert!(
            matches!(error, InjectorError::Usage(message) if message.contains("unknown argument for host start: --config"))
        );
    }

    #[test]
    fn accepts_host_stop_command_without_arguments() {
        let parsed =
            parse_args_from(["dwm-lut-cli", "host", "stop"]).expect("host stop should parse");
        assert!(matches!(
            parsed,
            ParseArgsResult::Command(CliCommand::HostStop)
        ));
    }

    #[test]
    fn rejects_host_stop_arguments() {
        let error = parse_args_from(["dwm-lut-cli", "host", "stop", "--host"])
            .expect_err("host stop must reject arguments");
        assert!(
            matches!(error, InjectorError::Usage(message) if message.contains("unknown argument for host stop: --host"))
        );
    }

    #[test]
    fn rejects_missing_host_command() {
        let error = parse_args_from(["dwm-lut-cli", "host"])
            .expect_err("host without subcommand must be rejected");
        assert!(
            matches!(error, InjectorError::Usage(message) if message.contains("missing host command"))
        );
    }

    #[test]
    fn rejects_unknown_host_command() {
        let error = parse_args_from(["dwm-lut-cli", "host", "restart"])
            .expect_err("unknown host subcommand must be rejected");
        assert!(
            matches!(error, InjectorError::Usage(message) if message.contains("unknown host command: restart"))
        );
    }

    #[test]
    fn rejects_old_run_command() {
        let error =
            parse_args_from(["dwm-lut-cli", "run"]).expect_err("old run command must be rejected");
        assert!(
            matches!(error, InjectorError::Usage(message) if message.contains("unknown command: run"))
        );
    }

    #[test]
    fn help_lists_status_and_host_commands() {
        let parsed = parse_args_from(["dwm-lut-cli", "--help"]).expect("help should parse");

        match parsed {
            ParseArgsResult::Help(message) => {
                assert!(message.contains("dwm-lut-cli status"));
                assert!(message.contains("dwm-lut-cli host start"));
                assert!(message.contains("dwm-lut-cli host stop"));
                assert!(!message.contains("dwm-lut-cli run"));
                assert!(message.contains("--host <host-exe-path>"));
            }
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn accepts_monitors_command_without_config() {
        let parsed =
            parse_args_from(["dwm-lut-cli", "monitors"]).expect("monitors command should parse");

        match parsed {
            ParseArgsResult::Command(CliCommand::Monitors) => {}
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn rejects_monitors_arguments() {
        let error = parse_args_from(["dwm-lut-cli", "monitors", "--config", "config.json"])
            .expect_err("monitors must reject arguments");

        match error {
            InjectorError::Usage(message) => {
                assert!(message.contains("unknown argument for monitors: --config"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rejects_apply_dll_argument() {
        let error = parse_args_from([
            "dwm-lut-cli",
            "apply",
            "--dll",
            "hook.dll",
            "--config",
            "config.json",
        ])
        .expect_err("apply --dll must be rejected");

        match error {
            InjectorError::Usage(message) => {
                assert!(message.contains("dwm-lut-cli host start --dll"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    fn apply_options(parsed: ParseArgsResult) -> ApplyOptions {
        match parsed {
            ParseArgsResult::Command(CliCommand::Apply(options)) => options,
            ParseArgsResult::Command(
                CliCommand::Disable
                | CliCommand::HostStart(_)
                | CliCommand::HostStop
                | CliCommand::Monitors
                | CliCommand::Status,
            ) => {
                panic!("expected apply command arguments")
            }
            ParseArgsResult::Help(_) => panic!("expected normal execution arguments"),
        }
    }
}
