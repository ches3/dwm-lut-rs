use crate::config;
use crate::host::HostCommandError;
use crate::paths::PathError;
use std::fmt;

#[derive(Debug)]
pub(super) enum GuiError {
    Config(config::ConfigError),
    Host(HostCommandError),
    Path(PathError),
    InvalidEdit(String),
}

impl fmt::Display for GuiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(formatter),
            Self::Host(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::InvalidEdit(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for GuiError {}

impl From<config::ConfigError> for GuiError {
    fn from(value: config::ConfigError) -> Self {
        Self::Config(value)
    }
}

impl From<HostCommandError> for GuiError {
    fn from(value: HostCommandError) -> Self {
        Self::Host(value)
    }
}

impl From<PathError> for GuiError {
    fn from(value: PathError) -> Self {
        Self::Path(value)
    }
}
