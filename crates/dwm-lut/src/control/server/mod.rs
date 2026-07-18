use std::sync::Arc;

use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::control::protocol::{
    CONTROL_PROTOCOL_VERSION, ControlCommand, ControlResponse, decode_request,
    decode_request_protocol_version, encode_response,
};
use crate::control::{build_runtime, current_pipe_name, read_message, write_message};
use crate::error::InjectorError;
use crate::platform::security::{SecurityDescriptor, UserSid};

mod pipe;

pub(crate) use pipe::ServerShutdown;
use pipe::{ConnectOutcome, connect, create_pipe};

const MAX_WORKER_THREADS: usize = 8;
const MAX_PIPE_INSTANCES: usize = MAX_WORKER_THREADS + 1;
const REQUEST_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const RESPONSE_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

pub(crate) fn run_server(
    handler: Arc<dyn ControlHandler>,
    shutdown: Arc<ServerShutdown>,
    on_ready: impl FnOnce() -> Result<(), InjectorError>,
) -> Result<(), InjectorError> {
    let runtime = build_runtime("create control server runtime")?;
    runtime.block_on(run_server_async(handler, shutdown, on_ready))
}

async fn run_server_async(
    handler: Arc<dyn ControlHandler>,
    shutdown: Arc<ServerShutdown>,
    on_ready: impl FnOnce() -> Result<(), InjectorError>,
) -> Result<(), InjectorError> {
    let pipe_name = current_pipe_name()?;
    let host_user_sid = UserSid::current_process()?;
    let pipe_security = SecurityDescriptor::read_write_for_user(&host_user_sid)?;
    let worker_slots = Arc::new(Semaphore::new(MAX_WORKER_THREADS));
    let mut workers = JoinSet::new();
    let pipe = create_pipe(&pipe_name, true, &pipe_security, MAX_PIPE_INSTANCES)?;
    on_ready()?;
    println!("dwm-lut host instance is running on {pipe_name}");
    let result = run_accept_loop(
        handler,
        Arc::clone(&shutdown),
        worker_slots,
        &mut workers,
        pipe,
        &pipe_name,
        &pipe_security,
    )
    .await;
    if result.is_err() {
        shutdown.request();
    }
    while let Some(result) = workers.join_next().await {
        if let Err(error) = result {
            eprintln!("control worker task failed: {error}");
        }
    }
    result
}

async fn run_accept_loop(
    handler: Arc<dyn ControlHandler>,
    shutdown: Arc<ServerShutdown>,
    worker_slots: Arc<Semaphore>,
    workers: &mut JoinSet<()>,
    mut pipe: NamedPipeServer,
    pipe_name: &str,
    pipe_security: &SecurityDescriptor,
) -> Result<(), InjectorError> {
    loop {
        while let Some(result) = workers.try_join_next() {
            if let Err(error) = result {
                eprintln!("control worker task failed: {error}");
            }
        }
        let worker_slot = Arc::clone(&worker_slots)
            .acquire_owned()
            .await
            .expect("control worker semaphore must remain open");
        match connect(&pipe, &shutdown).await {
            Ok(ConnectOutcome::Connected) => {}
            Ok(ConnectOutcome::Shutdown) => {
                drop(worker_slot);
                return Ok(());
            }
            Ok(ConnectOutcome::Abandoned) => {
                drop(worker_slot);
                pipe = create_pipe(pipe_name, false, pipe_security, MAX_PIPE_INSTANCES)?;
                continue;
            }
            Err(error) => {
                drop(worker_slot);
                eprintln!("{error}");
                pipe = create_pipe(pipe_name, false, pipe_security, MAX_PIPE_INSTANCES)?;
                continue;
            }
        }

        let handler = Arc::clone(&handler);
        workers.spawn(async move {
            let _worker_slot = worker_slot;
            if let Err(error) = handle_connected_client(pipe, handler).await {
                eprintln!("{error}");
            }
        });
        pipe = create_pipe(pipe_name, false, pipe_security, MAX_PIPE_INSTANCES)?;
    }
}

async fn handle_connected_client(
    mut pipe: NamedPipeServer,
    handler: Arc<dyn ControlHandler>,
) -> Result<(), InjectorError> {
    let bytes = match read_message(&mut pipe, REQUEST_READ_TIMEOUT, "read control request").await {
        Ok(bytes) => bytes,
        Err(error @ InjectorError::ControlTimeout { .. }) => return Err(error),
        Err(error) => {
            let response = response_from_injector_error(error);
            let response = encode_response(&response)?;
            return write_message(
                &mut pipe,
                &response,
                RESPONSE_WRITE_TIMEOUT,
                "write control response",
            )
            .await;
        }
    };
    let dispatch =
        tokio::task::spawn_blocking(move || handle_control_request_bytes(&bytes, handler.as_ref()))
            .await
            .map_err(|source| {
                InjectorError::ControlProtocol(format!("control handler task failed: {source}"))
            })?;
    let dispatch = match dispatch {
        Ok(dispatch) => dispatch,
        Err(error) => ControlDispatch::immediate(response_from_injector_error(error)),
    };
    let response = encode_response(&dispatch.response)?;
    write_message(
        &mut pipe,
        &response,
        RESPONSE_WRITE_TIMEOUT,
        "write control response",
    )
    .await?;
    dispatch.complete()?;
    Ok(())
}

pub(crate) trait ControlHandler: Send + Sync {
    fn dispatch(&self, command: ControlCommand) -> ControlDispatch;
}

fn response_from_injector_error(error: InjectorError) -> ControlResponse {
    ControlResponse::error(
        error.to_string(),
        crate::control::protocol::ControlStatus::Error,
    )
}

pub(crate) struct ControlDispatch {
    response: ControlResponse,
    completion: Option<Box<dyn FnOnce() -> Result<(), InjectorError> + Send>>,
}

impl ControlDispatch {
    pub(crate) fn immediate(response: ControlResponse) -> Self {
        Self {
            response,
            completion: None,
        }
    }

    pub(crate) fn after_response(
        response: ControlResponse,
        completion: impl FnOnce() -> Result<(), InjectorError> + Send + 'static,
    ) -> Self {
        Self {
            response,
            completion: Some(Box::new(completion)),
        }
    }

    #[cfg(test)]
    pub(crate) fn response(&self) -> &ControlResponse {
        &self.response
    }

    pub(crate) fn complete(self) -> Result<(), InjectorError> {
        match self.completion {
            Some(completion) => completion(),
            None => Ok(()),
        }
    }
}

fn handle_control_request_bytes(
    bytes: &[u8],
    handler: &dyn ControlHandler,
) -> Result<ControlDispatch, InjectorError> {
    let peer_version = decode_request_protocol_version(bytes)?;
    if peer_version != CONTROL_PROTOCOL_VERSION {
        return Ok(ControlDispatch::immediate(
            ControlResponse::protocol_mismatch(peer_version),
        ));
    }
    let request = decode_request(bytes)?;
    Ok(handler.dispatch(request.command))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use tokio::net::windows::named_pipe::{ClientOptions, PipeMode};

    use crate::control::protocol::{
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlRequest, ControlStatus,
        MAX_CONTROL_MESSAGE_BYTES, decode_response, encode_request,
    };
    use crate::control::{read_message, write_message};
    use crate::platform::security::{SecurityDescriptor, UserSid};

    use super::pipe::{connect, create_pipe};
    use super::{
        ConnectOutcome, ControlDispatch, ControlHandler, REQUEST_READ_TIMEOUT,
        RESPONSE_WRITE_TIMEOUT, ServerShutdown, handle_connected_client,
        handle_control_request_bytes,
    };

    static TEST_PIPE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct RecordingHandler {
        command: Mutex<Option<ControlCommand>>,
    }

    impl RecordingHandler {
        fn new() -> Self {
            Self {
                command: Mutex::new(None),
            }
        }
    }

    impl ControlHandler for RecordingHandler {
        fn dispatch(&self, command: ControlCommand) -> ControlDispatch {
            *self.command.lock().unwrap() = Some(command);
            ControlDispatch::immediate(crate::control::protocol::ControlResponse::ok(
                "ok",
                ControlStatus::Running,
            ))
        }
    }

    #[test]
    fn different_protocol_version_rejects_unknown_command_before_command_decode() {
        let handler = RecordingHandler::new();
        let request = format!(
            r#"{{"protocol_version":{},"command":"future_command"}}"#,
            CONTROL_PROTOCOL_VERSION + 1
        );

        let dispatch = handle_control_request_bytes(request.as_bytes(), &handler).unwrap();

        assert!(!dispatch.response.ok);
        assert_eq!(dispatch.response.status, ControlStatus::ProtocolMismatch);
        assert!(handler.command.lock().unwrap().is_none());
    }

    #[test]
    fn shutdown_interrupts_pending_accept() {
        let runtime = crate::control::build_runtime("create control server test runtime").unwrap();
        runtime.block_on(async {
            let pipe_name = test_pipe_name();
            let pipe_security = test_pipe_security();
            let server = create_pipe(&pipe_name, true, &pipe_security, 1).unwrap();
            let shutdown = ServerShutdown::new();
            let pending_accept = connect(&server, &shutdown);
            tokio::pin!(pending_accept);

            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(10),
                    pending_accept.as_mut()
                )
                .await
                .is_err()
            );
            shutdown.request();

            let outcome =
                tokio::time::timeout(std::time::Duration::from_secs(1), pending_accept.as_mut())
                    .await
                    .expect("shutdown should interrupt pending accept")
                    .unwrap();
            assert_eq!(outcome, ConnectOutcome::Shutdown);
        });
    }

    #[test]
    fn tokio_named_pipe_round_trip_dispatches_request() {
        let runtime = crate::control::build_runtime("create control server test runtime").unwrap();
        runtime.block_on(async {
            let pipe_name = test_pipe_name();
            let pipe_security = test_pipe_security();
            let server = create_pipe(&pipe_name, true, &pipe_security, 1).unwrap();
            let handler = Arc::new(RecordingHandler::new());
            let server_handler: Arc<dyn ControlHandler> = handler.clone();

            let mut options = ClientOptions::new();
            options.pipe_mode(PipeMode::Message);
            let mut client = options.open(&pipe_name).unwrap();
            let worker = tokio::spawn(async move {
                server.connect().await.unwrap();
                handle_connected_client(server, server_handler)
                    .await
                    .unwrap();
            });

            let request = encode_request(&ControlRequest {
                protocol_version: CONTROL_PROTOCOL_VERSION,
                command: ControlCommand::Status,
            })
            .unwrap();
            write_message(
                &mut client,
                &request,
                REQUEST_READ_TIMEOUT,
                "write test request",
            )
            .await
            .unwrap();
            let response = read_message(&mut client, RESPONSE_WRITE_TIMEOUT, "read test response")
                .await
                .unwrap();
            let response = decode_response(&response).unwrap();
            worker.await.unwrap();

            assert!(response.ok);
            assert_eq!(response.status, ControlStatus::Running);
            assert_eq!(
                *handler.command.lock().unwrap(),
                Some(ControlCommand::Status)
            );
        });
    }

    #[test]
    fn tokio_named_pipe_reads_maximum_message() {
        let runtime = crate::control::build_runtime("create control server test runtime").unwrap();
        runtime.block_on(async {
            let pipe_name = test_pipe_name();
            let pipe_security = test_pipe_security();
            let mut server = create_pipe(&pipe_name, true, &pipe_security, 1).unwrap();

            let mut options = ClientOptions::new();
            options.pipe_mode(PipeMode::Message);
            let mut client = options.open(&pipe_name).unwrap();
            server.connect().await.unwrap();

            let expected = vec![b'x'; MAX_CONTROL_MESSAGE_BYTES];
            write_message(
                &mut client,
                &expected,
                REQUEST_READ_TIMEOUT,
                "write maximum test message",
            )
            .await
            .unwrap();
            let actual = read_message(
                &mut server,
                REQUEST_READ_TIMEOUT,
                "read maximum test message",
            )
            .await
            .unwrap();

            assert_eq!(actual, expected);
        });
    }

    fn test_pipe_name() -> String {
        let sequence = TEST_PIPE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        format!(
            r"\\.\pipe\dwm-lut-rs-test-{}-{sequence}",
            std::process::id()
        )
    }

    fn test_pipe_security() -> SecurityDescriptor {
        let user_sid = UserSid::current_process().unwrap();
        SecurityDescriptor::read_write_for_user(&user_sid).unwrap()
    }
}
