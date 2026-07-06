use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::InjectorError;

pub(crate) const MAX_CONTROL_MESSAGE_BYTES: usize = 64 * 1024;
pub(crate) const BUILD_MISMATCH_STATUS: &str = "build_mismatch";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ControlRequest {
    pub(crate) build_hash: String,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ControlResponse {
    pub(crate) build_hash: String,
    pub(crate) ok: bool,
    pub(crate) message: String,
    pub(crate) status: String,
}

impl ControlResponse {
    pub(crate) fn ok(
        build_hash: impl Into<String>,
        message: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        Self {
            build_hash: build_hash.into(),
            ok: true,
            message: message.into(),
            status: status.into(),
        }
    }

    pub(crate) fn error(
        build_hash: impl Into<String>,
        message: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        Self {
            build_hash: build_hash.into(),
            ok: false,
            message: message.into(),
            status: status.into(),
        }
    }

    pub(crate) fn build_mismatch(client_hash: &str, server_hash: &str) -> Self {
        Self::error(
            server_hash,
            format!(
                "control build hash mismatch: client={}, server={}; restart the elevated primary instance",
                short_hash(client_hash),
                short_hash(server_hash)
            ),
            BUILD_MISMATCH_STATUS,
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

pub(crate) fn validate_response_build_hash(
    response: ControlResponse,
    local_build_hash: &str,
) -> Result<ControlResponse, InjectorError> {
    if response.build_hash == local_build_hash || response.status == BUILD_MISMATCH_STATUS {
        return Ok(response);
    }

    Err(InjectorError::ControlProtocol(format!(
        "control response build hash mismatch: client={}, server={}",
        short_hash(local_build_hash),
        short_hash(&response.build_hash)
    )))
}

fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrips_through_json() {
        let request = ControlRequest {
            build_hash: "client-hash".to_string(),
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
        let response = ControlResponse::ok("server-hash", "primary instance is running", "running");

        let encoded = encode_response(&response).expect("response should encode");
        let decoded = decode_response(&encoded).expect("response should decode");

        assert_eq!(decoded, response);
    }

    #[test]
    fn rejects_unknown_command() {
        let error = decode_request(br#"{"build_hash":"client-hash","command":"reload"}"#)
            .expect_err("unknown command fails");

        assert!(matches!(error, InjectorError::ControlProtocol(_)));
    }

    #[test]
    fn rejects_malformed_json() {
        let error = decode_request(br#"{"build_hash":"client-hash","command":"status""#)
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
    fn normal_response_rejects_different_build_hash() {
        let response = ControlResponse::ok("server-hash", "done", "running");
        let error = validate_response_build_hash(response, "client-hash")
            .expect_err("normal response from a different build must fail");

        assert!(
            matches!(error, InjectorError::ControlProtocol(message) if message.contains("build hash mismatch"))
        );
    }

    #[test]
    fn build_mismatch_response_is_displayable_with_different_build_hash() {
        let response = ControlResponse::build_mismatch("client-hash", "server-hash");
        let response = validate_response_build_hash(response, "client-hash")
            .expect("build mismatch response should remain displayable");

        assert!(!response.ok);
        assert_eq!(response.status, BUILD_MISMATCH_STATUS);
        assert!(response.message.contains("client=client-hash"));
        assert!(response.message.contains("server=server-hash"));
    }
}
