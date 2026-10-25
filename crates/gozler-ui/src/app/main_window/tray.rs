use super::{
    Cell, Choices, CommandId, DEFERRED, Hwnd, Instance, MainWindow, MenuItem, RefCell,
    RegisterError, Registration, ShowShortcut, TrayEvent, commands, shortcut, win,
};

pub(in crate::app) struct Tray {
    pub(super) shortcut: RefCell<Option<Registration>>,
    pub(super) starting_shortcut: RefCell<Option<Registration>>,
    pub(super) failure: RefCell<Option<RegisterError>>,
    pub(super) unshown: Cell<bool>,
}

impl Tray {
    pub(super) fn new() -> Self {
        Self {
            shortcut: RefCell::new(None),
            starting_shortcut: RefCell::new(None),
            failure: RefCell::new(None),
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
                let show = MenuItem::Run(CommandId::ShowWindow);
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
            TrayEvent::Show | TrayEvent::Menu { .. } => window.show_window(hwnd),
        }
    }

    /// Registers the saved shortcut to show Gozler. One that Windows
    /// refuses is said the first time the window shows, because a message
    /// at sign-in would come before the person looks.
    pub(super) fn register_saved_shortcut(&self, window: &MainWindow, hwnd: Hwnd) {
        let Some(saved) = window.model.borrow().choices().show_shortcut else {
            self.update_windows_shortcut(window);
            return;
        };
        match shortcut::register(
            hwnd,
            saved,
            window.show_message,
            window.created().waker.clone(),
        ) {
            Ok(registration) => {
                self.starting_shortcut.replace(Some(registration));
                self.finish_shortcut_start(window, hwnd);
            }
            Err(error) => {
                self.failure.replace(Some(error));
                self.update_windows_shortcut(window);
            }
        }
    }

    pub(super) fn finish_shortcut_start(&self, window: &MainWindow, hwnd: Hwnd) {
        let Some(mut registration) = self.starting_shortcut.take() else {
            return;
        };
        match registration.poll() {
            None => {
                self.starting_shortcut.replace(Some(registration));
            }
            Some(Ok(())) => {
                registration.activate();
                self.shortcut.replace(Some(registration));
                self.update_windows_shortcut(window);
            }
            Some(Err(error)) => {
                self.failure.replace(Some(error));
                self.update_windows_shortcut(window);
                hwnd.post(DEFERRED);
            }
        }
    }

    /// What the window says when Windows refused `shortcut`: in Settings,
    /// or at start, when the saved shortcut no longer works.
    pub(in crate::app) fn shortcut_message(
        &self,
        window: &MainWindow,
        shortcut: ShowShortcut,
        error: &RegisterError,
        at_start: bool,
    ) -> String {
        let model = window.model.borrow();
        let strings = model.strings();
        let keys = shortcut.text(strings);
        match error {
            RegisterError::Taken if at_start => strings.format("message.shortcut_taken", &[&keys]),
            RegisterError::Taken => strings.format("settings_keyboard.shortcut_taken", &[&keys]),
            RegisterError::Failed(error) => strings.format(
                "settings_keyboard.shortcut_failed",
                &[&keys, error.message().trim()],
            ),
        }
    }

    /// The shortcut that shows Gozler from any program, while Windows lets
    /// Gozler use it.
    pub(in crate::app) fn registered_shortcut(&self) -> Option<ShowShortcut> {
        self.shortcut.borrow().as_ref().map(Registration::shortcut)
    }

    pub(in crate::app) fn update_windows_shortcut(&self, window: &MainWindow) {
        let key = self
            .registered_shortcut()
            .and_then(ShowShortcut::explorer_key);
        window
            .created()
            .work
            .borrow_mut()
            .settings
            .set_windows_shortcut(key, &window.settings_file);
    }

    /// Says why the saved shortcut does not work, once the window shows.
    pub(super) fn show_shortcut_failure(&self, window: &MainWindow, hwnd: Hwnd) {
        if !hwnd.is_visible() {
            return;
        }
        let Some(error) = self.failure.take() else {
            return;
        };
        let saved = window.model.borrow().choices().show_shortcut;
        if let Some(saved) = saved {
            let text = self.shortcut_message(window, saved, &error, true);
            window.error_box(hwnd, &text);
        }
    }

    pub(in crate::app) fn prepare_shortcut(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        edited: &Choices,
        last: &Choices,
    ) -> Result<Option<Registration>, String> {
        // Another window saves the shortcut for the first window's next
        // start, because the first window holds it.
        if window.instance() != Instance::First || edited.show_shortcut == last.show_shortcut {
            return Ok(None);
        }
        edited
            .show_shortcut
            .map(|shortcut| {
                shortcut::register(
                    hwnd,
                    shortcut,
                    window.show_message,
                    window.created().waker.clone(),
                )
                .map_err(|error| self.shortcut_message(window, shortcut, &error, false))
            })
            .transpose()
    }
}
