use std::ffi::{OsStr, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, FALSE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
};
use windows_sys::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, INFINITE, LPTHREAD_START_ROUTINE, WaitForSingleObject,
};

use crate::error::{InjectionStep, InjectorError};

use super::last_os_error;

pub(crate) struct OwnedHandle(HANDLE);

impl OwnedHandle {
    pub(crate) fn new(handle: HANDLE, step: InjectionStep) -> Result<Self, InjectorError> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(InjectorError::StepFailed {
                step,
                source: last_os_error(),
            });
        }

        Ok(Self(handle))
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

pub(crate) struct RemoteAllocation {
    process: HANDLE,
    address: *mut c_void,
}

impl RemoteAllocation {
    pub(crate) fn write_utf16(
        process: &OwnedHandle,
        value: &[u16],
        allocate_step: InjectionStep,
        write_step: InjectionStep,
    ) -> Result<Self, InjectorError> {
        Self::write_bytes(
            process,
            bytes_from_slice(value),
            PAGE_READWRITE,
            allocate_step,
            write_step,
        )
    }

    pub(crate) fn write_copy<T: Copy>(
        process: &OwnedHandle,
        value: &T,
        protection: u32,
        allocate_step: InjectionStep,
        write_step: InjectionStep,
    ) -> Result<Self, InjectorError> {
        Self::write_bytes(
            process,
            bytes_from_value(value),
            protection,
            allocate_step,
            write_step,
        )
    }

    pub(crate) fn write_bytes(
        process: &OwnedHandle,
        value: &[u8],
        protection: u32,
        allocate_step: InjectionStep,
        write_step: InjectionStep,
    ) -> Result<Self, InjectorError> {
        let allocation = Self::allocate(process, value.len(), protection, allocate_step)?;
        allocation.write_buffer(value.as_ptr().cast(), value.len(), write_step)?;
        Ok(allocation)
    }

    pub(crate) fn read_copy<T: Copy>(&self, step: InjectionStep) -> Result<T, InjectorError> {
        let mut value = std::mem::MaybeUninit::<T>::uninit();
        let mut read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                self.process,
                self.address,
                value.as_mut_ptr().cast(),
                std::mem::size_of::<T>(),
                &mut read,
            )
        };
        if ok == FALSE {
            return Err(InjectorError::StepFailed {
                step,
                source: last_os_error(),
            });
        }
        if read != std::mem::size_of::<T>() {
            return Err(InjectorError::StepFailed {
                step,
                source: io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "remote read returned fewer bytes than requested",
                ),
            });
        }

        Ok(unsafe { value.assume_init() })
    }

    fn allocate(
        process: &OwnedHandle,
        size_in_bytes: usize,
        protection: u32,
        step: InjectionStep,
    ) -> Result<Self, InjectorError> {
        let address = unsafe {
            VirtualAllocEx(
                process.raw(),
                null_mut(),
                size_in_bytes,
                MEM_COMMIT | MEM_RESERVE,
                protection,
            )
        };
        if address.is_null() {
            return Err(InjectorError::StepFailed {
                step,
                source: last_os_error(),
            });
        }

        Ok(Self {
            process: process.raw(),
            address,
        })
    }

    fn write_buffer(
        &self,
        buffer: *const c_void,
        size_in_bytes: usize,
        step: InjectionStep,
    ) -> Result<(), InjectorError> {
        let mut written = 0usize;
        let ok = unsafe {
            WriteProcessMemory(
                self.process,
                self.address,
                buffer,
                size_in_bytes,
                &mut written,
            )
        };
        if ok == FALSE || written != size_in_bytes {
            return Err(InjectorError::StepFailed {
                step,
                source: last_os_error(),
            });
        }

        Ok(())
    }

    pub(crate) fn address(&self) -> *mut c_void {
        self.address
    }
}

impl Drop for RemoteAllocation {
    fn drop(&mut self) {
        if !self.address.is_null() {
            unsafe {
                let _ = VirtualFreeEx(self.process, self.address, 0, MEM_RELEASE);
            }
        }
    }
}

pub(crate) fn run_remote_thread(
    process: &OwnedHandle,
    start_address: usize,
    parameter: *mut c_void,
    start_step: InjectionStep,
    wait_step: InjectionStep,
) -> Result<u32, InjectorError> {
    let thread = unsafe {
        CreateRemoteThread(
            process.raw(),
            null(),
            0,
            thread_start_from_address(start_address),
            parameter,
            0,
            null_mut(),
        )
    };
    let thread = OwnedHandle::new(thread, start_step)?;

    let wait_result = unsafe { WaitForSingleObject(thread.raw(), INFINITE) };
    if wait_result != WAIT_OBJECT_0 {
        return Err(InjectorError::StepFailed {
            step: wait_step,
            source: last_os_error(),
        });
    }

    let mut exit_code = 0u32;
    let ok = unsafe { GetExitCodeThread(thread.raw(), &mut exit_code) };
    if ok == FALSE {
        return Err(InjectorError::StepFailed {
            step: wait_step,
            source: last_os_error(),
        });
    }

    Ok(exit_code)
}

pub(crate) fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn thread_start_from_address(address: usize) -> LPTHREAD_START_ROUTINE {
    unsafe { std::mem::transmute(address) }
}

fn bytes_from_slice<T>(value: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(value.as_ptr().cast(), std::mem::size_of_val(value)) }
}

fn bytes_from_value<T>(value: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts((value as *const T).cast(), std::mem::size_of::<T>()) }
}
