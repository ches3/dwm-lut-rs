use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use i_slint_backend_winit::WinitWindowAccessor;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{
    CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel,
};

use crate::config::ConfigColorMode;
use crate::host::HostProcessError;
use crate::host::{HostController, HostState};
use crate::monitor::{MonitorListing, list_monitor_listings};
use crate::platform;

use super::app::{
    self, ConfigState, DisplayMonitor, ErrorPresentation, GuiMutationState, LutBrowseRequest,
    LutBrowseState, MonitorChangeListener, MonitorChangeSignal, MouseFocusDismissListener,
    MouseFocusDismissSignal, ProfileNameEdit, ViewState, assignment_path, display_monitors,
    edit_and_save_config, escape_menu_ampersands, exit_is_available, hook_status_label,
    poll_lut_browse, profile_menu_label, start_lut_browse,
};
use super::error::GuiError;
use super::{
    HostSettings, HostTray, MainWindow, MonitorRow, ProfileNameEditKind, TrayProfile, UiCommand,
    clear_ui_wake, install_ui_wake, schedule_ui_wake,
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
    ApplyOnStart,
    RetryConfig,
    ProfileSelected(String),
    AddProfile,
    RenameProfile(String),
    DeleteProfile(String),
    SetDefaultProfile(String),
    OpenManageProfiles,
    CloseManageProfiles,
    OpenSettings,
    CloseSettings,
    SettingsChanged(HostSettings),
    BrowseLut { device_path: String, hdr: bool },
    ClearLut { device_path: String, hdr: bool },
    ErrorDismiss,
    DismissTop,
    DeleteConfirm,
    DeleteCancel,
    ProfileNameEditCommit,
    ProfileNameEditCancel,
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
    view: ViewState,
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
) -> Result<(), HostProcessError> {
    let tray = HostTray::new().map_err(|error| {
        HostProcessError::StartupFailed(format!("tray initialization failed: {error}"))
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
            view: ViewState::default(),
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
    shared.drain();

    let ready_failed = Rc::new(Cell::new(false));
    let ready_failed_from_event_loop = Rc::clone(&ready_failed);
    let shared_for_ready = Rc::clone(&shared);
    Timer::single_shot(Duration::ZERO, move || {
        if signal_ready_then_queue_apply_on_start(&ready, || {
            shared_for_ready.enqueue(SessionAction::ApplyOnStart);
        })
        .is_err()
        {
            ready_failed_from_event_loop.set(true);
            let _ = slint::quit_event_loop();
        }
    });

    let event_loop_result = slint::run_event_loop_until_quit()
        .map_err(|error| HostProcessError::StartupFailed(format!("UI event loop failed: {error}")));
    if ready_failed.get() {
        Err(HostProcessError::StartupFailed(
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
            SessionAction::ApplyOnStart => self.maybe_apply_on_start(),
            SessionAction::RetryConfig => self.retry_config(),
            SessionAction::ProfileSelected(profile) => self.set_selected_profile(profile),
            SessionAction::AddProfile => {
                self.open_profile_name_edit(ProfileNameEdit::Add);
            }
            SessionAction::RenameProfile(profile) => {
                if !profile.is_empty() {
                    self.open_profile_name_edit(ProfileNameEdit::Rename { original: profile });
                }
            }
            SessionAction::DeleteProfile(profile) => {
                if !profile.is_empty() {
                    self.open_delete_dialog(profile);
                }
            }
            SessionAction::SetDefaultProfile(profile) => self.set_default_profile(profile),
            SessionAction::OpenManageProfiles => self.open_manage_profiles(),
            SessionAction::CloseManageProfiles => self.close_manage_profiles(),
            SessionAction::OpenSettings => self.open_settings(),
            SessionAction::CloseSettings => self.close_settings(),
            SessionAction::SettingsChanged(settings) => self.settings_changed(settings),
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
            SessionAction::ErrorDismiss => self.dismiss_error(),
            SessionAction::DismissTop => self.dismiss_top(),
            SessionAction::DeleteConfirm => self.confirm_delete_profile(),
            SessionAction::DeleteCancel => self.cancel_manage_profiles_mode(),
            SessionAction::ProfileNameEditCommit => self.commit_profile_name_edit(),
            SessionAction::ProfileNameEditCancel => self.cancel_manage_profiles_mode(),
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
                Ok(UiCommand::HookStatusChanged { loss_revision }) => {
                    self.sync_tray_items();
                    self.refresh_window();
                    if loss_revision
                        .is_some_and(|revision| self.controller.should_report_hook_loss(revision))
                    {
                        platform::show_error(
                            "The DWM LUT hook is no longer active. The DWM process may have restarted, or the hook DLL was unloaded.",
                        );
                    }
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
        let hook_status = self.controller.hook_status();
        self.tray
            .set_hook_status(SharedString::from(escape_menu_ampersands(
                &hook_status_label(&hook_status),
            )));
        self.tray.set_disable_available(hook_status.can_disable());
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
        self.view.clear();
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
            ui.on_rename_profile(move |profile: SharedString| {
                enqueue_if_alive(
                    &shared,
                    SessionAction::RenameProfile(profile.as_str().to_string()),
                );
            });
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_delete_profile(move |profile: SharedString| {
                enqueue_if_alive(
                    &shared,
                    SessionAction::DeleteProfile(profile.as_str().to_string()),
                );
            });
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_set_default_profile(move |profile: SharedString| {
                enqueue_if_alive(
                    &shared,
                    SessionAction::SetDefaultProfile(profile.as_str().to_string()),
                );
            });
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_open_manage_profiles(move || {
                enqueue_if_alive(&shared, SessionAction::OpenManageProfiles)
            });
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_close_manage_profiles(move || {
                enqueue_if_alive(&shared, SessionAction::CloseManageProfiles)
            });
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_open_settings(move || enqueue_if_alive(&shared, SessionAction::OpenSettings));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_close_settings(move || enqueue_if_alive(&shared, SessionAction::CloseSettings));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_settings_changed(move |settings| {
                enqueue_if_alive(&shared, SessionAction::SettingsChanged(settings))
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
            ui.on_error_dismiss(move || enqueue_if_alive(&shared, SessionAction::ErrorDismiss));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_dismiss_top(move || enqueue_if_alive(&shared, SessionAction::DismissTop));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_delete_confirm(move || enqueue_if_alive(&shared, SessionAction::DeleteConfirm));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_delete_cancel(move || enqueue_if_alive(&shared, SessionAction::DeleteCancel));
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_profile_name_edit_commit(move || {
                enqueue_if_alive(&shared, SessionAction::ProfileNameEditCommit)
            });
        }
        {
            let shared = Rc::downgrade(&shared);
            ui.on_profile_name_edit_cancel(move || {
                enqueue_if_alive(&shared, SessionAction::ProfileNameEditCancel)
            });
        }
        Ok(())
    }

    fn destroy_window(&mut self) {
        self.view.clear();
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
        self.view.clear();
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
        self.select_profile(profile);
        self.refresh_window();
    }

    fn select_profile(&mut self, profile: String) {
        if let Some(editor) = self.config_state.editor_mut() {
            editor.selected_profile = profile;
        }
    }

    fn set_default_profile(&mut self, profile: String) {
        if profile.is_empty() {
            return;
        }
        let result = self.edit_profiles(|config| config.set_default_profile(&profile));
        self.refresh_window();
        if let Err(error) = result {
            self.show_gui_error(error.to_string());
        }
    }

    fn open_manage_profiles(&mut self) {
        if self.config_state.editor().is_none() {
            return;
        }
        self.view.open_manage_profiles();
        self.refresh_view();
    }

    fn close_manage_profiles(&mut self) {
        self.view.close_manage_profiles();
        self.refresh_view();
    }

    fn open_settings(&mut self) {
        if self.config_state.editor().is_none() {
            return;
        }
        self.view.open_settings();
        self.refresh_view();
        if let Some(window) = self.window.as_ref() {
            let ui = window.ui.clone_strong();
            ui.set_settings_focus_nonce(ui.get_settings_focus_nonce().wrapping_add(1));
        }
    }

    fn close_settings(&mut self) {
        self.view.close_settings();
        self.refresh_view();
    }

    fn settings_changed(&mut self, settings: HostSettings) {
        let result = self.persist_config(|config| {
            config.apply_on_start = settings.apply_on_start;
            config.flip_gate_enabled = settings.flip_gate_enabled;
            Ok::<(), GuiError>(())
        });
        self.refresh_window();
        if let Err(error) = result {
            self.show_gui_error(error.to_string());
        }
    }

    fn maybe_apply_on_start(&mut self) {
        let Some(editor) = self.config_state.editor() else {
            return;
        };
        let Some(request) = apply_on_start_request(
            editor.document.apply_on_start,
            &editor.path,
            &editor.document.default_profile,
        ) else {
            return;
        };
        self.submit_apply(request.path, request.profile, ErrorPresentation::Native);
    }

    fn open_profile_name_edit(&mut self, edit: ProfileNameEdit) {
        self.view.open_profile_name_edit(edit);
        self.refresh_view();
    }

    fn open_delete_dialog(&mut self, profile: String) {
        self.view.open_delete_dialog(profile);
        self.refresh_view();
    }

    fn dismiss_top(&mut self) {
        if self.view.dismiss_top() {
            self.refresh_view();
        }
    }

    fn dismiss_error(&mut self) {
        self.view.dismiss_error();
        self.refresh_view();
    }

    fn cancel_manage_profiles_mode(&mut self) {
        self.view.clear_manage_profiles_mode();
        self.refresh_view();
    }

    fn confirm_delete_profile(&mut self) {
        let Some(profile) = self.view.delete_dialog_profile().map(str::to_owned) else {
            return;
        };
        match self.edit_profiles(|config| config.delete_profile(&profile)) {
            Ok(next) => {
                let selected_was_deleted = profile_names_equal(self.selected_profile(), &profile);
                self.view.clear_manage_profiles_mode();
                if selected_was_deleted {
                    self.select_profile(next);
                }
                self.refresh_window();
            }
            Err(error) => {
                self.show_gui_error(error.to_string());
            }
        }
    }

    fn commit_profile_name_edit(&mut self) {
        let Some(window) = self.window.as_ref().map(|window| window.ui.clone_strong()) else {
            return;
        };
        enum Accept {
            Add,
            Rename(String),
        }
        let accept = match self.view.profile_name_edit() {
            Some(ProfileNameEdit::Add) => Accept::Add,
            Some(ProfileNameEdit::Rename { original }) => Accept::Rename(original.clone()),
            None => return,
        };
        match accept {
            Accept::Add => {
                let value = window.get_profile_name_edit_input().to_string();
                if should_cancel_empty_profile_add(&value) {
                    self.view.clear_manage_profiles_mode();
                } else {
                    match self.edit_profiles(|config| config.add_profile(&value)) {
                        Ok(name) => {
                            self.view.clear_manage_profiles_mode();
                            self.select_profile(name);
                        }
                        Err(error) => {
                            window
                                .set_profile_name_edit_error(SharedString::from(error.to_string()));
                            restore_profile_name_edit_focus(&window);
                            return;
                        }
                    }
                }
            }
            Accept::Rename(original) => {
                let value = window.get_profile_name_edit_input().to_string();
                match self.edit_profiles(|config| config.rename_profile(&original, &value)) {
                    Ok(name) => {
                        let selected_was_renamed =
                            profile_names_equal(self.selected_profile(), &original);
                        self.view.clear_manage_profiles_mode();
                        if selected_was_renamed {
                            self.select_profile(name);
                        }
                    }
                    Err(error) => {
                        window.set_profile_name_edit_error(SharedString::from(error.to_string()));
                        restore_profile_name_edit_focus(&window);
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
            self.view.show_error(message);
            self.refresh_view();
        } else {
            platform::show_error(&message);
        }
    }

    fn edit_profiles<T, E>(
        &mut self,
        edit: impl FnOnce(&mut crate::config::ConfigDocument) -> Result<T, E>,
    ) -> Result<T, GuiError>
    where
        E: Into<GuiError>,
    {
        let result = self.persist_config(edit)?;
        self.sync_tray_items();
        Ok(result)
    }

    fn persist_config<T, E>(
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
        Ok(result)
    }

    fn clear_assignment(&mut self, device_path: &str, color_mode: ConfigColorMode) {
        let selected_profile = self.selected_profile().to_string();
        if let Err(error) = self.edit_profiles(|config| {
            config.clear_assignment(&selected_profile, device_path, color_mode)
        }) {
            self.show_gui_error(error.to_string());
            return;
        }
        self.refresh_window();
    }

    fn set_assignment(&mut self, device_path: &str, color_mode: ConfigColorMode, path: PathBuf) {
        let selected_profile = self.selected_profile().to_string();
        if let Err(error) = self.edit_profiles(|config| {
            config.set_assignment(&selected_profile, device_path, color_mode, path)
        }) {
            self.show_gui_error(error.to_string());
            return;
        }
        self.refresh_window();
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
        let Some(result) = self.mutation_state.try_take_result() else {
            return;
        };
        self.mutation_state = GuiMutationState::Idle;
        let (result, presentation) = result;
        if let Err(error) = result {
            self.report_mutation_error(error.to_string(), presentation);
        }
        self.sync_tray_items();
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

    fn refresh_view(&mut self) {
        if let Some(window) = &self.window {
            let ui = window.ui.clone_strong();
            self.push_session_chrome(&ui);
        }
    }

    fn push_window_state(&self, ui: &MainWindow) {
        self.push_session_chrome(ui);
        ui.set_can_exit(self.can_exit());
        ui.set_mutation_busy(self.mutation_state.is_awaiting_result());
        let hook_status = self.controller.hook_status();
        ui.set_hook_status(SharedString::from(hook_status_label(&hook_status)));
        ui.set_disable_available(hook_status.can_disable());
        ui.set_monitor_error(SharedString::from(
            self.monitor_error.clone().unwrap_or_default(),
        ));
        self.push_document_models(ui);
    }

    fn push_session_chrome(&self, ui: &MainWindow) {
        let session_busy = self.mutation_state.is_awaiting_result()
            || self.controller.state() != HostState::Idle
            || matches!(self.lut_browse, LutBrowseState::Running(_));
        let controls_disabled = session_busy || self.view.blocks_session_controls();
        let main_controls_disabled = controls_disabled || self.view.covers_main();

        ui.set_controls_disabled(controls_disabled);
        ui.set_main_controls_disabled(main_controls_disabled);
        ui.set_suppress_mouse_focus_dismiss(self.view.suppresses_mouse_focus_dismiss());
        push_view_to_ui(&self.view, ui);
    }

    fn push_document_models(&self, ui: &MainWindow) {
        if let Some(error) = self.config_state.load_error() {
            ui.set_load_failed(true);
            ui.set_load_error(SharedString::from(error.to_string()));
            ui.set_profile_names(ModelRc::default());
            ui.set_selected_profile_index(0);
            ui.set_default_profile_index(0);
            ui.set_apply_on_start(false);
            ui.set_flip_gate_enabled(true);
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
            let default_profile_index = names
                .iter()
                .position(|name| name.as_str() == editor.document.default_profile)
                .unwrap_or(0) as i32;
            if !string_model_matches(&ui.get_profile_names(), &names) {
                ui.set_profile_names(ModelRc::new(VecModel::from(names)));
            }
            ui.set_selected_profile_index(selected_profile_index);
            ui.set_default_profile_index(default_profile_index);
            ui.set_apply_on_start(editor.document.apply_on_start);
            ui.set_flip_gate_enabled(editor.document.flip_gate_enabled);
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
    }
}

fn string_model_matches(model: &ModelRc<SharedString>, names: &[SharedString]) -> bool {
    model.row_count() == names.len()
        && names
            .iter()
            .enumerate()
            .all(|(index, name)| model.row_data(index).as_ref() == Some(name))
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

fn push_view_to_ui(view: &ViewState, ui: &MainWindow) {
    ui.set_manage_profiles_open(view.manage_profiles_open());
    ui.set_settings_open(view.settings_open());

    if let Some(edit) = view.profile_name_edit() {
        let (kind, original, initial_input) = match edit {
            ProfileNameEdit::Add => (
                ProfileNameEditKind::Add,
                SharedString::new(),
                SharedString::new(),
            ),
            ProfileNameEdit::Rename { original } => (
                ProfileNameEditKind::Rename,
                SharedString::from(original.as_str()),
                SharedString::from(original.as_str()),
            ),
        };
        let already_open = ui.get_profile_name_edit_kind() == kind
            && ui.get_profile_name_edit_original() == original;
        ui.set_profile_name_edit_kind(kind);
        ui.set_profile_name_edit_original(original);
        if !already_open {
            ui.set_profile_name_edit_input(initial_input);
            ui.set_profile_name_edit_error(SharedString::new());
        }
    } else {
        clear_profile_name_edit_props(ui);
    }

    ui.set_delete_dialog_profile(
        view.delete_dialog_profile()
            .map(SharedString::from)
            .unwrap_or_default(),
    );
    ui.set_error_message(view.error().map(SharedString::from).unwrap_or_default());
}

fn clear_profile_name_edit_props(ui: &MainWindow) {
    ui.set_profile_name_edit_kind(ProfileNameEditKind::None);
    ui.set_profile_name_edit_original(SharedString::new());
    ui.set_profile_name_edit_input(SharedString::new());
    ui.set_profile_name_edit_error(SharedString::new());
}

fn restore_profile_name_edit_focus(window: &MainWindow) {
    let window = window.as_weak();
    Timer::single_shot(Duration::ZERO, move || {
        let Some(window) = window.upgrade() else {
            return;
        };
        if window.get_profile_name_edit_kind() == ProfileNameEditKind::None {
            return;
        }
        window.set_profile_name_edit_focus_nonce(window.get_profile_name_edit_focus_nonce() + 1);
    });
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
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if ui.get_suppress_mouse_focus_dismiss() {
            return;
        }
        ui.invoke_restore_default_focus();
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

fn signal_ready_then_queue_apply_on_start(
    ready: &Sender<()>,
    queue_apply_on_start: impl FnOnce(),
) -> Result<(), ()> {
    ready.send(()).map_err(|_| ())?;
    queue_apply_on_start();
    Ok(())
}

struct ApplyOnStartRequest {
    path: PathBuf,
    profile: String,
}

fn apply_on_start_request(
    apply_on_start: bool,
    path: &Path,
    default_profile: &str,
) -> Option<ApplyOnStartRequest> {
    if !apply_on_start {
        return None;
    }
    Some(ApplyOnStartRequest {
        path: path.to_path_buf(),
        profile: default_profile.to_string(),
    })
}

fn should_cancel_empty_profile_add(input: &str) -> bool {
    input.trim().is_empty()
}

fn profile_names_equal(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::{
        profile_names_equal, should_cancel_empty_profile_add,
        signal_ready_then_queue_apply_on_start,
    };

    #[test]
    fn ready_notification_is_observable_before_apply_on_start_is_queued() {
        let (ready_sender, ready_receiver) = mpsc::channel();
        let mut apply_queued = false;

        signal_ready_then_queue_apply_on_start(&ready_sender, || {
            assert!(
                ready_receiver.try_recv().is_ok(),
                "ready must be observable before apply-on-start is queued"
            );
            apply_queued = true;
        })
        .expect("ready send should succeed");

        assert!(apply_queued);
    }

    #[test]
    fn apply_on_start_is_not_queued_when_ready_send_fails() {
        let (ready_sender, ready_receiver) = mpsc::channel();
        drop(ready_receiver);
        let mut apply_queued = false;

        let result = signal_ready_then_queue_apply_on_start(&ready_sender, || {
            apply_queued = true;
        });

        assert!(result.is_err());
        assert!(!apply_queued);
    }

    #[test]
    fn empty_profile_add_is_cancelled() {
        assert!(should_cancel_empty_profile_add(""));
        assert!(should_cancel_empty_profile_add("   "));
        assert!(should_cancel_empty_profile_add("\t\n"));
        assert!(!should_cancel_empty_profile_add("gaming"));
        assert!(!should_cancel_empty_profile_add("  gaming  "));
    }

    #[test]
    fn profile_names_equal_matches_case_insensitively() {
        assert!(profile_names_equal("Cinema", "cinema"));
        assert!(profile_names_equal("Default", "Default"));
        assert!(!profile_names_equal("Cinema", "Gaming"));
        assert!(!profile_names_equal("Default", "default-backup"));
    }
}
