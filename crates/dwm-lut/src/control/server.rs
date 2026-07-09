use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED,
    ERROR_PIPE_NOT_CONNECTED, FALSE, HANDLE, INVALID_HANDLE_VALUE, SetLastError, TRUE,
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
    CreateEventW, CreateMutexW, INFINITE, WaitForSingleObject,
};

use crate::control::protocol::{
    CONTROL_PROTOCOL_VERSION, ControlCommand, ControlRequest, ControlResponse,
    MAX_CONTROL_MESSAGE_BYTES, decode_request, encode_response, validate_message_len,
};
use crate::control::{SecurityDescriptor, UserSid, current_pipe_name, last_os_error, wide_null};
use crate::error::InjectorError;
use crate::runtime;

const MAX_WORKER_THREADS: usize = 8;
const MAX_PIPE_INSTANCES: u32 = MAX_WORKER_THREADS as u32 + 1;
const REQUEST_READ_TIMEOUT_MS: u32 = 2_000;
const PIPE_CREATE_RETRY_DELAY_MS: u64 = 500;
const MAX_PIPE_CREATE_RETRIES: usize = 5;

pub(crate) fn run_server(
    host_dll_path: Option<PathBuf>,
    on_ready: impl FnOnce() -> Result<(), InjectorError>,
) -> Result<(), InjectorError> {
    let _host_guard = HostInstanceGuard::acquire()?;
    let pipe_name = current_pipe_name()?;
    let host_user_sid = UserSid::current_process()?;
    let command_lock = Arc::new(Mutex::new(()));
    let worker_slots = Arc::new(WorkerSlots::new(MAX_WORKER_THREADS));
    let mut pipe = create_pipe(&pipe_name, true, &host_user_sid)?;
    on_ready()?;
    println!("dwm-lut host instance is running on {pipe_name}");
    loop {
        let worker_slot = worker_slots.acquire();
        match pipe.connect() {
            Ok(ConnectOutcome::Connected) => {}
            Ok(ConnectOutcome::Abandoned) => {
                drop(worker_slot);
                pipe.disconnect();
                pipe = create_pipe_for_accept_loop(&pipe_name, &host_user_sid)?;
                continue;
            }
            Err(error) => {
                drop(worker_slot);
                eprintln!("{error}");
                pipe.disconnect();
                pipe = create_pipe_for_accept_loop(&pipe_name, &host_user_sid)?;
                continue;
            }
        }

        let host_dll_path = host_dll_path.clone();
        let command_lock = Arc::clone(&command_lock);
        std::thread::spawn(move || {
            let _worker_slot = worker_slot;
            if let Err(error) = handle_connected_client(&pipe, host_dll_path, command_lock) {
                eprintln!("{error}");
            }
            pipe.disconnect();
        });
        pipe = create_pipe_for_accept_loop(&pipe_name, &host_user_sid)?;
    }
}

pub(crate) fn handle_control_request(
    request: ControlRequest,
    handler: impl FnOnce(ControlCommand) -> ControlResponse,
) -> ControlResponse {
    if request.protocol_version != CONTROL_PROTOCOL_VERSION {
        return ControlResponse::protocol_mismatch(request.protocol_version);
    }

    handler(request.command)
}

fn handle_connected_client(
    pipe: &PipeHandle,
    host_dll_path: Option<PathBuf>,
    command_lock: Arc<Mutex<()>>,
) -> Result<(), InjectorError> {
    let bytes = match pipe.read_message() {
        Ok(bytes) => bytes,
        Err(error @ InjectorError::ControlTimeout { .. }) => return Err(error),
        Err(error) => {
            let response = runtime::response_from_result(Err(error));
            let response = encode_response(&response)?;
            return pipe.write_message(&response);
        }
    };
    let result = handle_control_request_bytes(&bytes, host_dll_path, command_lock);
    let response = match result {
        Ok(response) => response,
        Err(error) => runtime::response_from_result(Err(error)),
    };
    let response = encode_response(&response)?;
    pipe.write_message(&response)
}

fn handle_control_request_bytes(
    bytes: &[u8],
    host_dll_path: Option<PathBuf>,
    command_lock: Arc<Mutex<()>>,
) -> Result<ControlResponse, InjectorError> {
    let request = decode_request(bytes)?;
    Ok(handle_control_request(request, |command| {
        if command == ControlCommand::Status {
            return runtime::handle_command(command, host_dll_path);
        }

        let _command_guard = match command_lock.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                return runtime::response_from_result(Err(InjectorError::HostBusy));
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        };
        runtime::handle_command(command, host_dll_path)
    }))
}

fn create_pipe(
    pipe_name: &str,
    first_instance: bool,
    host_user_sid: &UserSid,
) -> Result<PipeHandle, InjectorError> {
    let pipe_name = wide_null(pipe_name);
    let security_descriptor = SecurityDescriptor::from_pipe_dacl(host_user_sid)?;
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
            MAX_PIPE_INSTANCES,
            MAX_CONTROL_MESSAGE_BYTES as u32,
            MAX_CONTROL_MESSAGE_BYTES as u32,
            0,
            &security_attributes,
        )
    };
    PipeHandle::new(handle, "create server pipe")
}

fn create_pipe_for_accept_loop(
    pipe_name: &str,
    host_user_sid: &UserSid,
) -> Result<PipeHandle, InjectorError> {
    for attempt in 1..=MAX_PIPE_CREATE_RETRIES {
        match create_pipe(pipe_name, false, host_user_sid) {
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

fn host_mutex_name_for_current_session() -> Result<String, InjectorError> {
    let pipe_name = current_pipe_name()?;
    Ok(pipe_name.replace(r"\\.\pipe\", r"Local\"))
}

struct WorkerSlots {
    state: Mutex<WorkerSlotState>,
    available: Condvar,
}

impl WorkerSlots {
    fn new(max_active: usize) -> Self {
        Self {
            state: Mutex::new(WorkerSlotState {
                active: 0,
                max_active,
            }),
            available: Condvar::new(),
        }
    }

    fn acquire(self: &Arc<Self>) -> WorkerSlotGuard {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        while state.active >= state.max_active {
            state = match self.available.wait(state) {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
        state.active += 1;

        WorkerSlotGuard {
            slots: Arc::clone(self),
        }
    }

    fn release(&self) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.active -= 1;
        self.available.notify_one();
    }
}

struct WorkerSlotState {
    active: usize,
    max_active: usize,
}

struct WorkerSlotGuard {
    slots: Arc<WorkerSlots>,
}

impl Drop for WorkerSlotGuard {
    fn drop(&mut self) {
        self.slots.release();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::{Arc, Mutex};

    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED,
    };

    use crate::control::protocol::{
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlRequest, PROTOCOL_MISMATCH_STATUS,
        encode_request,
    };

    use super::{
        handle_control_request, handle_control_request_bytes, is_disconnected_pipe_error_code,
    };

    #[test]
    fn matching_protocol_version_dispatches_command() {
        let called = Cell::new(false);
        let response = handle_control_request(
            ControlRequest {
                protocol_version: CONTROL_PROTOCOL_VERSION,
                command: ControlCommand::Status,
            },
            |command| {
                called.set(true);
                assert_eq!(command, ControlCommand::Status);
                crate::control::protocol::ControlResponse::ok("ok", "running")
            },
        );

        assert!(called.get());
        assert!(response.ok);
    }

    #[test]
    fn different_protocol_version_rejects_without_dispatching_command() {
        let called = Cell::new(false);
        let response = handle_control_request(
            ControlRequest {
                protocol_version: CONTROL_PROTOCOL_VERSION + 1,
                command: ControlCommand::Status,
            },
            |_command| {
                called.set(true);
                crate::control::protocol::ControlResponse::ok("ok", "running")
            },
        );

        assert!(!called.get());
        assert!(!response.ok);
        assert_eq!(response.status, PROTOCOL_MISMATCH_STATUS);
    }

    #[test]
    fn malformed_request_bytes_become_error_response() {
        let result = handle_control_request_bytes(
            br#"{"protocol_version":1,"command":"status""#,
            None,
            Arc::new(Mutex::new(())),
        );

        let response = crate::runtime::response_from_result(result);

        assert!(!response.ok);
        assert_eq!(response.protocol_version, CONTROL_PROTOCOL_VERSION);
        assert_eq!(response.status, "error");
        assert!(response.message.contains("control protocol failed"));
    }

    #[test]
    fn mutating_request_returns_busy_when_command_lock_is_held() {
        let command_lock = Arc::new(Mutex::new(()));
        let _guard = command_lock
            .lock()
            .expect("command lock should be available");
        let request = encode_request(&ControlRequest {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            command: ControlCommand::Disable,
        })
        .expect("request should encode");

        let response = handle_control_request_bytes(&request, None, Arc::clone(&command_lock))
            .expect("busy response should be encoded as a control response");

        assert!(!response.ok);
        assert_eq!(response.protocol_version, CONTROL_PROTOCOL_VERSION);
        assert_eq!(response.status, "busy");
        assert!(response.message.contains("host instance is busy"));
    }

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

struct PipeHandle(HANDLE);

// Pipe handles are owned by exactly one worker thread after the listener accepts a client.
unsafe impl Send for PipeHandle {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectOutcome {
    Connected,
    Abandoned,
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

    fn connect(&self) -> Result<ConnectOutcome, InjectorError> {
        let mut operation = OverlappedOperation::new()?;
        let ok = unsafe { ConnectNamedPipe(self.0, operation.as_mut_ptr()) };
        if ok != FALSE {
            return Ok(ConnectOutcome::Connected);
        }

        let error = last_os_error();
        match error.raw_os_error() {
            Some(code) if is_disconnected_pipe_error_code(code) => Ok(ConnectOutcome::Abandoned),
            Some(code) if code == ERROR_IO_PENDING as i32 => {
                match operation.wait(self.0, "connect server pipe", INFINITE) {
                    Ok(_) => Ok(ConnectOutcome::Connected),
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
                    operation.wait(self.0, "write control response", INFINITE)?
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

    fn disconnect(&self) {
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

struct HostInstanceGuard(HANDLE);

impl HostInstanceGuard {
    fn acquire() -> Result<Self, InjectorError> {
        let mutex_name = wide_null(&host_mutex_name_for_current_session()?);
        unsafe {
            SetLastError(0);
        }
        let handle = unsafe { CreateMutexW(null_mut(), FALSE, mutex_name.as_ptr()) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(InjectorError::ControlPipe {
                operation: "create host instance mutex",
                source: last_os_error(),
            });
        }

        let error = last_os_error();
        if error.raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32) {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
            return Err(InjectorError::HostAlreadyRunning);
        }

        Ok(Self(handle))
    }
}

impl Drop for HostInstanceGuard {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
}
