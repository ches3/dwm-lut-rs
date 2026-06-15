mod backend;
mod cli;
mod config;
mod error;
mod lut;
mod monitor_list;

use backend::{ApplyOutcome, ApplyRequest, DisableOutcome};
use cli::{CliCommand, ParseArgsResult, parse_args};
use error::{InjectorError, ShutdownStatus};

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), InjectorError> {
    match parse_args()? {
        ParseArgsResult::Run(CliCommand::Apply(options)) => run_apply(options),
        ParseArgsResult::Run(CliCommand::Disable) => run_disable(),
        ParseArgsResult::Run(CliCommand::Monitors) => monitor_list::run_monitors(),
        ParseArgsResult::Help(message) => {
            println!("{message}");
            Ok(())
        }
    }
}

fn run_apply(options: cli::CliOptions) -> Result<(), InjectorError> {
    let report = backend::apply(ApplyRequest {
        dll_path: options.dll_path,
        config_path: options.config_path,
        profile: options.profile,
    })?;

    match report.outcome {
        ApplyOutcome::Replaced => {
            println!(
                "replaced assignments in dwm.exe (pid={pid}) from {} (profile={})",
                report.config_path.display(),
                report.profile_name,
                pid = report.pid,
            );
        }
        ApplyOutcome::Initialized => {
            println!(
                "initialized dwm.exe (pid={pid}) with {} staged from {} and {} (profile={})",
                report.staged_dll_path.display(),
                report.input_dll_path.display(),
                report.config_path.display(),
                report.profile_name,
                pid = report.pid,
            );
        }
        ApplyOutcome::Reinitialized => {
            println!(
                "reinitialized dwm.exe (pid={pid}) with {} staged from {} and {} (profile={})",
                report.staged_dll_path.display(),
                report.input_dll_path.display(),
                report.config_path.display(),
                report.profile_name,
                pid = report.pid,
            );
        }
    }
    Ok(())
}

fn run_disable() -> Result<(), InjectorError> {
    let report = backend::disable()?;

    match report.outcome {
        DisableOutcome::NotInjected => {
            println!(
                "disable skipped: hook DLL is not injected into dwm.exe (pid={})",
                report.pid
            );
            Ok(())
        }
        DisableOutcome::ShutDown(ShutdownStatus::Success) => {
            println!("disabled dwm.exe hook (pid={})", report.pid);
            Ok(())
        }
        DisableOutcome::ShutDown(ShutdownStatus::NotInitialized) => {
            println!(
                "disable skipped: hook DLL is loaded but not initialized in dwm.exe (pid={})",
                report.pid
            );
            Ok(())
        }
        DisableOutcome::ShutDown(ShutdownStatus::AlreadyShutDown) => {
            println!(
                "disable skipped: hook DLL is already shut down in dwm.exe (pid={})",
                report.pid
            );
            Ok(())
        }
        DisableOutcome::ShutDown(status) => Err(InjectorError::HookShutdownFailed(status)),
    }
}
