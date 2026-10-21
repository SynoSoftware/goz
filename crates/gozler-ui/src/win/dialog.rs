//! Modal dialogs from the .rc templates, their pages, and message boxes.
//! Windows runs a template dialog's modal loop, its Tab order, `Enter` and
//! `Esc`, and scales it to its monitor's DPI. [`run_modal`] sets the
//! dialog's title and captions, gives its push buttons one width, focuses
//! the control that its [`DialogProc`] names, keeps the fonts the dialog's
//! controls use until it ends, and ends it on an OK or Cancel that its
//! procedure leaves alone. A page, such as Settings > General, is a child
//! dialog of its own template inside the dialog ([`create_page`]), whose
//! controls the dialog's procedure handles.

use super::controls::{Button, TreeItem, has_image};
use super::gdi::{Font, TextMeter, icons_shown};
use super::{Hwnd, wide};
use crate::core::captions::{ButtonIcon, DialogCaptions, Guard, Question, QuestionBody};
use crate::core::commands::Keystroke;
use crate::core::file_operations::Choices;
use crate::core::glyph::Glyph;
use crate::core::ids::DialogId;
use crate::core::layout::{self, Px, Rect};
use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    DTN_DATETIMECHANGE, EMF_CENTERED, LVN_GETEMPTYMARKUP, LVN_GETINFOTIPW, LVN_ITEMCHANGED,
    NM_CLICK, NM_DBLCLK, NM_RETURN, NMHDR, NMLVEMPTYMARKUP, NMLVGETINFOTIPW, NMTREEVIEWW,
    NMTVITEMCHANGE, TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TASKDIALOGCONFIG_1,
    TD_INFORMATION_ICON, TD_WARNING_ICON, TDCBF_CANCEL_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
    TDF_POSITION_RELATIVE_TO_WINDOW, TDF_USE_COMMAND_LINKS, TVE_EXPAND, TVIS_STATEIMAGEMASK,
    TVN_ITEMCHANGEDW, TVN_ITEMEXPANDINGW, TaskDialogIndirect,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, BS_TYPEMASK, CBN_SELCHANGE, CreateDialogParamW,
    DialogBoxParamW, EN_CHANGE, EN_KILLFOCUS, EN_SETFOCUS, EndDialog, EnumChildWindows, GWL_STYLE,
    GetClassNameW, GetDlgCtrlID, GetDlgItem, GetParent, GetWindowLongPtrW, IDCANCEL, IDOK,
    MB_ICONERROR, MB_OK, MapDialogRect, MessageBoxW, SendMessageW, SetWindowLongPtrW,
    WINDOW_LONG_PTR_INDEX, WM_COMMAND, WM_INITDIALOG, WM_NOTIFY, WM_TIMER,
};
use windows::core::{BOOL, HSTRING, PCWSTR};

/// `DWLP_USER`: the dialog's pointer-sized slot after its result and its
/// procedure.
const DWLP_USER: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(2 * size_of::<isize>() as i32);

/// `DWLP_MSGRESULT`: the dialog's answer to the message it handled.
const DWLP_MSGRESULT: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(0);

/// The private message that tells a dialog that the main window holds a
/// reply for it ([`DialogMessage::Reply`]).
const REPLY: u32 = super::wake::WAKE + 6;

/// A dialog message decoded for its handler.
pub(crate) enum DialogMessage {
    /// A click on the button or check box `id`, or OK and Cancel, which are
    /// 1 and 2, from `Enter` and `Esc`.
    Command {
        id: u16,
    },
    /// The control `id` holds another value: an edit control's or a hotkey
    /// control's text, a list's selection, or a date picker's date or check
    /// box.
    Edited {
        id: u16,
    },
    /// The edit control `id` got the focus, or lost it.
    Focus {
        id: u16,
        focused: bool,
    },
    /// A [`super::shortcut::Capture`] caught keys that a shortcut can be:
    /// the others besides the Windows key, if it was held.
    KeysCaptured {
        windows: bool,
        keystroke: Keystroke,
    },
    /// A click or `Enter` on the SysLink control `id`.
    LinkActivated {
        id: u16,
    },
    /// A double-click on the list view or tree view `id`.
    DoubleClicked {
        id: u16,
    },
    /// The dialog's timer `id`.
    Timer {
        id: usize,
    },
    /// [`post_reply`] said that a reply waits for the dialog.
    Reply,
    /// The item `item` of a tree view changed as `change` says.
    Tree {
        item: TreeItem,
        change: TreeChange,
    },
    Other,
}

/// What happened to a tree view's item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TreeChange {
    /// It is about to open and show its children.
    Expanding,
    /// Its check box changed, by a click, the space bar or the dialog
    /// itself.
    Checked,
}

/// What a dialog sets up when it opens, besides its captions.
pub(crate) struct Opened {
    /// The control the focus starts on.
    pub(crate) focus: u16,
    /// The fonts the dialog gave its controls. They live until the dialog
    /// ends, because `WM_SETFONT` does not take ownership.
    pub(crate) fonts: Vec<Font>,
}

/// One dialog's message handling.
pub(crate) trait DialogProc {
    /// Sets what the captions do not, such as a text built from a fact, a
    /// list's items or a font. Runs after the captions are set.
    fn init(&self, dialog: Hwnd) -> Opened;
    /// Answers `true` for a message it handled. An OK or Cancel that it
    /// leaves ends the dialog with that result.
    fn message(&self, dialog: Hwnd, message: DialogMessage) -> bool;
    /// What the empty list view `id` says in its empty area, as File
    /// Explorer says that a folder is empty; `None` for nothing.
    fn empty_text(&self, _id: u16) -> Option<String> {
        None
    }
    /// The tooltip of row `row` of the list view `id`, for a list that
    /// shows info tips (`ReportList::show_info_tips`); `None` for none.
    fn info_tip(&self, _id: u16, _row: usize) -> Option<String> {
        None
    }
    /// The captions a push button can show after the dialog opens, each
    /// with whether the administrator shield shows beside it, so the
    /// buttons are wide enough for them from the start.
    fn later_captions(&self) -> Vec<(String, bool)> {
        Vec::new()
    }
    /// Places what follows the push buttons' one width `width`, such as
    /// Settings' status bar Cancel. Runs once, after [`DialogProc::init`].
    fn buttons_sized(&self, _dialog: Hwnd, _width: Px) {}
    /// The icon of the push button `id` if it is a Clear button, which
    /// shows only that icon, in its middle, while icons show, and its
    /// caption while they are hidden (DESIGN.md: Look); `None` for any
    /// other button.
    fn clear_glyph(&self, _id: u16) -> Option<Glyph> {
        None
    }
}

/// How a dialog ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogEnd {
    Ok,
    Cancel,
}

/// What the dialog procedure reads through `DWLP_USER` while the dialog
/// runs.
struct Running<'a, D> {
    state: &'a D,
    captions: &'a DialogCaptions,
    fonts: RefCell<Vec<Font>>,
}

/// Runs the dialog `dialog` owned by `owner`, until it ends. `state` is
/// borrowed for the dialog's whole life, because this returns only after
/// the dialog closes. Fails when Windows cannot create the dialog, such as
/// when it runs out of resources.
pub(crate) fn run_modal<D: DialogProc>(
    owner: Hwnd,
    dialog: DialogId,
    captions: &DialogCaptions,
    state: &D,
) -> windows::core::Result<DialogEnd> {
    let running = Running {
        state,
        captions,
        fonts: RefCell::new(Vec::new()),
    };
    // SAFETY: WM_INITDIALOG stores the pointer to `running`, which outlives
    // the modal loop that this call runs, and its fonts outlive the
    // dialog's controls.
    let result = unsafe {
        DialogBoxParamW(
            GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
            PCWSTR(usize::from(dialog.id()) as *const u16),
            Some(owner.0),
            Some(procedure::<D>),
            LPARAM(std::ptr::from_ref(&running) as isize),
        )
    };
    if super::window::quit_pending() {
        // A WM_QUIT ended the dialog with its default button's ID, which
        // is not the person's choice.
        return Ok(DialogEnd::Cancel);
    }
    match result {
        // 0 is an owner that is not a window, -1 any other failure.
        0 | -1 => Err(windows::core::Error::from_thread()),
        result if result == IDOK.0 as isize => Ok(DialogEnd::Ok),
        _ => Ok(DialogEnd::Cancel),
    }
}

unsafe extern "system" fn procedure<D: DialogProc>(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    if msg == WM_INITDIALOG {
        // SAFETY: lparam is the pointer to the `Running` that run_modal
        // passed.
        unsafe { SetWindowLongPtrW(hwnd, DWLP_USER, lparam.0) };
    }
    // SAFETY: DWLP_USER holds the pointer from WM_INITDIALOG, or 0 for the
    // few messages before it.
    let running = unsafe { GetWindowLongPtrW(hwnd, DWLP_USER) } as *const Running<D>;
    if running.is_null() {
        return 0;
    }
    // SAFETY: run_modal's local keeps the `Running` alive while the dialog
    // runs.
    let running = unsafe { &*running };
    let dialog = Hwnd(hwnd);
    let message = match msg {
        WM_INITDIALOG => {
            open(dialog, running);
            // False: `open` has set the focus.
            return 0;
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xffff) as u16;
            match ((wparam.0 >> 16) & 0xffff) as u32 {
                BN_CLICKED => DialogMessage::Command { id },
                // A list box's LBN_SELCHANGE is a combo box's CBN_SELCHANGE.
                EN_CHANGE | CBN_SELCHANGE => DialogMessage::Edited { id },
                EN_SETFOCUS => DialogMessage::Focus { id, focused: true },
                EN_KILLFOCUS => DialogMessage::Focus { id, focused: false },
                _ => DialogMessage::Other,
            }
        }
        WM_NOTIFY => {
            // SAFETY: WM_NOTIFY's lparam points to an NMHDR.
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            let id = header.idFrom as u16;
            match header.code {
                LVN_GETEMPTYMARKUP => {
                    let Some(text) = running.state.empty_text(id) else {
                        return 0;
                    };
                    // SAFETY: LVN_GETEMPTYMARKUP's lparam points to an
                    // NMLVEMPTYMARKUP that the list reads once this returns.
                    let markup = unsafe { &mut *(lparam.0 as *mut NMLVEMPTYMARKUP) };
                    markup.dwFlags = EMF_CENTERED;
                    // The list reads the text as markup, in which only a
                    // link's tags mean something, and no caption has one.
                    let text = wide(&text);
                    let length = text.len().min(markup.szMarkup.len());
                    markup.szMarkup[..length].copy_from_slice(&text[..length]);
                    markup.szMarkup[length - 1] = 0;
                    // SAFETY: plain values; true shows the text.
                    unsafe { SetWindowLongPtrW(hwnd, DWLP_MSGRESULT, 1) };
                    return 1;
                }
                LVN_GETINFOTIPW => {
                    // SAFETY: LVN_GETINFOTIPW's lparam points to an
                    // NMLVGETINFOTIPW whose pszText holds cchTextMax
                    // characters, which the list reads once this returns.
                    let request = unsafe { &mut *(lparam.0 as *mut NMLVGETINFOTIPW) };
                    let size = usize::try_from(request.cchTextMax).unwrap_or(0);
                    if let Ok(row) = usize::try_from(request.iItem)
                        && size > 0
                        && let Some(text) = running.state.info_tip(id, row)
                    {
                        let text = wide(&text);
                        let length = text.len().min(size);
                        // SAFETY: as above, pszText holds `size` characters.
                        let buffer =
                            unsafe { std::slice::from_raw_parts_mut(request.pszText.0, size) };
                        buffer[..length].copy_from_slice(&text[..length]);
                        buffer[length - 1] = 0;
                    }
                    return 0;
                }
                NM_CLICK | NM_RETURN => DialogMessage::LinkActivated { id },
                NM_DBLCLK => DialogMessage::DoubleClicked { id },
                DTN_DATETIMECHANGE | LVN_ITEMCHANGED => DialogMessage::Edited { id },
                TVN_ITEMEXPANDINGW => {
                    // SAFETY: TVN_ITEMEXPANDINGW's lparam points to an
                    // NMTREEVIEWW.
                    let notice = unsafe { &*(lparam.0 as *const NMTREEVIEWW) };
                    if notice.action == TVE_EXPAND {
                        DialogMessage::Tree {
                            item: TreeItem(notice.itemNew.hItem),
                            change: TreeChange::Expanding,
                        }
                    } else {
                        DialogMessage::Other
                    }
                }
                TVN_ITEMCHANGEDW => {
                    // SAFETY: TVN_ITEMCHANGEDW's lparam points to an
                    // NMTVITEMCHANGE.
                    let notice = unsafe { &*(lparam.0 as *const NMTVITEMCHANGE) };
                    if (notice.uStateNew ^ notice.uStateOld) & TVIS_STATEIMAGEMASK.0 != 0 {
                        DialogMessage::Tree {
                            item: TreeItem(notice.hItem),
                            change: TreeChange::Checked,
                        }
                    } else {
                        DialogMessage::Other
                    }
                }
                _ => DialogMessage::Other,
            }
        }
        WM_TIMER => DialogMessage::Timer { id: wparam.0 },
        REPLY => DialogMessage::Reply,
        super::shortcut::CAPTURED => match super::shortcut::captured(wparam, lparam) {
            Some((windows, keystroke)) => DialogMessage::KeysCaptured { windows, keystroke },
            None => DialogMessage::Other,
        },
        _ => DialogMessage::Other,
    };
    let command = match message {
        DialogMessage::Command { id, .. } => Some(id),
        DialogMessage::Edited { .. }
        | DialogMessage::Focus { .. }
        | DialogMessage::KeysCaptured { .. }
        | DialogMessage::LinkActivated { .. }
        | DialogMessage::DoubleClicked { .. }
        | DialogMessage::Timer { .. }
        | DialogMessage::Reply
        | DialogMessage::Tree { .. }
        | DialogMessage::Other => None,
    };
    if running.state.message(dialog, message) {
        return 1;
    }
    match command {
        Some(1) => end(dialog, DialogEnd::Ok),
        Some(2) => end(dialog, DialogEnd::Cancel),
        _ => return 0,
    }
    1
}

/// Sets the captions, lets the dialog set up the rest, and focuses the
/// control it names.
fn open<D: DialogProc>(dialog: Hwnd, running: &Running<D>) {
    dialog.set_text(&running.captions.title);
    for (id, text) in &running.captions.items {
        if let Some(control) = item(dialog, *id) {
            control.set_text(text);
        }
    }
    show_glyphs(dialog, running.captions);
    let opened = running.state.init(dialog);
    let width = size_buttons(dialog, running.state);
    running.state.buttons_sized(dialog, width);
    running.fonts.replace(opened.fonts);
    if let Some(control) = item(dialog, opened.focus) {
        control.focus();
    }
}

/// Creates the hidden page `page` inside `dialog`, with its captions set.
/// The dialog's procedure receives the page's `WM_COMMAND` and `WM_NOTIFY`,
/// so one [`DialogProc`] handles every page; the pages' control IDs differ
/// from each other's and from the dialog's. The page lives until its
/// dialog ends.
pub(crate) fn create_page(
    dialog: Hwnd,
    page: DialogId,
    captions: &DialogCaptions,
) -> windows::core::Result<Hwnd> {
    // SAFETY: the template is a resource of this module, a child dialog
    // whose procedure reads no state.
    let hwnd = unsafe {
        CreateDialogParamW(
            GetModuleHandleW(PCWSTR::null()).ok().map(Into::into),
            PCWSTR(usize::from(page.id()) as *const u16),
            Some(dialog.0),
            Some(page_procedure),
            LPARAM(0),
        )
    }?;
    let page = Hwnd(hwnd);
    for (id, text) in &captions.items {
        if let Some(control) = item(page, *id) {
            control.set_text(text);
        }
    }
    show_glyphs(page, captions);
    Ok(page)
}

/// Gives the push buttons of `dialog`, a dialog or a page, the icons of
/// their captions, again when icons are shown or hidden.
pub(crate) fn show_glyphs(dialog: Hwnd, captions: &DialogCaptions) {
    for (id, glyph) in &captions.glyphs {
        if let Some(button) = Button::of_dialog(dialog, *id) {
            button.set_icon(Some(ButtonIcon::Glyph(*glyph)));
        }
    }
}

unsafe extern "system" fn page_procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match msg {
        // False: the hidden page takes no focus when it is created.
        WM_INITDIALOG => 0,
        WM_COMMAND | WM_NOTIFY => {
            // SAFETY: the dialog receives the message as the page did, with
            // its pointer valid while this call runs.
            unsafe {
                let Ok(dialog) = GetParent(hwnd) else {
                    return 0;
                };
                let answer = SendMessageW(dialog, msg, Some(wparam), Some(lparam)).0;
                // The notifications whose answer the control reads, a list's
                // empty text: the page passes the dialog's on. Others keep
                // the page's 0, because the dialog leaves its own answer
                // unset for them.
                if msg == WM_NOTIFY && (*(lparam.0 as *const NMHDR)).code == LVN_GETEMPTYMARKUP {
                    SetWindowLongPtrW(hwnd, DWLP_MSGRESULT, answer);
                }
                answer
            }
        }
        _ => 0,
    }
}

/// Tells `dialog` that the main window holds a reply for it, such as Use
/// Total Commander's, which the dialog then reads.
pub(crate) fn post_reply(dialog: Hwnd) {
    dialog.post(REPLY);
}

/// Ends the dialog; [`run_modal`] returns `end`.
pub(crate) fn end(dialog: Hwnd, end: DialogEnd) {
    let result = match end {
        DialogEnd::Ok => 1,
        DialogEnd::Cancel => 2,
    };
    // SAFETY: plain values.
    let _ = unsafe { EndDialog(dialog.0, result) };
}

/// The dialog's control `id`.
pub(crate) fn item(dialog: Hwnd, id: u16) -> Option<Hwnd> {
    // SAFETY: plain values.
    unsafe { GetDlgItem(Some(dialog.0), i32::from(id)) }
        .ok()
        .map(Hwnd)
}

/// Makes `dialog` `extra` wider at `at`, an x of its client area: each
/// control from `at` on moves right, and each that reaches `at`, such as a
/// search box above a list, grows. Call it before the dialog adds controls
/// of its own, such as its pages, which it places itself.
pub(crate) fn widen(dialog: Hwnd, at: Px, extra: Px) {
    for control in descendants(dialog)
        .into_iter()
        .filter(|control| control.parent() == dialog)
    {
        let rect = control.rect_in(dialog);
        if rect.x >= at {
            control.set_rect(Rect {
                x: rect.x + extra,
                ..rect
            });
        } else if rect.right() >= at {
            control.set_rect(Rect {
                width: rect.width + extra,
                ..rect
            });
        }
    }
    // It stays centred where Windows centred it.
    let outer = dialog.screen_rect();
    dialog.set_rect(Rect {
        x: outer.x - Px(extra.0 / 2),
        width: outer.width + extra,
        ..outer
    });
}

/// Gives every push button of `dialog` and of its pages one width
/// (DESIGN.md: Look): 50 DLU, as the templates give them, or the widest
/// of their captions and `state`'s later captions in the current language,
/// placed as [`layout::sized_buttons`] says, and returns that width. Each
/// Clear that `state` names gets its own width ([`layout::clear_width`]),
/// placed the same way, and only its icon while icons show. A button inside
/// another control, such as the status bar's Cancel, keeps its own place,
/// because the control that holds it places it.
pub(crate) fn size_buttons<D: DialogProc>(dialog: Hwnd, state: &D) -> Px {
    let minimum = horizontal_units(dialog, 50);
    let Ok(font) = Font::of(dialog) else {
        return minimum;
    };
    let meter = TextMeter::new(&font);
    let dpi = dialog.dpi();
    let room = |caption: &str, shielded: bool| {
        layout::button_room(
            layout::button_content(meter.width(caption), shielded, dpi),
            dpi,
        )
    };
    let children = descendants(dialog);
    let parents: Vec<Hwnd> = std::iter::once(dialog)
        .chain(
            children
                .iter()
                .copied()
                .filter(|child| child.parent() == dialog && is_class(*child, "#32770")),
        )
        .collect();
    let (buttons, others): (Vec<Hwnd>, Vec<Hwnd>) = children
        .into_iter()
        .filter(|child| parents.contains(&child.parent()))
        .partition(|child| is_push_button(*child));
    let (clears, buttons): (Vec<Hwnd>, Vec<Hwnd>) = buttons
        .into_iter()
        .partition(|button| state.clear_glyph(control_id(*button)).is_some());
    let width = buttons
        .iter()
        .map(|&button| room(&button.text(), has_image(button)))
        .chain(
            state
                .later_captions()
                .iter()
                .map(|(caption, shielded)| room(caption, *shielded)),
        )
        .fold(minimum, Px::max);
    let margin = horizontal_units(dialog, 7);
    for parent in parents {
        let of_parent = |controls: &[Hwnd]| -> (Vec<Hwnd>, Vec<Rect>) {
            controls
                .iter()
                .filter(|control| control.parent() == parent)
                .map(|&control| (control, control.rect_in(parent)))
                .unzip()
        };
        let (parent_buttons, button_rects) = of_parent(&buttons);
        let (parent_others, other_rects) = of_parent(&others);
        let (placed, shrunk) = layout::sized_buttons(&button_rects, &other_rects, width, margin);
        let moves: Vec<(Hwnd, Rect)> = parent_buttons
            .into_iter()
            .zip(placed)
            .chain(
                shrunk
                    .into_iter()
                    .map(|(index, rect)| (parent_others[index], rect)),
            )
            .collect();
        super::move_children(&moves);
    }
    // After the rows have moved, so the control beside a Clear, such as
    // its box, ends its gap before it.
    for clear in clears {
        let parent = clear.parent();
        let id = control_id(clear);
        if let Some(button) = Button::of_dialog(parent, id) {
            button.set_lone_icon(state.clear_glyph(id).filter(|_| icons_shown()));
        }
        let beside: Vec<Hwnd> = others
            .iter()
            .copied()
            .filter(|control| control.parent() == parent)
            .collect();
        let rects: Vec<Rect> = beside
            .iter()
            .map(|control| control.rect_in(parent))
            .collect();
        let rect = clear.rect_in(parent);
        let caption = meter.width(&clear.text());
        let width = layout::clear_width(caption, icons_shown(), rect.height, dpi);
        let (placed, shrunk) = layout::sized_buttons(&[rect], &rects, width, margin);
        let moves: Vec<(Hwnd, Rect)> = std::iter::once((clear, placed[0]))
            .chain(
                shrunk
                    .into_iter()
                    .map(|(index, rect)| (beside[index], rect)),
            )
            .collect();
        super::move_children(&moves);
    }
    width
}

/// The control ID of `control`, a dialog's or a page's.
fn control_id(control: Hwnd) -> u16 {
    // SAFETY: plain values; a window without an ID gives 0.
    unsafe { GetDlgCtrlID(control.0) as u16 }
}

/// Every window inside `dialog`, at any depth.
fn descendants(dialog: Hwnd) -> Vec<Hwnd> {
    unsafe extern "system" fn add(child: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the vector that `descendants` passed, alive
        // while EnumChildWindows runs.
        unsafe { (*(found.0 as *mut Vec<Hwnd>)).push(Hwnd(child)) };
        BOOL(1)
    }
    let mut found: Vec<Hwnd> = Vec::new();
    // SAFETY: the callback writes only to `found`, which outlives the call.
    let _ = unsafe { EnumChildWindows(Some(dialog.0), Some(add), LPARAM(&raw mut found as isize)) };
    found
}

/// Whether `control`'s window class is `class`, such as "#32770", a
/// dialog's, which a page has.
fn is_class(control: Hwnd, class: &str) -> bool {
    let mut name = [0u16; 8];
    // SAFETY: `name` is a local whose length the call is given.
    let length = unsafe { GetClassNameW(control.0, &mut name) };
    usize::try_from(length)
        .is_ok_and(|n| String::from_utf16_lossy(&name[..n]).eq_ignore_ascii_case(class))
}

/// Whether `control` is a push button, rather than a check box, an option
/// button or a group box, which are buttons too.
fn is_push_button(control: Hwnd) -> bool {
    // SAFETY: plain values.
    let style = unsafe { GetWindowLongPtrW(control.0, GWL_STYLE) } as i32 & BS_TYPEMASK;
    is_class(control, "Button") && (style == BS_PUSHBUTTON || style == BS_DEFPUSHBUTTON)
}

/// Narrows the link control `link` to its text, keeping its right edge,
/// and gives the control `text` on the same line the width up to the link,
/// less the 4 DLU between related controls.
pub(crate) fn fit_link(link: Hwnd, text: Hwnd) {
    let parent = link.parent();
    let Ok(font) = Font::of(parent) else {
        return;
    };
    let caption = link.text().replace("<a>", "").replace("</a>", "");
    let (link_rect, text_rect) = layout::link_beside(
        link.rect_in(parent),
        TextMeter::new(&font).width(&caption),
        text.rect_in(parent),
        horizontal_units(parent, 4),
    );
    super::move_children(&[(link, link_rect), (text, text_rect)]);
}

/// `units` horizontal dialog units of `dialog`, in pixels.
fn horizontal_units(dialog: Hwnd, units: i32) -> Px {
    let mut rect = RECT {
        right: units,
        ..RECT::default()
    };
    // SAFETY: `rect` is a local the call converts in place.
    let _ = unsafe { MapDialogRect(dialog.0, &mut rect) };
    Px(rect.right)
}

/// Asks `question` in a Windows task dialog owned by `owner`, with two
/// buttons, the default and the icon as its [`Guard`] says. Returns whether
/// the person chose the confirm button; `Esc` and the close button choose
/// Cancel.
pub(crate) fn ask(owner: Hwnd, question: &Question) -> bool {
    const CONFIRM: i32 = 100;
    let body = match &question.body {
        QuestionBody::Text(text) => text.clone(),
        QuestionBody::Lines(lines) => aligned(owner, lines),
    };
    let [title, instruction, details, footer, confirm, cancel] = [
        question.title.as_str(),
        &question.instruction,
        &body,
        question.footer.as_deref().unwrap_or_default(),
        &question.confirm,
        &question.cancel,
    ]
    .map(wide);
    let warning = question.guard == Guard::Warning;
    let buttons = [
        TASKDIALOG_BUTTON {
            nButtonID: CONFIRM,
            pszButtonText: PCWSTR(confirm.as_ptr()),
        },
        TASKDIALOG_BUTTON {
            nButtonID: IDCANCEL.0,
            pszButtonText: PCWSTR(cancel.as_ptr()),
        },
    ];
    let config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner.0,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        Anonymous1: TASKDIALOGCONFIG_0 {
            pszMainIcon: if warning {
                TD_WARNING_ICON
            } else {
                PCWSTR::null()
            },
        },
        pszMainInstruction: PCWSTR(instruction.as_ptr()),
        pszContent: PCWSTR(details.as_ptr()),
        // Lines of labels and values, and a footer, which name versions and
        // folders, stay unwrapped; Windows wraps plain sentences, so a long
        // name in them never widens the dialog.
        cxWidth: match (&question.body, &question.footer) {
            (QuestionBody::Text(_), None) => 0,
            _ => unwrapped_width(
                owner,
                [question.instruction.as_str()],
                body.lines(),
                warning,
                question.footer.as_deref(),
            ),
        },
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: match question.guard {
            Guard::None => CONFIRM,
            Guard::CancelFirst | Guard::Warning => IDCANCEL.0,
        },
        Anonymous2: TASKDIALOGCONFIG_1 {
            pszFooterIcon: match &question.footer {
                Some(_) => TD_INFORMATION_ICON,
                None => PCWSTR::null(),
            },
        },
        pszFooter: match &question.footer {
            Some(_) => PCWSTR(footer.as_ptr()),
            None => PCWSTR::null(),
        },
        ..Default::default()
    };
    let mut chosen = 0;
    // SAFETY: the texts and the buttons outlive the call, which returns
    // when the dialog closes.
    let shown = unsafe { TaskDialogIndirect(&config, Some(&raw mut chosen), None, None) };
    shown.is_ok() && chosen == CONFIRM
}

/// Asks `choices` in a task dialog with a command link for each answer and
/// Cancel, as File Explorer asks about files with the same names; the index
/// of the answer, or `None` when the person cancelled.
pub(crate) fn choose(owner: Hwnd, choices: &Choices) -> Option<usize> {
    const FIRST: i32 = 100;
    let [title, instruction, body] =
        [&choices.title, &choices.instruction, &choices.body].map(|text| wide(text));
    let answers: Vec<Vec<u16>> = choices.answers.iter().map(|answer| wide(answer)).collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = answers
        .iter()
        .zip(FIRST..)
        .map(|(answer, id)| TASKDIALOG_BUTTON {
            nButtonID: id,
            pszButtonText: PCWSTR(answer.as_ptr()),
        })
        .collect();
    let config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner.0,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION
            | TDF_POSITION_RELATIVE_TO_WINDOW
            | TDF_USE_COMMAND_LINKS,
        // A task dialog shows its own buttons only in Windows' language; a
        // command link is a button too, so Cancel is the one common button.
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(instruction.as_ptr()),
        pszContent: if choices.body.is_empty() {
            PCWSTR::null()
        } else {
            PCWSTR(body.as_ptr())
        },
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        // Enter must not replace files at the destination.
        nDefaultButton: IDCANCEL.0,
        ..Default::default()
    };
    let mut chosen = 0;
    // SAFETY: the texts and the buttons outlive the call, which returns
    // when the dialog closes.
    let shown = unsafe { TaskDialogIndirect(&config, Some(&raw mut chosen), None, None) };
    shown.ok()?;
    usize::try_from(chosen - FIRST)
        .ok()
        .filter(|index| *index < choices.answers.len())
}

/// `lines` of labels and values as one body for [`ask`], with spaces after
/// each shorter label, so the values start within half a space of each
/// other in the dialog's message font, which is proportional.
fn aligned(owner: Hwnd, lines: &[(String, String)]) -> String {
    let Ok(font) = Font::message(owner.dpi()) else {
        return lines
            .iter()
            .map(|(label, value)| format!("{label} {value}"))
            .collect::<Vec<_>>()
            .join("\n");
    };
    let meter = TextMeter::new(&font);
    // Measured between letters, because a line's width leaves out a
    // trailing space.
    let space = (meter.width("x x").0 - meter.width("xx").0).max(1);
    let widths: Vec<i32> = lines
        .iter()
        .map(|(label, _)| meter.width(label).0)
        .collect();
    let widest = widths.iter().copied().max().unwrap_or(0);
    lines
        .iter()
        .zip(widths)
        .map(|((label, value), width)| {
            let spaces = 1 + ((widest - width + space / 2) / space).max(0) as usize;
            format!("{label}{}{value}", " ".repeat(spaces))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The client width, in dialog units, that shows each text of `heading` in
/// a task dialog's instruction font and each of `body` in its body font on
/// one line, after an icon when `icon`, such as the warning icon or a
/// command link's arrow, and `footer`; at most the width of the owner's
/// monitor's work area, and 0, which lets Windows choose, when the font
/// cannot be made. Windows' own width wraps a long line, which makes a path
/// or a version line hard to read, and shortens a path longer than a line
/// with "…".
fn unwrapped_width<'a>(
    owner: Hwnd,
    heading: impl IntoIterator<Item = &'a str>,
    body: impl IntoIterator<Item = &'a str>,
    icon: bool,
    footer: Option<&str>,
) -> u32 {
    // The room beside the text, in dialog units: the dialog's margins, the
    // icon with its gap, and the footer's small icon with its gap.
    const MARGINS: f64 = 24.0;
    const ICON: f64 = 26.0;
    const FOOTER_ICON: f64 = 12.0;
    let Ok(body_font) = Font::message(owner.dpi()) else {
        return 0;
    };
    let Ok(heading_font) = body_font.instruction() else {
        return 0;
    };
    let meter = TextMeter::new(&body_font);
    // A dialog unit is a quarter of the average width of the font's
    // letters, as Windows counts it.
    let unit = f64::from(
        meter
            .width("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz")
            .0,
    ) / 52.0
        / 4.0;
    if unit <= 0.0 {
        return 0;
    }
    let units = |width: Px| f64::from(width.0) / unit;
    let heading_meter = TextMeter::new(&heading_font);
    let widest = body
        .into_iter()
        .map(|text| meter.width(text))
        .chain(heading.into_iter().map(|text| heading_meter.width(text)))
        .max()
        .unwrap_or_default();
    let icon = if icon { ICON } else { 0.0 };
    let footer = footer.map_or(0.0, |footer| units(meter.width(footer)) + FOOTER_ICON);
    let needed = (units(widest) + icon).max(footer) + MARGINS;
    let most = units(owner.work_area_width());
    needed.min(most).ceil().max(0.0) as u32
}

/// Shows an error message box. With no owner it blocks only the calling
/// thread, as the panic hook needs (DESIGN.md: window rule 11).
pub(crate) fn error_box(owner: Option<Hwnd>, text: &str, title: &str) {
    let text = wide(text);
    // SAFETY: both texts are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            owner.map(|o| o.0),
            PCWSTR(text.as_ptr()),
            &HSTRING::from(title),
            MB_OK | MB_ICONERROR,
        );
    }
}
