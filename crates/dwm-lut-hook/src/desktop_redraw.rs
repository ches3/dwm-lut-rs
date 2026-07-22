#[cfg(not(test))]
use windows::Win32::Graphics::Gdi::{
    RDW_ALLCHILDREN, RDW_INTERNALPAINT, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow,
};

#[cfg(not(test))]
pub(crate) fn request_desktop_redraw() {
    let flags = RDW_INVALIDATE | RDW_INTERNALPAINT | RDW_ALLCHILDREN | RDW_UPDATENOW;
    let result = unsafe { RedrawWindow(None, None, None, flags) };
    crate::log::desktop_redraw_requested(result.0, flags.0);
}

#[cfg(test)]
pub(crate) fn request_desktop_redraw() {}
