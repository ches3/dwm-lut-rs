use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::InjectorError;

pub(crate) const MAX_CONTROL_MESSAGE_BYTES: usize = 64 * 1024;
pub(crate) const CONTROL_PROTOCOL_VERSION: u32 = 2;
pub(crate) const PROTOCOL_MISMATCH_STATUS: &str = "protocol_mismatch";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ControlRequest {
    pub(crate) protocol_version: u32,
    #[serde(flatten)]
    pub(crate) command: ControlCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub(crate) enum ControlCommand {
    Apply {
        config_path: PathBuf,
        profile: Option<String>,
    },
    Disable,
    Status,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ControlResponse {
    pub(crate) protocol_version: u32,
    pub(crate) ok: bool,
    pub(crate) message: String,
    pub(crate) status: String,
}

impl ControlResponse {
    pub(crate) fn ok(message: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            ok: true,
            message: message.into(),
            status: status.into(),
        }
    }

    pub(crate) fn error(message: impl Into<String>, status: impl Into<String>) -> Self {
        Self {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            ok: false,
            message: message.into(),
            status: status.into(),
        }
    }

    pub(crate) fn protocol_mismatch(peer_version: u32) -> Self {
        Self::error(
            format!(
                "control protocol version mismatch: peer={}, local={}; restart the host instance",
                peer_version, CONTROL_PROTOCOL_VERSION
            ),
            PROTOCOL_MISMATCH_STATUS,
        )
    }
}

pub(crate) fn encode_request(request: &ControlRequest) -> Result<Vec<u8>, InjectorError> {
    encode_json(request)
}

pub(crate) fn decode_request(bytes: &[u8]) -> Result<ControlRequest, InjectorError> {
    decode_json(bytes)
}

pub(crate) fn encode_response(response: &ControlResponse) -> Result<Vec<u8>, InjectorError> {
    encode_json(response)
}

pub(crate) fn decode_response(bytes: &[u8]) -> Result<ControlResponse, InjectorError> {
    decode_json(bytes)
}

fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, InjectorError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| InjectorError::ControlProtocol(error.to_string()))?;
    validate_message_len(bytes.len())?;
    Ok(bytes)
}

fn decode_json<T>(bytes: &[u8]) -> Result<T, InjectorError>
where
    T: for<'de> Deserialize<'de>,
{
    if bytes.is_empty() {
        return Err(InjectorError::ControlProtocol(
            "message must not be empty".to_string(),
        ));
    }
    validate_message_len(bytes.len())?;
    serde_json::from_slice(bytes).map_err(|error| InjectorError::ControlProtocol(error.to_string()))
}

pub(crate) fn validate_message_len(len: usize) -> Result<(), InjectorError> {
    if len > MAX_CONTROL_MESSAGE_BYTES {
        return Err(InjectorError::ControlProtocol(format!(
            "message is too large: {len} bytes exceeds {MAX_CONTROL_MESSAGE_BYTES} bytes"
        )));
    }

    Ok(())
}

pub(crate) fn validate_response_protocol(
    response: ControlResponse,
) -> Result<ControlResponse, InjectorError> {
    if response.protocol_version == CONTROL_PROTOCOL_VERSION
        || response.status == PROTOCOL_MISMATCH_STATUS
    {
        return Ok(response);
    }

    Err(InjectorError::ControlProtocol(format!(
        "control response protocol version mismatch: peer={}, local={}",
        response.protocol_version, CONTROL_PROTOCOL_VERSION
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrips_through_json() {
        let request = ControlRequest {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            command: ControlCommand::Apply {
                config_path: PathBuf::from(r"C:\profiles\config.json"),
                profile: Some("desktop".to_string()),
            },
        };

        let encoded = encode_request(&request).expect("request should encode");
        let decoded = decode_request(&encoded).expect("request should decode");

        assert_eq!(decoded, request);
    }

    #[test]
    fn response_roundtrips_through_json() {
        let response = ControlResponse::ok("host instance is running", "running");

        let encoded = encode_response(&response).expect("response should encode");
        let decoded = decode_response(&encoded).expect("response should decode");

        assert_eq!(decoded, response);
    }

    #[test]
    fn stop_request_roundtrips_through_json() {
        let request = ControlRequest {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            command: ControlCommand::Stop,
        };

        let encoded = encode_request(&request).expect("request should encode");
        let decoded = decode_request(&encoded).expect("request should decode");
        assert_eq!(decoded, request);
    }

    #[test]
    fn rejects_unknown_command() {
        let error = decode_request(br#"{"protocol_version":1,"command":"reload"}"#)
            .expect_err("unknown command fails");

        assert!(matches!(error, InjectorError::ControlProtocol(_)));
    }

    #[test]
    fn rejects_malformed_json() {
        let error = decode_request(br#"{"protocol_version":1,"command":"status""#)
            .expect_err("malformed json fails");

        assert!(matches!(error, InjectorError::ControlProtocol(_)));
    }

    #[test]
    fn rejects_empty_message() {
        let error = decode_request(b"").expect_err("empty message fails");

        assert!(
            matches!(error, InjectorError::ControlProtocol(message) if message.contains("empty"))
        );
    }

    #[test]
    fn rejects_oversized_message() {
        let error = validate_message_len(MAX_CONTROL_MESSAGE_BYTES + 1)
            .expect_err("oversized message fails");

        assert!(
            matches!(error, InjectorError::ControlProtocol(message) if message.contains("too large"))
        );
    }

    #[test]
    fn normal_response_rejects_different_protocol_version() {
        let mut response = ControlResponse::ok("done", "running");
        response.protocol_version = CONTROL_PROTOCOL_VERSION + 1;
        let error = validate_response_protocol(response)
            .expect_err("normal response from a different protocol version must fail");

        assert!(
            matches!(error, InjectorError::ControlProtocol(message) if message.contains("protocol version mismatch"))
        );
    }

    #[test]
    fn protocol_mismatch_response_is_displayable_with_different_protocol_version() {
        let response = ControlResponse::protocol_mismatch(CONTROL_PROTOCOL_VERSION + 1);
        let response = validate_response_protocol(response)
            .expect("protocol mismatch response should remain displayable");

        assert!(!response.ok);
        assert_eq!(response.status, PROTOCOL_MISMATCH_STATUS);
        assert!(response.message.contains("peer=3"));
        assert!(response.message.contains("local=2"));
    }
}
