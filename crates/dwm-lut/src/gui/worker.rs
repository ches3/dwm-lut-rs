use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui;

use crate::host::{HostApplication, HostCommandError};

use super::GuiError;

#[derive(Clone)]
pub(crate) enum Operation {
    Apply { path: PathBuf, profile: String },
    Disable,
}

pub(crate) struct Worker {
    application: Arc<HostApplication>,
    pending: Option<Operation>,
    receiver: Option<Receiver<Result<(), HostCommandError>>>,
}

impl Worker {
    pub(crate) fn new(application: Arc<HostApplication>) -> Self {
        Self {
            application,
            pending: None,
            receiver: None,
        }
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn pending_label(&self) -> Option<&'static str> {
        self.pending.as_ref().map(Operation::label)
    }

    pub(crate) fn spawn(&mut self, operation: Operation, context: egui::Context) {
        if self.is_busy() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let application = Arc::clone(&self.application);
        self.pending = Some(operation.clone());
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            let result = match operation {
                Operation::Apply { path, profile } => {
                    application.apply(path, Some(profile)).map(|_| ())
                }
                Operation::Disable => application.disable().map(|_| ()),
            };
            let _ = sender.send(result);
            context.request_repaint();
        });
    }

    pub(crate) fn poll(&mut self) -> Option<Result<(), GuiError>> {
        match self.receiver.as_ref()?.try_recv() {
            Ok(result) => {
                self.receiver = None;
                self.pending = None;
                Some(result.map_err(Into::into))
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.receiver = None;
                self.pending.take()?;
                Some(Err(GuiError::WorkerStopped))
            }
        }
    }
}

impl Operation {
    fn label(&self) -> &'static str {
        match self {
            Self::Apply { .. } => "Applying LUT configuration...",
            Self::Disable => "Disabling LUT...",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_worker_reports_error_and_clears_busy_state() {
        let (sender, receiver) = mpsc::channel();
        let mut worker = Worker {
            application: test_application(),
            pending: Some(Operation::Disable),
            receiver: Some(receiver),
        };
        drop(sender);

        let result = worker.poll().expect("disconnect should produce a result");

        assert!(matches!(result, Err(GuiError::WorkerStopped)));
        assert!(!worker.is_busy());
    }

    fn test_application() -> Arc<HostApplication> {
        HostApplication::test_instance()
    }
}
