mod backend;
mod cli;
mod config;
mod control;
mod error;
mod lut;
mod monitor_list;
mod runtime;

use cli::{CliCommand, ParseArgsResult, parse_args};
use error::InjectorError;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), InjectorError> {
    match parse_args()? {
        ParseArgsResult::Run(CliCommand::Apply(options)) => {
            run_control_command(CliCommand::Apply(options))
        }
        ParseArgsResult::Run(CliCommand::Disable) => run_control_command(CliCommand::Disable),
        ParseArgsResult::Run(CliCommand::Monitors) => monitor_list::run_monitors(),
        ParseArgsResult::Run(CliCommand::Run(options)) => run_primary(options.dll_path),
        ParseArgsResult::Run(CliCommand::Status) => run_control_command(CliCommand::Status),
        ParseArgsResult::Help(message) => {
            println!("{message}");
            Ok(())
        }
    }
}

fn run_primary(dll_path: Option<std::path::PathBuf>) -> Result<(), InjectorError> {
    backend::ensure_primary_privileges()?;
    let dll_path = runtime::resolve_primary_dll_path(dll_path)?;
    control::server::run_server(dll_path)
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
