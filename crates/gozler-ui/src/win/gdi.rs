//! Fonts, text measurement, the system image list and the little painting
//! the main window does itself: the one-pixel dividers and the
//! window-coloured label background. Every colour comes from `GetSysColor`,
//! so high contrast works.

use super::{Hwnd, to_rect};
use crate::core::layout::{Dpi, Px, Rect};
use windows::Win32::Foundation::{COLORREF, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_3DLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreateFontIndirectW,
    DRAW_TEXT_FORMAT, DT_CALCRECT, DT_NOPREFIX, DT_SINGLELINE, DT_WORDBREAK, DeleteObject,
    DrawTextW, EndPaint, FW_BOLD, FW_SEMIBOLD, FillRect, GetDC, GetObjectW, GetSysColor,
    GetSysColorBrush, GetTextMetricsW, HDC, HFONT, HGDIOBJ, LOGFONTW, PAINTSTRUCT, ReleaseDC,
    SelectObject, SetBkColor, SetTextColor, TEXTMETRICW,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
};
use windows::Win32::UI::Controls::{
    HIMAGELIST, ILD_TRANSPARENT, ImageList_Draw, ImageList_GetIconSize,
};
use windows::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_FLAGS, SHGFI_LARGEICON, SHGFI_SMALLICON, SHGFI_SYSICONINDEX,
    SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};
use windows::core::{HSTRING, PCWSTR, w};

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

    /// This font in semibold and a quarter larger, for the inspector's
    /// heading (DESIGN.md: Look).
    pub(crate) fn heading(&self) -> windows::core::Result<Font> {
        let mut logfont = logfont(self.0)?;
        logfont.lfWeight = FW_SEMIBOLD.0 as i32;
        logfont.lfHeight = logfont.lfHeight * 5 / 4;
        Font::from_logfont(&logfont)
    }

    /// `hwnd`'s font in semibold, as the About dialog shows the version.
    pub(crate) fn semibold_of(hwnd: Hwnd) -> windows::core::Result<Font> {
        let mut logfont = logfont(hwnd.font())?;
        logfont.lfWeight = FW_SEMIBOLD.0 as i32;
        Font::from_logfont(&logfont)
    }

    /// This font in bold, for the matched part of a cell.
    pub(crate) fn bold(&self) -> windows::core::Result<Font> {
        let mut logfont = logfont(self.0)?;
        logfont.lfWeight = FW_BOLD.0 as i32;
        Font::from_logfont(&logfont)
    }

    /// Consolas at the height of `hwnd`'s font, so text laid out in columns
    /// for a console lines up.
    pub(crate) fn monospace_of(hwnd: Hwnd) -> windows::core::Result<Font> {
        let mut logfont = logfont(hwnd.font())?;
        let face: Vec<u16> = "Consolas".encode_utf16().collect();
        logfont.lfFaceName = [0; 32];
        logfont.lfFaceName[..face.len()].copy_from_slice(&face);
        Font::from_logfont(&logfont)
    }

    pub(super) fn handle(&self) -> HFONT {
        self.0
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

fn logfont(font: HFONT) -> windows::core::Result<LOGFONTW> {
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
    Ok(logfont)
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: the font is this value's own, and no control holds it any
        // more (see the type's documentation).
        let _ = unsafe { DeleteObject(HGDIOBJ(self.0.0)) };
    }
}

/// A shell's system image list, of small or of large icons, which Windows
/// owns and shares with every program, so it is never destroyed.
#[derive(Clone, Copy)]
pub(crate) struct ImageList(HIMAGELIST);

impl ImageList {
    /// The system image list of small icons, from `SHGetFileInfoW`.
    pub(crate) fn system_small() -> Option<ImageList> {
        ImageList::system(SHGFI_SMALLICON)
    }

    /// The system image list of large icons, whose indexes are the small
    /// list's.
    pub(crate) fn system_large() -> Option<ImageList> {
        ImageList::system(SHGFI_LARGEICON)
    }

    fn system(size: SHGFI_FLAGS) -> Option<ImageList> {
        let handle = system_icon_info(w!("file"), FILE_ATTRIBUTE_NORMAL, size).0;
        (handle != 0).then_some(ImageList(HIMAGELIST(handle as isize)))
    }

    /// The index in the system image lists of the icon for an item named
    /// `name`, from its extension, without reading the item: a folder's
    /// icon, or the icon of the file type.
    pub(crate) fn system_index(name: &str, is_folder: bool) -> i32 {
        let attributes = if is_folder {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };
        system_icon_info(&HSTRING::from(name), attributes, SHGFI_SMALLICON).1
    }

    pub(super) fn handle(self) -> isize {
        self.0.0
    }

    /// Draws the icon `index` in the middle of `area`.
    pub(super) fn draw(self, dc: HDC, index: i32, area: RECT) {
        let (mut width, mut height) = (0, 0);
        // SAFETY: the system image list lives as long as the process; `dc`
        // is the caller's for the length of its message.
        unsafe {
            let _ = ImageList_GetIconSize(self.0, Some(&mut width), Some(&mut height));
            let x = area.left + (area.right - area.left - width) / 2;
            let y = area.top + (area.bottom - area.top - height) / 2;
            let _ = ImageList_Draw(self.0, index, dc, x, y, ILD_TRANSPARENT);
        }
    }
}

/// `SHGetFileInfoW` for a name the shell does not read
/// (`SHGFI_USEFILEATTRIBUTES`): the image list it returns, and the icon's
/// index in it.
fn system_icon_info(
    name: impl windows::core::Param<PCWSTR>,
    attributes: FILE_FLAGS_AND_ATTRIBUTES,
    size: SHGFI_FLAGS,
) -> (usize, i32) {
    let mut info = SHFILEINFOW::default();
    // SAFETY: `info` is a local of the size passed, which the call fills in.
    let list = unsafe {
        SHGetFileInfoW(
            name,
            attributes,
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES | size,
        )
    };
    (list, info.iIcon)
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

    /// Paints the window's background, fills `dividers` with the divider
    /// colour, and draws `icon`, an icon of an image list, in the middle of
    /// its rectangle.
    pub(crate) fn paint_window(self, dividers: &[Rect], icon: Option<(ImageList, i32, Rect)>) {
        let mut paint = PAINTSTRUCT::default();
        // SAFETY: BeginPaint and EndPaint pair on the same window and
        // structure; the brushes are system brushes, which are never freed.
        unsafe {
            let dc = BeginPaint(self.0, &mut paint);
            let brush = GetSysColorBrush(COLOR_3DLIGHT);
            for divider in dividers {
                FillRect(dc, &to_rect(*divider), brush);
            }
            if let Some((images, index, area)) = icon {
                images.draw(dc, index, to_rect(area));
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
        let mut rect = RECT::default();
        draw_text(self.dc, text, &mut rect, DT_CALCRECT | DT_SINGLELINE);
        Px(rect.right - rect.left)
    }

    /// The height of `text` wrapped at word ends to `width`, as a static
    /// control that shows `&` as itself draws it.
    pub(crate) fn wrapped_height(&self, text: &str, width: Px) -> Px {
        let mut rect = RECT {
            right: width.0,
            ..RECT::default()
        };
        draw_text(
            self.dc,
            text,
            &mut rect,
            DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
        );
        Px(rect.bottom - rect.top)
    }

    /// The font's line height, `tmHeight`.
    pub(crate) fn height(&self) -> Px {
        let mut metrics = TEXTMETRICW::default();
        // SAFETY: `metrics` is a local the call fills in.
        let _ = unsafe { GetTextMetricsW(self.dc, &mut metrics) };
        Px(metrics.tmHeight)
    }
}

/// `DrawTextW` on `dc`, which the caller holds for the call: draws `text` in
/// `rect`, or measures it into `rect` with `DT_CALCRECT`.
pub(super) fn draw_text(dc: HDC, text: &str, rect: &mut RECT, flags: DRAW_TEXT_FORMAT) {
    // DrawTextW reads the text even when its length is 0, so the buffer
    // always holds the NUL after it.
    let mut buffer: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let length = buffer.len() - 1;
    // SAFETY: the slice and `rect` outlive the call; DT_MODIFYSTRING is
    // never passed, so the call writes no text.
    unsafe { DrawTextW(dc, &mut buffer[..length], rect, flags) };
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
    /// colour (DESIGN.md: Look), and returns the brush the message answers
    /// with. It sets the text colour too: when the parent answers the message,
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
