use std::sync::{Arc, Condvar, Mutex};

use crate::control::current_pipe_name;
use crate::control::protocol::{
    CONTROL_PROTOCOL_VERSION, ControlCommand, ControlResponse, decode_request,
    decode_request_protocol_version, encode_response,
};
use crate::error::InjectorError;
use crate::security::UserSid;

mod pipe;

pub(crate) use pipe::ServerShutdown;
use pipe::{ConnectOutcome, PipeHandle, create_pipe, create_pipe_for_accept_loop};

const MAX_WORKER_THREADS: usize = 8;
const MAX_PIPE_INSTANCES: u32 = MAX_WORKER_THREADS as u32 + 1;

pub(crate) fn run_server(
    handler: Arc<dyn ControlHandler>,
    shutdown: Arc<ServerShutdown>,
    on_ready: impl FnOnce() -> Result<(), InjectorError>,
) -> Result<(), InjectorError> {
    let pipe_name = current_pipe_name()?;
    let host_user_sid = UserSid::current_process()?;
    let worker_slots = Arc::new(WorkerSlots::new(MAX_WORKER_THREADS));
    let pipe = create_pipe(&pipe_name, true, &host_user_sid, MAX_PIPE_INSTANCES)?;
    on_ready()?;
    println!("dwm-lut host instance is running on {pipe_name}");
    let result = run_accept_loop(
        handler,
        Arc::clone(&shutdown),
        Arc::clone(&worker_slots),
        pipe,
        &pipe_name,
        &host_user_sid,
    );
    if result.is_err() {
        let _ = shutdown.request();
    }
    worker_slots.wait_until_idle();
    result
}

fn run_accept_loop(
    handler: Arc<dyn ControlHandler>,
    shutdown: Arc<ServerShutdown>,
    worker_slots: Arc<WorkerSlots>,
    mut pipe: PipeHandle,
    pipe_name: &str,
    host_user_sid: &UserSid,
) -> Result<(), InjectorError> {
    loop {
        let worker_slot = worker_slots.acquire();
        match pipe.connect(&shutdown) {
            Ok(ConnectOutcome::Connected) => {}
            Ok(ConnectOutcome::Shutdown) => {
                drop(worker_slot);
                pipe.disconnect();
                return Ok(());
            }
            Ok(ConnectOutcome::Abandoned) => {
                drop(worker_slot);
                pipe.disconnect();
                pipe = create_pipe_for_accept_loop(pipe_name, host_user_sid, MAX_PIPE_INSTANCES)?;
                continue;
            }
            Err(error) => {
                drop(worker_slot);
                eprintln!("{error}");
                pipe.disconnect();
                pipe = create_pipe_for_accept_loop(pipe_name, host_user_sid, MAX_PIPE_INSTANCES)?;
                continue;
            }
        }

        let handler = Arc::clone(&handler);
        std::thread::spawn(move || {
            let _worker_slot = worker_slot;
            if let Err(error) = handle_connected_client(&pipe, handler) {
                eprintln!("{error}");
            }
            pipe.disconnect();
        });
        pipe = create_pipe_for_accept_loop(pipe_name, host_user_sid, MAX_PIPE_INSTANCES)?;
    }
}

fn handle_connected_client(
    pipe: &PipeHandle,
    handler: Arc<dyn ControlHandler>,
) -> Result<(), InjectorError> {
    let bytes = match pipe.read_message() {
        Ok(bytes) => bytes,
        Err(error @ InjectorError::ControlTimeout { .. }) => return Err(error),
        Err(error) => {
            let response = crate::host::response_from_injector_error(error);
            let response = encode_response(&response)?;
            return pipe.write_message(&response);
        }
    };
    let dispatch = match handle_control_request_bytes(&bytes, handler.as_ref()) {
        Ok(dispatch) => dispatch,
        Err(error) => ControlDispatch::immediate(crate::host::response_from_injector_error(error)),
    };
    let response = encode_response(dispatch.response())?;
    pipe.write_message(&response)?;
    dispatch.complete()?;
    Ok(())
}

pub(crate) trait ControlHandler: Send + Sync {
    fn dispatch(&self, command: ControlCommand) -> ControlDispatch;
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

    pub(crate) fn response(&self) -> &ControlResponse {
        &self.response
    }

    pub(crate) fn complete(mut self) -> Result<(), InjectorError> {
        match self.completion.take() {
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
        self.available.notify_all();
    }

    fn wait_until_idle(&self) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        while state.active != 0 {
            state = match self.available.wait(state) {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
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
    use std::sync::{Arc, Mutex};

    use crate::control::protocol::{
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlRequest, ControlStatus, encode_request,
    };

    use super::{ControlDispatch, ControlHandler, ServerShutdown, handle_control_request_bytes};

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
    fn matching_protocol_version_dispatches_command() {
        let handler = RecordingHandler::new();
        let request = encode_request(&ControlRequest {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            command: ControlCommand::Status,
        })
        .unwrap();

        let dispatch = handle_control_request_bytes(&request, &handler).unwrap();

        assert!(dispatch.response.ok);
        assert_eq!(
            *handler.command.lock().unwrap(),
            Some(ControlCommand::Status)
        );
    }

    #[test]
    fn different_protocol_version_rejects_without_dispatching_command() {
        let handler = RecordingHandler::new();
        let request = encode_request(&ControlRequest {
            protocol_version: CONTROL_PROTOCOL_VERSION + 1,
            command: ControlCommand::Status,
        })
        .unwrap();

        let dispatch = handle_control_request_bytes(&request, &handler).unwrap();

        assert!(!dispatch.response.ok);
        assert_eq!(dispatch.response.status, ControlStatus::ProtocolMismatch);
        assert!(handler.command.lock().unwrap().is_none());
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
    fn malformed_request_bytes_are_rejected_before_dispatch() {
        let handler = RecordingHandler::new();

        let result =
            handle_control_request_bytes(br#"{"protocol_version":1,"command":"status""#, &handler);

        assert!(result.is_err());
        assert!(handler.command.lock().unwrap().is_none());
    }

    #[test]
    fn completion_runs_only_when_dispatch_is_completed() {
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let completed_by_callback = Arc::clone(&completed);
        let dispatch = ControlDispatch::after_response(
            crate::control::protocol::ControlResponse::ok("ok", ControlStatus::Stopped),
            move || {
                completed_by_callback.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        );

        assert!(!completed.load(std::sync::atomic::Ordering::SeqCst));
        dispatch.complete().unwrap();
        assert!(completed.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn shutdown_event_records_request() {
        let shutdown = ServerShutdown::new().expect("shutdown event should be created");
        assert!(!shutdown.is_requested().expect("event should be readable"));

        shutdown.request().expect("shutdown should be signaled");

        assert!(shutdown.is_requested().expect("event should be readable"));
    }
}
