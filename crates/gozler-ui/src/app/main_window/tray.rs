use super::{
    Cell, CommandId, Hwnd, Instance, MainWindow, MenuItem, Mode, TrayEvent, commands, win,
};

pub(in crate::app) struct Tray {
    pub(super) unshown: Cell<bool>,
}

impl Tray {
    pub(super) fn new() -> Self {
        Self {
            unshown: Cell::new(false),
        }
    }
    /// Adds the tray icon while Gozler keeps running in the notification
    /// area, and removes it otherwise; another window has none (DESIGN.md:
    /// Window lifetime).
    pub(super) fn update_tray(&self, window: &MainWindow, _hwnd: Hwnd) {
        let keeps_running =
            window.instance() == Instance::First && window.model.borrow().choices().keep_running;
        let tip = window
            .model
            .borrow()
            .strings()
            .text("window.title")
            .to_owned();
        if let Some(tray) = &mut window.created().work.borrow_mut().tray {
            tray.update(keeps_running, &tip);
        }
    }

    /// A click on the tray icon, or its menu. While a dialog is open, the
    /// menu shows the window and its dialog instead, because Exit would end
    /// the window under the dialog.
    pub(super) fn on_tray(&self, window: &MainWindow, hwnd: Hwnd, event: TrayEvent) {
        match event {
            TrayEvent::Menu { point } if hwnd.is_enabled() => {
                let nodes =
                    commands::render_popup(&commands::tray_menu(), window.model.borrow().strings());
                let show = MenuItem::Run(CommandId::ShowSearch);
                let chosen = win::menu::Menu::track_popup_in_front(
                    hwnd,
                    &nodes,
                    point,
                    &[],
                    Some(show.id()),
                );
                if let Some(item) = chosen.and_then(MenuItem::from_id) {
                    window.run(hwnd, item);
                }
            }
            TrayEvent::Show | TrayEvent::Menu { .. } => window.show_mode(hwnd, Mode::Search),
        }
    }
}
