//! Owns global shortcut registration, replacement and dispatch for both modes.
//! Prepared replacements leave current keys working until every new registration
//! is ready. Their saved destinations travel with them through commit.

use super::{
    Choices, CommandId, Hwnd, Instance, MainWindow, Mode, RefCell, RegisterError, Registration,
    ShowShortcut, Strings, shortcut,
};
use crate::core::shortcuts::command_place;

pub(in crate::app) struct Shortcuts {
    registry: RefCell<shortcut::Registry>,
    active: RefCell<Vec<(Mode, Registration)>>,
    starting: RefCell<Vec<(Mode, Registration)>>,
    failures: RefCell<Vec<Failure>>,
}

pub(in crate::app) enum Failure {
    Duplicate(ShowShortcut),
    Registration {
        mode: Mode,
        shortcut: ShowShortcut,
        error: RegisterError,
    },
}

impl Failure {
    pub(in crate::app) fn mode(&self) -> Mode {
        match self {
            Self::Duplicate(_) => Mode::Commander,
            Self::Registration { mode, .. } => *mode,
        }
    }

    pub(in crate::app) fn message(&self, strings: &Strings, at_start: bool) -> String {
        match self {
            Self::Duplicate(shortcut) => strings.format(
                "settings_keyboard.refused_taken",
                &[
                    &shortcut.text(strings),
                    &command_place(strings, CommandId::ShowSearch),
                ],
            ),
            Self::Registration {
                shortcut, error, ..
            } => {
                let keys = shortcut.text(strings);
                match error {
                    RegisterError::Taken if at_start => {
                        strings.format("message.shortcut_taken", &[&keys])
                    }
                    RegisterError::Taken => {
                        strings.format("settings_keyboard.shortcut_taken", &[&keys])
                    }
                    RegisterError::Failed(error) => strings.format(
                        "settings_keyboard.shortcut_failed",
                        &[&keys, error.message().trim()],
                    ),
                }
            }
        }
    }
}

pub(in crate::app) struct Prepared {
    registrations: Vec<(Mode, Registration)>,
    choices: [Option<ShowShortcut>; 2],
}

impl Prepared {
    pub(in crate::app) fn poll(&mut self) -> Option<Result<(), Failure>> {
        let mut ready = true;
        for (mode, registration) in &mut self.registrations {
            match registration.poll() {
                None => ready = false,
                Some(Ok(())) => {}
                Some(Err(error)) => {
                    return Some(Err(Failure::Registration {
                        mode: *mode,
                        shortcut: registration.shortcut(),
                        error,
                    }));
                }
            }
        }
        ready.then_some(Ok(()))
    }
}

impl Shortcuts {
    pub(super) fn new() -> Self {
        Self {
            registry: RefCell::new(shortcut::Registry::default()),
            active: RefCell::new(Vec::new()),
            starting: RefCell::new(Vec::new()),
            failures: RefCell::new(Vec::new()),
        }
    }
    /// Registers the saved shortcuts. One that Windows
    /// refuses is said the first time the window shows, because a message
    /// at sign-in would come before the person looks.
    pub(super) fn start(&self, window: &MainWindow, hwnd: Hwnd) {
        for mode in Mode::ALL {
            let saved = window.model.borrow().choices().show_shortcut(mode);
            if let Some(saved) = saved {
                if self
                    .starting
                    .borrow()
                    .iter()
                    .any(|(_, registration)| registration.shortcut() == saved)
                {
                    self.failures.borrow_mut().push(Failure::Duplicate(saved));
                    continue;
                }
                match self.registry.borrow_mut().register(
                    hwnd,
                    saved,
                    window.created().waker.clone(),
                ) {
                    Ok(registration) => self.starting.borrow_mut().push((mode, registration)),
                    Err(error) => self.failures.borrow_mut().push(Failure::Registration {
                        mode,
                        shortcut: saved,
                        error,
                    }),
                }
            }
        }
        if self.starting.borrow().is_empty() {
            self.update_windows_shortcut(window);
        } else {
            self.poll_start(window, hwnd);
        }
    }

    pub(super) fn poll_start(&self, window: &MainWindow, hwnd: Hwnd) {
        if self.starting.borrow().is_empty() {
            return;
        }
        for (mode, mut registration) in self.starting.take() {
            match registration.poll() {
                None => self.starting.borrow_mut().push((mode, registration)),
                Some(Ok(())) => {
                    registration.activate();
                    self.active.borrow_mut().push((mode, registration));
                }
                Some(Err(error)) => {
                    self.failures.borrow_mut().push(Failure::Registration {
                        mode,
                        shortcut: registration.shortcut(),
                        error,
                    });
                    hwnd.post_deferred();
                }
            }
        }
        if self.starting.borrow().is_empty() {
            self.update_windows_shortcut(window);
        }
    }

    pub(in crate::app) fn registered(&self, mode: Mode) -> Option<ShowShortcut> {
        self.active
            .borrow()
            .iter()
            .find(|(target, _)| *target == mode)
            .map(|(_, registration)| registration.shortcut())
    }

    pub(super) fn mode(&self, id: i32) -> Option<Mode> {
        self.active
            .borrow()
            .iter()
            .find(|(_, registration)| registration.id() == id)
            .map(|(mode, _)| *mode)
    }

    pub(in crate::app) fn update_windows_shortcut(&self, window: &MainWindow) {
        let mut keys: Vec<_> = Mode::ALL
            .into_iter()
            .filter_map(|mode| self.registered(mode).and_then(ShowShortcut::explorer_key))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        window
            .created()
            .work
            .borrow_mut()
            .settings
            .set_windows_shortcut(keys, &window.settings_file);
    }

    pub(super) fn has_failures(&self) -> bool {
        !self.failures.borrow().is_empty()
    }

    /// Says why a saved shortcut does not work, once the window shows.
    pub(super) fn show_failures(&self, window: &MainWindow, hwnd: Hwnd) {
        if !hwnd.is_visible() {
            return;
        }
        for failure in self.failures.take() {
            let text = failure.message(window.model.borrow().strings(), true);
            window.error_box(hwnd, &text);
        }
    }

    pub(in crate::app) fn prepare(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        edited: &Choices,
        last: &Choices,
    ) -> Result<Option<Prepared>, Failure> {
        if let Some(shortcut) = edited.commander_shortcut
            && edited.show_shortcut == Some(shortcut)
        {
            return Err(Failure::Duplicate(shortcut));
        }
        // Another window saves the shortcut for the first window's next
        // start, because the first window holds it.
        if window.instance() != Instance::First
            || Mode::ALL
                .into_iter()
                .all(|mode| edited.show_shortcut(mode) == last.show_shortcut(mode))
        {
            return Ok(None);
        }
        let mut registrations = Vec::new();
        for mode in Mode::ALL {
            if let Some(shortcut) = edited.show_shortcut(mode) {
                if self
                    .active
                    .borrow()
                    .iter()
                    .any(|(_, registration)| registration.shortcut() == shortcut)
                {
                    continue;
                }
                let registration = self
                    .registry
                    .borrow_mut()
                    .register(hwnd, shortcut, window.created().waker.clone())
                    .map_err(|error| Failure::Registration {
                        mode,
                        shortcut,
                        error,
                    })?;
                registrations.push((mode, registration));
            }
        }
        Ok(Some(Prepared {
            registrations,
            choices: Mode::ALL.map(|mode| edited.show_shortcut(mode)),
        }))
    }

    pub(in crate::app) fn commit(&self, prepared: Prepared) {
        self.starting.take();
        let mut available = self.active.take();
        available.extend(prepared.registrations);
        let mut registered = Vec::new();
        for (mode, shortcut) in Mode::ALL.into_iter().zip(prepared.choices) {
            if let Some(shortcut) = shortcut {
                let index = available
                    .iter()
                    .position(|(_, registration)| registration.shortcut() == shortcut)
                    .expect("prepare registered or retained every chosen shortcut");
                let (_, registration) = available.remove(index);
                registration.activate();
                registered.push((mode, registration));
            }
        }
        self.active.replace(registered);
        self.failures.take();
    }

    pub(super) fn close(&self) {
        self.active.take();
        self.starting.take();
    }
}
