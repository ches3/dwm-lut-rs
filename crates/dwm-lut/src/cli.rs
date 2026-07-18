use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::control;
use crate::error::InjectorError;
use crate::host::launch;
use crate::{launcher, monitor_list, runtime, startup};

#[derive(Debug, PartialEq, Eq)]
pub struct ApplyOptions {
    pub config_path: Option<PathBuf>,
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
    Install,
    Monitors,
    Status,
    Uninstall,
}

#[derive(Debug, Parser)]
#[command(
    name = "dwm-lut-cli",
    version,
    about = "Control the dwm-lut host and LUT configuration"
)]
struct CliArgs {
    #[command(subcommand)]
    command: CommandArgs,
}

#[derive(Debug, Subcommand)]
enum CommandArgs {
    /// Apply a LUT configuration through the running host.
    Apply(ApplyArgs),
    /// Disable the LUT hook in DWM.
    Disable,
    /// Show the running host status.
    Status,
    /// List active monitors.
    Monitors,
    /// Manage the dwm-lut host instance.
    Host {
        #[command(subcommand)]
        command: HostCommandArgs,
    },
    /// Install the dwm-lut startup task.
    Install,
    /// Uninstall the dwm-lut startup task.
    Uninstall,
}

#[derive(Debug, Args)]
struct ApplyArgs {
    /// Path to the configuration file.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Configuration profile name.
    #[arg(long, value_name = "NAME", value_parser = parse_profile)]
    profile: Option<String>,
}

#[derive(Debug, Subcommand)]
enum HostCommandArgs {
    /// Start the elevated host instance.
    Start(HostStartArgs),
    /// Stop the running host instance.
    Stop,
}

#[derive(Debug, Args)]
struct HostStartArgs {
    /// Path to the host executable.
    #[arg(long, value_name = "PATH")]
    host: Option<PathBuf>,
    /// Path to the hook DLL.
    #[arg(long, value_name = "PATH")]
    dll: Option<PathBuf>,
}

pub fn parse_args() -> Result<CliCommand, clap::Error> {
    parse_args_from(std::env::args_os())
}

fn parse_args_from<I, T>(args: I) -> Result<CliCommand, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    CliArgs::try_parse_from(args).map(|args| args.command.into())
}

fn parse_profile(value: &str) -> Result<String, String> {
    let profile = value.trim();
    if profile.is_empty() {
        Err("profile must not be empty".to_string())
    } else {
        Ok(profile.to_owned())
    }
}

impl From<CommandArgs> for CliCommand {
    fn from(command: CommandArgs) -> Self {
        match command {
            CommandArgs::Apply(args) => Self::Apply(ApplyOptions {
                config_path: args.config,
                profile: args.profile,
            }),
            CommandArgs::Disable => Self::Disable,
            CommandArgs::Host {
                command: HostCommandArgs::Start(args),
            } => Self::HostStart(HostStartOptions {
                host_path: args.host,
                dll_path: args.dll,
            }),
            CommandArgs::Host {
                command: HostCommandArgs::Stop,
            } => Self::HostStop,
            CommandArgs::Install => Self::Install,
            CommandArgs::Monitors => Self::Monitors,
            CommandArgs::Status => Self::Status,
            CommandArgs::Uninstall => Self::Uninstall,
        }
    }
}

pub fn run_cli(command: CliCommand) -> Result<(), InjectorError> {
    match command {
        CliCommand::Apply(options) => run_control_command(CliCommand::Apply(options)),
        CliCommand::Disable => run_control_command(CliCommand::Disable),
        CliCommand::HostStart(options) => {
            let host_exe = launcher::resolve_host_executable_path(options.host_path)?;
            let message =
                host_start_message(launch::start_background_host(&host_exe, options.dll_path))?;
            println!("{message}");
            Ok(())
        }
        CliCommand::HostStop => run_control_command(CliCommand::HostStop),
        CliCommand::Install => {
            startup::install()?;
            println!("installed dwm-lut startup task");
            Ok(())
        }
        CliCommand::Monitors => monitor_list::run_monitors(),
        CliCommand::Status => run_control_command(CliCommand::Status),
        CliCommand::Uninstall => {
            startup::uninstall()?;
            println!("uninstalled dwm-lut startup task");
            Ok(())
        }
    }
}

pub fn report_cli_error(error: &InjectorError) -> i32 {
    eprintln!("{error}");
    match error {
        InjectorError::StartupTaskOperationFailed { exit_code, .. } => *exit_code as i32,
        _ => 1,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apply_options_and_normalizes_profile() {
        let command = parse_args_from([
            "dwm-lut-cli",
            "apply",
            "--config",
            "config.json",
            "--profile",
            "  GAMING  ",
        ])
        .unwrap();

        assert_eq!(
            command,
            CliCommand::Apply(ApplyOptions {
                config_path: Some(PathBuf::from("config.json")),
                profile: Some("GAMING".to_string()),
            })
        );
    }

    #[test]
    fn rejects_blank_profile() {
        for profile in ["", "   "] {
            let error = parse_args_from(["dwm-lut-cli", "apply", "--profile", profile])
                .expect_err("blank profile must be rejected");

            assert!(error.to_string().contains("profile must not be empty"));
        }
    }

    #[test]
    fn parses_host_start_options() {
        let command = parse_args_from([
            "dwm-lut-cli",
            "host",
            "start",
            "--host",
            "dwm-lut.exe",
            "--dll",
            "hook.dll",
        ])
        .unwrap();

        assert_eq!(
            command,
            CliCommand::HostStart(HostStartOptions {
                host_path: Some(PathBuf::from("dwm-lut.exe")),
                dll_path: Some(PathBuf::from("hook.dll")),
            })
        );
    }

    #[test]
    fn maps_commands_without_options() {
        let cases = [
            (vec!["dwm-lut-cli", "disable"], CliCommand::Disable),
            (vec!["dwm-lut-cli", "host", "stop"], CliCommand::HostStop),
            (vec!["dwm-lut-cli", "install"], CliCommand::Install),
            (vec!["dwm-lut-cli", "monitors"], CliCommand::Monitors),
            (vec!["dwm-lut-cli", "status"], CliCommand::Status),
            (vec!["dwm-lut-cli", "uninstall"], CliCommand::Uninstall),
        ];

        for (args, expected) in cases {
            assert_eq!(parse_args_from(args).unwrap(), expected);
        }
    }

    #[test]
    fn host_start_message_reports_started() {
        assert_eq!(
            host_start_message(Ok(())).unwrap(),
            "started dwm-lut host instance"
        );
    }

    #[test]
    fn host_start_message_treats_already_running_as_success() {
        assert_eq!(
            host_start_message(Err(InjectorError::HostAlreadyRunning)).unwrap(),
            "dwm-lut host instance is already running"
        );
    }
}
