use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, PipeMode};
use tokio::time::Instant;
use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, FALSE, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

use crate::control::protocol::{
    ControlCommand, ControlRequest, ControlResponse, decode_response, encode_request,
    validate_response_protocol,
};
use crate::control::{
    build_runtime, current_pipe_name, last_os_error, read_message, write_message,
};
use crate::error::InjectorError;

const WAIT_PIPE_TIMEOUT: Duration = Duration::from_secs(2);
const PIPE_OPEN_RETRY_DELAY: Duration = Duration::from_millis(50);
const RESPONSE_READ_TIMEOUT: Duration = Duration::from_secs(10);
const HOST_SHUTDOWN_TIMEOUT_MS: u32 = 5_000;

pub(crate) fn send_request(request: &ControlRequest) -> Result<ControlResponse, InjectorError> {
    let pipe_name = current_pipe_name()?;
    let runtime = build_runtime("create control client runtime")?;
    let (response, host_process) = runtime.block_on(send_request_async(&pipe_name, request))?;
    if response.ok
        && let Some(host_process) = host_process
    {
        host_process.wait_for_exit()?;
    }
    Ok(response)
}

async fn send_request_async(
    pipe_name: &str,
    request: &ControlRequest,
) -> Result<(ControlResponse, Option<HostProcessHandle>), InjectorError> {
    let mut pipe = open_pipe(pipe_name).await?;
    let host_process = if request.command == ControlCommand::Stop {
        Some(HostProcessHandle::from_pipe(&pipe)?)
    } else {
        None
    };
    let request = encode_request(request)?;
    write_message(
        &mut pipe,
        &request,
        WAIT_PIPE_TIMEOUT,
        "write control request",
    )
    .await?;
    let response = read_message(&mut pipe, RESPONSE_READ_TIMEOUT, "read control response").await?;
    let response = validate_response_protocol(decode_response(&response)?)?;
    Ok((response, host_process))
}

async fn open_pipe(pipe_name: &str) -> Result<NamedPipeClient, InjectorError> {
    let deadline = Instant::now() + WAIT_PIPE_TIMEOUT;
    let mut options = ClientOptions::new();
    options.pipe_mode(PipeMode::Message);
    loop {
        match options.open(pipe_name) {
            Ok(pipe) => return Ok(pipe),
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && Instant::now() < deadline =>
            {
                let remaining = deadline.saturating_duration_since(Instant::now());
                tokio::time::sleep(remaining.min(PIPE_OPEN_RETRY_DELAY)).await;
            }
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                return Err(InjectorError::HostBusy);
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) =>
            {
                return Err(InjectorError::HostUnavailable);
            }
            Err(source) => {
                return Err(InjectorError::ControlPipe {
                    operation: "open host pipe",
                    source,
                });
            }
        }
    }
}

struct HostProcessHandle {
    handle: OwnedHandle,
}

impl HostProcessHandle {
    fn from_pipe(pipe: &NamedPipeClient) -> Result<Self, InjectorError> {
        let mut process_id = 0;
        let ok =
            unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle() as HANDLE, &mut process_id) };
        if ok == FALSE {
            return Err(InjectorError::ControlPipe {
                operation: "resolve host process",
                source: last_os_error(),
            });
        }
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, FALSE, process_id) };
        Self::new(handle)
    }

    fn new(handle: HANDLE) -> Result<Self, InjectorError> {
        if handle.is_null() {
            return Err(InjectorError::ControlPipe {
                operation: "open host process",
                source: last_os_error(),
            });
        }
        // SAFETY: OpenProcess returned an owned process handle that must be closed.
        Ok(Self {
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
        })
    }

    fn wait_for_exit(&self) -> Result<(), InjectorError> {
        match unsafe { WaitForSingleObject(self.handle.as_raw_handle(), HOST_SHUTDOWN_TIMEOUT_MS) }
        {
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
