use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Instant;

use eframe::egui;

use crate::backend::{MonitorListing, list_monitor_listings};
use crate::error::InjectorError;
use crate::host::{HostApplication, HostState};

use super::fonts::{FontError, FontUpdate, SystemFonts};
use super::tray::{TrayAction, TrayState};
use super::worker::{Operation, Worker};
use super::{ConfigColorMode, ConfigDocument, GuiError, UiCommand, UiHandle};

mod config_editor;
mod file_dialog;
mod monitor_events;
mod view;

use config_editor::{ConfigEditor, ConfigState, edit_and_save_config};
pub(super) use file_dialog::LutBrowseRequest;
use file_dialog::LutBrowseState;
use monitor_events::{
    MonitorChangeListener, MonitorChangeSignal, RETRY_DELAY as MONITOR_CHANGE_RETRY_DELAY,
    SETTLE_DELAY as MONITOR_CHANGE_SETTLE_DELAY,
};

pub(super) fn run_host(
    application: Arc<HostApplication>,
    ui_handle: Arc<UiHandle>,
    ui_commands: Receiver<UiCommand>,
    ready: Sender<()>,
) -> Result<(), InjectorError> {
    let monitor_changes = Arc::new(MonitorChangeSignal::new());
    let app_icon = Arc::new(
        eframe::icon_data::from_png_bytes(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icon.png"
        )))
        .map_err(|error| {
            InjectorError::HostStartupFailed(format!("application icon decode failed: {error}"))
        })?,
    );
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("dwm-lut")
            .with_inner_size(MAIN_VIEWPORT_SIZE)
            .with_min_inner_size(MAIN_VIEWPORT_MIN_SIZE)
            .with_icon(Arc::clone(&app_icon))
            .with_visible(false),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "dwm-lut",
        options,
        Box::new(move |context| {
            ui_handle.attach_context(context.egui_ctx.clone());
            let app = DwmLutApp::new(
                context,
                monitor_changes,
                application,
                ui_commands,
                app_icon.as_ref(),
            )?;
            ready.send(()).map_err(|_| {
                "control server stopped before the host UI became ready".to_string()
            })?;
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| InjectorError::HostStartupFailed(format!("host UI failed: {error}")))
}

const MAIN_VIEWPORT_SIZE: [f32; 2] = [800.0, 580.0];
const MAIN_VIEWPORT_MIN_SIZE: [f32; 2] = [720.0, 480.0];
const LOAD_ERROR_VIEWPORT_SIZE: [f32; 2] = [600.0, 300.0];
const LOAD_ERROR_VIEWPORT_MIN_SIZE: [f32; 2] = [500.0, 240.0];
pub(super) fn resize_viewport_for_config_state(context: &egui::Context, load_failed: bool) {
    let (size, min_size) = if load_failed {
        (LOAD_ERROR_VIEWPORT_SIZE, LOAD_ERROR_VIEWPORT_MIN_SIZE)
    } else {
        (MAIN_VIEWPORT_SIZE, MAIN_VIEWPORT_MIN_SIZE)
    };
    context.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
        min_size[0],
        min_size[1],
    )));
    context.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
        size[0], size[1],
    )));
}

pub(super) struct DwmLutApp {
    config_state: ConfigState,
    pub(super) monitors: Vec<MonitorListing>,
    pub(super) monitor_error: Option<String>,
    pub(super) error_dialog: Option<String>,
    pub(super) profile_dialog_error: Option<String>,
    pub(super) worker: Worker,
    modal: Option<ModalState>,
    profile_dialog_generation: u64,
    lut_browse: LutBrowseState,
    pub(super) fonts: Option<SystemFonts>,
    font_texts_dirty: bool,
    _monitor_change_listener: MonitorChangeListener,
    monitor_changes: Arc<MonitorChangeSignal>,
    monitor_refresh: MonitorRefresh,
    application: Arc<HostApplication>,
    ui_commands: Receiver<UiCommand>,
    tray: TrayState,
    window_visible: bool,
    exit_requested: bool,
}

pub(super) enum ModalState {
    Profile(ProfileDialog),
    DeleteProfile(String),
}

#[derive(Debug, Clone, Copy)]
enum MonitorRefresh {
    Idle,
    Scheduled { at: Instant, retry_after: bool },
}

impl DwmLutApp {
    fn new(
        creation_context: &eframe::CreationContext<'_>,
        monitor_changes: Arc<MonitorChangeSignal>,
        application: Arc<HostApplication>,
        ui_commands: Receiver<UiCommand>,
        app_icon: &egui::IconData,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let context = &creation_context.egui_ctx;
        monitor_changes.set_context(context);
        let monitor_change_listener =
            MonitorChangeListener::attach(creation_context, Arc::clone(&monitor_changes))?;
        context.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(8.0, 4.0);
            style.spacing.scroll = egui::style::ScrollStyle::solid();
            let button_content_height = style.text_styles[&egui::TextStyle::Button]
                .size
                .max(style.spacing.icon_width);
            let button_height = button_content_height + 2.0 * style.spacing.button_padding.y;
            style.spacing.interact_size.y = style.spacing.interact_size.y.max(button_height);
            style.visuals.text_edit_bg_color = Some(style.visuals.widgets.inactive.bg_fill);
            style.visuals.widgets.noninteractive.fg_stroke.color = if style.visuals.dark_mode {
                egui::Color32::from_gray(200)
            } else {
                egui::Color32::from_gray(50)
            };
            style.visuals.widgets.inactive.fg_stroke.color = if style.visuals.dark_mode {
                egui::Color32::from_gray(200)
            } else {
                egui::Color32::from_gray(50)
            };
            style.visuals.widgets.hovered.fg_stroke.color = if style.visuals.dark_mode {
                egui::Color32::from_gray(220)
            } else {
                egui::Color32::from_gray(40)
            };
            style.visuals.widgets.active.fg_stroke.color = if style.visuals.dark_mode {
                egui::Color32::from_gray(220)
            } else {
                egui::Color32::from_gray(40)
            };
            style.visuals.weak_text_alpha = 0.85;
        });
        let config_state = ConfigState::load_default();
        let (monitors, monitor_error) = match list_monitor_listings() {
            Ok(monitors) => (monitors, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        let tray = TrayState::new(context, app_icon)?;
        let app = Self {
            config_state,
            monitors,
            monitor_error,
            error_dialog: None,
            profile_dialog_error: None,
            worker: Worker::new(Arc::clone(&application)),
            modal: None,
            profile_dialog_generation: 0,
            lut_browse: LutBrowseState::Idle,
            fonts: None,
            font_texts_dirty: true,
            _monitor_change_listener: monitor_change_listener,
            monitor_changes,
            monitor_refresh: MonitorRefresh::Idle,
            application,
            ui_commands,
            tray,
            window_visible: false,
            exit_requested: false,
        };
        if app.config_load_error().is_some() {
            resize_viewport_for_config_state(context, true);
        }
        Ok(app)
    }

    pub(super) fn refresh_system_fonts(
        &mut self,
        context: &egui::Context,
    ) -> Result<(), FontError> {
        if !self.font_texts_dirty {
            return Ok(());
        }
        if self.fonts.is_none() {
            return Ok(());
        }
        self.font_texts_dirty = false;

        let texts = self.font_fallback_texts();
        let update = self
            .fonts
            .as_mut()
            .expect("system fonts remained loaded")
            .ensure_texts(context, texts.iter().map(String::as_str))?;
        self.handle_font_update(update);
        Ok(())
    }

    pub(super) fn prepare_input_fonts<'a>(
        &mut self,
        context: &egui::Context,
        texts: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), FontError> {
        let Some(fonts) = self.fonts.as_mut() else {
            return Ok(());
        };
        let update = fonts.prepare_input_texts(context, texts)?;
        self.handle_font_update(update);
        Ok(())
    }

    fn handle_font_update(&mut self, update: FontUpdate) {
        if !update.unresolved.is_empty() {
            self.show_error(format!(
                "No installed font for {}",
                update
                    .unresolved
                    .iter()
                    .map(|codepoint| format!("U+{codepoint:04X}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    pub(super) fn show_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.error_dialog.as_deref() != Some(&message) {
            self.font_texts_dirty = true;
        }
        self.error_dialog = Some(message);
    }

    pub(super) fn dismiss_error(&mut self) {
        self.error_dialog = None;
    }

    fn font_fallback_texts(&self) -> Vec<String> {
        let mut texts = Vec::new();
        match &self.config_state {
            ConfigState::Ready(editor) => {
                texts.push(editor.path.display().to_string());
                texts.push(editor.selected_profile.clone());
                texts.push(editor.document.default_profile.clone());
                for (name, profile) in &editor.document.profiles {
                    texts.push(name.clone());
                    for assignment in &profile.assignments {
                        texts.push(assignment.monitor_device_path.clone());
                        texts.push(assignment.lut_path.display().to_string());
                    }
                }
            }
            ConfigState::LoadFailed { path, error } => {
                if let Some(path) = path {
                    texts.push(path.display().to_string());
                }
                texts.push(error.to_string());
            }
        }
        if let Some(error) = &self.error_dialog {
            texts.push(error.clone());
        }
        if let Some(error) = &self.monitor_error {
            texts.push(error.clone());
        }
        if let Some(error) = &self.profile_dialog_error {
            texts.push(error.clone());
        }
        for monitor in &self.monitors {
            texts.push(monitor.friendly_name.clone());
            texts.push(monitor.edid_pnp_id.clone());
            texts.push(monitor.monitor_device_path.clone());
        }
        if let Some(modal) = &self.modal {
            match modal {
                ModalState::Profile(dialog) => match dialog {
                    ProfileDialog::Add { value } => texts.push(value.clone()),
                    ProfileDialog::Rename { original, value } => {
                        texts.push(original.clone());
                        texts.push(value.clone());
                    }
                },
                ModalState::DeleteProfile(name) => texts.push(name.clone()),
            }
        }
        match &self.lut_browse {
            LutBrowseState::Pending(request) => texts.push(request.device_path.clone()),
            LutBrowseState::Running(_) | LutBrowseState::Idle => {}
        }
        texts
    }

    pub(super) fn config(&self) -> Option<&ConfigDocument> {
        self.config_state.document()
    }

    pub(super) fn config_load_error(&self) -> Option<&GuiError> {
        self.config_state.load_error()
    }

    pub(super) fn retry_config(&mut self) {
        self.config_state = self.config_state.retry();
        self.modal = None;
        self.profile_dialog_error = None;
        self.clear_lut_browse();
        self.font_texts_dirty = true;
    }

    pub(super) fn ui_blocked(&self) -> bool {
        self.worker.is_busy() || self.modal.is_some() || self.lut_browse.is_active()
    }

    pub(super) fn poll_worker(&mut self) {
        let Some(result) = self.worker.poll() else {
            return;
        };
        match result {
            Ok(()) => {}
            Err(error) => self.show_error(error.to_string()),
        }
    }

    pub(super) fn poll_monitor_changes(&mut self, context: &egui::Context) {
        if self.monitor_changes.take() {
            self.monitor_refresh = MonitorRefresh::Scheduled {
                at: Instant::now() + MONITOR_CHANGE_SETTLE_DELAY,
                retry_after: true,
            };
        }

        let MonitorRefresh::Scheduled { at, retry_after } = self.monitor_refresh else {
            return;
        };
        let now = Instant::now();
        if now < at {
            context.request_repaint_after(at - now);
            return;
        }

        self.refresh_monitors();
        if retry_after {
            self.monitor_refresh = MonitorRefresh::Scheduled {
                at: now + MONITOR_CHANGE_RETRY_DELAY,
                retry_after: false,
            };
            context.request_repaint_after(MONITOR_CHANGE_RETRY_DELAY);
        } else {
            self.monitor_refresh = MonitorRefresh::Idle;
        }
    }

    fn refresh_monitors(&mut self) {
        match list_monitor_listings() {
            Ok(monitors) => {
                if self.monitors != monitors {
                    self.monitors = monitors;
                    self.font_texts_dirty = true;
                }
                self.monitor_error = None;
            }
            Err(error) => {
                let error = error.to_string();
                if self.monitor_error.as_deref() != Some(&error) {
                    self.font_texts_dirty = true;
                }
                self.monitor_error = Some(error);
            }
        }
    }

    pub(super) fn apply(&mut self, context: &egui::Context) {
        let Some(editor) = self.editor() else {
            return;
        };
        self.worker.spawn(
            Operation::Apply {
                path: editor.path.clone(),
                profile: editor.selected_profile.clone(),
            },
            context.clone(),
        );
    }

    pub(super) fn handle_close_request(&mut self, context: &egui::Context) {
        let close_requested = context.input(|input| input.viewport().close_requested());
        match close_disposition(close_requested, self.exit_requested) {
            CloseDisposition::None | CloseDisposition::Exit => {}
            CloseDisposition::Hide => {
                self.window_visible = false;
                context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.unload_system_fonts(context);
            }
        }
    }

    pub(super) fn poll_host_events(&mut self, context: &egui::Context) {
        loop {
            match self.ui_commands.try_recv() {
                Ok(UiCommand::Show) => self.open_window(context),
                Ok(UiCommand::Exit) => self.close_app(context),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.close_app(context);
                    break;
                }
            }
        }

        self.tray.set_exit_enabled(self.can_exit());
        while let Some(action) = self.tray.poll() {
            match action {
                TrayAction::Open => self.open_window(context),
                TrayAction::Exit => {
                    self.exit_host(context);
                    break;
                }
            }
        }
    }

    fn open_window(&mut self, context: &egui::Context) {
        if open_disposition(self.window_visible) == OpenDisposition::FocusOnly {
            self.show_window(context);
            return;
        }

        let load_failed_before = self.config_load_error().is_some();
        self.config_state = self.config_state.reload();
        self.modal = None;
        self.profile_dialog_error = None;
        self.clear_lut_browse();
        self.font_texts_dirty = true;

        let load_failed_after = self.config_load_error().is_some();
        if load_failed_before != load_failed_after {
            resize_viewport_for_config_state(context, load_failed_after);
        }
        self.show_window(context);
    }

    fn show_window(&mut self, context: &egui::Context) {
        self.load_system_fonts(context);
        self.window_visible = true;
        context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        context.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        context.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn load_system_fonts(&mut self, context: &egui::Context) {
        if self.fonts.is_some() {
            return;
        }

        match SystemFonts::new(context) {
            Ok(fonts) => {
                self.fonts = Some(fonts);
                self.font_texts_dirty = true;
            }
            Err(error) => self.show_error(format!("Font fallback failed: {error}")),
        }
    }

    fn unload_system_fonts(&mut self, context: &egui::Context) {
        if self.fonts.take().is_none() {
            return;
        }

        context.set_fonts(egui::FontDefinitions::default());
        self.font_texts_dirty = true;
    }

    pub(super) fn can_exit(&self) -> bool {
        exit_is_available(
            self.worker.is_busy(),
            self.application.is_busy(),
            self.application.state(),
        )
    }

    pub(super) fn exit_host(&mut self, context: &egui::Context) {
        if let Err(error) = Arc::clone(&self.application).request_exit() {
            self.show_error(error.to_string());
            self.show_window(context);
            return;
        }
        self.close_app(context);
    }

    fn close_app(&mut self, context: &egui::Context) {
        self.clear_lut_browse();
        self.exit_requested = true;
        context.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    pub(super) fn edit_config<T, E>(
        &mut self,
        edit: impl FnOnce(&mut ConfigDocument) -> Result<T, E>,
    ) -> Result<T, GuiError>
    where
        E: Into<GuiError>,
    {
        let editor = self
            .editor()
            .ok_or_else(|| GuiError::InvalidEdit("configuration is not loaded".to_string()))?;
        let path = editor.path.clone();
        let document = editor.document.clone();
        let (document, result) = edit_and_save_config(&path, document, edit)?;
        self.editor_mut()
            .expect("configuration remained loaded during synchronous edit")
            .document = document;
        self.font_texts_dirty = true;
        Ok(result)
    }

    fn editor(&self) -> Option<&ConfigEditor> {
        self.config_state.editor()
    }

    fn editor_mut(&mut self) -> Option<&mut ConfigEditor> {
        self.config_state.editor_mut()
    }

    pub(super) fn selected_profile(&self) -> &str {
        self.editor()
            .map(|editor| editor.selected_profile.as_str())
            .unwrap_or("")
    }

    pub(super) fn set_selected_profile(&mut self, profile: String) {
        if let Some(editor) = self.editor_mut() {
            editor.selected_profile = profile;
            self.font_texts_dirty = true;
        }
    }

    pub(super) fn assignment_path(
        &self,
        device_path: &str,
        color_mode: ConfigColorMode,
    ) -> Option<PathBuf> {
        self.config()?
            .profiles
            .get(self.selected_profile())?
            .assignments
            .iter()
            .find(|assignment| {
                assignment
                    .monitor_device_path
                    .eq_ignore_ascii_case(device_path)
                    && assignment.color_mode == color_mode
            })
            .map(|assignment| assignment.lut_path.clone())
    }

    pub(super) fn set_assignment(
        &mut self,
        device_path: &str,
        color_mode: ConfigColorMode,
        path: PathBuf,
    ) {
        let selected_profile = self.selected_profile().to_string();
        if let Err(error) = self.edit_config(|config| {
            config.set_assignment(&selected_profile, device_path, color_mode, path)
        }) {
            self.show_error(error.to_string());
        }
    }

    pub(super) fn clear_assignment(&mut self, device_path: &str, color_mode: ConfigColorMode) {
        let selected_profile = self.selected_profile().to_string();
        if let Err(error) = self.edit_config(|config| {
            config.clear_assignment(&selected_profile, device_path, color_mode)
        }) {
            self.show_error(error.to_string());
        }
    }
    pub(super) fn open_profile_dialog(&mut self, dialog: ProfileDialog) {
        self.profile_dialog_generation += 1;
        self.profile_dialog_error = None;
        self.modal = Some(ModalState::Profile(dialog));
    }

    pub(super) fn open_delete_profile_dialog(&mut self, profile: String) {
        self.modal = Some(ModalState::DeleteProfile(profile));
    }

    pub(super) fn set_profile_dialog_error(&mut self, error: Option<String>) {
        if error
            .as_deref()
            .is_some_and(|error| self.profile_dialog_error.as_deref() != Some(error))
        {
            self.font_texts_dirty = true;
        }
        self.profile_dialog_error = error;
    }

    pub(super) fn profile_name_input_id(&self) -> u64 {
        self.profile_dialog_generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseDisposition {
    None,
    Hide,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenDisposition {
    FocusOnly,
    ReloadAndShow,
}

fn open_disposition(window_visible: bool) -> OpenDisposition {
    if window_visible {
        OpenDisposition::FocusOnly
    } else {
        OpenDisposition::ReloadAndShow
    }
}

fn close_disposition(close_requested: bool, exit_requested: bool) -> CloseDisposition {
    match (close_requested, exit_requested) {
        (false, _) => CloseDisposition::None,
        (true, false) => CloseDisposition::Hide,
        (true, true) => CloseDisposition::Exit,
    }
}

fn exit_is_available(worker_busy: bool, application_busy: bool, state: HostState) -> bool {
    !worker_busy && !application_busy && state == HostState::Running
}

pub(super) enum ProfileDialog {
    Add { value: String },
    Rename { original: String, value: String },
}

impl ProfileDialog {
    pub(super) fn title(&self) -> &'static str {
        match self {
            Self::Add { .. } => "Add Profile",
            Self::Rename { .. } => "Rename Profile",
        }
    }

    pub(super) fn value_mut(&mut self) -> &mut String {
        match self {
            Self::Add { value } | Self::Rename { value, .. } => value,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::config::{load_config_document, save_config_document};

    use super::*;

    fn temp_config_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "dwm-lut-gui-app-{name}-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn loading_config_selects_default_profile() {
        let path = temp_config_path("load-default");
        save_config_document(&path, &ConfigDocument::default()).unwrap();

        let state = ConfigState::load(path.clone());

        assert!(matches!(
            state,
            ConfigState::Ready(ConfigEditor {
                selected_profile,
                ..
            }) if selected_profile == "default"
        ));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reloading_config_for_show_reads_external_changes_and_preserves_selection() {
        let path = temp_config_path("reload-for-show");
        let mut initial = ConfigDocument::default();
        initial.add_profile("gaming").unwrap();
        save_config_document(&path, &initial).unwrap();
        let mut state = ConfigState::load_selecting(path.clone(), Some("gaming"));

        let mut changed = initial;
        changed.add_profile("external").unwrap();
        save_config_document(&path, &changed).unwrap();
        state = state.reload();

        assert!(matches!(
            state,
            ConfigState::Ready(ConfigEditor {
                document,
                selected_profile,
                ..
            }) if document.profiles.contains_key("external") && selected_profile == "gaming"
        ));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reloading_config_for_show_falls_back_when_selection_was_removed() {
        let path = temp_config_path("reload-selection-fallback");
        let mut initial = ConfigDocument::default();
        initial.add_profile("gaming").unwrap();
        save_config_document(&path, &initial).unwrap();
        let state = ConfigState::load_selecting(path.clone(), Some("gaming"));

        save_config_document(&path, &ConfigDocument::default()).unwrap();
        let state = state.reload();

        assert!(matches!(
            state,
            ConfigState::Ready(ConfigEditor {
                selected_profile,
                ..
            }) if selected_profile == "default"
        ));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn config_load_failures_enter_load_failed_state() {
        let invalid_path = temp_config_path("invalid-state");
        fs::write(&invalid_path, b"invalid").unwrap();
        let invalid_state = ConfigState::load(invalid_path.clone());

        assert!(matches!(invalid_state, ConfigState::LoadFailed { .. }));

        let unreadable_path = temp_config_path("unreadable-state");
        fs::create_dir(&unreadable_path).unwrap();
        let unreadable_state = ConfigState::load(unreadable_path.clone());

        assert!(matches!(unreadable_state, ConfigState::LoadFailed { .. }));

        fs::remove_file(invalid_path).unwrap();
        fs::remove_dir(unreadable_path).unwrap();
    }

    #[test]
    fn loading_missing_config_persists_default_config() {
        let path = temp_config_path("missing-default");

        let state = ConfigState::load(path.clone());

        assert!(matches!(
            state,
            ConfigState::Ready(ConfigEditor {
                selected_profile,
                ..
            }) if selected_profile == "default"
        ));
        assert_eq!(
            load_config_document(&path).unwrap(),
            ConfigDocument::default()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_edit_does_not_replace_saved_config() {
        let path = temp_config_path("failed-edit");
        let original = ConfigDocument::default();
        save_config_document(&path, &original).unwrap();

        let result = edit_and_save_config(&path, original.clone(), |config| {
            config.add_profile("new")?;
            Err::<(), _>(GuiError::InvalidEdit("rejected edit".to_string()))
        });

        assert!(result.is_err());
        assert_eq!(load_config_document(&path).unwrap(), original);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn window_close_hides_without_ending_event_loop() {
        assert_eq!(close_disposition(true, false), CloseDisposition::Hide);
    }

    #[test]
    fn explicit_exit_allows_event_loop_to_end() {
        assert_eq!(close_disposition(true, true), CloseDisposition::Exit);
    }

    #[test]
    fn opening_visible_window_only_focuses_existing_state() {
        assert_eq!(open_disposition(true), OpenDisposition::FocusOnly);
    }

    #[test]
    fn opening_hidden_window_reloads_external_state() {
        assert_eq!(open_disposition(false), OpenDisposition::ReloadAndShow);
    }

    #[test]
    fn pending_gui_worker_disables_exit() {
        assert!(!exit_is_available(true, false, HostState::Running));
        assert!(exit_is_available(false, false, HostState::Running));
    }
}
