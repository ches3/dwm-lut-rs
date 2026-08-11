use std::env;

fn main() {
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // xtask is run directly and is never linked through an import library.
        println!("cargo::rustc-link-arg-bin=xtask=/NOIMPLIB");
        println!("cargo::rustc-link-arg-bin=xtask=/NOEXP");
    }
}
