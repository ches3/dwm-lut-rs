use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::System::TaskScheduler::{
    IExecAction, ILogonTrigger, ITaskService, TASK_ACTION_EXEC, TASK_CREATE_OR_UPDATE,
    TASK_INSTANCES_IGNORE_NEW, TASK_LOGON_INTERACTIVE_TOKEN, TASK_RUNLEVEL_HIGHEST,
    TASK_TRIGGER_LOGON, TaskScheduler,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, HRESULT, Interface};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, FALSE, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};

use crate::error::InjectorError;

const TASK_NAME_PREFIX: &str = "dwm-lut-rs-";
const HOST_EXE_NAME: &str = "dwm-lut.exe";
const INFINITE: u32 = u32::MAX;

pub(crate) fn install() -> Result<(), InjectorError> {
    if is_process_elevated()? {
        install_elevated()
    } else {
        require_limited_admin_token(current_token_elevation_type()?)?;
        run_elevated_cli("installation", "install")
    }
}

pub(crate) fn uninstall() -> Result<(), InjectorError> {
    if is_process_elevated()? {
        uninstall_elevated()
    } else {
        require_limited_admin_token(current_token_elevation_type()?)?;
        run_elevated_cli("removal", "uninstall")
    }
}

fn install_elevated() -> Result<(), InjectorError> {
    let host_path = installed_host_path()?;
    let result = with_task_service(|service| unsafe {
        let definition = service.NewTask(0)?;
        let user_sid = current_user_sid()?;
        let task_name = bstr_from_os_str(&startup_task_name(&user_sid));
        let user_sid = bstr_from_os_str(&user_sid);

        let principal = definition.Principal()?;
        principal.SetUserId(&user_sid)?;
        principal.SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)?;
        principal.SetRunLevel(TASK_RUNLEVEL_HIGHEST)?;

        let trigger: ILogonTrigger = definition.Triggers()?.Create(TASK_TRIGGER_LOGON)?.cast()?;
        trigger.SetUserId(&user_sid)?;

        let settings = definition.Settings()?;
        settings.SetMultipleInstances(TASK_INSTANCES_IGNORE_NEW)?;
        settings.SetDisallowStartIfOnBatteries(VARIANT_BOOL(0))?;
        settings.SetStopIfGoingOnBatteries(VARIANT_BOOL(0))?;
        settings.SetAllowHardTerminate(VARIANT_BOOL(-1))?;
        settings.SetStartWhenAvailable(VARIANT_BOOL(-1))?;
        settings.SetAllowDemandStart(VARIANT_BOOL(-1))?;
        settings.SetEnabled(VARIANT_BOOL(-1))?;
        settings.SetHidden(VARIANT_BOOL(-1))?;
        settings.SetExecutionTimeLimit(&BSTR::from("PT0S"))?;

        let action: IExecAction = definition.Actions()?.Create(TASK_ACTION_EXEC)?.cast()?;
        action.SetPath(&bstr_from_os_str(host_path.as_os_str()))?;

        let folder = service.GetFolder(&BSTR::from("\\"))?;
        let empty = VARIANT::default();
        folder.RegisterTaskDefinition(
            &task_name,
            &definition,
            TASK_CREATE_OR_UPDATE.0,
            &empty,
            &empty,
            TASK_LOGON_INTERACTIVE_TOKEN,
            &empty,
        )?;
        Ok(())
    });
    map_com_result("installation", result)
}

fn uninstall_elevated() -> Result<(), InjectorError> {
    let result = with_task_service(|service| unsafe {
        let task_name = bstr_from_os_str(&startup_task_name(&current_user_sid()?));
        let folder = service.GetFolder(&BSTR::from("\\"))?;
        match folder.DeleteTask(&task_name, 0) {
            Ok(()) => Ok(()),
            Err(error) if error.code().0 as u32 == hresult_from_win32(ERROR_FILE_NOT_FOUND) => {
                Ok(())
            }
            Err(error) => Err(error),
        }
    });
    map_com_result("removal", result)
}

fn with_task_service<T>(
    operation: impl FnOnce(&ITaskService) -> windows::core::Result<T>,
) -> windows::core::Result<T> {
    let apartment = ComApartment::initialize()?;
    let service: ITaskService =
        unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)? };
    let empty = VARIANT::default();
    unsafe { service.Connect(&empty, &empty, &empty, &empty)? };
    let result = operation(&service);
    drop(service);
    drop(apartment);
    result
}

fn map_com_result(
    operation: &'static str,
    result: windows::core::Result<()>,
) -> Result<(), InjectorError> {
    result.map_err(|error| InjectorError::StartupTaskOperationFailed {
        operation,
        exit_code: error.code().0 as u32,
    })
}

fn installed_host_path() -> Result<PathBuf, InjectorError> {
    let cli_path =
        std::env::current_exe().map_err(|source| InjectorError::StartupTaskLaunchFailed {
            operation: "install executable resolution",
            source,
        })?;
    let directory = cli_path
        .parent()
        .ok_or_else(|| InjectorError::StartupTaskLaunchFailed {
            operation: "host executable resolution",
            source: io::Error::other("CLI executable has no parent directory"),
        })?;
    let host_path = directory.join(HOST_EXE_NAME);
    if !host_path.is_file() {
        return Err(InjectorError::MissingFile {
            kind: "host executable",
            path: host_path,
        });
    }
    Ok(host_path)
}

fn startup_task_name(user_sid: &OsStr) -> OsString {
    let mut name = OsString::from(TASK_NAME_PREFIX);
    name.push(user_sid);
    name
}

fn run_elevated_cli(operation: &'static str, command: &str) -> Result<(), InjectorError> {
    let executable = std::env::current_exe()
        .map_err(|source| InjectorError::StartupTaskLaunchFailed { operation, source })?;
    let executable = wide_null(executable.as_os_str());
    let verb = wide_null(OsStr::new("runas"));
    let parameters = wide_null(OsStr::new(command));
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
        let source = last_os_error();
        if source.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            return Err(InjectorError::StartupTaskElevationCancelled);
        }
        return Err(InjectorError::StartupTaskLaunchFailed { operation, source });
    }
    if execute.hProcess.is_null() {
        return Err(InjectorError::StartupTaskLaunchFailed {
            operation,
            source: io::Error::other("elevated CLI returned no process handle"),
        });
    }

    let process = ProcessHandle(execute.hProcess);
    if unsafe { WaitForSingleObject(process.0, INFINITE) } != WAIT_OBJECT_0 {
        return Err(InjectorError::StartupTaskLaunchFailed {
            operation,
            source: last_os_error(),
        });
    }
    let mut exit_code = 0;
    if unsafe { GetExitCodeProcess(process.0, &mut exit_code) } == FALSE {
        return Err(InjectorError::StartupTaskLaunchFailed {
            operation,
            source: last_os_error(),
        });
    }
    if exit_code == 0 {
        Ok(())
    } else {
        Err(InjectorError::StartupTaskOperationFailed {
            operation,
            exit_code,
        })
    }
}

fn is_process_elevated() -> Result<bool, InjectorError> {
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == FALSE {
        return Err(InjectorError::StartupTaskLaunchFailed {
            operation: "elevation check token open",
            source: last_os_error(),
        });
    }
    let token = ProcessHandle(token);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned_len = 0;
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_len,
        )
    } == FALSE
    {
        return Err(InjectorError::StartupTaskLaunchFailed {
            operation: "elevation check token read",
            source: last_os_error(),
        });
    }
    Ok(elevation.TokenIsElevated != 0)
}

fn current_token_elevation_type()
-> Result<windows_sys::Win32::Security::TOKEN_ELEVATION_TYPE, InjectorError> {
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION_TYPE, TOKEN_QUERY, TokenElevationType,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == FALSE {
        return Err(InjectorError::StartupTaskLaunchFailed {
            operation: "elevation type token open",
            source: last_os_error(),
        });
    }
    let token = ProcessHandle(token);
    let mut elevation_type: TOKEN_ELEVATION_TYPE = 0;
    let mut returned_len = 0;
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevationType,
            (&mut elevation_type as *mut TOKEN_ELEVATION_TYPE).cast(),
            std::mem::size_of::<TOKEN_ELEVATION_TYPE>() as u32,
            &mut returned_len,
        )
    } == FALSE
    {
        return Err(InjectorError::StartupTaskLaunchFailed {
            operation: "elevation type token read",
            source: last_os_error(),
        });
    }
    Ok(elevation_type)
}

fn require_limited_admin_token(
    elevation_type: windows_sys::Win32::Security::TOKEN_ELEVATION_TYPE,
) -> Result<(), InjectorError> {
    use windows_sys::Win32::Security::TokenElevationTypeLimited;

    if elevation_type == TokenElevationTypeLimited {
        Ok(())
    } else {
        Err(InjectorError::StartupTaskRequiresAdministratorUser)
    }
}

fn current_user_sid() -> windows::core::Result<OsString> {
    use windows_sys::Win32::Foundation::{FALSE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == FALSE {
        return Err(last_windows_error());
    }
    let token = ProcessHandle(token);
    let mut required_len = 0;
    unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut required_len) };
    if required_len == 0 {
        return Err(last_windows_error());
    }
    let mut buffer = vec![0u8; required_len as usize];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required_len,
            &mut required_len,
        )
    } == FALSE
    {
        return Err(last_windows_error());
    }
    let token_user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
    let mut sid_text = null_mut();
    if unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut sid_text) } == FALSE {
        return Err(last_windows_error());
    }
    let mut length = 0;
    while unsafe { *sid_text.add(length) } != 0 {
        length += 1;
    }
    let sid = OsString::from_wide(unsafe { std::slice::from_raw_parts(sid_text, length) });
    unsafe { LocalFree(sid_text.cast()) };
    Ok(sid)
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn bstr_from_os_str(value: &OsStr) -> BSTR {
    BSTR::from_wide(&value.encode_wide().collect::<Vec<_>>())
}

fn hresult_from_win32(code: u32) -> u32 {
    0x8007_0000 | code
}

fn last_os_error() -> io::Error {
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() } as i32;
    io::Error::from_raw_os_error(code)
}

fn last_windows_error() -> windows::core::Error {
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    windows::core::Error::from_hresult(HRESULT(hresult_from_win32(code) as i32))
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> windows::core::Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::Security::{
        TokenElevationTypeDefault, TokenElevationTypeFull, TokenElevationTypeLimited,
    };

    use super::{bstr_from_os_str, require_limited_admin_token, startup_task_name};

    #[test]
    fn bstr_conversion_preserves_ill_formed_utf16() {
        let units = [b'C' as u16, b':' as u16, b'\\' as u16, 0xd800];
        let value = OsString::from_wide(&units);

        assert_eq!(&*bstr_from_os_str(&value), units);
    }

    #[test]
    fn task_name_is_scoped_to_user_sid() {
        assert_eq!(
            startup_task_name(OsStr::new("S-1-5-21-1000")),
            OsStr::new("dwm-lut-rs-S-1-5-21-1000")
        );
    }

    #[test]
    fn limited_admin_token_can_request_elevation() {
        assert!(require_limited_admin_token(TokenElevationTypeLimited).is_ok());
    }

    #[test]
    fn default_and_full_tokens_cannot_request_elevation_from_non_elevated_path() {
        for elevation_type in [TokenElevationTypeDefault, TokenElevationTypeFull] {
            assert!(require_limited_admin_token(elevation_type).is_err());
        }
    }
}
