use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{
    ERROR_IO_PENDING, ERROR_MORE_DATA, ERROR_PIPE_CONNECTED, FALSE, GENERIC_READ, GENERIC_WRITE,
    HANDLE, INVALID_HANDLE_VALUE, TRUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetExitCodeProcess, GetProcessId, SetEvent, WaitForMultipleObjects,
    WaitForSingleObject,
};

use crate::error::InjectorError;
use crate::platform::elevation;
use crate::platform::security::{SecurityDescriptor, UserSid};

const STARTUP_RESULT_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_TERMINATION_TIMEOUT: Duration = Duration::from_secs(5);
const STARTUP_RESULT_OK: &str = "ok\n";
const STARTUP_RESULT_ACK: &str = "ack\n";
const STARTUP_RESULT_ERROR_PREFIX: &str = "err\n";
const STARTUP_RESULT_MAX_BYTES: usize = 16 * 1024;
const STARTUP_FAILURE_KIND_ERROR: &str = "error";
const STARTUP_FAILURE_KIND_HOST_ALREADY_RUNNING: &str = "host_already_running";
static STARTUP_PIPE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct StartupNotifier {
    pipe: Arc<OwnedHandle>,
    ready_for_ack: Arc<AtomicBool>,
    acknowledgement: mpsc::Receiver<Result<(), InjectorError>>,
}

impl StartupNotifier {
    pub(crate) fn connect(pipe_name: String) -> Result<Self, InjectorError> {
        let pipe_name = OsStr::new(&pipe_name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let handle = unsafe {
            CreateFileW(
                pipe_name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(InjectorError::HostLaunchFailed {
                operation: "connect startup result pipe",
                source: last_os_error(),
            });
        }

        // SAFETY: CreateFileW returned an owned pipe handle that must be closed.
        let pipe = Arc::new(unsafe { OwnedHandle::from_raw_handle(handle) });
        let ready_for_ack = Arc::new(AtomicBool::new(false));
        let (acknowledgement_sender, acknowledgement) = mpsc::sync_channel(1);
        let watcher_pipe = Arc::clone(&pipe);
        let watcher_ready_for_ack = Arc::clone(&ready_for_ack);
        std::thread::Builder::new()
            .name("dwm-lut-startup-channel".to_string())
            .spawn(move || {
                watch_startup_channel(watcher_pipe, watcher_ready_for_ack, acknowledgement_sender);
            })
            .map_err(|source| InjectorError::HostLaunchFailed {
                operation: "start startup channel watcher",
                source,
            })?;

        Ok(Self {
            pipe,
            ready_for_ack,
            acknowledgement,
        })
    }

    pub(crate) fn notify_success(self) -> Result<(), InjectorError> {
        self.ready_for_ack.store(true, Ordering::Release);
        write_message_with_timeout(
            self.pipe.as_raw_handle(),
            STARTUP_RESULT_OK.as_bytes(),
            STARTUP_RESULT_TIMEOUT,
            "write startup result",
        )?;
        match self.acknowledgement.recv_timeout(STARTUP_RESULT_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(InjectorError::HostStartupFailed(
                "read startup result acknowledgement timed out".to_string(),
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(InjectorError::HostStartupFailed(
                "startup channel watcher stopped before acknowledgement".to_string(),
            )),
        }
    }

    pub(crate) fn notify_failure(self, error: &InjectorError) -> Result<(), InjectorError> {
        let kind = startup_failure_kind(error);
        let message = error.to_string();
        let mut bytes =
            Vec::with_capacity(STARTUP_RESULT_ERROR_PREFIX.len() + kind.len() + 1 + message.len());
        bytes.extend_from_slice(STARTUP_RESULT_ERROR_PREFIX.as_bytes());
        bytes.extend_from_slice(kind.as_bytes());
        bytes.push(b'\n');
        bytes.extend_from_slice(message.as_bytes());
        if bytes.len() > STARTUP_RESULT_MAX_BYTES {
            return Err(InjectorError::HostStartupFailed(
                "startup result exceeded maximum length".to_string(),
            ));
        }
        write_message_with_timeout(
            self.pipe.as_raw_handle(),
            &bytes,
            STARTUP_RESULT_TIMEOUT,
            "write startup result",
        )
    }
}

fn watch_startup_channel(
    pipe: Arc<OwnedHandle>,
    ready_for_ack: Arc<AtomicBool>,
    acknowledgement: mpsc::SyncSender<Result<(), InjectorError>>,
) {
    let result = read_message_waiting(
        pipe.as_raw_handle(),
        STARTUP_RESULT_ACK.len(),
        "read startup result acknowledgement",
    )
    .and_then(|message| {
        if !ready_for_ack.load(Ordering::Acquire) {
            return Err(InjectorError::HostStartupFailed(
                "startup result acknowledgement arrived before startup result".to_string(),
            ));
        }
        parse_startup_ack(&message)
    });
    if result.is_err() {
        crate::panic_report::abort_startup();
    }
    let _ = acknowledgement.send(result);
}

pub(super) struct StartupEvent {
    name: String,
    event: OwnedHandle,
}

impl StartupEvent {
    pub(super) fn new(kind: &str, operation: &'static str) -> Result<Self, InjectorError> {
        let name = startup_event_name(kind);
        let name_wide = OsStr::new(&name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let user_sid = UserSid::current_process()?;
        let security_descriptor = SecurityDescriptor::read_write_for_user(&user_sid)?;
        let security_attributes = security_descriptor.as_security_attributes();
        let event = unsafe { CreateEventW(&security_attributes, TRUE, FALSE, name_wide.as_ptr()) };
        if event.is_null() {
            return Err(InjectorError::HostLaunchFailed {
                operation,
                source: last_os_error(),
            });
        }
        Ok(Self {
            name,
            // SAFETY: CreateEventW returned an owned event handle that must be closed.
            event: unsafe { OwnedHandle::from_raw_handle(event) },
        })
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    fn handle(&self) -> HANDLE {
        self.event.as_raw_handle()
    }

    #[cfg(test)]
    fn signal(&self, operation: &'static str) -> Result<(), InjectorError> {
        if unsafe { SetEvent(self.handle()) } == FALSE {
            return Err(InjectorError::HostLaunchFailed {
                operation,
                source: last_os_error(),
            });
        }
        Ok(())
    }

    fn ensure_not_reported(&self) -> Result<(), InjectorError> {
        match unsafe { WaitForSingleObject(self.handle(), 0) } {
            WAIT_OBJECT_0 => Err(InjectorError::HostPanicAlreadyReported),
            WAIT_TIMEOUT => Ok(()),
            WAIT_FAILED => Err(InjectorError::HostLaunchFailed {
                operation: "check panic report event",
                source: last_os_error(),
            }),
            value => Err(InjectorError::HostStartupFailed(format!(
                "unexpected panic report event wait result {value:#x}"
            ))),
        }
    }
}

pub(super) struct StartupResultPipe {
    name: String,
    connect: OverlappedOperation,
    pipe: OwnedHandle,
}

impl StartupResultPipe {
    pub(super) fn new() -> Result<Self, InjectorError> {
        let name = startup_pipe_name();
        let name_wide = OsStr::new(&name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let user_sid = UserSid::current_process()?;
        let security_descriptor = SecurityDescriptor::read_write_for_user(&user_sid)?;
        let security_attributes = security_descriptor.as_security_attributes();
        let pipe = unsafe {
            CreateNamedPipeW(
                name_wide.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                STARTUP_RESULT_MAX_BYTES as u32,
                STARTUP_RESULT_MAX_BYTES as u32,
                0,
                &security_attributes,
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            return Err(InjectorError::HostLaunchFailed {
                operation: "create named startup result pipe",
                source: last_os_error(),
            });
        }
        // SAFETY: CreateNamedPipeW returned an owned pipe handle that must be closed.
        let pipe = unsafe { OwnedHandle::from_raw_handle(pipe) };
        let mut connect = OverlappedOperation::new("create startup pipe connection event")?;
        let ok = unsafe { ConnectNamedPipe(pipe.as_raw_handle(), connect.as_mut_ptr()) };
        if ok != FALSE {
            unsafe {
                SetEvent(connect.event());
            }
        } else {
            let error = last_os_error();
            match error.raw_os_error() {
                Some(code) if code == ERROR_IO_PENDING as i32 => {
                    connect.mark_pending(pipe.as_raw_handle())
                }
                Some(code) if code == ERROR_PIPE_CONNECTED as i32 => unsafe {
                    SetEvent(connect.event());
                },
                _ => {
                    return Err(InjectorError::HostLaunchFailed {
                        operation: "wait for startup result pipe client",
                        source: error,
                    });
                }
            }
        }

        Ok(Self {
            name,
            connect,
            pipe,
        })
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn wait(
        mut self,
        process: &elevation::ElevatedProcess,
        panic_event: &StartupEvent,
    ) -> Result<(), InjectorError> {
        let deadline = Instant::now() + STARTUP_RESULT_TIMEOUT;
        let result = self.wait_inner(process, panic_event, deadline);
        match result {
            Ok(()) => Ok(()),
            Err(StartupWaitFailure::Error(error)) => Err(error),
            Err(StartupWaitFailure::Timeout) => {
                if self.connect.is_pending() {
                    self.connect.cancel_and_wait(self.pipe.as_raw_handle());
                }
                unsafe {
                    DisconnectNamedPipe(self.pipe.as_raw_handle());
                }
                drop(self);
                wait_after_startup_disconnect(process, panic_event)
            }
        }
    }

    fn wait_inner(
        &mut self,
        process: &elevation::ElevatedProcess,
        panic_event: &StartupEvent,
        deadline: Instant,
    ) -> StartupWaitResult<()> {
        wait_for_startup_operation(self.connect.event(), panic_event, process, deadline)?;
        if self.connect.is_pending() {
            self.connect.result(
                self.pipe.as_raw_handle(),
                "complete startup result pipe connection",
            )?;
        }

        let expected_pid = unsafe { GetProcessId(process.handle()) };
        let mut actual_pid = 0;
        let ok = unsafe { GetNamedPipeClientProcessId(self.pipe.as_raw_handle(), &mut actual_pid) };
        if ok == FALSE {
            return Err(InjectorError::HostLaunchFailed {
                operation: "identify startup result pipe client",
                source: last_os_error(),
            }
            .into());
        }
        if expected_pid == 0 || actual_pid != expected_pid {
            return Err(InjectorError::HostStartupFailed(
                "startup result pipe client did not match the launched host process".to_string(),
            )
            .into());
        }

        let message = read_startup_result(&self.pipe, panic_event, process, deadline)?;
        panic_event.ensure_not_reported()?;
        parse_startup_result(&message)?;
        write_startup_ack(&self.pipe, panic_event, process, deadline)?;
        panic_event.ensure_not_reported()?;
        Ok(())
    }
}

enum StartupWaitFailure {
    Timeout,
    Error(InjectorError),
}

type StartupWaitResult<T> = Result<T, StartupWaitFailure>;

impl From<InjectorError> for StartupWaitFailure {
    fn from(error: InjectorError) -> Self {
        Self::Error(error)
    }
}

fn startup_pipe_name() -> String {
    let pid = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = STARTUP_PIPE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(r"\\.\pipe\dwm-lut-rs-startup-{pid}-{nonce:032x}-{sequence:016x}")
}

fn startup_event_name(kind: &str) -> String {
    let pid = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = STARTUP_PIPE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(r"Local\dwm-lut-rs-{kind}-{pid}-{nonce:032x}-{sequence:016x}")
}

fn parse_startup_result(message: &str) -> Result<(), InjectorError> {
    if message == STARTUP_RESULT_OK {
        return Ok(());
    }
    if let Some(failure) = message.strip_prefix(STARTUP_RESULT_ERROR_PREFIX) {
        let Some((kind, message)) = failure.split_once('\n') else {
            return Err(InjectorError::HostStartupFailed(
                "host process reported an invalid startup result".to_string(),
            ));
        };
        return match kind {
            STARTUP_FAILURE_KIND_HOST_ALREADY_RUNNING => Err(InjectorError::HostAlreadyRunning),
            STARTUP_FAILURE_KIND_ERROR => {
                Err(InjectorError::HostStartupFailed(message.to_string()))
            }
            _ => Err(InjectorError::HostStartupFailed(
                "host process reported an invalid startup result".to_string(),
            )),
        };
    }
    if message.is_empty() {
        return Err(InjectorError::HostStartupFailed(
            "host process exited before reporting startup".to_string(),
        ));
    }
    Err(InjectorError::HostStartupFailed(
        "host process reported an invalid startup result".to_string(),
    ))
}

fn parse_startup_ack(message: &str) -> Result<(), InjectorError> {
    if message == STARTUP_RESULT_ACK {
        return Ok(());
    }
    Err(InjectorError::HostStartupFailed(
        "startup result acknowledgement was invalid".to_string(),
    ))
}

fn startup_failure_kind(error: &InjectorError) -> &'static str {
    match error {
        InjectorError::HostAlreadyRunning => STARTUP_FAILURE_KIND_HOST_ALREADY_RUNNING,
        _ => STARTUP_FAILURE_KIND_ERROR,
    }
}

fn wait_for_startup_operation(
    operation_event: HANDLE,
    panic_event: &StartupEvent,
    process: &elevation::ElevatedProcess,
    deadline: Instant,
) -> StartupWaitResult<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(StartupWaitFailure::Timeout);
    }
    let timeout = u32::try_from(remaining.as_millis()).unwrap_or(u32::MAX);
    let handles = [panic_event.handle(), operation_event, process.handle()];
    let wait = unsafe { WaitForMultipleObjects(3, handles.as_ptr(), FALSE, timeout) };
    match wait {
        WAIT_OBJECT_0 => Err(InjectorError::HostPanicAlreadyReported.into()),
        value if value == WAIT_OBJECT_0 + 1 => Ok(()),
        value if value == WAIT_OBJECT_0 + 2 => Err(host_exited_before_startup(process).into()),
        WAIT_TIMEOUT => Err(StartupWaitFailure::Timeout),
        WAIT_FAILED => Err(InjectorError::HostLaunchFailed {
            operation: "wait for host startup result",
            source: last_os_error(),
        }
        .into()),
        value => Err(InjectorError::HostStartupFailed(format!(
            "unexpected host startup wait result {value:#x}"
        ))
        .into()),
    }
}

fn wait_after_startup_disconnect(
    process: &elevation::ElevatedProcess,
    panic_event: &StartupEvent,
) -> Result<(), InjectorError> {
    let timeout = u32::try_from(STARTUP_TERMINATION_TIMEOUT.as_millis()).unwrap_or(u32::MAX);
    let handles = [panic_event.handle(), process.handle()];
    match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), FALSE, timeout) } {
        WAIT_OBJECT_0 => Err(InjectorError::HostPanicAlreadyReported),
        value if value == WAIT_OBJECT_0 + 1 => Err(InjectorError::HostStartupFailed(
            "timed out waiting for host startup result".to_string(),
        )),
        WAIT_TIMEOUT => Err(InjectorError::HostStartupFailed(
            "host did not terminate after startup timeout".to_string(),
        )),
        WAIT_FAILED => Err(InjectorError::HostLaunchFailed {
            operation: "wait for host termination after startup timeout",
            source: last_os_error(),
        }),
        value => Err(InjectorError::HostStartupFailed(format!(
            "unexpected host termination wait result {value:#x}"
        ))),
    }
}

fn host_exited_before_startup(process: &elevation::ElevatedProcess) -> InjectorError {
    let mut exit_code = 0;
    let ok = unsafe { GetExitCodeProcess(process.handle(), &mut exit_code) };
    if ok == FALSE {
        return InjectorError::HostLaunchFailed {
            operation: "read host process exit code",
            source: last_os_error(),
        };
    }
    InjectorError::HostStartupFailed(format!(
        "host process exited before reporting startup with exit code {exit_code}"
    ))
}

fn read_startup_result(
    pipe: &OwnedHandle,
    panic_event: &StartupEvent,
    process: &elevation::ElevatedProcess,
    deadline: Instant,
) -> StartupWaitResult<String> {
    let mut bytes = vec![0u8; STARTUP_RESULT_MAX_BYTES];
    let mut operation = OverlappedOperation::new("create startup result read event")?;
    let mut read = 0u32;
    let ok = unsafe {
        ReadFile(
            pipe.as_raw_handle(),
            bytes.as_mut_ptr().cast(),
            bytes.len() as u32,
            &mut read,
            operation.as_mut_ptr(),
        )
    };
    if ok == FALSE {
        let error = last_os_error();
        match error.raw_os_error() {
            Some(code) if code == ERROR_IO_PENDING as i32 => {
                operation.mark_pending(pipe.as_raw_handle());
                wait_for_startup_operation(operation.event(), panic_event, process, deadline)?;
                read = operation.result(pipe.as_raw_handle(), "read startup result")?;
            }
            Some(code) if code == ERROR_MORE_DATA as i32 => {
                return Err(InjectorError::HostStartupFailed(
                    "startup result exceeded maximum length".to_string(),
                )
                .into());
            }
            _ => {
                return Err(InjectorError::HostLaunchFailed {
                    operation: "read startup result",
                    source: error,
                }
                .into());
            }
        }
    }

    bytes.truncate(read as usize);
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn write_startup_ack(
    pipe: &OwnedHandle,
    panic_event: &StartupEvent,
    process: &elevation::ElevatedProcess,
    deadline: Instant,
) -> StartupWaitResult<()> {
    let mut operation = OverlappedOperation::new("create startup acknowledgement write event")?;
    let mut written = 0u32;
    let bytes = STARTUP_RESULT_ACK.as_bytes();
    let ok = unsafe {
        WriteFile(
            pipe.as_raw_handle(),
            bytes.as_ptr().cast(),
            bytes.len() as u32,
            &mut written,
            operation.as_mut_ptr(),
        )
    };
    if ok == FALSE {
        let error = last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(InjectorError::HostLaunchFailed {
                operation: "write startup result acknowledgement",
                source: error,
            }
            .into());
        }
        operation.mark_pending(pipe.as_raw_handle());
        wait_for_startup_operation(operation.event(), panic_event, process, deadline)?;
        written = operation.result(pipe.as_raw_handle(), "write startup result acknowledgement")?;
    }
    verify_complete_write(bytes.len(), written, "startup result acknowledgement")?;
    Ok(())
}

fn write_message_with_timeout(
    handle: HANDLE,
    bytes: &[u8],
    timeout: Duration,
    operation_name: &'static str,
) -> Result<(), InjectorError> {
    let len = u32::try_from(bytes.len()).map_err(|_| {
        InjectorError::HostStartupFailed("startup result length does not fit u32".to_string())
    })?;
    let mut operation = OverlappedOperation::new("create startup result write event")?;
    let mut written = 0u32;
    let ok = unsafe {
        WriteFile(
            handle,
            bytes.as_ptr().cast(),
            len,
            &mut written,
            operation.as_mut_ptr(),
        )
    };
    if ok == FALSE {
        let error = last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(InjectorError::HostLaunchFailed {
                operation: operation_name,
                source: error,
            });
        }
        operation.mark_pending(handle);
        wait_for_event(operation.event(), timeout, operation_name)?;
        written = operation.result(handle, operation_name)?;
    }
    verify_complete_write(bytes.len(), written, "startup result")
}

fn read_message_waiting(
    handle: HANDLE,
    max_bytes: usize,
    operation_name: &'static str,
) -> Result<String, InjectorError> {
    let mut bytes = vec![0u8; max_bytes];
    let mut operation = OverlappedOperation::new("create startup acknowledgement read event")?;
    let mut read = 0u32;
    let ok = unsafe {
        ReadFile(
            handle,
            bytes.as_mut_ptr().cast(),
            bytes.len() as u32,
            &mut read,
            operation.as_mut_ptr(),
        )
    };
    if ok == FALSE {
        let error = last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(InjectorError::HostLaunchFailed {
                operation: operation_name,
                source: error,
            });
        }
        operation.mark_pending(handle);
        read = operation.result_waiting(handle, operation_name)?;
    }
    bytes.truncate(read as usize);
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn verify_complete_write(
    expected: usize,
    actual: u32,
    message_kind: &'static str,
) -> Result<(), InjectorError> {
    if actual as usize == expected {
        return Ok(());
    }
    Err(InjectorError::HostStartupFailed(format!(
        "{message_kind} was only partially written"
    )))
}

fn wait_for_event(
    event: HANDLE,
    timeout: Duration,
    operation: &'static str,
) -> Result<(), InjectorError> {
    let timeout_ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    let wait =
        unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(event, timeout_ms) };
    match wait {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(InjectorError::HostStartupFailed(format!(
            "{operation} timed out"
        ))),
        WAIT_FAILED => Err(InjectorError::HostLaunchFailed {
            operation,
            source: last_os_error(),
        }),
        value => Err(InjectorError::HostStartupFailed(format!(
            "unexpected startup wait result {value:#x} during {operation}"
        ))),
    }
}

struct OverlappedOperation {
    overlapped: Box<OVERLAPPED>,
    event: OwnedHandle,
    pending_handle: Option<HANDLE>,
}

impl OverlappedOperation {
    fn new(operation: &'static str) -> Result<Self, InjectorError> {
        let event = unsafe { CreateEventW(null(), TRUE, FALSE, null()) };
        if event.is_null() {
            return Err(InjectorError::HostLaunchFailed {
                operation,
                source: last_os_error(),
            });
        }
        // SAFETY: CreateEventW returned an owned event handle that must be closed.
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        let mut overlapped = Box::new(OVERLAPPED::default());
        overlapped.hEvent = event.as_raw_handle();
        Ok(Self {
            overlapped,
            event,
            pending_handle: None,
        })
    }

    fn as_mut_ptr(&mut self) -> *mut OVERLAPPED {
        self.overlapped.as_mut()
    }

    fn event(&self) -> HANDLE {
        self.event.as_raw_handle()
    }

    fn mark_pending(&mut self, handle: HANDLE) {
        self.pending_handle = Some(handle);
    }

    fn is_pending(&self) -> bool {
        self.pending_handle.is_some()
    }

    fn result(&mut self, handle: HANDLE, operation: &'static str) -> Result<u32, InjectorError> {
        self.result_with_wait(handle, operation, FALSE)
    }

    fn result_waiting(
        &mut self,
        handle: HANDLE,
        operation: &'static str,
    ) -> Result<u32, InjectorError> {
        self.result_with_wait(handle, operation, TRUE)
    }

    fn result_with_wait(
        &mut self,
        handle: HANDLE,
        operation: &'static str,
        wait: i32,
    ) -> Result<u32, InjectorError> {
        let mut transferred = 0u32;
        let ok = unsafe {
            GetOverlappedResult(handle, self.overlapped.as_mut(), &mut transferred, wait)
        };
        self.pending_handle = None;
        if ok == FALSE {
            let source = last_os_error();
            if source.raw_os_error() == Some(ERROR_MORE_DATA as i32) {
                return Err(InjectorError::HostStartupFailed(
                    "startup result exceeded maximum length".to_string(),
                ));
            }
            return Err(InjectorError::HostLaunchFailed { operation, source });
        }
        Ok(transferred)
    }

    fn cancel_and_wait(&mut self, handle: HANDLE) {
        unsafe {
            CancelIoEx(handle, self.overlapped.as_mut());
        }
        let mut transferred = 0u32;
        unsafe {
            GetOverlappedResult(handle, self.overlapped.as_mut(), &mut transferred, TRUE);
        }
        self.pending_handle = None;
    }
}

impl Drop for OverlappedOperation {
    fn drop(&mut self) {
        if let Some(handle) = self.pending_handle {
            self.cancel_and_wait(handle);
        }
    }
}

fn last_os_error() -> io::Error {
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() } as i32;
    io::Error::from_raw_os_error(code)
}

#[cfg(test)]
mod tests {
    use crate::error::InjectorError;

    use super::{StartupEvent, parse_startup_ack, parse_startup_result, startup_failure_kind};

    #[test]
    fn startup_event_latches_reported_state() {
        let event = StartupEvent::new("panic-test", "create test panic event")
            .expect("test event should be created");

        event
            .ensure_not_reported()
            .expect("new event must not be signaled");
        event.signal("signal test panic event").unwrap();

        assert!(matches!(
            event.ensure_not_reported(),
            Err(InjectorError::HostPanicAlreadyReported)
        ));
    }

    #[test]
    fn parses_startup_success_result() {
        parse_startup_result("ok\n").expect("ok result should parse");
    }

    #[test]
    fn parses_startup_acknowledgement() {
        parse_startup_ack("ack\n").expect("acknowledgement should parse");
    }

    #[test]
    fn rejects_invalid_startup_acknowledgement() {
        let error = parse_startup_ack("ok\n").expect_err("invalid acknowledgement must fail");

        assert!(error.to_string().contains("acknowledgement was invalid"));
    }

    #[test]
    fn parses_startup_failure_result() {
        let error =
            parse_startup_result("err\nerror\nmissing hook").expect_err("error result must fail");

        assert!(error.to_string().contains("missing hook"));
    }

    #[test]
    fn parses_host_already_running_startup_failure_as_specific_error() {
        let error = parse_startup_result(
            "err\nhost_already_running\ndwm-lut host instance is already running in this session",
        )
        .expect_err("already running result must be surfaced as a specific error");

        assert!(matches!(error, InjectorError::HostAlreadyRunning));
    }

    #[test]
    fn rejects_unstructured_startup_failure_result() {
        let error =
            parse_startup_result("err\nmissing hook").expect_err("legacy error result must fail");
        assert!(error.to_string().contains("invalid startup result"));
    }

    #[test]
    fn startup_failure_kind_preserves_host_already_running() {
        assert_eq!(
            startup_failure_kind(&InjectorError::HostAlreadyRunning),
            "host_already_running"
        );
    }
}
