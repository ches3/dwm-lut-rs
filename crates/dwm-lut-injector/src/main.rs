mod cli;
mod config;
mod error;
mod injector;
mod lut;
mod monitor;
mod staging;
mod win32;

use cli::{CliCommand, ParseArgsResult, parse_args};
use error::{InjectionStep, InjectorError, ShutdownStatus};
use injector::{
    ApplyOutcome, DisableOutcome, apply_or_initialize, canonicalize_existing_file,
    disable_injected_hook,
};
use staging::{default_hook_dll_path, stage_hook_dll};
use win32::{enable_debug_privilege, find_process_id_by_name};

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
        ParseArgsResult::Help(message) => {
            println!("{message}");
            Ok(())
        }
    }
}

fn run_apply(options: cli::CliOptions) -> Result<(), InjectorError> {
    let input_dll_path = options.dll_path.unwrap_or(default_hook_dll_path()?);
    let input_dll_path = canonicalize_existing_file(
        &input_dll_path,
        InjectionStep::ResolveLocalHookDll,
        "hook DLL",
    )?;
    let config_path = canonicalize_existing_file(
        &options.config_path,
        InjectionStep::ResolveConfigPath,
        "config file",
    )?;
    let payload = config::load_payload(&config_path)?;
    let payload_bytes = dwm_lut_payload::serialize_payload(&payload)?;
    let staged_dll_path = stage_hook_dll(&input_dll_path)?;
    let pid = find_process_id_by_name("dwm.exe")?;

    enable_debug_privilege()?;
    let outcome = apply_or_initialize(pid, &staged_dll_path, &payload_bytes)?;

    match outcome {
        ApplyOutcome::Reloaded => {
            println!(
                "reloaded payload in dwm.exe (pid={pid}) from {}",
                config_path.display()
            );
        }
        ApplyOutcome::Initialized => {
            println!(
                "initialized dwm.exe (pid={pid}) with {} staged from {} and {}",
                staged_dll_path.display(),
                input_dll_path.display(),
                config_path.display()
            );
        }
        ApplyOutcome::Reinitialized => {
            println!(
                "reinitialized dwm.exe (pid={pid}) with {} staged from {} and {}",
                staged_dll_path.display(),
                input_dll_path.display(),
                config_path.display()
            );
        }
    }
    Ok(())
}

fn run_disable() -> Result<(), InjectorError> {
    let pid = find_process_id_by_name("dwm.exe")?;

    enable_debug_privilege()?;
    match disable_injected_hook(pid)? {
        DisableOutcome::NotInjected => {
            println!("disable skipped: hook DLL is not injected into dwm.exe (pid={pid})");
            Ok(())
        }
        DisableOutcome::ShutDown(ShutdownStatus::Success) => {
            println!("disabled dwm.exe hook (pid={pid})");
            Ok(())
        }
        DisableOutcome::ShutDown(ShutdownStatus::NotInitialized) => {
            println!(
                "disable skipped: hook DLL is loaded but not initialized in dwm.exe (pid={pid})"
            );
            Ok(())
        }
        DisableOutcome::ShutDown(ShutdownStatus::AlreadyShutDown) => {
            println!("disable skipped: hook DLL is already shut down in dwm.exe (pid={pid})");
            Ok(())
        }
        DisableOutcome::ShutDown(status) => Err(InjectorError::HookShutdownFailed(status)),
    }
}
