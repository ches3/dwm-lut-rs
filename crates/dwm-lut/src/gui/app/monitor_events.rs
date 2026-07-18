use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            DBT_CONFIGCHANGED, DBT_DEVNODES_CHANGED, IsWindow, WM_DEVICECHANGE, WM_DISPLAYCHANGE,
        },
    },
};

pub(super) const SETTLE_DELAY: Duration = Duration::from_millis(250);
pub(super) const RETRY_DELAY: Duration = Duration::from_secs(1);
const MONITOR_CHANGE_SUBCLASS_ID: usize = 1;

pub(super) struct MonitorChangeListener {
    hwnd: HWND,
    signal: Option<Arc<MonitorChangeSignal>>,
}

impl MonitorChangeListener {
    pub(super) fn attach(
        context: &eframe::CreationContext<'_>,
        signal: Arc<MonitorChangeSignal>,
    ) -> io::Result<Self> {
        let window_handle = context
            .window_handle()
            .map_err(|error| io::Error::other(format!("get host window handle: {error}")))?;
        let RawWindowHandle::Win32(window_handle) = window_handle.as_raw() else {
            return Err(io::Error::other(
                "host UI did not provide a Win32 window handle",
            ));
        };
        let hwnd = window_handle.hwnd.get() as HWND;
        let signal_pointer = Arc::as_ptr(&signal) as usize;

        // SAFETY: the HWND belongs to this UI thread, the callback has the required ABI,
        // and `signal` is retained by the listener while the subclass can call it.
        let result = unsafe {
            SetWindowSubclass(
                hwnd,
                Some(monitor_change_window_proc),
                MONITOR_CHANGE_SUBCLASS_ID,
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

impl Drop for MonitorChangeListener {
    fn drop(&mut self) {
        // SAFETY: this removes the callback installed by `attach` from the same HWND.
        let removed = unsafe {
            RemoveWindowSubclass(
                self.hwnd,
                Some(monitor_change_window_proc),
                MONITOR_CHANGE_SUBCLASS_ID,
            )
        };
        if removed == 0 {
            // SAFETY: querying whether the stored HWND still identifies a live window does not
            // dereference application memory.
            let window_is_alive = unsafe { IsWindow(self.hwnd) } != 0;
            if window_is_alive {
                // The callback may still be registered. Keep its reference data alive rather than
                // allowing a later window message to dereference freed memory.
                if let Some(signal) = self.signal.take() {
                    std::mem::forget(signal);
                }
            }
        }
    }
}

pub(super) struct MonitorChangeSignal {
    pending: AtomicBool,
    context: OnceLock<egui::Context>,
}

impl MonitorChangeSignal {
    pub(super) fn new() -> Self {
        Self {
            pending: AtomicBool::new(false),
            context: OnceLock::new(),
        }
    }

    pub(super) fn set_context(&self, context: &egui::Context) {
        let _ = self.context.set(context.clone());
    }

    pub(super) fn notify(&self) {
        self.pending.store(true, Ordering::Release);
        if let Some(context) = self.context.get() {
            context.request_repaint();
        }
    }

    pub(super) fn take(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }
}

unsafe extern "system" fn monitor_change_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference_data: usize,
) -> LRESULT {
    if is_monitor_change_message(message, wparam) {
        // SAFETY: `reference_data` points to the signal retained by MonitorChangeListener.
        let signal = unsafe { &*(reference_data as *const MonitorChangeSignal) };
        let _ = catch_unwind(AssertUnwindSafe(|| signal.notify()));
    }

    // SAFETY: all messages must continue through the existing subclass chain.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn is_monitor_change_message(message: u32, wparam: WPARAM) -> bool {
    message == WM_DISPLAYCHANGE
        || (message == WM_DEVICECHANGE
            && matches!(wparam as u32, DBT_DEVNODES_CHANGED | DBT_CONFIGCHANGED))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_relevant_display_and_device_change_messages() {
        assert!(is_monitor_change_message(WM_DISPLAYCHANGE, 0));
        assert!(is_monitor_change_message(
            WM_DEVICECHANGE,
            DBT_DEVNODES_CHANGED as usize
        ));
        assert!(is_monitor_change_message(
            WM_DEVICECHANGE,
            DBT_CONFIGCHANGED as usize
        ));
        assert!(!is_monitor_change_message(WM_DEVICECHANGE, 0));
    }
}
