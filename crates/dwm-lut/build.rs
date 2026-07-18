use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-link-arg-bin=dwm-lut=/MANIFEST:EMBED");
    println!("cargo::rustc-link-arg-bin=dwm-lut=/MANIFESTUAC:level='asInvoker' uiAccess='false'");
    println!(
        "cargo::rustc-link-arg-bin=dwm-lut=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
    );
    println!("cargo::rustc-link-arg-bin=dwm-lut-cli=/MANIFEST:EMBED");
    println!(
        "cargo::rustc-link-arg-bin=dwm-lut-cli=/MANIFESTUAC:level='asInvoker' uiAccess='false'"
    );

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon_path = manifest_dir.join("assets/icon.ico");
    println!("cargo:rerun-if-changed={}", icon_path.display());

    if env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS") == "windows" {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(icon_path.to_str().expect("icon path is valid UTF-8"));
        resource
            .compile()
            .unwrap_or_else(|error| panic!("failed to embed application icon: {error}"));
    }
}
