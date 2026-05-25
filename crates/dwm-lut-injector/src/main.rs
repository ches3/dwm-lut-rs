mod cli;
mod error;
mod injector;
mod staging;
mod win32;

use cli::{ParseArgsResult, parse_args};
use error::{InjectionStep, InjectorError};
use injector::{canonicalize_existing_file, inject_and_initialize};
use staging::{default_hook_dll_path, stage_hook_dll};
use win32::{enable_debug_privilege, find_process_id_by_name};

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), InjectorError> {
    let options = match parse_args()? {
        ParseArgsResult::Run(options) => options,
        ParseArgsResult::Help(message) => {
            println!("{message}");
            return Ok(());
        }
    };
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
    inject_and_initialize(pid, &staged_dll_path, &manifest_path)?;

    println!(
        "initialized dwm.exe (pid={pid}) with {} staged from {} and {}",
        staged_dll_path.display(),
        input_dll_path.display(),
        manifest_path.display()
    );
    Ok(())
}
