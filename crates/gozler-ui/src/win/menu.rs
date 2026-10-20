//! The menu bar and the accelerator table, built from what
//! `core::commands` says (DESIGN.md: window rule 9).

use super::gdi::{draw_glyph, high_contrast, icons_shown, system_color};
use super::keys::virtual_key;
use super::{Hwnd, wide};
use crate::core::commands::{Keystroke, Mark, MenuNode};
use crate::core::glyph::Glyph;
use crate::core::layout::icon_glyph_height;
use windows::Win32::Foundation::{LPARAM, LRESULT};
use windows::Win32::Graphics::Gdi::{COLOR_GRAYTEXT, COLOR_HIGHLIGHTTEXT, COLOR_MENUTEXT, HBITMAP};
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_DISABLED, ODS_FLAGS, ODS_GRAYED, ODS_SELECTED, ODT_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ACCEL, ACCEL_VIRT_FLAGS, CheckMenuItem, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu,
    DestroyAcceleratorTable, DestroyMenu, DrawMenuBar, EnableMenuItem, FALT, FCONTROL, FSHIFT,
    FVIRTKEY, GetMenu, HACCEL, HBMMENU_CALLBACK, HMENU, InsertMenuItemW, MENU_ITEM_TYPE, MENUINFO,
    MENUITEMINFOW, MF_BYCOMMAND, MF_CHECKED, MF_ENABLED, MF_GRAYED, MF_UNCHECKED, MFT_RADIOCHECK,
    MFT_RIGHTJUSTIFY, MFT_SEPARATOR, MFT_STRING, MIIM_BITMAP, MIIM_DATA, MIIM_FTYPE, MIIM_ID,
    MIIM_STRING, MIIM_SUBMENU, MIM_STYLE, MNS_CHECKORBMP, MSG, SetMenu, SetMenuDefaultItem,
    SetMenuInfo, SetMenuItemInfoW, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
    TranslateAcceleratorW, WM_DRAWITEM, WM_MEASUREITEM,
};
use windows::core::PWSTR;

/// A menu handle. A menu bar that a window shows is destroyed with the
/// window or by [`replace_menu_bar`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Menu(HMENU);

/// Gives `window` a menu bar of `menus`, each a [`MenuNode::Submenu`], and
/// destroys the bar it had, which no window shows any more.
pub(crate) fn replace_menu_bar(window: Hwnd, menus: &[MenuNode]) -> windows::core::Result<()> {
    // SAFETY: no parameters.
    let bar = Menu(unsafe { CreateMenu() }?);
    if let Err(error) = bar.append(menus) {
        bar.destroy();
        return Err(error);
    }
    let old = Menu::of(window);
    // SAFETY: the window then owns the new bar, and shows the old one no
    // more, so destroying it leaves no window with a destroyed menu.
    if let Err(error) = unsafe { SetMenu(window.0, Some(bar.0)) } {
        bar.destroy();
        return Err(error);
    }
    // SAFETY: plain values.
    let _ = unsafe { DrawMenuBar(window.0) };
    if let Some(old) = old {
        old.destroy();
    }
    Ok(())
}

impl Menu {
    /// The menu bar of `window`, if it has one.
    pub(crate) fn of(window: Hwnd) -> Option<Menu> {
        // SAFETY: plain values; a window without a menu gives null.
        let menu = unsafe { GetMenu(window.0) };
        (!menu.is_invalid()).then_some(Menu(menu))
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
                    glyph,
                } => {
                    text = wide(label);
                    item.fMask |= MIIM_ID | MIIM_STRING | MIIM_BITMAP | MIIM_DATA;
                    item.fType = if *mark == Mark::Radio {
                        MFT_RADIOCHECK
                    } else {
                        MFT_STRING
                    };
                    item.wID = u32::from(*id);
                    (item.hbmpItem, item.dwItemData) = icon(*glyph);
                }
                MenuNode::Submenu {
                    text: label,
                    children,
                    aligned_right,
                } => {
                    text = wide(label);
                    let submenu = Menu::popup()?;
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

    /// Replaces the item whose ID is one of `ids`, wherever it is in the
    /// menu's submenus, with `item`, a [`MenuNode::Item`], so one place of
    /// a menu shows one of several commands.
    pub(crate) fn replace_item(self, ids: &[u16], item: &MenuNode) {
        let MenuNode::Item {
            id, text, glyph, ..
        } = item
        else {
            return;
        };
        let mut text = wide(text);
        let (bitmap, data) = icon(*glyph);
        let info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_ID | MIIM_STRING | MIIM_BITMAP | MIIM_DATA,
            wID: u32::from(*id),
            dwTypeData: PWSTR(text.as_mut_ptr()),
            hbmpItem: bitmap,
            dwItemData: data,
            ..Default::default()
        };
        for old in ids {
            // SAFETY: `info` and its text outlive the call, which copies
            // them; an ID the menu lacks fails and changes nothing.
            if unsafe { SetMenuItemInfoW(self.0, u32::from(*old), false, &info) }.is_ok() {
                return;
            }
        }
    }

    /// Shows `items` as a context menu at `point`, in screen coordinates,
    /// with the item `default` in bold, and returns the ID of the item
    /// chosen. The menu runs its own message loop, so the caller holds no
    /// borrow across this call (DESIGN.md: window rule 1) and acts on the
    /// choice after it returns (DESIGN.md: window rule 9).
    pub(crate) fn track_popup(
        owner: Hwnd,
        items: &[MenuNode],
        point: (i32, i32),
        states: &[(u16, bool, bool)],
        default: Option<u16>,
    ) -> Option<u16> {
        let menu = Menu::popup().ok()?;
        let chosen = menu.append(items).ok().map(|()| {
            for &(id, enabled, checked) in states {
                menu.set_state(id, enabled, checked);
            }
            if let Some(id) = default {
                // SAFETY: plain values; an ID the menu lacks fails.
                let _ = unsafe { SetMenuDefaultItem(menu.0, u32::from(id), 0) };
            }
            // SAFETY: the menu is this function's own; with TPM_RETURNCMD
            // the call returns the chosen ID instead of sending WM_COMMAND.
            unsafe {
                TrackPopupMenuEx(
                    menu.0,
                    (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
                    point.0,
                    point.1,
                    owner.0,
                    None,
                )
            }
            .0
        });
        menu.destroy();
        chosen
            .and_then(|id| u16::try_from(id).ok())
            .filter(|id| *id != 0)
    }

    /// A popup menu whose items show their check mark or their icon in one
    /// column, because no item has both (DESIGN.md: Look).
    fn popup() -> windows::core::Result<Menu> {
        // SAFETY: no parameters.
        let menu = Menu(unsafe { CreatePopupMenu() }?);
        let info = MENUINFO {
            cbSize: size_of::<MENUINFO>() as u32,
            fMask: MIM_STYLE,
            dwStyle: MNS_CHECKORBMP,
            ..Default::default()
        };
        // SAFETY: `info` outlives the call. Without the style the menu
        // still works, with a wider margin.
        let _ = unsafe { SetMenuInfo(menu.0, &info) };
        Ok(menu)
    }

    /// Destroys a menu that no window shows.
    fn destroy(self) {
        // SAFETY: the callers' menus are their own, or replaced on their
        // window.
        let _ = unsafe { DestroyMenu(self.0) };
    }
}

/// An item's bitmap and data for `glyph`, while icons show: the menu asks
/// its owner to draw the icon, and the data names the glyph. Windows draws
/// a bitmap of its own as it is, not gray when the item is unavailable, so
/// the owner draws the icon in the item's state.
fn icon(glyph: Option<Glyph>) -> (HBITMAP, usize) {
    glyph
        .filter(|_| icons_shown())
        .map_or((HBITMAP::default(), 0), |glyph| {
            (HBMMENU_CALLBACK, glyph.data())
        })
}

/// Answers the `WM_MEASUREITEM` and `WM_DRAWITEM` of a menu item's icon,
/// which Windows sends to `window`, the menu's owner, and gives `None` for
/// any other message. The icon is drawn in the menu's text colour, gray
/// when the item is unavailable, and in the selection's text colour in high
/// contrast, where the selection is a solid colour.
///
/// # Safety
/// `lparam` is the one Windows sent with `msg`.
pub(super) unsafe fn icon_message(window: Hwnd, msg: u32, lparam: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_MEASUREITEM => {
            // SAFETY: WM_MEASUREITEM's lparam points to a MEASUREITEMSTRUCT
            // that the sender reads after the answer.
            let item = unsafe { &mut *(lparam.0 as *mut MEASUREITEMSTRUCT) };
            if item.CtlType != ODT_MENU {
                return None;
            }
            let side = icon_glyph_height(window.dpi()).0 as u32;
            item.itemWidth = side;
            item.itemHeight = side;
        }
        WM_DRAWITEM => {
            // SAFETY: WM_DRAWITEM's lparam points to a DRAWITEMSTRUCT that
            // is valid for the length of the message.
            let item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
            if item.CtlType != ODT_MENU {
                return None;
            }
            let glyph = Glyph::from_data(item.itemData)?;
            let state = |flag: ODS_FLAGS| item.itemState.0 & flag.0 != 0;
            let color = if state(ODS_GRAYED) || state(ODS_DISABLED) {
                COLOR_GRAYTEXT
            } else if state(ODS_SELECTED) && high_contrast() {
                COLOR_HIGHLIGHTTEXT
            } else {
                COLOR_MENUTEXT
            };
            let side = icon_glyph_height(window.dpi());
            draw_glyph(item.hDC, glyph, item.rcItem, side, system_color(color));
        }
        _ => return None,
    }
    Some(LRESULT(1))
}

/// The accelerator table, destroyed on drop.
pub(crate) struct Accelerators(HACCEL);

impl Accelerators {
    /// A table of `shortcuts`, each with the menu item ID it sends.
    pub(crate) fn new(shortcuts: &[(Keystroke, u16)]) -> windows::core::Result<Accelerators> {
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

    /// Sends `window` the command of the shortcut that `msg` presses, and
    /// answers whether it did.
    pub(super) fn translate(&self, window: Hwnd, msg: &MSG) -> bool {
        // SAFETY: `msg` is the message the loop read, and the table is this
        // value's own.
        unsafe { TranslateAcceleratorW(window.0, self.0, msg) != 0 }
    }
}

impl Drop for Accelerators {
    fn drop(&mut self) {
        // SAFETY: the table is this value's own.
        let _ = unsafe { DestroyAcceleratorTable(self.0) };
    }
}
