use std::ffi::c_void;

use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};
use tokio::sync::watch;
use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED};

use crate::control::protocol::MAX_CONTROL_MESSAGE_BYTES;
use crate::error::InjectorError;
use crate::platform::security::SecurityDescriptor;

pub(crate) struct ServerShutdown {
    requested: watch::Sender<bool>,
}

impl ServerShutdown {
    pub(crate) fn new() -> Self {
        let (requested, _) = watch::channel(false);
        Self { requested }
    }

    pub(crate) fn request(&self) {
        self.requested.send_replace(true);
    }

    async fn wait(&self) {
        let mut requested = self.requested.subscribe();
        if !*requested.borrow_and_update() {
            let _ = requested.changed().await;
        }
    }
}

pub(super) fn create_pipe(
    pipe_name: &str,
    first_instance: bool,
    security_descriptor: &SecurityDescriptor,
    max_instances: usize,
) -> Result<NamedPipeServer, InjectorError> {
    let mut security_attributes = security_descriptor.as_security_attributes();
    let mut options = ServerOptions::new();
    options
        .pipe_mode(PipeMode::Message)
        .first_pipe_instance(first_instance)
        .max_instances(max_instances)
        .in_buffer_size(MAX_CONTROL_MESSAGE_BYTES as u32)
        .out_buffer_size(MAX_CONTROL_MESSAGE_BYTES as u32)
        .reject_remote_clients(true);
    let pipe = unsafe {
        options.create_with_security_attributes_raw(
            pipe_name,
            std::ptr::from_mut(&mut security_attributes).cast::<c_void>(),
        )
    };
    pipe.map_err(|source| InjectorError::ControlPipe {
        operation: "create server pipe",
        source,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnectOutcome {
    Connected,
    Abandoned,
    Shutdown,
}

pub(super) async fn connect(
    pipe: &NamedPipeServer,
    shutdown: &ServerShutdown,
) -> Result<ConnectOutcome, InjectorError> {
    tokio::select! {
        biased;
        () = shutdown.wait() => Ok(ConnectOutcome::Shutdown),
        result = pipe.connect() => match result {
            Ok(()) => Ok(ConnectOutcome::Connected),
            Err(source) if is_disconnected_pipe_error(&source) => Ok(ConnectOutcome::Abandoned),
            Err(source) => Err(InjectorError::ControlPipe {
                operation: "connect server pipe",
                source,
            }),
        },
    }
}

fn is_disconnected_pipe_error(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code)
            if code == ERROR_BROKEN_PIPE as i32
                || code == ERROR_NO_DATA as i32
                || code == ERROR_PIPE_NOT_CONNECTED as i32
    )
}

#[cfg(test)]
mod tests {
    use super::is_disconnected_pipe_error;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED,
    };

    #[test]
    fn disconnected_pipe_errors_are_client_connection_abandonment() {
        for code in [ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED] {
            assert!(is_disconnected_pipe_error(
                &std::io::Error::from_raw_os_error(code as i32)
            ));
        }
        assert!(!is_disconnected_pipe_error(
            &std::io::Error::from_raw_os_error(ERROR_ACCESS_DENIED as i32)
        ));
    }
}
