mod app;
mod error;
mod fonts;
mod tray;
mod worker;

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, OnceLock};

use eframe::egui;

use crate::error::InjectorError;
use crate::host::HostApplication;

use crate::config::{ConfigColorMode, ConfigDocument};
use error::GuiError;

#[derive(Debug, Clone)]
pub(crate) enum UiCommand {
    Show,
    Exit,
}

pub(crate) struct UiHandle {
    sender: mpsc::Sender<UiCommand>,
    context: OnceLock<egui::Context>,
}

impl UiHandle {
    pub(crate) fn new() -> (Arc<Self>, Receiver<UiCommand>) {
        let (sender, receiver) = mpsc::channel();
        (
            Arc::new(Self {
                sender,
                context: OnceLock::new(),
            }),
            receiver,
        )
    }

    pub(crate) fn attach_context(&self, context: egui::Context) {
        let _ = self.context.set(context);
    }

    pub(crate) fn send(&self, command: UiCommand) -> Result<(), InjectorError> {
        self.sender.send(command).map_err(|_| {
            InjectorError::HostStartupFailed("host UI event loop is unavailable".to_string())
        })?;
        if let Some(context) = self.context.get() {
            context.request_repaint();
        }
        Ok(())
    }
}

pub(crate) fn run_host_ui(
    application: Arc<HostApplication>,
    ui_handle: Arc<UiHandle>,
    ui_commands: Receiver<UiCommand>,
    ready: mpsc::Sender<()>,
) -> Result<(), InjectorError> {
    app::run_host(application, ui_handle, ui_commands, ready)
}
