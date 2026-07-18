use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Security::{TOKEN_ELEVATION_TYPE, TokenElevationTypeLimited};

use crate::error::InjectorError;
use crate::platform::elevation;

pub(crate) use super::startup_ipc::StartupNotifier;
use super::startup_ipc::{StartupEvent, StartupResultPipe};

const ARG_SEPARATOR: u16 = b' ' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const DOUBLE_QUOTE: u16 = b'"' as u16;
const TAB: u16 = b'\t' as u16;
const NEWLINE: u16 = b'\n' as u16;

pub(crate) fn start_background_host(
    executable: &Path,
    dll_path: Option<PathBuf>,
) -> Result<(), InjectorError> {
    require_same_user_elevation()?;
    let startup_pipe = StartupResultPipe::new()?;
    let panic_event = StartupEvent::new("panic", "create panic report event")?;
    let command_line = host_parameters(
        dll_path.as_deref(),
        Some(startup_pipe.name()),
        Some(panic_event.name()),
    );
    let process = elevation::run_as(executable, &command_line).map_err(|error| match error {
        elevation::RunAsError::Cancelled => InjectorError::HostElevationCancelled,
        elevation::RunAsError::Launch(source) => InjectorError::HostLaunchFailed {
            operation: "request elevation",
            source,
        },
        elevation::RunAsError::MissingProcessHandle => InjectorError::HostStartupFailed(
            "elevated host launch returned no process handle".to_string(),
        ),
    })?;
    startup_pipe.wait(&process, &panic_event)
}

fn require_same_user_elevation() -> Result<(), InjectorError> {
    let is_elevated =
        elevation::is_process_elevated().map_err(|source| InjectorError::HostLaunchFailed {
            operation: "check process elevation",
            source,
        })?;
    if is_elevated {
        return Ok(());
    }
    let elevation_type = elevation::current_token_elevation_type().map_err(|source| {
        InjectorError::HostLaunchFailed {
            operation: "check process elevation type",
            source,
        }
    })?;
    validate_same_user_elevation(false, elevation_type)
}

fn validate_same_user_elevation(
    is_elevated: bool,
    elevation_type: TOKEN_ELEVATION_TYPE,
) -> Result<(), InjectorError> {
    if is_elevated || elevation_type == TokenElevationTypeLimited {
        Ok(())
    } else {
        Err(InjectorError::HostRequiresAdministratorUser)
    }
}

fn host_parameters(
    dll_path: Option<&Path>,
    startup_result_pipe: Option<&str>,
    panic_report_event: Option<&str>,
) -> Vec<u16> {
    let mut args = vec![OsString::from("--background")];
    if let Some(pipe_name) = startup_result_pipe {
        args.push(OsString::from("--startup-result-pipe"));
        args.push(OsString::from(pipe_name));
    }
    if let Some(event_name) = panic_report_event {
        args.push(OsString::from("--panic-report-event"));
        args.push(OsString::from(event_name));
    }
    if let Some(dll_path) = dll_path {
        args.push(OsString::from("--dll"));
        args.push(dll_path.as_os_str().to_owned());
    }

    command_line_from_args(args)
}

fn command_line_from_args(args: impl IntoIterator<Item = OsString>) -> Vec<u16> {
    let mut command_line = Vec::new();
    for arg in args {
        if !command_line.is_empty() {
            command_line.push(ARG_SEPARATOR);
        }
        command_line.extend(quote_windows_arg(arg.as_os_str()));
    }
    command_line
}

fn quote_windows_arg(arg: &OsStr) -> Vec<u16> {
    let text = arg.encode_wide().collect::<Vec<_>>();
    if !text.is_empty()
        && !text
            .iter()
            .any(|&ch| matches!(ch, ARG_SEPARATOR | TAB | NEWLINE | DOUBLE_QUOTE))
    {
        return text;
    }

    let mut quoted = vec![DOUBLE_QUOTE];
    let mut backslashes = 0usize;
    for ch in text {
        match ch {
            BACKSLASH => backslashes += 1,
            DOUBLE_QUOTE => {
                quoted.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2 + 1));
                quoted.push(DOUBLE_QUOTE);
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n(BACKSLASH, backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }
    quoted.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2));
    quoted.push(DOUBLE_QUOTE);
    quoted
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Security::{
        TokenElevationTypeDefault, TokenElevationTypeFull, TokenElevationTypeLimited,
    };

    use super::*;

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().collect()
    }

    #[test]
    fn leaves_plain_arguments_unquoted() {
        assert_eq!(quote_windows_arg(OsStr::new("--dll")), wide("--dll"));
    }

    #[test]
    fn quotes_arguments_with_spaces() {
        assert_eq!(
            quote_windows_arg(OsStr::new(r"C:\hook dll\dwm_lut_hook.dll")),
            wide(r#""C:\hook dll\dwm_lut_hook.dll""#)
        );
    }

    #[test]
    fn escapes_quotes_and_trailing_backslashes() {
        assert_eq!(
            quote_windows_arg(OsStr::new(r#"C:\quoted "path"\"#)),
            wide(r#""C:\quoted \"path\"\\""#)
        );
    }

    #[test]
    fn preserves_ill_formed_utf16_arguments() {
        let arg =
            OsString::from_wide(&[b'a' as u16, 0xD800, b' ' as u16, b'"' as u16, b'\\' as u16]);

        assert_eq!(
            quote_windows_arg(arg.as_os_str()),
            vec![
                b'"' as u16,
                b'a' as u16,
                0xD800,
                b' ' as u16,
                b'\\' as u16,
                b'"' as u16,
                b'\\' as u16,
                b'\\' as u16,
                b'"' as u16,
            ]
        );
        assert!(!quote_windows_arg(arg.as_os_str()).contains(&0xFFFD));
    }

    #[test]
    fn builds_host_parameters_with_dll_path() {
        let command_line = host_parameters(
            Some(Path::new(r"C:\hook dll\dwm_lut_hook.dll")),
            Some(r"\\.\pipe\dwm-lut-rs-startup-1234"),
            Some(r"Local\dwm-lut-rs-panic-1234"),
        );

        assert_eq!(
            command_line,
            wide(
                r#"--background --startup-result-pipe \\.\pipe\dwm-lut-rs-startup-1234 --panic-report-event Local\dwm-lut-rs-panic-1234 --dll "C:\hook dll\dwm_lut_hook.dll""#
            )
        );
    }

    #[test]
    fn limited_admin_token_can_launch_elevated_host() {
        assert!(validate_same_user_elevation(false, TokenElevationTypeLimited).is_ok());
    }

    #[test]
    fn standard_user_token_cannot_launch_host_as_another_user() {
        assert!(matches!(
            validate_same_user_elevation(false, TokenElevationTypeDefault),
            Err(InjectorError::HostRequiresAdministratorUser)
        ));
    }

    #[test]
    fn elevated_process_can_launch_host_without_a_limited_token() {
        for elevation_type in [TokenElevationTypeDefault, TokenElevationTypeFull] {
            assert!(validate_same_user_elevation(true, elevation_type).is_ok());
        }
    }
}
