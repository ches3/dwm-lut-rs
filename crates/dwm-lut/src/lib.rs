#[cfg(not(all(
    target_arch = "x86_64",
    target_vendor = "pc",
    target_os = "windows",
    target_env = "msvc",
)))]
compile_error!("dwm-lut supports only x86_64-pc-windows-msvc");

pub mod cli;
mod config;
mod control;
pub mod entry;
mod gui;
pub mod host;
mod inject;
mod lut;
mod monitor;
#[doc(hidden)]
pub mod panic_report;
mod paths;
mod platform;

pub use cli::{report_cli_error, run_cli};
pub use entry::run_app_launcher;
pub use host::{HostProcessError, HostRunError, run_background, run_host};
pub use platform::show_error;
