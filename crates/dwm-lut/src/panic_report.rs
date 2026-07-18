use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use windows_sys::Win32::Foundation::{FALSE, HANDLE};
use windows_sys::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};

use crate::error::InjectorError;

pub const PANIC_EXIT_CODE: i32 = 101;
const STATE_STANDALONE: u8 = 0;
const STATE_STARTING: u8 = 1;
const STATE_INITIATOR_REPORTING: u8 = 2;
const STATE_BACKGROUND_REPORTING_PANIC: u8 = 3;
const STATE_RUNNING: u8 = 4;

static PANIC_REPORTED: AtomicBool = AtomicBool::new(false);
static REPORT_STATE: AtomicU8 = AtomicU8::new(STATE_STANDALONE);
static STARTUP_EVENTS: OnceLock<StartupEvents> = OnceLock::new();

struct StartupEvents {
    panic: EventHandle,
}

struct EventHandle(OwnedHandle);

impl EventHandle {
    fn open(name: &str, access: u32, operation: &'static str) -> Result<Self, InjectorError> {
        let name = wide_null(name);
        let handle = unsafe { OpenEventW(access, FALSE, name.as_ptr()) };
        if handle.is_null() {
            return Err(InjectorError::HostLaunchFailed {
                operation,
                source: std::io::Error::last_os_error(),
            });
        }
        // SAFETY: OpenEventW returned an owned event handle that must be closed.
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle) }))
    }

    fn signal(&self) -> bool {
        (unsafe { SetEvent(self.raw()) }) != FALSE
    }

    fn raw(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanicReportAction {
    ShowDialog,
    SuppressDialog,
    AlreadyReported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupChannelFailureAction {
    Exit,
    ClaimAndExit,
    Ignore,
}

pub fn configure(panic_event_name: Option<&str>) -> Result<(), InjectorError> {
    let Some(panic_event_name) = panic_event_name else {
        return Ok(());
    };
    let panic = EventHandle::open(
        panic_event_name,
        EVENT_MODIFY_STATE,
        "open startup panic event",
    )?;
    STARTUP_EVENTS.set(StartupEvents { panic }).map_err(|_| {
        InjectorError::HostStartupFailed("startup reporting was already configured".to_string())
    })?;
    REPORT_STATE.store(STATE_STARTING, Ordering::Release);
    Ok(())
}

pub(crate) fn abort_startup() {
    loop {
        let state = REPORT_STATE.load(Ordering::Acquire);
        match startup_channel_failure_action(state) {
            StartupChannelFailureAction::Exit => std::process::exit(1),
            StartupChannelFailureAction::ClaimAndExit => {
                if REPORT_STATE
                    .compare_exchange(
                        STATE_STARTING,
                        STATE_INITIATOR_REPORTING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    std::process::exit(1);
                }
            }
            StartupChannelFailureAction::Ignore => return,
        }
    }
}

fn startup_channel_failure_action(state: u8) -> StartupChannelFailureAction {
    match state {
        STATE_INITIATOR_REPORTING => StartupChannelFailureAction::Exit,
        STATE_STARTING => StartupChannelFailureAction::ClaimAndExit,
        _ => StartupChannelFailureAction::Ignore,
    }
}

pub fn begin_panic_report() -> PanicReportAction {
    if PANIC_REPORTED.swap(true, Ordering::AcqRel) {
        return PanicReportAction::AlreadyReported;
    }

    loop {
        match REPORT_STATE.load(Ordering::Acquire) {
            STATE_STANDALONE | STATE_RUNNING => return PanicReportAction::ShowDialog,
            STATE_INITIATOR_REPORTING => return PanicReportAction::SuppressDialog,
            STATE_BACKGROUND_REPORTING_PANIC => return PanicReportAction::AlreadyReported,
            STATE_STARTING => {
                if REPORT_STATE
                    .compare_exchange(
                        STATE_STARTING,
                        STATE_BACKGROUND_REPORTING_PANIC,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    return if signal_panic_event() {
                        PanicReportAction::ShowDialog
                    } else {
                        PanicReportAction::SuppressDialog
                    };
                }
            }
            _ => return PanicReportAction::SuppressDialog,
        }
    }
}

pub(crate) fn claim_startup_failure() -> bool {
    loop {
        match REPORT_STATE.load(Ordering::Acquire) {
            STATE_STANDALONE | STATE_INITIATOR_REPORTING => return true,
            STATE_BACKGROUND_REPORTING_PANIC | STATE_RUNNING => return false,
            STATE_STARTING => {
                if REPORT_STATE
                    .compare_exchange(
                        STATE_STARTING,
                        STATE_INITIATOR_REPORTING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    return true;
                }
            }
            _ => return false,
        }
    }
}

pub(crate) fn complete_startup() -> Result<(), InjectorError> {
    match REPORT_STATE.compare_exchange(
        STATE_STARTING,
        STATE_RUNNING,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => Ok(()),
        Err(STATE_STANDALONE) | Err(STATE_RUNNING) => Ok(()),
        Err(STATE_BACKGROUND_REPORTING_PANIC) => Err(InjectorError::HostPanicAlreadyReported),
        Err(_) => Err(InjectorError::HostStartupFailed(
            "startup reporting ownership was already committed".to_string(),
        )),
    }
}

pub(crate) fn was_reported() -> bool {
    PANIC_REPORTED.load(Ordering::Acquire)
}

fn signal_panic_event() -> bool {
    let Some(events) = STARTUP_EVENTS.get() else {
        return false;
    };
    events.panic.signal()
}

fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicU64, Ordering};

    use windows_sys::Win32::Foundation::{TRUE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    use super::*;

    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    static NEXT_EVENT_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn opened_event_remains_valid_after_creator_handle_is_closed() {
        let event_name = format!(
            "Local\\dwm-lut-rs-panic-report-test-{}-{}",
            std::process::id(),
            NEXT_EVENT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let event_name_wide = wide_null(&event_name);
        let creator = unsafe { CreateEventW(null_mut(), TRUE, FALSE, event_name_wide.as_ptr()) };
        assert!(!creator.is_null());
        // SAFETY: CreateEventW returned an owned event handle that must be closed.
        let creator = unsafe { OwnedHandle::from_raw_handle(creator) };

        let opened = EventHandle::open(
            &event_name,
            EVENT_MODIFY_STATE | SYNCHRONIZE_ACCESS,
            "open test startup event",
        )
        .expect("event should open while creator owns it");
        drop(creator);

        assert!(opened.signal());
        assert_eq!(
            unsafe { WaitForSingleObject(opened.raw(), 0) },
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn startup_channel_failure_exits_only_before_startup_commit() {
        assert_eq!(
            startup_channel_failure_action(STATE_STARTING),
            StartupChannelFailureAction::ClaimAndExit
        );
        assert_eq!(
            startup_channel_failure_action(STATE_INITIATOR_REPORTING),
            StartupChannelFailureAction::Exit
        );
        assert_eq!(
            startup_channel_failure_action(STATE_BACKGROUND_REPORTING_PANIC),
            StartupChannelFailureAction::Ignore
        );
        assert_eq!(
            startup_channel_failure_action(STATE_RUNNING),
            StartupChannelFailureAction::Ignore
        );
    }
}
