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
    CreateEventW, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

use crate::control::protocol::{
    ControlCommand, ControlRequest, ControlResponse, MAX_CONTROL_MESSAGE_BYTES, decode_response,
    encode_request, validate_message_len, validate_response_protocol,
};
use crate::control::{current_pipe_name, last_os_error, wide_null};
use crate::error::InjectorError;

const WAIT_PIPE_TIMEOUT_MS: u32 = 2_000;
const RESPONSE_READ_TIMEOUT_MS: u32 = 120_000;
const HOST_SHUTDOWN_TIMEOUT_MS: u32 = 5_000;

pub(crate) fn send_request(request: &ControlRequest) -> Result<ControlResponse, InjectorError> {
    let pipe_name = current_pipe_name()?;
    let pipe = PipeHandle::open(&pipe_name)?;
    let host_process = if request.command == ControlCommand::Stop {
        Some(pipe.server_process()?)
    } else {
        None
    };
    let request = encode_request(request)?;
    pipe.write_message(&request)?;
    let response = pipe.read_message()?;
    let response = validate_response_protocol(decode_response(&response)?)?;
    if response.ok
        && let Some(host_process) = host_process
    {
        host_process.wait_for_exit()?;
    }
    Ok(response)
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
                        return Err(InjectorError::HostUnavailable);
                    }
                    Some(code) if code == ERROR_PIPE_BUSY as i32 && Instant::now() < deadline => {
                        continue;
                    }
                    Some(code)
                        if code == ERROR_PIPE_BUSY as i32 || code == ERROR_SEM_TIMEOUT as i32 =>
                    {
                        return Err(InjectorError::HostBusy);
                    }
                    _ => {}
                }

                return Err(InjectorError::ControlPipe {
                    operation: "wait for host pipe",
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
                return Ok(pipe);
            }

            let error = last_os_error();
            match error.raw_os_error() {
                Some(code) if code == ERROR_FILE_NOT_FOUND as i32 => {
                    return Err(InjectorError::HostUnavailable);
                }
                Some(code) if code == ERROR_PIPE_BUSY as i32 && Instant::now() < deadline => {}
                _ => {
                    return Err(InjectorError::ControlPipe {
                        operation: "open host pipe",
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
                operation: "set host pipe read mode",
                source: last_os_error(),
            });
        }

        Ok(())
    }

    fn server_process(&self) -> Result<HostProcessHandle, InjectorError> {
        let mut process_id = 0;
        let ok = unsafe { GetNamedPipeServerProcessId(self.0, &mut process_id) };
        if ok == FALSE {
            return Err(InjectorError::ControlPipe {
                operation: "resolve host process",
                source: last_os_error(),
            });
        }

        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, FALSE, process_id) };
        HostProcessHandle::new(handle)
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

struct HostProcessHandle(HANDLE);

impl HostProcessHandle {
    fn new(handle: HANDLE) -> Result<Self, InjectorError> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(InjectorError::ControlPipe {
                operation: "open host process",
                source: last_os_error(),
            });
        }
        Ok(Self(handle))
    }

    fn wait_for_exit(&self) -> Result<(), InjectorError> {
        match unsafe { WaitForSingleObject(self.0, HOST_SHUTDOWN_TIMEOUT_MS) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => Err(InjectorError::ControlTimeout {
                operation: "wait for host shutdown",
            }),
            _ => Err(InjectorError::ControlPipe {
                operation: "wait for host shutdown",
                source: last_os_error(),
            }),
        }
    }
}

impl Drop for HostProcessHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
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

impl Drop for PipeHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
}
