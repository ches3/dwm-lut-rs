use std::error::Error;
use std::io;
use std::process::ExitCode;

mod assets;

const HELP: &str = "Usage: cargo xtask <COMMAND>\n\nCommands:\n  assets  Generate application icon assets\n\nOptions:\n  -h, --help  Print help";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("\n{HELP}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [command] if command == "assets" => assets::generate(),
        [flag] if flag == "-h" || flag == "--help" => {
            println!("{HELP}");
            Ok(())
        }
        [] => Err(io::Error::new(io::ErrorKind::InvalidInput, "missing command").into()),
        [command, ..] => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown command or arguments: {command}"),
        )
        .into()),
    }
}
