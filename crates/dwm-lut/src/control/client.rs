use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_SEM_TIMEOUT, FALSE, HANDLE,
    INVALID_HANDLE_VALUE, TRUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    GetNamedPipeServerProcessId, PIPE_READMODE_MESSAGE, SetNamedPipeHandleState, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    WaitForSingleObject,
};

use crate::control::build_hash::{current_build_hash, file_build_hash};
use crate::control::protocol::{
    ControlRequest, ControlResponse, MAX_CONTROL_MESSAGE_BYTES, decode_response, encode_request,
    validate_message_len, validate_response_build_hash,
};
use crate::control::{current_pipe_name, last_os_error, wide_null};
use crate::error::InjectorError;

const WAIT_PIPE_TIMEOUT_MS: u32 = 2_000;
const RESPONSE_READ_TIMEOUT_MS: u32 = 120_000;

pub(crate) fn send_request(request: &ControlRequest) -> Result<ControlResponse, InjectorError> {
    let pipe_name = current_pipe_name()?;
    let pipe = PipeHandle::open(&pipe_name)?;
    let local_build_hash = request.build_hash.clone();
    let request = encode_request(request)?;
    pipe.write_message(&request)?;
    let response = pipe.read_message()?;
    validate_response_build_hash(decode_response(&response)?, &local_build_hash)
}

struct PipeHandle(HANDLE);

impl PipeHandle {
    fn open(pipe_name: &str) -> Result<Self, InjectorError> {
        let pipe_name_wide = wide_null(pipe_name);
        let deadline = Instant::now() + Duration::from_millis(WAIT_PIPE_TIMEOUT_MS.into());
        loop {
            let wait_ok = unsafe { WaitNamedPipeW(pipe_name_wide.as_ptr(), WAIT_PIPE_TIMEOUT_MS) };
            if wait_ok == FALSE {
                let error = last_os_error();
                match error.raw_os_error() {
                    Some(code) if code == ERROR_FILE_NOT_FOUND as i32 => {
                        return Err(InjectorError::PrimaryUnavailable);
                    }
                    Some(code) if code == ERROR_PIPE_BUSY as i32 && Instant::now() < deadline => {
                        continue;
                    }
                    Some(code)
                        if code == ERROR_PIPE_BUSY as i32 || code == ERROR_SEM_TIMEOUT as i32 =>
                    {
                        return Err(InjectorError::PrimaryBusy);
                    }
                    _ => {}
                }

                return Err(InjectorError::ControlPipe {
                    operation: "wait for primary pipe",
                    source: error,
                });
            }

            let handle = unsafe {
                CreateFileW(
                    pipe_name_wide.as_ptr(),
                    FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    null_mut(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                    null_mut(),
                )
            };
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                let pipe = Self(handle);
                pipe.set_message_read_mode()?;
                pipe.verify_server_process()?;
                return Ok(pipe);
            }

            let error = last_os_error();
            match error.raw_os_error() {
                Some(code) if code == ERROR_FILE_NOT_FOUND as i32 => {
                    return Err(InjectorError::PrimaryUnavailable);
                }
                Some(code) if code == ERROR_PIPE_BUSY as i32 && Instant::now() < deadline => {}
                _ => {
                    return Err(InjectorError::ControlPipe {
                        operation: "open primary pipe",
                        source: error,
                    });
                }
            }
        }
    }

    fn set_message_read_mode(&self) -> Result<(), InjectorError> {
        let mode = PIPE_READMODE_MESSAGE;
        let ok = unsafe { SetNamedPipeHandleState(self.0, &mode, null_mut(), null_mut()) };
        if ok == FALSE {
            return Err(InjectorError::ControlPipe {
                operation: "set primary pipe read mode",
                source: last_os_error(),
            });
        }

        Ok(())
    }

    fn verify_server_process(&self) -> Result<(), InjectorError> {
        let server_pid = self.server_process_id()?;
        let server = ProcessHandle::open(server_pid)?;
        let server_path = server.image_path()?;
        let local_hash = current_build_hash()?;
        let server_hash = file_build_hash(&server_path)?;
        if server_hash != local_hash {
            return Err(InjectorError::ControlProtocol(format!(
                "primary process image mismatch: pid={server_pid}, path={}",
                server_path.display()
            )));
        }

        Ok(())
    }

    fn server_process_id(&self) -> Result<u32, InjectorError> {
        let mut pid = 0u32;
        let ok = unsafe { GetNamedPipeServerProcessId(self.0, &mut pid) };
        if ok == FALSE {
            return Err(InjectorError::ControlPipe {
                operation: "resolve primary pipe server process",
                source: last_os_error(),
            });
        }

        Ok(pid)
    }

    fn write_message(&self, bytes: &[u8]) -> Result<(), InjectorError> {
        validate_message_len(bytes.len())?;
        let len = u32::try_from(bytes.len()).map_err(|_| {
            InjectorError::ControlProtocol("message length does not fit u32".to_string())
        })?;
        let mut operation = OverlappedOperation::new()?;
        let ok = unsafe {
            WriteFile(
                self.0,
                bytes.as_ptr().cast(),
                len,
                null_mut(),
                operation.as_mut_ptr(),
            )
        };
        let written = if ok == FALSE {
            let error = last_os_error();
            match error.raw_os_error() {
                Some(code) if code == ERROR_IO_PENDING as i32 => {
                    operation.wait(self.0, "write control request", WAIT_PIPE_TIMEOUT_MS)?
                }
                _ => {
                    return Err(InjectorError::ControlPipe {
                        operation: "write control request",
                        source: error,
                    });
                }
            }
        } else {
            operation.result(self.0, "write control request")?
        };
        if written != len {
            return Err(InjectorError::ControlProtocol(format!(
                "partial control request write: wrote {written} of {len} bytes"
            )));
        }

        Ok(())
    }

    fn read_message(&self) -> Result<Vec<u8>, InjectorError> {
        let mut buffer = vec![0u8; MAX_CONTROL_MESSAGE_BYTES];
        let mut operation = OverlappedOperation::new()?;
        let ok = unsafe {
            ReadFile(
                self.0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                null_mut(),
                operation.as_mut_ptr(),
            )
        };
        let read = if ok == FALSE {
            let error = last_os_error();
            match error.raw_os_error() {
                Some(code) if code == ERROR_IO_PENDING as i32 => {
                    operation.wait(self.0, "read control response", RESPONSE_READ_TIMEOUT_MS)?
                }
                _ => {
                    return Err(InjectorError::ControlPipe {
                        operation: "read control response",
                        source: error,
                    });
                }
            }
        } else {
            operation.result(self.0, "read control response")?
        } as usize;
        validate_message_len(read)?;
        buffer.truncate(read);
        Ok(buffer)
    }
}

struct OverlappedOperation {
    overlapped: OVERLAPPED,
    event: EventHandle,
}

impl OverlappedOperation {
    fn new() -> Result<Self, InjectorError> {
        let event = EventHandle::new()?;
        let overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };

        Ok(Self { overlapped, event })
    }

    fn as_mut_ptr(&mut self) -> *mut OVERLAPPED {
        &mut self.overlapped
    }

    fn wait(
        &mut self,
        handle: HANDLE,
        operation: &'static str,
        timeout_ms: u32,
    ) -> Result<u32, InjectorError> {
        let wait_result = unsafe { WaitForSingleObject(self.event.0, timeout_ms) };
        match wait_result {
            WAIT_OBJECT_0 => self.result(handle, operation),
            WAIT_TIMEOUT => {
                unsafe {
                    CancelIoEx(handle, self.as_mut_ptr());
                }
                self.wait_for_cancel(handle);
                Err(InjectorError::ControlTimeout { operation })
            }
            _ => {
                let error = last_os_error();
                unsafe {
                    CancelIoEx(handle, self.as_mut_ptr());
                }
                self.wait_for_cancel(handle);
                Err(InjectorError::ControlPipe {
                    operation,
                    source: error,
                })
            }
        }
    }

    fn result(&mut self, handle: HANDLE, operation: &'static str) -> Result<u32, InjectorError> {
        let mut transferred = 0u32;
        let ok = unsafe { GetOverlappedResult(handle, self.as_mut_ptr(), &mut transferred, FALSE) };
        if ok == FALSE {
            return Err(InjectorError::ControlPipe {
                operation,
                source: last_os_error(),
            });
        }

        Ok(transferred)
    }

    fn wait_for_cancel(&mut self, handle: HANDLE) {
        let mut transferred = 0u32;
        unsafe {
            GetOverlappedResult(handle, self.as_mut_ptr(), &mut transferred, TRUE);
        }
    }
}

struct EventHandle(HANDLE);

impl EventHandle {
    fn new() -> Result<Self, InjectorError> {
        let handle = unsafe { CreateEventW(null_mut(), TRUE, FALSE, null_mut()) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(InjectorError::ControlPipe {
                operation: "create control pipe event",
                source: last_os_error(),
            });
        }

        Ok(Self(handle))
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
}

struct ProcessHandle(HANDLE);

impl ProcessHandle {
    fn open(pid: u32) -> Result<Self, InjectorError> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(InjectorError::ControlPipe {
                operation: "open primary process",
                source: last_os_error(),
            });
        }

        Ok(Self(handle))
    }

    fn image_path(&self) -> Result<PathBuf, InjectorError> {
        let mut buffer = vec![0u16; 32_768];
        let mut len = buffer.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(self.0, 0, buffer.as_mut_ptr(), &mut len) };
        if ok == FALSE {
            return Err(InjectorError::ControlPipe {
                operation: "query primary process image",
                source: last_os_error(),
            });
        }

        buffer.truncate(len as usize);
        Ok(PathBuf::from(OsString::from_wide(&buffer)))
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
}

impl Drop for PipeHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
}
