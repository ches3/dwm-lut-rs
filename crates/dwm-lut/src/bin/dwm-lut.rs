#![windows_subsystem = "windows"]

fn main() {
    let result = parse_host_args()
        .and_then(|options| dwm_lut::run_host(options.dll_path, options.startup_result_handle));
    if let Err(err) = result {
        std::process::exit(dwm_lut::report_host_startup_error(&err));
    }
}

struct HostOptions {
    dll_path: Option<std::path::PathBuf>,
    startup_result_handle: Option<usize>,
}

fn parse_host_args() -> Result<HostOptions, dwm_lut::error::InjectorError> {
    let mut dll_path = None;
    let mut startup_result_handle = None;
    let mut args = std::env::args_os();
    let _program = args.next();

    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--dll" => {
                let value = args.next().ok_or_else(|| {
                    dwm_lut::error::InjectorError::Usage("--dll requires a value".to_string())
                })?;
                dll_path = Some(std::path::PathBuf::from(value));
            }
            "--startup-result-handle" => {
                let value = args.next().ok_or_else(|| {
                    dwm_lut::error::InjectorError::Usage(
                        "--startup-result-handle requires a value".to_string(),
                    )
                })?;
                startup_result_handle =
                    Some(value.to_string_lossy().parse::<usize>().map_err(|_| {
                        dwm_lut::error::InjectorError::Usage(
                            "--startup-result-handle must be an integer".to_string(),
                        )
                    })?);
            }
            other => {
                return Err(dwm_lut::error::InjectorError::Usage(format!(
                    "unknown host argument: {other}"
                )));
            }
        }
    }

    Ok(HostOptions {
        dll_path,
        startup_result_handle,
    })
}
