use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;

use crate::control::protocol::{MAX_CONTROL_MESSAGE_BYTES, validate_message_len};
use crate::error::InjectorError;

pub(crate) mod client;
pub(crate) mod protocol;
pub(crate) mod server;

pub(crate) fn build_runtime(
    operation: &'static str,
) -> Result<tokio::runtime::Runtime, InjectorError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|source| InjectorError::ControlPipe { operation, source })
}

pub(crate) fn current_pipe_name() -> Result<String, InjectorError> {
    let session_id = current_session_id()?;
    Ok(format!(r"\\.\pipe\dwm-lut-rs-{session_id}"))
}

fn current_session_id() -> Result<u32, InjectorError> {
    let mut session_id = 0u32;
    let pid = unsafe { GetCurrentProcessId() };
    let ok = unsafe { ProcessIdToSessionId(pid, &mut session_id) };
    if ok == 0 {
        return Err(InjectorError::ControlPipe {
            operation: "resolve current session",
            source: last_os_error(),
        });
    }

    Ok(session_id)
}

pub(crate) fn last_os_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

async fn read_message<T>(
    pipe: &mut T,
    timeout: Duration,
    operation: &'static str,
) -> Result<Vec<u8>, InjectorError>
where
    T: AsyncRead + Unpin,
{
    let mut buffer = vec![0u8; MAX_CONTROL_MESSAGE_BYTES];
    let read = tokio::time::timeout(timeout, pipe.read(&mut buffer))
        .await
        .map_err(|_| InjectorError::ControlTimeout { operation })?
        .map_err(|source| InjectorError::ControlPipe { operation, source })?;
    validate_message_len(read)?;
    buffer.truncate(read);
    Ok(buffer)
}

async fn write_message<T>(
    pipe: &mut T,
    bytes: &[u8],
    timeout: Duration,
    operation: &'static str,
) -> Result<(), InjectorError>
where
    T: AsyncWrite + Unpin,
{
    validate_message_len(bytes.len())?;
    let written = tokio::time::timeout(timeout, pipe.write(bytes))
        .await
        .map_err(|_| InjectorError::ControlTimeout { operation })?
        .map_err(|source| InjectorError::ControlPipe { operation, source })?;
    if written != bytes.len() {
        return Err(InjectorError::ControlProtocol(format!(
            "partial {operation}: wrote {written} of {} bytes",
            bytes.len()
        )));
    }
    Ok(())
}
