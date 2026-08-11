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
