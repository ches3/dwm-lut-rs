use std::ffi::{OsStr, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle as StdOwnedHandle};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{FALSE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
};
use windows_sys::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, INFINITE, LPTHREAD_START_ROUTINE, WaitForSingleObject,
};

use crate::inject::{InjectError, InjectionStep};

use super::last_os_error;

pub(crate) struct OwnedHandle(StdOwnedHandle);

impl OwnedHandle {
    pub(crate) fn new(handle: HANDLE, step: InjectionStep) -> Result<Self, InjectError> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(InjectError::StepFailed {
                step,
                source: last_os_error(),
            });
        }

        // SAFETY: the creating Win32 API returned an owned handle that must be closed.
        Ok(Self(unsafe { StdOwnedHandle::from_raw_handle(handle) }))
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.0.as_raw_handle()
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
    ) -> Result<Self, InjectError> {
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
    ) -> Result<Self, InjectError> {
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
    ) -> Result<Self, InjectError> {
        let allocation = Self::allocate(process, value.len(), protection, allocate_step)?;
        allocation.write_buffer(value.as_ptr().cast(), value.len(), write_step)?;
        Ok(allocation)
    }

    /// # Safety
    ///
    /// The bytes stored in this allocation must be a valid representation of `T`.
    pub(crate) unsafe fn read_copy<T: Copy>(&self, step: InjectionStep) -> Result<T, InjectError> {
        let bytes = read_process_memory_raw(
            self.process,
            self.address as usize,
            std::mem::size_of::<T>(),
            step,
        )?;
        let mut value = std::mem::MaybeUninit::<T>::uninit();
        // SAFETY: read_process_memory_raw returned exactly size_of::<T>()
        // initialized bytes, and the caller guarantees they represent a valid T.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), value.as_mut_ptr().cast(), bytes.len());
            Ok(value.assume_init())
        }
    }

    fn allocate(
        process: &OwnedHandle,
        size_in_bytes: usize,
        protection: u32,
        step: InjectionStep,
    ) -> Result<Self, InjectError> {
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
            return Err(InjectError::StepFailed {
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
    ) -> Result<(), InjectError> {
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
            return Err(InjectError::StepFailed {
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

pub(crate) fn read_process_memory(
    process: &OwnedHandle,
    address: usize,
    len: usize,
    step: InjectionStep,
) -> Result<Vec<u8>, InjectError> {
    read_process_memory_raw(process.raw(), address, len, step)
}

fn read_process_memory_raw(
    process: HANDLE,
    address: usize,
    len: usize,
    step: InjectionStep,
) -> Result<Vec<u8>, InjectError> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let mut value = vec![0_u8; len];
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            process,
            address as *const c_void,
            value.as_mut_ptr().cast(),
            value.len(),
            &mut read,
        )
    };
    if ok == FALSE {
        return Err(InjectError::StepFailed {
            step,
            source: last_os_error(),
        });
    }
    if read != value.len() {
        return Err(InjectError::StepFailed {
            step,
            source: io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "remote read returned fewer bytes than requested",
            ),
        });
    }
    Ok(value)
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
) -> Result<u32, InjectError> {
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
        return Err(InjectError::StepFailed {
            step: wait_step,
            source: last_os_error(),
        });
    }

    let mut exit_code = 0u32;
    let ok = unsafe { GetExitCodeThread(thread.raw(), &mut exit_code) };
    if ok == FALSE {
        return Err(InjectError::StepFailed {
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
