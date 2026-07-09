mod background;
mod export;
mod module;
mod process;
mod remote;

use std::io;

use windows_sys::Win32::Foundation::{ERROR_BAD_LENGTH, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::CreateToolhelp32Snapshot;

use crate::error::{InjectionStep, InjectorError};

pub(crate) use background::{StartupNotifier, start_background_host};
pub(crate) use export::{resolve_remote_export_address, resolve_remote_module_export_address};
pub(crate) use module::{
    NamedRemoteModule, RemoteModule, find_remote_module, find_remote_modules_by_name,
};
pub(crate) use process::{enable_debug_privilege, find_process_id_by_name, open_target_process};
pub(crate) use remote::{OwnedHandle, RemoteAllocation, run_remote_thread, wide_null};

fn create_toolhelp_snapshot(
    flags: u32,
    pid: u32,
    step: InjectionStep,
    retry_bad_length: bool,
) -> Result<OwnedHandle, InjectorError> {
    const MAX_SNAPSHOT_RETRIES: usize = 8;

    for attempt in 0..MAX_SNAPSHOT_RETRIES {
        let snapshot = unsafe { CreateToolhelp32Snapshot(flags, pid) };
        if !snapshot.is_null() && snapshot != INVALID_HANDLE_VALUE {
            return OwnedHandle::new(snapshot, step);
        }

        let error = last_os_error();
        if retry_bad_length
            && error.raw_os_error() == Some(ERROR_BAD_LENGTH as i32)
            && attempt + 1 < MAX_SNAPSHOT_RETRIES
        {
            continue;
        }

        return Err(InjectorError::StepFailed {
            step,
            source: error,
        });
    }

    Err(InjectorError::StepFailed {
        step,
        source: io::Error::from_raw_os_error(ERROR_BAD_LENGTH as i32),
    })
}

fn last_os_error() -> io::Error {
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() } as i32;
    io::Error::from_raw_os_error(code)
}

fn is_no_more_files_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32)
}

fn utf16_to_string(buffer: &[u16]) -> String {
    let len = buffer
        .iter()
        .position(|&value| value == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}
