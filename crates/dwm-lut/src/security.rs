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
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::error::InjectorError;

const SDDL_REVISION_1: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UserSid {
    sddl: String,
}

impl UserSid {
    pub(crate) fn current_process() -> Result<Self, InjectorError> {
        let mut token = null_mut();
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if ok == 0 {
            return Err(security_error(
                "open current process token",
                last_os_error(),
            ));
        }
        let token = TokenHandle(token);

        let mut required_len = 0u32;
        unsafe {
            GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut required_len);
        }
        if required_len == 0 {
            return Err(security_error(
                "measure current process user token information",
                last_os_error(),
            ));
        }

        let buffer_len = required_len;
        let mut buffer = vec![0u8; buffer_len as usize];
        let mut returned_len = 0u32;
        let ok = unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                buffer_len,
                &mut returned_len,
            )
        };
        if ok == 0 {
            return Err(security_error(
                "read current process user token information",
                last_os_error(),
            ));
        }
        if returned_len < std::mem::size_of::<TOKEN_USER>() as u32 {
            return Err(security_error(
                "read current process user token information",
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "token user information was shorter than TOKEN_USER",
                ),
            ));
        }

        // SAFETY: GetTokenInformation initialized at least TOKEN_USER bytes. The backing
        // Vec<u8> need not satisfy TOKEN_USER's alignment because this performs an unaligned read.
        let token_user = unsafe { buffer.as_ptr().cast::<TOKEN_USER>().read_unaligned() };
        let mut sid_text = null_mut();
        let ok = unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut sid_text) };
        if ok == 0 {
            return Err(security_error(
                "convert current process user SID to string",
                last_os_error(),
            ));
        }
        let sddl = unsafe { wide_ptr_to_string(sid_text) };
        unsafe {
            LocalFree(sid_text.cast());
        }

        Ok(Self { sddl })
    }

    pub(crate) fn as_sddl(&self) -> &str {
        &self.sddl
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

fn read_write_security_descriptor_for_user(user_sid: &str) -> String {
    format!("D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGW;;;{user_sid})S:(ML;;NW;;;ME)")
}

fn full_access_security_descriptor_for_user(user_sid: &str) -> String {
    format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{user_sid})S:(ML;;NW;;;ME)")
}

pub(crate) struct SecurityDescriptor {
    ptr: PSECURITY_DESCRIPTOR,
}

impl SecurityDescriptor {
    pub(crate) fn read_write_for_user(user_sid: &UserSid) -> Result<Self, InjectorError> {
        Self::from_sddl(
            read_write_security_descriptor_for_user(&user_sid.sddl),
            "build read-write security descriptor",
        )
    }

    pub(crate) fn full_access_for_user(user_sid: &UserSid) -> Result<Self, InjectorError> {
        Self::from_sddl(
            full_access_security_descriptor_for_user(&user_sid.sddl),
            "build full-access security descriptor",
        )
    }

    fn from_sddl(sddl: String, operation: &'static str) -> Result<Self, InjectorError> {
        let sddl = wide_null(&sddl);
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
            return Err(security_error(operation, last_os_error()));
        }

        Ok(Self { ptr })
    }

    pub(crate) fn as_security_attributes(&self) -> SECURITY_ATTRIBUTES {
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

fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn last_os_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

fn security_error(operation: &'static str, source: io::Error) -> InjectorError {
    InjectorError::SecurityOperationFailed { operation, source }
}

#[cfg(test)]
mod tests {
    use super::{
        full_access_security_descriptor_for_user, read_write_security_descriptor_for_user,
    };

    #[test]
    fn read_write_descriptor_grants_user_access_at_medium_integrity() {
        let descriptor = read_write_security_descriptor_for_user("S-1-5-21-1-2-3-1001");

        assert_eq!(
            descriptor,
            "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGW;;;S-1-5-21-1-2-3-1001)S:(ML;;NW;;;ME)"
        );
        assert!(!descriptor.contains(";;;IU"));
    }

    #[test]
    fn full_access_descriptor_grants_user_access_at_medium_integrity() {
        let descriptor = full_access_security_descriptor_for_user("S-1-5-21-1-2-3-1001");

        assert_eq!(
            descriptor,
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;S-1-5-21-1-2-3-1001)S:(ML;;NW;;;ME)"
        );
        assert!(!descriptor.contains(";;;IU"));
    }
}
