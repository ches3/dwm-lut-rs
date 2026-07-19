use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{IsWindow, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN},
    },
};

const MOUSE_FOCUS_SUBCLASS_ID: usize = 2;

pub(crate) struct MouseFocusDismissListener {
    hwnd: HWND,
    signal: Option<Arc<MouseFocusDismissSignal>>,
}

impl MouseFocusDismissListener {
    pub(crate) fn attach(hwnd: HWND, signal: Arc<MouseFocusDismissSignal>) -> io::Result<Self> {
        let signal_pointer = Arc::as_ptr(&signal) as usize;

        // SAFETY: the HWND belongs to this UI thread, the callback has the required ABI,
        // and `signal` is retained by the listener while the subclass can call it.
        let result = unsafe {
            SetWindowSubclass(
                hwnd,
                Some(mouse_focus_window_proc),
                MOUSE_FOCUS_SUBCLASS_ID,
                signal_pointer,
            )
        };
        if result == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            hwnd,
            signal: Some(signal),
        })
    }
}

impl Drop for MouseFocusDismissListener {
    fn drop(&mut self) {
        // SAFETY: this removes the callback installed by `attach` from the same HWND.
        let removed = unsafe {
            RemoveWindowSubclass(
                self.hwnd,
                Some(mouse_focus_window_proc),
                MOUSE_FOCUS_SUBCLASS_ID,
            )
        };
        if removed == 0 {
            // SAFETY: querying whether the stored HWND still identifies a live window does not
            // dereference application memory.
            let window_is_alive = unsafe { IsWindow(self.hwnd) } != 0;
            if window_is_alive && let Some(signal) = self.signal.take() {
                std::mem::forget(signal);
            }
        }
    }
}

pub(crate) struct MouseFocusDismissSignal {
    dismiss: Arc<dyn Fn() + Send + Sync>,
}

impl MouseFocusDismissSignal {
    pub(crate) fn new(dismiss: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { dismiss }
    }

    fn notify(&self) {
        (self.dismiss)();
    }
}

unsafe extern "system" fn mouse_focus_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    if is_client_left_button_down(message) {
        // SAFETY: `reference_data` points to the signal retained by MouseFocusDismissListener.
        let signal = unsafe { &*(reference_data as *const MouseFocusDismissSignal) };
        let _ = catch_unwind(AssertUnwindSafe(|| signal.notify()));
    }

    // SAFETY: all messages must continue through the existing subclass chain.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn is_client_left_button_down(message: u32) -> bool {
    matches!(message, WM_LBUTTONDOWN | WM_LBUTTONDBLCLK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_client_left_button_down_messages() {
        assert!(is_client_left_button_down(WM_LBUTTONDOWN));
        assert!(is_client_left_button_down(WM_LBUTTONDBLCLK));
        assert!(!is_client_left_button_down(0));
    }
}
