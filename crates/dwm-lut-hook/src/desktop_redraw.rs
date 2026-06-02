#[cfg(not(test))]
use std::ffi::c_void;
#[cfg(not(test))]
use std::ptr;

#[cfg(not(test))]
const RDW_INVALIDATE: u32 = 0x0001;
#[cfg(not(test))]
const RDW_INTERNALPAINT: u32 = 0x0002;
#[cfg(not(test))]
const RDW_ALLCHILDREN: u32 = 0x0080;
#[cfg(not(test))]
const RDW_UPDATENOW: u32 = 0x0100;

#[cfg(not(test))]
#[link(name = "user32")]
unsafe extern "system" {
    fn RedrawWindow(hwnd: *mut c_void, rect: *const c_void, region: *mut c_void, flags: u32)
    -> i32;
}

#[cfg(not(test))]
pub(crate) fn request_desktop_redraw() {
    let flags = RDW_INVALIDATE | RDW_INTERNALPAINT | RDW_ALLCHILDREN | RDW_UPDATENOW;
    #[cfg(debug_assertions)]
    {
        let result = unsafe { RedrawWindow(ptr::null_mut(), ptr::null(), ptr::null_mut(), flags) };
        debug_log!(
            "event=desktop_redraw_requested result={} flags=0x{:x}",
            result,
            flags
        );
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = unsafe { RedrawWindow(ptr::null_mut(), ptr::null(), ptr::null_mut(), flags) };
    }
}

#[cfg(test)]
pub(crate) fn request_desktop_redraw() {}
