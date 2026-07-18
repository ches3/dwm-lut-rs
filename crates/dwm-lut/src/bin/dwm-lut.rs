#![windows_subsystem = "windows"]

use std::panic::PanicHookInfo;

fn main() {
    install_panic_hook();
    let exit_code = match dwm_lut::entry::parse_app_args() {
        Ok(dwm_lut::entry::AppMode::Launcher) => match dwm_lut::run_app_launcher() {
            Ok(()) => 0,
            Err(dwm_lut::error::InjectorError::HostPanicAlreadyReported) => 1,
            Err(error) => {
                dwm_lut::show_error(&error.to_string());
                1
            }
        },
        Ok(dwm_lut::entry::AppMode::Background(options)) => {
            match dwm_lut::run_background(options) {
                Ok(()) => 0,
                Err(_) => 1,
            }
        }
        Err(error) => {
            dwm_lut::show_error(&error.to_string());
            1
        }
    };
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let action = dwm_lut::panic_report::begin_panic_report();
        previous(info);
        match action {
            dwm_lut::panic_report::PanicReportAction::ShowDialog => {
                dwm_lut::show_error(&format_panic_message(info));
                std::process::exit(dwm_lut::panic_report::PANIC_EXIT_CODE);
            }
            dwm_lut::panic_report::PanicReportAction::SuppressDialog => {
                std::process::exit(dwm_lut::panic_report::PANIC_EXIT_CODE);
            }
            dwm_lut::panic_report::PanicReportAction::AlreadyReported => {}
        }
    }));
}

fn format_panic_message(info: &PanicHookInfo<'_>) -> String {
    let message = if let Some(message) = info.payload().downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = info.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "panic payload was not a string".to_string()
    };
    match info.location() {
        Some(location) => format!(
            "dwm-lut encountered an unexpected error.\n\nmessage: {message}\nlocation: {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        ),
        None => format!("dwm-lut encountered an unexpected error.\n\nmessage: {message}"),
    }
}
