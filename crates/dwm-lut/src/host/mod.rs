use std::fmt;
use std::io;

use crate::control::ControlError;
use crate::inject::InjectError;
use crate::paths::PathError;
use crate::platform::elevation::ElevationError;
use crate::platform::security::SecurityError;

pub const HOST_BUSY_MESSAGE: &str =
    "dwm-lut host instance is busy; retry after the current command finishes";

#[derive(Debug)]
pub enum HostProcessError {
    AlreadyRunning,
    LaunchFailed {
        operation: &'static str,
        source: io::Error,
    },
    Elevation(ElevationError),
    Security(SecurityError),
    StartupFailed(String),
    UiUnavailable,
    PanicAlreadyReported,
    Instance {
        operation: &'static str,
        source: io::Error,
    },
}

impl fmt::Display for HostProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => write!(
                f,
                "dwm-lut host instance is already running in this session"
            ),
            Self::LaunchFailed { operation, source } => {
                write!(f, "host {operation} failed: {source}")
            }
            Self::Elevation(ElevationError::Cancelled) => write!(f, "host elevation was canceled"),
            Self::Elevation(ElevationError::RequiresAdministratorUser) => write!(
                f,
                "host launch requires signing in with an administrator account"
            ),
            Self::Elevation(error) => write!(f, "host {error}"),
            Self::Security(error) => error.fmt(f),
            Self::StartupFailed(message) => f.write_str(message),
            Self::UiUnavailable => write!(f, "host UI event loop is unavailable"),
            Self::PanicAlreadyReported => {
                write!(f, "host panic was already reported")
            }
            Self::Instance { operation, source } => {
                write!(f, "host instance {operation} failed: {source}")
            }
        }
    }
}

impl std::error::Error for HostProcessError {}

impl From<ElevationError> for HostProcessError {
    fn from(value: ElevationError) -> Self {
        Self::Elevation(value)
    }
}

impl From<SecurityError> for HostProcessError {
    fn from(value: SecurityError) -> Self {
        Self::Security(value)
    }
}

#[derive(Debug)]
pub enum HostRunError {
    Process(HostProcessError),
    Control(ControlError),
    Inject(InjectError),
    Path(PathError),
}

impl fmt::Display for HostRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Process(error) => error.fmt(f),
            Self::Control(error) => error.fmt(f),
            Self::Inject(error) => error.fmt(f),
            Self::Path(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for HostRunError {}

impl From<HostProcessError> for HostRunError {
    fn from(value: HostProcessError) -> Self {
        Self::Process(value)
    }
}

impl From<ControlError> for HostRunError {
    fn from(value: ControlError) -> Self {
        Self::Control(value)
    }
}

impl From<InjectError> for HostRunError {
    fn from(value: InjectError) -> Self {
        Self::Inject(value)
    }
}

impl From<PathError> for HostRunError {
    fn from(value: PathError) -> Self {
        Self::Path(value)
    }
}

impl From<SecurityError> for HostRunError {
    fn from(value: SecurityError) -> Self {
        Self::Process(HostProcessError::from(value))
    }
}

mod control_handler;
mod controller;
mod hook_status;
mod instance;
pub(crate) mod launch;
mod run;
mod startup_ipc;
mod status_poller;

pub(crate) use control_handler::ControlCommandHandler;
pub(crate) use controller::{HostCommandError, HostController, HostState, MutationCompletion};
pub(crate) use hook_status::HookRuntimeStatus;
pub(crate) use instance::{HostInstanceClaim, HostInstanceGuard, HostInstanceWaiter};
pub use run::{run_background, run_host};
