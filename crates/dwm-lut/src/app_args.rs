use std::ffi::OsString;
use std::path::PathBuf;

use crate::error::InjectorError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    Launcher,
    Background(BackgroundOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundOptions {
    pub dll_path: Option<PathBuf>,
    pub startup_result_pipe: Option<String>,
    pub panic_report_event: Option<String>,
    pub startup_abort_event: Option<String>,
}

pub fn parse_app_args() -> Result<AppMode, InjectorError> {
    parse_app_args_from(std::env::args_os())
}

pub fn parse_app_args_from(
    args: impl IntoIterator<Item = impl Into<OsString>>,
) -> Result<AppMode, InjectorError> {
    let mut background = false;
    let mut dll_path = None;
    let mut startup_result_pipe = None;
    let mut panic_report_event = None;
    let mut startup_abort_event = None;
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next();

    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--background" if !background => background = true,
            "--background" => {
                return Err(InjectorError::Usage(
                    "--background may only be specified once".to_string(),
                ));
            }
            "--dll" if dll_path.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| InjectorError::Usage("--dll requires a value".to_string()))?;
                dll_path = Some(PathBuf::from(value));
            }
            "--dll" => {
                return Err(InjectorError::Usage(
                    "--dll may only be specified once".to_string(),
                ));
            }
            "--startup-result-pipe" if startup_result_pipe.is_none() => {
                let value = args.next().ok_or_else(|| {
                    InjectorError::Usage("--startup-result-pipe requires a value".to_string())
                })?;
                startup_result_pipe = Some(value.to_string_lossy().into_owned());
            }
            "--startup-result-pipe" => {
                return Err(InjectorError::Usage(
                    "--startup-result-pipe may only be specified once".to_string(),
                ));
            }
            "--panic-report-event" if panic_report_event.is_none() => {
                let value = args.next().ok_or_else(|| {
                    InjectorError::Usage("--panic-report-event requires a value".to_string())
                })?;
                panic_report_event = Some(value.to_string_lossy().into_owned());
            }
            "--panic-report-event" => {
                return Err(InjectorError::Usage(
                    "--panic-report-event may only be specified once".to_string(),
                ));
            }
            "--startup-abort-event" if startup_abort_event.is_none() => {
                let value = args.next().ok_or_else(|| {
                    InjectorError::Usage("--startup-abort-event requires a value".to_string())
                })?;
                startup_abort_event = Some(value.to_string_lossy().into_owned());
            }
            "--startup-abort-event" => {
                return Err(InjectorError::Usage(
                    "--startup-abort-event may only be specified once".to_string(),
                ));
            }
            other => {
                return Err(InjectorError::Usage(format!(
                    "unknown application argument: {other}"
                )));
            }
        }
    }

    if !background {
        if dll_path.is_some()
            || startup_result_pipe.is_some()
            || panic_report_event.is_some()
            || startup_abort_event.is_some()
        {
            return Err(InjectorError::Usage(
                "internal background options require --background".to_string(),
            ));
        }
        return Ok(AppMode::Launcher);
    }

    let coordination_argument_count = [
        startup_result_pipe.is_some(),
        panic_report_event.is_some(),
        startup_abort_event.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if coordination_argument_count != 0 && coordination_argument_count != 3 {
        return Err(InjectorError::Usage(
            "--startup-result-pipe, --panic-report-event, and --startup-abort-event must be specified together"
                .to_string(),
        ));
    }

    Ok(AppMode::Background(BackgroundOptions {
        dll_path,
        startup_result_pipe,
        panic_report_event,
        startup_abort_event,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_select_launcher_mode() {
        assert_eq!(
            parse_app_args_from(["dwm-lut.exe"]).unwrap(),
            AppMode::Launcher
        );
    }

    #[test]
    fn background_accepts_internal_options() {
        let parsed = parse_app_args_from([
            "dwm-lut.exe",
            "--background",
            "--dll",
            "hook.dll",
            "--startup-result-pipe",
            r"\\.\pipe\startup",
            "--panic-report-event",
            r"Local\dwm-lut-rs-panic-test",
            "--startup-abort-event",
            r"Local\dwm-lut-rs-abort-test",
        ])
        .unwrap();

        assert_eq!(
            parsed,
            AppMode::Background(BackgroundOptions {
                dll_path: Some(PathBuf::from("hook.dll")),
                startup_result_pipe: Some(r"\\.\pipe\startup".to_string()),
                panic_report_event: Some(r"Local\dwm-lut-rs-panic-test".to_string()),
                startup_abort_event: Some(r"Local\dwm-lut-rs-abort-test".to_string()),
            })
        );
    }

    #[test]
    fn host_options_require_background_mode() {
        let error = parse_app_args_from(["dwm-lut.exe", "--dll", "hook.dll"])
            .expect_err("host-only options must not be accepted in launcher mode");

        assert!(error.to_string().contains("require --background"));
    }

    #[test]
    fn startup_result_pipe_requires_background_mode() {
        let error =
            parse_app_args_from(["dwm-lut.exe", "--startup-result-pipe", r"\\.\pipe\startup"])
                .expect_err("internal option must not be accepted in launcher mode");

        assert!(error.to_string().contains("require --background"));
    }

    #[test]
    fn startup_coordination_arguments_must_be_paired() {
        let missing_event = parse_app_args_from([
            "dwm-lut.exe",
            "--background",
            "--startup-result-pipe",
            r"\\.\pipe\startup",
        ])
        .expect_err("startup result pipe must require panic event");
        let missing_pipe = parse_app_args_from([
            "dwm-lut.exe",
            "--background",
            "--panic-report-event",
            r"Local\dwm-lut-rs-panic-test",
            "--startup-abort-event",
            r"Local\dwm-lut-rs-abort-test",
        ])
        .expect_err("panic event must require startup result pipe");

        assert!(
            missing_event
                .to_string()
                .contains("must be specified together")
        );
        assert!(
            missing_pipe
                .to_string()
                .contains("must be specified together")
        );
    }

    #[test]
    fn rejects_unknown_argument() {
        let error = parse_app_args_from(["dwm-lut.exe", "--show-gui"])
            .expect_err("unknown public modes must be rejected");

        assert!(error.to_string().contains("unknown application argument"));
    }

    #[test]
    fn rejects_duplicate_background_option() {
        let error = parse_app_args_from(["dwm-lut.exe", "--background", "--background"])
            .expect_err("duplicate mode arguments must be rejected");

        assert!(error.to_string().contains("only be specified once"));
    }
}
