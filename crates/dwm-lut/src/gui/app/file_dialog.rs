use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui;

use crate::gui::ConfigColorMode;

use super::DwmLutApp;

pub(crate) struct LutBrowseRequest {
    pub(crate) device_path: String,
    pub(crate) color_mode: ConfigColorMode,
}

pub(super) struct LutBrowseTask {
    request: LutBrowseRequest,
    receiver: Receiver<Option<PathBuf>>,
}

pub(super) enum LutBrowseState {
    Idle,
    Pending(LutBrowseRequest),
    Running(LutBrowseTask),
}

impl LutBrowseState {
    pub(super) fn is_active(&self) -> bool {
        !matches!(self, Self::Idle)
    }
}

impl DwmLutApp {
    pub(super) fn request_lut_browse(&mut self, request: LutBrowseRequest) {
        if matches!(self.lut_browse, LutBrowseState::Idle) {
            self.lut_browse = LutBrowseState::Pending(request);
        }
    }

    pub(super) fn clear_lut_browse(&mut self) {
        self.lut_browse = LutBrowseState::Idle;
    }

    pub(super) fn start_pending_lut_browse(
        &mut self,
        frame: &eframe::Frame,
        context: &egui::Context,
    ) {
        let request = match std::mem::replace(&mut self.lut_browse, LutBrowseState::Idle) {
            LutBrowseState::Pending(request) => request,
            state => {
                self.lut_browse = state;
                return;
            }
        };

        let dialog = rfd::FileDialog::new()
            .set_parent(frame)
            .add_filter("3D LUT", &["cube", "txt"]);
        let (sender, receiver) = mpsc::channel();
        let repaint = context.clone();
        let spawn_result = std::thread::Builder::new()
            .name("dwm-lut-file-dialog".to_string())
            .spawn(move || {
                let selected = dialog.pick_file();
                if sender.send(selected).is_ok() {
                    repaint.request_repaint();
                }
            });

        match spawn_result {
            Ok(_) => {
                self.lut_browse = LutBrowseState::Running(LutBrowseTask { request, receiver });
            }
            Err(error) => self.show_error(format!("failed to start file dialog: {error}")),
        }
    }

    pub(super) fn poll_lut_browse(&mut self) {
        let result = match &self.lut_browse {
            LutBrowseState::Running(task) => match task.receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.lut_browse = LutBrowseState::Idle;
                    self.show_error("file dialog stopped unexpectedly");
                    return;
                }
            },
            LutBrowseState::Idle | LutBrowseState::Pending(_) => return,
        };
        let task = match std::mem::replace(&mut self.lut_browse, LutBrowseState::Idle) {
            LutBrowseState::Running(task) => task,
            _ => {
                self.show_error("file dialog stopped unexpectedly");
                return;
            }
        };
        if let Some(path) = result {
            self.set_assignment(&task.request.device_path, task.request.color_mode, path);
        }
    }
}
