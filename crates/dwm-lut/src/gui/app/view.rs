#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProfileNameEdit {
    Add,
    Rename { original: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManageProfilesMode {
    ProfileNameEdit(ProfileNameEdit),
    DeleteDialog(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum View {
    #[default]
    Main,
    ManageProfiles(Option<ManageProfilesMode>),
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ViewState {
    view: View,
    error: Option<String>,
}

impl ViewState {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn open_manage_profiles(&mut self) {
        self.view = View::ManageProfiles(None);
    }

    pub(crate) fn close_manage_profiles(&mut self) {
        self.view = View::Main;
    }

    pub(crate) fn open_settings(&mut self) {
        self.view = View::Settings;
    }

    pub(crate) fn close_settings(&mut self) {
        self.view = View::Main;
    }

    pub(crate) fn open_profile_name_edit(&mut self, edit: ProfileNameEdit) {
        let View::ManageProfiles(layer) = &mut self.view else {
            return;
        };
        *layer = Some(ManageProfilesMode::ProfileNameEdit(edit));
        self.error = None;
    }

    pub(crate) fn open_delete_dialog(&mut self, profile: String) {
        let View::ManageProfiles(layer) = &mut self.view else {
            return;
        };
        *layer = Some(ManageProfilesMode::DeleteDialog(profile));
        self.error = None;
    }

    pub(crate) fn dismiss_top(&mut self) -> bool {
        if self.error.take().is_some() {
            return true;
        }
        match &mut self.view {
            View::ManageProfiles(layer) => {
                if layer.take().is_some() {
                    return true;
                }
                self.view = View::Main;
                true
            }
            View::Settings => {
                self.view = View::Main;
                true
            }
            View::Main => false,
        }
    }

    pub(crate) fn dismiss_error(&mut self) {
        self.error = None;
    }

    pub(crate) fn clear_manage_profiles_mode(&mut self) {
        if let View::ManageProfiles(layer) = &mut self.view {
            *layer = None;
        }
    }

    pub(crate) fn show_error(&mut self, message: String) {
        self.error = Some(message);
    }

    pub(crate) fn manage_profiles_open(&self) -> bool {
        matches!(self.view, View::ManageProfiles(_))
    }

    pub(crate) fn settings_open(&self) -> bool {
        matches!(self.view, View::Settings)
    }

    pub(crate) fn profile_name_edit(&self) -> Option<&ProfileNameEdit> {
        match &self.view {
            View::ManageProfiles(Some(ManageProfilesMode::ProfileNameEdit(edit))) => Some(edit),
            _ => None,
        }
    }

    pub(crate) fn delete_dialog_profile(&self) -> Option<&str> {
        match &self.view {
            View::ManageProfiles(Some(ManageProfilesMode::DeleteDialog(name))) => {
                Some(name.as_str())
            }
            _ => None,
        }
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(crate) fn blocks_session_controls(&self) -> bool {
        self.error.is_some()
            || matches!(
                self.view,
                View::ManageProfiles(Some(ManageProfilesMode::DeleteDialog(_)))
            )
    }

    pub(crate) fn covers_main(&self) -> bool {
        matches!(self.view, View::ManageProfiles(_) | View::Settings)
    }

    pub(crate) fn suppresses_mouse_focus_dismiss(&self) -> bool {
        self.error.is_some()
            || matches!(
                self.view,
                View::ManageProfiles(Some(
                    ManageProfilesMode::ProfileNameEdit(_) | ManageProfilesMode::DeleteDialog(_)
                ))
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manage_and_settings_are_exclusive() {
        let mut state = ViewState::default();
        state.open_manage_profiles();
        state.open_profile_name_edit(ProfileNameEdit::Add);
        assert!(state.profile_name_edit().is_some());

        state.open_settings();
        assert!(state.settings_open());
        assert!(!state.manage_profiles_open());
        assert!(state.profile_name_edit().is_none());

        state.open_manage_profiles();
        assert!(state.manage_profiles_open());
        assert!(!state.settings_open());
    }

    #[test]
    fn delete_dialog_only_attaches_to_manage_profiles() {
        let mut state = ViewState::default();
        state.open_delete_dialog("gaming".into());
        assert!(state.delete_dialog_profile().is_none());

        state.open_manage_profiles();
        state.open_delete_dialog("gaming".into());
        assert_eq!(state.delete_dialog_profile(), Some("gaming"));

        state.close_manage_profiles();
        assert!(state.delete_dialog_profile().is_none());
        assert!(!state.manage_profiles_open());
    }

    #[test]
    fn profile_name_edit_only_attaches_to_manage_profiles() {
        let mut state = ViewState::default();
        state.open_profile_name_edit(ProfileNameEdit::Add);
        assert!(state.profile_name_edit().is_none());

        state.open_manage_profiles();
        state.open_profile_name_edit(ProfileNameEdit::Add);
        assert!(state.profile_name_edit().is_some());
    }

    #[test]
    fn error_stacks_over_manage_profiles_mode() {
        let mut state = ViewState::default();
        state.open_manage_profiles();
        state.open_profile_name_edit(ProfileNameEdit::Rename {
            original: "a".into(),
        });
        state.show_error("boom".into());

        assert!(state.manage_profiles_open());
        assert!(state.profile_name_edit().is_some());
        assert_eq!(state.error(), Some("boom"));
        assert!(state.blocks_session_controls());
        assert!(state.covers_main());
        assert!(state.suppresses_mouse_focus_dismiss());
    }

    #[test]
    fn profile_name_edit_does_not_block_session_controls() {
        let mut state = ViewState::default();
        state.open_manage_profiles();
        state.open_profile_name_edit(ProfileNameEdit::Add);
        assert!(!state.blocks_session_controls());
        assert!(state.covers_main());
        assert!(state.suppresses_mouse_focus_dismiss());
    }

    #[test]
    fn delete_dialog_blocks_session_controls() {
        let mut state = ViewState::default();
        state.open_manage_profiles();
        state.open_delete_dialog("default".into());
        assert!(state.blocks_session_controls());
        assert!(state.suppresses_mouse_focus_dismiss());
    }

    #[test]
    fn covers_main_for_manage_and_settings() {
        let mut state = ViewState::default();
        assert!(!state.covers_main());

        state.open_manage_profiles();
        assert!(state.covers_main());

        state.open_settings();
        assert!(state.covers_main());

        state.close_settings();
        assert!(!state.covers_main());
    }

    #[test]
    fn manage_without_child_does_not_suppress_mouse_focus_dismiss() {
        let mut state = ViewState::default();
        state.open_manage_profiles();
        assert!(!state.suppresses_mouse_focus_dismiss());
    }

    #[test]
    fn dismiss_top_dismisses_error_only() {
        let mut state = ViewState::default();
        state.show_error("boom".into());

        assert!(state.dismiss_top());
        assert!(state.error().is_none());
    }

    #[test]
    fn dismiss_top_dismisses_manage_profiles_mode_only() {
        let mut state = ViewState::default();
        state.open_manage_profiles();
        state.open_delete_dialog("default".into());

        assert!(state.dismiss_top());
        assert!(state.delete_dialog_profile().is_none());
        assert!(state.manage_profiles_open());
    }

    #[test]
    fn dismiss_top_prefers_error() {
        let mut state = ViewState::default();
        state.open_manage_profiles();
        state.open_delete_dialog("default".into());
        state.show_error("boom".into());

        assert!(state.dismiss_top());
        assert!(state.error().is_none());
        assert_eq!(state.delete_dialog_profile(), Some("default"));
    }

    #[test]
    fn dismiss_top_closes_manage_profiles() {
        let mut state = ViewState::default();
        state.open_manage_profiles();

        assert!(state.dismiss_top());
        assert!(!state.manage_profiles_open());
    }

    #[test]
    fn dismiss_top_closes_settings() {
        let mut state = ViewState::default();
        state.open_settings();

        assert!(state.dismiss_top());
        assert!(!state.settings_open());
    }

    #[test]
    fn dismiss_top_noop_on_main() {
        let mut state = ViewState::default();
        assert!(!state.dismiss_top());
    }

    #[test]
    fn open_delete_dialog_clears_error() {
        let mut state = ViewState::default();
        state.open_manage_profiles();
        state.show_error("boom".into());
        state.open_delete_dialog("default".into());

        assert!(state.error().is_none());
        assert_eq!(state.delete_dialog_profile(), Some("default"));
    }

    #[test]
    fn dismiss_error_clears_error_only() {
        let mut state = ViewState::default();
        state.show_error("boom".into());

        state.dismiss_error();
        assert!(state.error().is_none());
    }

    #[test]
    fn dismiss_error_keeps_manage_profiles_mode() {
        let mut state = ViewState::default();
        state.open_manage_profiles();
        state.open_delete_dialog("default".into());
        state.show_error("boom".into());

        state.dismiss_error();
        assert!(state.error().is_none());
        assert_eq!(state.delete_dialog_profile(), Some("default"));
        assert!(state.manage_profiles_open());
    }

    #[test]
    fn clear_manage_profiles_mode_clears_layer_only() {
        let mut state = ViewState::default();
        state.open_manage_profiles();
        state.open_delete_dialog("default".into());

        state.clear_manage_profiles_mode();
        assert!(state.delete_dialog_profile().is_none());
        assert!(state.manage_profiles_open());
    }

    #[test]
    fn clear_manage_profiles_mode_keeps_error() {
        let mut state = ViewState::default();
        state.open_manage_profiles();
        state.open_delete_dialog("default".into());
        state.show_error("boom".into());

        state.clear_manage_profiles_mode();
        assert_eq!(state.error(), Some("boom"));
        assert!(state.delete_dialog_profile().is_none());
        assert!(state.manage_profiles_open());
    }
}
