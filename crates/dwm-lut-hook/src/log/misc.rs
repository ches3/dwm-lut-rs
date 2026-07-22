#![cfg_attr(not(debug_assertions), allow(dead_code))]

pub(crate) fn desktop_redraw_requested(result: i32, flags: u32) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=desktop_redraw_requested result={result} flags=0x{flags:x}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (result, flags);
}

pub(crate) fn back_buffer_identity_fallback(reason: &str, back_buffer: usize, identity: usize) {
    #[cfg(debug_assertions)]
    {
        super::write(format_args!(
            "event=back_buffer_identity_fallback reason={reason} back_buffer=0x{back_buffer:x} identity=0x{identity:x}"
        ));
    }
    #[cfg(not(debug_assertions))]
    let _ = (reason, back_buffer, identity);
}
