use std::io;
use std::mem::size_of;

use windows_sys::Win32::Foundation::FALSE;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
};

use crate::error::{InjectionStep, InjectorError};

use super::remote::OwnedHandle;
use super::{create_toolhelp_snapshot, is_no_more_files_error, last_os_error, utf16_to_string};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RemoteModule {
    pub(crate) base_address: usize,
}

pub(crate) fn find_remote_module(
    pid: u32,
    module_name: &str,
    step: InjectionStep,
) -> Result<RemoteModule, InjectorError> {
    let snapshot = create_module_snapshot(pid, step)?;
    let mut entry = empty_module_entry();

    let mut has_entry = unsafe { Module32FirstW(snapshot.raw(), &mut entry) };
    if has_entry == FALSE {
        return module_lookup_error(last_os_error(), step, module_name);
    }

    loop {
        if utf16_to_string(&entry.szModule).eq_ignore_ascii_case(module_name) {
            return Ok(RemoteModule {
                base_address: entry.modBaseAddr as usize,
            });
        }

        has_entry = unsafe { Module32NextW(snapshot.raw(), &mut entry) };
        if has_entry == FALSE {
            return module_lookup_error(last_os_error(), step, module_name);
        }
    }
}

fn create_module_snapshot(pid: u32, step: InjectionStep) -> Result<OwnedHandle, InjectorError> {
    create_toolhelp_snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid, step, true)
}

fn empty_module_entry() -> MODULEENTRY32W {
    MODULEENTRY32W {
        dwSize: size_of::<MODULEENTRY32W>() as u32,
        th32ModuleID: 0,
        th32ProcessID: 0,
        GlblcntUsage: 0,
        ProccntUsage: 0,
        modBaseAddr: std::ptr::null_mut(),
        modBaseSize: 0,
        hModule: std::ptr::null_mut(),
        szModule: [0; 256],
        szExePath: [0; 260],
    }
}

fn module_lookup_error(
    error: io::Error,
    step: InjectionStep,
    module_name: &str,
) -> Result<RemoteModule, InjectorError> {
    if is_no_more_files_error(&error) {
        return Err(InjectorError::RemoteModuleNotFound {
            module: module_name.to_string(),
        });
    }

    Err(InjectorError::StepFailed {
        step,
        source: error,
    })
}

#[cfg(test)]
mod tests {
    use std::io;

    use windows_sys::Win32::Foundation::ERROR_NO_MORE_FILES;

    use crate::error::{InjectionStep, InjectorError};

    use super::module_lookup_error;

    #[test]
    fn maps_no_more_files_to_remote_module_not_found() {
        let error = module_lookup_error(
            io::Error::from_raw_os_error(ERROR_NO_MORE_FILES as i32),
            InjectionStep::ResolveKernel32,
            "kernel32.dll",
        )
        .expect_err("missing module must be reported");

        assert!(matches!(
            error,
            InjectorError::RemoteModuleNotFound { module } if module == "kernel32.dll"
        ));
    }
}
