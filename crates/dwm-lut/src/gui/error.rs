use crate::config;
use crate::error::InjectorError;
use crate::host::HostCommandError;
use std::fmt;

#[derive(Debug)]
pub(super) enum GuiError {
    Injector(InjectorError),
    Config(config::ConfigError),
    Host(HostCommandError),
    WorkerStopped,
    InvalidEdit(String),
}

impl fmt::Display for GuiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Injector(error) => error.fmt(formatter),
            Self::Config(error) => error.fmt(formatter),
            Self::Host(error) => error.fmt(formatter),
            Self::WorkerStopped => formatter.write_str("background operation stopped unexpectedly"),
            Self::InvalidEdit(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for GuiError {}

impl From<InjectorError> for GuiError {
    fn from(value: InjectorError) -> Self {
        Self::Injector(value)
    }
}

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
