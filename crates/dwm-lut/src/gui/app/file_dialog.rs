use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::config::ConfigColorMode;

pub(crate) struct LutBrowseRequest {
    pub(crate) device_path: String,
    pub(crate) color_mode: ConfigColorMode,
}

pub(crate) struct LutBrowseTask {
    pub(crate) request: LutBrowseRequest,
    receiver: Receiver<Option<PathBuf>>,
}

pub(crate) enum LutBrowseState {
    Idle,
    Running(LutBrowseTask),
}

pub(crate) fn start_lut_browse(
    request: LutBrowseRequest,
    dialog: rfd::FileDialog,
    wake: impl Fn() + Send + 'static,
) -> Result<LutBrowseTask, String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("dwm-lut-file-dialog".to_string())
        .spawn(move || {
            let selected = dialog.pick_file();
            if sender.send(selected).is_ok() {
                wake();
            }
        })
        .map_err(|error| format!("failed to start file dialog: {error}"))?;
    Ok(LutBrowseTask { request, receiver })
}

pub(crate) fn poll_lut_browse(
    state: &mut LutBrowseState,
) -> Result<Option<(LutBrowseRequest, Option<PathBuf>)>, String> {
    let result = match state {
        LutBrowseState::Running(task) => match task.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                *state = LutBrowseState::Idle;
                return Err("file dialog stopped unexpectedly".to_string());
            }
        },
        LutBrowseState::Idle => return Ok(None),
    };
    let task = match std::mem::replace(state, LutBrowseState::Idle) {
        LutBrowseState::Running(task) => task,
        _ => return Err("file dialog stopped unexpectedly".to_string()),
    };
    Ok(Some((task.request, result)))
}
