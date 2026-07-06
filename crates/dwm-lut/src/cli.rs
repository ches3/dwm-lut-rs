use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use crate::error::InjectorError;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ApplyOptions {
    pub(crate) config_path: PathBuf,
    pub(crate) profile: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RunOptions {
    pub(crate) dll_path: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliCommand {
    Apply(ApplyOptions),
    Disable,
    Monitors,
    Run(RunOptions),
    Status,
}

#[derive(Debug)]
pub(crate) enum ParseArgsResult {
    Run(CliCommand),
    Help(String),
}

pub(crate) fn parse_args() -> Result<ParseArgsResult, InjectorError> {
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
        "monitors" => parse_monitors_args(args),
        "run" => parse_run_args(args),
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
                    "apply --dll is not supported through the primary instance; start the primary with `dwm-lut run --dll <hook-dll-path>`",
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

    Ok(ParseArgsResult::Run(CliCommand::Apply(ApplyOptions {
        config_path,
        profile,
    })))
}

fn parse_run_args(
    mut args: impl Iterator<Item = OsString>,
) -> Result<ParseArgsResult, InjectorError> {
    let mut dll_path = None;
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
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
                    "unknown argument for run: {other}"
                ))));
            }
        }
    }

    Ok(ParseArgsResult::Run(CliCommand::Run(RunOptions {
        dll_path,
    })))
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

    Ok(ParseArgsResult::Run(CliCommand::Disable))
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

    Ok(ParseArgsResult::Run(CliCommand::Monitors))
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

    Ok(ParseArgsResult::Run(parsed))
}

fn usage_message(problem: &str) -> String {
    let usage = "usage: dwm-lut apply --config <config-path> [--profile <profile-name>]\n       dwm-lut disable\n       dwm-lut status\n       dwm-lut monitors\n       dwm-lut run [--dll <hook-dll-path>]";
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

    use super::{ApplyOptions, CliCommand, ParseArgsResult, RunOptions, parse_args_from};

    #[test]
    fn reports_help_without_treating_it_as_invalid_usage() {
        let parsed = parse_args_from(["dwm-lut", "--help"]).expect("help should parse");

        match parsed {
            ParseArgsResult::Help(message) => {
                assert!(message.starts_with("usage: dwm-lut"));
            }
            ParseArgsResult::Run(_) => panic!("help must not continue to normal execution"),
        }
    }

    #[test]
    fn requires_config_path() {
        let error =
            parse_args_from(["dwm-lut", "apply"]).expect_err("missing config must be rejected");

        match error {
            InjectorError::Usage(message) => {
                assert!(message.contains("missing --config"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rejects_config_without_command() {
        let error = parse_args_from(["dwm-lut", "--config", "config.json"])
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
        let parsed = parse_args_from(["dwm-lut", "apply", "--config", "config.json"])
            .expect("explicit apply command should parse");

        assert_eq!(
            run_options(parsed),
            ApplyOptions {
                config_path: PathBuf::from("config.json"),
                profile: None,
            }
        );
    }

    #[test]
    fn accepts_profile_argument() {
        let parsed = parse_args_from([
            "dwm-lut",
            "apply",
            "--config",
            "config.json",
            "--profile",
            "gaming",
        ])
        .expect("profile argument should parse");

        assert_eq!(
            run_options(parsed),
            ApplyOptions {
                config_path: PathBuf::from("config.json"),
                profile: Some("gaming".to_string()),
            }
        );
    }

    #[test]
    fn accepts_profile_argument_with_mixed_case() {
        let parsed = parse_args_from([
            "dwm-lut",
            "apply",
            "--config",
            "config.json",
            "--profile",
            "GAMING",
        ])
        .expect("mixed-case profile argument should parse");

        assert_eq!(
            run_options(parsed),
            ApplyOptions {
                config_path: PathBuf::from("config.json"),
                profile: Some("GAMING".to_string()),
            }
        );
    }

    #[test]
    fn accepts_profile_argument_with_surrounding_whitespace() {
        let parsed = parse_args_from([
            "dwm-lut",
            "apply",
            "--config",
            "config.json",
            "--profile",
            "  gaming  ",
        ])
        .expect("profile argument should parse");

        assert_eq!(
            run_options(parsed),
            ApplyOptions {
                config_path: PathBuf::from("config.json"),
                profile: Some("gaming".to_string()),
            }
        );
    }

    #[test]
    fn rejects_empty_profile_argument() {
        let error = parse_args_from([
            "dwm-lut",
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
            "dwm-lut",
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
        let error = parse_args_from(["dwm-lut", "apply", "--config", "config.json", "--profile"])
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
        let parsed = parse_args_from(["dwm-lut", "disable"]).expect("disable command should parse");

        match parsed {
            ParseArgsResult::Run(CliCommand::Disable) => {}
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn accepts_status_command_without_arguments() {
        let parsed = parse_args_from(["dwm-lut", "status"]).expect("status command should parse");

        match parsed {
            ParseArgsResult::Run(CliCommand::Status) => {}
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn accepts_run_command_without_arguments() {
        let parsed = parse_args_from(["dwm-lut", "run"]).expect("run command should parse");

        match parsed {
            ParseArgsResult::Run(CliCommand::Run(RunOptions { dll_path: None })) => {}
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn accepts_run_dll_argument() {
        let parsed =
            parse_args_from(["dwm-lut", "run", "--dll", "hook.dll"]).expect("run should parse");

        match parsed {
            ParseArgsResult::Run(CliCommand::Run(RunOptions {
                dll_path: Some(path),
            })) => {
                assert_eq!(path, PathBuf::from("hook.dll"));
            }
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_run_arguments() {
        let error = parse_args_from(["dwm-lut", "run", "--config", "config.json"])
            .expect_err("run must reject arguments");

        match error {
            InjectorError::Usage(message) => {
                assert!(message.contains("unknown argument for run: --config"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn help_lists_status_and_run_commands() {
        let parsed = parse_args_from(["dwm-lut", "--help"]).expect("help should parse");

        match parsed {
            ParseArgsResult::Help(message) => {
                assert!(message.contains("dwm-lut status"));
                assert!(message.contains("dwm-lut run"));
            }
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn accepts_monitors_command_without_config() {
        let parsed =
            parse_args_from(["dwm-lut", "monitors"]).expect("monitors command should parse");

        match parsed {
            ParseArgsResult::Run(CliCommand::Monitors) => {}
            other => panic!("unexpected parse result: {other:?}"),
        }
    }

    #[test]
    fn rejects_monitors_arguments() {
        let error = parse_args_from(["dwm-lut", "monitors", "--config", "config.json"])
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
            "dwm-lut",
            "apply",
            "--dll",
            "hook.dll",
            "--config",
            "config.json",
        ])
        .expect_err("apply --dll must be rejected");

        match error {
            InjectorError::Usage(message) => {
                assert!(message.contains("dwm-lut run --dll"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    fn run_options(parsed: ParseArgsResult) -> ApplyOptions {
        match parsed {
            ParseArgsResult::Run(CliCommand::Apply(options)) => options,
            ParseArgsResult::Run(
                CliCommand::Disable
                | CliCommand::Monitors
                | CliCommand::Run(_)
                | CliCommand::Status,
            ) => {
                panic!("expected apply command arguments")
            }
            ParseArgsResult::Help(_) => panic!("expected normal execution arguments"),
        }
    }
}
