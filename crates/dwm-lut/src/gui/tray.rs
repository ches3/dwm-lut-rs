use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::error::InjectorError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TrayAction {
    Open,
    Apply(String),
    Disable,
    Exit,
}

pub(super) struct TrayState {
    _icon: TrayIcon,
    apply_submenu: Submenu,
    apply_profile_ids: Arc<Mutex<HashMap<MenuId, String>>>,
    disable_item: MenuItem,
    exit_item: MenuItem,
    actions: Receiver<TrayAction>,
}

impl TrayState {
    pub(super) fn new(
        context: &egui::Context,
        icon_data: &egui::IconData,
    ) -> Result<Self, InjectorError> {
        let tray_icon =
            tray_icon::Icon::from_rgba(icon_data.rgba.clone(), icon_data.width, icon_data.height)
                .map_err(|error| {
                InjectorError::HostStartupFailed(format!("tray icon creation failed: {error}"))
            })?;

        let open_item = MenuItem::new("Open", true, None);
        let separator_top = PredefinedMenuItem::separator();
        let apply_submenu = Submenu::new("Apply", false);
        let disable_item = MenuItem::new("Disable", false, None);
        let separator_bottom = PredefinedMenuItem::separator();
        let exit_item = MenuItem::new("Exit", true, None);
        let menu = Menu::with_items(&[
            &open_item,
            &separator_top,
            &apply_submenu,
            &disable_item,
            &separator_bottom,
            &exit_item,
        ])
        .map_err(|error| {
            InjectorError::HostStartupFailed(format!("tray menu creation failed: {error}"))
        })?;

        let icon = TrayIconBuilder::new()
            .with_tooltip("dwm-lut")
            .with_icon(tray_icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|error| {
                InjectorError::HostStartupFailed(format!("tray initialization failed: {error}"))
            })?;

        let (sender, actions) = mpsc::channel();
        let tray_sender = sender.clone();
        let tray_context = context.clone();
        TrayIconEvent::set_event_handler(Some(move |event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = tray_sender.send(TrayAction::Open);
                tray_context.request_repaint();
            }
        }));

        let open_id = open_item.id().clone();
        let disable_id = disable_item.id().clone();
        let exit_id = exit_item.id().clone();
        let apply_profile_ids = Arc::new(Mutex::new(HashMap::new()));
        let menu_profile_ids = Arc::clone(&apply_profile_ids);
        let menu_context = context.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == open_id {
                Some(TrayAction::Open)
            } else if event.id == disable_id {
                Some(TrayAction::Disable)
            } else if event.id == exit_id {
                Some(TrayAction::Exit)
            } else {
                menu_profile_ids
                    .lock()
                    .ok()
                    .and_then(|profiles| profiles.get(&event.id).cloned())
                    .map(TrayAction::Apply)
            };
            if let Some(action) = action {
                let _ = sender.send(action);
                menu_context.request_repaint();
            }
        }));

        Ok(Self {
            _icon: icon,
            apply_submenu,
            apply_profile_ids,
            disable_item,
            exit_item,
            actions,
        })
    }

    pub(super) fn poll(&self) -> Option<TrayAction> {
        self.actions.try_recv().ok()
    }

    pub(super) fn refresh_apply_profiles<'a>(
        &self,
        profiles: impl IntoIterator<Item = &'a str>,
        default_profile: &str,
    ) {
        while self.apply_submenu.remove_at(0).is_some() {}

        let mut profile_ids = match self.apply_profile_ids.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        profile_ids.clear();

        for name in profiles {
            let label = profile_menu_label(name, name == default_profile);
            let item = MenuItem::new(label, true, None);
            profile_ids.insert(item.id().clone(), name.to_string());
            let _ = self.apply_submenu.append(&item);
        }
    }

    pub(super) fn set_apply_enabled(&self, enabled: bool) {
        if self.apply_submenu.is_enabled() != enabled {
            self.apply_submenu.set_enabled(enabled);
        }
    }

    pub(super) fn set_disable_enabled(&self, enabled: bool) {
        if self.disable_item.is_enabled() != enabled {
            self.disable_item.set_enabled(enabled);
        }
    }

    pub(super) fn set_exit_enabled(&self, enabled: bool) {
        if self.exit_item.is_enabled() != enabled {
            self.exit_item.set_enabled(enabled);
        }
    }
}

impl Drop for TrayState {
    fn drop(&mut self) {
        TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
    }
}

fn profile_menu_label(name: &str, is_default: bool) -> String {
    let name = name.replace('&', "&&");
    if is_default {
        format!("{name} (default)")
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::profile_menu_label;

    #[test]
    fn profile_label_escapes_windows_menu_mnemonics() {
        assert_eq!(profile_menu_label("SDR & HDR", false), "SDR && HDR");
    }

    #[test]
    fn default_profile_label_marks_escaped_name() {
        assert_eq!(
            profile_menu_label("SDR & HDR", true),
            "SDR && HDR (default)"
        );
    }
}
