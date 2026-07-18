#[cfg(not(test))]
use windows::Win32::Graphics::Gdi::{
    RDW_ALLCHILDREN, RDW_INTERNALPAINT, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow,
};

#[cfg(not(test))]
pub(crate) fn request_desktop_redraw() {
    let flags = RDW_INVALIDATE | RDW_INTERNALPAINT | RDW_ALLCHILDREN | RDW_UPDATENOW;
    #[cfg(debug_assertions)]
    {
        let result = unsafe { RedrawWindow(None, None, None, flags) };
        debug_log!(
            "event=desktop_redraw_requested result={} flags=0x{:x}",
            result.0,
            flags.0
        );
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = unsafe { RedrawWindow(None, None, None, flags) };
    }
}

#[cfg(test)]
pub(crate) fn request_desktop_redraw() {}
