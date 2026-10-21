//! Two panels' controls and handlers (file-manager-spec.md: Two panels).
//! `core::panel` decides what each panel shows; these handlers read each
//! list's [`Place`], start the disk threads of `win::folder`, and show what
//! the model then says. The list owns the focus and the selection, as
//! Search's list does (DESIGN.md: The selection).

use super::{MainWindow, timer};
use crate::core::captions::{self, PanelCaptions};
use crate::core::commands::{Key, Keystroke, Mark, MenuNode, PanelAction, PanelKey};
use crate::core::fetch::Purpose;
use crate::core::ids::ControlId;
use crate::core::layout::{self, ColumnFit, PanelLayout, Px, Rect};
use crate::core::model::Column;
use crate::core::open;
use crate::core::panel::{
    self, EnterAction, Entry, FolderChange, Place, ReadAsk, ReadChange, ReadEvent, ReadFailure,
    Row, Side, Step, View,
};
use crate::core::settings::Mode;
use crate::win::controls::{Button, Edit, Label, ListColumn, ListView};
use crate::win::folder::{self, PanelEvent, Removal};
use crate::win::gdi::{StaticColors, TextMeter};
use crate::win::notify::{
    CustomDraw, DispInfo, DrawAnswer, DrawStage, ListNotify, PanelRow, RowBar, RowFonts,
};
use crate::win::{self, Hwnd, shell};
use gozler_core::types::{SortDir, SortKey, SortSpec};
use windows::Win32::Foundation::LRESULT;

/// The panels' columns, in order: each one's Search column, whose default
/// width it takes, its sort key and whether it is right-aligned.
const COLUMNS: [(Column, SortKey, bool); 4] = [
    (Column::Name, SortKey::Name, false),
    (Column::Extension, SortKey::Extension, false),
    (Column::Size, SortKey::Size, true),
    (Column::DateModified, SortKey::DateModified, false),
];

/// A panel asks gozlerd for its folder sizes and reads its free space this
/// long after the last change in its folder (file-manager-spec.md: Folder
/// sizes).
const SETTLE_MS: u32 = 500;

/// gozlerd applies a change from the volume's journal after the disk has
/// it, so a panel asks for its sizes once more this long after an
/// operation in it ends.
const AGAIN_MS: u32 = 2_000;

/// Entries whose Ext, Size and Date modified are about the widest a panel
/// shows: a common long extension, a size near 100 GB, and a date late in a
/// year at each hour of a day, because which hour shows two digits and
/// which day part is wider depend on the time zone and the locale.
fn widest() -> impl Iterator<Item = Entry> {
    const HOUR: i64 = 36_000_000_000;
    (0..24).map(|hour| {
        let name = "x.xlsx".encode_utf16().collect();
        Entry::new(
            name,
            0x20,
            99_999_999 * 1024,
            134_429_400_000_000_000 + hour * HOUR,
        )
    })
}

/// The drive menu's item IDs: a drive's is this plus its letter's index,
/// a folder's [`FOLDER_IDS`] plus its place in the menu.
const DRIVE_IDS: u16 = 0x900;
const FOLDER_IDS: u16 = 0x940;

/// The key bar's buttons, `F3` to `F8` (file-manager-spec.md: The key bar).
pub(super) const KEY_BAR: [ControlId; 6] = [
    ControlId::KeyF3,
    ControlId::KeyF4,
    ControlId::KeyF5,
    ControlId::KeyF6,
    ControlId::KeyF7,
    ControlId::KeyF8,
];

/// One panel's controls, created in Tab order. The message and Retry come
/// before the list, so they lie above it (`ListView::create`).
pub(super) struct PanelControls {
    pub(super) drive: Button,
    pub(super) path: Edit,
    pub(super) message: Label,
    pub(super) retry: Button,
    pub(super) list: ListView,
    pub(super) counts: Label,
    /// The quick search box, in the status line's place while it is open.
    pub(super) quick: Edit,
    pub(super) free: Label,
}

impl PanelControls {
    pub(super) fn create(parent: Hwnd, side: Side) -> windows::core::Result<PanelControls> {
        use ControlId as C;
        let ids = match side {
            Side::Left => [
                C::LeftDrive,
                C::LeftPath,
                C::LeftMessage,
                C::LeftRetry,
                C::LeftPanel,
                C::LeftCounts,
                C::LeftQuick,
                C::LeftFree,
            ],
            Side::Right => [
                C::RightDrive,
                C::RightPath,
                C::RightMessage,
                C::RightRetry,
                C::RightPanel,
                C::RightCounts,
                C::RightQuick,
                C::RightFree,
            ],
        };
        let [drive, path, message, retry, list, counts, quick, free] = ids.map(ControlId::id);
        let controls = PanelControls {
            drive: Button::create_drop_down(parent, drive)?,
            path: Edit::create(parent, path)?,
            message: Label::create_centred(parent, message)?,
            retry: Button::create(parent, retry)?,
            list: ListView::create(parent, list)?,
            counts: Label::create_line(parent, counts)?,
            quick: Edit::create_echo(parent, quick)?,
            free: Label::create_right_line(parent, free)?,
        };
        let dpi = parent.dpi();
        for (index, (column, _, right_aligned)) in COLUMNS.into_iter().enumerate() {
            controls.list.insert_column(
                index,
                &ListColumn {
                    header: "",
                    width: layout::column_width(column, dpi),
                    right_aligned,
                },
            );
        }
        Ok(controls)
    }

    /// Every control, which the window shows only in Two panels.
    pub(super) fn hwnds(&self) -> [Hwnd; 8] {
        [
            self.drive.hwnd(),
            self.path.hwnd(),
            self.message.hwnd(),
            self.retry.hwnd(),
            self.list.hwnd(),
            self.counts.hwnd(),
            self.quick.hwnd(),
            self.free.hwnd(),
        ]
    }

    /// Where each control goes, the quick search box over the whole status
    /// line.
    pub(super) fn placed(&self, layout: &PanelLayout) -> [(Hwnd, Rect); 8] {
        let line = Rect {
            width: layout.free.right() - layout.counts.x,
            ..layout.counts
        };
        [
            (self.drive.hwnd(), layout.drive),
            (self.path.hwnd(), layout.path),
            (self.message.hwnd(), layout.message),
            (self.retry.hwnd(), layout.retry),
            (self.list.hwnd(), layout.list),
            (self.counts.hwnd(), layout.counts),
            (self.quick.hwnd(), line),
            (self.free.hwnd(), layout.free),
        ]
    }
}

/// Whether showing a panel's rows scrolls its focused row into view.
#[derive(Clone, Copy)]
enum Scroll {
    /// Another folder, or a new order.
    ToFocus,
    /// The same rows changed, which stay where the person sees them.
    Keep,
}

/// Sets `control`'s text when it differs, so an update that changes
/// nothing does not repaint it.
fn show_text(control: Hwnd, text: &str) {
    if control.text() != text {
        control.set_text(text);
    }
}

/// The folder a panel that saved none shows the first time: Documents on
/// the left and Downloads on the right, the two folders a person copies
/// between most (file-manager-spec.md: A panel).
fn first_folder(side: Side) -> String {
    let folder = match side {
        Side::Left => shell::documents(),
        Side::Right => shell::downloads(),
    };
    folder.unwrap_or_else(|_| r"C:\".to_owned())
}

impl MainWindow {
    pub(super) fn panel(&self, side: Side) -> &PanelControls {
        &self.created().controls.panels[side.index()]
    }

    /// The panel whose list is `control`.
    pub(super) fn panel_list(&self, control: Hwnd) -> Option<Side> {
        Side::BOTH
            .into_iter()
            .find(|side| self.panel(*side).list.hwnd() == control)
    }

    /// The panel whose path box is `control`.
    pub(super) fn panel_path(&self, control: Hwnd) -> Option<Side> {
        Side::BOTH
            .into_iter()
            .find(|side| self.panel(*side).path.hwnd() == control)
    }

    /// The list's place, after the panel has read a selection the list
    /// changed, so the place and the panel agree.
    fn place(&self, side: Side) -> Place {
        self.read_selection(side);
        let list = self.panel(side).list;
        Place {
            focused: list.focused(),
            selected: list.selected(),
        }
    }

    /// Shows the rows of `side` with `place`, where the list showed
    /// `shown`: it writes only the selection that changed, and keeps the
    /// scroll position unless `scroll` asks to show the focused row.
    fn show_place(&self, side: Side, shown: &Place, place: &Place, scroll: Scroll) {
        let list = self.panel(side).list;
        let count = self.model.borrow().panels().panel(side).count();
        list.set_count(count);
        list.change_selected(&shown.selected, &place.selected);
        match scroll {
            Scroll::ToFocus => list.set_focused(place.focused),
            Scroll::Keep => list.focus_in_place(place.focused),
        }
        // The list wrote its own selection, which the panel already has.
        self.panel_selection_pending[side.index()].set(false);
        list.redraw_rows(list.visible_rows());
        self.show_panel_sort(side);
        // A vertical scroll bar that came or went changed the room.
        self.fill_name_column(side);
    }

    /// Shows everything of `side` but its rows: the drive button, the path
    /// box, the status line or the quick search box, and the list's message
    /// and Retry. A path box with the focus keeps what the person types.
    pub(super) fn show_panel(&self, side: Side) {
        let captions = PanelCaptions::new(&self.model.borrow(), side, &*self.numbers.borrow());
        let c = self.panel(side);
        show_text(c.drive.hwnd(), &captions.drive);
        if Hwnd::focused() != Some(c.path.hwnd()) {
            show_text(c.path.hwnd(), &captions.path);
        }
        show_text(c.counts.hwnd(), &captions.counts);
        show_text(c.quick.hwnd(), &captions.counts);
        show_text(c.free.hwnd(), &captions.free);
        show_text(
            c.message.hwnd(),
            captions.message.as_deref().unwrap_or_default(),
        );
        let shown = self.model.borrow().mode() == Mode::TwoPanels;
        c.quick.hwnd().set_visible(shown && captions.quick);
        c.counts.hwnd().set_visible(shown && !captions.quick);
        c.free.hwnd().set_visible(shown && !captions.quick);
        c.message
            .hwnd()
            .set_visible(shown && captions.message.is_some());
        c.retry.hwnd().set_visible(shown && captions.retry);
    }

    /// The sort arrow on `side`'s header.
    fn show_panel_sort(&self, side: Side) {
        let sort = self.model.borrow().panels().panel(side).sort();
        let column = COLUMNS.iter().position(|(_, key, _)| *key == sort.key);
        self.panel(side)
            .list
            .show_sort(COLUMNS.len(), column.map(|c| (c, sort.dir == SortDir::Asc)));
    }

    /// Gives each panel's list its column headers, Retry its caption, and
    /// the key bar its captions and tooltips.
    pub(super) fn apply_panel_text(&self, captions: &captions::WindowCaptions) {
        let controls = &self.created().controls;
        for side in Side::BOTH {
            let c = self.panel(side);
            for (index, header) in captions.panel_headers.iter().enumerate() {
                c.list.set_header(index, header);
            }
            c.retry.hwnd().set_text(&captions.retry);
            self.show_panel(side);
            self.show_panel_sort(side);
        }
        for ((button, id), (key, word, tip)) in
            controls.key_bar.iter().zip(KEY_BAR).zip(&captions.key_bar)
        {
            // What screen readers say; the window draws the button.
            button.hwnd().set_text(&format!("{key} {word}"));
            if let Some(tip) = tip {
                controls.key_tips.set_area_text(id.id(), tip);
            }
        }
    }

    /// Sizes each panel's columns to its list: Ext, Size and Date modified
    /// fit the widest text they show, in a selected row's font, and Name
    /// takes the rest (file-manager-spec.md: A panel).
    pub(super) fn fill_panel_columns(&self) {
        let created = self.created();
        let dpi = created.metrics.get().dpi;
        let (samples, headers) = {
            let model = self.model.borrow();
            let numbers = self.numbers.borrow();
            let widest: Vec<Entry> = widest().collect();
            let samples: Vec<Vec<String>> = (1..COLUMNS.len())
                .map(|column| {
                    widest
                        .iter()
                        .map(|entry| {
                            let row = Row::Entry(entry);
                            captions::panel_cell(model.strings(), &*numbers, row, column)
                        })
                        .collect()
                })
                .collect();
            (samples, captions::panel_headers(model.strings()))
        };
        let widths: Vec<Px> = {
            let fonts = created.fonts.borrow();
            let plain = TextMeter::new(&fonts.ui);
            let selected = [TextMeter::new(&fonts.semibold), TextMeter::new(&fonts.bold)];
            samples
                .iter()
                .zip(&headers[1..])
                .map(|(column, header)| {
                    // Segoe UI's semibold digits are wider than its bold
                    // ones, so a selected row takes the wider.
                    let text = column
                        .iter()
                        .flat_map(|sample| selected.iter().map(|meter| meter.width(sample)))
                        .max();
                    let cell = layout::cell_width(text.unwrap_or_default(), Px(0), false, dpi);
                    ColumnFit::new(plain.plain_width(header), Some(cell), dpi).width
                })
                .collect()
        };
        for side in Side::BOTH {
            let list = self.panel(side).list;
            for (index, width) in widths.iter().enumerate() {
                list.set_column_width(index + 1, *width);
            }
            self.fill_name_column(side);
        }
    }

    /// Gives Name the room the other columns leave in `side`'s list, to
    /// its edge or its scroll bar, so the rows reach the divider.
    fn fill_name_column(&self, side: Side) {
        let list = self.panel(side).list;
        let dpi = self.created().metrics.get().dpi;
        let others = (1..COLUMNS.len()).fold(Px(0), |sum, index| sum + list.column_width(index));
        let width = (list.hwnd().client_size().width - others).max(Px(layout::column_width(
            Column::Name,
            dpi,
        )
        .0 / 2));
        if list.column_width(0) != width {
            list.set_column_width(0, width);
        }
    }

    /// Starts reading both panels' folders, when Two panels shows
    /// (file-manager-spec.md: The window).
    pub(super) fn start_panels(&self) {
        for side in Side::BOTH {
            let ask = self
                .model
                .borrow_mut()
                .panels_mut()
                .start_reading(side, || first_folder(side));
            self.read(side, Some(ask));
        }
        self.created().panel_threads.read_labels();
    }

    /// Stops the panels' threads and requests, when Search shows, because
    /// a watched folder on a USB drive blocks its removal and nobody sees
    /// the changes.
    pub(super) fn stop_panels(&self, hwnd: Hwnd) {
        let created = self.created();
        created.panel_threads.stop_all();
        self.model.borrow_mut().panels_mut().stop_reading();
        for side in Side::BOTH {
            created.requests.cancel(Purpose::FolderSizes(side));
            hwnd.kill_timer(timer::PANEL_SETTLE[side.index()]);
            hwnd.kill_timer(timer::PANEL_AGAIN[side.index()]);
        }
    }

    pub(super) fn read(&self, side: Side, ask: Option<ReadAsk>) {
        if let Some(ask) = ask {
            self.created().panel_threads.read(side, ask);
            self.show_panel(side);
        }
    }

    /// Applies what the panels' threads found. The changes of one wake
    /// apply as one batch for each panel, before any later event of that
    /// panel, so a burst of changes costs the window one pass.
    pub(super) fn on_panel_events(&self, hwnd: Hwnd) {
        let threads = &self.created().panel_threads;
        let mut pending: [Vec<FolderChange>; 2] = Default::default();
        for event in threads.drain() {
            match event {
                PanelEvent::Read { side, event } => {
                    self.apply_panel_changes(
                        hwnd,
                        side,
                        std::mem::take(&mut pending[side.index()]),
                    );
                    self.on_read(hwnd, side, event);
                }
                PanelEvent::Changes {
                    side,
                    reading,
                    changes,
                } => {
                    if threads.is_reading(side, reading) {
                        pending[side.index()].extend(changes);
                    }
                }
                PanelEvent::Lost { side, reading } => {
                    self.apply_panel_changes(
                        hwnd,
                        side,
                        std::mem::take(&mut pending[side.index()]),
                    );
                    if threads.is_reading(side, reading) {
                        let ask = self.model.borrow_mut().panels_mut().read_again(side);
                        self.read(side, ask);
                    }
                }
                PanelEvent::Free { side, folder, free } => {
                    let shown = self
                        .model
                        .borrow_mut()
                        .panels_mut()
                        .set_free(side, &folder, free);
                    if shown {
                        self.show_panel(side);
                    }
                }
                PanelEvent::Label { letter, label } => {
                    self.model
                        .borrow_mut()
                        .panels_mut()
                        .set_label(letter, label);
                }
                // Before the side's pending changes, which the sort did not
                // see, so its folders still match the panel's.
                PanelEvent::FolderOrder { side, sorted } => {
                    let shown = self.place(side);
                    let place = self.model.borrow_mut().panels_mut().accept_folder_order(
                        side,
                        sorted,
                        shown.focused,
                    );
                    if let Some(place) = place {
                        self.show_place(side, &shown, &place, Scroll::Keep);
                        self.show_panel(side);
                    }
                }
            }
        }
        for side in Side::BOTH {
            self.apply_panel_changes(hwnd, side, std::mem::take(&mut pending[side.index()]));
        }
    }

    /// Applies one batch of `side`'s changes and shows the rows.
    fn apply_panel_changes(&self, hwnd: Hwnd, side: Side, changes: Vec<FolderChange>) {
        if changes.is_empty() {
            return;
        }
        let shown = self.place(side);
        let applied =
            self.model
                .borrow_mut()
                .panels_mut()
                .apply_changes(side, changes, shown.focused);
        let Some((place, again)) = applied else {
            return;
        };
        self.show_place(side, &shown, &place, Scroll::Keep);
        self.show_panel(side);
        hwnd.set_timer(timer::PANEL_SETTLE[side.index()], SETTLE_MS);
        if again {
            let ask = self.model.borrow_mut().panels_mut().read_again(side);
            self.read(side, ask);
        }
    }

    fn on_read(&self, hwnd: Hwnd, side: Side, event: ReadEvent) {
        let shown = self.place(side);
        let change = self
            .model
            .borrow_mut()
            .panels_mut()
            .accept_read(side, event, shown.focused);
        let threads = &self.created().panel_threads;
        match change {
            ReadChange::Stale => return,
            ReadChange::Opened | ReadChange::NotReady => {
                self.created().requests.cancel(Purpose::FolderSizes(side));
                self.show_place(side, &shown, &Place::default(), Scroll::ToFocus);
            }
            ReadChange::Listed {
                place,
                again,
                stale,
            } => {
                let scroll = if again { Scroll::Keep } else { Scroll::ToFocus };
                self.show_place(side, &shown, &place, scroll);
                if stale {
                    let ask = self.model.borrow_mut().panels_mut().read_again(side);
                    self.read(side, ask);
                }
                let folder = self.model.borrow().panels().panel(side).folder().to_owned();
                threads.read_free(side, &folder);
                self.ask_panel_sizes(side);
                // The listing may release the ended operations that it
                // started after.
                self.release_operations();
            }
            ReadChange::Refused {
                path,
                failure,
                reason,
                typed,
            } => {
                // Starting the refused reading ended the shown folder's watch,
                // so the panel reads its folder again, which keeps the rows
                // and watches it again.
                let ask = self.model.borrow_mut().panels_mut().read_again(side);
                self.read(side, ask);
                self.show_panel(side);
                self.refuse(hwnd, side, &path, failure, &reason, typed);
                return;
            }
        }
        self.show_panel(side);
        self.update_status();
    }

    /// The message box for a folder that could not be opened, after which
    /// the path box keeps a typed path for the person to correct
    /// (file-manager-spec.md: A panel).
    fn refuse(
        &self,
        hwnd: Hwnd,
        side: Side,
        path: &str,
        failure: ReadFailure,
        reason: &str,
        typed: bool,
    ) {
        let (title, text) = {
            let model = self.model.borrow();
            let strings = model.strings();
            let text = match failure {
                ReadFailure::Missing => strings.format("panel.missing", &[path]),
                ReadFailure::NotReady => {
                    strings.format("panel.drive_not_ready", &[&panel::drive_root(path), reason])
                }
                ReadFailure::Other => strings.format("panel.cannot_open", &[path, reason]),
            };
            (strings.text("panel.open_title").to_owned(), text)
        };
        win::dialog::error_box(Some(hwnd), &text, &title);
        let c = self.panel(side);
        let typed_text = self.panel_typed.borrow_mut()[side.index()].take();
        match typed_text.filter(|_| typed) {
            Some(text) => {
                c.path.hwnd().focus();
                c.path.hwnd().set_text(&text);
                c.path.select_to_end();
            }
            None => c.list.hwnd().focus(),
        }
    }

    /// Asks gozlerd for `side`'s folder sizes.
    pub(super) fn ask_panel_sizes(&self, side: Side) {
        let fetch = self.model.borrow_mut().ask_sizes(side);
        match fetch {
            Some(fetch) => self.start(fetch),
            None => self.created().requests.cancel(Purpose::FolderSizes(side)),
        }
    }

    /// The settle timer of `side` ran out: its sizes and free space are
    /// read again.
    pub(super) fn on_panel_settled(&self, hwnd: Hwnd, side: Side) {
        hwnd.kill_timer(timer::PANEL_SETTLE[side.index()]);
        self.ask_panel_sizes(side);
        let folder = self.model.borrow().panels().panel(side).folder().to_owned();
        if !folder.is_empty() {
            self.created().panel_threads.read_free(side, &folder);
        }
    }

    /// gozlerd answered `side`'s folder sizes: a panel sorted by size gets
    /// them when a thread has sorted its folders by them.
    pub(super) fn show_panel_sizes(&self, side: Side) {
        let job = self.model.borrow_mut().panels_mut().take_folder_sort(side);
        match job {
            Some(job) => self.created().panel_threads.sort_folders(side, job),
            None => {
                let list = self.panel(side).list;
                list.redraw_rows(list.visible_rows());
                self.show_panel(side);
            }
        }
    }

    /// Sorts `side` by `sort`. The header shows the new sort at once; the
    /// rows follow when the reading thread has listed them in that order.
    fn sort_panel(&self, side: Side, sort: SortSpec) {
        let ask = self.model.borrow_mut().panels_mut().sort(side, sort);
        self.read(side, ask);
        self.show_panel_sort(side);
    }

    /// View > Sort by in Two panels sorts the active panel.
    pub(super) fn sort_active_panel(&self, key: Option<SortKey>, dir: Option<SortDir>) {
        let side = self.model.borrow().panels().active();
        let shown = self.model.borrow().panels().panel(side).sort();
        let sort = match (key, dir) {
            (Some(key), _) if key == shown.key => shown,
            (Some(key), _) => SortSpec::default_for(key),
            (None, Some(dir)) => SortSpec { dir, ..shown },
            (None, None) => shown,
        };
        self.sort_panel(side, sort);
    }

    /// Ctrl+R: the active panel reads its folder again and asks for its
    /// folder sizes again.
    pub(super) fn refresh_panel(&self) {
        let side = self.model.borrow().panels().active();
        let ask = self.model.borrow_mut().panels_mut().read_again(side);
        self.read(side, ask);
    }

    /// Makes `side` the active panel, and with `focus` gives its list the
    /// focus. The path boxes and the focused rows show which is active.
    pub(super) fn activate_panel(&self, side: Side, focus: bool) {
        let changed = self.model.borrow_mut().panels_mut().activate(side);
        // A click in the other panel closes this one's quick search box.
        self.close_quick_search_of(side.other());
        if focus {
            self.panel(side).list.hwnd().focus();
        }
        if changed {
            for side in Side::BOTH {
                let c = self.panel(side);
                c.path.hwnd().invalidate();
                if let Some(row) = c.list.focused() {
                    c.list.redraw_rows(row..row + 1);
                }
            }
            self.update_status();
        }
    }

    /// Closes the active panel's quick search box, before a command runs
    /// (file-manager-spec.md: Quick search).
    pub(super) fn close_quick_search(&self) {
        let side = self.model.borrow().panels().active();
        self.close_quick_search_of(side);
    }

    fn close_quick_search_of(&self, side: Side) {
        if self.model.borrow_mut().panels_mut().close_quick(side) {
            self.show_panel(side);
        }
    }

    /// A key of a panel's list or path box (file-manager-spec.md: Moving
    /// around, Selecting entries, Quick search). Returns whether the
    /// window took it.
    pub(super) fn on_panel_key(
        &self,
        hwnd: Hwnd,
        target: Hwnd,
        keystroke: Keystroke,
        key: PanelKey,
    ) -> bool {
        let Some(side) = self.panel_list(target).or(self.panel_path(target)) else {
            return false;
        };
        let c = self.panel(side);
        let quick = self
            .model
            .borrow()
            .panels()
            .panel(side)
            .quick_text()
            .is_some();
        if quick {
            let moved = match key {
                PanelKey::Move(step @ (Step::Up | Step::Down)) => {
                    let focused = c.list.focused();
                    Some(self.model.borrow_mut().panels_mut().quick_step(
                        side,
                        focused,
                        step == Step::Down,
                    ))
                }
                PanelKey::Back => Some(self.model.borrow_mut().panels_mut().quick_back(side)),
                // A space is a character while the box is open, which the
                // list must not take as a selection.
                PanelKey::Toggle if keystroke.key == Key::Space => {
                    Some(self.model.borrow_mut().panels_mut().quick_char(side, ' '))
                }
                _ => None,
            };
            if let Some(row) = moved {
                if row.is_some() {
                    c.list.set_focused(row);
                }
                self.show_panel(side);
                return true;
            }
            self.model.borrow_mut().panels_mut().close_quick(side);
            self.show_panel(side);
            if key == PanelKey::Escape {
                return true;
            }
        }
        match key {
            PanelKey::Enter => self.enter_row(hwnd, side),
            PanelKey::Back => {
                let ask = self.model.borrow_mut().panels_mut().go_up(side);
                self.read(side, ask);
            }
            PanelKey::OtherPanel => self.activate_panel(side.other(), true),
            PanelKey::Escape => c.list.set_selected(&[]),
            PanelKey::Toggle => {
                if let Some(row) = c.list.focused() {
                    let entry = self
                        .model
                        .borrow()
                        .panels()
                        .panel(side)
                        .entry(row)
                        .is_some();
                    if entry {
                        let (_, selected) = c.list.row_state(row);
                        c.list.select(row, !selected);
                    }
                }
                self.move_focus(side, Step::Down, false);
            }
            PanelKey::Move(step) => self.move_focus(side, step, false),
            PanelKey::Select(step) => self.move_focus(side, step, true),
            PanelKey::PathEnter => {
                let typed = c.path.text();
                let ask = self
                    .model
                    .borrow_mut()
                    .panels_mut()
                    .go_typed(side, &typed, |name| std::env::var(name).ok());
                self.panel_typed.borrow_mut()[side.index()] = Some(typed);
                c.list.hwnd().focus();
                self.read(side, ask);
            }
            PanelKey::PathEscape => {
                c.list.hwnd().focus();
                self.show_panel(side);
            }
        }
        true
    }

    /// Moves the focus of `side` by `step`; with `select`, each row it
    /// passes is selected, as in Total Commander.
    fn move_focus(&self, side: Side, step: Step, select: bool) {
        let list = self.panel(side).list;
        let from = list.focused();
        let count = self.model.borrow().panels().panel(side).count();
        let Some(to) = panel::step(step, from, count, list.page_rows()) else {
            return;
        };
        if select && let Some(from) = from {
            let rows: Vec<u64> = {
                let model = self.model.borrow();
                let panel = model.panels().panel(side);
                panel::passed(from, to, count)
                    .filter(|row| panel.entry(*row).is_some())
                    .collect()
            };
            for row in rows {
                list.select(row, true);
            }
        }
        list.set_focused(Some(to));
    }

    /// `Enter` or a double-click on the focused row of `side`.
    fn enter_row(&self, hwnd: Hwnd, side: Side) {
        let Some(row) = self.panel(side).list.focused() else {
            return;
        };
        let action = self.model.borrow_mut().panels_mut().enter(side, row);
        match action {
            EnterAction::Read(ask) => self.read(side, Some(ask)),
            EnterAction::Open(path) => {
                let command = self.model.borrow().choices().open_files.clone();
                match open::expand(&command, &path) {
                    Some(line) => {
                        self.run_open_command(hwnd, &line);
                    }
                    None => shell::open_entry(hwnd, &path.encode_utf16().collect::<Vec<u16>>()),
                }
            }
            EnterAction::Nothing => {}
        }
    }

    /// A character typed in a panel's list: quick search (file-manager-spec.md:
    /// Quick search). Returns whether the window took it.
    pub(super) fn on_panel_char(&self, target: Hwnd, character: char, alt: bool) -> bool {
        let Some(side) = self.panel_list(target) else {
            return false;
        };
        // Control characters, such as Ctrl with a letter, are keys, and
        // Alt+Space opens the window menu.
        if character.is_control() || alt && character == ' ' {
            return false;
        }
        if *self.model.borrow().panels().panel(side).view() != View::Folder {
            return false;
        }
        let row = self
            .model
            .borrow_mut()
            .panels_mut()
            .quick_char(side, character);
        if row.is_some() {
            self.panel(side).list.set_focused(row);
        }
        self.show_panel(side);
        true
    }

    /// Two panels' fixed keys that run no command.
    pub(super) fn run_panel_action(&self, hwnd: Hwnd, action: PanelAction) {
        match action {
            PanelAction::PathBox => {
                let side = self.model.borrow().panels().active();
                let path = self.panel(side).path;
                path.hwnd().focus();
                path.select_all();
            }
            PanelAction::Drives(side) => self.drive_menu(hwnd, side),
            PanelAction::MenuBar => win::window::open_menu_bar(hwnd),
        }
    }

    /// The drive menu of `side`, under its drive button
    /// (file-manager-spec.md: The drive menu).
    pub(super) fn drive_menu(&self, hwnd: Hwnd, side: Side) {
        self.activate_panel(side, false);
        let c = self.panel(side);
        let drives = folder::drives();
        let (nodes, states, folders) = {
            let model = self.model.borrow();
            let panels = model.panels();
            let panel = panels.panel(side);
            let own = panel.drive();
            let folders: Vec<String> = panel
                .recent_folders()
                .into_iter()
                .map(str::to_owned)
                .collect();
            let mut nodes: Vec<MenuNode> = drives
                .iter()
                .map(|letter| MenuNode::Item {
                    id: drive_id(*letter),
                    text: panels.drive_label(*letter),
                    mark: Mark::Radio,
                    glyph: None,
                })
                .collect();
            if !folders.is_empty() {
                nodes.push(MenuNode::Separator);
            }
            nodes.extend(
                folders
                    .iter()
                    .enumerate()
                    .map(|(index, folder)| MenuNode::Item {
                        id: FOLDER_IDS + index as u16,
                        text: format!("&{}  {}", (index + 1) % 10, folder.replace('&', "&&")),
                        mark: Mark::None,
                        glyph: None,
                    }),
            );
            let states: Vec<(u16, bool, bool)> = drives
                .iter()
                .map(|letter| (drive_id(*letter), true, format!("{letter}:") == own))
                .collect();
            (nodes, states, folders)
        };
        // The menu opens on the panel's own drive, so the arrow keys start
        // from it (file-manager-spec.md: The drive menu).
        let own = states.iter().position(|(_, _, own)| *own).unwrap_or(0);
        let button = c.drive.hwnd().screen_rect();
        let chosen = win::menu::Menu::track_popup_highlighted(
            hwnd,
            &nodes,
            (button.x.0, button.bottom().0),
            &states,
            own,
        );
        c.list.hwnd().focus();
        let ask = match chosen {
            Some(id) if id >= FOLDER_IDS => {
                folders.get(usize::from(id - FOLDER_IDS)).map(|folder| {
                    self.model
                        .borrow_mut()
                        .panels_mut()
                        .go(side, folder.clone(), None)
                })
            }
            Some(id) => drives
                .iter()
                .find(|letter| drive_id(**letter) == id)
                .map(|letter| {
                    self.model
                        .borrow_mut()
                        .panels_mut()
                        .go_drive(side, &format!(r"{letter}:\"))
                }),
            None => None,
        };
        self.read(side, ask);
    }

    /// A notification of `side`'s list.
    pub(super) fn on_panel_notify(
        &self,
        hwnd: Hwnd,
        side: Side,
        notify: ListNotify,
    ) -> Option<LRESULT> {
        match notify {
            ListNotify::GetDispInfo(mut info) => {
                self.fill_panel_cell(side, &mut info);
                Some(LRESULT(0))
            }
            ListNotify::CacheHint(_) => Some(LRESULT(0)),
            ListNotify::ColumnClick(index) => {
                self.close_quick_search_of(side);
                if let Some((_, key, _)) = COLUMNS.get(index) {
                    let shown = self.model.borrow().panels().panel(side).sort();
                    self.sort_panel(side, panel::header_sort(shown, *key));
                }
                Some(LRESULT(0))
            }
            ListNotify::CustomDraw(draw) => Some(CustomDraw::answer(match draw.stage() {
                DrawStage::Paint => DrawAnswer::EachRow,
                DrawStage::Row if self.draw_panel_row(side, &draw) => DrawAnswer::Drawn,
                DrawStage::Row | DrawStage::Cell { .. } | DrawStage::Other => DrawAnswer::Default,
            })),
            ListNotify::SelectionChanged => {
                if !self.panel_selection_pending[side.index()].replace(true) {
                    hwnd.post(super::DEFERRED);
                }
                None
            }
            // A click anywhere in the list, or the focus leaving it, closes
            // the quick search box (file-manager-spec.md: Quick search).
            ListNotify::Clicked | ListNotify::Unfocused => {
                self.close_quick_search_of(side);
                None
            }
            ListNotify::Activated => {
                self.enter_row(hwnd, side);
                None
            }
            ListNotify::Focused => {
                self.activate_panel(side, false);
                None
            }
            ListNotify::BeginDrag | ListNotify::DividerDoubleClick(_) | ListNotify::Other => None,
        }
    }

    /// Reads the panels' selections that their lists changed, once per
    /// person's action, as Search reads its list's (DESIGN.md: The
    /// selection).
    pub(super) fn read_panel_selection(&self) {
        for side in Side::BOTH {
            self.read_selection(side);
        }
    }

    /// Reads `side`'s selection when its list changed it. `..` cannot be
    /// selected, and a change of the selection, such as a click, closes
    /// the quick search box.
    fn read_selection(&self, side: Side) {
        if !self.panel_selection_pending[side.index()].replace(false) {
            return;
        }
        let list = self.panel(side).list;
        let up = matches!(
            self.model.borrow().panels().panel(side).row(0),
            Some(Row::Up)
        );
        if up && list.row_state(0).1 {
            list.select(0, false);
            self.panel_selection_pending[side.index()].set(false);
        }
        let selected = list.selected();
        {
            let mut model = self.model.borrow_mut();
            let panels = model.panels_mut();
            if panels.set_selected(side, selected) {
                panels.close_quick(side);
            }
        }
        self.show_panel(side);
    }

    /// The cell's text and the row's icon of `side`'s list, which screen
    /// readers and the list's label tips read; the window draws the rows.
    fn fill_panel_cell(&self, side: Side, info: &mut DispInfo) {
        let (text, icon) = {
            let model = self.model.borrow();
            let numbers = self.numbers.borrow();
            match model.panels().panel(side).row(info.row()) {
                Some(row) => (
                    info.wants_text().then(|| {
                        captions::panel_cell(model.strings(), &*numbers, row, info.column())
                    }),
                    Some(icon_of(row)),
                ),
                None => (None, None),
            }
        };
        if info.wants_text() {
            info.set_text(text.as_deref().unwrap_or_default());
        }
        if info.wants_image() {
            info.set_image(icon.map(|(name, is_folder)| self.icons.index(&name, is_folder)));
        }
    }

    /// Draws the row of `draw` in `side`'s list; `false` when the panel no
    /// longer has it, so the list draws it.
    fn draw_panel_row(&self, side: Side, draw: &CustomDraw) -> bool {
        let row = draw.row();
        let (focused, selected) = self.panel(side).list.row_state(row);
        let (cells, icon, dimmed, up, active) = {
            let model = self.model.borrow();
            let numbers = self.numbers.borrow();
            let panels = model.panels();
            let Some(shown) = panels.panel(side).row(row) else {
                return false;
            };
            let cells: Vec<String> = (0..COLUMNS.len())
                .map(|column| captions::panel_cell(model.strings(), &*numbers, shown, column))
                .collect();
            let dimmed = matches!(shown, Row::Entry(entry) if entry.is_dimmed());
            let up = shown == Row::Up;
            (cells, icon_of(shown), dimmed, up, panels.active() == side)
        };
        let icon = self.icons.index(&icon.0, icon.1);
        let bar = match (focused, active) {
            (false, _) => RowBar::None,
            (true, true) => RowBar::Active,
            (true, false) => RowBar::Inactive,
        };
        let created = self.created();
        let fonts = created.fonts.borrow();
        draw.draw_panel_row(
            &PanelRow {
                cells: &cells,
                right_aligned: &COLUMNS.map(|(_, _, right)| right),
                icon: created.images.map(|images| (images, icon)),
                up,
                bar,
                selected,
                dimmed,
            },
            &RowFonts {
                plain: &fonts.ui,
                selected: &fonts.semibold,
                selected_on_bar: &fonts.bold,
            },
        );
        true
    }

    /// The colors of a path box: the system highlight for the active
    /// panel's while it does not have the focus, as Total Commander shows
    /// its active path (file-manager-spec.md: A panel).
    pub(super) fn path_box_colors(&self, colors: StaticColors, control: Hwnd) -> Option<LRESULT> {
        let side = self.panel_path(control)?;
        let active = self.model.borrow().panels().active() == side;
        (active && Hwnd::focused() != Some(control)).then(|| colors.highlight())
    }

    /// Windows asks to remove, or removed, the drive whose watch it names
    /// with `notification` (file-manager-spec.md: A USB drive can always be
    /// removed).
    pub(super) fn on_device_removal(&self, removal: Removal, notification: isize) {
        let threads = &self.created().panel_threads;
        let Some(side) = threads.removal(notification, removal) else {
            return;
        };
        match removal {
            // The watch let its handle go.
            Removal::Query => {}
            // Another program kept the drive, so the reading that the query
            // stopped starts again, or the panel follows its folder again.
            Removal::Failed => {
                let ask = self.model.borrow_mut().panels_mut().restart_reading(side);
                self.read(side, ask);
            }
            Removal::Complete => {
                let shown = self.place(side);
                self.model.borrow_mut().panels_mut().drive_removed(side);
                self.show_place(side, &shown, &Place::default(), Scroll::ToFocus);
                self.show_panel(side);
            }
        }
    }

    /// The panels after the taken entries changed, and the panels that
    /// ended operations make read their folders again or ask for their
    /// sizes, as `follow` says (fast-copy-delete-spec.md: Taken entries).
    pub(super) fn show_panels_taken(
        &self,
        hwnd: Hwnd,
        taken: std::sync::Arc<gozler_fileops::Taken>,
        follow: Vec<(Side, bool, bool)>,
    ) {
        let shown = Side::BOTH.map(|side| self.place(side));
        let places = self
            .model
            .borrow_mut()
            .panels_mut()
            .set_taken(taken, [shown[0].focused, shown[1].focused]);
        for side in Side::BOTH {
            self.show_place(
                side,
                &shown[side.index()],
                &places[side.index()],
                Scroll::Keep,
            );
            self.show_panel(side);
        }
        if self.model.borrow().mode() != Mode::TwoPanels {
            return;
        }
        for (side, again, sizes) in follow {
            if again {
                let ask = self.model.borrow_mut().panels_mut().read_again(side);
                self.read(side, ask);
            }
            if sizes {
                self.ask_panel_sizes(side);
                hwnd.set_timer(timer::PANEL_AGAIN[side.index()], AGAIN_MS);
            }
        }
    }

    /// Removes from the queue the ended deletes and moves that no list
    /// keeps any more.
    pub(super) fn release_operations(&self) {
        if let Some(operations) = self.file_operations.borrow().as_ref() {
            let model = self.model.borrow();
            operations.release(|id| model.keeps(id));
        }
    }
}

fn drive_id(letter: char) -> u16 {
    DRIVE_IDS + (u32::from(letter) - u32::from('A')) as u16
}

/// The name and the kind that a row's icon is looked up by: `..` shows a
/// folder's, which the row draws an arrow over.
fn icon_of(row: Row) -> (String, bool) {
    match row {
        Row::Up => (String::new(), true),
        Row::Entry(entry) => (entry.display_name(), entry.is_folder()),
    }
}
