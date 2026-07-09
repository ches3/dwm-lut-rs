fn main() {
    if let Err(err) = dwm_lut::run_cli() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
