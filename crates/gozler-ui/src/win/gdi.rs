//! Fonts, text measurement, the system image list, the icons, and the
//! colours the windows give their labels and the main window's dividers.
//! Every colour comes from `GetSysColor`, so high contrast works.

use super::Hwnd;
use crate::core::glyph::Glyph;
use crate::core::layout::{Dpi, Px};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use windows::Win32::Foundation::{COLORREF, LRESULT, MAX_PATH, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, COLOR_3DLIGHT, COLOR_BTNTEXT,
    COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT,
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, DEFAULT_CHARSET,
    DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE,
    DT_VCENTER, DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW, FW_BOLD, FW_SEMIBOLD,
    GGI_MARK_NONEXISTING_GLYPHS, GdiFlush, GetDC, GetGlyphIndicesW, GetObjectW, GetSysColor,
    GetSysColorBrush, GetTextFaceW, GetTextMetricsW, HBITMAP, HDC, HFONT, HGDIOBJ, LOGFONTW,
    ReleaseDC, SYS_COLOR_INDEX, SelectObject, SetBkColor, SetBkMode, SetTextColor, TEXTMETRICW,
    TRANSPARENT,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::Controls::{
    HIMAGELIST, ILC_COLOR32, ILD_TRANSPARENT, ImageList_Create, ImageList_Draw, ImageList_GetIcon,
    ImageList_GetIconSize, ImageList_ReplaceIcon,
};
use windows::Win32::UI::Shell::{
    PathCompactPathW, SHFILEINFOW, SHGFI_FLAGS, SHGFI_LARGEICON, SHGFI_SMALLICON,
    SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SHGSI_SYSICONINDEX, SHGetFileInfoW,
    SHGetStockIconInfo, SHSTOCKICONINFO, SIID_FOLDEROPEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, HICON, ICONINFO, NONCLIENTMETRICSW, SPI_GETHIGHCONTRAST,
    SPI_GETNONCLIENTMETRICS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
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

    /// This font a third larger, as a task dialog's main instruction is
    /// beside its text.
    pub(crate) fn instruction(&self) -> windows::core::Result<Font> {
        let mut logfont = logfont(self.0)?;
        logfont.lfHeight = logfont.lfHeight * 4 / 3;
        Font::from_logfont(&logfont)
    }

    /// This font in semibold and a quarter larger, for the inspector's
    /// heading (DESIGN.md: Look).
    pub(crate) fn heading(&self) -> windows::core::Result<Font> {
        let mut logfont = logfont(self.0)?;
        logfont.lfWeight = FW_SEMIBOLD.0 as i32;
        logfont.lfHeight = logfont.lfHeight * 5 / 4;
        Font::from_logfont(&logfont)
    }

    /// A copy of `hwnd`'s font, which a control given another font can get
    /// back.
    pub(crate) fn of(hwnd: Hwnd) -> windows::core::Result<Font> {
        Font::from_logfont(&logfont(hwnd.font())?)
    }

    /// This font in semibold, as Settings' section headings are.
    pub(crate) fn semibold(&self) -> windows::core::Result<Font> {
        let mut logfont = logfont(self.0)?;
        logfont.lfWeight = FW_SEMIBOLD.0 as i32;
        Font::from_logfont(&logfont)
    }

    /// This font in bold, for the matched part of a cell.
    pub(crate) fn bold(&self) -> windows::core::Result<Font> {
        let mut logfont = logfont(self.0)?;
        logfont.lfWeight = FW_BOLD.0 as i32;
        Font::from_logfont(&logfont)
    }

    /// This font struck through, for a row whose entry a delete or a move
    /// took.
    pub(crate) fn struck_out(&self) -> windows::core::Result<Font> {
        let mut logfont = logfont(self.0)?;
        logfont.lfStrikeOut = 1;
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

    pub(super) fn from_logfont(logfont: &LOGFONTW) -> windows::core::Result<Font> {
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

/// The Windows icon font, which draws every [`Glyph`] (DESIGN.md: Look):
/// Segoe Fluent Icons, or Segoe MDL2 Assets on Windows 10, which lacks it.
/// The face is chosen once, and the fonts, which glyphs the face has and
/// the icons' images are kept, because the window draws the same few icons
/// on every paint.
pub(super) struct IconFont {
    face: &'static str,
    /// By height in pixels.
    fonts: HashMap<i32, Font>,
    /// Whether the face draws each glyph asked about.
    present: HashMap<Glyph, bool>,
    /// By glyph, side, and the text and gray colours they are drawn in. A
    /// button holds its image list without owning it, so each list lives as
    /// long as the thread.
    images: HashMap<(Glyph, i32, u32, u32), HIMAGELIST>,
    /// By glyphs, side and colour: the image lists of toolbars and lists,
    /// one image for each glyph. A control holds them without owning them,
    /// so each lives as long as the thread.
    lists: HashMap<(Vec<Glyph>, i32, u32), HIMAGELIST>,
    /// By glyph, side and colour: the icons of status bar parts and static
    /// controls, which hold an icon without owning it, so each lives as
    /// long as the thread.
    icons: HashMap<(Glyph, i32, u32), Icon>,
    /// By glyph, side and colour: the icons drawn on a DC and the menu
    /// items' bitmaps. A menu holds an item's bitmap without owning it, so
    /// each bitmap lives as long as the thread.
    bitmaps: HashMap<(Glyph, i32, u32), Bitmap>,
}

const FLUENT_ICONS: &str = "Segoe Fluent Icons";
const MDL2_ASSETS: &str = "Segoe MDL2 Assets";
/// How many times larger [`glyph_coverage`] draws a glyph before it averages
/// it down to the icon's size.
const OVERSAMPLE: i32 = 4;

thread_local! {
    /// The window's thread draws every icon.
    static ICON_FONT: RefCell<Option<IconFont>> = const { RefCell::new(None) };
    static ICONS_SHOWN: Cell<bool> = const { Cell::new(true) };
}

/// Shows or hides the icons beside text, as Settings > General's Show icons
/// says (DESIGN.md: Look): those of menu items, buttons with a caption,
/// list items, group captions and the status bar. An icon-only button
/// keeps its icon, because it has no text. The caller then has the
/// controls that show icons draw again.
pub(crate) fn show_icons(shown: bool) {
    ICONS_SHOWN.set(shown);
}

/// Whether icons show beside text; see [`show_icons`].
pub(crate) fn icons_shown() -> bool {
    ICONS_SHOWN.get()
}

impl IconFont {
    pub(super) fn with<R>(use_font: impl FnOnce(&mut IconFont) -> R) -> R {
        ICON_FONT.with_borrow_mut(|font| {
            use_font(font.get_or_insert_with(|| IconFont {
                face: if installed(FLUENT_ICONS) {
                    FLUENT_ICONS
                } else {
                    MDL2_ASSETS
                },
                fonts: HashMap::new(),
                present: HashMap::new(),
                images: HashMap::new(),
                lists: HashMap::new(),
                icons: HashMap::new(),
                bitmaps: HashMap::new(),
            }))
        })
    }

    fn font(&mut self, height: Px) -> Option<HFONT> {
        if let Entry::Vacant(entry) = self.fonts.entry(height.0) {
            entry.insert(icon_font(self.face, height).ok()?);
        }
        self.fonts.get(&height.0).map(Font::handle)
    }

    /// Whether the face draws `glyph`; `dc` holds one of its fonts.
    fn has(&mut self, dc: HDC, glyph: Glyph) -> bool {
        *self
            .present
            .entry(glyph)
            .or_insert_with(|| font_has(dc, glyph))
    }

    /// `glyph`, `side` pixels square, in `color`: a 32-bit bitmap with
    /// premultiplied alpha, which menus draw. `None` when
    /// the icon font lacks the glyph or GDI fails.
    fn bitmap(&mut self, glyph: Glyph, side: Px, color: COLORREF) -> Option<HBITMAP> {
        let key = (glyph, side.0, color.0);
        if let Some(bitmap) = self.bitmaps.get(&key) {
            return Some(bitmap.handle);
        }
        let coverage = glyph_coverage(self, glyph, side)?;
        let mut bitmap = Bitmap::new(side)?;
        let [red, green, blue, _] = color.0.to_le_bytes();
        let premultiplied = |channel: u8, alpha: u8| u32::from(channel) * u32::from(alpha) / 255;
        for (pixel, alpha) in bitmap.pixels_mut().iter_mut().zip(coverage) {
            *pixel = (u32::from(alpha) << 24)
                | (premultiplied(red, alpha) << 16)
                | (premultiplied(green, alpha) << 8)
                | premultiplied(blue, alpha);
        }
        let handle = bitmap.handle;
        self.bitmaps.insert(key, bitmap);
        Some(handle)
    }
}

/// The face `face` at `height` pixels, antialiased without ClearType:
/// ClearType draws an icon's thin upright strokes in colour, and an image
/// with transparency cannot hold its colours.
fn icon_font(face: &str, height: Px) -> windows::core::Result<Font> {
    let mut logfont = LOGFONTW {
        lfHeight: -height.0,
        lfCharSet: DEFAULT_CHARSET,
        lfQuality: ANTIALIASED_QUALITY,
        ..LOGFONTW::default()
    };
    let face: Vec<u16> = face.encode_utf16().collect();
    logfont.lfFaceName[..face.len()].copy_from_slice(&face);
    Font::from_logfont(&logfont)
}

/// Whether Windows has the face `face`: it gives another face for one it
/// does not have.
fn installed(face: &str) -> bool {
    icon_font(face, Px(16)).is_ok_and(|font| TextMeter::new(&font).face() == face)
}

/// Whether the font that `dc` holds draws `glyph`, instead of the box that
/// a missing glyph shows.
fn font_has(dc: HDC, glyph: Glyph) -> bool {
    let mut buffer = [0u16; 2];
    let text = glyph.code_point().encode_utf16(&mut buffer);
    let mut indexes = [0u16; 2];
    // SAFETY: `text` and `indexes` are locals at least as long as passed.
    let converted = unsafe {
        GetGlyphIndicesW(
            dc,
            PCWSTR(text.as_ptr()),
            text.len() as i32,
            indexes.as_mut_ptr(),
            GGI_MARK_NONEXISTING_GLYPHS,
        )
    };
    converted as usize == text.len() && indexes[..text.len()].iter().all(|i| *i != 0xffff)
}

/// The colour of the system colour `index`.
pub(super) fn system_color(index: SYS_COLOR_INDEX) -> COLORREF {
    // SAFETY: plain values.
    COLORREF(unsafe { GetSysColor(index) })
}

/// The image list of a push button that shows `glyph` before its caption,
/// `side` pixels square: one image for each of the button's states, in
/// their order, in the button text colour, and gray while it is disabled.
/// `None` while icons are hidden (see [`show_icons`]), or when the icon font
/// lacks the glyph or GDI fails.
pub(super) fn button_images(glyph: Glyph, side: Px) -> Option<HIMAGELIST> {
    if !icons_shown() {
        return None;
    }
    lone_button_images(glyph, side)
}

/// [`button_images`] also while icons are hidden, for a button that shows
/// only its icon.
pub(super) fn lone_button_images(glyph: Glyph, side: Px) -> Option<HIMAGELIST> {
    let text = system_color(COLOR_BTNTEXT);
    let gray = system_color(COLOR_GRAYTEXT);
    IconFont::with(|icons| {
        let key = (glyph, side.0, text.0, gray.0);
        if let Some(images) = icons.images.get(&key) {
            return Some(*images);
        }
        let coverage = glyph_coverage(icons, glyph, side)?;
        let normal = Icon::colored(&coverage, side, text)?;
        let disabled = Icon::colored(&coverage, side, gray)?;
        // SAFETY: plain values; the list copies each icon.
        let images = unsafe { ImageList_Create(side.0, side.0, ILC_COLOR32, 6, 0) };
        if images.is_invalid() {
            return None;
        }
        // Normal, hot, pressed, disabled, default and stylus hot.
        for icon in [&normal, &normal, &normal, &disabled, &normal, &normal] {
            // SAFETY: as above.
            unsafe { ImageList_ReplaceIcon(images, -1, icon.0) };
        }
        icons.images.insert(key, images);
        Some(images)
    })
}

/// An image list of `glyphs`, `side` pixels square, in `color`, one image
/// for each in their order, as a toolbar or a list view takes it. A glyph
/// the icon font lacks gets a blank image, which keeps the others' indexes.
/// `None` when GDI fails.
pub(super) fn glyph_list(glyphs: &[Glyph], side: Px, color: COLORREF) -> Option<HIMAGELIST> {
    IconFont::with(|icons| {
        let key = (glyphs.to_vec(), side.0, color.0);
        if let Some(images) = icons.lists.get(&key) {
            return Some(*images);
        }
        let count = glyphs.len() as i32;
        // SAFETY: plain values; the list copies each icon.
        let images = unsafe { ImageList_Create(side.0, side.0, ILC_COLOR32, count, 0) };
        if images.is_invalid() {
            return None;
        }
        for &glyph in glyphs {
            let icon = glyph_coverage(icons, glyph, side)
                .and_then(|coverage| Icon::colored(&coverage, side, color));
            let handle = icon.as_ref().map(|icon| icon.0).unwrap_or_default();
            // SAFETY: as above.
            unsafe { ImageList_ReplaceIcon(images, -1, handle) };
        }
        icons.lists.insert(key, images);
        Some(images)
    })
}

/// The icon of `glyph`, `side` pixels square, in `color`, which lives as
/// long as the thread, as a status bar part or a static control needs.
/// `None` when the icon font lacks the glyph or GDI fails.
pub(super) fn glyph_icon(glyph: Glyph, side: Px, color: COLORREF) -> Option<HICON> {
    IconFont::with(|icons| {
        let key = (glyph, side.0, color.0);
        if let Some(icon) = icons.icons.get(&key) {
            return Some(icon.0);
        }
        let coverage = glyph_coverage(icons, glyph, side)?;
        let icon = Icon::colored(&coverage, side, color)?;
        let handle = icon.0;
        icons.icons.insert(key, icon);
        Some(handle)
    })
}

/// The bitmap of a menu item that shows `glyph` before its text, `side`
/// pixels square, in `color`. It is a 32-bit bitmap with premultiplied
/// alpha, because Windows draws a menu without its visual style when an
/// item's bitmap is `HBMMENU_CALLBACK` or owner-drawn, and then hides the
/// text of unavailable items. `None` while icons are hidden (see
/// [`show_icons`]), or when the icon font lacks the glyph or GDI fails.
pub(super) fn menu_bitmap(glyph: Glyph, side: Px, color: COLORREF) -> Option<HBITMAP> {
    if !icons_shown() {
        return None;
    }
    IconFont::with(|icons| icons.bitmap(glyph, side, color))
}

/// How much of each pixel of a `side`-pixel square `glyph` covers, from 0
/// to 255, row by row; `None` when the icon font lacks it.
///
/// The glyph is drawn at [`OVERSAMPLE`] times the side and each block of
/// pixels averaged into one, because GDI's own antialiasing at icon sizes
/// draws diagonal strokes much lighter than upright ones. Every icon is
/// smoothed here, so all of them have the same weight.
pub(super) fn glyph_coverage(icons: &mut IconFont, glyph: Glyph, side: Px) -> Option<Vec<u8>> {
    let large = Px(side.0 * OVERSAMPLE);
    let font = icons.font(large)?;
    let bitmap = Bitmap::new(large)?;
    // SAFETY: the memory DC holds the bitmap and the font only inside this
    // block and is deleted at its end; the pixels are read after GdiFlush.
    unsafe {
        let dc = CreateCompatibleDC(None);
        let previous_bitmap = SelectObject(dc, HGDIOBJ(bitmap.handle.0));
        let previous_font = SelectObject(dc, HGDIOBJ(font.0));
        let coverage = icons.has(dc, glyph).then(|| {
            SetTextColor(dc, COLORREF(0x00ff_ffff));
            SetBkMode(dc, TRANSPARENT);
            let mut rect = RECT {
                right: large.0,
                bottom: large.0,
                ..RECT::default()
            };
            draw_text(
                dc,
                &glyph.code_point().to_string(),
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            let _ = GdiFlush();
            // White on black: each colour channel holds the coverage.
            let pixels = bitmap.pixels();
            let (side, large, block) = (side.0 as usize, large.0 as usize, OVERSAMPLE as usize);
            let covered = |x: usize, y: usize| u32::from((pixels[y * large + x] >> 8) as u8);
            (0..side * side)
                .map(|at| {
                    let (x, y) = (at % side * block, at / side * block);
                    let sum: u32 = (0..block * block)
                        .map(|i| covered(x + i % block, y + i / block))
                        .sum();
                    (sum / (block * block) as u32) as u8
                })
                .collect()
        });
        SelectObject(dc, previous_font);
        SelectObject(dc, previous_bitmap);
        let _ = DeleteDC(dc);
        coverage
    }
}

/// A 32-bit top-down bitmap whose pixels the program reads and writes,
/// deleted on drop.
struct Bitmap {
    handle: HBITMAP,
    /// Its pixels as 0xAARRGGBB, which live as long as the bitmap.
    bits: *mut u32,
    length: usize,
}

impl Bitmap {
    /// A black `side`-pixel square.
    fn new(side: Px) -> Option<Bitmap> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: side.0,
                biHeight: -side.0,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits = std::ptr::null_mut();
        // SAFETY: `info` describes the bitmap, and the call points `bits`
        // at its zeroed pixels.
        let handle =
            unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
        Some(Bitmap {
            handle,
            bits: bits.cast(),
            length: usize::try_from(side.0 * side.0).unwrap_or(0),
        })
    }

    fn pixels(&self) -> &[u32] {
        // SAFETY: `bits` points to `length` pixels while the bitmap lives.
        unsafe { std::slice::from_raw_parts(self.bits, self.length) }
    }

    fn pixels_mut(&mut self) -> &mut [u32] {
        // SAFETY: as in `pixels`, borrowed once.
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.length) }
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: the bitmap is this value's own and no DC holds it.
        let _ = unsafe { DeleteObject(HGDIOBJ(self.handle.0)) };
    }
}

/// An icon, destroyed on drop.
pub(super) struct Icon(pub(super) HICON);

impl Icon {
    /// A `side`-pixel square of `color` whose opacity is `coverage`.
    pub(super) fn colored(coverage: &[u8], side: Px, color: COLORREF) -> Option<Icon> {
        let mut bitmap = Bitmap::new(side)?;
        // COLORREF is 0x00BBGGRR, a pixel 0xAARRGGBB.
        let rgb = ((color.0 & 0xff) << 16) | (color.0 & 0xff00) | ((color.0 >> 16) & 0xff);
        for (pixel, alpha) in bitmap.pixels_mut().iter_mut().zip(coverage) {
            *pixel = (u32::from(*alpha) << 24) | rgb;
        }
        // An icon with transparency ignores its mask, which is still
        // required: a monochrome bitmap, zeroed so it is defined.
        let row = (side.0 + 15) / 16 * 2;
        let mask_bits = vec![0u8; usize::try_from(row * side.0).ok()?];
        // SAFETY: `mask_bits` holds the monochrome bitmap's rows. The icon
        // copies both bitmaps, so the mask is deleted after the call.
        unsafe {
            let mask = CreateBitmap(side.0, side.0, 1, 1, Some(mask_bits.as_ptr().cast()));
            if mask.is_invalid() {
                return None;
            }
            let info = ICONINFO {
                fIcon: true.into(),
                hbmMask: mask,
                hbmColor: bitmap.handle,
                ..ICONINFO::default()
            };
            let icon = CreateIconIndirect(&info).ok().map(Icon);
            let _ = DeleteObject(HGDIOBJ(mask.0));
            icon
        }
    }

    /// The handle, which the caller destroys from now on.
    pub(super) fn into_handle(self) -> HICON {
        let handle = self.0;
        std::mem::forget(self);
        handle
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: the icon is this value's own.
        let _ = unsafe { DestroyIcon(self.0) };
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

    /// A copy of the icon `index`, which the caller owns.
    pub(super) fn icon(self, index: i32) -> Option<Icon> {
        // SAFETY: the system image list lives as long as the process; the
        // copy it returns is the caller's.
        let icon = unsafe { ImageList_GetIcon(self.0, index, ILD_TRANSPARENT) };
        (!icon.is_invalid()).then_some(Icon(icon))
    }

    /// The index in the system image lists of Windows' open folder icon,
    /// which is already there, so nothing is added to the lists that the
    /// shell keeps in step; `None` when the shell does not say.
    pub(crate) fn open_folder_index() -> Option<i32> {
        let mut info = SHSTOCKICONINFO {
            cbSize: size_of::<SHSTOCKICONINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `info` is a local with its size set, which the call fills.
        unsafe { SHGetStockIconInfo(SIID_FOLDEROPEN, SHGSI_SYSICONINDEX, &mut info) }.ok()?;
        Some(info.iSysImageIndex)
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
    /// Gives the control `font` and marks it, to paint when the update ends
    /// ([`super::present`]) instead of at once.
    pub(crate) fn set_font(self, font: &Font) {
        if self.font() == font.0 {
            return;
        }
        // SAFETY: WM_SETFONT takes a font handle and a redraw flag.
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                self.0,
                windows::Win32::UI::WindowsAndMessaging::WM_SETFONT,
                Some(WPARAM(font.0.0 as usize)),
                Some(windows::Win32::Foundation::LPARAM(0)),
            );
        }
        self.invalidate();
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

    /// The width of `text` drawn as it is, `&` included, as a value shows it.
    pub(crate) fn plain_width(&self, text: &str) -> Px {
        let mut rect = RECT::default();
        draw_text(
            self.dc,
            text,
            &mut rect,
            DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
        );
        Px(rect.right - rect.left)
    }

    /// `path` shortened to `width` as Windows shortens a path, with
    /// `PathCompactPathW`: its middle gives way to "...", so its start and
    /// its last part stay.
    pub(crate) fn compact_path(&self, path: &str, width: Px) -> String {
        let mut buffer = [0u16; MAX_PATH as usize];
        let units: Vec<u16> = path.encode_utf16().collect();
        let room = buffer.len() - 1;
        if units.len() <= room {
            buffer[..units.len()].copy_from_slice(&units);
        } else {
            // The call takes at most MAX_PATH - 1 characters, so a longer
            // path loses its middle here first, as the call would.
            let last = units.iter().rposition(|&u| u == u16::from(b'\\'));
            let tail = &units[last.unwrap_or(0)..];
            let tail = &tail[..tail.len().min(room - 1)];
            let head = room - 1 - tail.len();
            buffer[..head].copy_from_slice(&units[..head]);
            buffer[head] = '…' as u16;
            buffer[head + 1..room].copy_from_slice(tail);
        }
        // When the call fails, the buffer's text shows as it is.
        // SAFETY: the DC and `buffer`, NUL-terminated, outlive the call.
        let _ = unsafe { PathCompactPathW(Some(self.dc), &mut buffer, width.0.max(0) as u32) };
        let length = buffer.iter().position(|&u| u == 0).unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..length])
    }

    /// The name of the face Windows chose for the font.
    fn face(&self) -> String {
        let mut face = [0u16; 32];
        // SAFETY: `face` is a local whose length the call is given.
        let length = unsafe { GetTextFaceW(self.dc, Some(&mut face)) };
        let length = usize::try_from(length).unwrap_or(0).saturating_sub(1);
        String::from_utf16_lossy(&face[..length.min(face.len())])
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
        self.on_window(COLOR_WINDOWTEXT)
    }

    /// [`window_background`](Self::window_background) with gray text, for a
    /// hint beside the content.
    pub(crate) fn gray_text(self) -> LRESULT {
        self.on_window(COLOR_GRAYTEXT)
    }

    /// [`window_background`](Self::window_background) with the muted text
    /// colour, for a part of a value that steps back.
    pub(crate) fn muted_text(self) -> LRESULT {
        self.on_window(muted_text())
    }

    /// The system highlight colours, as the active panel's path box shows
    /// them while it does not have the focus (DESIGN.md: A panel).
    pub(crate) fn highlight(self) -> LRESULT {
        self.on(COLOR_HIGHLIGHTTEXT, COLOR_HIGHLIGHT)
    }

    /// The brush of a divider between the main window's panes, which fills
    /// the whole control.
    pub(crate) fn divider(self) -> LRESULT {
        // SAFETY: system brushes are never freed.
        LRESULT(unsafe { GetSysColorBrush(COLOR_3DLIGHT) }.0 as isize)
    }

    fn on_window(self, text: SYS_COLOR_INDEX) -> LRESULT {
        self.on(text, COLOR_WINDOW)
    }

    fn on(self, text: SYS_COLOR_INDEX, ground: SYS_COLOR_INDEX) -> LRESULT {
        // SAFETY: a stale DC makes the calls fail; system brushes are never
        // freed.
        unsafe {
            SetTextColor(self.0, COLORREF(GetSysColor(text)));
            SetBkColor(self.0, COLORREF(GetSysColor(ground)));
            LRESULT(GetSysColorBrush(ground).0 as isize)
        }
    }
}

/// The colour of text that steps back from the content: gray, or the normal
/// text colour in high contrast, where gray text means unavailable.
pub(super) fn muted_text() -> SYS_COLOR_INDEX {
    if high_contrast() {
        COLOR_WINDOWTEXT
    } else {
        COLOR_GRAYTEXT
    }
}

/// The colour of a disabled toolbar button's icon on `ground`, as Windows
/// 11's command bars draw a disabled command: the text colour at 36 % over
/// the ground. `COLOR_GRAYTEXT` is too close to the text colour to read as
/// unavailable. High contrast keeps it, because its scheme chooses that
/// colour for unavailable text.
pub(super) fn disabled_color(ground: SYS_COLOR_INDEX) -> COLORREF {
    if high_contrast() {
        return system_color(COLOR_GRAYTEXT);
    }
    let text = system_color(COLOR_WINDOWTEXT).0;
    let ground = system_color(ground).0;
    // 92 of 255, WinUI's disabled text opacity.
    let channel = |shift: u32| {
        let text = (text >> shift) & 0xFF;
        let ground = (ground >> shift) & 0xFF;
        ((text * 92 + ground * (255 - 92) + 127) / 255) << shift
    };
    COLORREF(channel(0) | channel(8) | channel(16))
}

/// Whether a high contrast theme is on, which draws with the system colours
/// and without visual styles.
pub(super) fn high_contrast() -> bool {
    let mut contrast = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..HIGHCONTRASTW::default()
    };
    // SAFETY: `contrast` is a local with its size set, as the call needs.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            Some((&raw mut contrast).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_ok() && contrast.dwFlags.contains(HCF_HIGHCONTRASTON)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window names only glyphs that both icon fonts draw, so it shows
    /// the same icons on Windows 10 and 11. A glyph either font lacks would
    /// show no icon there. Segoe Fluent Icons is checked only where it is
    /// installed, as the window uses it only there; a Windows Server image
    /// may not have it.
    #[test]
    fn both_icon_fonts_draw_every_glyph() {
        let faces = [FLUENT_ICONS, MDL2_ASSETS]
            .into_iter()
            .filter(|face| *face == MDL2_ASSETS || installed(face));
        for face in faces {
            let font = icon_font(face, Px(16)).expect("GDI makes a font");
            let meter = TextMeter::new(&font);
            assert_eq!(meter.face(), face, "the machine has {face}");
            let missing: Vec<Glyph> = Glyph::ALL
                .into_iter()
                .filter(|glyph| !font_has(meter.dc, *glyph))
                .collect();
            assert!(missing.is_empty(), "{face} lacks {missing:?}");
        }
    }
}
