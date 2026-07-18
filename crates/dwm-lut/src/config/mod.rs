use std::fmt;
use std::path::PathBuf;

use dwm_lut_payload::PayloadError;

mod document;
mod resolve;

pub(crate) use document::{
    ConfigAssignmentDocument, ConfigColorMode, ConfigDocument, load_config_document,
    save_config_document,
};
pub use dwm_lut_payload::{ColorMode, MonitorIdentity, MonitorTarget};
pub(crate) use resolve::load_payload;

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse {
        line: Option<usize>,
        message: String,
    },
    Unsupported(&'static str),
    InvalidLut {
        path: PathBuf,
        source: PayloadError,
    },
    InvalidPayload(PayloadError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Parse {
                line: Some(line),
                message,
            } => write!(f, "parse error at line {line}: {message}"),
            Self::Parse {
                line: None,
                message,
            } => write!(f, "parse error: {message}"),
            Self::Unsupported(message) => write!(f, "unsupported: {message}"),
            Self::InvalidLut { path, source } => {
                write!(f, "invalid LUT {}: {source}", path.display())
            }
            Self::InvalidPayload(source) => write!(f, "invalid payload: {source}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl ConfigError {
    pub(crate) fn parse_at(line: usize, message: impl Into<String>) -> Self {
        Self::Parse {
            line: Some(line),
            message: message.into(),
        }
    }

    pub(crate) fn parse_message(message: impl Into<String>) -> Self {
        Self::Parse {
            line: None,
            message: message.into(),
        }
    }
}
