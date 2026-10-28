//! The virtual-key code of each [`Key`], one match for the accelerators,
//! the keys the window reads, the hotkey controls and the shortcut to show
//! Gozler.

use crate::core::commands::Key;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_ADD, VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F24, VK_HOME, VK_INSERT,
    VK_LEFT, VK_NEXT, VK_OEM_COMMA, VK_OEM_PERIOD, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE,
    VK_SUBTRACT, VK_TAB, VK_UP,
};

pub(crate) fn virtual_key(key: Key) -> u16 {
    match key {
        // A capital letter's or a digit's virtual-key code is its ASCII
        // code.
        Key::Letter(c) | Key::Digit(c) => u16::from(c),
        Key::Function(n) => VK_F1.0 + u16::from(n) - 1,
        Key::Enter => VK_RETURN.0,
        Key::Escape => VK_ESCAPE.0,
        Key::Tab => VK_TAB.0,
        Key::Space => VK_SPACE.0,
        Key::Backspace => VK_BACK.0,
        Key::Delete => VK_DELETE.0,
        Key::Insert => VK_INSERT.0,
        Key::Home => VK_HOME.0,
        Key::End => VK_END.0,
        Key::PageUp => VK_PRIOR.0,
        Key::PageDown => VK_NEXT.0,
        Key::Left => VK_LEFT.0,
        Key::Up => VK_UP.0,
        Key::Right => VK_RIGHT.0,
        Key::Down => VK_DOWN.0,
        Key::Comma => VK_OEM_COMMA.0,
        Key::Period => VK_OEM_PERIOD.0,
        Key::NumPlus => VK_ADD.0,
        Key::NumMinus => VK_SUBTRACT.0,
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
        code => Key::NAMED.into_iter().find(|key| virtual_key(*key) == code),
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
