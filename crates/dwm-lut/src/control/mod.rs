use std::fmt;
use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use windows_sys::Win32::Foundation::GetLastError;

use crate::control::protocol::{MAX_CONTROL_MESSAGE_BYTES, validate_message_len};
use crate::platform::session;

pub(crate) mod client;
pub(crate) mod protocol;
pub(crate) mod server;

#[derive(Debug)]
pub enum ControlError {
    EndpointMissing,
    EndpointBusy,
    Io {
        operation: &'static str,
        source: io::Error,
    },
    Timeout {
        operation: &'static str,
    },
    Protocol(String),
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EndpointMissing => write!(
                f,
                "dwm-lut host instance is not running; start it with `dwm-lut-cli host start`"
            ),
            Self::EndpointBusy => write!(
                f,
                "dwm-lut host instance is busy; retry after the current command finishes"
            ),
            Self::Io { operation, source } => {
                write!(f, "control {operation} failed: {source}")
            }
            Self::Timeout { operation } => write!(f, "control {operation} timed out"),
            Self::Protocol(message) => write!(f, "control protocol failed: {message}"),
        }
    }
}

impl std::error::Error for ControlError {}

impl ControlError {
    pub(crate) fn is_transient_host_probe(&self) -> bool {
        match self {
            Self::EndpointMissing | Self::EndpointBusy | Self::Timeout { .. } => true,
            Self::Io { source, .. } => matches!(
                source.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::NotConnected
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::UnexpectedEof
            ),
            Self::Protocol(_) => false,
        }
    }
}

pub(crate) fn build_runtime(
    operation: &'static str,
) -> Result<tokio::runtime::Runtime, ControlError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|source| ControlError::Io { operation, source })
}

pub(crate) fn current_pipe_name() -> Result<String, ControlError> {
    let session_id = session::current_session_id().map_err(|source| ControlError::Io {
        operation: "resolve current session",
        source,
    })?;
    Ok(format!(r"\\.\pipe\dwm-lut-rs-{session_id}"))
}

pub(crate) fn last_os_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

async fn read_message<T>(
    pipe: &mut T,
    timeout: Duration,
    operation: &'static str,
) -> Result<Vec<u8>, ControlError>
where
    T: AsyncRead + Unpin,
{
    let mut buffer = vec![0u8; MAX_CONTROL_MESSAGE_BYTES];
    let read = tokio::time::timeout(timeout, pipe.read(&mut buffer))
        .await
        .map_err(|_| ControlError::Timeout { operation })?
        .map_err(|source| ControlError::Io { operation, source })?;
    validate_message_len(read)?;
    buffer.truncate(read);
    Ok(buffer)
}

async fn write_message<T>(
    pipe: &mut T,
    bytes: &[u8],
    timeout: Duration,
    operation: &'static str,
) -> Result<(), ControlError>
where
    T: AsyncWrite + Unpin,
{
    validate_message_len(bytes.len())?;
    let written = tokio::time::timeout(timeout, pipe.write(bytes))
        .await
        .map_err(|_| ControlError::Timeout { operation })?
        .map_err(|source| ControlError::Io { operation, source })?;
    if written != bytes.len() {
        return Err(ControlError::Protocol(format!(
            "partial {operation}: wrote {written} of {} bytes",
            bytes.len()
        )));
    }
    Ok(())
}
