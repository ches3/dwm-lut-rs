use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::error::InjectorError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TrayAction {
    Open,
    Exit,
}

pub(super) struct TrayState {
    _icon: TrayIcon,
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
        let separator = PredefinedMenuItem::separator();
        let exit_item = MenuItem::new("Exit", true, None);
        let menu = Menu::with_items(&[&open_item, &separator, &exit_item]).map_err(|error| {
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
        let exit_id = exit_item.id().clone();
        let menu_context = context.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == open_id {
                Some(TrayAction::Open)
            } else if event.id == exit_id {
                Some(TrayAction::Exit)
            } else {
                None
            };
            if let Some(action) = action {
                let _ = sender.send(action);
                menu_context.request_repaint();
            }
        }));

        Ok(Self {
            _icon: icon,
            exit_item,
            actions,
        })
    }

    pub(super) fn poll(&self) -> Option<TrayAction> {
        self.actions.try_recv().ok()
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
