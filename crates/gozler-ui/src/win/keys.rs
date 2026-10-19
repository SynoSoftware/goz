//! The virtual-key code of each [`Key`], one table for the accelerators,
//! the keys the window reads, the hotkey controls and the shortcut to show
//! Gozler.

use crate::core::commands::Key;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F24, VK_HOME, VK_INSERT,
    VK_LEFT, VK_NEXT, VK_OEM_COMMA, VK_OEM_PERIOD, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB,
    VK_UP,
};

/// The keys without a number, with their codes.
const NAMED: [(Key, VIRTUAL_KEY); 17] = [
    (Key::Enter, VK_RETURN),
    (Key::Escape, VK_ESCAPE),
    (Key::Tab, VK_TAB),
    (Key::Space, VK_SPACE),
    (Key::Backspace, VK_BACK),
    (Key::Delete, VK_DELETE),
    (Key::Insert, VK_INSERT),
    (Key::Home, VK_HOME),
    (Key::End, VK_END),
    (Key::PageUp, VK_PRIOR),
    (Key::PageDown, VK_NEXT),
    (Key::Left, VK_LEFT),
    (Key::Up, VK_UP),
    (Key::Right, VK_RIGHT),
    (Key::Down, VK_DOWN),
    (Key::Comma, VK_OEM_COMMA),
    (Key::Period, VK_OEM_PERIOD),
];

pub(crate) fn virtual_key(key: Key) -> u16 {
    match key {
        // A capital letter's or a digit's virtual-key code is its ASCII
        // code.
        Key::Letter(c) | Key::Digit(c) => u16::from(c),
        Key::Function(n) => VK_F1.0 + u16::from(n) - 1,
        named => NAMED
            .iter()
            .find_map(|(k, vk)| (*k == named).then_some(vk.0))
            .expect("NAMED holds every key without a number"),
    }
}

/// The key of the virtual-key code `code`, if a shortcut can be it.
pub(crate) fn key_of(code: u16) -> Option<Key> {
    match code {
        0x41..=0x5A => Some(Key::Letter(code as u8)),
        0x30..=0x39 => Some(Key::Digit(code as u8)),
        code if (VK_F1.0..=VK_F24.0).contains(&code) => {
            Some(Key::Function((code - VK_F1.0 + 1) as u8))
        }
        code => NAMED.iter().find(|(_, vk)| vk.0 == code).map(|(k, _)| *k),
    }
}

/// Whether Windows sends `key` as an extended key, which a hotkey control
/// must be told, or it names the arrow keys after the number pad's.
pub(crate) fn is_extended(key: Key) -> bool {
    matches!(
        key,
        Key::Insert
            | Key::Delete
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
            | Key::Left
            | Key::Up
            | Key::Right
            | Key::Down
    )
}
