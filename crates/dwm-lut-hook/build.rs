fn main() {
    cc::Build::new()
        .file("src/d3d11_back_buffer_25h2.c")
        .compile("dwm_lut_d3d11_back_buffer_25h2");
}
