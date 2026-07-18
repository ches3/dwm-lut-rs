use std::ptr::null_mut;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, FALSE, HANDLE, INVALID_HANDLE_VALUE, TRUE,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

use crate::control::protocol::{MAX_CONTROL_MESSAGE_BYTES, validate_message_len};
use crate::control::{last_os_error, wide_null};
use crate::error::InjectorError;
use crate::security::{SecurityDescriptor, UserSid};

const REQUEST_READ_TIMEOUT_MS: u32 = 2_000;
const RESPONSE_WRITE_TIMEOUT_MS: u32 = 2_000;
const PIPE_CREATE_RETRY_DELAY_MS: u64 = 500;
const MAX_PIPE_CREATE_RETRIES: usize = 5;

pub(crate) struct ServerShutdown {
    event: EventHandle,
}

// Windows event handles may be signaled and waited on from different threads.
unsafe impl Send for ServerShutdown {}
unsafe impl Sync for ServerShutdown {}

impl ServerShutdown {
    pub(crate) fn new() -> Result<Self, InjectorError> {
        Ok(Self {
            event: EventHandle::new()?,
        })
    }

    pub(crate) fn request(&self) -> Result<(), InjectorError> {
        let ok = unsafe { SetEvent(self.event.0) };
        if ok == FALSE {
            return Err(InjectorError::ControlPipe {
                operation: "signal host shutdown",
                source: last_os_error(),
            });
        }
        Ok(())
    }

    pub(super) fn is_requested(&self) -> Result<bool, InjectorError> {
        match unsafe { WaitForSingleObject(self.event.0, 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(InjectorError::ControlPipe {
                operation: "check host shutdown",
                source: last_os_error(),
            }),
        }
    }
}

pub(super) fn create_pipe(
    pipe_name: &str,
    first_instance: bool,
    host_user_sid: &UserSid,
    max_instances: u32,
) -> Result<PipeHandle, InjectorError> {
    let pipe_name = wide_null(pipe_name);
    let security_descriptor = SecurityDescriptor::read_write_for_user(host_user_sid)?;
    let security_attributes = security_descriptor.as_security_attributes();
    let open_mode = if first_instance {
        PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED
    } else {
        PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED
    };
    let handle = unsafe {
        CreateNamedPipeW(
            pipe_name.as_ptr(),
            open_mode,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            max_instances,
            MAX_CONTROL_MESSAGE_BYTES as u32,
            MAX_CONTROL_MESSAGE_BYTES as u32,
            0,
            &security_attributes,
        )
    };
    PipeHandle::new(handle, "create server pipe")
}

pub(super) fn create_pipe_for_accept_loop(
    pipe_name: &str,
    host_user_sid: &UserSid,
    max_instances: u32,
) -> Result<PipeHandle, InjectorError> {
    for attempt in 1..=MAX_PIPE_CREATE_RETRIES {
        match create_pipe(pipe_name, false, host_user_sid, max_instances) {
            Ok(pipe) => return Ok(pipe),
            Err(error) if attempt < MAX_PIPE_CREATE_RETRIES => {
                eprintln!("{error}; retrying pipe creation ({attempt}/{MAX_PIPE_CREATE_RETRIES})");
                std::thread::sleep(Duration::from_millis(PIPE_CREATE_RETRY_DELAY_MS));
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!("pipe creation retry loop always returns")
}

pub(super) struct PipeHandle(HANDLE);

// Pipe handles are owned by exactly one worker thread after the listener accepts a client.
unsafe impl Send for PipeHandle {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnectOutcome {
    Connected,
    Abandoned,
    Shutdown,
}

impl PipeHandle {
    fn new(handle: HANDLE, operation: &'static str) -> Result<Self, InjectorError> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(InjectorError::ControlPipe {
                operation,
                source: last_os_error(),
            });
        }

        Ok(Self(handle))
    }

    pub(super) fn connect(
        &self,
        shutdown: &ServerShutdown,
    ) -> Result<ConnectOutcome, InjectorError> {
        if shutdown.is_requested()? {
            return Ok(ConnectOutcome::Shutdown);
        }
        let mut operation = OverlappedOperation::new()?;
        let ok = unsafe { ConnectNamedPipe(self.0, operation.as_mut_ptr()) };
        if ok != FALSE {
            return Ok(ConnectOutcome::Connected);
        }

        let error = last_os_error();
        match error.raw_os_error() {
            Some(code) if is_disconnected_pipe_error_code(code) => Ok(ConnectOutcome::Abandoned),
            Some(code) if code == ERROR_IO_PENDING as i32 => {
                match operation.wait_or_shutdown(self.0, "connect server pipe", shutdown) {
                    Ok(Some(_)) => Ok(ConnectOutcome::Connected),
                    Ok(None) => Ok(ConnectOutcome::Shutdown),
                    Err(error) if is_disconnected_pipe_error(&error) => {
                        Ok(ConnectOutcome::Abandoned)
                    }
                    Err(error) => Err(error),
                }
            }
            Some(code) if code == ERROR_PIPE_CONNECTED as i32 => Ok(ConnectOutcome::Connected),
            _ => Err(InjectorError::ControlPipe {
                operation: "connect server pipe",
                source: error,
            }),
        }
    }

    pub(super) fn read_message(&self) -> Result<Vec<u8>, InjectorError> {
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
                    operation.wait(self.0, "read control request", REQUEST_READ_TIMEOUT_MS)?
                }
                _ => {
                    return Err(InjectorError::ControlPipe {
                        operation: "read control request",
                        source: error,
                    });
                }
            }
        } else {
            operation.result(self.0, "read control request")?
        } as usize;
        validate_message_len(read)?;
        buffer.truncate(read);
        Ok(buffer)
    }

    pub(super) fn write_message(&self, bytes: &[u8]) -> Result<(), InjectorError> {
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
                    operation.wait(self.0, "write control response", RESPONSE_WRITE_TIMEOUT_MS)?
                }
                _ => {
                    return Err(InjectorError::ControlPipe {
                        operation: "write control response",
                        source: error,
                    });
                }
            }
        } else {
            operation.result(self.0, "write control response")?
        };
        if written != len {
            return Err(InjectorError::ControlProtocol(format!(
                "partial control response write: wrote {written} of {len} bytes"
            )));
        }

        Ok(())
    }

    pub(super) fn disconnect(&self) {
        unsafe {
            DisconnectNamedPipe(self.0);
        }
    }
}

fn is_disconnected_pipe_error(error: &InjectorError) -> bool {
    match error {
        InjectorError::ControlPipe { source, .. } => source
            .raw_os_error()
            .is_some_and(is_disconnected_pipe_error_code),
        _ => false,
    }
}

fn is_disconnected_pipe_error_code(code: i32) -> bool {
    code == ERROR_BROKEN_PIPE as i32
        || code == ERROR_NO_DATA as i32
        || code == ERROR_PIPE_NOT_CONNECTED as i32
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

    fn wait_or_shutdown(
        &mut self,
        handle: HANDLE,
        operation: &'static str,
        shutdown: &ServerShutdown,
    ) -> Result<Option<u32>, InjectorError> {
        let handles = [self.event.0, shutdown.event.0];
        let wait_result = unsafe {
            WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), FALSE, INFINITE)
        };
        match wait_result {
            WAIT_OBJECT_0 => self.result(handle, operation).map(Some),
            result if result == WAIT_OBJECT_0 + 1 => {
                unsafe {
                    CancelIoEx(handle, self.as_mut_ptr());
                }
                match self.result_waiting(handle, operation) {
                    Ok(transferred) => Ok(Some(transferred)),
                    Err(InjectorError::ControlPipe { source, .. })
                        if source.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) =>
                    {
                        Ok(None)
                    }
                    Err(error) => Err(error),
                }
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

    fn result_waiting(
        &mut self,
        handle: HANDLE,
        operation: &'static str,
    ) -> Result<u32, InjectorError> {
        let mut transferred = 0u32;
        let ok = unsafe { GetOverlappedResult(handle, self.as_mut_ptr(), &mut transferred, TRUE) };
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
            GetOverlappedResult(handle, self.as_mut_ptr(), &mut transferred, 1);
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

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED,
    };

    use super::is_disconnected_pipe_error_code;

    #[test]
    fn disconnected_pipe_errors_are_client_connection_abandonment() {
        assert!(is_disconnected_pipe_error_code(ERROR_BROKEN_PIPE as i32));
        assert!(is_disconnected_pipe_error_code(ERROR_NO_DATA as i32));
        assert!(is_disconnected_pipe_error_code(
            ERROR_PIPE_NOT_CONNECTED as i32
        ));
    }

    #[test]
    fn unrelated_pipe_errors_are_not_connection_abandonment() {
        assert!(!is_disconnected_pipe_error_code(ERROR_ACCESS_DENIED as i32));
    }
}
