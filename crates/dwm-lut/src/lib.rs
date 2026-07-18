pub mod app_args;
mod backend;
pub mod cli;
mod config;
mod control;
mod elevation;
pub mod error;
mod gui;
mod host;
mod host_runner;
mod launcher;
mod lut;
mod monitor_list;
mod native_dialog;
#[doc(hidden)]
pub mod panic_report;
mod paths;
mod runtime;
mod security;
mod startup;

pub use cli::{report_cli_error, run_cli};
pub use host_runner::{run_background, run_host};
pub use launcher::run_app_launcher;
pub use native_dialog::show_error;
