use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::mpsc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, FALSE, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    SetHandleInformation, TRUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DETACHED_PROCESS, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetExitCodeProcess, InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROCESS_INFORMATION, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

use crate::error::InjectorError;

use super::{last_os_error, wide_null};

const STARTUP_RESULT_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_TERMINATE_WAIT_MS: u32 = 5_000;
const STARTUP_RESULT_OK: &str = "ok\n";
const STARTUP_RESULT_ERROR_PREFIX: &str = "err\n";
const STARTUP_RESULT_MAX_BYTES: usize = 16 * 1024;
const STARTUP_TIMEOUT_EXIT_CODE: u32 = 0xE000_0001;
const STARTUP_FAILURE_KIND_ERROR: &str = "error";
const STARTUP_FAILURE_KIND_HOST_ALREADY_RUNNING: &str = "host_already_running";
const ARG_SEPARATOR: u16 = b' ' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const DOUBLE_QUOTE: u16 = b'"' as u16;
const TAB: u16 = b'\t' as u16;
const NEWLINE: u16 = b'\n' as u16;

pub(crate) fn start_background_host(
    executable: &std::path::Path,
    dll_path: Option<PathBuf>,
) -> Result<(), InjectorError> {
    let startup_pipe = StartupResultPipe::new()?;
    let mut command_line = host_command_line(
        executable.as_os_str(),
        dll_path.as_deref(),
        Some(startup_pipe.write_handle_value()),
    );
    let executable = wide_null(executable.as_os_str());
    command_line.push(0);

    let mut inherited_handles = [startup_pipe.write_handle()];
    let mut startup_attributes =
        StartupAttributeList::for_inherited_handles(&mut inherited_handles)?;
    let mut startup = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: size_of::<STARTUPINFOEXW>() as u32,
            lpReserved: null_mut(),
            lpDesktop: null_mut(),
            lpTitle: null_mut(),
            dwX: 0,
            dwY: 0,
            dwXSize: 0,
            dwYSize: 0,
            dwXCountChars: 0,
            dwYCountChars: 0,
            dwFillAttribute: 0,
            dwFlags: 0,
            wShowWindow: 0,
            cbReserved2: 0,
            lpReserved2: null_mut(),
            hStdInput: null_mut(),
            hStdOutput: null_mut(),
            hStdError: null_mut(),
        },
        lpAttributeList: startup_attributes.as_mut_ptr(),
    };
    let mut process = PROCESS_INFORMATION {
        hProcess: null_mut(),
        hThread: null_mut(),
        dwProcessId: 0,
        dwThreadId: 0,
    };

    let ok = unsafe {
        CreateProcessW(
            executable.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            TRUE,
            DETACHED_PROCESS | EXTENDED_STARTUPINFO_PRESENT,
            null(),
            null(),
            (&mut startup as *mut STARTUPINFOEXW).cast(),
            &mut process,
        )
    };
    if ok == FALSE {
        return Err(InjectorError::HostLaunchFailed {
            operation: "start detached process",
            source: last_os_error(),
        });
    }

    let process_handle = ProcessHandle(process.hProcess);
    let _thread_handle = ProcessHandle(process.hThread);
    startup_pipe.wait(&process_handle, process.dwProcessId)
}

pub(crate) struct StartupNotifier {
    handle: ProcessHandle,
}

impl StartupNotifier {
    pub(crate) fn from_raw_handle(handle: usize) -> Self {
        Self {
            handle: ProcessHandle(handle as HANDLE),
        }
    }

    pub(crate) fn notify_success(self) -> Result<(), InjectorError> {
        self.write_result(STARTUP_RESULT_OK.as_bytes())
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
        self.write_result(&bytes)
    }

    fn write_result(self, bytes: &[u8]) -> Result<(), InjectorError> {
        write_all(self.handle.0, bytes).map_err(|source| InjectorError::HostLaunchFailed {
            operation: "write startup result",
            source,
        })
    }
}

struct StartupResultPipe {
    read: ProcessHandle,
    write: ProcessHandle,
}

impl StartupResultPipe {
    fn new() -> Result<Self, InjectorError> {
        let mut read = null_mut();
        let mut write = null_mut();
        let security_attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: TRUE,
        };

        let ok = unsafe { CreatePipe(&mut read, &mut write, &security_attributes, 0) };
        if ok == FALSE {
            return Err(InjectorError::HostLaunchFailed {
                operation: "create startup result pipe",
                source: last_os_error(),
            });
        }

        let read = ProcessHandle(read);
        let write = ProcessHandle(write);

        let ok = unsafe { SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0) };
        if ok == FALSE {
            return Err(InjectorError::HostLaunchFailed {
                operation: "make startup result pipe read handle private",
                source: last_os_error(),
            });
        }

        Ok(Self { read, write })
    }

    fn write_handle_value(&self) -> usize {
        self.write.0 as usize
    }

    fn write_handle(&self) -> HANDLE {
        self.write.0
    }

    fn wait(mut self, process: &ProcessHandle, process_id: u32) -> Result<(), InjectorError> {
        let read = std::mem::replace(&mut self.read, ProcessHandle(null_mut()));
        drop(self.write);

        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(read_startup_result(read));
        });

        let message = receiver
            .recv_timeout(STARTUP_RESULT_TIMEOUT)
            .map_err(|_| handle_startup_timeout(process, process_id))?
            .map_err(|source| InjectorError::HostLaunchFailed {
                operation: "read startup result",
                source,
            })?;
        parse_startup_result(&message)
    }
}

fn handle_startup_timeout(process: &ProcessHandle, process_id: u32) -> InjectorError {
    let wait = unsafe { WaitForSingleObject(process.0, 0) };
    if wait == WAIT_OBJECT_0 {
        let mut exit_code = 0;
        let ok = unsafe { GetExitCodeProcess(process.0, &mut exit_code) };
        if ok != FALSE {
            return InjectorError::HostStartupFailed(format!(
                "host process exited before reporting startup with exit code {exit_code}"
            ));
        }
        return InjectorError::HostLaunchFailed {
            operation: "read host process exit code after startup timeout",
            source: last_os_error(),
        };
    }

    let ok = unsafe { TerminateProcess(process.0, STARTUP_TIMEOUT_EXIT_CODE) };
    if ok == FALSE {
        return InjectorError::HostLaunchFailed {
            operation: "terminate host process after startup timeout",
            source: last_os_error(),
        };
    }

    let wait = unsafe { WaitForSingleObject(process.0, STARTUP_TERMINATE_WAIT_MS) };
    if wait == WAIT_OBJECT_0 {
        return InjectorError::HostStartupFailed(format!(
            "timed out waiting for host startup result; terminated host process {process_id}"
        ));
    }

    InjectorError::HostStartupFailed(format!(
        "timed out waiting for host startup result; requested termination for host process {process_id}, but it did not exit within {STARTUP_TERMINATE_WAIT_MS}ms"
    ))
}

struct StartupAttributeList {
    words: Vec<usize>,
}

impl StartupAttributeList {
    fn for_inherited_handles(handles: &mut [HANDLE]) -> Result<Self, InjectorError> {
        let mut size = 0;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        }
        let word_count = size.div_ceil(size_of::<usize>());
        let mut words = vec![0; word_count];

        let raw_attribute_list = words.as_mut_ptr().cast();
        let ok = unsafe { InitializeProcThreadAttributeList(raw_attribute_list, 1, 0, &mut size) };
        if ok == FALSE {
            return Err(InjectorError::HostLaunchFailed {
                operation: "initialize startup attribute list",
                source: last_os_error(),
            });
        }

        let mut attribute_list = Self { words };
        let ok = unsafe {
            UpdateProcThreadAttribute(
                attribute_list.as_mut_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_mut_ptr().cast(),
                std::mem::size_of_val(handles),
                null_mut(),
                null_mut(),
            )
        };
        if ok == FALSE {
            return Err(InjectorError::HostLaunchFailed {
                operation: "set inherited startup handles",
                source: last_os_error(),
            });
        }

        Ok(attribute_list)
    }

    fn as_mut_ptr(&mut self) -> *mut std::ffi::c_void {
        self.words.as_mut_ptr().cast()
    }
}

impl Drop for StartupAttributeList {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.as_mut_ptr());
        }
    }
}

fn host_command_line(
    executable: &OsStr,
    dll_path: Option<&std::path::Path>,
    startup_result_handle: Option<usize>,
) -> Vec<u16> {
    let mut args = vec![OsString::from(executable)];
    if let Some(handle) = startup_result_handle {
        args.push(OsString::from("--startup-result-handle"));
        args.push(OsString::from(handle.to_string()));
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

fn startup_failure_kind(error: &InjectorError) -> &'static str {
    match error {
        InjectorError::HostAlreadyRunning => STARTUP_FAILURE_KIND_HOST_ALREADY_RUNNING,
        _ => STARTUP_FAILURE_KIND_ERROR,
    }
}

fn read_startup_result(handle: ProcessHandle) -> io::Result<String> {
    let mut bytes = Vec::new();
    loop {
        let mut buffer = [0u8; 4096];
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                handle.0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut read,
                null_mut(),
            )
        };
        if ok == FALSE {
            let error = last_os_error();
            if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                break;
            }
            return Err(error);
        }
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read as usize]);
        if bytes.len() > STARTUP_RESULT_MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "startup result exceeded maximum length",
            ));
        }
    }

    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn write_all(handle: HANDLE, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let len = u32::try_from(bytes.len().min(u32::MAX as usize)).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "startup result chunk length does not fit u32",
            )
        })?;
        let mut written = 0u32;
        let ok = unsafe { WriteFile(handle, bytes.as_ptr().cast(), len, &mut written, null_mut()) };
        if ok == FALSE {
            return Err(last_os_error());
        }
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "startup result write made no progress",
            ));
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

struct ProcessHandle(HANDLE);

unsafe impl Send for ProcessHandle {}

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

    use super::{host_command_line, parse_startup_result, quote_windows_arg, startup_failure_kind};

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
    fn builds_host_command_line_with_dll_path() {
        let command_line = host_command_line(
            OsStr::new(r"C:\Program Files\dwm-lut\dwm-lut.exe"),
            Some(Path::new(r"C:\hook dll\dwm_lut_hook.dll")),
            Some(1234),
        );

        assert_eq!(
            command_line,
            wide(
                r#""C:\Program Files\dwm-lut\dwm-lut.exe" --startup-result-handle 1234 --dll "C:\hook dll\dwm_lut_hook.dll""#
            )
        );
    }

    #[test]
    fn parses_startup_success_result() {
        parse_startup_result("ok\n").expect("ok result should parse");
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
