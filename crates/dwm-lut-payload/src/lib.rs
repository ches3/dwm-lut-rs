use std::fmt;

use bincode::{Decode, Encode};

mod status;

pub use status::{InitializeStatus, ReplaceAssignmentsStatus, ShutdownStatus};

#[cfg(not(target_pointer_width = "64"))]
compile_error!("dwm-lut-rs supports only 64-bit Windows targets");

pub const PAYLOAD_VERSION: u32 = 1;
pub const PAYLOAD_HEADER_LEN: u32 = 12;
pub const MAX_PAYLOAD_BYTES: usize = 128 * 1024 * 1024;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DwmLutPayloadBuffer {
    pub data: *const u8,
    pub len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
pub enum ColorMode {
    Sdr,
    Hdr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
pub struct AdapterLuid {
    pub high_part: i32,
    pub low_part: u32,
}

impl fmt::Display for AdapterLuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08x}:{:08x}", self.high_part as u32, self.low_part)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
pub struct MonitorIdentity {
    pub adapter_luid: AdapterLuid,
    pub target_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct MonitorTarget {
    pub identity: MonitorIdentity,
    pub color_mode: ColorMode,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct PayloadLut {
    pub size: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub values: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct PayloadAssignment {
    pub target: MonitorTarget,
    pub lut: PayloadLut,
}

#[derive(Debug, Clone, PartialEq, Default, Encode, Decode)]
pub struct HookPayload {
    pub assignments: Vec<PayloadAssignment>,
}

#[derive(Debug)]
pub enum PayloadError {
    EmptyBuffer,
    TooLarge {
        len: usize,
        max: usize,
    },
    HeaderTooShort {
        len: usize,
    },
    UnsupportedVersion {
        actual: u32,
    },
    InvalidHeaderLength {
        actual: u32,
    },
    LengthMismatch {
        header_len: u32,
        body_len: u32,
        len: usize,
    },
    Encode(bincode::error::EncodeError),
    Decode(bincode::error::DecodeError),
    TrailingBytes {
        consumed: usize,
        body_len: usize,
    },
    NoAssignments,
    LutTooSmall {
        size: u32,
    },
    LutEntryCountOverflow {
        size: u32,
    },
    LutEntryCountMismatch {
        size: u32,
        expected: usize,
        actual: usize,
    },
    NonFiniteLutValue,
    NonFiniteDomain,
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyBuffer => write!(f, "payload buffer is empty"),
            Self::TooLarge { len, max } => {
                write!(f, "payload is too large: {len} bytes exceeds {max} bytes")
            }
            Self::HeaderTooShort { len } => write!(f, "payload header is incomplete: {len} bytes"),
            Self::UnsupportedVersion { actual } => {
                write!(f, "payload version is unsupported: {actual}")
            }
            Self::InvalidHeaderLength { actual } => {
                write!(f, "payload header length is invalid: {actual}")
            }
            Self::LengthMismatch {
                header_len,
                body_len,
                len,
            } => write!(
                f,
                "payload length mismatch: header_len={header_len}, body_len={body_len}, len={len}"
            ),
            Self::Encode(error) => write!(f, "{error}"),
            Self::Decode(error) => write!(f, "{error}"),
            Self::TrailingBytes { consumed, body_len } => write!(
                f,
                "payload body has trailing bytes: consumed={consumed}, body_len={body_len}"
            ),
            Self::NoAssignments => write!(f, "payload does not contain any LUT assignments"),
            Self::LutTooSmall { size } => write!(f, "LUT size must be at least 2, got {size}"),
            Self::LutEntryCountOverflow { size } => write!(f, "LUT size is too large: {size}"),
            Self::LutEntryCountMismatch {
                size,
                expected,
                actual,
            } => write!(
                f,
                "LUT entry count mismatch for size {size}: expected {expected}, got {actual}"
            ),
            Self::NonFiniteLutValue => write!(f, "LUT values must be finite"),
            Self::NonFiniteDomain => write!(f, "LUT domain values must be finite"),
        }
    }
}

impl std::error::Error for PayloadError {}

pub fn serialize_payload(payload: &HookPayload) -> Result<Vec<u8>, PayloadError> {
    validate_payload(payload)?;
    let body = bincode::encode_to_vec(payload, bincode::config::standard())
        .map_err(PayloadError::Encode)?;
    let len = usize::try_from(PAYLOAD_HEADER_LEN).expect("header length fits usize") + body.len();
    if len > MAX_PAYLOAD_BYTES {
        return Err(PayloadError::TooLarge {
            len,
            max: MAX_PAYLOAD_BYTES,
        });
    }

    let body_len = u32::try_from(body.len()).map_err(|_| PayloadError::TooLarge {
        len,
        max: MAX_PAYLOAD_BYTES,
    })?;
    let mut bytes = Vec::with_capacity(len);
    bytes.extend_from_slice(&PAYLOAD_VERSION.to_le_bytes());
    bytes.extend_from_slice(&PAYLOAD_HEADER_LEN.to_le_bytes());
    bytes.extend_from_slice(&body_len.to_le_bytes());
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

pub fn deserialize_payload(bytes: &[u8]) -> Result<HookPayload, PayloadError> {
    validate_payload_bytes(bytes)?;
    let body = &bytes[PAYLOAD_HEADER_LEN as usize..];
    let (payload, consumed): (HookPayload, usize) =
        bincode::decode_from_slice(body, bincode::config::standard())
            .map_err(PayloadError::Decode)?;
    if consumed != body.len() {
        return Err(PayloadError::TrailingBytes {
            consumed,
            body_len: body.len(),
        });
    }
    validate_payload(&payload)?;
    Ok(payload)
}

/// Deserializes a payload from an FFI buffer.
///
/// # Safety
///
/// `buffer` must either be null or point to a readable `DwmLutPayloadBuffer` in
/// the current process. When `buffer->data` is non-null, it must be readable for
/// `buffer->len` bytes.
pub unsafe fn deserialize_payload_buffer(
    buffer: *const DwmLutPayloadBuffer,
) -> Result<HookPayload, PayloadError> {
    if buffer.is_null() {
        return Err(PayloadError::EmptyBuffer);
    }
    let buffer = unsafe { &*buffer };
    if buffer.data.is_null() || buffer.len == 0 {
        return Err(PayloadError::EmptyBuffer);
    }
    if buffer.len > MAX_PAYLOAD_BYTES {
        return Err(PayloadError::TooLarge {
            len: buffer.len,
            max: MAX_PAYLOAD_BYTES,
        });
    }

    let bytes = unsafe { std::slice::from_raw_parts(buffer.data, buffer.len) };
    deserialize_payload(bytes)
}

pub fn validate_payload(payload: &HookPayload) -> Result<(), PayloadError> {
    if payload.assignments.is_empty() {
        return Err(PayloadError::NoAssignments);
    }

    for assignment in &payload.assignments {
        validate_lut(&assignment.lut)?;
    }

    Ok(())
}

pub fn validate_lut(lut: &PayloadLut) -> Result<(), PayloadError> {
    if lut.size < 2 {
        return Err(PayloadError::LutTooSmall { size: lut.size });
    }

    let expected = expected_entry_count(lut.size)?;
    if lut.values.len() != expected {
        return Err(PayloadError::LutEntryCountMismatch {
            size: lut.size,
            expected,
            actual: lut.values.len(),
        });
    }

    if lut
        .domain_min
        .iter()
        .chain(lut.domain_max.iter())
        .any(|value| !value.is_finite())
    {
        return Err(PayloadError::NonFiniteDomain);
    }
    if lut
        .values
        .iter()
        .flat_map(|value| value.iter())
        .any(|value| !value.is_finite())
    {
        return Err(PayloadError::NonFiniteLutValue);
    }

    Ok(())
}

fn validate_payload_bytes(bytes: &[u8]) -> Result<(), PayloadError> {
    if bytes.is_empty() {
        return Err(PayloadError::EmptyBuffer);
    }
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(PayloadError::TooLarge {
            len: bytes.len(),
            max: MAX_PAYLOAD_BYTES,
        });
    }
    if bytes.len() < PAYLOAD_HEADER_LEN as usize {
        return Err(PayloadError::HeaderTooShort { len: bytes.len() });
    }

    let version = u32::from_le_bytes(bytes[0..4].try_into().expect("slice length is fixed"));
    let header_len = u32::from_le_bytes(bytes[4..8].try_into().expect("slice length is fixed"));
    let body_len = u32::from_le_bytes(bytes[8..12].try_into().expect("slice length is fixed"));

    if version != PAYLOAD_VERSION {
        return Err(PayloadError::UnsupportedVersion { actual: version });
    }
    if header_len != PAYLOAD_HEADER_LEN {
        return Err(PayloadError::InvalidHeaderLength { actual: header_len });
    }
    let expected_len = usize::try_from(header_len).ok().and_then(|header_len| {
        usize::try_from(body_len)
            .ok()
            .and_then(|body_len| header_len.checked_add(body_len))
    });
    if expected_len != Some(bytes.len()) {
        return Err(PayloadError::LengthMismatch {
            header_len,
            body_len,
            len: bytes.len(),
        });
    }

    Ok(())
}

fn expected_entry_count(size: u32) -> Result<usize, PayloadError> {
    let size = usize::try_from(size).map_err(|_| PayloadError::LutEntryCountOverflow { size })?;
    size.checked_mul(size)
        .and_then(|value| value.checked_mul(size))
        .ok_or(PayloadError::LutEntryCountOverflow { size: size as u32 })
}

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use super::*;

    fn payload() -> HookPayload {
        HookPayload {
            assignments: vec![PayloadAssignment {
                target: MonitorTarget {
                    identity: MonitorIdentity {
                        adapter_luid: AdapterLuid {
                            high_part: 0,
                            low_part: 0x14e02,
                        },
                        target_id: 4357,
                    },
                    color_mode: ColorMode::Sdr,
                },
                lut: PayloadLut {
                    size: 2,
                    domain_min: [0.0, 0.0, 0.0],
                    domain_max: [1.0, 1.0, 1.0],
                    values: vec![[0.0, 0.0, 0.0]; 8],
                },
            }],
        }
    }

    #[test]
    fn payload_roundtrips_through_envelope() {
        let payload = payload();
        let bytes = serialize_payload(&payload).expect("payload should serialize");

        assert_eq!(
            deserialize_payload(&bytes).expect("payload should decode"),
            payload
        );
    }

    #[test]
    fn payload_rejects_unsupported_version_before_decode() {
        let mut bytes = serialize_payload(&payload()).expect("payload should serialize");
        bytes[0] ^= 0xff;

        assert!(matches!(
            deserialize_payload(&bytes),
            Err(PayloadError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn payload_rejects_trailing_body_bytes() {
        let mut bytes = serialize_payload(&payload()).expect("payload should serialize");
        bytes.extend_from_slice(&[0]);
        let body_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) + 1;
        bytes[8..12].copy_from_slice(&body_len.to_le_bytes());

        assert!(matches!(
            deserialize_payload(&bytes),
            Err(PayloadError::Decode(_)) | Err(PayloadError::TrailingBytes { .. })
        ));
    }

    #[test]
    fn payload_rejects_empty_assignments() {
        let error = serialize_payload(&HookPayload::default()).expect_err("empty payload fails");

        assert!(matches!(error, PayloadError::NoAssignments));
    }

    #[test]
    fn validate_lut_rejects_size_below_two() {
        let error = validate_lut(&PayloadLut {
            size: 1,
            domain_min: [0.0, 0.0, 0.0],
            domain_max: [1.0, 1.0, 1.0],
            values: vec![[0.0, 0.0, 0.0]],
        })
        .expect_err("size 1 should fail");

        assert!(matches!(error, PayloadError::LutTooSmall { size: 1 }));
    }

    #[test]
    fn validate_lut_rejects_entry_count_mismatch() {
        let error = validate_lut(&PayloadLut {
            size: 2,
            domain_min: [0.0, 0.0, 0.0],
            domain_max: [1.0, 1.0, 1.0],
            values: vec![[0.0, 0.0, 0.0]],
        })
        .expect_err("incomplete LUT should fail");

        assert!(matches!(
            error,
            PayloadError::LutEntryCountMismatch {
                size: 2,
                expected: 8,
                actual: 1,
            }
        ));
    }

    #[test]
    fn validate_lut_rejects_non_finite_domain() {
        let error = validate_lut(&PayloadLut {
            size: 2,
            domain_min: [f32::NAN, 0.0, 0.0],
            domain_max: [1.0, 1.0, 1.0],
            values: vec![[0.0, 0.0, 0.0]; 8],
        })
        .expect_err("non-finite domain should fail");

        assert!(matches!(error, PayloadError::NonFiniteDomain));
    }

    #[test]
    fn payload_buffer_layout_matches_c_contract() {
        assert_eq!(size_of::<DwmLutPayloadBuffer>(), 16);
        assert_eq!(offset_of!(DwmLutPayloadBuffer, data), 0);
        assert_eq!(offset_of!(DwmLutPayloadBuffer, len), 8);
    }
}
