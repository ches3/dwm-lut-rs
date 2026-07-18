use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, INVALID_HANDLE_VALUE, SetLastError, WAIT_ABANDONED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

use crate::control::current_pipe_name;
use crate::error::InjectorError;
use crate::security::{SecurityDescriptor, UserSid};

pub(crate) enum HostInstanceClaim {
    Acquired(HostInstanceGuard),
    Contended(HostInstanceWaiter),
}

pub(crate) struct HostInstanceGuard(OwnedHandle);

pub(crate) struct HostInstanceWaiter(Option<OwnedHandle>);

impl HostInstanceGuard {
    pub(crate) fn claim() -> Result<HostInstanceClaim, InjectorError> {
        claim_host_instance(&host_mutex_name_for_current_session()?)
    }
}

fn host_mutex_name_for_current_session() -> Result<String, InjectorError> {
    let pipe_name = current_pipe_name()?;
    Ok(pipe_name.replace(r"\\.\pipe\", r"Local\"))
}

fn claim_host_instance(mutex_name: &str) -> Result<HostInstanceClaim, InjectorError> {
    let mutex_name = wide_null(mutex_name);
    let user_sid = UserSid::current_process()?;
    let security_descriptor = SecurityDescriptor::full_access_for_user(&user_sid)?;
    let security_attributes = security_descriptor.as_security_attributes();
    unsafe {
        SetLastError(0);
    }
    let handle = unsafe { CreateMutexW(&security_attributes, 1, mutex_name.as_ptr()) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(InjectorError::ControlPipe {
            operation: "create host instance mutex",
            source: last_os_error(),
        });
    }

    let error = last_os_error();
    // SAFETY: CreateMutexW returned an owned mutex handle that must be closed.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    if error.raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32) {
        return Ok(HostInstanceClaim::Contended(HostInstanceWaiter(Some(
            handle,
        ))));
    }
    Ok(HostInstanceClaim::Acquired(HostInstanceGuard(handle)))
}

impl HostInstanceWaiter {
    pub(crate) fn wait(
        &mut self,
        timeout_ms: u32,
    ) -> Result<Option<HostInstanceGuard>, InjectorError> {
        let handle = self
            .0
            .as_ref()
            .expect("host instance waiter must own its mutex");
        match unsafe { WaitForSingleObject(handle.as_raw_handle(), timeout_ms) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => {
                let handle = self
                    .0
                    .take()
                    .expect("host instance waiter must own its mutex");
                Ok(Some(HostInstanceGuard(handle)))
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(InjectorError::ControlPipe {
                operation: "wait for host instance mutex",
                source: last_os_error(),
            }),
        }
    }
}

impl Drop for HostInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0.as_raw_handle());
        }
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn last_os_error() -> io::Error {
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() } as i32;
    io::Error::from_raw_os_error(code)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn waiter_acquires_mutex_after_owner_releases_it() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let mutex_name = format!(r"Local\dwm-lut-rs-test-{}-{unique}", std::process::id());
        let guard = match claim_host_instance(&mutex_name).expect("mutex claim should succeed") {
            HostInstanceClaim::Acquired(guard) => guard,
            HostInstanceClaim::Contended(_) => panic!("unique mutex should not be contended"),
        };
        let (ready_sender, ready_receiver) = mpsc::channel();
        let (acquired_sender, acquired_receiver) = mpsc::channel();

        let worker = std::thread::spawn(move || {
            let mut waiter = match claim_host_instance(&mutex_name)
                .expect("second mutex claim should succeed")
            {
                HostInstanceClaim::Acquired(_) => panic!("owned mutex should be contended"),
                HostInstanceClaim::Contended(waiter) => waiter,
            };
            ready_sender.send(()).expect("ready signal should send");
            let acquired = waiter
                .wait(5_000)
                .expect("mutex wait should succeed")
                .expect("mutex should become available");
            acquired_sender
                .send(())
                .expect("acquired signal should send");
            drop(acquired);
        });

        ready_receiver.recv().expect("worker should become ready");
        drop(guard);
        acquired_receiver
            .recv()
            .expect("worker should acquire released mutex");
        worker.join().expect("worker should complete");
    }
}
