use std::io;
use std::mem::size_of;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_NOT_ALL_ASSIGNED, FALSE, LUID};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_DEBUG_NAME,
    SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_CREATE_THREAD,
    PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
};

use crate::error::{InjectionStep, InjectorError};

use super::remote::OwnedHandle;
use super::{create_toolhelp_snapshot, is_no_more_files_error, last_os_error, utf16_to_string};

pub(crate) fn open_target_process(pid: u32) -> Result<OwnedHandle, InjectorError> {
    let handle = unsafe {
        OpenProcess(
            PROCESS_CREATE_THREAD
                | PROCESS_QUERY_INFORMATION
                | PROCESS_VM_OPERATION
                | PROCESS_VM_WRITE
                | PROCESS_VM_READ,
            FALSE,
            pid,
        )
    };
    if handle.is_null() {
        return Err(classify_open_target_process_error(pid, last_os_error()));
    }

    OwnedHandle::new(handle, InjectionStep::OpenTargetProcess)
}

pub(crate) fn enable_debug_privilege() -> Result<(), InjectorError> {
    let mut token = null_mut();
    let ok = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    };
    if ok == FALSE {
        return Err(InjectorError::StepFailed {
            step: InjectionStep::EnableDebugPrivilege,
            source: last_os_error(),
        });
    }
    let token = OwnedHandle::new(token, InjectionStep::EnableDebugPrivilege)?;

    let mut luid = LUID {
        LowPart: 0,
        HighPart: 0,
    };
    let ok = unsafe { LookupPrivilegeValueW(null(), SE_DEBUG_NAME, &mut luid) };
    if ok == FALSE {
        return Err(InjectorError::StepFailed {
            step: InjectionStep::EnableDebugPrivilege,
            source: last_os_error(),
        });
    }

    let state = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    unsafe {
        windows_sys::Win32::Foundation::SetLastError(0);
    }
    let ok =
        unsafe { AdjustTokenPrivileges(token.raw(), FALSE, &state, 0, null_mut(), null_mut()) };
    if ok == FALSE {
        return Err(InjectorError::StepFailed {
            step: InjectionStep::EnableDebugPrivilege,
            source: last_os_error(),
        });
    }

    finalize_debug_privilege_adjustment(last_os_error())
}

pub(crate) fn find_process_id_by_name(name: &str) -> Result<u32, InjectorError> {
    let current_session_id = current_session_id()?;
    let snapshot = create_process_snapshot()?;

    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        cntUsage: 0,
        th32ProcessID: 0,
        th32DefaultHeapID: 0,
        th32ModuleID: 0,
        cntThreads: 0,
        th32ParentProcessID: 0,
        pcPriClassBase: 0,
        dwFlags: 0,
        szExeFile: [0; 260],
    };

    let mut has_entry = unsafe { Process32FirstW(snapshot.raw(), &mut entry) };
    if has_entry == FALSE {
        let error = last_os_error();
        if is_no_more_files_error(&error) {
            return Err(InjectorError::DwmProcessNotFound);
        }
        return Err(InjectorError::StepFailed {
            step: InjectionStep::FindDwmProcess,
            source: error,
        });
    }

    let mut candidates = Vec::new();
    loop {
        candidates.push((entry.th32ProcessID, utf16_to_string(&entry.szExeFile)));
        has_entry = unsafe { Process32NextW(snapshot.raw(), &mut entry) };
        if has_entry == FALSE {
            let error = last_os_error();
            if is_no_more_files_error(&error) {
                break;
            }
            return Err(InjectorError::StepFailed {
                step: InjectionStep::FindDwmProcess,
                source: error,
            });
        }
    }

    select_process_id(candidates, name, current_session_id, process_session_id)?
        .ok_or(InjectorError::DwmProcessNotFound)
}

fn create_process_snapshot() -> Result<OwnedHandle, InjectorError> {
    create_toolhelp_snapshot(TH32CS_SNAPPROCESS, 0, InjectionStep::FindDwmProcess, false)
}

fn select_process_id<I, F>(
    entries: I,
    target_name: &str,
    current_session_id: u32,
    mut session_for_pid: F,
) -> Result<Option<u32>, InjectorError>
where
    I: IntoIterator<Item = (u32, String)>,
    F: FnMut(u32) -> Result<u32, InjectorError>,
{
    for (pid, process_name) in entries {
        if process_name.eq_ignore_ascii_case(target_name)
            && session_for_pid(pid)? == current_session_id
        {
            return Ok(Some(pid));
        }
    }

    Ok(None)
}

fn classify_open_target_process_error(pid: u32, error: io::Error) -> InjectorError {
    if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) {
        InjectorError::TargetAccessDenied { pid }
    } else {
        InjectorError::StepFailed {
            step: InjectionStep::OpenTargetProcess,
            source: error,
        }
    }
}

fn finalize_debug_privilege_adjustment(error: io::Error) -> Result<(), InjectorError> {
    if error.raw_os_error() == Some(ERROR_NOT_ALL_ASSIGNED as i32) {
        return Err(InjectorError::DebugPrivilegeUnavailable);
    }

    Ok(())
}

fn current_session_id() -> Result<u32, InjectorError> {
    let pid = unsafe { GetCurrentProcessId() };
    process_session_id_with_step(pid, InjectionStep::ResolveCurrentSession)
}

fn process_session_id(pid: u32) -> Result<u32, InjectorError> {
    process_session_id_with_step(pid, InjectionStep::FindDwmProcess)
}

fn process_session_id_with_step(pid: u32, step: InjectionStep) -> Result<u32, InjectorError> {
    let mut session_id = 0u32;
    let ok = unsafe { ProcessIdToSessionId(pid, &mut session_id) };
    if ok == FALSE {
        return Err(InjectorError::StepFailed {
            step,
            source: last_os_error(),
        });
    }

    Ok(session_id)
}

#[cfg(test)]
mod tests {
    use std::io;

    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_NOT_ALL_ASSIGNED};

    use crate::error::InjectorError;

    use super::{
        classify_open_target_process_error, finalize_debug_privilege_adjustment, select_process_id,
    };

    #[test]
    fn selects_named_process_from_current_session() {
        let pid = select_process_id(
            vec![
                (100, "dwm.exe".to_string()),
                (200, "notepad.exe".to_string()),
                (300, "DWM.EXE".to_string()),
            ],
            "dwm.exe",
            2,
            |pid| match pid {
                100 => Ok(1),
                200 => Ok(2),
                300 => Ok(2),
                other => panic!("unexpected pid: {other}"),
            },
        )
        .expect("session lookup should succeed");

        assert_eq!(pid, Some(300));
    }
    #[test]
    fn maps_access_denied_to_target_access_denied() {
        let error = classify_open_target_process_error(
            4242,
            io::Error::from_raw_os_error(ERROR_ACCESS_DENIED as i32),
        );

        assert!(matches!(
            error,
            InjectorError::TargetAccessDenied { pid: 4242 }
        ));
    }

    #[test]
    fn maps_not_all_assigned_to_debug_privilege_unavailable() {
        let error = finalize_debug_privilege_adjustment(io::Error::from_raw_os_error(
            ERROR_NOT_ALL_ASSIGNED as i32,
        ))
        .expect_err("missing privilege assignment must be rejected");

        assert!(matches!(error, InjectorError::DebugPrivilegeUnavailable));
    }
}
