use std::collections::{BTreeMap, BTreeSet};

use eframe::egui;

use super::{
    DwmLutApp, LutBrowseRequest, ModalState, ProfileDialog, resize_viewport_for_config_state,
};
use crate::config::ConfigAssignmentDocument;
use crate::gui::ConfigColorMode;
use crate::gui::worker::Operation;
use crate::monitor::MonitorListing;

fn panel_frame(style: &egui::Style, inner_margin: egui::Margin) -> egui::Frame {
    egui::Frame::new()
        .inner_margin(inner_margin)
        .fill(style.visuals.panel_fill)
}

fn modal_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::popup(style).inner_margin(egui::Margin::symmetric(16, 12))
}

fn modal(id: egui::Id, context: &egui::Context) -> egui::Modal {
    egui::Modal::new(id)
        .backdrop_color(egui::Color32::from_black_alpha(180))
        .frame(modal_frame(&context.style_of(context.theme())))
}

fn input_event_text(event: &egui::Event) -> Option<&str> {
    match event {
        egui::Event::Paste(text) | egui::Event::Text(text) => Some(text),
        egui::Event::Ime(egui::ImeEvent::Preedit { text, .. })
        | egui::Event::Ime(egui::ImeEvent::Commit(text)) => Some(text),
        _ => None,
    }
}

impl eframe::App for DwmLutApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.window_fill().to_normalized_gamma_f32()
    }

    fn raw_input_hook(&mut self, context: &egui::Context, raw_input: &mut egui::RawInput) {
        let texts = raw_input.events.iter().filter_map(input_event_text);
        if let Err(error) = self.prepare_input_fonts(context, texts) {
            self.show_error(format!("Font fallback failed: {error}"));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        self.poll_worker();
        self.poll_host_events(&context);
        self.poll_lut_browse();
        self.poll_monitor_changes(&context);
        self.handle_close_request(&context);
        if let Err(error) = self.refresh_system_fonts(&context) {
            self.show_error(format!("Font fallback failed: {error}"));
        }
        let ui_blocked = self.ui_blocked();
        let panel_margin = egui::Margin::symmetric(12, 10);

        if self.config_load_error().is_some() {
            self.show_load_failed_panel(ui, &context, ui_blocked, egui::Margin::symmetric(20, 16));
            self.show_error_dialog(&context);
            return;
        }

        self.show_footer(ui, &context, ui_blocked, panel_margin);
        self.show_main_panel(ui, ui_blocked, panel_margin);
        self.show_profile_dialog(&context);
        self.show_delete_confirm_dialog(&context);
        self.show_error_dialog(&context);
        self.start_pending_lut_browse(frame, &context);
    }
}

impl DwmLutApp {
    fn show_footer(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        ui_blocked: bool,
        panel_margin: egui::Margin,
    ) {
        egui::Panel::bottom("footer")
            .show_separator_line(false)
            .frame(panel_frame(ui.style(), panel_margin))
            .show(ui, |ui| {
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!ui_blocked, egui::Button::new("Apply"))
                        .clicked()
                    {
                        self.apply(context);
                    }
                    if ui
                        .add_enabled(!ui_blocked, egui::Button::new("Disable"))
                        .clicked()
                    {
                        self.worker.spawn(Operation::Disable, context.clone());
                    }
                    if let Some(label) = self.worker.pending_label() {
                        ui.spinner();
                        ui.label(label);
                    }
                });
            });
    }

    fn show_main_panel(&mut self, ui: &mut egui::Ui, ui_blocked: bool, panel_margin: egui::Margin) {
        egui::CentralPanel::default()
            .frame(panel_frame(ui.style(), panel_margin))
            .show(ui, |ui| {
                self.show_profile_controls(ui, ui_blocked);
                ui.add_space(4.0);
                self.show_monitor_list(ui, ui_blocked);
            });
    }

    fn show_load_failed_panel(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        ui_blocked: bool,
        panel_margin: egui::Margin,
    ) {
        let error = self
            .config_load_error()
            .expect("load failure panel requires a config error")
            .to_string();

        egui::CentralPanel::default()
            .frame(panel_frame(ui.style(), panel_margin))
            .show(ui, |ui| {
                ui.heading("Failed to load configuration");
                ui.add_space(8.0);
                ui.label(error);
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!ui_blocked, egui::Button::new("Reload"))
                        .clicked()
                    {
                        self.retry_config();
                        if self.config().is_some() {
                            resize_viewport_for_config_state(context, false);
                        }
                    }
                    if ui
                        .add_enabled(self.can_exit(), egui::Button::new("Exit"))
                        .clicked()
                    {
                        self.exit_host(context);
                    }
                    if let Some(label) = self.worker.pending_label() {
                        ui.spinner();
                        ui.label(label);
                    }
                });
            });
    }

    fn show_profile_controls(&mut self, ui: &mut egui::Ui, ui_blocked: bool) {
        let config = self
            .config()
            .expect("profile controls require a loaded config");
        let profile_names = config.profiles.keys().cloned().collect::<Vec<_>>();
        let default_profile = config.default_profile.clone();
        let selected_profile = self.selected_profile().to_string();
        ui.add_enabled_ui(!ui_blocked, |ui| {
            ui.horizontal(|ui| {
                ui.label("Profile");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("profile")
                        .selected_text(&selected_profile)
                        .show_ui(ui, |ui| {
                            for name in &profile_names {
                                if ui
                                    .selectable_label(selected_profile == *name, name)
                                    .clicked()
                                {
                                    self.set_selected_profile(name.clone());
                                }
                            }
                        });
                    if selected_profile == default_profile {
                        ui.label("(default)");
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            selected_profile != default_profile,
                            egui::Button::new("Set Default"),
                        )
                        .clicked()
                    {
                        let selected_profile = selected_profile.clone();
                        if let Err(error) =
                            self.edit_config(|config| config.set_default_profile(&selected_profile))
                        {
                            self.show_error(error.to_string());
                        }
                    }
                    if ui.button("Delete").clicked() {
                        self.open_delete_profile_dialog(selected_profile.clone());
                    }
                    if ui.button("Rename").clicked() {
                        self.open_profile_dialog(ProfileDialog::Rename {
                            original: selected_profile.clone(),
                            value: selected_profile.clone(),
                        });
                    }
                    if ui.button("Add").clicked() {
                        self.open_profile_dialog(ProfileDialog::Add {
                            value: String::new(),
                        });
                    }
                });
            });
        });
    }

    fn show_monitor_list(&mut self, ui: &mut egui::Ui, ui_blocked: bool) {
        if let Some(error) = &self.monitor_error {
            ui.colored_label(
                egui::Color32::RED,
                format!("Failed to refresh monitors: {error}"),
            );
            ui.add_space(4.0);
        }
        egui::ScrollArea::vertical()
            .id_salt("monitors")
            .auto_shrink([false, true])
            .max_height(ui.available_height())
            .show(ui, |ui| {
                ui.add_enabled_ui(!ui_blocked, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    let rows = self.display_monitors();
                    if rows.is_empty() {
                        ui.label("No active monitors or saved assignments.");
                    }
                    for row in rows {
                        egui::Frame::group(ui.style())
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let mut title =
                                        egui::RichText::new(&row.title).size(14.0).strong();
                                    if !row.connected {
                                        let color =
                                            ui.visuals().weak_text_color().linear_multiply(0.7);
                                        title = title.color(color);
                                        ui.label(title);
                                        ui.label(egui::RichText::new("Disconnected").color(color));
                                    } else {
                                        ui.label(title);
                                    }
                                });
                                self.assignment_row(
                                    ui,
                                    &row.device_path,
                                    ConfigColorMode::Sdr,
                                    "SDR LUT",
                                );
                                self.assignment_row(
                                    ui,
                                    &row.device_path,
                                    ConfigColorMode::Hdr,
                                    "HDR LUT",
                                );
                            });
                    }
                });
            });
    }

    fn display_monitors(&self) -> Vec<DisplayMonitor> {
        let mut rows = self
            .monitors
            .iter()
            .map(|monitor| DisplayMonitor {
                device_path: monitor.monitor_device_path.clone(),
                title: monitor_title(monitor),
                connected: true,
            })
            .collect::<Vec<_>>();
        let connected = self
            .monitors
            .iter()
            .map(|monitor| monitor.monitor_device_path.to_ascii_uppercase())
            .collect::<BTreeSet<_>>();
        if let Some(profile) = self
            .config()
            .and_then(|config| config.profiles.get(self.selected_profile()))
        {
            rows.extend(
                disconnected_monitor_paths(&connected, &profile.assignments)
                    .into_iter()
                    .map(|device_path| DisplayMonitor {
                        title: disconnected_monitor_title(&device_path),
                        device_path,
                        connected: false,
                    }),
            );
        }
        rows
    }

    fn assignment_row(
        &mut self,
        ui: &mut egui::Ui,
        device_path: &str,
        color_mode: ConfigColorMode,
        label: &str,
    ) {
        let path = self.assignment_path(device_path, color_mode);
        let display = path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Not assigned".to_string());
        ui.horizontal(|ui| {
            ui.set_width(ui.available_width());
            ui.add_sized(
                [64.0, ui.spacing().interact_size.y],
                egui::Label::new(label),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(path.is_some(), egui::Button::new("Clear"))
                    .clicked()
                {
                    self.clear_assignment(device_path, color_mode);
                }
                if ui.button("Browse").clicked() {
                    self.request_lut_browse(LutBrowseRequest {
                        device_path: device_path.to_string(),
                        color_mode,
                    });
                }
                let path_width = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::vec2(path_width, ui.spacing().interact_size.y),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_width(path_width);
                        ui.add(egui::Label::new(egui::RichText::new(display).weak()).truncate());
                    },
                );
            });
        });
    }

    fn show_error_dialog(&mut self, context: &egui::Context) {
        let Some(error) = self.error_dialog.clone() else {
            return;
        };
        let mut dismiss = false;
        let modal = modal(egui::Id::new("error_dialog"), context).show(context, |ui| {
            ui.set_min_width(320.0);
            ui.heading("Error");
            ui.add_space(8.0);
            ui.label(&error);
            ui.add_space(12.0);
            if ui.button("OK").clicked() {
                dismiss = true;
            }
        });
        if modal.should_close() {
            dismiss = true;
        }
        if dismiss {
            self.dismiss_error();
        }
    }

    fn show_profile_dialog(&mut self, context: &egui::Context) {
        let mut dialog = match self.modal.take() {
            Some(ModalState::Profile(dialog)) => dialog,
            other => {
                self.modal = other;
                return;
            }
        };
        let mut keep_open = true;
        let modal = modal(egui::Id::new("profile_dialog"), context).show(context, |ui| {
            ui.set_min_width(280.0);
            ui.heading(dialog.title());
            ui.add_space(8.0);
            ui.label("Profile name");
            let response = ui.add(
                egui::TextEdit::singleline(dialog.value_mut())
                    .id_salt(self.profile_name_input_id())
                    .margin(egui::Margin::symmetric(8, 6)),
            );
            if !response.has_focus() {
                response.request_focus();
            }
            if let Some(error) = &self.profile_dialog_error {
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let submit = ui.button("OK").clicked()
                    || (response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                if submit {
                    let result = match &dialog {
                        ProfileDialog::Add { value } => {
                            self.edit_config(|config| config.add_profile(value))
                        }
                        ProfileDialog::Rename { original, value } => {
                            self.edit_config(|config| config.rename_profile(original, value))
                        }
                    };
                    match result {
                        Ok(name) => {
                            self.set_selected_profile(name);
                            keep_open = false;
                        }
                        Err(error) => {
                            self.set_profile_dialog_error(Some(error.to_string()));
                        }
                    }
                }
                if ui.button("Cancel").clicked() {
                    keep_open = false;
                }
            });
        });
        if modal.should_close() {
            keep_open = false;
        }
        if keep_open {
            self.modal = Some(ModalState::Profile(dialog));
        } else {
            self.set_profile_dialog_error(None);
        }
    }

    fn show_delete_confirm_dialog(&mut self, context: &egui::Context) {
        let profile_name = match self.modal.take() {
            Some(ModalState::DeleteProfile(profile)) => profile,
            other => {
                self.modal = other;
                return;
            }
        };
        let mut keep_open = true;
        let modal = modal(egui::Id::new("delete_profile_dialog"), context).show(context, |ui| {
            ui.set_min_width(280.0);
            ui.heading("Delete Profile");
            ui.add_space(8.0);
            ui.label(format!(
                "Are you sure you want to delete profile \"{profile_name}\"?"
            ));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    match self.edit_config(|config| config.delete_profile(&profile_name)) {
                        Ok(next) => {
                            self.set_selected_profile(next);
                            keep_open = false;
                        }
                        Err(error) => self.show_error(error.to_string()),
                    }
                }
                if ui.button("Cancel").clicked() {
                    keep_open = false;
                }
            });
        });
        if modal.should_close() {
            keep_open = false;
        }
        if keep_open {
            self.modal = Some(ModalState::DeleteProfile(profile_name));
        }
    }
}

struct DisplayMonitor {
    device_path: String,
    title: String,
    connected: bool,
}

fn disconnected_monitor_paths(
    connected: &BTreeSet<String>,
    assignments: &[ConfigAssignmentDocument],
) -> Vec<String> {
    let mut disconnected = BTreeMap::new();
    for assignment in assignments {
        let canonical_path = assignment.monitor_device_path.to_ascii_uppercase();
        if !connected.contains(&canonical_path) {
            disconnected
                .entry(canonical_path)
                .or_insert_with(|| assignment.monitor_device_path.clone());
        }
    }
    disconnected.into_values().collect()
}

fn disconnected_monitor_title(device_path: &str) -> String {
    crate::monitor::extract_edid_pnp_id(device_path)
        .unwrap_or(device_path)
        .to_string()
}

fn monitor_title(monitor: &MonitorListing) -> String {
    let name = if monitor.friendly_name.is_empty() {
        "Unknown monitor"
    } else {
        &monitor.friendly_name
    };
    format!(
        "#{} {} ({}, {}x{})",
        monitor.number,
        name,
        monitor.edid_pnp_id,
        monitor.resolution.width,
        monitor.resolution.height
    )
}
