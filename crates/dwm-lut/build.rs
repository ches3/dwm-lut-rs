fn main() {
    println!("cargo::rustc-link-arg-bin=dwm-lut=/MANIFEST:EMBED");
    println!(
        "cargo::rustc-link-arg-bin=dwm-lut=/MANIFESTUAC:level='requireAdministrator' uiAccess='false'"
    );
    println!("cargo::rustc-link-arg-bin=dwm-lut-cli=/MANIFEST:EMBED");
    println!(
        "cargo::rustc-link-arg-bin=dwm-lut-cli=/MANIFESTUAC:level='asInvoker' uiAccess='false'"
    );
}
