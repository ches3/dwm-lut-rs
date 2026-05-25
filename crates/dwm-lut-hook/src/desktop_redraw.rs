#[cfg(test)]
use std::cell::Cell;

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

#[cfg(test)]
thread_local! {
    static REQUEST_COUNT: Cell<usize> = const { Cell::new(0) };
}

#[cfg(not(test))]
#[link(name = "user32")]
unsafe extern "system" {
    fn RedrawWindow(hwnd: *mut c_void, rect: *const c_void, region: *mut c_void, flags: u32)
    -> i32;
}

pub(crate) fn request_desktop_redraw() {
    #[cfg(test)]
    {
        REQUEST_COUNT.with(|count| count.set(count.get() + 1));
    }

    #[cfg(not(test))]
    {
        let flags = RDW_INVALIDATE | RDW_INTERNALPAINT | RDW_ALLCHILDREN | RDW_UPDATENOW;
        #[cfg(debug_assertions)]
        {
            let result =
                unsafe { RedrawWindow(ptr::null_mut(), ptr::null(), ptr::null_mut(), flags) };
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
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    REQUEST_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn request_count_for_tests() -> usize {
    REQUEST_COUNT.with(Cell::get)
}
