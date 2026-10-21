//! The Viewer windows (DESIGN.md: The Viewer): each `F3` on a file in a panel
//! opens one, a top-level window that no window owns. `core::viewer_window`
//! decides what a Viewer shows; this module gives it the blocks that the
//! reading thread of `win::viewer` reads, and asks for the next block that the
//! page or a move waits for, one at a time.

use crate::core::captions::ButtonIcon;
use crate::core::ids::{ControlId, IconId};
use crate::core::layout::{
    Px, Rect, image_button_width, status_last_part, viewer_message, viewer_message_width,
};
use crate::core::settings::{Placement, ViewerState};
use crate::core::text::{self, Strings};
use crate::core::viewer::Mode;
use crate::core::viewer_window::{
    Document, ScrollBar, Shown, ViewerCommand, ViewerKey, file_text, menu_bar, menu_states,
    place_samples, text_inset, text_room, viewer_key,
};
use crate::win::controls::{Button, Label, StatusBar};
use crate::win::folder::Removal;
use crate::win::gdi::{Font, TextMeter};
use crate::win::locale::{WindowsNumbers, ansi_text};
use crate::win::menu::{Menu, replace_menu_bar};
use crate::win::notify::Command;
use crate::win::viewer::{
    AREA_PAINT, AreaText, FileReader, ViewerEvent, caption_height, create_area, paint_area,
    register_area, set_area_scroll,
};
use crate::win::wake::OpenWaker;
use crate::win::window::{self, DEFERRED, Message, WindowProc};
use crate::win::{self, Hwnd};
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};
use windows::Win32::Foundation::LRESULT;

const WINDOW_CLASS: &str = "gozler.ui.viewer";

/// The open Viewers, as the main window holds them, and what they remember
/// in `ui.json`.
pub(super) struct Viewers {
    shared: Rc<Shared>,
    /// The window classes are registered, which the first Viewer does.
    registered: Cell<bool>,
}

/// What every Viewer reaches: the open ones, which Exit closes, and the
/// state the last one left, which the next one opens with.
struct Shared {
    open: RefCell<Vec<Rc<Viewer>>>,
    state: Cell<ViewerState>,
}

impl Viewers {
    pub(super) fn new(state: ViewerState) -> Viewers {
        Viewers {
            shared: Rc::new(Shared {
                open: RefCell::new(Vec::new()),
                state: Cell::new(state),
            }),
            registered: Cell::new(false),
        }
    }

    /// Opens a Viewer of the file at `path`, a full path in UTF-16, which
    /// holds every name a folder can hold, in front, worded with `strings`.
    /// Closing it activates `main`, which opened it, while `main` shows.
    pub(super) fn open(
        &self,
        path: Vec<u16>,
        strings: Strings,
        main: Hwnd,
    ) -> windows::core::Result<()> {
        if !self.registered.get() {
            window::register_class::<Viewer>(
                WINDOW_CLASS,
                IconId::Gozler.id(),
                window::Background::Window,
            )?;
            register_area()?;
            self.registered.set(true);
        }
        let state = self.shared.state.get();
        let placement = self.placement(state.placement);
        let viewer = Rc::new(Viewer {
            hwnd: Cell::new(None),
            shared: Rc::downgrade(&self.shared),
            name: text::name_of(&String::from_utf16_lossy(&path)).to_owned(),
            path,
            main,
            strings: RefCell::new(strings),
            numbers: RefCell::new(WindowsNumbers::new()),
            controls: OnceCell::new(),
            fonts: RefCell::new(None),
            shown: RefCell::new(Shown::Reading),
            reader: RefCell::new(None),
            waker: OnceCell::new(),
            resume: Cell::new(None),
            in_flight: Cell::new(None),
            wrap: Cell::new(state.wrap),
            focused: Cell::new(None),
        });
        let hwnd = window::create_window(WINDOW_CLASS, Rc::clone(&viewer), placement)?;
        if let Err(error) = viewer.start(hwnd) {
            hwnd.destroy();
            return Err(error);
        }
        window::show(hwnd, placement);
        viewer.layout(hwnd);
        viewer.controls().area.focus();
        self.shared.open.borrow_mut().push(viewer);
        Ok(())
    }

    /// The state that `ui.json` saves: the last Viewer's size and place,
    /// and Wrap long lines.
    pub(super) fn state(&self) -> ViewerState {
        self.shared.state.get()
    }

    /// Closes every Viewer, as Exit does; each leaves its state first.
    pub(super) fn close_all(&self) {
        let open = std::mem::take(&mut *self.shared.open.borrow_mut());
        // The first Viewer closes last, so the saved place stays its own,
        // not one that each later Viewer moved a caption further.
        for viewer in open.into_iter().rev() {
            if let Some(hwnd) = viewer.hwnd.get() {
                hwnd.destroy();
            }
        }
    }

    /// Words every Viewer in the language of `strings`.
    pub(super) fn set_strings(&self, strings: &Strings) {
        for viewer in self.opened() {
            viewer.strings.replace(strings.clone());
            if let Some(hwnd) = viewer.hwnd.get() {
                viewer.apply_strings(hwnd);
                viewer.layout(hwnd);
            }
        }
    }

    /// Shows or hides the icons of every Viewer's menus and Retry, after
    /// icons were shown or hidden.
    pub(super) fn apply_glyphs(&self) {
        for viewer in self.opened() {
            if let Some(hwnd) = viewer.hwnd.get() {
                viewer.apply_glyphs(hwnd);
            }
        }
    }

    /// Where a new Viewer opens: at `saved`, the size and place the last
    /// Viewer left, or, while a Viewer is open, at the saved size below and
    /// to the right of the last one opened by its caption's height, and not
    /// maximized, so the two never cover each other exactly.
    fn placement(&self, saved: Option<Placement>) -> Option<Placement> {
        let last = self.opened().last().and_then(|viewer| viewer.hwnd.get());
        let Some((last, hwnd)) = last.and_then(|hwnd| Some((hwnd.placement()?, hwnd))) else {
            return saved;
        };
        let size = saved.unwrap_or(last);
        let step = caption_height(hwnd.dpi()).0;
        let (left, top) = (last.left + step, last.top + step);
        Some(Placement {
            left,
            top,
            right: left + (size.right - size.left),
            bottom: top + (size.bottom - size.top),
            maximized: false,
        })
    }

    /// A copy of the list, so a Viewer that closes meanwhile can leave it.
    fn opened(&self) -> Vec<Rc<Viewer>> {
        self.shared.open.borrow().clone()
    }
}

/// A Viewer's controls, created in Tab order. The message and Retry come
/// before the area, so they lie above it.
struct Controls {
    message: Label,
    retry: Button,
    area: Hwnd,
    status: StatusBar,
}

struct Fonts {
    ui: Font,
    /// Consolas, in which the area draws the file.
    text: Font,
    char_width: Px,
    line_height: Px,
}

struct Viewer {
    hwnd: Cell<Option<Hwnd>>,
    shared: Weak<Shared>,
    path: Vec<u16>,
    name: String,
    main: Hwnd,
    strings: RefCell<Strings>,
    numbers: RefCell<WindowsNumbers>,
    controls: OnceCell<Controls>,
    fonts: RefCell<Option<Fonts>>,
    shown: RefCell<Shown>,
    /// The reading of the file; `None` after the drive was removed.
    reader: RefCell<Option<FileReader>>,
    /// Wakes the window from the reading thread, which outlives it, until
    /// `WM_DESTROY` closes it.
    waker: OnceCell<OpenWaker>,
    /// The mode and the first shown byte that the next opening of the file
    /// shows again, after a removal or a failure.
    resume: Cell<Option<(Mode, u64)>>,
    /// The block asked for and not yet read, so a fast drag of the scroll
    /// bar asks for one block at a time.
    in_flight: Cell<Option<u64>>,
    wrap: Cell<bool>,
    /// The control that had the focus when the window was deactivated.
    focused: Cell<Option<Hwnd>>,
}

impl Viewer {
    fn controls(&self) -> &Controls {
        self.controls.get().expect("start creates the controls")
    }

    /// Everything the new window needs before it shows: its keys, its
    /// controls, menus and texts, and the reading of its file.
    fn start(&self, hwnd: Hwnd) -> windows::core::Result<()> {
        self.hwnd.set(Some(hwnd));
        window::own_keys(hwnd, true);
        let controls = Controls {
            message: Label::create_centred(hwnd, ControlId::ViewerMessage.id())?,
            retry: Button::create(hwnd, ControlId::ViewerRetry.id())?,
            area: create_area(hwnd, ControlId::ViewerArea.id())?,
            status: StatusBar::create(hwnd, ControlId::ViewerStatus.id(), true)?,
        };
        let _ = self.controls.set(controls);
        self.apply_fonts(hwnd);
        self.apply_strings(hwnd);
        let _ = self.waker.set(OpenWaker::new(hwnd));
        self.read_file(hwnd);
        Ok(())
    }

    /// Opens the file again from its start, or from `resume`.
    fn read_file(&self, hwnd: Hwnd) {
        let Some(waker) = self.waker.get() else {
            return;
        };
        self.in_flight.set(None);
        // The reading it replaces ends, and closes its file on its own.
        self.reader
            .replace(Some(FileReader::open(&self.path, hwnd, waker.clone())));
        self.set_shown(hwnd, Shown::Reading);
    }

    fn remember_place(&self) {
        if let Some(document) = self.shown.borrow().document() {
            self.resume
                .set(Some((document.mode(), document.first_byte())));
        }
    }

    fn set_shown(&self, hwnd: Hwnd, shown: Shown) {
        self.shown.replace(shown);
        self.show_message();
        self.layout(hwnd);
    }

    /// Creates the fonts for the window's DPI and gives them to the
    /// controls.
    fn apply_fonts(&self, hwnd: Hwnd) {
        let Some(c) = self.controls.get() else {
            return;
        };
        let Ok(ui) = Font::message(hwnd.dpi()) else {
            return;
        };
        for control in [c.message.hwnd(), c.retry.hwnd(), c.status.hwnd()] {
            control.set_font(&ui);
        }
        // At the height of the message font, which the message now has.
        let Ok(text) = Font::monospace_of(c.message.hwnd()) else {
            return;
        };
        let (char_width, line_height) = {
            let meter = TextMeter::new(&text);
            (meter.width("0"), meter.height())
        };
        // The controls hold the old fonts until they get the new ones.
        self.fonts.replace(Some(Fonts {
            ui,
            text,
            char_width,
            line_height,
        }));
    }

    /// The title, the menus and Retry in the window's language.
    fn apply_strings(&self, hwnd: Hwnd) {
        let Some(c) = self.controls.get() else {
            return;
        };
        {
            let strings = self.strings.borrow();
            hwnd.set_text(&strings.format("viewer.title", &[&self.name]));
            c.retry.hwnd().set_text(strings.text("panel.retry"));
        }
        self.apply_glyphs(hwnd);
        self.show_message();
        self.show();
    }

    /// Gives the menus and Retry their icons, at the window's DPI, in the
    /// system colours of now and only while icons show.
    fn apply_glyphs(&self, hwnd: Hwnd) {
        let _ = replace_menu_bar(hwnd, &menu_bar(&self.strings.borrow()));
        if let Some(c) = self.controls.get() {
            c.retry.set_icon(ButtonIcon::of("panel.retry", false));
        }
    }

    /// The message instead of the file, and Retry under it, as what the
    /// area shows asks.
    fn show_message(&self) {
        let Some(c) = self.controls.get() else {
            return;
        };
        let message = self
            .shown
            .borrow()
            .message(&self.strings.borrow(), &self.name);
        let (text, retry) = message.clone().unwrap_or_default();
        c.message.hwnd().set_text(&text);
        c.message.hwnd().set_visible(message.is_some());
        if !retry && Hwnd::focused() == Some(c.retry.hwnd()) {
            c.area.focus();
        }
        c.retry.hwnd().set_visible(retry);
    }

    /// Places the controls, and fits the page to the area.
    fn layout(&self, hwnd: Hwnd) {
        let Some(c) = self.controls.get() else {
            return;
        };
        let size = hwnd.client_size();
        // Minimized: the page stays as it was.
        if size.width.0 == 0 || size.height.0 == 0 {
            return;
        }
        let dpi = hwnd.dpi();
        let status_height = c.status.fit();
        let area = Rect {
            x: Px(0),
            y: Px(0),
            width: size.width,
            height: (size.height - status_height).max(Px(0)),
        };
        let (message, retry, place_width, char_width, line_height) = {
            let fonts = self.fonts.borrow();
            let Some(fonts) = fonts.as_ref() else {
                return;
            };
            let meter = TextMeter::new(&fonts.ui);
            let strings = self.strings.borrow();
            let widest = place_samples(&strings, &*self.numbers.borrow())
                .iter()
                .map(|sample| meter.width(sample))
                .max()
                .unwrap_or_default();
            let text = c.message.hwnd().text();
            let (message, retry) = viewer_message(
                area,
                meter.wrapped_height(&text, viewer_message_width(area, dpi)),
                meter.height(),
                image_button_width(meter.width(strings.text("panel.retry")), dpi),
                dpi,
            );
            (
                message,
                retry,
                status_last_part(widest, dpi),
                fonts.char_width,
                fonts.line_height,
            )
        };
        win::move_children(&[
            (c.area, area),
            (c.message.hwnd(), message),
            (c.retry.hwnd(), retry),
        ]);
        c.status.set_parts(&[(size.width - place_width).max(Px(0))]);
        let (columns, rows) = text_room(c.area.client_size(), char_width, line_height, dpi);
        if let Some(document) = self.shown.borrow_mut().document_mut() {
            document.set_area(columns, rows);
        }
        self.pump();
    }

    /// Goes on with the page as far as the kept blocks allow, asks for the
    /// block it waits for, and shows the page.
    fn pump(&self) {
        let waits = self
            .shown
            .borrow_mut()
            .document_mut()
            .and_then(|document| document.advance(&ansi_text));
        if let Some(index) = waits
            && self.in_flight.get().is_none()
            && let Some(reader) = self.reader.borrow().as_ref()
        {
            reader.read(index);
            self.in_flight.set(Some(index));
        }
        self.show();
    }

    /// Shows the page, its scroll bar and the status bar.
    fn show(&self) {
        let Some(c) = self.controls.get() else {
            return;
        };
        let shown = self.shown.borrow();
        let document = shown.document();
        let strings = self.strings.borrow();
        let numbers = self.numbers.borrow();
        set_area_scroll(
            c.area,
            document.map_or(ScrollBar::NONE, Document::scroll_bar),
        );
        c.status
            .set_text(0, &file_text(&strings, &*numbers, &self.name, document));
        let place = document.map(|document| document.place(&strings, &*numbers));
        c.status.set_text(1, &place.unwrap_or_default());
        c.area.invalidate();
    }

    /// Paints the area; `false` before the window has its controls.
    fn paint(&self, hwnd: Hwnd) -> bool {
        let Some(c) = self.controls.get() else {
            return false;
        };
        let fonts = self.fonts.borrow();
        let Some(fonts) = fonts.as_ref() else {
            return false;
        };
        let shown = self.shown.borrow();
        let rows = shown.document().map(Document::shown).unwrap_or_default();
        paint_area(
            c.area,
            &AreaText {
                font: &fonts.text,
                rows: &rows,
                line_height: fonts.line_height,
                char_width: fonts.char_width,
                inset: text_inset(hwnd.dpi()),
            },
        );
        true
    }

    /// Applies what the reading thread found.
    fn read_events(&self, hwnd: Hwnd) {
        let events = self
            .reader
            .borrow()
            .as_ref()
            .map(FileReader::drain)
            .unwrap_or_default();
        for event in events {
            match event {
                ViewerEvent::Opened { len, first } => {
                    let mut document = Document::new(len, first, self.wrap.get());
                    if let Some((mode, offset)) = self.resume.take() {
                        document.jump_as(mode, offset);
                    }
                    self.set_shown(hwnd, Shown::File(document));
                }
                ViewerEvent::Block { index, bytes } => {
                    self.in_flight.set(None);
                    if let Some(document) = self.shown.borrow_mut().document_mut() {
                        document.receive(index, bytes);
                    }
                    self.pump();
                }
                ViewerEvent::Failed { opened, reason } => {
                    self.remember_place();
                    self.reader.replace(None);
                    self.set_shown(hwnd, Shown::Failed { opened, reason });
                }
            }
        }
    }

    /// Windows asks `removal` of the file's drive (DESIGN.md: The Viewer);
    /// `false` when `notification` is not this Viewer's.
    fn removal(&self, hwnd: Hwnd, notification: isize, removal: Removal) -> bool {
        let ours = self
            .reader
            .borrow()
            .as_ref()
            .is_some_and(|reader| reader.is_registration(notification));
        if !ours {
            return false;
        }
        match removal {
            Removal::Query => {
                self.remember_place();
                if let Some(reader) = self.reader.borrow_mut().as_mut() {
                    reader.release();
                }
                self.set_shown(hwnd, Shown::Removed);
            }
            // Another program kept the drive, so the file can show again.
            Removal::Failed => self.read_file(hwnd),
            // A drive pulled out without asking sends only this.
            Removal::Complete => {
                self.remember_place();
                self.reader.replace(None);
                self.set_shown(hwnd, Shown::Removed);
            }
        }
        true
    }

    fn on_key(&self, hwnd: Hwnd, key: ViewerKey) {
        match key {
            ViewerKey::Close => return self.close(hwnd),
            ViewerKey::SwitchMode => {
                let mode = self.shown.borrow().document().map(Document::mode);
                match mode {
                    Some(Mode::Text) => self.set_mode(Mode::Hex),
                    Some(Mode::Hex) => self.set_mode(Mode::Text),
                    None => {}
                }
            }
            ViewerKey::Wrap => self.toggle_wrap(),
            key => {
                if let Some(document) = self.shown.borrow_mut().document_mut() {
                    match key {
                        ViewerKey::LineUp => document.line_up(),
                        ViewerKey::LineDown => document.line_down(),
                        ViewerKey::PageUp => document.page_up(),
                        ViewerKey::PageDown => document.page_down(),
                        ViewerKey::Home => document.home(),
                        ViewerKey::End => document.end(),
                        ViewerKey::Left => document.scroll_left(),
                        ViewerKey::Right => document.scroll_right(),
                        ViewerKey::Close | ViewerKey::SwitchMode | ViewerKey::Wrap => {}
                    }
                }
            }
        }
        self.pump();
    }

    fn set_mode(&self, mode: Mode) {
        if let Some(document) = self.shown.borrow_mut().document_mut() {
            document.set_mode(mode);
        }
    }

    /// Wrap long lines on or off, which the next Viewer opens with; in Hex
    /// it does nothing, as its menu item is disabled there.
    fn toggle_wrap(&self) {
        let mode = self.shown.borrow().document().map(Document::mode);
        if mode == Some(Mode::Hex) {
            return;
        }
        let wrap = !self.wrap.get();
        self.wrap.set(wrap);
        if let Some(document) = self.shown.borrow_mut().document_mut() {
            document.set_wrap(wrap);
        }
        if let Some(shared) = self.shared.upgrade() {
            shared.state.set(ViewerState {
                wrap,
                ..shared.state.get()
            });
            self.main.post(DEFERRED);
        }
    }

    fn on_command(&self, hwnd: Hwnd, command: ViewerCommand) {
        match command {
            ViewerCommand::Close => return self.close(hwnd),
            ViewerCommand::Text => self.set_mode(Mode::Text),
            ViewerCommand::Hex => self.set_mode(Mode::Hex),
            ViewerCommand::Wrap => self.toggle_wrap(),
        }
        self.pump();
    }

    /// Activates the main window, which keeps its focused row, so `F3`,
    /// `Esc`, `Down`, `F3` walks through the files; Windows would
    /// otherwise activate whichever window was behind the Viewer. A main
    /// window the person minimized stays minimized.
    fn close(&self, hwnd: Hwnd) {
        if self.main.is_on_screen() {
            self.main.bring_to_front();
        }
        hwnd.destroy();
    }

    /// Keeps the shown window's size and place for the next Viewer, which
    /// the main window saves.
    fn remember_placement(&self, hwnd: Hwnd) {
        if !hwnd.is_visible() {
            return;
        }
        if let (Some(shared), Some(placement)) = (self.shared.upgrade(), hwnd.placement()) {
            shared.state.set(ViewerState {
                placement: Some(placement),
                ..shared.state.get()
            });
            self.main.post(DEFERRED);
        }
    }
}

impl WindowProc for Viewer {
    fn message(&self, hwnd: Hwnd, message: Message<'_>) -> Option<LRESULT> {
        match message {
            Message::Size => {
                self.layout(hwnd);
                Some(LRESULT(0))
            }
            Message::WindowPosChanged => {
                self.remember_placement(hwnd);
                None
            }
            Message::DpiChanged { suggested, .. } => {
                hwnd.set_rect(suggested);
                self.apply_fonts(hwnd);
                self.apply_glyphs(hwnd);
                self.layout(hwnd);
                Some(LRESULT(0))
            }
            Message::Other(AREA_PAINT) => Some(LRESULT(isize::from(self.paint(hwnd)))),
            Message::Wake => {
                self.read_events(hwnd);
                Some(LRESULT(0))
            }
            Message::KeyDown {
                keystroke, repeat, ..
            } => {
                let key = viewer_key(keystroke)?;
                // A held F3 would otherwise close this Viewer and open it
                // again from the main window, for as long as it is held.
                if !(repeat && key == ViewerKey::Close) {
                    self.on_key(hwnd, key);
                }
                Some(LRESULT(1))
            }
            Message::MouseWheel { delta, .. } => {
                if let Some(document) = self.shown.borrow_mut().document_mut() {
                    document.wheel(delta);
                }
                self.pump();
                Some(LRESULT(0))
            }
            Message::VerticalScroll { from, request } => {
                if from != self.controls.get()?.area {
                    return None;
                }
                if let Some(document) = self.shown.borrow_mut().document_mut() {
                    use crate::core::layout::ScrollRequest as R;
                    match request {
                        R::LineUp => document.line_up(),
                        R::LineDown => document.line_down(),
                        R::PageUp => document.page_up(),
                        R::PageDown => document.page_down(),
                        R::Top => document.home(),
                        R::Bottom => document.end(),
                        R::To(position) => document.jump(position.0),
                        R::Wheel(delta) => document.wheel(delta),
                    }
                }
                self.pump();
                Some(LRESULT(0))
            }
            Message::DeviceRemoval {
                removal,
                notification,
            } => self
                .removal(hwnd, notification, removal)
                .then_some(LRESULT(1)),
            Message::InitMenuPopup => {
                let mode = self.shown.borrow().document().map(Document::mode);
                if let Some(menu) = Menu::of(hwnd) {
                    for (id, enabled, checked) in menu_states(mode, self.wrap.get()) {
                        menu.set_state(id, enabled, checked);
                    }
                }
                Some(LRESULT(0))
            }
            Message::Command(Command::Menu(id)) => {
                self.on_command(hwnd, ViewerCommand::from_id(id)?);
                Some(LRESULT(0))
            }
            Message::Command(Command::Control(event)) => {
                if self.controls.get()?.retry.clicked(event) {
                    self.read_file(hwnd);
                }
                Some(LRESULT(0))
            }
            Message::CtlColorStatic { colors, control }
                if control == self.controls.get()?.message.hwnd() =>
            {
                Some(colors.window_background())
            }
            Message::Deactivate => {
                self.focused
                    .set(Hwnd::focused().filter(|focused| hwnd.is_parent_of(*focused)));
                None
            }
            Message::Activate { .. } => {
                let area = self.controls.get()?.area;
                let focus = self
                    .focused
                    .take()
                    .filter(|focused| focused.is_visible())
                    .unwrap_or(area);
                focus.focus();
                Some(LRESULT(0))
            }
            Message::SysColorChange => {
                self.apply_glyphs(hwnd);
                if let Some(c) = self.controls.get() {
                    c.area.invalidate();
                }
                None
            }
            Message::SettingChange {
                regional_format: true,
            } => {
                self.numbers.replace(WindowsNumbers::new());
                self.layout(hwnd);
                None
            }
            Message::Close => {
                self.close(hwnd);
                Some(LRESULT(0))
            }
            Message::Destroy => {
                self.remember_placement(hwnd);
                if let Some(waker) = self.waker.get() {
                    waker.close();
                }
                self.reader.replace(None);
                window::own_keys(hwnd, false);
                if let Some(shared) = self.shared.upgrade() {
                    shared
                        .open
                        .borrow_mut()
                        .retain(|viewer| !std::ptr::eq(Rc::as_ptr(viewer), self));
                }
                None
            }
            _ => None,
        }
    }
}
