use std::env;
use std::path::PathBuf;

use dwm_lut_config::{LutManifest, load_manifest};
use dwm_lut_hook::{BuildProfile, HookConfig, initialize};

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = manifest_path_from_args()?;

    let manifest = match load_manifest(&manifest_path) {
        Ok(manifest) => manifest,
        Err(_) => LutManifest::empty(),
    };

    let config = HookConfig {
        manifest_path,
        profile: BuildProfile::Windows11_25H2,
    };

    initialize(config, manifest)?;

    println!("injector skeleton ready");
    Ok(())
}

fn manifest_path_from_args() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut args = env::args_os();
    let _program = args.next();

    match args.next() {
        Some(path) => Ok(PathBuf::from(path)),
        None => Err("usage: dwm-lut-injector <manifest-path>".into()),
    }
}
