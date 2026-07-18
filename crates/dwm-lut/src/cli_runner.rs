use crate::cli::{CliCommand, ParseArgsResult, parse_args};
use crate::control;
use crate::error::InjectorError;
use crate::host::launch;
use crate::{launcher, monitor_list, runtime, startup};

pub fn run_cli() -> Result<(), InjectorError> {
    match parse_args()? {
        ParseArgsResult::Command(CliCommand::Apply(options)) => {
            run_control_command(CliCommand::Apply(options))
        }
        ParseArgsResult::Command(CliCommand::Disable) => run_control_command(CliCommand::Disable),
        ParseArgsResult::Command(CliCommand::HostStart(options)) => {
            let host_exe = launcher::resolve_host_executable_path(options.host_path)?;
            let message =
                host_start_message(launch::start_background_host(&host_exe, options.dll_path))?;
            println!("{message}");
            Ok(())
        }
        ParseArgsResult::Command(CliCommand::HostStop) => run_control_command(CliCommand::HostStop),
        ParseArgsResult::Command(CliCommand::Install) => {
            startup::install()?;
            println!("installed dwm-lut startup task");
            Ok(())
        }
        ParseArgsResult::Command(CliCommand::Monitors) => monitor_list::run_monitors(),
        ParseArgsResult::Command(CliCommand::Status) => run_control_command(CliCommand::Status),
        ParseArgsResult::Command(CliCommand::Uninstall) => {
            startup::uninstall()?;
            println!("uninstalled dwm-lut startup task");
            Ok(())
        }
        ParseArgsResult::Help(message) => {
            println!("{message}");
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
