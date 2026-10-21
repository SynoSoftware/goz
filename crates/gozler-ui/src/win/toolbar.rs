//! The native toolbar (`TOOLBARCLASSNAME`), which holds the main window's
//! icon buttons and its Match case and Match path (DESIGN.md: Look), so
//! Windows draws their hover, pressed, checked and disabled looks and the
//! separators in every theme. A toolbar button is not a window: its click
//! arrives as the toolbar's `WM_COMMAND` with the button's ID, its tooltip
//! as `TBN_GETINFOTIPW`, and its access key reaches it through
//! [`press_access_key`], because `IsDialogMessageW` matches access keys only
//! against windows' texts.

use super::controls::{child, send, send_pointer};
use super::gdi::{disabled_color, glyph_list, icons_shown, system_color};
use super::notify::{ControlEvent, Notification};
use super::{Hwnd, wide};
use crate::core::glyph::Glyph;
use crate::core::layout::{Px, icon_glyph_height};
use windows::Win32::Foundation::{HWND, LPARAM, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_BTNTEXT, COLOR_WINDOW};
use windows::Win32::UI::Controls::{
    BTNS_AUTOSIZE, BTNS_BUTTON, BTNS_CHECK, BTNS_SEP, BTNS_SHOWTEXT, CCS_NODIVIDER,
    CCS_NOPARENTALIGN, CCS_NORESIZE, I_IMAGENONE, NMTBGETINFOTIPW, TB_ADDBUTTONSW, TB_BUTTONCOUNT,
    TB_BUTTONSTRUCTSIZE, TB_CHECKBUTTON, TB_ENABLEBUTTON, TB_GETBUTTON, TB_GETBUTTONSIZE,
    TB_GETIDEALSIZE, TB_ISBUTTONCHECKED, TB_ISBUTTONENABLED, TB_MAPACCELERATORW, TB_SETBUTTONINFOW,
    TB_SETBUTTONSIZE, TB_SETDISABLEDIMAGELIST, TB_SETEXTENDEDSTYLE, TB_SETIMAGELIST, TBBUTTON,
    TBBUTTONINFOW, TBIF_IMAGE, TBIF_STYLE, TBIF_TEXT, TBN_GETINFOTIPW, TBSTATE_ENABLED,
    TBSTYLE_EX_MIXEDBUTTONS, TBSTYLE_FLAT, TBSTYLE_LIST, TBSTYLE_TOOLTIPS, TOOLBARCLASSNAMEW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, EnumChildWindows, GetClassNameW, SendMessageW, WINDOW_EX_STYLE, WM_COMMAND,
};
use windows::core::{BOOL, PWSTR};

/// A toolbar button, by the ID its clicks carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    /// Shows only `glyph`, also while icons are hidden, because it has no
    /// text to show then; its text is its name for screen readers.
    Icon {
        id: u16,
        glyph: Glyph,
    },
    /// Shows `glyph` while icons show and its text while they are hidden,
    /// as File Explorer's command bar shows text buttons.
    IconOrText {
        id: u16,
        glyph: Glyph,
    },
    /// A check button that shows its text and no image; Windows draws it
    /// pressed in while it is checked.
    Check {
        id: u16,
    },
    Separator,
}

impl Tool {
    fn glyph(self) -> Option<Glyph> {
        match self {
            Tool::Icon { glyph, .. } | Tool::IconOrText { glyph, .. } => Some(glyph),
            Tool::Check { .. } | Tool::Separator => None,
        }
    }

    /// What the button keeps as its data, from which [`Toolbar::show_images`]
    /// reads its image: its glyph's, with the lowest bit set for
    /// [`Tool::IconOrText`]; 0 for none.
    fn data(self) -> usize {
        match self {
            Tool::Icon { glyph, .. } => glyph.data() << 1,
            Tool::IconOrText { glyph, .. } => (glyph.data() << 1) | 1,
            Tool::Check { .. } | Tool::Separator => 0,
        }
    }
}

/// A toolbar that places nothing itself: the window gives it its place
/// from the layout. Only [`Toolbar::create`] makes one, so its handle is a
/// toolbar's, which decoding its notifications relies on.
#[derive(Clone, Copy)]
pub(crate) struct Toolbar(Hwnd);

impl Toolbar {
    /// A toolbar in `parent` with `tools` in their order, without texts
    /// until [`Toolbar::set_text`] and without images until
    /// [`Toolbar::show_images`]. Its buttons are no Tab stops: the keyboard
    /// reaches them by their access keys.
    pub(crate) fn create(parent: Hwnd, id: u16, tools: &[Tool]) -> windows::core::Result<Toolbar> {
        // A toolbar otherwise moves itself to the top of its parent at the
        // parent's width.
        let style = TBSTYLE_FLAT
            | TBSTYLE_LIST
            | TBSTYLE_TOOLTIPS
            | (CCS_NOPARENTALIGN | CCS_NORESIZE | CCS_NODIVIDER) as u32;
        let toolbar = child(parent, TOOLBARCLASSNAMEW, style, WINDOW_EX_STYLE(0), id)?;
        send(toolbar, TB_BUTTONSTRUCTSIZE, size_of::<TBBUTTON>(), 0);
        send(
            toolbar,
            TB_SETEXTENDEDSTYLE,
            0,
            TBSTYLE_EX_MIXEDBUTTONS as isize,
        );
        let mut images = 0;
        let buttons: Vec<TBBUTTON> = tools
            .iter()
            .map(|tool| {
                let (id, style) = match *tool {
                    Tool::Icon { id, .. } | Tool::IconOrText { id, .. } => (id, BTNS_BUTTON),
                    Tool::Check { id } => (id, BTNS_CHECK | BTNS_SHOWTEXT | BTNS_AUTOSIZE),
                    Tool::Separator => (0, BTNS_SEP),
                };
                let image = match tool.glyph() {
                    Some(_) => {
                        images += 1;
                        images - 1
                    }
                    None if *tool == Tool::Separator => 0,
                    None => I_IMAGENONE,
                };
                TBBUTTON {
                    iBitmap: image,
                    idCommand: i32::from(id),
                    fsState: TBSTATE_ENABLED as u8,
                    fsStyle: style as u8,
                    dwData: tool.data(),
                    // No string: the texts come from TB_SETBUTTONINFOW,
                    // which copies them.
                    iString: -1,
                    ..Default::default()
                }
            })
            .collect();
        // SAFETY: `buttons` holds as many buttons as passed and outlives the
        // call, which copies them.
        unsafe { send_pointer(toolbar, TB_ADDBUTTONSW, buttons.len(), buttons.as_ptr()) };
        Ok(Toolbar(toolbar))
    }

    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Gives the buttons their images at the toolbar's DPI, in the system
    /// colours of now and as Show icons says, and makes each button
    /// `height` pixels tall, and each icon-only one square, as the control
    /// beside it. Call it again on a DPI, theme or Show icons change, after
    /// the toolbar's font, which resets the buttons' size.
    pub(crate) fn show_images(self, height: Px) {
        let side = icon_glyph_height(self.0.dpi());
        let buttons = self.buttons();
        let glyphs: Vec<Glyph> = buttons
            .iter()
            .filter_map(|(_, tool)| tool.glyph())
            .collect();
        let normal = glyph_list(&glyphs, side, system_color(COLOR_BTNTEXT)).unwrap_or_default();
        let disabled = glyph_list(&glyphs, side, disabled_color(COLOR_WINDOW)).unwrap_or_default();
        // The toolbar keeps the lists without owning them, and `glyph_list`
        // keeps them alive.
        send(self.0, TB_SETIMAGELIST, 0, normal.0);
        send(self.0, TB_SETDISABLEDIMAGELIST, 0, disabled.0);
        let mut image = 0;
        for (id, tool) in buttons {
            if let Tool::IconOrText { .. } = tool {
                let (index, style) = if icons_shown() {
                    (image, BTNS_BUTTON)
                } else {
                    (I_IMAGENONE, BTNS_BUTTON | BTNS_SHOWTEXT | BTNS_AUTOSIZE)
                };
                let info = TBBUTTONINFOW {
                    cbSize: size_of::<TBBUTTONINFOW>() as u32,
                    dwMask: TBIF_IMAGE | TBIF_STYLE,
                    iImage: index,
                    fsStyle: style as u8,
                    ..Default::default()
                };
                // SAFETY: `info` outlives the call, which copies it.
                unsafe {
                    send_pointer(self.0, TB_SETBUTTONINFOW, usize::from(id), &raw const info)
                };
            }
            if tool.glyph().is_some() {
                image += 1;
            }
        }
        let side = height.0 as u16 as isize;
        send(self.0, TB_SETBUTTONSIZE, 0, side | (side << 16));
    }

    /// Each button's ID and what it is, from the data [`Tool::data`] gave
    /// it.
    fn buttons(self) -> Vec<(u16, Tool)> {
        let count = usize::try_from(send(self.0, TB_BUTTONCOUNT, 0, 0)).unwrap_or(0);
        (0..count)
            .filter_map(|index| {
                let mut button = TBBUTTON::default();
                // SAFETY: `button` is a local the call fills in.
                let found = unsafe { send_pointer(self.0, TB_GETBUTTON, index, &raw mut button) };
                let id = u16::try_from(button.idCommand).ok()?;
                let tool = match (Glyph::from_data(button.dwData >> 1), button.dwData & 1) {
                    _ if button.fsStyle as u32 & BTNS_SEP != 0 => Tool::Separator,
                    (Some(glyph), 0) => Tool::Icon { id, glyph },
                    (Some(glyph), _) => Tool::IconOrText { id, glyph },
                    (None, _) => Tool::Check { id },
                };
                (found != 0).then_some((id, tool))
            })
            .collect()
    }

    /// Sets the text of the button `id`: what a text button shows and what
    /// screen readers say for every button.
    pub(crate) fn set_text(self, id: u16, text: &str) {
        let mut text = wide(text);
        let info = TBBUTTONINFOW {
            cbSize: size_of::<TBBUTTONINFOW>() as u32,
            dwMask: TBIF_TEXT,
            pszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        // A new text can size the buttons again, so they keep the height,
        // and an icon-only button the square, that `show_images` gave them.
        let height = (send(self.0, TB_GETBUTTONSIZE, 0, 0) >> 16) & 0xffff;
        // SAFETY: `info` and the text it points to outlive the call, which
        // copies the text.
        unsafe { send_pointer(self.0, TB_SETBUTTONINFOW, usize::from(id), &raw const info) };
        send(self.0, TB_SETBUTTONSIZE, 0, height | (height << 16));
    }

    /// Checks or unchecks the check button `id`, unless it is so.
    pub(crate) fn set_checked(self, id: u16, checked: bool) {
        if self.is_checked(id) != checked {
            send(
                self.0,
                TB_CHECKBUTTON,
                usize::from(id),
                isize::from(checked),
            );
        }
    }

    pub(crate) fn is_checked(self, id: u16) -> bool {
        send(self.0, TB_ISBUTTONCHECKED, usize::from(id), 0) != 0
    }

    /// Enables or disables the button `id`, unless it is so.
    pub(crate) fn set_enabled(self, id: u16, enabled: bool) {
        if self.is_enabled(id) != enabled {
            send(
                self.0,
                TB_ENABLEBUTTON,
                usize::from(id),
                isize::from(enabled),
            );
        }
    }

    fn is_enabled(self, id: u16) -> bool {
        send(self.0, TB_ISBUTTONENABLED, usize::from(id), 0) != 0
    }

    /// The width that shows every button whole, as the layout places it.
    pub(crate) fn width(self) -> Px {
        let mut size = SIZE::default();
        // SAFETY: `size` is a local the call fills in; FALSE asks for the
        // width.
        unsafe { send_pointer(self.0, TB_GETIDEALSIZE, 0, &raw mut size) };
        Px(size.cx)
    }

    /// The ID of the button that `event` says was clicked, if this toolbar
    /// sent it.
    pub(crate) fn clicked(self, event: ControlEvent) -> Option<u16> {
        (event.from == self.0).then_some(event.id)
    }

    /// `notification` as a request for a button's tooltip, if this toolbar
    /// sent one.
    pub(crate) fn tip_request<'a>(self, notification: &Notification<'a>) -> Option<TipRequest<'a>> {
        let header = notification.header();
        if header.hwndFrom != self.0.0 || header.code != TBN_GETINFOTIPW {
            return None;
        }
        // SAFETY: a toolbar sends TBN_GETINFOTIPW with an NMTBGETINFOTIPW,
        // valid and unaliased for the notification's lifetime.
        Some(TipRequest(unsafe {
            &mut *notification.header.cast::<NMTBGETINFOTIPW>()
        }))
    }

    /// Runs the button whose access key `key` is, if the toolbar has an
    /// enabled one, as a click does: a check button changes its check
    /// first. Returns whether it ran one.
    fn press_access_key(self, key: u16) -> bool {
        let mut id = 0u32;
        // SAFETY: `id` is a local the call fills in.
        let mapped =
            unsafe { send_pointer(self.0, TB_MAPACCELERATORW, usize::from(key), &raw mut id) };
        let Ok(id) = u16::try_from(id) else {
            return false;
        };
        if mapped == 0 || !self.is_enabled(id) {
            return false;
        }
        if let Some((_, Tool::Check { .. })) = self.buttons().into_iter().find(|(b, _)| *b == id) {
            send(
                self.0,
                TB_CHECKBUTTON,
                usize::from(id),
                isize::from(!self.is_checked(id)),
            );
        }
        let wparam = usize::from(id) | (BN_CLICKED as usize) << 16;
        // SAFETY: plain values; the parent decodes the toolbar's handle as
        // a click's sender.
        unsafe {
            SendMessageW(
                self.0.parent().0,
                WM_COMMAND,
                Some(WPARAM(wparam)),
                Some(LPARAM(self.0.0.0 as isize)),
            )
        };
        true
    }
}

/// `TBN_GETINFOTIPW`: a toolbar asks for the tooltip of a button.
pub(crate) struct TipRequest<'a>(&'a mut NMTBGETINFOTIPW);

impl TipRequest<'_> {
    /// The ID of the button whose tip the toolbar asks for.
    pub(crate) fn id(&self) -> Option<u16> {
        u16::try_from(self.0.iItem).ok()
    }

    /// Answers with `text`, cut to the room the toolbar gives.
    pub(crate) fn set(self, text: &str) {
        let Ok(room) = usize::try_from(self.0.cchTextMax) else {
            return;
        };
        if room == 0 || self.0.pszText.is_null() {
            return;
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        let kept = units.len().min(room - 1);
        // SAFETY: the toolbar gives pszText with room for cchTextMax
        // characters, valid while it waits for the answer.
        let buffer = unsafe { std::slice::from_raw_parts_mut(self.0.pszText.0, room) };
        buffer[..kept].copy_from_slice(&units[..kept]);
        buffer[kept] = 0;
    }
}

/// Runs the button of a visible toolbar in `window` whose access key is
/// the `WM_SYSCHAR` character `key`, as `IsDialogMessageW` runs a
/// control's. Returns whether one ran.
pub(crate) fn press_access_key(window: Hwnd, key: WPARAM) -> bool {
    unsafe extern "system" fn add(child: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the vector that `press_access_key` passed,
        // alive while EnumChildWindows runs.
        unsafe { (*(found.0 as *mut Vec<Hwnd>)).push(Hwnd(child)) };
        BOOL(1)
    }
    let Ok(key) = u16::try_from(key.0) else {
        return false;
    };
    let mut children: Vec<Hwnd> = Vec::new();
    // SAFETY: the callback writes only to `children`, which outlives the
    // call.
    let _ = unsafe {
        EnumChildWindows(
            Some(window.0),
            Some(add),
            LPARAM(&raw mut children as isize),
        )
    };
    children
        .into_iter()
        .filter(|child| is_toolbar(*child) && child.is_visible())
        .any(|toolbar| Toolbar(toolbar).press_access_key(key))
}

fn is_toolbar(window: Hwnd) -> bool {
    let mut name = [0u16; 16];
    // SAFETY: `name` is a local whose length the call is given.
    let length = unsafe { GetClassNameW(window.0, &mut name) };
    usize::try_from(length)
        .is_ok_and(|n| String::from_utf16_lossy(&name[..n]).eq_ignore_ascii_case("ToolbarWindow32"))
}
