//! The menu bar and the accelerator table, built from what
//! `core::commands` says (design rule 9).

use super::wide;
use crate::core::commands::{Key, Mark, MenuNode, Shortcut};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_F1, VK_OEM_COMMA, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::{
    ACCEL, ACCEL_VIRT_FLAGS, CheckMenuItem, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu,
    DestroyAcceleratorTable, DestroyMenu, EnableMenuItem, FALT, FCONTROL, FSHIFT, FVIRTKEY, HACCEL,
    HMENU, InsertMenuItemW, MENU_ITEM_TYPE, MENUITEMINFOW, MF_BYCOMMAND, MF_CHECKED, MF_ENABLED,
    MF_GRAYED, MF_UNCHECKED, MFT_RADIOCHECK, MFT_RIGHTJUSTIFY, MFT_SEPARATOR, MFT_STRING,
    MIIM_FTYPE, MIIM_ID, MIIM_STRING, MIIM_SUBMENU,
};
use windows::core::PWSTR;

/// A menu handle. A menu bar that a window shows is destroyed with the
/// window; one that is replaced must be destroyed with [`Menu::destroy`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Menu(HMENU);

impl Menu {
    /// A menu bar of `menus`, each a [`MenuNode::Submenu`].
    pub(crate) fn bar(menus: &[MenuNode]) -> windows::core::Result<Menu> {
        // SAFETY: no parameters.
        let bar = Menu(unsafe { CreateMenu() }?);
        bar.append(menus)?;
        Ok(bar)
    }

    fn append(self, nodes: &[MenuNode]) -> windows::core::Result<()> {
        for (position, node) in nodes.iter().enumerate() {
            let mut text = Vec::new();
            let mut item = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE,
                ..Default::default()
            };
            match node {
                MenuNode::Separator => item.fType = MFT_SEPARATOR,
                MenuNode::Item {
                    id,
                    text: label,
                    mark,
                } => {
                    text = wide(label);
                    item.fMask |= MIIM_ID | MIIM_STRING;
                    item.fType = if *mark == Mark::Radio {
                        MFT_RADIOCHECK
                    } else {
                        MFT_STRING
                    };
                    item.wID = u32::from(*id);
                }
                MenuNode::Submenu {
                    text: label,
                    children,
                    aligned_right,
                } => {
                    text = wide(label);
                    // SAFETY: no parameters.
                    let submenu = Menu(unsafe { CreatePopupMenu() }?);
                    submenu.append(children)?;
                    item.fMask |= MIIM_STRING | MIIM_SUBMENU;
                    item.fType = if *aligned_right {
                        MFT_RIGHTJUSTIFY
                    } else {
                        MENU_ITEM_TYPE(0)
                    };
                    item.hSubMenu = submenu.0;
                }
            }
            item.dwTypeData = PWSTR(text.as_mut_ptr());
            // SAFETY: `item` and its text outlive the call, which copies
            // them; a submenu then belongs to this menu.
            unsafe { InsertMenuItemW(self.0, position as u32, true, &item) }?;
        }
        Ok(())
    }

    /// Enables or greys the item `id` and sets its check or radio mark,
    /// wherever it is in the menu's submenus.
    pub(crate) fn set_state(self, id: u16, enabled: bool, checked: bool) {
        // SAFETY: plain values; an ID the menu lacks changes nothing.
        unsafe {
            let _ = EnableMenuItem(
                self.0,
                u32::from(id),
                MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
            );
            CheckMenuItem(
                self.0,
                u32::from(id),
                (MF_BYCOMMAND | if checked { MF_CHECKED } else { MF_UNCHECKED }).0,
            );
        }
    }

    /// Destroys a menu bar that no window shows any more.
    pub(crate) fn destroy(self) {
        // SAFETY: the caller has replaced this menu on its window.
        let _ = unsafe { DestroyMenu(self.0) };
    }

    pub(super) fn handle(self) -> HMENU {
        self.0
    }
}

/// The accelerator table, destroyed on drop.
pub(crate) struct Accelerators(HACCEL);

impl Accelerators {
    /// A table of `shortcuts`, each with the menu item ID it sends.
    pub(crate) fn new(shortcuts: &[(Shortcut, u16)]) -> windows::core::Result<Accelerators> {
        let table: Vec<ACCEL> = shortcuts
            .iter()
            .map(|(shortcut, id)| {
                let mut flags = FVIRTKEY.0;
                for (on, flag) in [
                    (shortcut.ctrl, FCONTROL),
                    (shortcut.alt, FALT),
                    (shortcut.shift, FSHIFT),
                ] {
                    if on {
                        flags |= flag.0;
                    }
                }
                ACCEL {
                    fVirt: ACCEL_VIRT_FLAGS(flags),
                    key: virtual_key(shortcut.key),
                    cmd: *id,
                }
            })
            .collect();
        // SAFETY: the call copies `table`.
        Ok(Accelerators(unsafe { CreateAcceleratorTableW(&table) }?))
    }

    pub(super) fn handle(&self) -> HACCEL {
        self.0
    }
}

impl Drop for Accelerators {
    fn drop(&mut self) {
        // SAFETY: the table is this value's own.
        let _ = unsafe { DestroyAcceleratorTable(self.0) };
    }
}

fn virtual_key(key: Key) -> u16 {
    match key {
        // A capital letter's virtual-key code is its ASCII code.
        Key::Letter(letter) => u16::from(letter),
        Key::Function(n) => VK_F1.0 + u16::from(n) - 1,
        Key::Enter => VK_RETURN.0,
        Key::Comma => VK_OEM_COMMA.0,
    }
}
