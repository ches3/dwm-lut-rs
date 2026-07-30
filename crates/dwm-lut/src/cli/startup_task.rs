use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

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
use windows::core::{BSTR, Interface};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, FALSE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

use crate::paths::PathError;
use crate::platform::elevation::{self, ElevationError};
use crate::platform::security::{SecurityError, UserSid};

#[derive(Debug)]
pub enum StartupTaskError {
    Elevation(ElevationError),
    LaunchFailed {
        operation: &'static str,
        source: io::Error,
    },
    OperationFailed {
        operation: &'static str,
        exit_code: u32,
    },
    Path(PathError),
    Security(SecurityError),
}

impl fmt::Display for StartupTaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Elevation(ElevationError::Cancelled) => {
                write!(f, "startup task elevation was canceled")
            }
            Self::Elevation(ElevationError::RequiresAdministratorUser) => write!(
                f,
                "startup task installation and removal require signing in with an administrator account"
            ),
            Self::Elevation(error) => write!(f, "startup task {error}"),
            Self::LaunchFailed { operation, source } => {
                write!(f, "startup task {operation} failed: {source}")
            }
            Self::OperationFailed {
                operation,
                exit_code,
            } => write!(
                f,
                "startup task {operation} failed with exit code {exit_code:#x}"
            ),
            Self::Path(error) => error.fmt(f),
            Self::Security(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StartupTaskError {}

impl From<ElevationError> for StartupTaskError {
    fn from(value: ElevationError) -> Self {
        Self::Elevation(value)
    }
}

impl From<PathError> for StartupTaskError {
    fn from(value: PathError) -> Self {
        Self::Path(value)
    }
}

impl From<SecurityError> for StartupTaskError {
    fn from(value: SecurityError) -> Self {
        Self::Security(value)
    }
}

const TASK_NAME_PREFIX: &str = "dwm-lut-rs-";
const INFINITE: u32 = u32::MAX;

pub(crate) fn install() -> Result<(), StartupTaskError> {
    if current_process_is_elevated()? {
        install_elevated()
    } else {
        require_limited_admin_token(current_process_elevation_type()?)?;
        run_elevated_cli("installation", "install")
    }
}

pub(crate) fn uninstall() -> Result<(), StartupTaskError> {
    if current_process_is_elevated()? {
        uninstall_elevated()
    } else {
        require_limited_admin_token(current_process_elevation_type()?)?;
        run_elevated_cli("removal", "uninstall")
    }
}

fn current_process_is_elevated() -> Result<bool, StartupTaskError> {
    elevation::is_process_elevated().map_err(|source| StartupTaskError::LaunchFailed {
        operation: "elevation check",
        source,
    })
}

fn current_process_elevation_type()
-> Result<windows_sys::Win32::Security::TOKEN_ELEVATION_TYPE, StartupTaskError> {
    elevation::current_token_elevation_type().map_err(|source| StartupTaskError::LaunchFailed {
        operation: "elevation type check",
        source,
    })
}

fn install_elevated() -> Result<(), StartupTaskError> {
    let host_path = installed_host_path()?;
    let user_sid = UserSid::current_process()?;
    let result = with_task_service(|service| unsafe {
        let definition = service.NewTask(0)?;
        let task_name = BSTR::from(startup_task_name(user_sid.as_sddl()));
        let user_sid = BSTR::from(user_sid.as_sddl());

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
        action.SetArguments(&BSTR::from("--background"))?;

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

fn uninstall_elevated() -> Result<(), StartupTaskError> {
    let user_sid = UserSid::current_process()?;
    let result = with_task_service(|service| unsafe {
        let task_name = BSTR::from(startup_task_name(user_sid.as_sddl()));
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
) -> Result<(), StartupTaskError> {
    result.map_err(|error| StartupTaskError::OperationFailed {
        operation,
        exit_code: error.code().0 as u32,
    })
}

fn installed_host_path() -> Result<PathBuf, StartupTaskError> {
    Ok(crate::entry::launcher::resolve_host_executable_path(None)?)
}

fn startup_task_name(user_sid: &str) -> String {
    format!("{TASK_NAME_PREFIX}{user_sid}")
}

fn run_elevated_cli(operation: &'static str, command: &str) -> Result<(), StartupTaskError> {
    let executable = std::env::current_exe()
        .map_err(|source| StartupTaskError::LaunchFailed { operation, source })?;
    let parameters = OsStr::new(command).encode_wide().collect::<Vec<_>>();
    let process = elevation::run_as(&executable, &parameters).map_err(|error| match error {
        elevation::RunAsError::Cancelled => StartupTaskError::Elevation(ElevationError::Cancelled),
        elevation::RunAsError::Launch(source) => {
            StartupTaskError::LaunchFailed { operation, source }
        }
        elevation::RunAsError::MissingProcessHandle => StartupTaskError::LaunchFailed {
            operation,
            source: io::Error::other("elevated CLI returned no process handle"),
        },
    })?;
    if unsafe { WaitForSingleObject(process.handle(), INFINITE) } != WAIT_OBJECT_0 {
        return Err(StartupTaskError::LaunchFailed {
            operation,
            source: last_os_error(),
        });
    }
    let mut exit_code = 0;
    if unsafe { GetExitCodeProcess(process.handle(), &mut exit_code) } == FALSE {
        return Err(StartupTaskError::LaunchFailed {
            operation,
            source: last_os_error(),
        });
    }
    if exit_code == 0 {
        Ok(())
    } else {
        Err(StartupTaskError::OperationFailed {
            operation,
            exit_code,
        })
    }
}

fn require_limited_admin_token(
    elevation_type: windows_sys::Win32::Security::TOKEN_ELEVATION_TYPE,
) -> Result<(), StartupTaskError> {
    use windows_sys::Win32::Security::TokenElevationTypeLimited;

    if elevation_type == TokenElevationTypeLimited {
        Ok(())
    } else {
        Err(StartupTaskError::Elevation(
            ElevationError::RequiresAdministratorUser,
        ))
    }
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

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
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
            startup_task_name("S-1-5-21-1000"),
            "dwm-lut-rs-S-1-5-21-1000"
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
