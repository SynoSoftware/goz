//! Fonts, text measurement and the little painting the main window does
//! itself: the one-pixel dividers and the window-coloured label background.
//! Every colour comes from `GetSysColor`, so high contrast works.

use super::{Hwnd, to_rect};
use crate::core::layout::{Dpi, Px, Rect};
use windows::Win32::Foundation::{COLORREF, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_3DLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreateFontIndirectW, DT_CALCRECT,
    DT_SINGLELINE, DeleteObject, DrawTextW, EndPaint, FW_SEMIBOLD, FillRect, GetDC, GetObjectW,
    GetSysColor, GetSysColorBrush, GetTextMetricsW, HDC, HFONT, HGDIOBJ, LOGFONTW, PAINTSTRUCT,
    ReleaseDC, SelectObject, SetBkColor, SetTextColor, TEXTMETRICW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

/// An owned GDI font, deleted on drop. A control given it with
/// [`Hwnd::set_font`] does not own it, so the owner replaces the control's
/// font before it drops this one.
pub(crate) struct Font(HFONT);

impl Font {
    /// The Windows message font at `dpi`, which the controls use.
    pub(crate) fn message(dpi: Dpi) -> windows::core::Result<Font> {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        // SAFETY: `metrics` is a local with its size set, as the call needs.
        unsafe {
            windows::Win32::UI::HiDpi::SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                metrics.cbSize,
                Some((&raw mut metrics).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0).0,
                dpi.get(),
            )
        }?;
        Font::from_logfont(&metrics.lfMessageFont)
    }

    /// `hwnd`'s font in semibold, as the About dialog shows the version.
    pub(crate) fn semibold_of(hwnd: Hwnd) -> windows::core::Result<Font> {
        let font = hwnd.font();
        let mut logfont = LOGFONTW::default();
        // SAFETY: `logfont` is a local of the size passed.
        let read = unsafe {
            GetObjectW(
                HGDIOBJ(font.0),
                size_of::<LOGFONTW>() as i32,
                Some((&raw mut logfont).cast()),
            )
        };
        if read == 0 {
            return Err(windows::core::Error::from_thread());
        }
        logfont.lfWeight = FW_SEMIBOLD.0 as i32;
        Font::from_logfont(&logfont)
    }

    fn from_logfont(logfont: &LOGFONTW) -> windows::core::Result<Font> {
        // SAFETY: `logfont` is a complete LOGFONTW.
        let font = unsafe { CreateFontIndirectW(logfont) };
        if font.is_invalid() {
            return Err(windows::core::Error::from_thread());
        }
        Ok(Font(font))
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: the font is this value's own, and no control holds it any
        // more (see the type's documentation).
        let _ = unsafe { DeleteObject(HGDIOBJ(self.0.0)) };
    }
}

impl Hwnd {
    /// Gives the control `font` and redraws it.
    pub(crate) fn set_font(self, font: &Font) {
        // SAFETY: WM_SETFONT takes a font handle and a redraw flag.
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.0,
                windows::Win32::UI::WindowsAndMessaging::WM_SETFONT,
                Some(WPARAM(font.0.0 as usize)),
                Some(windows::Win32::Foundation::LPARAM(1)),
            );
        }
    }

    fn font(self) -> HFONT {
        // SAFETY: WM_GETFONT has no parameters and returns a font handle.
        let font = unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.0,
                windows::Win32::UI::WindowsAndMessaging::WM_GETFONT,
                None,
                None,
            )
        };
        HFONT(font.0 as *mut _)
    }

    /// Paints the window's background and fills `dividers` with the divider
    /// colour.
    pub(crate) fn paint_dividers(self, dividers: &[Rect]) {
        let mut paint = PAINTSTRUCT::default();
        // SAFETY: BeginPaint and EndPaint pair on the same window and
        // structure; the brushes are system brushes, which are never freed.
        unsafe {
            let dc = BeginPaint(self.0, &mut paint);
            let brush = GetSysColorBrush(COLOR_3DLIGHT);
            for divider in dividers {
                FillRect(dc, &to_rect(*divider), brush);
            }
            let _ = EndPaint(self.0, &paint);
        }
    }
}

/// Measures text in one font, as its controls draw it: `DrawTextW` removes
/// an access-key `&` as a control does.
pub(crate) struct TextMeter {
    dc: HDC,
    previous: HGDIOBJ,
}

impl TextMeter {
    pub(crate) fn new(font: &Font) -> TextMeter {
        // SAFETY: the screen DC is released, with its font restored, on drop.
        unsafe {
            let dc = GetDC(None);
            let previous = SelectObject(dc, HGDIOBJ(font.0.0));
            TextMeter { dc, previous }
        }
    }

    pub(crate) fn width(&self, text: &str) -> Px {
        let mut text: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT::default();
        // SAFETY: DT_CALCRECT only measures into `rect`; `text` outlives the
        // call.
        unsafe { DrawTextW(self.dc, &mut text, &mut rect, DT_CALCRECT | DT_SINGLELINE) };
        Px(rect.right - rect.left)
    }

    /// The font's line height, `tmHeight`.
    pub(crate) fn height(&self) -> Px {
        let mut metrics = TEXTMETRICW::default();
        // SAFETY: `metrics` is a local the call fills in.
        let _ = unsafe { GetTextMetricsW(self.dc, &mut metrics) };
        Px(metrics.tmHeight)
    }
}

impl Drop for TextMeter {
    fn drop(&mut self) {
        // SAFETY: restores the DC's font and releases the DC that `new` got.
        unsafe {
            SelectObject(self.dc, self.previous);
            ReleaseDC(None, self.dc);
        }
    }
}

/// `WM_CTLCOLORSTATIC`'s device context, for a label or a check box.
pub(crate) struct StaticColors(HDC);

impl StaticColors {
    pub(crate) fn from_wparam(wparam: WPARAM) -> StaticColors {
        StaticColors(HDC(wparam.0 as *mut _))
    }

    /// Draws the control on the window colour instead of the grey dialog
    /// colour (spec: Look), and returns the brush the message answers with.
    /// It sets the text colour too: when the parent answers the message,
    /// Windows sets none, and the default black can match a high contrast
    /// theme's window colour.
    pub(crate) fn window_background(self) -> LRESULT {
        // SAFETY: a stale DC makes the calls fail; system brushes are never
        // freed.
        unsafe {
            SetTextColor(self.0, COLORREF(GetSysColor(COLOR_WINDOWTEXT)));
            SetBkColor(self.0, COLORREF(GetSysColor(COLOR_WINDOW)));
            LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize)
        }
    }
}
