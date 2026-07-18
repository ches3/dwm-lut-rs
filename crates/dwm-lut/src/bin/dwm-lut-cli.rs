fn main() {
    let command = dwm_lut::cli::parse_args().unwrap_or_else(|error| error.exit());
    if let Err(err) = dwm_lut::run_cli(command) {
        std::process::exit(dwm_lut::report_cli_error(&err));
    }
}
