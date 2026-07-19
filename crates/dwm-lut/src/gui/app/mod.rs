mod config_editor;
mod file_dialog;
mod monitor_events;
mod mouse_focus;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::config::{ConfigAssignmentDocument, ConfigColorMode, ConfigDocument};
use crate::host::{HostCommandError, HostState, MutationCompletion};
use crate::inject::{ApplyReport, DisableReport};
use crate::monitor::MonitorListing;

pub(crate) use config_editor::{ConfigState, edit_and_save_config};
pub(crate) use file_dialog::{LutBrowseRequest, LutBrowseState, poll_lut_browse, start_lut_browse};
pub(crate) use monitor_events::{
    MonitorChangeListener, MonitorChangeSignal, RETRY_DELAY as MONITOR_CHANGE_RETRY_DELAY,
    SETTLE_DELAY as MONITOR_CHANGE_SETTLE_DELAY,
};
pub(crate) use mouse_focus::{MouseFocusDismissListener, MouseFocusDismissSignal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ErrorPresentation {
    Gui,
    Native,
}

pub(super) enum GuiMutationState {
    Idle,
    AwaitingApplyResult(MutationCompletion<ApplyReport>, ErrorPresentation),
    AwaitingDisableResult(MutationCompletion<DisableReport>, ErrorPresentation),
}

impl GuiMutationState {
    pub(super) fn is_awaiting_result(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    pub(super) fn status_label(&self) -> Option<&'static str> {
        match self {
            Self::Idle => None,
            Self::AwaitingApplyResult(_, _) => Some("Applying LUT configuration..."),
            Self::AwaitingDisableResult(_, _) => Some("Disabling LUT..."),
        }
    }

    pub(super) fn try_take_result(
        &mut self,
    ) -> Option<(Result<(), HostCommandError>, ErrorPresentation)> {
        match self {
            Self::Idle => None,
            Self::AwaitingApplyResult(completion, presentation) => completion
                .try_take()
                .map(|result| (result.map(|_| ()), *presentation)),
            Self::AwaitingDisableResult(completion, presentation) => completion
                .try_take()
                .map(|result| (result.map(|_| ()), *presentation)),
        }
    }
}

pub(super) fn exit_is_available(awaiting_mutation_result: bool, state: HostState) -> bool {
    !awaiting_mutation_result && state == HostState::Idle
}

pub(super) fn profile_menu_label(name: &str, is_default: bool) -> String {
    let name = name.replace('&', "&&");
    if is_default {
        format!("{name} (default)")
    } else {
        name
    }
}

pub(super) struct DisplayMonitor {
    pub(super) device_path: String,
    pub(super) title: String,
    pub(super) connected: bool,
}

pub(super) fn display_monitors(
    monitors: &[MonitorListing],
    config: Option<&ConfigDocument>,
    selected_profile: &str,
) -> Vec<DisplayMonitor> {
    let mut rows = monitors
        .iter()
        .map(|monitor| DisplayMonitor {
            device_path: monitor.monitor_device_path.clone(),
            title: monitor_title(monitor),
            connected: true,
        })
        .collect::<Vec<_>>();
    let connected = monitors
        .iter()
        .map(|monitor| monitor.monitor_device_path.to_ascii_uppercase())
        .collect::<BTreeSet<_>>();
    if let Some(profile) = config.and_then(|config| config.profiles.get(selected_profile)) {
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

pub(super) fn assignment_path(
    config: &ConfigDocument,
    selected_profile: &str,
    device_path: &str,
    color_mode: ConfigColorMode,
) -> Option<PathBuf> {
    config
        .profiles
        .get(selected_profile)?
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

    pub(super) fn value(&self) -> &str {
        match self {
            Self::Add { value } | Self::Rename { value, .. } => value,
        }
    }
}

pub(super) enum ModalState {
    Error(String),
    Profile(ProfileDialog),
    DeleteProfile(String),
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::config::{load_config_document, save_config_document};
    use crate::gui::app::config_editor::ConfigEditor;
    use crate::gui::error::GuiError;

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
    fn pending_or_active_host_mutation_disables_exit() {
        assert!(!exit_is_available(true, HostState::Idle));
        assert!(!exit_is_available(false, HostState::Mutating));
        assert!(!exit_is_available(false, HostState::Stopping));
        assert!(exit_is_available(false, HostState::Idle));
    }

    #[test]
    fn disconnected_mutation_completion_reports_executor_failure() {
        let mut state = GuiMutationState::AwaitingApplyResult(
            MutationCompletion::disconnected(),
            ErrorPresentation::Gui,
        );

        assert!(matches!(
            state.try_take_result(),
            Some((
                Err(HostCommandError::MutationExecutorStopped),
                ErrorPresentation::Gui
            ))
        ));
    }

    #[test]
    fn profile_label_escapes_windows_menu_mnemonics() {
        assert_eq!(
            profile_menu_label("SDR & HDR", true),
            "SDR && HDR (default)"
        );
        assert_eq!(profile_menu_label("SDR & HDR", false), "SDR && HDR");
    }
}
