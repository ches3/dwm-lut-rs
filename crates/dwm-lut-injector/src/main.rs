mod cli;
mod error;
mod injector;
mod staging;
mod win32;

use cli::{CliCommand, ParseArgsResult, parse_args};
use error::{HookShutdownStatus, InjectionStep, InjectorError};
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
    let manifest_path = canonicalize_existing_file(
        &options.manifest_path,
        InjectionStep::ResolveManifestPath,
        "manifest file",
    )?;
    let staged_dll_path = stage_hook_dll(&input_dll_path)?;
    let pid = find_process_id_by_name("dwm.exe")?;

    enable_debug_privilege()?;
    let outcome = apply_or_initialize(pid, &staged_dll_path, &manifest_path)?;

    match outcome {
        ApplyOutcome::Reloaded => {
            println!(
                "reloaded manifest in dwm.exe (pid={pid}) from {}",
                manifest_path.display()
            );
        }
        ApplyOutcome::Initialized => {
            println!(
                "initialized dwm.exe (pid={pid}) with {} staged from {} and {}",
                staged_dll_path.display(),
                input_dll_path.display(),
                manifest_path.display()
            );
        }
        ApplyOutcome::Reinitialized => {
            println!(
                "reinitialized dwm.exe (pid={pid}) with {} staged from {} and {}",
                staged_dll_path.display(),
                input_dll_path.display(),
                manifest_path.display()
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
        DisableOutcome::ShutDown(HookShutdownStatus::Success) => {
            println!("disabled dwm.exe hook (pid={pid})");
            Ok(())
        }
        DisableOutcome::ShutDown(HookShutdownStatus::NotInitialized) => {
            println!(
                "disable skipped: hook DLL is loaded but not initialized in dwm.exe (pid={pid})"
            );
            Ok(())
        }
        DisableOutcome::ShutDown(HookShutdownStatus::AlreadyShutDown) => {
            println!("disable skipped: hook DLL is already shut down in dwm.exe (pid={pid})");
            Ok(())
        }
        DisableOutcome::ShutDown(status) => Err(InjectorError::HookShutdownFailed(status)),
    }
}
