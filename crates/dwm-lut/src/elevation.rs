use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_CANCELLED, FALSE, HANDLE};
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE, TOKEN_QUERY, TokenElevation,
    TokenElevationType,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};

pub(crate) fn is_process_elevated() -> io::Result<bool> {
    let token = current_process_token()?;
    let mut elevation = TOKEN_ELEVATION::default();
    get_token_information(
        &token,
        TokenElevation,
        (&mut elevation as *mut TOKEN_ELEVATION).cast(),
        std::mem::size_of::<TOKEN_ELEVATION>() as u32,
    )?;
    Ok(elevation.TokenIsElevated != 0)
}

pub(crate) fn current_token_elevation_type() -> io::Result<TOKEN_ELEVATION_TYPE> {
    let token = current_process_token()?;
    let mut elevation_type: TOKEN_ELEVATION_TYPE = 0;
    get_token_information(
        &token,
        TokenElevationType,
        (&mut elevation_type as *mut TOKEN_ELEVATION_TYPE).cast(),
        std::mem::size_of::<TOKEN_ELEVATION_TYPE>() as u32,
    )?;
    Ok(elevation_type)
}

pub(crate) fn run_as(executable: &Path, parameters: &[u16]) -> Result<ElevatedProcess, RunAsError> {
    let executable = wide_null(executable.as_os_str());
    let verb = wide_null(OsStr::new("runas"));
    let mut parameters = parameters.to_vec();
    parameters.push(0);
    let mut execute = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        hwnd: null_mut(),
        lpVerb: verb.as_ptr(),
        lpFile: executable.as_ptr(),
        lpParameters: parameters.as_ptr(),
        lpDirectory: null(),
        nShow: 0,
        hInstApp: null_mut(),
        lpIDList: null_mut(),
        lpClass: null(),
        hkeyClass: null_mut(),
        dwHotKey: 0,
        Anonymous: Default::default(),
        hProcess: null_mut(),
    };

    if unsafe { ShellExecuteExW(&mut execute) } == FALSE {
        let source = io::Error::last_os_error();
        if source.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            return Err(RunAsError::Cancelled);
        }
        return Err(RunAsError::Launch(source));
    }
    if execute.hProcess.is_null() {
        return Err(RunAsError::MissingProcessHandle);
    }
    // SAFETY: ShellExecuteExW returned an owned process handle that must be closed.
    Ok(ElevatedProcess(unsafe {
        OwnedHandle::from_raw_handle(execute.hProcess)
    }))
}

fn current_process_token() -> io::Result<OwnedHandle> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == FALSE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcessToken returned an owned token handle that must be closed.
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}

fn get_token_information(
    token: &OwnedHandle,
    information_class: windows_sys::Win32::Security::TOKEN_INFORMATION_CLASS,
    information: *mut std::ffi::c_void,
    information_len: u32,
) -> io::Result<()> {
    let mut returned_len = 0;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            information_class,
            information,
            information_len,
            &mut returned_len,
        )
    } == FALSE
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

pub(crate) enum RunAsError {
    Cancelled,
    Launch(io::Error),
    MissingProcessHandle,
}

pub(crate) struct ElevatedProcess(OwnedHandle);

impl ElevatedProcess {
    pub(crate) fn handle(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}
