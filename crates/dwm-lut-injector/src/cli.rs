use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use crate::error::InjectorError;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CliOptions {
    pub(crate) dll_path: PathBuf,
    pub(crate) manifest_path: PathBuf,
}

#[derive(Debug)]
pub(crate) enum ParseArgsResult {
    Run(CliOptions),
    Help(String),
}

pub(crate) fn parse_args() -> Result<ParseArgsResult, InjectorError> {
    parse_args_from(env::args_os())
}

fn parse_args_from<I, T>(args: I) -> Result<ParseArgsResult, InjectorError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let _program = args.next();

    let mut dll_path = None;
    let mut manifest_path = None;
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--dll" => {
                let value = args
                    .next()
                    .ok_or_else(|| InjectorError::Usage(usage_message("--dll requires a value")))?;
                dll_path = Some(PathBuf::from(value));
            }
            "--manifest" => {
                let value = args.next().ok_or_else(|| {
                    InjectorError::Usage(usage_message("--manifest requires a value"))
                })?;
                manifest_path = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                return Ok(ParseArgsResult::Help(usage_message("")));
            }
            other => {
                return Err(InjectorError::Usage(usage_message(&format!(
                    "unknown argument: {other}"
                ))));
            }
        }
    }

    let dll_path = dll_path.ok_or_else(|| InjectorError::Usage(usage_message("missing --dll")))?;
    let manifest_path =
        manifest_path.ok_or_else(|| InjectorError::Usage(usage_message("missing --manifest")))?;

    Ok(ParseArgsResult::Run(CliOptions {
        dll_path,
        manifest_path,
    }))
}

fn usage_message(problem: &str) -> String {
    let usage = "usage: dwm-lut-injector --dll <hook-dll-path> --manifest <manifest-path>";
    if problem.is_empty() {
        usage.to_string()
    } else {
        format!("{problem}\n{usage}")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::error::InjectorError;

    use super::{CliOptions, ParseArgsResult, parse_args_from};

    #[test]
    fn reports_help_without_treating_it_as_invalid_usage() {
        let parsed = parse_args_from(["dwm-lut-injector", "--help"]).expect("help should parse");

        match parsed {
            ParseArgsResult::Help(message) => {
                assert!(message.starts_with("usage: dwm-lut-injector"));
            }
            ParseArgsResult::Run(_) => panic!("help must not continue to normal execution"),
        }
    }

    #[test]
    fn requires_both_dll_and_manifest_paths() {
        let error = parse_args_from(["dwm-lut-injector", "--dll", "hook.dll"])
            .expect_err("missing manifest must be rejected");

        match error {
            InjectorError::Usage(message) => {
                assert!(message.contains("missing --manifest"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn accepts_required_arguments() {
        let parsed = parse_args_from([
            "dwm-lut-injector",
            "--dll",
            "hook.dll",
            "--manifest",
            "manifest.json",
        ])
        .expect("valid arguments should parse");

        assert_eq!(
            run_options(parsed),
            CliOptions {
                dll_path: PathBuf::from("hook.dll"),
                manifest_path: PathBuf::from("manifest.json"),
            }
        );
    }

    fn run_options(parsed: ParseArgsResult) -> CliOptions {
        match parsed {
            ParseArgsResult::Run(options) => options,
            ParseArgsResult::Help(_) => panic!("expected normal execution arguments"),
        }
    }
}
