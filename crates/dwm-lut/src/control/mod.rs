use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;

use crate::error::InjectorError;

pub(crate) mod client;
pub(crate) mod protocol;
pub(crate) mod server;

pub(crate) fn current_pipe_name() -> Result<String, InjectorError> {
    let session_id = current_session_id()?;
    Ok(format!(r"\\.\pipe\dwm-lut-rs-{session_id}"))
}

fn current_session_id() -> Result<u32, InjectorError> {
    let mut session_id = 0u32;
    let pid = unsafe { GetCurrentProcessId() };
    let ok = unsafe { ProcessIdToSessionId(pid, &mut session_id) };
    if ok == 0 {
        return Err(InjectorError::ControlPipe {
            operation: "resolve current session",
            source: last_os_error(),
        });
    }

    Ok(session_id)
}

pub(crate) fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub(crate) fn last_os_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}
