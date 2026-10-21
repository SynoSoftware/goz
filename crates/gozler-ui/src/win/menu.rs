//! The menu bar and the accelerator table, built from what
//! `core::commands` says (DESIGN.md: window rule 9).

use super::gdi::{menu_bitmap, system_color};
use super::keys::virtual_key;
use super::{Hwnd, wide};
use crate::core::commands::{Keystroke, Mark, MenuNode};
use crate::core::glyph::Glyph;
use crate::core::layout::{Px, icon_glyph_height};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_GRAYTEXT, COLOR_MENUTEXT, HBITMAP};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_DOWN;
use windows::Win32::UI::WindowsAndMessaging::{
    ACCEL, ACCEL_VIRT_FLAGS, CheckMenuItem, CreateAcceleratorTableW, CreateMenu, CreatePopupMenu,
    DestroyAcceleratorTable, DestroyMenu, EnableMenuItem, FALT, FCONTROL, FSHIFT, FVIRTKEY,
    GetMenu, GetMenuInfo, GetMenuItemCount, GetMenuItemInfoW, HACCEL, HMENU, InsertMenuItemW,
    MENU_ITEM_TYPE, MENUINFO, MENUITEMINFOW, MF_BYCOMMAND, MF_CHECKED, MF_ENABLED, MF_GRAYED,
    MF_UNCHECKED, MFT_RADIOCHECK, MFT_RIGHTJUSTIFY, MFT_SEPARATOR, MFT_STRING, MIIM_BITMAP,
    MIIM_DATA, MIIM_FTYPE, MIIM_ID, MIIM_STRING, MIIM_SUBMENU, MIM_MENUDATA, MIM_STYLE,
    MNS_CHECKORBMP, MSG, PostMessageW, SetMenu, SetMenuDefaultItem, SetMenuInfo, SetMenuItemInfoW,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, TranslateAcceleratorW, WM_KEYDOWN, WM_NULL,
};
use windows::core::PWSTR;

/// A menu handle, and the side of its items' icons in its window's pixels.
/// A menu bar that a window shows is destroyed with the window or by
/// [`replace_menu_bar`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Menu {
    handle: HMENU,
    icon_side: Px,
}

/// Gives `window` a menu bar of `menus`, each a [`MenuNode::Submenu`], and
/// destroys the bar it had, which no window shows any more. The window
/// calls it again when its DPI changes, because the icons are bitmaps of
/// one size.
///
/// A bar with the same titles at the same DPI keeps its items and gets only
/// new menus, so it is not drawn again (DESIGN.md: Painting); `SetMenu`
/// draws a new bar once, and Windows measures it at the window's DPI.
pub(crate) fn replace_menu_bar(window: Hwnd, menus: &[MenuNode]) -> windows::core::Result<()> {
    let old = Menu::of(window);
    if let Some(old) = old.filter(|old| old.built_side() == old.icon_side && old.has_titles(menus))
    {
        return old.replace_submenus(menus);
    }
    let bar = Menu {
        // SAFETY: no parameters.
        handle: unsafe { CreateMenu() }?,
        icon_side: icon_glyph_height(window.dpi()),
    };
    // The bar keeps the side its icons were built at.
    let info = MENUINFO {
        cbSize: size_of::<MENUINFO>() as u32,
        fMask: MIM_MENUDATA,
        dwMenuData: bar.icon_side.0 as usize,
        ..Default::default()
    };
    // SAFETY: `info` outlives the call. Without the data the next
    // replacement builds the bar again.
    let _ = unsafe { SetMenuInfo(bar.handle, &info) };
    if let Err(error) = bar.append(menus) {
        bar.destroy();
        return Err(error);
    }
    // SAFETY: the window then owns the new bar, and shows the old one no
    // more, so destroying it leaves no window with a destroyed menu.
    if let Err(error) = unsafe { SetMenu(window.0, Some(bar.handle)) } {
        bar.destroy();
        return Err(error);
    }
    if let Some(old) = old {
        old.destroy();
    }
    Ok(())
}

impl Menu {
    /// The menu bar of `window`, if it has one.
    pub(crate) fn of(window: Hwnd) -> Option<Menu> {
        // SAFETY: plain values; a window without a menu gives null.
        let handle = unsafe { GetMenu(window.0) };
        (!handle.is_invalid()).then(|| Menu {
            handle,
            icon_side: icon_glyph_height(window.dpi()),
        })
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
                    (item.hbmpItem, item.dwItemData) = self.icon(*glyph);
                }
                MenuNode::Submenu {
                    text: label,
                    children,
                    aligned_right,
                } => {
                    text = wide(label);
                    let submenu = Menu::popup(self.icon_side)?;
                    submenu.append(children)?;
                    item.fMask |= MIIM_STRING | MIIM_SUBMENU;
                    item.fType = if *aligned_right {
                        MFT_RIGHTJUSTIFY
                    } else {
                        MENU_ITEM_TYPE(0)
                    };
                    item.hSubMenu = submenu.handle;
                }
            }
            item.dwTypeData = PWSTR(text.as_mut_ptr());
            // SAFETY: `item` and its text outlive the call, which copies
            // them; a submenu then belongs to this menu.
            unsafe { InsertMenuItemW(self.handle, position as u32, true, &item) }?;
        }
        Ok(())
    }

    /// Enables or greys the item `id`, its icon included, and sets its
    /// check or radio mark, wherever it is in the menu's submenus.
    pub(crate) fn set_state(self, id: u16, enabled: bool, checked: bool) {
        // SAFETY: plain values; an ID the menu lacks changes nothing.
        unsafe {
            let _ = EnableMenuItem(
                self.handle,
                u32::from(id),
                MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
            );
            CheckMenuItem(
                self.handle,
                u32::from(id),
                (MF_BYCOMMAND | if checked { MF_CHECKED } else { MF_UNCHECKED }).0,
            );
        }
        // Windows draws an item's bitmap as it is, so the icon of an
        // unavailable item is a gray one.
        let mut item = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_DATA,
            ..Default::default()
        };
        // SAFETY: `item` is a local that the call fills in; an ID the menu
        // lacks fails.
        if unsafe { GetMenuItemInfoW(self.handle, u32::from(id), false, &mut item) }.is_err() {
            return;
        }
        let color = system_color(if enabled {
            COLOR_MENUTEXT
        } else {
            COLOR_GRAYTEXT
        });
        let Some(bitmap) = Glyph::from_data(item.dwItemData)
            .and_then(|glyph| menu_bitmap(glyph, self.icon_side, color))
        else {
            return;
        };
        let item = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_BITMAP,
            hbmpItem: bitmap,
            ..Default::default()
        };
        // SAFETY: plain values; the bitmap lives as long as the thread.
        let _ = unsafe { SetMenuItemInfoW(self.handle, u32::from(id), false, &item) };
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
        let menu = Menu::popup(icon_glyph_height(owner.dpi())).ok()?;
        let chosen = menu.append(items).ok().map(|()| {
            for &(id, enabled, checked) in states {
                menu.set_state(id, enabled, checked);
            }
            if let Some(id) = default {
                // SAFETY: plain values; an ID the menu lacks fails.
                let _ = unsafe { SetMenuDefaultItem(menu.handle, u32::from(id), 0) };
            }
            // SAFETY: the menu is this function's own; with TPM_RETURNCMD
            // the call returns the chosen ID instead of sending WM_COMMAND.
            unsafe {
                TrackPopupMenuEx(
                    menu.handle,
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

    /// As [`Menu::track_popup`], with the item at `highlighted` among the
    /// items that are not separators highlighted when the menu opens, so
    /// the arrow keys start from it. The menu's own loop takes the `Down`
    /// keys posted before it opens, as it takes the person's.
    pub(crate) fn track_popup_highlighted(
        owner: Hwnd,
        items: &[MenuNode],
        point: (i32, i32),
        states: &[(u16, bool, bool)],
        highlighted: usize,
    ) -> Option<u16> {
        let down = WPARAM(usize::from(VK_DOWN.0));
        for _ in 0..=highlighted {
            // SAFETY: plain values.
            let _ = unsafe { PostMessageW(Some(owner.0), WM_KEYDOWN, down, LPARAM(0)) };
        }
        Menu::track_popup(owner, items, point, states, None)
    }

    /// [`Menu::track_popup`] for a window that may not be in front, as under
    /// the notification area's icon or for a drop from another program. The
    /// window comes to the front first and is sent a message after, as the
    /// `Shell_NotifyIconW` documentation requires, so the menu closes on a
    /// click elsewhere.
    pub(crate) fn track_popup_in_front(
        owner: Hwnd,
        items: &[MenuNode],
        point: (i32, i32),
        states: &[(u16, bool, bool)],
        default: Option<u16>,
    ) -> Option<u16> {
        owner.set_foreground();
        let chosen = Menu::track_popup(owner, items, point, states, default);
        owner.post(WM_NULL);
        chosen
    }

    /// A popup menu whose items show their check mark or their icon in one
    /// column, because no item has both (DESIGN.md: Look).
    fn popup(icon_side: Px) -> windows::core::Result<Menu> {
        let menu = Menu {
            // SAFETY: no parameters.
            handle: unsafe { CreatePopupMenu() }?,
            icon_side,
        };
        let info = MENUINFO {
            cbSize: size_of::<MENUINFO>() as u32,
            fMask: MIM_STYLE,
            dwStyle: MNS_CHECKORBMP,
            ..Default::default()
        };
        // SAFETY: `info` outlives the call. Without the style the menu
        // still works, with a wider margin.
        let _ = unsafe { SetMenuInfo(menu.handle, &info) };
        Ok(menu)
    }

    /// The icon side the bar was built at ([`replace_menu_bar`]).
    fn built_side(self) -> Px {
        let mut info = MENUINFO {
            cbSize: size_of::<MENUINFO>() as u32,
            fMask: MIM_MENUDATA,
            ..Default::default()
        };
        // SAFETY: `info` is a local that the call fills in.
        let _ = unsafe { GetMenuInfo(self.handle, &mut info) };
        Px(i32::try_from(info.dwMenuData).unwrap_or(0))
    }

    /// Whether the bar's items are the titles of `menus`, each a
    /// [`MenuNode::Submenu`], in the same order and alignment.
    fn has_titles(self, menus: &[MenuNode]) -> bool {
        // SAFETY: plain values.
        let count = unsafe { GetMenuItemCount(Some(self.handle)) };
        if usize::try_from(count).ok() != Some(menus.len()) {
            return false;
        }
        menus.iter().enumerate().all(|(position, node)| {
            let MenuNode::Submenu {
                text,
                aligned_right,
                ..
            } = node
            else {
                return false;
            };
            let mut shown = [0u16; 128];
            let mut item = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_STRING,
                dwTypeData: PWSTR(shown.as_mut_ptr()),
                cch: shown.len() as u32,
                ..Default::default()
            };
            // SAFETY: `item` and `shown` are locals that outlive the call,
            // which writes at most `cch` characters, its NUL included.
            if unsafe { GetMenuItemInfoW(self.handle, position as u32, true, &mut item) }.is_err() {
                return false;
            }
            let length = (item.cch as usize).min(shown.len());
            let right = item.fType.0 & MFT_RIGHTJUSTIFY.0 != 0;
            String::from_utf16_lossy(&shown[..length]) == *text && right == *aligned_right
        })
    }

    /// Gives each of the bar's items the menu of its node in `menus`, which
    /// [`Self::has_titles`] matched, and destroys the menu it had.
    fn replace_submenus(self, menus: &[MenuNode]) -> windows::core::Result<()> {
        for (position, node) in menus.iter().enumerate() {
            let MenuNode::Submenu { children, .. } = node else {
                continue;
            };
            let submenu = Menu::popup(self.icon_side)?;
            if let Err(error) = submenu.append(children) {
                submenu.destroy();
                return Err(error);
            }
            let mut item = MENUITEMINFOW {
                cbSize: size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_SUBMENU,
                ..Default::default()
            };
            // SAFETY: `item` is a local that the call fills in.
            let had = unsafe { GetMenuItemInfoW(self.handle, position as u32, true, &mut item) }
                .map(|()| item.hSubMenu);
            item.hSubMenu = submenu.handle;
            // SAFETY: `item` outlives the call; the submenu then belongs to
            // the bar, and the bar no longer shows the one it had.
            let set = unsafe { SetMenuItemInfoW(self.handle, position as u32, true, &item) };
            if let Err(error) = set {
                submenu.destroy();
                return Err(error);
            }
            if let Ok(had) = had
                && !had.is_invalid()
            {
                Menu {
                    handle: had,
                    icon_side: self.icon_side,
                }
                .destroy();
            }
        }
        Ok(())
    }

    /// Destroys a menu that no window shows.
    fn destroy(self) {
        // SAFETY: the callers' menus are their own, or replaced on their
        // window.
        let _ = unsafe { DestroyMenu(self.handle) };
    }

    /// An item's bitmap and data for `glyph`, while icons show: the icon in
    /// the menu's text colour, and the glyph, which [`Self::set_state`]
    /// reads to gray the icon of an unavailable item.
    fn icon(self, glyph: Option<Glyph>) -> (HBITMAP, usize) {
        glyph
            .and_then(|glyph| {
                let bitmap = menu_bitmap(glyph, self.icon_side, system_color(COLOR_MENUTEXT))?;
                Some((bitmap, glyph.data()))
            })
            .unwrap_or_default()
    }
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
