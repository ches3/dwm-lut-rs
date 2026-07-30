use std::io;

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;

pub(crate) fn current_session_id() -> Result<u32, io::Error> {
    let mut session_id = 0u32;
    let pid = unsafe { GetCurrentProcessId() };
    let ok = unsafe { ProcessIdToSessionId(pid, &mut session_id) };
    if ok == 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { GetLastError() } as i32
        ));
    }
    Ok(session_id)
}
