//! About's acrylic surface, adapted from TinyTorrent's desktop splash and
//! popup. The dialog and its native buttons retain ownership, accessibility,
//! Tab order and activation; this module supplies their appearance only.

use super::gdi::Font;
use super::{Hwnd, dialog, wide};
use crate::core::captions::DialogItem;
use crate::core::ids::{ControlId, IconId};
use crate::core::layout::{Px, Rect};
use std::cell::{Cell, RefCell};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT,
    DC_BRUSH, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, EndPaint, FillRect,
    GetMonitorInfoW, GetStockObject, GetSysColor, HBRUSH, HDC, LOGFONTW, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromRect, PAINTSTRUCT, RGBQUAD, SelectObject, SetBkMode, SetDCBrushColor,
    SetTextColor, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::Controls::{
    BP_PAINTPARAMS, BPBF_TOPDOWNDIB, BPPF_ERASE, BeginBufferedPaint, BufferedPaintInit,
    BufferedPaintSetAlpha, BufferedPaintUnInit, CloseThemeData, DRAWITEMSTRUCT, DTT_COMPOSITED,
    DTT_TEXTCOLOR, DTTOPTS, DrawThemeTextEx, EndBufferedPaint, GetBufferedPaintBits, HTHEME,
    ODS_FOCUS, ODS_HOTLIGHT, ODS_NOFOCUSRECT, ODS_SELECTED, ODT_BUTTON, OpenThemeData,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    DI_NORMAL, DrawIconEx, GetClientRect, GetParent, HICON, HTCAPTION, IMAGE_ICON, LR_SHARED,
    LoadImageW, MINMAXINFO, SC_MAXIMIZE, SC_SIZE, SPI_GETHIGHCONTRAST, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOZORDER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetWindowPos,
    SystemParametersInfoW, WM_DPICHANGED, WM_DRAWITEM, WM_DWMCOMPOSITIONCHANGED, WM_ERASEBKGND,
    WM_GETMINMAXINFO, WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_PAINT,
    WM_SETTINGCHANGE, WM_SIZE, WM_SYSCOLORCHANGE, WM_SYSCOMMAND, WM_THEMECHANGED,
};
use windows::core::{BOOL, PCWSTR, s, w};

const WIDTH: i32 = 900;
const HEIGHT: i32 = 424;
const LEFT_TOP: i32 = (HEIGHT - 304) / 2;

pub(crate) struct Surface {
    state: Box<State>,
}

struct State {
    window: Cell<Option<Hwnd>>,
    dpi: Cell<u32>,
    fonts: RefCell<Fonts>,
    palette: Cell<Palette>,
    buffered: bool,
    transparency: Cell<bool>,
}

struct Fonts {
    body: Font,
    small: Font,
    title: Font,
    glyph: Font,
}

impl Fonts {
    fn new(dpi: u32) -> windows::core::Result<Self> {
        let make = |height, weight, face: &str| {
            let mut font = LOGFONTW {
                lfHeight: -scale(height, dpi),
                lfWeight: weight,
                ..Default::default()
            };
            let face = wide(face);
            font.lfFaceName[..face.len()].copy_from_slice(&face);
            Font::from_logfont(&font)
        };
        Ok(Self {
            body: make(14, 400, "Segoe UI")?,
            small: make(13, 400, "Segoe UI")?,
            title: make(34, 600, "Segoe UI")?,
            glyph: make(20, 400, "Segoe MDL2 Assets")?,
        })
    }
}

impl Surface {
    pub(crate) fn attach(window: Hwnd) -> windows::core::Result<Self> {
        // SAFETY: the dialog is live on this thread.
        let dpi = unsafe { GetDpiForWindow(window.0) }.max(96);
        let surface = Self {
            state: Box::new(State {
                window: Cell::new(Some(window)),
                dpi: Cell::new(dpi),
                fonts: RefCell::new(Fonts::new(dpi)?),
                palette: Cell::new(Palette::read(false)),
                // SAFETY: balanced by Surface's drop on this UI thread.
                buffered: unsafe { BufferedPaintInit() }.is_ok(),
                transparency: Cell::new(false),
            }),
        };
        // SAFETY: the box keeps the callback's state at a fixed address until
        // the subclass is removed, either at NCDESTROY or in Surface's drop.
        if !unsafe {
            SetWindowSubclass(
                window.0,
                Some(procedure),
                0,
                std::ptr::from_ref(surface.state.as_ref()) as usize,
            )
        }
        .as_bool()
        {
            return Err(windows::core::Error::from_thread());
        }
        let owner = {
            // SAFETY: the dialog has its main window as owner.
            unsafe { GetParent(window.0) }.ok().map(Hwnd)
        };
        let bounds = owner.unwrap_or(window).screen_rect();
        surface.state.fit(window, dpi, super::to_rect(bounds))?;
        surface.state.apply_theme(window);
        Ok(surface)
    }

    pub(crate) fn set_transparency(&self, enabled: bool) {
        self.state.transparency.set(enabled);
        if let Some(window) = self.state.window.get() {
            self.state.apply_theme(window);
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        if let Some(window) = self.state.window.take() {
            // SAFETY: the callback cannot read the box after removal.
            let _ = unsafe { RemoveWindowSubclass(window.0, Some(procedure), 0) };
        }
        if self.state.buffered {
            // SAFETY: balances the successful initialization on this thread.
            let _ = unsafe { BufferedPaintUnInit() };
        }
    }
}

impl State {
    fn fit(&self, window: Hwnd, dpi: u32, anchor: RECT) -> windows::core::Result<()> {
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: the monitor and anchor are valid locals with the expected sizes.
        let work = if unsafe {
            GetMonitorInfoW(
                MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
        }
        .as_bool()
        {
            monitor.rcWork
        } else {
            anchor
        };
        let available = ((work.right - work.left) * 96 / WIDTH)
            .min((work.bottom - work.top) * 96 / HEIGHT)
            .max(1) as u32;
        let dpi = dpi.min(available);
        if dpi != self.dpi.get() {
            self.fonts.replace(Fonts::new(dpi)?);
            self.dpi.set(dpi);
        }
        let width = scale(WIDTH, dpi);
        let height = scale(HEIGHT, dpi);
        let left = (anchor.left + (anchor.right - anchor.left - width) / 2)
            .clamp(work.left, work.right - width);
        let top = (anchor.top + (anchor.bottom - anchor.top - height) / 2)
            .clamp(work.top, work.bottom - height);
        // SAFETY: the frame is retained for DWM rounding but NCCALCSIZE removes
        // its chrome; fit the complete composition without scrollbars.
        unsafe {
            SetWindowPos(
                window.0,
                None,
                left,
                top,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
        }
    }

    fn layout(&self, window: Hwnd) {
        let dpi = self.dpi.get();
        let size = window.client_size();
        let right = size.width.0;
        let bottom = size.height.0;
        let margin = 24;
        let button_height = 32;
        let card_height = 76;
        let card_step = card_height + 12;
        let cards_bottom = HEIGHT - margin - button_height - margin;
        let cards_top = cards_bottom - 2 * card_step - card_height;
        let rect = |x, y, width, height| Rect {
            x: Px(scale(x, dpi)),
            y: Px(scale(y, dpi)),
            width: Px(scale(width, dpi)),
            height: Px(scale(height, dpi)),
        };
        let mut moves = Vec::new();
        for (id, bounds) in [
            (ControlId::AboutIcon.id(), rect(48, LEFT_TOP, 64, 64)),
            (ControlId::AboutName.id(), rect(128, LEFT_TOP, 290, 64)),
            (
                ControlId::AboutDescription.id(),
                rect(48, LEFT_TOP + 86, 350, 100),
            ),
            (
                ControlId::AboutVersion.id(),
                rect(48, LEFT_TOP + 201, 350, 22),
            ),
            (
                ControlId::AboutService.id(),
                rect(48, LEFT_TOP + 227, 350, 22),
            ),
            (
                ControlId::AboutCopyright.id(),
                rect(48, LEFT_TOP + 282, 350, 22),
            ),
            (
                ControlId::AboutCompany.id(),
                rect(482, cards_top, 370, card_height),
            ),
            (
                ControlId::AboutLink.id(),
                rect(482, cards_top + card_step, 370, card_height),
            ),
            (
                ControlId::AboutCredits.id(),
                rect(482, cards_top + 2 * card_step, 370, card_height),
            ),
            (
                ControlId::AboutClose.id(),
                Rect {
                    x: Px(right - scale(44, dpi)),
                    y: Px(scale(12, dpi)),
                    width: Px(scale(32, dpi)),
                    height: Px(scale(32, dpi)),
                },
            ),
            (
                DialogItem::Cancel.id(),
                Rect {
                    x: Px(right - scale(margin + 88, dpi)),
                    y: Px(bottom - scale(margin + button_height, dpi)),
                    width: Px(scale(88, dpi)),
                    height: Px(scale(button_height, dpi)),
                },
            ),
        ] {
            if let Some(control) = dialog::item(window, id) {
                moves.push((control, bounds));
            }
        }
        super::place_children(&moves, &[]);
        for (control, _) in moves {
            control.invalidate();
        }
    }

    fn apply_theme(&self, window: Hwnd) {
        let mut palette = Palette::read(self.transparency.get());
        palette.blur = set_blur(window, self.buffered && palette.blur) && palette.blur;
        let dark = BOOL(0);
        let corners = DWMWCP_ROUND;
        // SAFETY: these attributes read their correctly sized values only
        // during the call; older Windows versions simply reject them.
        unsafe {
            let _ = DwmSetWindowAttribute(
                window.0,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&raw const dark).cast(),
                size_of::<BOOL>() as u32,
            );
            let _ = DwmSetWindowAttribute(
                window.0,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&raw const corners).cast(),
                size_of_val(&corners) as u32,
            );
        }
        self.palette.set(palette);
        self.layout(window);
        window.invalidate();
    }

    fn paint(&self, window: Hwnd) {
        let mut paint = PAINTSTRUCT::default();
        let mut bounds = RECT::default();
        // SAFETY: the locals are valid for this complete paint pass.
        let dc = unsafe {
            GetClientRect(window.0, &mut bounds).ok();
            BeginPaint(window.0, &mut paint)
        };
        {
            let canvas = Canvas::new(window, dc, bounds, self.palette.get(), self.buffered);
            let dpi = self.dpi.get();
            let line = RECT {
                left: scale(450, dpi),
                top: scale(LEFT_TOP, dpi),
                right: scale(451, dpi),
                bottom: scale(344, dpi),
            };
            canvas.fill(line, self.palette.get().foreground, 24, 0);
        }
        // SAFETY: ends this window's matching BeginPaint.
        let _ = unsafe { EndPaint(window.0, &paint) };
    }

    fn draw_control(&self, window: Hwnd, item: &DRAWITEMSTRUCT) {
        let palette = self.palette.get();
        let canvas = Canvas::new(window, item.hDC, item.rcItem, palette, self.buffered);
        let dpi = self.dpi.get();
        let fonts = self.fonts.borrow();
        let id = item.CtlID as u16;
        let text = Hwnd(item.hwndItem).text();
        let mut bounds = item.rcItem;
        if item.CtlType == ODT_BUTTON {
            let close = id == ControlId::AboutClose.id();
            let opacity = if item.itemState.0 & ODS_SELECTED.0 != 0 {
                8
            } else if item.itemState.0 & ODS_HOTLIGHT.0 != 0 {
                26
            } else if close {
                0
            } else {
                15
            };
            canvas.fill(bounds, palette.foreground, opacity, scale(4, dpi));
            if palette.contrast {
                canvas.outline(bounds, palette.edge, scale(1, dpi), scale(4, dpi));
            }
            if item.itemState.0 & ODS_FOCUS.0 != 0 && item.itemState.0 & ODS_NOFOCUSRECT.0 == 0 {
                canvas.outline(bounds, palette.focus, scale(2, dpi), scale(4, dpi));
            }
            if close || id == DialogItem::Cancel.id() {
                canvas.text(
                    if close { "\u{e8bb}" } else { &text },
                    bounds,
                    if close { &fonts.glyph } else { &fonts.body },
                    palette.foreground,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            } else {
                let glyph = if id == ControlId::AboutCompany.id() {
                    "\u{e80f}"
                } else if id == ControlId::AboutCredits.id() {
                    "\u{e716}"
                } else {
                    "\u{e943}"
                };
                let icon = RECT {
                    left: bounds.left + scale(16, dpi),
                    right: bounds.left + scale(40, dpi),
                    ..bounds
                };
                canvas.text(
                    glyph,
                    icon,
                    &fonts.glyph,
                    palette.foreground,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
                let arrow = RECT {
                    left: bounds.right - scale(32, dpi),
                    right: bounds.right - scale(12, dpi),
                    ..bounds
                };
                canvas.text(
                    "\u{e8a7}",
                    arrow,
                    &fonts.glyph,
                    palette.foreground,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
                bounds.left += scale(56, dpi);
                bounds.right -= scale(40, dpi);
                let (title, detail) = text.split_once('\n').unwrap_or((&text, ""));
                if detail.is_empty() {
                    canvas.text(
                        title,
                        bounds,
                        &fonts.body,
                        palette.foreground,
                        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
                    );
                    return;
                }
                bounds.top += scale(12, dpi);
                let title_bounds = RECT {
                    bottom: bounds.top + scale(21, dpi),
                    ..bounds
                };
                canvas.text(
                    title,
                    title_bounds,
                    &fonts.body,
                    palette.foreground,
                    DT_SINGLELINE | DT_NOPREFIX,
                );
                bounds.top += scale(22, dpi);
                bounds.bottom -= scale(8, dpi);
                canvas.text(
                    detail,
                    bounds,
                    &fonts.small,
                    palette.secondary,
                    DT_WORDBREAK | DT_NOPREFIX,
                );
            }
        } else if id == ControlId::AboutIcon.id() {
            // SAFETY: LR_SHARED leaves resource ownership with Windows. The
            // icon's alpha is preserved by the buffered DIB, as in the splash.
            unsafe {
                if let Ok(icon) = LoadImageW(
                    GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
                    PCWSTR(usize::from(IconId::Gozler.id()) as *const u16),
                    IMAGE_ICON,
                    scale(64, dpi),
                    scale(64, dpi),
                    LR_SHARED,
                ) {
                    let _ = DrawIconEx(
                        canvas.dc,
                        bounds.left,
                        bounds.top,
                        HICON(icon.0),
                        scale(64, dpi),
                        scale(64, dpi),
                        0,
                        None,
                        DI_NORMAL,
                    );
                }
            }
        } else {
            let title = id == ControlId::AboutName.id();
            let secondary = id == ControlId::AboutVersion.id()
                || id == ControlId::AboutService.id()
                || id == ControlId::AboutCopyright.id();
            canvas.text(
                &text,
                bounds,
                if title {
                    &fonts.title
                } else if secondary {
                    &fonts.small
                } else {
                    &fonts.body
                },
                if secondary {
                    palette.secondary
                } else {
                    palette.foreground
                },
                if title {
                    DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX
                } else {
                    DT_WORDBREAK | DT_NOPREFIX
                },
            );
        }
    }
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    first: WPARAM,
    second: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: the box is retained until this subclass is removed.
    let state = unsafe { &*(data as *const State) };
    // SAFETY: these are the original parameters of this native message.
    let _update = unsafe { super::present::enter(hwnd, message, second) };
    let window = Hwnd(hwnd);
    match message {
        WM_NCCALCSIZE | WM_NCLBUTTONDBLCLK => return LRESULT(0),
        WM_NCHITTEST => return LRESULT(HTCAPTION as isize),
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            state.paint(window);
            return LRESULT(0);
        }
        WM_DRAWITEM if second.0 != 0 => {
            // SAFETY: DRAWITEM supplies a DRAWITEMSTRUCT for this call.
            state.draw_control(window, unsafe { &*(second.0 as *const DRAWITEMSTRUCT) });
            return LRESULT(1);
        }
        WM_SIZE => state.layout(window),
        WM_SYSCOMMAND if matches!(first.0 as u32 & 0xfff0, SC_SIZE | SC_MAXIMIZE) => {
            return LRESULT(0);
        }
        WM_GETMINMAXINFO => {
            // SAFETY: GETMINMAXINFO supplies a writable MINMAXINFO.
            let limits = unsafe { &mut *(second.0 as *mut MINMAXINFO) };
            limits.ptMinTrackSize.x = scale(WIDTH, state.dpi.get());
            limits.ptMinTrackSize.y = scale(HEIGHT, state.dpi.get());
            limits.ptMaxTrackSize = limits.ptMinTrackSize;
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            let dpi = (first.0 & 0xffff) as u32;
            // SAFETY: DPICHANGED supplies the suggested screen rectangle.
            let bounds = unsafe { &*(second.0 as *const RECT) };
            let _ = state.fit(window, dpi, *bounds);
            state.layout(window);
            window.invalidate();
            return LRESULT(0);
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED | WM_DWMCOMPOSITIONCHANGED => {
            state.apply_theme(window);
        }
        WM_NCDESTROY => {
            state.window.set(None);
            // SAFETY: no message may read this state after NCDESTROY.
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(procedure), 0) };
        }
        _ => {}
    }
    // SAFETY: forwards the original parameters to the native dialog.
    unsafe { DefSubclassProc(hwnd, message, first, second) }
}

fn scale(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + 48) / 96) as i32
}

#[derive(Clone, Copy)]
struct Palette {
    foreground: COLORREF,
    background: COLORREF,
    secondary: COLORREF,
    focus: COLORREF,
    edge: COLORREF,
    tint: u8,
    contrast: bool,
    blur: bool,
}

impl Palette {
    fn read(transparency: bool) -> Self {
        let mut contrast = HIGHCONTRASTW {
            cbSize: size_of::<HIGHCONTRASTW>() as u32,
            ..Default::default()
        };
        // SAFETY: the local has the size and layout the system requires.
        let _ = unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                contrast.cbSize,
                Some((&raw mut contrast).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        let contrast = contrast.dwFlags.contains(HCF_HIGHCONTRASTON);
        // SAFETY: these are system-owned color values, not GDI objects.
        let color = |index| COLORREF(unsafe { GetSysColor(index) });
        let foreground = if contrast {
            color(COLOR_WINDOWTEXT)
        } else {
            COLORREF(0)
        };
        let background = if contrast {
            color(COLOR_WINDOW)
        } else {
            COLORREF(0xf3f3f3)
        };
        Self {
            foreground,
            background,
            secondary: if contrast {
                color(COLOR_GRAYTEXT)
            } else {
                mix(foreground, background, 180)
            },
            focus: if contrast {
                color(COLOR_HIGHLIGHT)
            } else {
                foreground
            },
            edge: color(COLOR_BTNTEXT),
            tint: 0xd9,
            contrast,
            blur: !contrast && transparency,
        }
    }
}

pub(crate) fn transparency(_worker: &super::work::OffThread) -> bool {
    let mut value = 1u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: reads one DWORD into a correctly sized local.
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("EnableTransparency"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    result.is_err() || value != 0
}

fn set_blur(window: Hwnd, enabled: bool) -> bool {
    #[repr(C)]
    struct Accent {
        state: u32,
        flags: u32,
        color: u32,
        animation: u32,
    }
    #[repr(C)]
    struct Composition {
        attribute: i32,
        data: *mut Accent,
        size: usize,
    }
    type Setter = unsafe extern "system" fn(HWND, *mut Composition) -> BOOL;
    // SAFETY: user32 remains loaded for the window's entire lifetime.
    let setter = unsafe {
        GetModuleHandleW(w!("user32.dll"))
            .ok()
            .and_then(|module| GetProcAddress(module, s!("SetWindowCompositionAttribute")))
    };
    let Some(setter) = setter else {
        return false;
    };
    // SAFETY: this is the export's ABI, also used by TinyTorrent and rawBit.
    let setter: Setter = unsafe { std::mem::transmute(setter) };
    let mut accent = Accent {
        state: if enabled { 3 } else { 0 },
        flags: 0,
        color: 0xccffffff,
        animation: 0,
    };
    let mut composition = Composition {
        attribute: 19,
        data: &mut accent,
        size: size_of::<Accent>(),
    };
    // SAFETY: the synchronous setter reads these stack values during its call.
    unsafe { setter(window.0, &mut composition) }.as_bool()
}

struct Canvas {
    dc: HDC,
    buffer: isize,
    theme: HTHEME,
    bounds: RECT,
    background: COLORREF,
}

impl Canvas {
    fn new(window: Hwnd, target: HDC, bounds: RECT, palette: Palette, buffered: bool) -> Self {
        let parameters = BP_PAINTPARAMS {
            cbSize: size_of::<BP_PAINTPARAMS>() as u32,
            dwFlags: BPPF_ERASE,
            ..Default::default()
        };
        let mut dc = HDC::default();
        let buffer = if buffered {
            // SAFETY: all buffers and parameters outlive this synchronous call.
            unsafe {
                BeginBufferedPaint(target, &bounds, BPBF_TOPDOWNDIB, Some(&parameters), &mut dc)
            }
        } else {
            0
        };
        if buffer == 0 {
            dc = target;
        }
        // SAFETY: the theme is closed when this paint pass ends.
        let theme = unsafe { OpenThemeData(Some(window.0), w!("CompositedWindow::Window")) };
        let alpha = if palette.blur && buffer != 0 && !theme.is_invalid() {
            palette.tint
        } else {
            255
        };
        let canvas = Self {
            dc,
            buffer,
            theme,
            bounds,
            background: palette.background,
        };
        canvas.solid(bounds, mix(palette.background, COLORREF(0), alpha));
        if buffer != 0 {
            // SAFETY: this is this canvas's live paint buffer.
            let _ = unsafe { BufferedPaintSetAlpha(buffer, None, alpha) };
        }
        canvas
    }

    fn solid(&self, bounds: RECT, color: COLORREF) {
        // SAFETY: DC_BRUSH is system-owned; only this DC's brush color changes.
        unsafe {
            SetDCBrushColor(self.dc, color);
            FillRect(self.dc, &bounds, HBRUSH(GetStockObject(DC_BRUSH).0));
        }
    }

    fn fill(&self, bounds: RECT, color: COLORREF, opacity: u8, radius: i32) {
        self.shape(bounds, color, opacity, radius, 0);
    }

    fn outline(&self, bounds: RECT, color: COLORREF, width: i32, radius: i32) {
        self.shape(bounds, color, 255, radius, width);
    }

    fn shape(&self, bounds: RECT, color: COLORREF, opacity: u8, radius: i32, border: i32) {
        let mut pixels = std::ptr::null_mut::<RGBQUAD>();
        let mut stride = 0;
        if self.buffer == 0
            // SAFETY: the buffer owns its DIB until this canvas is dropped.
            || unsafe { GetBufferedPaintBits(self.buffer, &mut pixels, &mut stride) }.is_err()
        {
            let fill = mix(color, self.background, opacity);
            if border == 0 {
                self.solid(bounds, fill);
            } else {
                for side in [
                    RECT {
                        bottom: bounds.top + border,
                        ..bounds
                    },
                    RECT {
                        top: bounds.bottom - border,
                        ..bounds
                    },
                    RECT {
                        right: bounds.left + border,
                        ..bounds
                    },
                    RECT {
                        left: bounds.right - border,
                        ..bounds
                    },
                ] {
                    self.solid(side, fill);
                }
            }
            return;
        }
        let inner = RECT {
            left: bounds.left + border,
            top: bounds.top + border,
            right: bounds.right - border,
            bottom: bounds.bottom - border,
        };
        for y in bounds.top.max(self.bounds.top)..bounds.bottom.min(self.bounds.bottom) {
            for x in bounds.left.max(self.bounds.left)..bounds.right.min(self.bounds.right) {
                let mut alpha = coverage(bounds, radius, x, y);
                if border > 0 {
                    alpha -= coverage(inner, (radius - border).max(0), x, y);
                }
                let alpha = alpha * f64::from(opacity) / 255.0;
                // SAFETY: the clipped coordinates address this top-down DIB;
                // stride is its row width, returned with the pixel pointer.
                let pixel = unsafe {
                    &mut *pixels
                        .offset(((y - self.bounds.top) * stride + x - self.bounds.left) as isize)
                };
                let blend = |over: u8, under: u8| {
                    (f64::from(over) * alpha + f64::from(under) * (1.0 - alpha) + 0.5) as u8
                };
                pixel.rgbRed = blend(color.0 as u8, pixel.rgbRed);
                pixel.rgbGreen = blend((color.0 >> 8) as u8, pixel.rgbGreen);
                pixel.rgbBlue = blend((color.0 >> 16) as u8, pixel.rgbBlue);
                pixel.rgbReserved = blend(255, pixel.rgbReserved);
            }
        }
    }

    fn text(
        &self,
        text: &str,
        mut bounds: RECT,
        font: &Font,
        color: COLORREF,
        format: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
    ) {
        // SAFETY: the owned font outlives this call; the previous font is
        // restored before the canvas releases its device context.
        let previous = unsafe { SelectObject(self.dc, font.handle().into()) };
        if self.buffer != 0 && !self.theme.is_invalid() {
            let options = DTTOPTS {
                dwSize: size_of::<DTTOPTS>() as u32,
                dwFlags: DTT_COMPOSITED | DTT_TEXTCOLOR,
                crText: color,
                ..Default::default()
            };
            let text: Vec<_> = text.encode_utf16().collect();
            // SAFETY: the text, rectangle and options outlive this call.
            let _ = unsafe {
                DrawThemeTextEx(
                    self.theme,
                    self.dc,
                    0,
                    0,
                    &text,
                    format,
                    &mut bounds,
                    Some(&options),
                )
            };
        } else {
            // SAFETY: these change only this paint pass's device context.
            unsafe {
                SetTextColor(self.dc, color);
                SetBkMode(self.dc, TRANSPARENT);
            }
            super::gdi::draw_text(self.dc, text, &mut bounds, format);
        }
        // SAFETY: restores the object returned by SelectObject above.
        unsafe { SelectObject(self.dc, previous) };
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: these resources belong to this paint pass, on this thread.
        unsafe {
            if !self.theme.is_invalid() {
                let _ = CloseThemeData(self.theme);
            }
            if self.buffer != 0 {
                let _ = EndBufferedPaint(self.buffer, true);
            }
        }
    }
}

fn coverage(bounds: RECT, radius: i32, x: i32, y: i32) -> f64 {
    if x < bounds.left || x >= bounds.right || y < bounds.top || y >= bounds.bottom {
        return 0.0;
    }
    let x = f64::from(x) + 0.5;
    let y = f64::from(y) + 0.5;
    let dx = 0.0f64
        .max(f64::from(bounds.left + radius) - x)
        .max(x - f64::from(bounds.right - radius));
    let dy = 0.0f64
        .max(f64::from(bounds.top + radius) - y)
        .max(y - f64::from(bounds.bottom - radius));
    if dx > 0.0 && dy > 0.0 {
        (f64::from(radius) + 0.5 - dx.hypot(dy)).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

fn mix(over: COLORREF, under: COLORREF, alpha: u8) -> COLORREF {
    let mut color = 0;
    for shift in [0, 8, 16] {
        let top = (over.0 >> shift) & 255;
        let bottom = (under.0 >> shift) & 255;
        color |= ((top * u32::from(alpha) + bottom * (255 - u32::from(alpha))) / 255) << shift;
    }
    COLORREF(color)
}
