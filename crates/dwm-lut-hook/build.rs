use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{ID3DBlob, ID3DInclude};
use windows::core::PCSTR;

#[path = "src/blue_noise.rs"]
mod blue_noise;

const SHADER_TEMPLATE: &str = include_str!("shaders/lut_pipeline.hlsl");

fn main() {
    println!("cargo:rerun-if-changed=shaders/lut_pipeline.hlsl");
    println!("cargo:rerun-if-changed=src/blue_noise.rs");

    cc::Build::new()
        .file("src/d3d11_back_buffer_25h2.c")
        .compile("dwm_lut_d3d11_back_buffer_25h2");

    let source = SHADER_TEMPLATE.replace("__BLUE_NOISE_64X64__", &render_blue_noise_hlsl());
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"));
    compile_shader(
        &source,
        c"VS",
        c"vs_5_0",
        &out_dir.join("lut_pipeline_vs.cso"),
    );
    compile_shader(
        &source,
        c"PS",
        c"ps_5_0",
        &out_dir.join("lut_pipeline_ps.cso"),
    );
}

fn render_blue_noise_hlsl() -> String {
    let mut source = String::new();

    for (row_index, row) in blue_noise::BLUE_NOISE_64X64.iter().enumerate() {
        source.push_str("    ");
        for (column_index, value) in row.iter().enumerate() {
            if column_index > 0 {
                source.push_str(", ");
            }

            let _ = write!(&mut source, "({value}.0 + 0.5) / 256.0");
        }

        if row_index + 1 == blue_noise::BLUE_NOISE_SIZE {
            source.push('\n');
        } else {
            source.push_str(",\n");
        }
    }

    source
}

fn compile_shader(
    source: &str,
    entry: &std::ffi::CStr,
    profile: &std::ffi::CStr,
    output: &PathBuf,
) {
    let mut code = None;
    let mut errors = None;
    let result = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR(c"lut_pipeline.hlsl".as_ptr().cast()),
            None,
            None::<&ID3DInclude>,
            PCSTR(entry.as_ptr().cast()),
            PCSTR(profile.as_ptr().cast()),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };

    if let Err(error) = result {
        let diagnostics = errors
            .as_ref()
            .map(blob_text)
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| error.to_string());
        panic!(
            "failed to compile {} for {}:\n{}",
            entry.to_string_lossy(),
            profile.to_string_lossy(),
            diagnostics
        );
    }

    if let Some(diagnostics) = errors
        .as_ref()
        .map(blob_text)
        .filter(|message| !message.is_empty())
    {
        for line in diagnostics.lines() {
            println!("cargo:warning={line}");
        }
    }

    let code = code.expect("D3DCompile succeeded without shader bytecode");
    fs::write(output, blob_bytes(&code)).expect("failed to write compiled shader bytecode");
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    let size = unsafe { blob.GetBufferSize() };
    let pointer = unsafe { blob.GetBufferPointer() }.cast::<u8>();
    assert!(!pointer.is_null(), "D3DCompile returned a null buffer");
    unsafe { std::slice::from_raw_parts(pointer, size) }
}

fn blob_text(blob: &ID3DBlob) -> String {
    String::from_utf8_lossy(blob_bytes(blob))
        .trim_end_matches('\0')
        .trim()
        .to_owned()
}
