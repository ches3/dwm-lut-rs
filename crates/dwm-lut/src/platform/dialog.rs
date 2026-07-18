use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};

use windows_sys::Win32::UI::Controls::{TD_ERROR_ICON, TDCBF_CLOSE_BUTTON, TaskDialog};

pub fn show_error(message: &str) {
    let title = wide_null("dwm-lut");
    let message = wide_null(message);
    unsafe {
        TaskDialog(
            null_mut(),
            null_mut(),
            title.as_ptr(),
            null(),
            message.as_ptr(),
            TDCBF_CLOSE_BUTTON,
            TD_ERROR_ICON,
            null_mut(),
        );
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
