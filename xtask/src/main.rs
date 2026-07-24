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
        #[arg(long, conflicts_with_all = ["dll", "pdb", "version"])]
        system: bool,
        /// Path to dwmcore.dll
        #[arg(long, conflicts_with_all = ["system", "version"])]
        dll: Option<PathBuf>,
        /// Path to dwmcore.pdb
        #[arg(long, conflicts_with_all = ["system", "version"])]
        pdb: Option<PathBuf>,
        /// FileVersion like 10.0.26100.4484
        #[arg(
            long,
            value_name = "FILE_VERSION",
            conflicts_with_all = ["system", "dll", "pdb"]
        )]
        version: Option<String>,
    },
    /// Extract signature/layout materials from DLL + PDB
    Extract {
        /// Use %SystemRoot%\\System32\\dwmcore.dll
        #[arg(long, conflicts_with_all = ["dll", "pdb", "version"])]
        system: bool,
        /// Path to dwmcore.dll
        #[arg(long, conflicts_with_all = ["system", "version"])]
        dll: Option<PathBuf>,
        /// Path to dwmcore.pdb
        #[arg(long, conflicts_with_all = ["system", "version"])]
        pdb: Option<PathBuf>,
        /// FileVersion like 10.0.26100.4484
        #[arg(
            long,
            value_name = "FILE_VERSION",
            conflicts_with_all = ["system", "dll", "pdb"]
        )]
        version: Option<String>,
    },
    /// Fetch dwmcore.dll / PDB (FileVersion like 10.0.26100.4484)
    Fetch {
        #[command(subcommand)]
        command: FetchCommand,
    },
}

#[derive(Subcommand)]
enum FetchCommand {
    /// Fetch amd64 dwmcore.dll for a FileVersion
    Dll {
        #[arg(long)]
        version: String,
        /// Output directory (default: <workspace>/dwmcore)
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Fetch PDB for a FileVersion (requires local DLL) or a local DLL path
    Pdb {
        #[arg(long, conflicts_with = "dll")]
        version: Option<String>,
        #[arg(long, conflicts_with = "version")]
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
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        Commands::Assets => assets::generate(),
        Commands::Profile {
            command:
                ProfileCommand::Check {
                    system,
                    dll,
                    pdb,
                    version,
                },
        } => profile::run_check(system, dll, pdb, version),
        Commands::Profile {
            command:
                ProfileCommand::Extract {
                    system,
                    dll,
                    pdb,
                    version,
                },
        } => profile::run_extract(system, dll, pdb, version),
        Commands::Profile {
            command:
                ProfileCommand::Fetch {
                    command: FetchCommand::Dll { version, out },
                },
        } => profile::run_fetch_dll(version, out),
        Commands::Profile {
            command:
                ProfileCommand::Fetch {
                    command: FetchCommand::Pdb { version, dll, out },
                },
        } => profile::run_fetch_pdb(version, dll, out),
    }
}
