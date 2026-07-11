fn main() {
    if let Err(err) = dwm_lut::run_cli() {
        std::process::exit(dwm_lut::report_cli_error(&err));
    }
}
