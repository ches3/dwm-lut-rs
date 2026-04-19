use std::ffi::{CString, OsStr};
use std::io;
use std::mem::size_of;
use std::path::Path;

use windows_sys::Win32::Foundation::FALSE;
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

use crate::error::{InjectionStep, InjectorError};

use super::last_os_error;
use super::remote::{OwnedHandle, wide_null};

trait MemoryReader {
    fn read(
        &self,
        address: usize,
        size: usize,
        step: InjectionStep,
    ) -> Result<Vec<u8>, InjectorError>;
}

struct ProcessMemoryReader<'a> {
    process: &'a OwnedHandle,
}

impl MemoryReader for ProcessMemoryReader<'_> {
    fn read(
        &self,
        address: usize,
        size: usize,
        step: InjectionStep,
    ) -> Result<Vec<u8>, InjectorError> {
        let mut buffer = vec![0u8; size];
        let mut read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                self.process.raw(),
                address as *const _,
                buffer.as_mut_ptr().cast(),
                size,
                &mut read,
            )
        };
        if ok == FALSE {
            return Err(InjectorError::StepFailed {
                step,
                source: last_os_error(),
            });
        }
        if read != size {
            return Err(InjectorError::StepFailed {
                step,
                source: io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "remote module read returned fewer bytes than requested",
                ),
            });
        }

        Ok(buffer)
    }
}

pub(crate) fn resolve_remote_export_address(
    remote_module_base: usize,
    module_name: &str,
    export_name: &str,
    module_step: InjectionStep,
    export_step: InjectionStep,
) -> Result<usize, InjectorError> {
    let module_name_wide = wide_null(OsStr::new(module_name));
    let local_module = unsafe { GetModuleHandleW(module_name_wide.as_ptr()) };
    if local_module.is_null() {
        return Err(InjectorError::StepFailed {
            step: module_step,
            source: last_os_error(),
        });
    }

    let export_name = CString::new(export_name).expect("export names do not contain nul");
    let local_proc = unsafe { GetProcAddress(local_module, export_name.as_ptr().cast()) }
        .ok_or_else(|| InjectorError::StepFailed {
            step: export_step,
            source: last_os_error(),
        })?;

    Ok(remote_module_base + proc_rva(local_module as usize, local_proc as usize))
}

pub(crate) fn resolve_remote_module_export_address(
    process: &OwnedHandle,
    remote_module_base: usize,
    export_name: &str,
    step: InjectionStep,
    dll_path: &Path,
) -> Result<usize, InjectorError> {
    let reader = ProcessMemoryReader { process };
    resolve_module_export_address(&reader, remote_module_base, export_name, step, dll_path)
}

fn resolve_module_export_address<R: MemoryReader>(
    reader: &R,
    remote_module_base: usize,
    export_name: &str,
    step: InjectionStep,
    dll_path: &Path,
) -> Result<usize, InjectorError> {
    let dos_header = reader.read(remote_module_base, 64, step)?;
    if read_u16(&dos_header, 0, step)? != 0x5a4d {
        return invalid_remote_image(step, "missing MZ header");
    }

    let pe_header_offset = read_i32(&dos_header, 0x3c, step)?;
    if pe_header_offset < 0 {
        return invalid_remote_image(step, "negative PE header offset");
    }

    let nt_headers = reader.read(remote_module_base + pe_header_offset as usize, 0x98, step)?;
    if read_u32(&nt_headers, 0, step)? != 0x0000_4550 {
        return invalid_remote_image(step, "missing PE header");
    }

    let optional_magic = read_u16(&nt_headers, 24, step)?;
    let export_directory_offset = match optional_magic {
        0x10b => 24 + 0x60,
        0x20b => 24 + 0x70,
        _ => return invalid_remote_image(step, "unsupported optional header"),
    };

    let export_rva = read_u32(&nt_headers, export_directory_offset, step)? as usize;
    let export_size = read_u32(&nt_headers, export_directory_offset + 4, step)? as usize;
    if export_rva == 0 || export_size < 40 {
        return Err(InjectorError::ExportNotFound {
            export: export_name.to_string(),
            dll_path: dll_path.to_path_buf(),
        });
    }

    let export_directory = reader.read(remote_module_base + export_rva, 40, step)?;
    let number_of_functions = read_u32(&export_directory, 20, step)? as usize;
    let number_of_names = read_u32(&export_directory, 24, step)? as usize;
    let functions_rva = read_u32(&export_directory, 28, step)? as usize;
    let names_rva = read_u32(&export_directory, 32, step)? as usize;
    let ordinals_rva = read_u32(&export_directory, 36, step)? as usize;

    if number_of_functions == 0 || number_of_names == 0 {
        return Err(InjectorError::ExportNotFound {
            export: export_name.to_string(),
            dll_path: dll_path.to_path_buf(),
        });
    }

    let functions = reader.read(
        remote_module_base + functions_rva,
        number_of_functions * size_of::<u32>(),
        step,
    )?;
    let names = reader.read(
        remote_module_base + names_rva,
        number_of_names * size_of::<u32>(),
        step,
    )?;
    let ordinals = reader.read(
        remote_module_base + ordinals_rva,
        number_of_names * size_of::<u16>(),
        step,
    )?;

    for index in 0..number_of_names {
        let name_rva = read_u32(&names, index * size_of::<u32>(), step)? as usize;
        let name = read_remote_c_string(reader, remote_module_base + name_rva, step)?;
        if name != export_name {
            continue;
        }

        let ordinal = read_u16(&ordinals, index * size_of::<u16>(), step)? as usize;
        if ordinal >= number_of_functions {
            return invalid_remote_image(step, "export ordinal is out of range");
        }

        let function_rva = read_u32(&functions, ordinal * size_of::<u32>(), step)? as usize;
        if (export_rva..export_rva + export_size).contains(&function_rva) {
            return invalid_remote_image(step, "forwarded exports are not supported");
        }

        return Ok(remote_module_base + function_rva);
    }

    Err(InjectorError::ExportNotFound {
        export: export_name.to_string(),
        dll_path: dll_path.to_path_buf(),
    })
}

fn proc_rva(module_base: usize, proc_address: usize) -> usize {
    proc_address - module_base
}

fn read_remote_c_string<R: MemoryReader>(
    reader: &R,
    address: usize,
    step: InjectionStep,
) -> Result<String, InjectorError> {
    let mut bytes = Vec::new();
    let mut offset = 0usize;
    loop {
        let byte = reader.read(address + offset, 1, step)?[0];
        if byte == 0 {
            return String::from_utf8(bytes).map_err(|_| InjectorError::StepFailed {
                step,
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "export name was not valid UTF-8",
                ),
            });
        }
        bytes.push(byte);
        offset += 1;
    }
}

fn read_u16(buffer: &[u8], offset: usize, step: InjectionStep) -> Result<u16, InjectorError> {
    let bytes = buffer
        .get(offset..offset + size_of::<u16>())
        .ok_or_else(|| InjectorError::StepFailed {
            step,
            source: io::Error::new(io::ErrorKind::InvalidData, "truncated PE field"),
        })?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(buffer: &[u8], offset: usize, step: InjectionStep) -> Result<u32, InjectorError> {
    let bytes = buffer
        .get(offset..offset + size_of::<u32>())
        .ok_or_else(|| InjectorError::StepFailed {
            step,
            source: io::Error::new(io::ErrorKind::InvalidData, "truncated PE field"),
        })?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_i32(buffer: &[u8], offset: usize, step: InjectionStep) -> Result<i32, InjectorError> {
    Ok(read_u32(buffer, offset, step)? as i32)
}

fn invalid_remote_image<T>(step: InjectionStep, message: &str) -> Result<T, InjectorError> {
    Err(InjectorError::StepFailed {
        step,
        source: io::Error::new(io::ErrorKind::InvalidData, message),
    })
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::Path;

    use crate::error::{InjectionStep, InjectorError};

    use super::{MemoryReader, resolve_module_export_address};

    const TEST_BASE: usize = 0x1000;
    const TEST_STEP: InjectionStep = InjectionStep::ResolveInitializeExport;

    #[test]
    fn resolves_named_export_from_pe32_image() {
        let reader = TestMemory::new(build_export_image(0x10b, 0x400, 0));

        let resolved = resolve_module_export_address(
            &reader,
            TEST_BASE,
            "dwm_lut_initialize",
            TEST_STEP,
            Path::new(r"C:\work\hook.dll"),
        )
        .expect("PE32 export should resolve");

        assert_eq!(resolved, TEST_BASE + 0x400);
    }

    #[test]
    fn resolves_named_export_from_pe32_plus_image() {
        let reader = TestMemory::new(build_export_image(0x20b, 0x480, 0));

        let resolved = resolve_module_export_address(
            &reader,
            TEST_BASE,
            "dwm_lut_initialize",
            TEST_STEP,
            Path::new(r"C:\work\hook.dll"),
        )
        .expect("PE32+ export should resolve");

        assert_eq!(resolved, TEST_BASE + 0x480);
    }

    #[test]
    fn reports_export_not_found_when_image_has_no_export_directory() {
        let mut image = build_export_image(0x20b, 0x400, 0);
        write_u32(&mut image, 0x80 + 24 + 0x70, 0);
        write_u32(&mut image, 0x80 + 24 + 0x70 + 4, 0);
        let reader = TestMemory::new(image);

        let error = resolve_module_export_address(
            &reader,
            TEST_BASE,
            "dwm_lut_initialize",
            TEST_STEP,
            Path::new(r"C:\work\hook.dll"),
        )
        .expect_err("missing export directory must be rejected");

        match error {
            InjectorError::ExportNotFound { export, dll_path } => {
                assert_eq!(export, "dwm_lut_initialize");
                assert_eq!(dll_path, Path::new(r"C:\work\hook.dll"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rejects_invalid_export_directory_entries() {
        for (function_rva, ordinal, message) in [
            (0x400, 1, "export ordinal is out of range"),
            (0x210, 0, "forwarded exports are not supported"),
        ] {
            let reader = TestMemory::new(build_export_image(0x20b, function_rva, ordinal));

            let error = resolve_module_export_address(
                &reader,
                TEST_BASE,
                "dwm_lut_initialize",
                TEST_STEP,
                Path::new(r"C:\work\hook.dll"),
            )
            .expect_err("invalid export entry must be rejected");

            match error {
                InjectorError::StepFailed { step, source } => {
                    assert_eq!(step, TEST_STEP);
                    assert_eq!(source.kind(), io::ErrorKind::InvalidData);
                    assert!(source.to_string().contains(message));
                }
                other => panic!("unexpected error: {other}"),
            }
        }
    }

    struct TestMemory {
        image: Vec<u8>,
    }

    impl TestMemory {
        fn new(image: Vec<u8>) -> Self {
            Self { image }
        }
    }

    impl MemoryReader for TestMemory {
        fn read(
            &self,
            address: usize,
            size: usize,
            step: InjectionStep,
        ) -> Result<Vec<u8>, InjectorError> {
            let offset =
                address
                    .checked_sub(TEST_BASE)
                    .ok_or_else(|| InjectorError::StepFailed {
                        step,
                        source: io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "address before module base",
                        ),
                    })?;
            let end = offset
                .checked_add(size)
                .ok_or_else(|| InjectorError::StepFailed {
                    step,
                    source: io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "read exceeds image bounds",
                    ),
                })?;
            let bytes = self
                .image
                .get(offset..end)
                .ok_or_else(|| InjectorError::StepFailed {
                    step,
                    source: io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "read exceeds image bounds",
                    ),
                })?;
            Ok(bytes.to_vec())
        }
    }

    fn build_export_image(optional_magic: u16, function_rva: u32, ordinal: u16) -> Vec<u8> {
        let mut image = vec![0u8; 0x500];
        image[0] = 0x4d;
        image[1] = 0x5a;
        write_u32(&mut image, 0x3c, 0x80);

        write_u32(&mut image, 0x80, 0x0000_4550);
        write_u16(&mut image, 0x80 + 24, optional_magic);

        let export_directory_offset = match optional_magic {
            0x10b => 0x80 + 24 + 0x60,
            0x20b => 0x80 + 24 + 0x70,
            other => panic!("unsupported optional header for test: {other:#x}"),
        };
        write_u32(&mut image, export_directory_offset, 0x200);
        write_u32(&mut image, export_directory_offset + 4, 0x80);

        write_u32(&mut image, 0x200 + 20, 1);
        write_u32(&mut image, 0x200 + 24, 1);
        write_u32(&mut image, 0x200 + 28, 0x240);
        write_u32(&mut image, 0x200 + 32, 0x250);
        write_u32(&mut image, 0x200 + 36, 0x260);

        write_u32(&mut image, 0x240, function_rva);
        write_u32(&mut image, 0x250, 0x270);
        write_u16(&mut image, 0x260, ordinal);
        write_bytes(&mut image, 0x270, b"dwm_lut_initialize\0");

        image
    }

    fn write_u16(buffer: &mut [u8], offset: usize, value: u16) {
        buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_bytes(buffer: &mut [u8], offset: usize, value: &[u8]) {
        buffer[offset..offset + value.len()].copy_from_slice(value);
    }
}
