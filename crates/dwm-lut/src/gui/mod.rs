mod app;
mod error;
mod session;

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};

use crate::error::InjectorError;
use crate::host::HostController;

slint::include_modules!();

thread_local! {
    static UI_WAKE: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UiCommand {
    Show,
    HostStateChanged,
    Exit,
}

pub(crate) struct UiHandle {
    sender: mpsc::Sender<UiCommand>,
}

impl UiHandle {
    pub(crate) fn new() -> (Arc<Self>, Receiver<UiCommand>) {
        let (sender, receiver) = mpsc::channel();
        (Arc::new(Self { sender }), receiver)
    }

    pub(crate) fn send(&self, command: UiCommand) -> Result<(), InjectorError> {
        self.sender
            .send(command)
            .map_err(|_| InjectorError::HostUiUnavailable)?;
        schedule_ui_wake();
        Ok(())
    }
}

fn install_ui_wake(callback: impl Fn() + 'static) {
    UI_WAKE.with(|wake| {
        *wake.borrow_mut() = Some(Box::new(callback));
    });
}

fn clear_ui_wake() {
    UI_WAKE.with(|wake| {
        *wake.borrow_mut() = None;
    });
}

fn invoke_installed_ui_wake() {
    UI_WAKE.with(|wake| {
        if let Some(callback) = wake.borrow().as_ref() {
            callback();
        }
    });
}

fn schedule_ui_wake() {
    let _ = slint::invoke_from_event_loop(invoke_installed_ui_wake);
}

pub(crate) fn run_host_ui(
    controller: Arc<HostController>,
    ui_commands: Receiver<UiCommand>,
    ready: mpsc::Sender<()>,
) -> Result<(), InjectorError> {
    session::run(controller, ui_commands, ready)
}
