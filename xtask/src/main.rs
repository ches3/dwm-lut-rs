use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod assets;
mod profile;

#[derive(Parser)]
#[command(name = "xtask", bin_name = "cargo xtask")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate application icon assets
    Assets,
    /// Hook profile tooling
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// Check whether the registered profile supports a dwmcore.dll (signatures + layout)
    Check {
        /// Use %SystemRoot%\\System32\\dwmcore.dll
        #[arg(long, conflicts_with_all = ["dll", "pdb", "version", "build_latest"])]
        system: bool,
        /// Path to dwmcore.dll
        #[arg(long, conflicts_with_all = ["system", "version", "build_latest"])]
        dll: Option<PathBuf>,
        /// Path to dwmcore.pdb
        #[arg(long, conflicts_with_all = ["system", "version", "build_latest"])]
        pdb: Option<PathBuf>,
        /// FileVersion like 10.0.26100.4484
        #[arg(
            long,
            value_name = "FILE_VERSION",
            conflicts_with_all = ["system", "dll", "pdb", "build_latest"]
        )]
        version: Option<String>,
        /// Winbindex build number; resolve its latest amd64 FileVersion (e.g. 26100)
        #[arg(
            long,
            value_name = "BUILD",
            conflicts_with_all = ["system", "dll", "pdb", "version"]
        )]
        build_latest: Option<u16>,
        /// Fetch missing DLL/PDB without confirmation
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },
    /// Extract signature/layout materials from DLL + PDB
    Extract {
        /// Use %SystemRoot%\\System32\\dwmcore.dll
        #[arg(long, conflicts_with_all = ["dll", "pdb", "version", "build_latest"])]
        system: bool,
        /// Path to dwmcore.dll
        #[arg(long, conflicts_with_all = ["system", "version", "build_latest"])]
        dll: Option<PathBuf>,
        /// Path to dwmcore.pdb
        #[arg(long, conflicts_with_all = ["system", "version", "build_latest"])]
        pdb: Option<PathBuf>,
        /// FileVersion like 10.0.26100.4484
        #[arg(
            long,
            value_name = "FILE_VERSION",
            conflicts_with_all = ["system", "dll", "pdb", "build_latest"]
        )]
        version: Option<String>,
        /// Winbindex build number; resolve its latest amd64 FileVersion (e.g. 26100)
        #[arg(
            long,
            value_name = "BUILD",
            conflicts_with_all = ["system", "dll", "pdb", "version"]
        )]
        build_latest: Option<u16>,
        /// Fetch missing DLL/PDB without confirmation
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },
    /// Fetch dwmcore.dll / PDB
    Fetch {
        #[command(subcommand)]
        command: FetchCommand,
    },
}

#[derive(Subcommand)]
enum FetchCommand {
    /// Fetch amd64 dwmcore.dll
    Dll {
        /// FileVersion like 10.0.26100.4484
        #[arg(
            long,
            required_unless_present = "build_latest",
            conflicts_with = "build_latest"
        )]
        version: Option<String>,
        /// Winbindex build number; resolve its latest amd64 FileVersion (e.g. 26100)
        #[arg(long, value_name = "BUILD", conflicts_with = "version")]
        build_latest: Option<u16>,
        /// Output directory (default: <workspace>/dwmcore)
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Fetch PDB via --version, --build-latest, or --dll (--version/--build-latest need a local DLL)
    Pdb {
        /// FileVersion like 10.0.26100.4484
        #[arg(long, conflicts_with_all = ["dll", "build_latest"])]
        version: Option<String>,
        /// Winbindex build number; resolve its latest amd64 FileVersion (e.g. 26100)
        #[arg(long, value_name = "BUILD", conflicts_with_all = ["version", "dll"])]
        build_latest: Option<u16>,
        /// Path to local dwmcore.dll
        #[arg(long, conflicts_with_all = ["version", "build_latest"])]
        dll: Option<PathBuf>,
        /// Output directory (default: <workspace>/dwmcore)
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            exit_code(error)
        }
    }
}

enum RunError {
    ProfileMismatch(String),
    Other(Box<dyn Error>),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProfileMismatch(message) => f.write_str(message),
            Self::Other(error) => write!(f, "{error}"),
        }
    }
}

impl From<Box<dyn Error>> for RunError {
    fn from(error: Box<dyn Error>) -> Self {
        Self::Other(error)
    }
}

impl From<profile::CheckError> for RunError {
    fn from(error: profile::CheckError) -> Self {
        match error {
            profile::CheckError::Mismatch(message) => Self::ProfileMismatch(message),
            profile::CheckError::Other(error) => Self::Other(error),
        }
    }
}

fn exit_code(error: RunError) -> ExitCode {
    match error {
        RunError::ProfileMismatch(_) => ExitCode::from(2),
        RunError::Other(_) => ExitCode::FAILURE,
    }
}

fn run(cli: Cli) -> Result<(), RunError> {
    match cli.command {
        Commands::Assets => Ok(assets::generate()?),
        Commands::Profile {
            command:
                ProfileCommand::Check {
                    system,
                    dll,
                    pdb,
                    version,
                    build_latest,
                    yes,
                },
        } => Ok(profile::run_check(
            system,
            dll,
            pdb,
            version,
            build_latest,
            yes,
        )?),
        Commands::Profile {
            command:
                ProfileCommand::Extract {
                    system,
                    dll,
                    pdb,
                    version,
                    build_latest,
                    yes,
                },
        } => Ok(profile::run_extract(
            system,
            dll,
            pdb,
            version,
            build_latest,
            yes,
        )?),
        Commands::Profile {
            command:
                ProfileCommand::Fetch {
                    command:
                        FetchCommand::Dll {
                            version,
                            build_latest,
                            out,
                        },
                },
        } => Ok(profile::run_fetch_dll(version, build_latest, out)?),
        Commands::Profile {
            command:
                ProfileCommand::Fetch {
                    command:
                        FetchCommand::Pdb {
                            version,
                            build_latest,
                            dll,
                            out,
                        },
                },
        } => Ok(profile::run_fetch_pdb(version, build_latest, dll, out)?),
    }
}

#[cfg(test)]
mod tests {
    use super::{RunError, exit_code};
    use crate::profile::CheckError;
    use std::process::ExitCode;

    #[test]
    fn profile_mismatch_exits_with_code_2() {
        let code = exit_code(RunError::ProfileMismatch("layout failed".into()));
        assert_eq!(code, ExitCode::from(2));
    }

    #[test]
    fn other_errors_exit_with_failure() {
        let code = exit_code(RunError::Other("network failed".into()));
        assert_eq!(code, ExitCode::FAILURE);
    }

    #[test]
    fn check_mismatch_maps_to_profile_mismatch() {
        let error = RunError::from(CheckError::Mismatch("1 layout value failed".into()));
        assert!(
            matches!(error, RunError::ProfileMismatch(ref message) if message == "1 layout value failed")
        );
        assert_eq!(exit_code(error), ExitCode::from(2));
    }

    #[test]
    fn check_other_maps_to_other() {
        let error = RunError::from(CheckError::Other("fetch failed".into()));
        assert!(matches!(error, RunError::Other(_)));
        assert_eq!(exit_code(error), ExitCode::FAILURE);
    }
}
