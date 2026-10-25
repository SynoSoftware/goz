use super::{child, send, send_pointer};
use crate::core::glyph::Glyph;
use crate::core::layout::{self, Px, icon_glyph_height};
use crate::win::gdi::{Font, TextMeter, glyph_icon, icons_shown, system_color};
use crate::win::{Hwnd, wide};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::COLOR_BTNTEXT;
use windows::Win32::UI::Controls::{
    NMHDR, SB_GETICON, SB_GETPARTS, SB_GETRECT, SB_GETTEXTLENGTHW, SB_GETTEXTW, SB_GETTIPTEXTW,
    SB_SETICON, SB_SETMINHEIGHT, SB_SETPARTS, SB_SETTEXTW, SB_SETTIPTEXTW, SBARS_SIZEGRIP,
    SBARS_TOOLTIPS, SBT_NOBORDERS, STATUSCLASSNAMEW,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetParent, GetWindowRect, IsChild, SendMessageW, WM_COMMAND, WM_NCDESTROY, WM_NOTIFY, WM_SIZE,
    WS_CLIPCHILDREN, WS_EX_CONTROLPARENT,
};

/// The status bar, which sizes itself to its font at the window's bottom.
#[derive(Clone, Copy)]
pub(crate) struct StatusBar(Hwnd);

impl StatusBar {
    pub(crate) fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Creates the status bar as the parent of the controls in its parts,
    /// whose notifications it passes to `parent`. `Tab` and the access keys
    /// reach them, as `WS_EX_CONTROLPARENT` lets `IsDialogMessageW` look
    /// inside it. A parent that the person resizes gets the size grip. A
    /// part's tooltip shows only while its text is cut off.
    pub(crate) fn create(
        parent: Hwnd,
        id: u16,
        size_grip: bool,
    ) -> windows::core::Result<StatusBar> {
        let grip = if size_grip { SBARS_SIZEGRIP } else { 0 };
        let bar = child(
            parent,
            STATUSCLASSNAMEW,
            grip | SBARS_TOOLTIPS | WS_CLIPCHILDREN.0,
            WS_EX_CONTROLPARENT,
            id,
        )?;
        // SAFETY: the procedure uses no data, and removes itself on
        // WM_NCDESTROY, the status bar's last message.
        if !unsafe { SetWindowSubclass(bar.0, Some(status_bar_procedure), 0, 0) }.as_bool() {
            return Err(windows::core::Error::from_thread());
        }
        Ok(StatusBar(bar))
    }

    /// Sets the height of the drawing area, which Windows makes at least
    /// as tall as the font; [`Self::fit`] applies it.
    pub(crate) fn set_min_height(self, height: Px) {
        send(self.0, SB_SETMINHEIGHT, height.0.max(0) as usize, 0);
    }

    /// Fits the status bar to the parent's new size and returns its height.
    pub(crate) fn fit(self) -> Px {
        send(self.0, WM_SIZE, 0, 0);
        let mut rect = windows::Win32::Foundation::RECT::default();
        // SAFETY: `rect` is a local the call fills in.
        let _ = unsafe { GetWindowRect(self.0.0, &mut rect) };
        Px(rect.bottom - rect.top)
    }

    /// Sets the widths of the first parts; the last part takes the rest.
    /// A part keeps the text and icon of the part that had its index.
    pub(crate) fn set_parts(self, widths: &[Px]) {
        let mut right = 0;
        let mut edges: Vec<i32> = widths
            .iter()
            .map(|w| {
                right += w.0;
                right
            })
            .collect();
        edges.push(-1);
        let mut current = vec![0; edges.len()];
        // SAFETY: `current` has room for the `current.len()` integers the
        // message writes at most, and outlives the call.
        let count =
            unsafe { send_pointer(self.0, SB_GETPARTS, current.len(), current.as_mut_ptr()) };
        // Setting the parts draws the whole bar again, which the status
        // updates would make flicker.
        if count == edges.len() as isize && current == edges {
            return;
        }
        // SAFETY: `edges` holds the `edges.len()` integers the message reads,
        // and outlives the call.
        unsafe { send_pointer(self.0, SB_SETPARTS, edges.len(), edges.as_ptr()) };
    }

    /// Sets the text of `part`. A text it shows already is not set again,
    /// because the bar erases and draws the part for every text it is
    /// given, also an unchanged one (DESIGN.md: Painting).
    pub(crate) fn set_text(self, part: usize, text: &str) {
        self.set_text_with_border(part, text, true);
    }

    pub(crate) fn set_text_with_border(self, part: usize, text: &str, border: bool) {
        let style = if border { 0 } else { SBT_NOBORDERS as usize };
        let current = (send(self.0, SB_GETTEXTLENGTHW, part, 0) >> 16) as usize;
        if self.text(part) == text && current == style {
            return;
        }
        let text = wide(text);
        // SAFETY: `text` is NUL-terminated and outlives the call, which
        // copies it.
        unsafe { send_pointer(self.0, SB_SETTEXTW, part | style, text.as_ptr()) };
    }

    pub(crate) fn text(self, part: usize) -> String {
        // The low word is the length; the high word says how it is drawn.
        let length = (send(self.0, SB_GETTEXTLENGTHW, part, 0) & 0xFFFF) as usize;
        let mut text = vec![0u16; length + 1];
        // SAFETY: `text` has room for the part's text and its NUL, and
        // outlives the call, which fills it in.
        unsafe { send_pointer(self.0, SB_GETTEXTW, part, text.as_mut_ptr()) };
        String::from_utf16_lossy(&text[..length])
    }

    /// Shows `glyph` before the text of `part`, at the bar's DPI, in the
    /// system colours of now and only while icons show, or no icon for
    /// `None`, so the owner calls this again when any of them changes.
    pub(crate) fn set_icon(self, part: usize, glyph: Option<Glyph>) {
        let side = icon_glyph_height(self.0.dpi());
        let icon = glyph
            .filter(|_| icons_shown())
            .and_then(|glyph| glyph_icon(glyph, side, system_color(COLOR_BTNTEXT)))
            .unwrap_or_default();
        // The bar keeps the icon without owning it, and `glyph_icon` keeps
        // it alive; one that it shows already is not set again, because each
        // setting draws the part again.
        if send(self.0, SB_GETICON, part, 0) != icon.0 as isize {
            send(self.0, SB_SETICON, part, icon.0 as isize);
        }
    }

    /// The width of a part that shows `text` whole, after `glyph`'s icon
    /// while icons show, as [`Self::set_icon`] shows it.
    pub(crate) fn part_width(self, text: &str, glyph: Option<Glyph>) -> Px {
        let text = Font::of(self.0).map_or(Px(0), |font| TextMeter::new(&font).width(text));
        layout::status_part_width(text, glyph.is_some() && icons_shown(), self.0.dpi())
    }

    /// Where `part` is, in the bar's client coordinates, after
    /// [`Self::set_parts`].
    pub(crate) fn part_rect(self, part: usize) -> layout::Rect {
        let mut rect = RECT::default();
        // SAFETY: `rect` is a local the message fills in.
        unsafe { send_pointer(self.0, SB_GETRECT, part, &raw mut rect) };
        crate::win::from_rect(rect)
    }

    /// Sets the tooltip of `part`, which shows while its text is cut off.
    pub(crate) fn set_tip(self, part: usize, text: &str) {
        let text = wide(text);
        let capacity = (text.len() + 1).min(u16::MAX as usize);
        let mut shown = vec![0u16; capacity];
        // SAFETY: `shown` holds the character count packed into the high
        // word; the low word identifies the part.
        unsafe {
            send_pointer(
                self.0,
                SB_GETTIPTEXTW,
                part | (capacity << 16),
                shown.as_mut_ptr(),
            )
        };
        if shown.starts_with(&text) {
            return;
        }
        // SAFETY: `text` is NUL-terminated and outlives the call, which
        // copies it.
        unsafe { send_pointer(self.0, SB_SETTIPTEXTW, part, text.as_ptr()) };
    }
}

/// The procedure of the status bar, before its own: it passes the
/// notifications of the controls in its parts to its parent, which handles
/// them as its own controls'.
unsafe extern "system" fn status_bar_procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let forwarded = match msg {
        WM_COMMAND => true,
        // SAFETY: a WM_NOTIFY's lparam points to an NMHDR. The bar's own
        // tooltips are popups, not children, and their notifications stay
        // the bar's, which answers them.
        WM_NOTIFY => unsafe { IsChild(hwnd, (*(lparam.0 as *const NMHDR)).hwndFrom) }.as_bool(),
        _ => false,
    };
    match msg {
        _ if forwarded => {
            // SAFETY: the parameters are the ones Windows passed, and the
            // parent decodes them as its own controls'.
            unsafe {
                if let Ok(parent) = GetParent(hwnd) {
                    return SendMessageW(parent, msg, Some(wparam), Some(lparam));
                }
            }
        }
        WM_NCDESTROY => {
            // SAFETY: the status bar gets no message after this one.
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(status_bar_procedure), 0) };
        }
        _ => {}
    }
    // SAFETY: the parameters are the ones Windows passed.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
