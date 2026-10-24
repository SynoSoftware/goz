use super::{
    Button, ControlEvent, ControlId, Focus, Hwnd, MainWindow, MenuItem, State, Tooltip,
    WindowCaptions, commands,
};

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
}

impl KeyBar {
    pub(super) fn create(parent: Hwnd) -> windows::core::Result<Self> {
        let buttons = KEY_BAR.map(|id| Button::create_key(parent, id.id()));
        let [a, b, c, d, e, f] = buttons;
        Ok(Self {
            buttons: [a?, b?, c?, d?, e?, f?],
            tips: Tooltip::create_areas(parent, KEY_TIPS.len())?,
        })
    }

    pub(super) fn set_captions(&self, captions: &WindowCaptions) {
        for (button, (key, word, _)) in self.buttons.iter().zip(&captions.key_bar) {
            button.hwnd().set_text(&format!("{key} {word}"));
        }
        for (area, index) in KEY_TIPS.into_iter().enumerate() {
            if let (_, _, Some(tip)) = &captions.key_bar[index] {
                self.tips.set_area(area, tip);
            }
        }
    }

    pub(super) fn clicked(&self, window: &MainWindow, hwnd: Hwnd, event: ControlEvent) -> bool {
        let Some(index) = self.buttons.iter().position(|button| button.clicked(event)) else {
            return false;
        };
        let active = window.model.borrow().panels().active();
        window.panel(active).list.hwnd().focus();
        window.run(hwnd, MenuItem::Run(commands::key_bar_command(index)));
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
                .map(|index| (commands::key_bar_command(index).row().enabled)(&state))
                .collect()
        };
        for (button, enabled) in self.buttons.iter().zip(enabled) {
            if button.hwnd().is_enabled() != enabled {
                button.hwnd().set_enabled(enabled);
            }
        }
    }
}
