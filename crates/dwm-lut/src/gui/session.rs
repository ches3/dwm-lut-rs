use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use i_slint_backend_winit::WinitWindowAccessor;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{
    CloseRequestResponse, ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel,
};

use crate::config::ConfigColorMode;
use crate::error::InjectorError;
use crate::host::{HostController, HostState};
use crate::monitor::{MonitorListing, list_monitor_listings};
use crate::platform;

use super::app::{
    self, ConfigState, DisplayMonitor, ErrorPresentation, GuiMutationState, LutBrowseRequest,
    LutBrowseState, ModalState, MonitorChangeListener, MonitorChangeSignal,
    MouseFocusDismissListener, MouseFocusDismissSignal, ProfileDialog, assignment_path,
    display_monitors, edit_and_save_config, exit_is_available, poll_lut_browse, profile_menu_label,
    start_lut_browse,
};
use super::error::GuiError;
use super::{
    DialogKind, HostTray, MainWindow, MonitorRow, TrayProfile, UiCommand, clear_ui_wake,
    install_ui_wake, schedule_ui_wake,
};

const MONITOR_LISTENER_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAIN_WINDOW_WIDTH: f32 = 800.0;
const MAIN_WINDOW_HEIGHT: f32 = 580.0;
const LOAD_ERROR_WINDOW_WIDTH: f32 = 600.0;
const LOAD_ERROR_WINDOW_HEIGHT: f32 = 300.0;

struct WindowState {
    ui: MainWindow,
    monitor_listener: Option<MonitorChangeListener>,
    mouse_focus_listener: Option<MouseFocusDismissListener>,
}

struct SharedSession {
    inner: RefCell<HostSession>,
    pending: RefCell<VecDeque<SessionAction>>,
    draining: Cell<bool>,
}

struct UiWakeRegistration;

impl Drop for UiWakeRegistration {
    fn drop(&mut self) {
        clear_ui_wake();
    }
}

enum SessionAction {
    OpenWindow,
    ApplyFromTray(String),
    DisableFromTray,
    RequestExit,
    DestroyWindow,
    ApplyFromGui,
    DisableFromGui,
    RetryConfig,
    ProfileSelected(String),
    AddProfile,
    RenameProfile,
    DeleteProfile,
    SetDefaultProfile,
    BrowseLut { device_path: String, hdr: bool },
    ClearLut { device_path: String, hdr: bool },
    DialogAccept,
    DialogCancel,
    RefreshMonitors,
}

struct HostSession {
    controller: Arc<HostController>,
    ui_commands: Receiver<UiCommand>,
    tray: HostTray,
    config_state: ConfigState,
    mutation_state: GuiMutationState,
    window: Option<WindowState>,
    monitors: Vec<MonitorListing>,
    monitor_error: Option<String>,
    monitor_changes: Arc<MonitorChangeSignal>,
    monitor_refresh: MonitorRefresh,
    lut_browse: LutBrowseState,
    modal: Option<ModalState>,
    exit_requested: bool,
    monitor_refresh_timer: Timer,
    shared: Option<Weak<SharedSession>>,
}

#[derive(Debug, Clone, Copy)]
enum MonitorRefresh {
    Idle,
    Scheduled { retry_after: bool },
}

impl SharedSession {
    fn enqueue(self: &Rc<Self>, action: SessionAction) {
        self.pending.borrow_mut().push_back(action);
        if self.draining.get() {
            return;
        }
        let shared = Rc::clone(self);
        Timer::single_shot(Duration::ZERO, move || {
            shared.drain();
        });
    }

    fn drain(self: &Rc<Self>) {
        if self.draining.replace(true) {
            return;
        }

        loop {
            while let Some(action) = self.pending.borrow_mut().pop_front() {
                self.inner.borrow_mut().handle_action(action);
            }

            {
                let mut inner = self.inner.borrow_mut();
                inner.poll_host_events();
                inner.poll_monitor_changes();
                inner.poll_file_dialog();
                inner.poll_mutation_result();
            }

            if self.pending.borrow().is_empty() {
                break;
            }
        }

        self.draining.set(false);
    }
}

fn enqueue_if_alive(shared: &Weak<SharedSession>, action: SessionAction) {
    if let Some(shared) = shared.upgrade() {
        shared.enqueue(action);
    }
}

pub(super) fn run(
    controller: Arc<HostController>,
    ui_commands: Receiver<UiCommand>,
    ready: Sender<()>,
) -> Result<(), InjectorError> {
    let tray = HostTray::new().map_err(|error| {
        InjectorError::HostStartupFailed(format!("tray initialization failed: {error}"))
    })?;
    tray.set_tray_tooltip(SharedString::from("dwm-lut"));
    tray.set_tray_visible(true);

    let wake = Arc::new(schedule_ui_wake);
    let monitor_changes = Arc::new(MonitorChangeSignal::new(wake));

    let shared = Rc::new(SharedSession {
        inner: RefCell::new(HostSession {
            controller,
            ui_commands,
            tray,
            config_state: ConfigState::load_default(),
            mutation_state: GuiMutationState::Idle,
            window: None,
            monitors: Vec::new(),
            monitor_error: None,
            monitor_changes,
            monitor_refresh: MonitorRefresh::Idle,
            lut_browse: LutBrowseState::Idle,
            modal: None,
            exit_requested: false,
            monitor_refresh_timer: Timer::default(),
            shared: None,
        }),
        pending: RefCell::new(VecDeque::new()),
        draining: Cell::new(false),
    });
    shared.inner.borrow_mut().shared = Some(Rc::downgrade(&shared));

    let shared_weak = Rc::downgrade(&shared);
    install_ui_wake(move || {
        if let Some(shared) = shared_weak.upgrade() {
            shared.drain();
        }
    });
    let _ui_wake_registration = UiWakeRegistration;

    wire_tray(&shared);

    shared.inner.borrow_mut().sync_tray_items();

    let ready_failed = Rc::new(Cell::new(false));
    let ready_failed_from_event_loop = Rc::clone(&ready_failed);
    Timer::single_shot(Duration::ZERO, move || {
        if ready.send(()).is_err() {
            ready_failed_from_event_loop.set(true);
            let _ = slint::quit_event_loop();
        }
    });

    let event_loop_result = slint::run_event_loop_until_quit()
        .map_err(|error| InjectorError::HostStartupFailed(format!("host UI failed: {error}")));
    if ready_failed.get() {
        Err(InjectorError::HostStartupFailed(
            "control server stopped before the host UI became ready".to_string(),
        ))
    } else {
        event_loop_result
    }
}

fn wire_tray(shared: &Rc<SharedSession>) {
    let tray = shared.inner.borrow().tray.clone_strong();

    {
        let shared = Rc::downgrade(shared);
        tray.on_open_window(move || {
            enqueue_if_alive(&shared, SessionAction::OpenWindow);
        });
    }
    {
        let shared = Rc::downgrade(shared);
        tray.on_apply_profile(move |profile: SharedString| {
            enqueue_if_alive(
                &shared,
                SessionAction::ApplyFromTray(profile.as_str().to_string()),
            );
        });
    }
    {
        let shared = Rc::downgrade(shared);
        tray.on_disable(move || {
            enqueue_if_alive(&shared, SessionAction::DisableFromTray);
        });
    }
    {
        let shared = Rc::downgrade(shared);
        tray.on_exit(move || {
            enqueue_if_alive(&shared, SessionAction::RequestExit);
        });
    }
}

impl HostSession {
    fn shared(&self) -> Option<Rc<SharedSession>> {
        self.shared.as_ref().and_then(Weak::upgrade)
    }

    fn handle_action(&mut self, action: SessionAction) {
        match action {
            SessionAction::OpenWindow => self.open_window(),
            SessionAction::ApplyFromTray(profile) => self.apply_from_tray(profile),
            SessionAction::DisableFromTray => self.disable_from_tray(),
            SessionAction::RequestExit => {
                if self.can_exit() {
                    self.stop_host();
                }
            }
            SessionAction::DestroyWindow => self.destroy_window(),
            SessionAction::ApplyFromGui => self.apply_from_gui(),
            SessionAction::DisableFromGui => self.disable_from_gui(),
            SessionAction::RetryConfig => self.retry_config(),
            SessionAction::ProfileSelected(profile) => self.set_selected_profile(profile),
            SessionAction::AddProfile => {
                self.open_profile_dialog(ProfileDialog::Add {
                    value: String::new(),
                });
            }
            SessionAction::RenameProfile => {
                let selected = self.selected_profile().to_string();
                if !selected.is_empty() {
                    self.open_profile_dialog(ProfileDialog::Rename {
                        original: selected.clone(),
                        value: selected,
                    });
                }
            }
            SessionAction::DeleteProfile => {
                let selected = self.selected_profile().to_string();
                if !selected.is_empty() {
                    self.open_delete_profile_dialog(selected);
                }
            }
            SessionAction::SetDefaultProfile => self.set_default_profile(),
            SessionAction::BrowseLut { device_path, hdr } => {
                let color_mode = if hdr {
                    ConfigColorMode::Hdr
                } else {
                    ConfigColorMode::Sdr
                };
                self.request_lut_browse(LutBrowseRequest {
                    device_path,
                    color_mode,
                });
            }
            SessionAction::ClearLut { device_path, hdr } => {
                let color_mode = if hdr {
                    ConfigColorMode::Hdr
                } else {
                    ConfigColorMode::Sdr
                };
                self.clear_assignment(&device_path, color_mode);
            }
            SessionAction::DialogAccept => self.accept_dialog(),
            SessionAction::DialogCancel => self.cancel_dialog(),
            SessionAction::RefreshMonitors => self.refresh_monitors_due(),
        }
    }

    fn poll_host_events(&mut self) {
        loop {
            match self.ui_commands.try_recv() {
                Ok(UiCommand::Show) => self.open_window(),
                Ok(UiCommand::HostStateChanged) => {
                    self.sync_tray_items();
                    self.refresh_window();
                }
                Ok(UiCommand::Exit) => self.close_app(),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.close_app();
                    break;
                }
            }
        }
    }

    fn can_exit(&self) -> bool {
        exit_is_available(
            self.mutation_state.is_awaiting_result(),
            self.controller.state(),
        )
    }

    fn sync_tray_items(&self) {
        let host_idle =
            !self.mutation_state.is_awaiting_result() && self.controller.state() == HostState::Idle;
        let profiles = if let Some(editor) = self.config_state.editor() {
            editor
                .document
                .profiles
                .keys()
                .map(|name: &String| TrayProfile {
                    name: SharedString::from(name.as_str()),
                    label: SharedString::from(profile_menu_label(
                        name,
                        name == &editor.document.default_profile,
                    )),
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let apply_enabled = host_idle && !profiles.is_empty();
        self.tray
            .set_apply_profiles(ModelRc::new(VecModel::from(profiles)));
        self.tray.set_apply_enabled(apply_enabled);
        self.tray.set_disable_enabled(host_idle);
        self.tray.set_exit_enabled(host_idle);
    }

    fn open_window(&mut self) {
        if let Some(window) = &self.window {
            let _ = window.ui.show();
            let _ = window.ui.window().with_winit_window(|winit_window| {
                winit_window.set_minimized(false);
                winit_window.focus_window();
            });
            return;
        }

        self.config_state = self.config_state.reload();
        self.modal = None;
        self.refresh_monitors();
        self.sync_tray_items();

        if let Err(error) = self.create_window() {
            platform::show_error(&error);
        }
    }

    fn create_window(&mut self) -> Result<(), String> {
        let ui = MainWindow::new().map_err(|error| format!("failed to create window: {error}"))?;
        self.wire_window(&ui)?;
        self.push_window_state(&ui);
        resize_window_for_config_state(&ui, self.config_state.load_error().is_some());
        ui.show()
            .map_err(|error| format!("failed to show window: {error}"))?;
        ui.window().set_minimized(false);

        let monitor_listener = attach_monitor_listener(&ui, &self.monitor_changes);
        let mouse_focus_listener = attach_mouse_focus_dismiss(&ui);
        let listener_missing = monitor_listener.is_none() || mouse_focus_listener.is_none();
        self.window = Some(WindowState {
            ui,
            monitor_listener,
            mouse_focus_listener,
        });
        if listener_missing && let Some(shared) = self.shared() {
            Timer::single_shot(MONITOR_LISTENER_RETRY_DELAY, move || {
                shared.drain();
            });
        }
        Ok(())
    }

    fn wire_window(&self, ui: &MainWindow) -> Result<(), String> {
        let Some(shared) = self.shared() else {
            return Err("host session is unavailable".to_string());
        };

        {
            let shared = Rc::downgrade(&shared);
            ui.window().on_close_requested(move || {
                let Some(shared) = shared.upgrade() else {
                    return CloseRequestResponse::HideWindow;
                };
                let exit_requested = shared
                    .inner
                    .try_borrow()
                    .map(|session| session.exit_requested)
                    .unwrap_or(false);
                if !exit_requested {
                    shared.enqueue(SessionAction::DestroyWindow);
                }
                CloseRequestResponse::HideWindow
            });
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_apply(move || enqueue_if_alive(&shared, SessionAction::ApplyFromGui));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_disable(move || enqueue_if_alive(&shared, SessionAction::DisableFromGui));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_reload_config(move || enqueue_if_alive(&shared, SessionAction::RetryConfig));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_exit(move || enqueue_if_alive(&shared, SessionAction::RequestExit));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_profile_selected(move |profile: SharedString| {
                enqueue_if_alive(
                    &shared,
                    SessionAction::ProfileSelected(profile.as_str().to_string()),
                );
            });
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_add_profile(move || enqueue_if_alive(&shared, SessionAction::AddProfile));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_rename_profile(move || enqueue_if_alive(&shared, SessionAction::RenameProfile));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_delete_profile(move || enqueue_if_alive(&shared, SessionAction::DeleteProfile));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_set_default_profile(move || {
                enqueue_if_alive(&shared, SessionAction::SetDefaultProfile);
            });
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_browse_lut(move |device_path: SharedString, hdr| {
                enqueue_if_alive(
                    &shared,
                    SessionAction::BrowseLut {
                        device_path: device_path.as_str().to_string(),
                        hdr,
                    },
                );
            });
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_clear_lut(move |device_path: SharedString, hdr| {
                enqueue_if_alive(
                    &shared,
                    SessionAction::ClearLut {
                        device_path: device_path.as_str().to_string(),
                        hdr,
                    },
                );
            });
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_dialog_accept(move || enqueue_if_alive(&shared, SessionAction::DialogAccept));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_dialog_cancel(move || enqueue_if_alive(&shared, SessionAction::DialogCancel));
        }
        Ok(())
    }

    fn destroy_window(&mut self) {
        self.modal = None;
        self.window = None;
    }

    fn close_app(&mut self) {
        self.exit_requested = true;
        self.destroy_window();
        let _ = slint::quit_event_loop();
    }

    fn stop_host(&mut self) {
        if let Err(error) = self.controller.stop() {
            self.report_mutation_error(error.to_string(), ErrorPresentation::Native);
            self.open_window();
        }
    }

    fn retry_config(&mut self) {
        self.config_state = self.config_state.retry();
        self.modal = None;
        self.sync_tray_items();
        if let Some(window) = &self.window {
            self.push_window_state(&window.ui);
            resize_window_for_config_state(&window.ui, self.config_state.load_error().is_some());
        }
    }

    fn selected_profile(&self) -> &str {
        self.config_state
            .editor()
            .map(|editor| editor.selected_profile.as_str())
            .unwrap_or("")
    }

    fn set_selected_profile(&mut self, profile: String) {
        if let Some(editor) = self.config_state.editor_mut() {
            editor.selected_profile = profile;
        }
        self.refresh_window();
    }

    fn set_default_profile(&mut self) {
        let selected = self.selected_profile().to_string();
        if let Err(error) = self.edit_config(|config| config.set_default_profile(&selected)) {
            self.show_gui_error(error.to_string());
        }
    }

    fn open_profile_dialog(&mut self, dialog: ProfileDialog) {
        self.modal = Some(ModalState::Profile(dialog));
        self.refresh_window();
    }

    fn open_delete_profile_dialog(&mut self, profile: String) {
        self.modal = Some(ModalState::DeleteProfile(profile));
        self.refresh_window();
    }

    fn cancel_dialog(&mut self) {
        self.modal = None;
        self.refresh_window();
    }

    fn accept_dialog(&mut self) {
        let Some(window) = self.window.as_ref().map(|window| window.ui.clone_strong()) else {
            return;
        };
        enum Accept {
            DismissError,
            Add,
            Rename(String),
            Delete(String),
        }
        let accept = match &self.modal {
            None => return,
            Some(ModalState::Error(_)) => Accept::DismissError,
            Some(ModalState::Profile(ProfileDialog::Add { .. })) => Accept::Add,
            Some(ModalState::Profile(ProfileDialog::Rename { original, .. })) => {
                Accept::Rename(original.clone())
            }
            Some(ModalState::DeleteProfile(profile)) => Accept::Delete(profile.clone()),
        };
        match accept {
            Accept::DismissError => {
                self.modal = None;
            }
            Accept::Add => {
                let value = window.get_dialog_input().to_string();
                match self.edit_config(|config| config.add_profile(&value)) {
                    Ok(name) => {
                        self.set_selected_profile(name);
                        self.modal = None;
                    }
                    Err(error) => {
                        window.set_dialog_error(SharedString::from(error.to_string()));
                        return;
                    }
                }
            }
            Accept::Rename(original) => {
                let value = window.get_dialog_input().to_string();
                match self.edit_config(|config| config.rename_profile(&original, &value)) {
                    Ok(name) => {
                        self.set_selected_profile(name);
                        self.modal = None;
                    }
                    Err(error) => {
                        window.set_dialog_error(SharedString::from(error.to_string()));
                        return;
                    }
                }
            }
            Accept::Delete(profile) => {
                match self.edit_config(|config| config.delete_profile(&profile)) {
                    Ok(next) => {
                        self.set_selected_profile(next);
                        self.modal = None;
                    }
                    Err(error) => {
                        self.show_gui_error(error.to_string());
                        return;
                    }
                }
            }
        }
        self.refresh_window();
    }

    fn show_gui_error(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.window.is_some() {
            self.modal = Some(ModalState::Error(message));
            self.refresh_window();
        } else {
            platform::show_error(&message);
        }
    }

    fn edit_config<T, E>(
        &mut self,
        edit: impl FnOnce(&mut crate::config::ConfigDocument) -> Result<T, E>,
    ) -> Result<T, GuiError>
    where
        E: Into<GuiError>,
    {
        let editor = self
            .config_state
            .editor()
            .ok_or_else(|| GuiError::InvalidEdit("configuration is not loaded".to_string()))?;
        let path = editor.path.clone();
        let document = editor.document.clone();
        let (document, result) = edit_and_save_config(&path, document, edit)?;
        self.config_state
            .editor_mut()
            .expect("configuration remained loaded during synchronous edit")
            .document = document;
        self.sync_tray_items();
        self.refresh_window();
        Ok(result)
    }

    fn clear_assignment(&mut self, device_path: &str, color_mode: ConfigColorMode) {
        let selected_profile = self.selected_profile().to_string();
        if let Err(error) = self.edit_config(|config| {
            config.clear_assignment(&selected_profile, device_path, color_mode)
        }) {
            self.show_gui_error(error.to_string());
        }
    }

    fn set_assignment(&mut self, device_path: &str, color_mode: ConfigColorMode, path: PathBuf) {
        let selected_profile = self.selected_profile().to_string();
        if let Err(error) = self.edit_config(|config| {
            config.set_assignment(&selected_profile, device_path, color_mode, path)
        }) {
            self.show_gui_error(error.to_string());
        }
    }

    fn request_lut_browse(&mut self, request: LutBrowseRequest) {
        if !matches!(self.lut_browse, LutBrowseState::Idle) {
            return;
        }
        let dialog = self.parented_lut_dialog();
        match start_lut_browse(request, dialog, schedule_ui_wake) {
            Ok(task) => {
                self.lut_browse = LutBrowseState::Running(task);
                self.refresh_window();
            }
            Err(error) => self.show_gui_error(error),
        }
    }

    fn parented_lut_dialog(&self) -> rfd::FileDialog {
        let dialog = new_lut_dialog();
        match self.window.as_ref() {
            Some(window) => window
                .ui
                .window()
                .with_winit_window(|winit_window| dialog.set_parent(winit_window))
                .unwrap_or_else(new_lut_dialog),
            None => dialog,
        }
    }

    fn poll_file_dialog(&mut self) {
        match poll_lut_browse(&mut self.lut_browse) {
            Ok(Some((request, Some(path)))) => {
                self.set_assignment(&request.device_path, request.color_mode, path);
            }
            Ok(Some((_, None))) => self.refresh_window(),
            Ok(None) => {}
            Err(error) => self.show_gui_error(error),
        }
    }

    fn apply_from_gui(&mut self) {
        let Some(editor) = self.config_state.editor() else {
            return;
        };
        let path = editor.path.clone();
        let profile = editor.selected_profile.clone();
        self.submit_apply(path, profile, ErrorPresentation::Gui);
    }

    fn disable_from_gui(&mut self) {
        self.submit_disable(ErrorPresentation::Gui);
    }

    fn apply_from_tray(&mut self, profile: String) {
        let Some(editor) = self.config_state.editor() else {
            let message = self
                .config_state
                .load_error()
                .map(ToString::to_string)
                .unwrap_or_else(|| "Configuration is not loaded.".to_string());
            platform::show_error(&message);
            return;
        };
        let path = editor.path.clone();
        self.submit_apply(path, profile, ErrorPresentation::Native);
    }

    fn disable_from_tray(&mut self) {
        self.submit_disable(ErrorPresentation::Native);
    }

    fn submit_apply(&mut self, path: PathBuf, profile: String, presentation: ErrorPresentation) {
        if self.mutation_state.is_awaiting_result() {
            return;
        }
        match self.controller.submit_apply(path, Some(profile)) {
            Ok(completion) => {
                self.mutation_state =
                    GuiMutationState::AwaitingApplyResult(completion, presentation);
                self.sync_tray_items();
                self.refresh_window();
            }
            Err(error) => self.report_mutation_error(error.to_string(), presentation),
        }
    }

    fn submit_disable(&mut self, presentation: ErrorPresentation) {
        if self.mutation_state.is_awaiting_result() {
            return;
        }
        match self.controller.submit_disable() {
            Ok(completion) => {
                self.mutation_state =
                    GuiMutationState::AwaitingDisableResult(completion, presentation);
                self.sync_tray_items();
                self.refresh_window();
            }
            Err(error) => self.report_mutation_error(error.to_string(), presentation),
        }
    }

    fn poll_mutation_result(&mut self) {
        if !self.mutation_state.is_awaiting_result() {
            return;
        }
        let Some((result, presentation)) = self.mutation_state.try_take_result() else {
            return;
        };
        self.mutation_state = GuiMutationState::Idle;
        self.sync_tray_items();
        if let Err(error) = result {
            self.report_mutation_error(error.to_string(), presentation);
        }
        self.refresh_window();
    }

    fn report_mutation_error(
        &mut self,
        message: impl Into<String>,
        presentation: ErrorPresentation,
    ) {
        match presentation {
            ErrorPresentation::Gui => self.show_gui_error(message),
            ErrorPresentation::Native => platform::show_error(&message.into()),
        }
    }

    fn poll_monitor_changes(&mut self) {
        if self.window.is_none() {
            let _ = self.monitor_changes.take();
            self.monitor_refresh = MonitorRefresh::Idle;
            self.monitor_refresh_timer.stop();
            return;
        }

        self.ensure_window_subclasses();

        if self.monitor_changes.take() {
            self.schedule_monitor_refresh(app::MONITOR_CHANGE_SETTLE_DELAY, true);
        }
    }

    fn ensure_window_subclasses(&mut self) {
        if let Some(window) = &mut self.window {
            if window.monitor_listener.is_none() {
                window.monitor_listener =
                    attach_monitor_listener(&window.ui, &self.monitor_changes);
            }
            if window.mouse_focus_listener.is_none() {
                window.mouse_focus_listener = attach_mouse_focus_dismiss(&window.ui);
            }
        }
    }

    fn schedule_monitor_refresh(&mut self, delay: Duration, retry_after: bool) {
        self.monitor_refresh = MonitorRefresh::Scheduled { retry_after };
        let Some(shared) = self.shared() else {
            return;
        };
        self.monitor_refresh_timer
            .start(TimerMode::SingleShot, delay, move || {
                shared.enqueue(SessionAction::RefreshMonitors);
            });
    }

    fn refresh_monitors_due(&mut self) {
        if self.window.is_none() {
            self.monitor_refresh = MonitorRefresh::Idle;
            return;
        }
        let MonitorRefresh::Scheduled { retry_after } = self.monitor_refresh else {
            return;
        };

        self.refresh_monitors();
        if retry_after {
            self.schedule_monitor_refresh(app::MONITOR_CHANGE_RETRY_DELAY, false);
        } else {
            self.monitor_refresh = MonitorRefresh::Idle;
        }
        self.refresh_window();
    }

    fn refresh_monitors(&mut self) {
        match list_monitor_listings() {
            Ok(monitors) => {
                self.monitors = monitors;
                self.monitor_error = None;
            }
            Err(error) => {
                self.monitor_error = Some(error.to_string());
            }
        }
    }

    fn refresh_window(&mut self) {
        if let Some(window) = &self.window {
            let ui = window.ui.clone_strong();
            self.push_window_state(&ui);
        }
    }

    fn push_window_state(&self, ui: &MainWindow) {
        let controls_disabled = self.mutation_state.is_awaiting_result()
            || self.controller.state() != HostState::Idle
            || self.modal.is_some()
            || matches!(self.lut_browse, LutBrowseState::Running(_));

        ui.set_controls_disabled(controls_disabled);
        ui.set_can_exit(self.can_exit());
        ui.set_mutation_status(SharedString::from(
            self.mutation_state.status_label().unwrap_or(""),
        ));
        ui.set_monitor_error(SharedString::from(
            self.monitor_error.clone().unwrap_or_default(),
        ));

        if let Some(error) = self.config_state.load_error() {
            ui.set_load_failed(true);
            ui.set_load_error(SharedString::from(error.to_string()));
            ui.set_profile_names(ModelRc::default());
            ui.set_selected_profile_index(0);
            ui.set_default_profile(SharedString::new());
            ui.set_monitors(ModelRc::default());
        } else if let Some(editor) = self.config_state.editor() {
            ui.set_load_failed(false);
            ui.set_load_error(SharedString::new());
            let names = editor
                .document
                .profiles
                .keys()
                .map(|name: &String| SharedString::from(name.as_str()))
                .collect::<Vec<_>>();
            let selected_profile_index = names
                .iter()
                .position(|name| name.as_str() == editor.selected_profile)
                .unwrap_or(0) as i32;
            ui.set_profile_names(ModelRc::new(VecModel::from(names)));
            ui.set_selected_profile_index(selected_profile_index);
            ui.set_default_profile(SharedString::from(editor.document.default_profile.as_str()));
            let rows = display_monitors(
                &self.monitors,
                Some(&editor.document),
                &editor.selected_profile,
            )
            .into_iter()
            .map(|row| to_monitor_row(&editor.document, &editor.selected_profile, row))
            .collect::<Vec<_>>();
            ui.set_monitors(ModelRc::new(VecModel::from(rows)));
        }

        match &self.modal {
            None => {
                ui.set_dialog_kind(DialogKind::None);
                ui.set_dialog_title(SharedString::new());
                ui.set_dialog_message(SharedString::new());
                ui.set_dialog_input(SharedString::new());
                ui.set_dialog_error(SharedString::new());
            }
            Some(ModalState::Error(message)) => {
                ui.set_dialog_kind(DialogKind::Error);
                ui.set_dialog_title(SharedString::from("Error"));
                ui.set_dialog_message(SharedString::from(message.as_str()));
                ui.set_dialog_input(SharedString::new());
                ui.set_dialog_error(SharedString::new());
            }
            Some(ModalState::Profile(dialog)) => {
                let kind = match dialog {
                    ProfileDialog::Add { .. } => DialogKind::AddProfile,
                    ProfileDialog::Rename { .. } => DialogKind::RenameProfile,
                };
                let already_open = ui.get_dialog_kind() == kind;
                ui.set_dialog_kind(kind);
                ui.set_dialog_title(SharedString::from(dialog.title()));
                ui.set_dialog_message(SharedString::new());
                if !already_open {
                    ui.set_dialog_input(SharedString::from(dialog.value()));
                    ui.set_dialog_error(SharedString::new());
                }
            }
            Some(ModalState::DeleteProfile(name)) => {
                ui.set_dialog_kind(DialogKind::DeleteProfile);
                ui.set_dialog_title(SharedString::from("Delete Profile"));
                ui.set_dialog_message(SharedString::from(format!(
                    "Are you sure you want to delete profile \"{name}\"?"
                )));
                ui.set_dialog_input(SharedString::new());
                ui.set_dialog_error(SharedString::new());
            }
        }
    }
}

fn to_monitor_row(
    config: &crate::config::ConfigDocument,
    selected_profile: &str,
    row: DisplayMonitor,
) -> MonitorRow {
    let sdr = assignment_path(
        config,
        selected_profile,
        &row.device_path,
        ConfigColorMode::Sdr,
    );
    let hdr = assignment_path(
        config,
        selected_profile,
        &row.device_path,
        ConfigColorMode::Hdr,
    );
    MonitorRow {
        device_path: SharedString::from(row.device_path),
        title: SharedString::from(row.title),
        connected: row.connected,
        sdr_path: SharedString::from(
            sdr.as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Not assigned".to_string()),
        ),
        hdr_path: SharedString::from(
            hdr.as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Not assigned".to_string()),
        ),
        has_sdr: sdr.is_some(),
        has_hdr: hdr.is_some(),
    }
}

fn new_lut_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new().add_filter("3D LUT", &["cube", "txt"])
}

fn attach_monitor_listener(
    ui: &MainWindow,
    signal: &Arc<MonitorChangeSignal>,
) -> Option<MonitorChangeListener> {
    window_hwnd(ui)
        .ok()
        .and_then(|hwnd| MonitorChangeListener::attach(hwnd, Arc::clone(signal)).ok())
}

fn attach_mouse_focus_dismiss(ui: &MainWindow) -> Option<MouseFocusDismissListener> {
    let ui_weak = ui.as_weak();
    let signal = Arc::new(MouseFocusDismissSignal::new(Arc::new(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.invoke_restore_default_focus();
        }
    })));
    window_hwnd(ui)
        .ok()
        .and_then(|hwnd| MouseFocusDismissListener::attach(hwnd, signal).ok())
}

fn resize_window_for_config_state(ui: &MainWindow, load_failed: bool) {
    let (width, height) = if load_failed {
        (LOAD_ERROR_WINDOW_WIDTH, LOAD_ERROR_WINDOW_HEIGHT)
    } else {
        (MAIN_WINDOW_WIDTH, MAIN_WINDOW_HEIGHT)
    };
    ui.window().set_size(slint::LogicalSize::new(width, height));
}

fn window_hwnd(ui: &MainWindow) -> Result<windows_sys::Win32::Foundation::HWND, String> {
    ui.window()
        .with_winit_window(|window| {
            let handle = window
                .window_handle()
                .map_err(|error| format!("get host window handle: {error}"))?;
            match handle.as_raw() {
                RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get() as _),
                _ => Err("host UI did not provide a Win32 window handle".to_string()),
            }
        })
        .ok_or_else(|| "host UI window is not backed by winit".to_string())?
}
