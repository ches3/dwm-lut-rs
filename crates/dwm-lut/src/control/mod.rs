use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcessToken,
};

use crate::error::InjectorError;

pub(crate) mod build_hash;
pub(crate) mod client;
pub(crate) mod protocol;
pub(crate) mod server;

const SDDL_REVISION_1: u32 = 1;

fn current_pipe_name() -> Result<String, InjectorError> {
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

fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn last_os_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UserSid {
    sddl: String,
}

impl UserSid {
    fn current_process() -> Result<Self, InjectorError> {
        let mut token = null_mut();
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if ok == 0 {
            return Err(InjectorError::ControlPipe {
                operation: "open primary process token",
                source: last_os_error(),
            });
        }
        let token = TokenHandle(token);

        let mut required_len = 0u32;
        unsafe {
            GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut required_len);
        }
        if required_len == 0 {
            return Err(InjectorError::ControlPipe {
                operation: "measure primary token user",
                source: last_os_error(),
            });
        }

        let mut buffer = vec![0u8; required_len as usize];
        let ok = unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                required_len,
                &mut required_len,
            )
        };
        if ok == 0 {
            return Err(InjectorError::ControlPipe {
                operation: "read primary token user",
                source: last_os_error(),
            });
        }

        let token_user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
        let mut sid_text = null_mut();
        let ok = unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut sid_text) };
        if ok == 0 {
            return Err(InjectorError::ControlPipe {
                operation: "convert primary user sid",
                source: last_os_error(),
            });
        }
        let sddl = unsafe { wide_ptr_to_string(sid_text) };
        unsafe {
            LocalFree(sid_text.cast());
        }

        Ok(Self { sddl })
    }
}

struct TokenHandle(HANDLE);

impl Drop for TokenHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

unsafe fn wide_ptr_to_string(ptr: *const u16) -> String {
    let mut len = 0usize;
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) })
}

fn pipe_security_descriptor_for_user(user_sid: &str) -> String {
    format!("D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGW;;;{user_sid})S:(ML;;NW;;;ME)")
}

struct SecurityDescriptor {
    ptr: PSECURITY_DESCRIPTOR,
}

impl SecurityDescriptor {
    fn from_pipe_dacl(user_sid: &UserSid) -> Result<Self, InjectorError> {
        let sddl = wide_null(&pipe_security_descriptor_for_user(&user_sid.sddl));
        let mut ptr = null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut ptr,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(InjectorError::ControlPipe {
                operation: "build pipe security descriptor",
                source: last_os_error(),
            });
        }

        Ok(Self { ptr })
    }

    fn as_security_attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.ptr,
            bInheritHandle: 0,
        }
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                LocalFree(self.ptr);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pipe_security_descriptor_for_user;

    #[test]
    fn pipe_security_descriptor_grants_user_access_at_medium_integrity() {
        let descriptor = pipe_security_descriptor_for_user("S-1-5-21-1-2-3-1001");

        assert_eq!(
            descriptor,
            "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGW;;;S-1-5-21-1-2-3-1001)S:(ML;;NW;;;ME)"
        );
        assert!(!descriptor.contains(";;;IU"));
    }
}
