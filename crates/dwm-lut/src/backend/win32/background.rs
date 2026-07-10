use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_CANCELLED, ERROR_IO_PENDING, ERROR_MORE_DATA, ERROR_PIPE_CONNECTED, FALSE,
    GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, TRUE, WAIT_FAILED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetExitCodeProcess, GetProcessId, SetEvent, WaitForMultipleObjects,
};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};

use crate::control::{SecurityDescriptor, UserSid};
use crate::error::InjectorError;

use super::{last_os_error, wide_null};

const STARTUP_RESULT_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_RESULT_OK: &str = "ok\n";
const STARTUP_RESULT_ACK: &str = "ack\n";
const STARTUP_RESULT_ERROR_PREFIX: &str = "err\n";
const STARTUP_RESULT_MAX_BYTES: usize = 16 * 1024;
const STARTUP_FAILURE_KIND_ERROR: &str = "error";
const STARTUP_FAILURE_KIND_HOST_ALREADY_RUNNING: &str = "host_already_running";
const ARG_SEPARATOR: u16 = b' ' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const DOUBLE_QUOTE: u16 = b'"' as u16;
const TAB: u16 = b'\t' as u16;
const NEWLINE: u16 = b'\n' as u16;
static STARTUP_PIPE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn start_background_host(
    executable: &std::path::Path,
    dll_path: Option<PathBuf>,
) -> Result<(), InjectorError> {
    let startup_pipe = StartupResultPipe::new()?;
    let mut command_line = host_parameters(dll_path.as_deref(), Some(startup_pipe.name()));
    let executable = wide_null(executable.as_os_str());
    command_line.push(0);
    let runas = wide_null(OsStr::new("runas"));
    let mut execute = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        hwnd: null_mut(),
        lpVerb: runas.as_ptr(),
        lpFile: executable.as_ptr(),
        lpParameters: command_line.as_ptr(),
        lpDirectory: null(),
        nShow: 0,
        hInstApp: null_mut(),
        lpIDList: null_mut(),
        lpClass: null(),
        hkeyClass: null_mut(),
        dwHotKey: 0,
        Anonymous: Default::default(),
        hProcess: null_mut(),
    };

    let ok = unsafe { ShellExecuteExW(&mut execute) };
    if ok == FALSE {
        let source = last_os_error();
        if source.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            return Err(InjectorError::HostElevationCancelled);
        }
        return Err(InjectorError::HostLaunchFailed {
            operation: "request elevation",
            source,
        });
    }
    if execute.hProcess.is_null() {
        return Err(InjectorError::HostStartupFailed(
            "elevated host launch returned no process handle".to_string(),
        ));
    }

    let process = ProcessHandle(execute.hProcess);
    startup_pipe.wait(&process)
}

pub(crate) struct StartupNotifier {
    pipe_name: Vec<u16>,
}

impl StartupNotifier {
    pub(crate) fn new(pipe_name: String) -> Self {
        Self {
            pipe_name: OsStr::new(&pipe_name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect(),
        }
    }

    pub(crate) fn notify_success(self) -> Result<(), InjectorError> {
        let handle = self.connect()?;
        write_message_with_timeout(
            handle.0,
            STARTUP_RESULT_OK.as_bytes(),
            STARTUP_RESULT_TIMEOUT,
            "write startup result",
        )?;
        let acknowledgement = read_message_with_timeout(
            handle.0,
            STARTUP_RESULT_ACK.len(),
            STARTUP_RESULT_TIMEOUT,
            "read startup result acknowledgement",
        )?;
        parse_startup_ack(&acknowledgement)
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
        let handle = self.connect()?;
        write_message_with_timeout(
            handle.0,
            &bytes,
            STARTUP_RESULT_TIMEOUT,
            "write startup result",
        )
    }

    fn connect(&self) -> Result<ProcessHandle, InjectorError> {
        let handle = unsafe {
            CreateFileW(
                self.pipe_name.as_ptr(),
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
        Ok(ProcessHandle(handle))
    }
}

struct StartupResultPipe {
    name: String,
    connect: OverlappedOperation,
    pipe: ProcessHandle,
}

impl StartupResultPipe {
    fn new() -> Result<Self, InjectorError> {
        let name = startup_pipe_name();
        let name_wide = OsStr::new(&name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let user_sid = UserSid::current_process()?;
        let security_descriptor = SecurityDescriptor::from_pipe_dacl(&user_sid)?;
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
        let pipe = ProcessHandle(pipe);
        let mut connect = OverlappedOperation::new("create startup pipe connection event")?;
        let ok = unsafe { ConnectNamedPipe(pipe.0, connect.as_mut_ptr()) };
        if ok != FALSE {
            unsafe {
                SetEvent(connect.event());
            }
        } else {
            let error = last_os_error();
            match error.raw_os_error() {
                Some(code) if code == ERROR_IO_PENDING as i32 => connect.mark_pending(pipe.0),
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

    fn name(&self) -> &str {
        &self.name
    }

    fn wait(mut self, process: &ProcessHandle) -> Result<(), InjectorError> {
        let deadline = Instant::now() + STARTUP_RESULT_TIMEOUT;
        wait_for_event_or_process(self.connect.event(), process, deadline)?;
        if self.connect.is_pending() {
            self.connect
                .result(self.pipe.0, "complete startup result pipe connection")?;
        }

        let expected_pid = unsafe { GetProcessId(process.0) };
        let mut actual_pid = 0;
        let ok = unsafe { GetNamedPipeClientProcessId(self.pipe.0, &mut actual_pid) };
        if ok == FALSE {
            return Err(InjectorError::HostLaunchFailed {
                operation: "identify startup result pipe client",
                source: last_os_error(),
            });
        }
        if expected_pid == 0 || actual_pid != expected_pid {
            return Err(InjectorError::HostStartupFailed(
                "startup result pipe client did not match the launched host process".to_string(),
            ));
        }

        let message = read_startup_result(&self.pipe, process, deadline)?;
        parse_startup_result(&message)?;
        write_startup_ack(&self.pipe, process, deadline)
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

fn host_parameters(
    dll_path: Option<&std::path::Path>,
    startup_result_pipe: Option<&str>,
) -> Vec<u16> {
    let mut args = Vec::new();
    if let Some(pipe_name) = startup_result_pipe {
        args.push(OsString::from("--startup-result-pipe"));
        args.push(OsString::from(pipe_name));
    }
    if let Some(dll_path) = dll_path {
        args.push(OsString::from("--dll"));
        args.push(dll_path.as_os_str().to_owned());
    }

    command_line_from_args(args)
}

fn command_line_from_args(args: impl IntoIterator<Item = OsString>) -> Vec<u16> {
    let mut command_line = Vec::new();
    for arg in args {
        if !command_line.is_empty() {
            command_line.push(ARG_SEPARATOR);
        }
        command_line.extend(quote_windows_arg(arg.as_os_str()));
    }
    command_line
}

fn quote_windows_arg(arg: &OsStr) -> Vec<u16> {
    let text = arg.encode_wide().collect::<Vec<_>>();
    if !text.is_empty()
        && !text
            .iter()
            .any(|&ch| matches!(ch, ARG_SEPARATOR | TAB | NEWLINE | DOUBLE_QUOTE))
    {
        return text;
    }

    let mut quoted = vec![DOUBLE_QUOTE];
    let mut backslashes = 0usize;
    for ch in text {
        match ch {
            BACKSLASH => backslashes += 1,
            DOUBLE_QUOTE => {
                quoted.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2 + 1));
                quoted.push(DOUBLE_QUOTE);
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n(BACKSLASH, backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }
    quoted.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2));
    quoted.push(DOUBLE_QUOTE);
    quoted
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

fn wait_for_event_or_process(
    event: HANDLE,
    process: &ProcessHandle,
    deadline: Instant,
) -> Result<(), InjectorError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(InjectorError::HostStartupFailed(
            "timed out waiting for host startup result".to_string(),
        ));
    }
    let timeout = u32::try_from(remaining.as_millis()).unwrap_or(u32::MAX);
    let handles = [event, process.0];
    let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), FALSE, timeout) };
    match wait {
        WAIT_OBJECT_0 => Ok(()),
        value if value == WAIT_OBJECT_0 + 1 => Err(host_exited_before_startup(process)),
        WAIT_TIMEOUT => Err(InjectorError::HostStartupFailed(
            "timed out waiting for host startup result".to_string(),
        )),
        WAIT_FAILED => Err(InjectorError::HostLaunchFailed {
            operation: "wait for host startup result",
            source: last_os_error(),
        }),
        value => Err(InjectorError::HostStartupFailed(format!(
            "unexpected host startup wait result {value:#x}"
        ))),
    }
}

fn host_exited_before_startup(process: &ProcessHandle) -> InjectorError {
    let mut exit_code = 0;
    let ok = unsafe { GetExitCodeProcess(process.0, &mut exit_code) };
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
    pipe: &ProcessHandle,
    process: &ProcessHandle,
    deadline: Instant,
) -> Result<String, InjectorError> {
    let mut bytes = vec![0u8; STARTUP_RESULT_MAX_BYTES];
    let mut operation = OverlappedOperation::new("create startup result read event")?;
    let mut read = 0u32;
    let ok = unsafe {
        ReadFile(
            pipe.0,
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
                operation.mark_pending(pipe.0);
                wait_for_event_or_process(operation.event(), process, deadline)?;
                read = operation.result(pipe.0, "read startup result")?;
            }
            Some(code) if code == ERROR_MORE_DATA as i32 => {
                return Err(InjectorError::HostStartupFailed(
                    "startup result exceeded maximum length".to_string(),
                ));
            }
            _ => {
                return Err(InjectorError::HostLaunchFailed {
                    operation: "read startup result",
                    source: error,
                });
            }
        }
    }

    bytes.truncate(read as usize);
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn write_startup_ack(
    pipe: &ProcessHandle,
    process: &ProcessHandle,
    deadline: Instant,
) -> Result<(), InjectorError> {
    let mut operation = OverlappedOperation::new("create startup acknowledgement write event")?;
    let mut written = 0u32;
    let bytes = STARTUP_RESULT_ACK.as_bytes();
    let ok = unsafe {
        WriteFile(
            pipe.0,
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
            });
        }
        operation.mark_pending(pipe.0);
        wait_for_event_or_process(operation.event(), process, deadline)?;
        written = operation.result(pipe.0, "write startup result acknowledgement")?;
    }
    verify_complete_write(bytes.len(), written, "startup result acknowledgement")
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

fn read_message_with_timeout(
    handle: HANDLE,
    max_bytes: usize,
    timeout: Duration,
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
        wait_for_event(operation.event(), timeout, operation_name)?;
        read = operation.result(handle, operation_name)?;
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
    event: ProcessHandle,
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
        let event = ProcessHandle(event);
        let mut overlapped = Box::new(OVERLAPPED::default());
        overlapped.hEvent = event.0;
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
        self.event.0
    }

    fn mark_pending(&mut self, handle: HANDLE) {
        self.pending_handle = Some(handle);
    }

    fn is_pending(&self) -> bool {
        self.pending_handle.is_some()
    }

    fn result(&mut self, handle: HANDLE, operation: &'static str) -> Result<u32, InjectorError> {
        let mut transferred = 0u32;
        let ok = unsafe {
            GetOverlappedResult(handle, self.overlapped.as_mut(), &mut transferred, FALSE)
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

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;

    use crate::error::InjectorError;

    use super::{
        host_parameters, parse_startup_ack, parse_startup_result, quote_windows_arg,
        startup_failure_kind,
    };

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().collect()
    }

    #[test]
    fn leaves_plain_arguments_unquoted() {
        assert_eq!(quote_windows_arg(OsStr::new("--dll")), wide("--dll"));
    }

    #[test]
    fn quotes_arguments_with_spaces() {
        assert_eq!(
            quote_windows_arg(OsStr::new(r"C:\hook dll\dwm_lut_hook.dll")),
            wide(r#""C:\hook dll\dwm_lut_hook.dll""#)
        );
    }

    #[test]
    fn escapes_quotes_and_trailing_backslashes() {
        assert_eq!(
            quote_windows_arg(OsStr::new(r#"C:\quoted "path"\"#)),
            wide(r#""C:\quoted \"path\"\\""#)
        );
    }

    #[test]
    fn preserves_ill_formed_utf16_arguments() {
        let arg =
            OsString::from_wide(&[b'a' as u16, 0xD800, b' ' as u16, b'"' as u16, b'\\' as u16]);

        assert_eq!(
            quote_windows_arg(arg.as_os_str()),
            vec![
                b'"' as u16,
                b'a' as u16,
                0xD800,
                b' ' as u16,
                b'\\' as u16,
                b'"' as u16,
                b'\\' as u16,
                b'\\' as u16,
                b'"' as u16,
            ]
        );
        assert!(!quote_windows_arg(arg.as_os_str()).contains(&0xFFFD));
    }

    #[test]
    fn builds_host_parameters_with_dll_path() {
        let command_line = host_parameters(
            Some(Path::new(r"C:\hook dll\dwm_lut_hook.dll")),
            Some(r"\\.\pipe\dwm-lut-rs-startup-1234"),
        );

        assert_eq!(
            command_line,
            wide(
                r#"--startup-result-pipe \\.\pipe\dwm-lut-rs-startup-1234 --dll "C:\hook dll\dwm_lut_hook.dll""#
            )
        );
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
