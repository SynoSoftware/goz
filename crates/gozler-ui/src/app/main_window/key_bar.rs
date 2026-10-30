use super::{
    Button, ButtonIcon, ControlEvent, ControlId, Focus, Hwnd, MainWindow, State, Tooltip,
    WindowCaptions, commands,
};
use std::cell::Cell;

/// The key bar's buttons, `F3` to `F8` (DESIGN.md: The key bar).
pub(super) const KEY_BAR: [ControlId; 6] = [
    ControlId::KeyF3,
    ControlId::KeyF4,
    ControlId::KeyF5,
    ControlId::KeyF6,
    ControlId::KeyF7,
    ControlId::KeyF8,
];

/// The key bar buttons that have a tip, `F4 Edit` and `F6 Move`, by their
/// tooltip area, which the key bar's tooltip numbers from 0.
pub(super) const KEY_TIPS: [usize; 2] = [1, 3];

pub(super) struct KeyBar {
    pub(super) buttons: [Button; 6],
    pub(super) tips: Tooltip,
    alt: Cell<bool>,
}

impl KeyBar {
    pub(super) fn create(parent: Hwnd) -> windows::core::Result<Self> {
        let buttons = KEY_BAR.map(|id| Button::create_key(parent, id.id()));
        let [a, b, c, d, e, f] = buttons;
        Ok(Self {
            buttons: [a?, b?, c?, d?, e?, f?],
            tips: Tooltip::create_areas(parent, KEY_TIPS.len())?,
            alt: Cell::new(false),
        })
    }

    pub(super) fn set_captions(&self, captions: &WindowCaptions) {
        self.set_labels(&captions.key_bar[usize::from(self.alt.get())]);
    }

    fn set_labels(&self, captions: &[(String, String, Option<String>); 6]) {
        for (button, (key, word, _)) in self.buttons.iter().zip(captions) {
            button.hwnd().set_text(format!("{key} {word}").trim());
        }
        for (area, index) in KEY_TIPS.into_iter().enumerate() {
            self.tips
                .set_area(area, captions[index].2.as_deref().unwrap_or(""));
        }
    }

    pub(super) fn sync_alt(&self, window: &MainWindow) {
        let alt = super::win::window::only_alt_held()
            && window.model.borrow().mode() == super::Mode::Commander;
        self.set_alt(window, alt);
    }

    pub(super) fn set_alt(&self, window: &MainWindow, alt: bool) {
        if self.alt.replace(alt) == alt {
            return;
        }
        let captions = crate::core::captions::key_bar(window.model.borrow().strings(), alt);
        self.set_labels(&captions);
        self.apply_glyphs();
        self.update_key_bar(window);
    }

    pub(super) fn apply_glyphs(&self) {
        for (index, button) in self.buttons.iter().enumerate() {
            let glyph = commands::key_bar_item(index, self.alt.get()).and_then(|item| item.glyph());
            button.set_icon(glyph.map(ButtonIcon::Glyph));
        }
    }

    pub(super) fn clicked(&self, window: &MainWindow, hwnd: Hwnd, event: ControlEvent) -> bool {
        let Some(index) = self.buttons.iter().position(|button| button.clicked(event)) else {
            return false;
        };
        let Some(item) = commands::key_bar_item(index, self.alt.get()) else {
            return true;
        };
        let active = window.model.borrow().panels().active();
        window.panel(active).list.hwnd().focus();
        window.run(hwnd, item);
        true
    }
    /// Enables each key bar button as its command is, on the active panel's
    /// state (DESIGN.md: The key bar).
    pub(super) fn update_key_bar(&self, window: &MainWindow) {
        let enabled: Vec<bool> = {
            let model = window.model.borrow();
            let state = State {
                model: &model,
                operations_ready: window.file_operations.get().is_some(),
                focus: Focus::Panel,
                list: model.active_list(),
            };
            (0..KEY_BAR.len())
                .map(|index| {
                    commands::key_bar_item(index, self.alt.get()).is_some_and(|item| {
                        item.enabled(&state)
                            && !window.selection_command_disabled(state.list, item, state.focus)
                    })
                })
                .collect()
        };
        for (button, enabled) in self.buttons.iter().zip(enabled) {
            if button.hwnd().is_enabled() != enabled {
                button.hwnd().set_enabled(enabled);
            }
        }
    }
}
