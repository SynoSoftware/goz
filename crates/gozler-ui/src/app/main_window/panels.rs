//! Commander mode's controls and handlers (DESIGN.md: Commander mode). `core::panel`
//! decides what each panel shows; these handlers read each list's [`Place`],
//! submit folder work through the work owner, and show what the model says.
//! The list owns the focus and the selection, as Search's list does (DESIGN.md:
//! The selection).

use super::super::dialogs::{NewFolder, SelectFiles, TransferMode, TransferStart};
use super::{MainWindow, MeasuredColumn, Selected, Timer, measure_fits};
use crate::core::captions::{self, PanelCaptions};
use crate::core::commands::{self, CommandId, Mark, MenuItem, MenuNode, PanelAction, PanelKey};
use crate::core::fetch::{List, Purpose};
use crate::core::ids::ControlId;
use crate::core::ids::DialogId;
use crate::core::layout::column_sizing::{self, ColumnFit};
use crate::core::layout::{Body, PanelLayout, Px, Rect};
use crate::core::model::{Accepted, Areas, Effect, TakenShown};
use crate::core::open::{Entries, EntryAction};
use crate::core::panel::{
    self, Entered, FolderChange, LookIn, PanelColumn, PanelFollow, PanelUpdate,
    Panels as PanelState, Place, QuickAction, ReadAccepted, ReadAsk, ReadChange, ReadEvent,
    ReadFailure, Row, Side, TakenRows, UpdatedKind, View,
};
use crate::core::selection::SELECTION_LIMIT;
use crate::core::settings::Mode;
use crate::core::work::Waiting;
use crate::win::controls::{Button, DropDownList, Edit, Label, ListColumn, ListView};
use crate::win::dialog::DialogEnd;
use crate::win::folder::{self, PanelEvent, Removal};
use crate::win::gdi::StaticColors;
use crate::win::notify::{CustomDraw, DispInfo, DrawAnswer, DrawStage, ListNotify};
use crate::win::{self, Hwnd, shell};
use gozler_core::types::{SortDir, SortKey, SortSpec};
use gozler_core::winpath::parent_of;
use gozler_fileops::Request;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::Instant;
use windows::Win32::Foundation::LRESULT;

/// Where a search panel's list puts its Folder column.
const FOLDER_COLUMN: usize = 2;

/// A panel asks gozlerd for its folder sizes and reads its free space this
/// long after the last change in its folder (DESIGN.md: Folder sizes).
const SETTLE_MS: u32 = 500;

/// A panel that goes to a folder keeps showing the folder it leaves until
/// the new listing replaces it in one update, so the list never shows empty
/// in between; a listing that takes longer than this shows the new folder
/// reading (DESIGN.md: A panel).
const READING_MS: u32 = 250;

/// gozlerd applies a change from the volume's journal after the disk has
/// it, so a panel asks for its sizes once more this long after an
/// operation in it ends.
const AGAIN_MS: u32 = 2_000;

/// The drive menu's item IDs: a drive's is this plus its letter's index,
/// a folder's [`FOLDER_IDS`] plus its place in the menu.
const DRIVE_IDS: u16 = 0x900;
const FOLDER_IDS: u16 = 0x940;

/// One panel's controls, created in Tab order. The message and Retry come
/// before the list, so they lie above it (`ListView::create`). A search
/// panel shows its search box and Look in in the path box's place
/// (DESIGN.md: Search in a panel).
pub(super) struct PanelControls {
    pub(super) changed: Cell<ListChanged>,
    pub(super) typed: std::cell::RefCell<Option<String>>,
    pub(super) drive: Button,
    pub(super) path: Edit,
    pub(super) search: Edit,
    /// Right before its list, so its access key focuses the list.
    pub(super) look_in_label: Label,
    pub(super) look_in: DropDownList,
    pub(super) message: Label,
    pub(super) retry: Button,
    pub(super) list: ListView,
    pub(super) counts: Label,
    /// The quick search box, in the status line's place while it is open.
    pub(super) quick: Edit,
    pub(super) free: Label,
    /// A search panel's focused result's folder, while it searches all
    /// drives.
    pub(super) folder: Label,
    /// Where the controls show the panel searching, `None` for a folder
    /// panel: which controls show, the list's columns, and the status
    /// line's parts ([`Panels::show_panel_kind`]).
    pub(super) kind: Cell<Option<LookIn>>,
}

impl PanelControls {
    pub(super) fn create(parent: Hwnd, side: Side) -> windows::core::Result<PanelControls> {
        use ControlId as C;
        let ids = match side {
            Side::Left => [
                C::LeftDrive,
                C::LeftPath,
                C::LeftSearch,
                C::LeftLookInLabel,
                C::LeftLookIn,
                C::LeftMessage,
                C::LeftRetry,
                C::LeftPanel,
                C::LeftCounts,
                C::LeftQuick,
                C::LeftFree,
                C::LeftFolder,
            ],
            Side::Right => [
                C::RightDrive,
                C::RightPath,
                C::RightSearch,
                C::RightLookInLabel,
                C::RightLookIn,
                C::RightMessage,
                C::RightRetry,
                C::RightPanel,
                C::RightCounts,
                C::RightQuick,
                C::RightFree,
                C::RightFolder,
            ],
        };
        let [
            drive,
            path,
            search,
            look_in_label,
            look_in,
            message,
            retry,
            list,
            counts,
            quick,
            free,
            folder,
        ] = ids.map(ControlId::id);
        let controls = PanelControls {
            changed: Cell::new(ListChanged::Nothing),
            typed: std::cell::RefCell::new(None),
            drive: Button::create_drop_down(parent, drive)?,
            path: Edit::create(parent, path)?,
            search: Edit::create(parent, search)?,
            look_in_label: Label::create(parent, look_in_label)?,
            look_in: DropDownList::create(parent, look_in)?,
            message: Label::create_centred(parent, message)?,
            retry: Button::create(parent, retry)?,
            list: ListView::create(parent, list)?,
            counts: Label::create_heading(parent, counts)?,
            quick: Edit::create_echo(parent, quick)?,
            free: Label::create_right_heading(parent, free)?,
            folder: Label::create_right_path(parent, folder)?,
            kind: Cell::new(None),
        };
        // The panel's fills give the widths.
        for (index, column) in PanelColumn::FOLDER.into_iter().enumerate() {
            controls.list.insert_column(
                index,
                &ListColumn {
                    header: "",
                    width: Px(0),
                    right_aligned: column.right_aligned(),
                },
            );
        }
        Ok(controls)
    }

    /// Every control, which the window shows only in Commander mode.
    pub(super) fn hwnds(&self) -> [Hwnd; 12] {
        [
            self.drive.hwnd(),
            self.path.hwnd(),
            self.search.hwnd(),
            self.look_in_label.hwnd(),
            self.look_in.hwnd(),
            self.message.hwnd(),
            self.retry.hwnd(),
            self.list.hwnd(),
            self.counts.hwnd(),
            self.quick.hwnd(),
            self.free.hwnd(),
            self.folder.hwnd(),
        ]
    }

    /// Where each control goes, the quick search box over the whole status
    /// line, and the counts in its left half while the panel searches all
    /// drives. The message and Retry go where their text needs
    /// ([`Panels::show_panel`]).
    pub(super) fn placed(&self, layout: &PanelLayout) -> [(Hwnd, Rect); 10] {
        let line = Rect {
            width: layout.free.right() - layout.counts.x,
            ..layout.counts
        };
        let counts = match self.kind.get() {
            Some(LookIn::AllDrives) => layout.counts_half,
            Some(LookIn::ThisFolder) | None => layout.counts,
        };
        [
            (self.drive.hwnd(), layout.drive),
            (self.path.hwnd(), layout.path),
            (self.search.hwnd(), layout.search),
            (self.look_in_label.hwnd(), layout.look_in_label),
            // A drop-down list's height includes its open list.
            (
                self.look_in.hwnd(),
                Rect {
                    height: Px(layout.look_in.height.0 * 10),
                    ..layout.look_in
                },
            ),
            (self.list.hwnd(), layout.list),
            (self.counts.hwnd(), counts),
            (self.quick.hwnd(), line),
            (self.free.hwnd(), layout.free),
            (self.folder.hwnd(), layout.folder),
        ]
    }

    /// The columns the list shows.
    fn columns(&self) -> &'static [PanelColumn] {
        match self.kind.get() {
            Some(_) => &PanelColumn::SEARCH,
            None => &PanelColumn::FOLDER,
        }
    }
}

/// What a panel's list changed since the panel last read it: a change of
/// the selection also reads the focus, so the later variant includes the
/// earlier ([`Panels::read_selection`]).
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ListChanged {
    #[default]
    Nothing,
    Focus,
    Selection,
}

/// A folder that `side` could not open, whose message box waits for the
/// other events of its wake ([`super::ModalStep::PanelRefused`]).
pub(super) struct Refused {
    side: Side,
    path: String,
    failure: ReadFailure,
    /// Windows' reason, such as "Access is denied."
    reason: String,
    /// The path the person typed, which the path box shows again.
    typed: Option<String>,
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
/// between most (DESIGN.md: A panel).
fn first_folder(side: Side) -> String {
    let folder = match side {
        Side::Left => shell::documents(),
        Side::Right => shell::downloads(),
    };
    folder.unwrap_or_else(|_| r"C:\".to_owned())
}

pub(super) struct Panels {
    pub(super) sides: [PanelControls; 2],
    /// The reading that Show in Commander started while Search shows.
    /// Commander mode shows when its listing arrives, so the panel appears
    /// with the requested folder, not with the folder it showed before.
    commander_reading: Cell<Option<Side>>,
    /// The mask that Select files… and Unselect files… show first: the
    /// last one chosen, while the window runs.
    select_mask: RefCell<String>,
}

impl Panels {
    pub(super) fn panel(&self, side: Side) -> &PanelControls {
        &self.sides[side.index()]
    }

    pub(super) fn create(parent: Hwnd) -> windows::core::Result<Self> {
        Ok(Self {
            sides: [
                PanelControls::create(parent, Side::Left)?,
                PanelControls::create(parent, Side::Right)?,
            ],
            commander_reading: Cell::new(None),
            select_mask: RefCell::new("*.*".to_owned()),
        })
    }
}

impl MainWindow {
    pub(super) fn panel(&self, side: Side) -> &PanelControls {
        &self.controls().panels.sides[side.index()]
    }
}

impl Panels {
    pub(super) fn on_control(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        event: crate::win::notify::ControlEvent,
    ) -> bool {
        if let Some(side) = self.panel_search(event.sender()) {
            let search = self.panel(side).search;
            if search.changed(event) {
                self.on_panel_search_text(window, hwnd, side);
            } else if search.focused(event) {
                self.activate_panel(window, side, false);
            }
        } else if let Some(side) = self.panel_look_in(event.sender()) {
            let look_in = self.panel(side).look_in;
            if look_in.selection_changed(event) {
                self.on_look_in(window, hwnd, side);
            }
        } else if let Some(side) = self.panel_path(event.sender()) {
            // The active path box shows the highlight colors only while it
            // does not have the focus; a path left without Enter goes back.
            let path = self.panel(side).path;
            if path.focused(event) {
                self.activate_panel(window, side, false);
                path.hwnd().invalidate();
            } else if path.unfocused(event) {
                path.hwnd().invalidate();
                self.show_panel(window, side);
            }
        } else if let Some(side) = Side::BOTH
            .into_iter()
            .find(|side| self.panel(*side).retry.clicked(event))
        {
            self.activate_panel(window, side, true);
            self.panel_message_clicked(window, hwnd, side);
        } else if let Some(side) = Side::BOTH
            .into_iter()
            .find(|side| self.panel(*side).drive.clicked(event))
        {
            self.drive_menu(window, hwnd, side);
        } else {
            return false;
        }
        true
    }
}

fn drive_id(letter: char) -> u16 {
    DRIVE_IDS + (u32::from(letter) - u32::from('A')) as u16
}

/// What a row's icon is looked up by.
enum RowIcon {
    /// `..`'s icon ([`Icons::up`](super::super::icons::Icons::up)).
    Up,
    /// An entry's or a result's name and whether it is a folder.
    Item(String, bool),
}

fn icon_of(row: Row) -> RowIcon {
    match row {
        Row::Up => RowIcon::Up,
        Row::Entry(entry) => RowIcon::Item(entry.display_name(), entry.is_folder()),
        Row::Result(item) => RowIcon::Item(panel::last_name(&item.path).to_owned(), item.is_dir),
    }
}

/// `side`'s row at `index` with its text, also when a delete or a move took
/// a search panel's result, which then shows struck through, as Search's
/// list shows it (DESIGN.md: Search in a panel).
fn shown_or_taken(panels: &PanelState, side: Side, index: u64) -> Option<Row<'_>> {
    match panels.search(side) {
        Some(search) => search.list.results().row(index).map(Row::Result),
        None => panels.row(side, index),
    }
}

impl Panels {
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

    /// The panel whose search box is `control`.
    pub(super) fn panel_search(&self, control: Hwnd) -> Option<Side> {
        Side::BOTH
            .into_iter()
            .find(|side| self.panel(*side).search.hwnd() == control)
    }

    /// The panel whose Look in list is `control`.
    pub(super) fn panel_look_in(&self, control: Hwnd) -> Option<Side> {
        Side::BOTH
            .into_iter()
            .find(|side| self.panel(*side).look_in.hwnd() == control)
    }

    /// `side`'s place in the window's layout, while Commander mode shows.
    pub(super) fn panel_layout(&self, window: &MainWindow, side: Side) -> Option<PanelLayout> {
        match &window.created().layout.borrow().as_ref()?.body {
            Body::Commander { panels, .. } => Some(panels[side.index()]),
            Body::Search(_) => None,
        }
    }

    /// The list's place, after the panel has read a selection the list
    /// changed, so the place and the panel agree.
    pub(super) fn place(&self, window: &MainWindow, side: Side) -> Place {
        self.read_selection(window, side, true);
        window.model.borrow().panels().place(side)
    }

    /// Shows the rows of `side` with `place`, where the list showed
    /// `shown`, keeping the scroll position unless `scroll` asks to show
    /// the focused row. A remap preserves an accepted selection command.
    fn show_place(
        &self,
        window: &MainWindow,
        side: Side,
        shown: &Place,
        place: &Place,
        scroll: Scroll,
        keep_action: bool,
    ) {
        window.show_native_place(
            List::Panel(side),
            shown,
            place,
            matches!(scroll, Scroll::ToFocus),
            keep_action,
        );
        self.show_panel_sort(window, side);
    }

    /// Shows everything of `side` but its rows: the drive button, the path
    /// box or the search row, the status line or the quick search box, and
    /// the list's message and Retry, placed for their text. A path box with
    /// the focus keeps what the person types.
    pub(super) fn show_panel(&self, window: &MainWindow, side: Side) {
        let placed = self.panel_layout(window, side);
        let mut moves = Vec::new();
        let mut shown = Vec::new();
        self.prepare_panel(window, side, placed.as_ref(), &mut moves, &mut shown);
        win::place_children(&moves, &shown);
    }

    pub(super) fn prepare_panel(
        &self,
        window: &MainWindow,
        side: Side,
        placed: Option<&PanelLayout>,
        moves: &mut Vec<(Hwnd, Rect)>,
        shown: &mut Vec<(Hwnd, bool)>,
    ) {
        window.retire_model();
        let panes = window.model.borrow().panes();
        if panes.inspector
            && panes.maximized
            && panes.content == crate::core::layout::InspectorContent::Transfers
        {
            shown.extend(self.panel(side).hwnds().map(|hwnd| (hwnd, false)));
            return;
        }
        self.show_panel_kind(window, side);
        let captions = PanelCaptions::new(&window.model.borrow(), side, &*window.numbers.borrow());
        let c = self.panel(side);
        let kind = c.kind.get();
        show_text(c.drive.hwnd(), &captions.drive);
        if Hwnd::focused() != Some(c.path.hwnd()) {
            show_text(c.path.hwnd(), &captions.path);
        }
        show_text(c.counts.hwnd(), &captions.counts);
        show_text(c.quick.hwnd(), &captions.counts);
        show_text(c.free.hwnd(), &captions.free);
        show_text(c.folder.hwnd(), &captions.folder);
        if let Some(look_in) = kind {
            c.look_in
                .select(LookIn::ALL.iter().position(|l| *l == look_in).unwrap_or(0));
        }
        let visible = self.panel_visible(window);
        let all_drives = kind == Some(LookIn::AllDrives);
        shown.extend([
            (c.drive.hwnd(), visible),
            (c.list.hwnd(), visible),
            (c.path.hwnd(), visible && kind.is_none()),
            (c.quick.hwnd(), visible && captions.quick),
            (c.counts.hwnd(), visible && !captions.quick),
            (c.free.hwnd(), visible && !captions.quick && !all_drives),
            (c.folder.hwnd(), visible && !captions.quick && all_drives),
        ]);
        for control in [c.search.hwnd(), c.look_in_label.hwnd(), c.look_in.hwnd()] {
            shown.push((control, visible && kind.is_some()));
        }
        if let Some(placed) = placed {
            moves.extend(c.placed(placed));
        }
        self.prepare_panel_message(window, side, placed, moves, shown);
        window.controls().key_bar.update_key_bar(window);
    }

    fn panel_visible(&self, window: &MainWindow) -> bool {
        let model = window.model.borrow();
        let panes = model.panes();
        model.mode() == Mode::Commander
            && !(panes.inspector
                && panes.maximized
                && panes.content == crate::core::layout::InspectorContent::Transfers)
    }

    pub(super) fn show_panel_message(&self, window: &MainWindow, side: Side) {
        let placed = self.panel_layout(window, side);
        let mut moves = Vec::new();
        let mut shown = Vec::new();
        self.prepare_panel_message(window, side, placed.as_ref(), &mut moves, &mut shown);
        win::place_children(&moves, &shown);
    }

    fn prepare_panel_message(
        &self,
        window: &MainWindow,
        side: Side,
        placed: Option<&PanelLayout>,
        moves: &mut Vec<(Hwnd, Rect)>,
        shown: &mut Vec<(Hwnd, bool)>,
    ) {
        let folder_slow = window
            .created()
            .work
            .borrow()
            .folders
            .waiting(side, Instant::now())
            == Some(Waiting::Slow);
        let slow_request = window.slow_list_work(List::Panel(side));
        let slow = folder_slow || slow_request.is_some();
        let (message, button, key, caption) = {
            let model = window.model.borrow();
            let panel = model.panels().panel(side);
            let (mut message, retry) = captions::panel_message(&model, side);
            if slow {
                let all_drives = panel
                    .search()
                    .is_some_and(|search| search.look_in == LookIn::AllDrives);
                let scoped = matches!(
                    slow_request,
                    Some(super::Awaited::Request(_) | super::Awaited::Selection(_))
                );
                let path = (folder_slow || scoped && !all_drives).then(|| panel.path_box());
                message = Some(crate::core::work::waiting(model.strings(), path));
            }
            let key = if slow { "button.cancel" } else { "panel.retry" };
            (
                message,
                slow || retry && panel.reading_generation().is_none(),
                key,
                model.strings().text(key).to_owned(),
            )
        };
        let visible = self.panel_visible(window);
        let c = self.panel(side);
        let text = message.as_deref().unwrap_or_default();
        show_text(c.message.hwnd(), text);
        show_text(c.retry.hwnd(), &caption);
        c.retry.set_glyph(captions::button_glyph(key));
        if visible
            && message.is_some()
            && let Some(placed) = placed
        {
            let header = c.list.header().screen_rect().height;
            let retry = [c.retry.hwnd()];
            let buttons = if button { &retry[..] } else { &[] };
            moves.extend(window.message_moves(
                placed.list,
                header,
                c.message.hwnd(),
                text,
                buttons,
                true,
            ));
        }
        shown.push((c.message.hwnd(), visible && message.is_some()));
        shown.push((c.retry.hwnd(), visible && button));
    }

    pub(super) fn panel_message_clicked(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        let (slow, generation) = {
            let work = window.created().work.borrow();
            (
                work.folders.waiting(side, Instant::now()) == Some(Waiting::Slow),
                work.folders.generation(side),
            )
        };
        if slow {
            if let Some(generation) = generation {
                self.cancel_read(window, hwnd, side, generation);
            }
            return;
        }
        if let Some(awaited) = window.slow_list_work(List::Panel(side)) {
            window.cancel_list_work(hwnd, List::Panel(side), awaited);
            return;
        }
        let retry = {
            let model = window.model.borrow();
            let panel = model.panels().panel(side);
            panel.reading_generation().is_none()
                && matches!(panel.view(), View::Cancelled | View::NotReady { .. })
        };
        if retry {
            let ask = window.model.borrow_mut().panels_mut().read_again(side);
            self.read(window, side, ask);
        }
    }

    pub(super) fn cancel_read(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        side: Side,
        generation: u64,
    ) -> bool {
        let (shown, place, cancelled, recovery) = {
            let mut model = window.model.borrow_mut();
            let panels = model.panels_mut();
            let shown = panels.place(side);
            if !panels.cancel_read(side, generation) {
                return false;
            }
            let cancelled = matches!(panels.panel(side).view(), View::Cancelled);
            let recovery = if panels.take_reread(side) {
                panels.read_again(side)
            } else {
                None
            };
            (shown, panels.place(side), cancelled, recovery)
        };
        window.created().work.borrow_mut().folders.cancel_read(side);
        self.panel(side).typed.borrow_mut().take();
        if cancelled {
            self.show_place(window, side, &shown, &place, Scroll::Keep, false);
        }
        self.read(window, side, recovery);
        window.retire_model();
        self.show_panel(window, side);
        window.update_status();
        window.note_saved_state(hwnd);
        true
    }

    /// Prepares the Folder column before a changed panel's rows refresh;
    /// [`Self::show_panel`] places and shows its controls together.
    pub(super) fn show_panel_kind(&self, window: &MainWindow, side: Side) {
        let wanted = window
            .model
            .borrow()
            .panels()
            .search(side)
            .map(|s| s.look_in);
        let c = self.panel(side);
        let shown = c.kind.replace(wanted);
        if shown == wanted {
            return;
        }
        if shown.is_none() != wanted.is_none() {
            match wanted {
                Some(_) => {
                    let header = {
                        let model = window.model.borrow();
                        captions::panel_headers(model.strings(), &[PanelColumn::Folder])
                    };
                    c.list.insert_column(
                        FOLDER_COLUMN,
                        &ListColumn {
                            header: &header[0].full,
                            width: Px(0),
                            right_aligned: false,
                        },
                    );
                }
                None => c.list.delete_column(FOLDER_COLUMN),
            }
            self.show_panel_sort(window, side);
            self.fill_panel_columns(window);
        }
    }

    /// The sort arrow on `side`'s header.
    pub(super) fn show_panel_sort(&self, window: &MainWindow, side: Side) {
        let sort = window.model.borrow().panels().panel(side).sort();
        let columns = self.panel(side).columns();
        let column = columns.iter().position(|c| c.sort_key() == Some(sort.key));
        self.panel(side)
            .list
            .show_sort(columns.len(), column.map(|c| (c, sort.dir == SortDir::Asc)));
    }

    /// Gives each panel's list its column headers, Retry its caption, the
    /// search row its texts, and the key bar its captions and tooltips.
    pub(super) fn apply_panel_text(
        &self,
        window: &MainWindow,
        captions: &captions::WindowCaptions,
    ) {
        let controls = window.controls();
        for side in Side::BOTH {
            let c = self.panel(side);
            let headers = {
                let model = window.model.borrow();
                captions::panel_headers(model.strings(), c.columns())
            };
            for (index, header) in headers.iter().enumerate() {
                c.list.set_header(index, c.list.fitting_text(index, header));
            }
            c.retry.hwnd().set_text(&captions.retry);
            c.look_in_label.hwnd().set_text(&captions.look_in_label);
            // `show_panel` selects the panel's choice.
            c.look_in.set_items(&captions.look_in, 0);
            self.show_panel(window, side);
            self.show_panel_sort(window, side);
        }
        controls.key_bar.set_captions(captions);
    }

    /// Fills each panel's list with its columns (DESIGN.md: A panel).
    pub(super) fn fill_panel_columns(&self, window: &MainWindow) {
        self.fill_panel_columns_at(
            window,
            Side::BOTH.map(|side| Some(self.panel(side).list.fill_width())),
        );
    }

    pub(super) fn fill_panel_columns_at(&self, window: &MainWindow, available: [Option<Px>; 2]) {
        for (side, available) in Side::BOTH.into_iter().zip(available) {
            if let Some(available) = available {
                self.fill_panel_at(window, side, available);
            }
        }
    }

    /// Fills `side`'s list with its columns when its rows changed.
    pub(super) fn fill_panel(&self, window: &MainWindow, side: Side) {
        self.fill_panel_at(window, side, self.panel(side).list.fill_width());
    }

    /// Fills `available`, the list's [`ListView::fill_width`], with `side`'s
    /// columns as Search fills its list ([`column_sizing::fill_widths`]),
    /// so the rows reach the divider and a folder whose rows bring the
    /// vertical scroll bar brings no horizontal one.
    fn fill_panel_at(&self, window: &MainWindow, side: Side, available: Px) {
        let widths = column_sizing::fill_widths(&self.panel_fits(window, side), available);
        self.panel(side)
            .list
            .set_column_widths(widths.into_iter().enumerate());
    }

    /// The fits of `side`'s columns: each fits its header and its cells in
    /// [`ListView::fit_rows`], within the limits of Search's column of the
    /// same kind, as Search's columns do.
    fn panel_fits(&self, window: &MainWindow, side: Side) -> Vec<ColumnFit> {
        let c = self.panel(side);
        let (rows, label) = c.list.fit_rows();
        // The model is borrowed while the texts are formatted, and the
        // measuring follows.
        let measured = {
            let model = window.model.borrow();
            let numbers = window.numbers.borrow();
            let dates = window.dates.borrow();
            let strings = model.strings();
            let sizes = model.choices().size_format;
            let panels = model.panels();
            let panel = panels.panel(side);
            (c.columns().iter().enumerate())
                .map(|(index, &column)| MeasuredColumn {
                    column: column.search_column(),
                    first: index == 0,
                    header: captions::panel_header(strings, column),
                    cells: (rows.clone())
                        .filter_map(|index| shown_or_taken(panels, side, index))
                        .map(|row| {
                            let cell = captions::panel_cell(
                                strings, &*numbers, sizes, &dates, panel, row, column,
                            );
                            (cell.full, Vec::new())
                        })
                        .collect(),
                })
                .collect::<Vec<_>>()
        };
        measure_fits(window, measured, label)
    }

    /// Gives Name the room the other columns leave in `side`'s list,
    /// `width` wide, while the person drags the window's border, because
    /// the rows the list shows change with its height and fits measured on
    /// them would move every column. A search panel's Name and Folder
    /// share the room as they shared it before.
    pub(super) fn fill_name_column_at(&self, side: Side, width: Px) {
        let c = self.panel(side);
        let shared =
            |column: &PanelColumn| matches!(column, PanelColumn::Name | PanelColumn::Folder);
        let others = (c.columns().iter().enumerate())
            .filter(|(_, column)| !shared(column))
            .fold(Px(0), |sum, (index, _)| sum + c.list.column_width(index));
        let room = (width - others).max(Px(0));
        let widths = match c.columns().iter().position(|c| *c == PanelColumn::Folder) {
            Some(folder) => {
                let (name, path) = (c.list.column_width(0), c.list.column_width(folder));
                let name =
                    i64::from(room.0) * i64::from(name.0) / i64::from((name + path).0.max(1));
                let name = Px(i32::try_from(name).expect("a part of the room fits in i32"));
                vec![(0, name), (folder, room - name)]
            }
            None => vec![(0, room)],
        };
        c.list.set_column_widths(widths);
    }

    /// Starts reading both panels' folders, or a search panel's search,
    /// when Commander mode shows (DESIGN.md: Commander mode's window).
    pub(super) fn start_panels(&self, window: &MainWindow, hwnd: Hwnd) -> Accepted {
        let mut accepted = Accepted {
            touched: Areas::LEFT_PANEL | Areas::RIGHT_PANEL | Areas::STATUS | Areas::KEY_BAR,
            effects: vec![
                Effect::PreparePanel(Side::Left),
                Effect::PreparePanel(Side::Right),
            ],
        };
        for side in Side::BOTH {
            // Show in Commander's reading of this side runs already.
            if window.created().work.borrow().folders.watches(side) {
                continue;
            }
            let ask = window
                .model
                .borrow_mut()
                .panels_mut()
                .start_reading(side, || first_folder(side));
            match ask {
                Some(ask) => self.read(window, side, Some(ask)),
                None => {
                    let search = window.begin_search(hwnd, List::Panel(side));
                    accepted.touched |= search.touched;
                    accepted.effects.extend(search.effects);
                }
            }
        }
        window.created().work.borrow_mut().folders.read_labels();
        accepted
    }

    /// Stops the panels' threads and requests, when Search shows, because
    /// a watched folder on a USB drive blocks its removal and nobody sees
    /// the changes. A search panel lets go of its rows, its selection and
    /// its kept operations, but keeps its text and Look in, because it
    /// searches again when Commander mode shows.
    pub(super) fn stop_panels(&self, window: &MainWindow, hwnd: Hwnd) -> Accepted {
        let mut accepted = Accepted {
            touched: Areas::LEFT_PANEL | Areas::RIGHT_PANEL | Areas::STATUS | Areas::KEY_BAR,
            effects: Vec::new(),
        };
        self.commander_reading.set(None);
        window.created().work.borrow_mut().folders.stop_all();
        window.model.borrow_mut().panels_mut().stop_reading();
        for side in Side::BOTH {
            let dropped = {
                let mut model = window.model.borrow_mut();
                let search = model.panels_mut().search_mut(side);
                search.map(|search| search.list.drop_rows()).is_some()
            };
            window.retire_model();
            if dropped {
                let list = List::Panel(side);
                accepted.effects.extend([
                    Effect::ClearSelection(list),
                    Effect::SetCount { list, count: 0 },
                ]);
            }
            self.end_folder_work(window, hwnd, side);
            self.end_search_work(window, hwnd, side);
        }
        window.release_operations();
        accepted
    }

    /// Ends what `side` does as a folder panel besides its reading and
    /// watch: its folder sizes and their timers. The caller then releases
    /// the ended operations that its folder kept.
    pub(super) fn end_folder_work(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        window.cancel_request(Purpose::FolderSizes(side));
        hwnd.kill_timer(Timer::PanelSettle(side).id());
        hwnd.kill_timer(Timer::PanelAgain(side).id());
    }

    /// Ends what `side` does as a search panel: its search's and its
    /// selection's requests and their timers. The caller then releases the
    /// ended operations that its list kept.
    pub(super) fn end_search_work(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        window.cancel_request(Purpose::Results(List::Panel(side)));
        window.cancel_request(Purpose::Selection(List::Panel(side)));
        hwnd.kill_timer(Timer::Search(List::Panel(side)).id());
        hwnd.kill_timer(Timer::Selection(List::Panel(side)).id());
    }

    /// `side` left its search, whose box is empty for the next one.
    pub(super) fn left_search(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        self.end_search_work(window, hwnd, side);
        window.release_operations();
        self.panel(side).search.hwnd().set_text("");
    }

    pub(super) fn read(&self, window: &MainWindow, side: Side, ask: Option<ReadAsk>) {
        if let Some(ask) = ask {
            window.created().work.borrow_mut().folders.read(side, ask);
            window.retire_model();
            self.show_panel(window, side);
            // A new sort is saved state; the folder changes when the
            // reading opens, in `on_panel_events`.
            window.note_saved_state(self.panel(side).list.hwnd().parent());
        }
    }

    /// Applies what the panels' threads found. The changes of one wake
    /// apply as one batch for each panel, before any later event of that
    /// panel, so a burst of changes costs the window one pass.
    pub(super) fn on_panel_events(&self, window: &MainWindow, hwnd: Hwnd) {
        let events = window.created().work.borrow_mut().folders.drain();
        let mut pending: [Vec<FolderChange>; 2] = Default::default();
        for event in events {
            match event {
                PanelEvent::Register {
                    side,
                    generation,
                    request,
                } => {
                    let current = window
                        .created()
                        .work
                        .borrow()
                        .folders
                        .is_reading(side, generation);
                    if current {
                        let retired = request.complete(hwnd);
                        window.created().work.borrow_mut().retire(retired);
                    }
                }
                PanelEvent::Read { side, event } => {
                    self.apply_panel_changes(
                        window,
                        hwnd,
                        side,
                        std::mem::take(&mut pending[side.index()]),
                    );
                    self.on_read(window, hwnd, side, event);
                }
                PanelEvent::Changes {
                    side,
                    reading,
                    changes,
                } => {
                    if window
                        .created()
                        .work
                        .borrow()
                        .folders
                        .is_reading(side, reading)
                    {
                        pending[side.index()].extend(changes);
                    }
                }
                PanelEvent::Lost { side, reading } => {
                    self.apply_panel_changes(
                        window,
                        hwnd,
                        side,
                        std::mem::take(&mut pending[side.index()]),
                    );
                    if window
                        .created()
                        .work
                        .borrow()
                        .folders
                        .is_reading(side, reading)
                    {
                        let ask = window.model.borrow_mut().panels_mut().read_again(side);
                        self.read(window, side, ask);
                    }
                }
                PanelEvent::Free { side, folder, free } => {
                    let shown = window
                        .model
                        .borrow_mut()
                        .panels_mut()
                        .set_free(side, &folder, free);
                    if shown {
                        self.show_panel(window, side);
                    }
                }
                PanelEvent::Label { letter, label } => {
                    window
                        .model
                        .borrow_mut()
                        .panels_mut()
                        .set_label(letter, label);
                }
            }
        }
        for side in Side::BOTH {
            self.apply_panel_changes(
                window,
                hwnd,
                side,
                std::mem::take(&mut pending[side.index()]),
            );
            let reread = window.model.borrow_mut().panels_mut().take_reread(side);
            if reread {
                let ask = window.model.borrow_mut().panels_mut().read_again(side);
                self.read(window, side, ask);
            }
        }
        window.start_list_passes();
        window.retire_model();
        // A reading that opened or ended changes the saved folder and its
        // history.
        window.note_saved_state(hwnd);
    }

    /// Applies one batch of `side`'s changes and shows the rows.
    pub(super) fn apply_panel_changes(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        side: Side,
        changes: Vec<FolderChange>,
    ) {
        if changes.is_empty() {
            return;
        }
        let shown = self.place(window, side);
        let applied =
            window
                .model
                .borrow_mut()
                .panels_mut()
                .apply_changes(side, changes, shown.focused);
        let Some((place, again)) = applied else {
            return;
        };
        self.apply_panel_update(
            window,
            hwnd,
            side,
            PanelUpdate {
                before: shown,
                place,
                kind: UpdatedKind::Watch,
                read_again: again,
                sizes: false,
            },
        );
    }

    pub(super) fn apply_panel_update(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        side: Side,
        update: PanelUpdate,
    ) {
        // The panel that Show in Commander reads while Search shows takes
        // its rows before Commander mode shows it.
        if window.model.borrow().mode() != Mode::Commander
            && self.commander_reading.get() != Some(side)
        {
            return;
        }
        let shown = &update.before;
        let listed = matches!(update.kind, UpdatedKind::Listed { .. });
        if listed {
            let list = List::Panel(side);
            if let Some(input) = window.model.borrow().mark_matches(list) {
                window.created().work.borrow_mut().lists.mark(list, input);
            }
        }
        if listed || update.sizes {
            self.ask_panel_sizes(window, side);
        }
        let scroll = if matches!(update.kind, UpdatedKind::Listed { again: false }) {
            Scroll::ToFocus
        } else {
            Scroll::Keep
        };
        self.show_place(
            window,
            side,
            shown,
            &update.place,
            scroll,
            !matches!(update.kind, UpdatedKind::Listed { again: false }),
        );
        if listed || matches!(update.kind, UpdatedKind::Sizes) {
            self.fill_panel(window, side);
        }
        if matches!(
            update.kind,
            UpdatedKind::Listed { .. } | UpdatedKind::Watch | UpdatedKind::Await
        ) {
            self.show_awaited(window, side, &update.place);
        }
        if matches!(update.kind, UpdatedKind::Watch) {
            hwnd.set_timer(Timer::PanelSettle(side).id(), SETTLE_MS);
        }
        if update.read_again {
            let ask = window.model.borrow_mut().panels_mut().read_again(side);
            self.read(window, side, ask);
        }
        if listed {
            let folder = window
                .model
                .borrow()
                .panels()
                .panel(side)
                .folder()
                .to_owned();
            window
                .created()
                .work
                .borrow_mut()
                .folders
                .read_free(side, &folder);
        }
        self.show_panel(window, side);
        window.update_status();
        window.note_saved_state(hwnd);
        window.release_operations();
        // Every listing arrives here, also one calculated in the background.
        if listed {
            self.show_commander_after(window, hwnd, side);
        }
    }

    /// Applies `event` of `side`'s reading.
    pub(super) fn on_read(&self, window: &MainWindow, hwnd: Hwnd, side: Side, event: ReadEvent) {
        let shown = self.place(window, side);
        let accepted =
            window
                .model
                .borrow_mut()
                .panels_mut()
                .accept_read(side, event, shown.focused);
        self.show_read(window, hwnd, side, shown, accepted);
    }

    /// `READING_MS` after the folder of `side`'s Go opened: the folder shows
    /// reading, unless its listing came first.
    pub(super) fn on_panel_reading(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        hwnd.kill_timer(Timer::PanelReading(side).id());
        let shown = self.place(window, side);
        let accepted = window.model.borrow_mut().panels_mut().show_reading(side);
        self.show_read(window, hwnd, side, shown, accepted);
    }

    /// Shows what a reading of `side` changed in the panel, which showed
    /// `shown` before.
    fn show_read(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        side: Side,
        shown: Place,
        accepted: ReadAccepted,
    ) {
        if accepted.search_ended {
            self.left_search(window, hwnd, side);
        }
        match accepted.change {
            ReadChange::Pending => {
                window.start_list_passes();
                self.show_panel(window, side);
                return;
            }
            ReadChange::Stale => return,
            ReadChange::Opened => {
                hwnd.set_timer(Timer::PanelReading(side).id(), READING_MS);
                return;
            }
            ReadChange::Cleared => {
                window.cancel_request(Purpose::FolderSizes(side));
                self.show_place(
                    window,
                    side,
                    &shown,
                    &Place::default(),
                    Scroll::ToFocus,
                    false,
                );
            }
            ReadChange::Listed {
                place,
                again,
                stale,
            } => {
                self.apply_panel_update(
                    window,
                    hwnd,
                    side,
                    PanelUpdate {
                        before: shown,
                        place,
                        kind: UpdatedKind::Listed { again },
                        read_again: stale,
                        sizes: false,
                    },
                );
                return;
            }
            ReadChange::Refused {
                path,
                failure,
                reason,
                typed,
            } => {
                if self.commander_reading.get() == Some(side) {
                    // The person stays in Search, where a panel watches
                    // nothing; it reads its folder when Commander mode shows.
                    self.commander_reading.set(None);
                    window.created().work.borrow_mut().folders.cancel_read(side);
                } else {
                    // Starting the refused reading ended the shown folder's
                    // watch, so the panel reads its folder again, which keeps
                    // the rows and watches it again.
                    let ask = window.model.borrow_mut().panels_mut().read_again(side);
                    self.read(window, side, ask);
                }
                self.show_panel(window, side);
                let typed = self.panel(side).typed.borrow_mut().take().filter(|_| typed);
                let refused = Refused {
                    side,
                    path,
                    failure,
                    reason,
                    typed,
                };
                window.defer(hwnd, super::ModalStep::PanelRefused(refused));
                return;
            }
        }
        self.show_panel(window, side);
        window.update_status();
        // A folder that opened is still read; a drive that is not ready ends
        // the reading, and the panel says so.
        if window
            .model
            .borrow()
            .panels()
            .panel(side)
            .reading_generation()
            .is_none()
        {
            self.show_commander_after(window, hwnd, side);
        }
    }

    /// Shows Commander mode when `side`'s reading is the one Show in
    /// Commander started from Search.
    fn show_commander_after(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        if self.commander_reading.get() == Some(side) {
            self.commander_reading.set(None);
            let shown = window.model.borrow_mut().show_mode(Mode::Commander);
            window.apply(hwnd, shown);
            // The listing scrolled while the list was hidden and had no
            // height, so the entry comes into view now.
            let list = self.panel(side).list;
            if let Some(row) = list.focused() {
                list.ensure_visible(row);
            }
        }
    }

    /// The message box for a folder that could not be opened, after which
    /// the path box takes back a typed path for the person to correct
    /// (DESIGN.md: A panel).
    pub(super) fn refuse(&self, window: &MainWindow, hwnd: Hwnd, refused: Refused) {
        let Refused {
            side,
            path,
            failure,
            reason,
            typed,
        } = refused;
        let (title, text) = {
            let model = window.model.borrow();
            let strings = model.strings();
            let text = match failure {
                ReadFailure::Missing => strings.format("panel.missing", &[&path]),
                ReadFailure::NotReady => strings.format(
                    "panel.drive_not_ready",
                    &[&panel::drive_root(&path), &reason],
                ),
                ReadFailure::Other => strings.format("panel.cannot_open", &[&path, &reason]),
            };
            (strings.text("panel.open_title").to_owned(), text)
        };
        win::dialog::error_box(Some(hwnd), &text, &title);
        let c = self.panel(side);
        match typed {
            Some(text) => {
                c.path.hwnd().focus();
                c.path.hwnd().set_text(&text);
                c.path.select_to_end();
            }
            None => c.list.hwnd().focus(),
        }
    }

    /// Focuses the name that this window's operation gave an entry of
    /// `side`, once its list, which shows `shown`, has a row for it.
    pub(super) fn show_awaited(&self, window: &MainWindow, side: Side, shown: &Place) {
        let place = window.model.borrow_mut().panels_mut().show_awaited(side);
        if let Some(place) = place {
            self.show_place(window, side, shown, &place, Scroll::ToFocus, false);
        }
    }

    /// Asks gozlerd for `side`'s folder sizes.
    pub(super) fn ask_panel_sizes(&self, window: &MainWindow, side: Side) {
        let fetch = window.model.borrow_mut().ask_sizes(side);
        match fetch {
            Some(fetch) => window.start(fetch),
            None => window.cancel_request(Purpose::FolderSizes(side)),
        }
    }

    /// The settle timer of `side` ran out: its sizes and free space are
    /// read again.
    pub(super) fn on_panel_settled(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        hwnd.kill_timer(Timer::PanelSettle(side).id());
        self.ask_panel_sizes(window, side);
        let folder = window
            .model
            .borrow()
            .panels()
            .panel(side)
            .folder()
            .to_owned();
        if !folder.is_empty() {
            window
                .created()
                .work
                .borrow_mut()
                .folders
                .read_free(side, &folder);
        }
    }

    /// gozlerd answered `side`'s folder sizes: a panel sorted by size gets
    /// them when a thread has sorted its folders by them.
    pub(super) fn show_panel_sizes(&self, window: &MainWindow, side: Side) {
        let update = window.model.borrow_mut().panels_mut().take_update(side);
        if let Some(update) = update {
            let hwnd = window.controls().search.row.input.hwnd().parent();
            self.apply_panel_update(window, hwnd, side, update);
        } else {
            self.fill_panel(window, side);
            self.show_panel(window, side);
        }
        window.start_list_passes();
    }

    /// Sorts `side` by `sort`. The header shows the new sort at once; the
    /// rows follow when the reading thread has listed them in that order,
    /// or when gozlerd has sorted a search panel's search again.
    pub(super) fn sort_panel(&self, window: &MainWindow, hwnd: Hwnd, side: Side, sort: SortSpec) {
        let ask = window.model.borrow_mut().panels_mut().sort(side, sort);
        let searches = window.model.borrow().panels().search(side).is_some();
        match ask {
            None if searches => window.search(hwnd, List::Panel(side)),
            ask => self.read(window, side, ask),
        }
        self.show_panel_sort(window, side);
        window.note_saved_state(hwnd);
    }

    /// View > Sort by uses the panel where the menu opened.
    pub(super) fn change_panel_sort(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        side: Side,
        key: Option<SortKey>,
        dir: Option<SortDir>,
    ) {
        let shown = window.model.borrow().panels().panel(side).sort();
        let sort = match (key, dir) {
            (Some(key), _) if key == shown.key => shown,
            (Some(key), _) => SortSpec::default_for(key),
            (None, Some(dir)) => SortSpec { dir, ..shown },
            (None, None) => shown,
        };
        self.sort_panel(window, hwnd, side, sort);
    }

    /// Ctrl+R: the panel reads its folder again and asks for its
    /// folder sizes again, or a search panel searches again.
    pub(super) fn refresh_panel(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        if window.model.borrow().panels().search(side).is_some() {
            return window.search(hwnd, List::Panel(side));
        }
        let ask = window.model.borrow_mut().panels_mut().read_again(side);
        self.read(window, side, ask);
    }

    /// Find in Commander mode: the panel becomes a search panel of its
    /// folder, which ends its reading and its folder's requests, and its
    /// search box takes the focus with its text selected
    /// (DESIGN.md: Search in a panel).
    pub(super) fn find_in_panel(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        self.close_quick_search_of(window, side);
        let shown = self.place(window, side);
        let entered = window.model.borrow_mut().panels_mut().enter_search(side);
        if entered {
            window.created().work.borrow_mut().folders.cancel_read(side);
            self.end_folder_work(window, hwnd, side);
            window.release_operations();
            self.show_place(
                window,
                side,
                &shown,
                &Place::default(),
                Scroll::ToFocus,
                false,
            );
            window.search(hwnd, List::Panel(side));
            window.note_saved_state(hwnd);
        }
        let search = self.panel(side).search;
        search.hwnd().focus();
        search.select_all();
    }

    /// The text of `side`'s search box changed; its search runs once the
    /// person pauses.
    pub(super) fn on_panel_search_text(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        let text = self.panel(side).search.text();
        let accepted = (window.model.borrow_mut()).set_typed_search(List::Panel(side), text);
        window.apply(hwnd, accepted);
    }

    /// The person chose where `side` searches; it searches there at once.
    pub(super) fn on_look_in(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        let Some(chosen) = (self.panel(side).look_in.selected()).and_then(|i| LookIn::ALL.get(i))
        else {
            return;
        };
        if let Some(search) = window.model.borrow_mut().panels_mut().search_mut(side) {
            search.look_in = *chosen;
        }
        window.search(hwnd, List::Panel(side));
        self.show_panel(window, side);
    }

    /// Makes `side` the active panel, and with `focus` gives its list the
    /// focus. The path boxes show which is active.
    pub(super) fn activate_panel(&self, window: &MainWindow, side: Side, focus: bool) {
        let changed = window.model.borrow_mut().panels_mut().activate(side);
        // A click in the other panel closes this one's quick search box.
        self.close_quick_search_of(window, side.other());
        if focus {
            self.panel(side).list.hwnd().focus();
        }
        if changed {
            for side in Side::BOTH {
                self.panel(side).path.hwnd().invalidate();
            }
            window.update_status();
            window.note_saved_state(self.panel(side).list.hwnd().parent());
        }
    }

    /// Closes the active panel's quick search box, before a command runs
    /// (DESIGN.md: Quick search).
    pub(super) fn close_quick_search(&self, window: &MainWindow) {
        let side = window.model.borrow().panels().active();
        self.close_quick_search_of(window, side);
    }

    pub(super) fn close_quick_search_of(&self, window: &MainWindow, side: Side) {
        if window.model.borrow_mut().panels_mut().close_quick(side) {
            self.show_panel(window, side);
        }
    }

    /// A key of a panel's list or path box (DESIGN.md: Moving around, Selecting
    /// entries, Quick search). Returns whether the window took it.
    pub(super) fn on_panel_key(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        target: Hwnd,
        key: PanelKey,
    ) -> bool {
        let Some(side) = (self.panel_list(target))
            .or(self.panel_path(target))
            .or(self.panel_search(target))
            .or(self.panel_look_in(target))
        else {
            return false;
        };
        // The list never sees the keys the window takes, so it does not
        // show its focus outline, which Windows hides until the keyboard
        // is used.
        hwnd.show_focus_cues();
        let c = self.panel(side);
        if target == c.list.hwnd() && !target.is_enabled() {
            return true;
        }
        let action = (key == PanelKey::Back).then_some(QuickAction::Back);
        match self.quick_key(window, side, action) {
            Some(true) => return true,
            Some(false) if key == PanelKey::Escape => return true,
            _ => {}
        }
        match key {
            PanelKey::Enter => self.enter_row(window, side),
            PanelKey::Back => {
                let mut model = window.model.borrow_mut();
                let panels = model.panels_mut();
                let ask = match panels.search(side) {
                    Some(_) => panels.leave_search(side),
                    None => panels.go_up(side),
                };
                drop(model);
                self.read(window, side, ask);
            }
            PanelKey::OtherPanel => self.activate_panel(window, side.other(), true),
            PanelKey::ShowIn(to) => {
                let row = c.list.focused();
                let ask = window
                    .model
                    .borrow_mut()
                    .panels_mut()
                    .show_in(side, row, to);
                self.read(window, to, ask);
            }
            // The selection the list holds now decides, which the panel may
            // not have read yet.
            PanelKey::Escape => {
                let selected = c.list.selected_count() != 0;
                let ask = window
                    .model
                    .borrow_mut()
                    .panels_mut()
                    .leave_on_escape(side, selected);
                match ask {
                    Some(ask) => self.read(window, side, Some(ask)),
                    None => c.list.deselect_all(),
                }
            }
            // The focus goes to the row the person left, and selects
            // nothing, as a panel's cursor keys do.
            PanelKey::ToList => {
                c.list.hwnd().focus();
                let count = window.model.borrow().panels().panel(side).count();
                if c.list.focused().is_none() && count > 0 {
                    c.list.set_focused(Some(0));
                }
            }
            PanelKey::LeaveSearch => {
                let ask = window.model.borrow_mut().panels_mut().leave_search(side);
                c.list.hwnd().focus();
                self.read(window, side, ask);
            }
            PanelKey::LookInEnter => c.search.hwnd().focus(),
            PanelKey::PathEnter => {
                let typed = c.path.text();
                let ask = window
                    .model
                    .borrow_mut()
                    .panels_mut()
                    .go_typed(side, &typed, |name| std::env::var(name).ok());
                *self.panel(side).typed.borrow_mut() = Some(typed);
                c.list.hwnd().focus();
                self.read(window, side, ask);
            }
            PanelKey::PathEscape => {
                c.list.hwnd().focus();
                self.show_panel(window, side);
            }
        }
        true
    }

    /// While `side`'s quick search box is open, runs `action` in it, or
    /// for another key (`None`) closes the box, so the key then does what
    /// it does without the box. Returns whether `action` ran, or `None`
    /// when the box was closed already.
    pub(super) fn quick_key(
        &self,
        window: &MainWindow,
        side: Side,
        action: Option<QuickAction>,
    ) -> Option<bool> {
        if !window.model.borrow().panels().quick_active(side) {
            return None;
        }
        if let Some(action) = action {
            window
                .model
                .borrow_mut()
                .panels_mut()
                .find_quick(side, action);
            window.start_list_passes();
            return Some(true);
        }
        window.model.borrow_mut().panels_mut().close_quick(side);
        self.show_panel(window, side);
        Some(false)
    }

    /// Focuses `row` of `side`, which quick search went to, as a cursor key
    /// would, and keeps the selection.
    pub(super) fn quick_go(&self, window: &MainWindow, side: Side, row: u64) {
        self.panel(side).list.set_focused(Some(row));
        self.read_selection(window, side, false);
    }

    /// `Enter` or a double-click on the focused row of `side`, which acts
    /// on that row alone, as in Total Commander ([`PanelState::enter`]).
    pub(super) fn enter_row(&self, window: &MainWindow, side: Side) {
        let Some(row) = self.panel(side).list.focused() else {
            return;
        };
        let entered = window.model.borrow_mut().panels_mut().enter(side, row);
        match entered {
            Entered::Read(ask) => self.read(window, side, Some(ask)),
            Entered::Open(entry) => window.open_entries(
                List::Panel(side),
                EntryAction::Open,
                Entries::Ready(vec![entry]),
            ),
            Entered::Nothing => {}
        }
    }

    /// Shows `folder` in `side`, as `Enter` on a folder row does.
    pub(super) fn show_folder(&self, window: &MainWindow, side: Side, folder: String) {
        let ask = window
            .model
            .borrow_mut()
            .panels_mut()
            .go(side, folder, None);
        self.read(window, side, Some(ask));
    }

    pub(super) fn show_entry_in_commander(&self, window: &MainWindow, entry: Selected) {
        let path = String::from_utf16_lossy(&entry.path);
        let (path, focus) = if entry.is_folder {
            (path, None)
        } else {
            let name = path
                .rsplit('\\')
                .next()
                .unwrap_or(&path)
                .encode_utf16()
                .collect();
            (parent_of(&path), Some(name))
        };
        let side = window.model.borrow().panels().active();
        let ask = window.model.borrow_mut().panels_mut().go(side, path, focus);
        if window.model.borrow().mode() == Mode::Search {
            self.commander_reading.set(Some(side));
        }
        self.read(window, side, Some(ask));
    }

    /// A character typed in a panel's list: quick search (DESIGN.md: Quick
    /// search). Returns whether the window took it.
    pub(super) fn on_panel_char(
        &self,
        window: &MainWindow,
        target: Hwnd,
        character: char,
        alt: bool,
    ) -> bool {
        let Some(side) = self.panel_list(target) else {
            return false;
        };
        // Control characters, such as Ctrl with a letter, are keys, and
        // Alt+Space opens the window menu.
        if character.is_control()
            || character == ' ' && (alt || !window.model.borrow().panels().quick_active(side))
        {
            return false;
        }
        if !matches!(
            window.model.borrow().panels().panel(side).view(),
            View::Folder | View::Search(_)
        ) {
            return false;
        }
        window
            .model
            .borrow_mut()
            .panels_mut()
            .find_quick(side, QuickAction::Character(character));
        window.start_list_passes();
        true
    }

    /// Commander mode's fixed keys that run no command.
    pub(super) fn run_panel_action(&self, window: &MainWindow, hwnd: Hwnd, action: PanelAction) {
        match action {
            // A search panel's search box stands in its path box's place
            // (DESIGN.md: Search in a panel).
            PanelAction::PathBox => {
                let side = window.model.borrow().panels().active();
                let c = self.panel(side);
                let path = match c.kind.get() {
                    Some(_) => c.search,
                    None => c.path,
                };
                path.hwnd().focus();
                path.select_all();
            }
            PanelAction::Drives(side) => self.drive_menu(window, hwnd, side),
            PanelAction::MenuBar => win::window::open_menu_bar(hwnd),
            // Find in Search focuses the search box with its text selected.
            PanelAction::SearchBox => {
                let shown = window.model.borrow_mut().show_mode(Mode::Search);
                window.apply(hwnd, shown);
                window.run(hwnd, MenuItem::Run(CommandId::Find));
            }
        }
    }

    /// The drive menu of `side`, under its drive button
    /// (DESIGN.md: The drive menu).
    pub(super) fn drive_menu(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        self.activate_panel(window, side, false);
        let c = self.panel(side);
        let drives = folder::drives();
        let (nodes, states, folders) = {
            let model = window.model.borrow();
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
        // from it (DESIGN.md: The drive menu).
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
                    window
                        .model
                        .borrow_mut()
                        .panels_mut()
                        .go(side, folder.clone(), None)
                })
            }
            Some(id) => drives
                .iter()
                .find(|letter| drive_id(**letter) == id)
                .map(|letter| {
                    window
                        .model
                        .borrow_mut()
                        .panels_mut()
                        .go_drive(side, &format!(r"{letter}:\"))
                }),
            None => None,
        };
        self.read(window, side, ask);
    }

    /// A notification of `side`'s list.
    pub(super) fn on_panel_notify(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        side: Side,
        notify: ListNotify,
    ) -> Option<LRESULT> {
        match notify {
            ListNotify::GetDispInfo(mut info) => {
                self.fill_panel_cell(window, side, &mut info);
                Some(LRESULT(0))
            }
            // A folder panel has every row; a search panel asks for those
            // it shows.
            ListNotify::CacheHint(_) => {
                window.fetch_visible(List::Panel(side));
                Some(LRESULT(0))
            }
            ListNotify::ColumnClick(index) => {
                self.close_quick_search_of(window, side);
                let column = self.panel(side).columns().get(index);
                if let Some(key) = column.and_then(|c| c.sort_key()) {
                    let shown = window.model.borrow().panels().panel(side).sort();
                    self.sort_panel(window, hwnd, side, panel::header_sort(shown, key));
                }
                Some(LRESULT(0))
            }
            ListNotify::CustomDraw(mut draw) => {
                let answer = match draw.stage() {
                    DrawStage::Paint => DrawAnswer::EachRow,
                    DrawStage::Row { row } => self.draw_panel_row(window, side, &mut draw, row),
                    DrawStage::RowPainted => {
                        draw.restore_font(&window.created().fonts.borrow().ui);
                        DrawAnswer::Default
                    }
                    DrawStage::Cell { .. } | DrawStage::Other => DrawAnswer::Default,
                };
                Some(draw.answer(answer, false))
            }
            // Read later, because the list tells them while the window may
            // hold the model.
            ListNotify::SelectionChanged(change) => {
                if !window.selection_changed(List::Panel(side), change) {
                    return None;
                }
                self.list_changed(hwnd, side, ListChanged::Selection);
                None
            }
            ListNotify::FocusChanged => {
                if window.native_update_active(List::Panel(side)) {
                    return None;
                }
                self.list_changed(hwnd, side, ListChanged::Focus);
                None
            }
            // A click anywhere in the list, or the focus leaving it, closes
            // the quick search box (DESIGN.md: Quick search).
            ListNotify::Clicked | ListNotify::Unfocused => {
                self.close_quick_search_of(window, side);
                None
            }
            ListNotify::Activated => {
                self.enter_row(window, side);
                None
            }
            ListNotify::Focused => {
                self.activate_panel(window, side, false);
                None
            }
            ListNotify::ColumnWidthChanged(index) => {
                let c = self.panel(side);
                let header = {
                    let model = window.model.borrow();
                    c.columns()
                        .get(index)
                        .map(|column| captions::panel_header(model.strings(), *column))
                };
                if let Some(header) = header {
                    c.list
                        .set_header(index, c.list.fitting_text(index, &header));
                }
                None
            }
            ListNotify::BeginDrag => {
                self.close_quick_search_of(window, side);
                window.drag_selected(hwnd, List::Panel(side));
                None
            }
            ListNotify::DividerDoubleClick(_) | ListNotify::ColumnDragEnded | ListNotify::Other => {
                None
            }
        }
    }

    /// Reads the panels' selections that their lists changed, once per
    /// person's action, as Search reads its list's (DESIGN.md: The
    /// selection).
    pub(super) fn read_panel_selection(&self, window: &MainWindow) -> bool {
        let mut ready = true;
        for side in Side::BOTH {
            ready &= self.read_selection(window, side, true);
        }
        ready
    }

    /// Notes that `side`'s list changed `change`, which the panel reads on
    /// the next `DEFERRED` ([`Hwnd::post_deferred`]), so the status line and
    /// the key bar change with the rows.
    pub(super) fn list_changed(&self, hwnd: Hwnd, side: Side, change: ListChanged) {
        let changed = &self.panel(side).changed;
        if changed.get() == ListChanged::Nothing {
            hwnd.post_deferred();
        }
        changed.set(changed.get().max(change));
    }

    /// Reads `side`'s selection and focused row when its list changed them;
    /// a change of the focus alone reads only the focused row, because
    /// `Insert` moves it and the selection can hold every row.
    /// `..` cannot be selected, and with `closes_quick` a change of the
    /// selection, such as a click, closes the quick search box. A search
    /// panel's list then reads the selected results, as Search's does.
    pub(super) fn read_selection(
        &self,
        window: &MainWindow,
        side: Side,
        closes_quick: bool,
    ) -> bool {
        if window.native_update_active(List::Panel(side))
            || window.native_deselection_pending(List::Panel(side))
        {
            return false;
        }
        let list = self.panel(side).list;
        match self.panel(side).changed.replace(ListChanged::Nothing) {
            ListChanged::Nothing => return true,
            ListChanged::Focus => {
                let focused = list.focused();
                let nothing_selected = list.selected_count() == 0;
                let accepted = {
                    let mut model = window.model.borrow_mut();
                    model.panels_mut().set_focused(side, focused);
                    let search = model.panels().search(side).is_some();
                    (search && nothing_selected)
                        .then(|| model.set_selection(List::Panel(side), Vec::new().into(), focused))
                };
                if let Some(accepted) = accepted {
                    window.apply(list.hwnd().parent(), accepted);
                }
                // The status line shows the focused result's folder.
                if self.panel(side).kind.get() == Some(LookIn::AllDrives) {
                    self.show_panel(window, side);
                }
                window.controls().key_bar.update_key_bar(window);
                return true;
            }
            ListChanged::Selection => {}
        }
        window
            .model
            .borrow_mut()
            .panels_mut()
            .native_selection_pending(side, true);
        let up = matches!(window.model.borrow().panels().row(side, 0), Some(Row::Up));
        if up && list.row_state(0).1 {
            list.select(0, false);
            self.panel(side).changed.set(ListChanged::Nothing);
        }
        let searching = window.model.borrow().panels().search(side).is_some();
        let count = list.selected_count();
        if searching && count > SELECTION_LIMIT {
            let accepted = (window.model.borrow_mut()).select_too_many(List::Panel(side), count);
            window.apply(list.hwnd().parent(), accepted);
            self.show_panel(window, side);
            window
                .model
                .borrow_mut()
                .panels_mut()
                .native_selection_pending(side, false);
            return true;
        }
        let Some(selected) = window.selected_rows(List::Panel(side)) else {
            self.panel(side).changed.set(ListChanged::Selection);
            return false;
        };
        let focused = list.focused();
        let search = {
            let mut model = window.model.borrow_mut();
            let panels = model.panels_mut();
            panels.native_selection_pending(side, false);
            let changed = panels.set_selected(side, selected.clone());
            if changed && closes_quick {
                panels.close_quick(side);
            }
            panels.set_focused(side, focused);
            changed && panels.search(side).is_some()
        };
        if search {
            let accepted =
                window
                    .model
                    .borrow_mut()
                    .set_selection(List::Panel(side), selected, focused);
            window.apply(list.hwnd().parent(), accepted);
        }
        window.start_list_passes();
        window.retire_model();
        self.show_panel(window, side);
        true
    }

    /// The context menu of `side`'s list (DESIGN.md: Menus in Commander mode): an
    /// entry's, at the row the person right-clicked or at the focused row for
    /// the Menu key, else the empty area's. The header shows none.
    pub(super) fn panel_menu(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        side: Side,
        point: Option<(i32, i32)>,
    ) {
        let list = self.panel(side).list;
        if point.is_some_and(|point| list.header().screen_rect().contains(point)) {
            return;
        }
        self.activate_panel(window, side, false);
        if let Some(context) = window.await_panel_menu(hwnd, side, point) {
            self.panel_menu_ready(window, hwnd, side, point, context);
        }
    }

    pub(super) fn panel_menu_ready(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        side: Side,
        point: Option<(i32, i32)>,
        context: super::selection::CommandContext,
    ) {
        let list = self.panel(side).list;
        let (point, row) = match point {
            Some(point) => (point, list.row_at(point)),
            None => match list.focused_row_point() {
                Some(point) => (point, list.focused()),
                None => {
                    let rect = list.hwnd().screen_rect();
                    ((rect.x.0, rect.y.0), None)
                }
            },
        };
        let entry = row.is_some_and(|row| {
            let model = window.model.borrow();
            matches!(
                model.panels().row(side, row),
                Some(Row::Entry(_) | Row::Result(_))
            )
        });
        let (groups, default) = match entry {
            true => (
                commands::panel_row_menu(),
                Some(MenuItem::Run(CommandId::Open)),
            ),
            false => (commands::panel_menu(), None),
        };
        window.context_menu(hwnd, &groups, point, default, context);
    }

    /// `F5` and `F6`: the destination as text, then the operation
    /// (DESIGN.md: Copy and Move). Gozler's own rename in the panel's folder
    /// keeps the focus, and the selection, on the renamed entry, which a rename
    /// by another program does not.
    pub(super) fn transfer(&self, window: &MainWindow, _hwnd: Hwnd, moves: bool, side: Side) {
        let (place, selected, count) = {
            let model = window.model.borrow();
            let panels = model.panels();
            (
                panels.command_place(side),
                panels.panel(side).has_selection(),
                panels.chosen_count(side),
            )
        };
        let Some(start) = self.transfer_start(window, side, moves, count) else {
            return;
        };
        window.prepare_selected(
            List::Panel(side),
            super::PathAction::PanelTransfer {
                moves,
                place,
                selected,
                start: Box::new(start),
            },
        );
    }

    /// What the Copy and Move dialog opens with: the other panel's folder,
    /// with the entry's name for `F6` on one entry, selected without its
    /// extension (DESIGN.md: Copy and Move). A rename keeps a search panel's
    /// result in its own folder. `None` when no entry is chosen.
    pub(super) fn transfer_start(
        &self,
        window: &MainWindow,
        side: Side,
        moves: bool,
        count: usize,
    ) -> Option<TransferStart> {
        if count == 0 {
            return None;
        }
        let model = window.model.borrow();
        let strings = model.strings();
        let numbers = window.numbers.borrow();
        let panels = model.panels();
        let active = panels.panel(side);
        let other = panels.panel(side.other());
        let chosen = if count == 1 {
            panels.chosen_source(side).and_then(|source| source.one())
        } else {
            None
        };
        let one = match &chosen {
            Some(entry) => {
                let path = String::from_utf16_lossy(&entry.path);
                Some((panel::last_name(&path).to_owned(), entry.is_folder))
            }
            _ => None,
        };
        let (title, name_line, items_line) = match moves {
            false => (
                "file_operations.copy_to_title",
                "transfer.copy_name",
                "transfer.copy_items",
            ),
            true => (
                "file_operations.move_to_title",
                "transfer.move_name",
                "transfer.move_items",
            ),
        };
        let line = match &one {
            Some((name, _)) => strings.format(name_line, &[name]),
            None => strings.count(items_line, count as u64, &*numbers, &[]),
        };
        let other =
            (other.destination()).map_or_else(String::new, |folder| panel::join(folder, ""));
        let mut text = other.clone();
        let units = |text: &str| text.encode_utf16().count();
        let selected = match &one {
            Some((name, folder)) if moves && !text.is_empty() => {
                let start = units(&text);
                text.push_str(name);
                let stem = match name.rfind('.') {
                    Some(dot) if dot > 0 && !folder => &name[..dot],
                    _ => name,
                };
                (start, start + units(stem))
            }
            _ => (0, units(&text)),
        };
        let folder = match (active.search(), &chosen) {
            (Some(_), Some(entry)) => parent_of(&String::from_utf16_lossy(&entry.path)),
            _ => active.folder().to_owned(),
        };
        Some(TransferStart {
            policy: model.choices().copy_policy,
            mode: if moves {
                TransferMode::Move
            } else {
                TransferMode::Copy {
                    single_folder: one.as_ref().is_some_and(|(_, folder)| *folder),
                }
            },
            title: strings.text(title).to_owned(),
            line,
            text,
            selected,
            folder,
            other,
            one: one.map(|(name, _)| name),
        })
    }

    /// `F7`: the new folder's name, then the operation, after which the
    /// focus goes to the folder (DESIGN.md: New folder).
    pub(super) fn new_folder(&self, window: &MainWindow, hwnd: Hwnd, side: Side) {
        let folder = window
            .model
            .borrow()
            .panels()
            .panel(side)
            .folder()
            .to_owned();
        let shown_name = |name: &str| window.model.borrow().panels().panel(side).shown_name(name);
        let dialog = NewFolder::new(window.model.borrow().strings(), &shown_name);
        if window.open_dialog(hwnd, DialogId::NewFolder, &dialog) != Some(DialogEnd::Ok) {
            return;
        }
        let Some(name) = dialog.chosen() else {
            return;
        };
        let path = PathBuf::from(panel::join(&folder, &name));
        if let Some(id) = window.start_operation(Request::NewFolder { path }, None) {
            window
                .model
                .borrow_mut()
                .panels_mut()
                .await_name(side, &name, None, false, id);
        }
    }

    /// `Num +` and `Num -`: the mask, then the files of `side`'s folder that
    /// it matches are selected, or unselected when not `select`
    /// (DESIGN.md: Selecting entries).
    pub(super) fn select_files(&self, window: &MainWindow, hwnd: Hwnd, side: Side, select: bool) {
        let dialog = SelectFiles::new(&self.select_mask.borrow());
        let id = if select {
            DialogId::SelectFiles
        } else {
            DialogId::UnselectFiles
        };
        if window.open_dialog(hwnd, id, &dialog) != Some(DialogEnd::Ok) {
            return;
        }
        let Some((text, mask)) = dialog.chosen() else {
            return;
        };
        self.select_mask.replace(text);
        let shown = self.place(window, side);
        let place = (window.model.borrow_mut().panels_mut()).select_matching(side, &mask, select);
        if let Some(place) = place {
            self.show_place(window, side, &shown, &place, Scroll::Keep, false);
        }
    }

    /// After a delete is submitted, the selection is cleared and the first
    /// row after the deleted ones is focused (DESIGN.md: Delete). A search
    /// panel's rows stay in place, struck through, so its focus stays.
    pub(super) fn focus_after_chosen(&self, window: &MainWindow) {
        let side = window.model.borrow().panels().active();
        let row = {
            let model = window.model.borrow();
            let panel = model.panels().panel(side);
            panel
                .search()
                .map_or_else(|| panel.row_after_chosen(), |_| None)
        };
        let list = self.panel(side).list;
        list.deselect_all();
        if row.is_some() {
            list.set_focused(row);
        }
    }

    /// The cell's text and the row's icon of `side`'s list, which the list
    /// draws.
    pub(super) fn fill_panel_cell(&self, window: &MainWindow, side: Side, info: &mut DispInfo) {
        let (text, icon) = {
            let model = window.model.borrow();
            let numbers = window.numbers.borrow();
            let panels = model.panels();
            let sizes = model.choices().size_format;
            let columns = self.panel(side).columns();
            match (
                shown_or_taken(panels, side, info.row()),
                columns.get(info.column()),
            ) {
                (Some(row), Some(column)) => (
                    info.wants_text().then(|| {
                        let panel = panels.panel(side);
                        captions::panel_cell(
                            model.strings(),
                            &*numbers,
                            sizes,
                            &window.dates.borrow(),
                            panel,
                            row,
                            *column,
                        )
                    }),
                    Some(icon_of(row)),
                ),
                _ => (None, None),
            }
        };
        if info.wants_text() {
            let text = text.unwrap_or_default();
            info.set_text(self.panel(side).list.fitting_text(info.column(), &text));
        }
        if info.wants_image() {
            info.set_image(icon.map(|icon| match icon {
                RowIcon::Up => window.icons.up(),
                RowIcon::Item(name, is_folder) => window.icons.index(&name, is_folder),
            }));
        }
    }

    /// Has the list draw `row` of `side` in grey when it is a hidden or
    /// system entry, or struck through in grey when a delete or a move took
    /// it (DESIGN.md: Selecting entries); the list draws every other row as
    /// it is.
    pub(super) fn draw_panel_row(
        &self,
        window: &MainWindow,
        side: Side,
        draw: &mut CustomDraw,
        row: u64,
    ) -> DrawAnswer {
        let (taken, dimmed) = {
            let model = window.model.borrow();
            let taken = model.taken_rows(List::Panel(side), std::slice::from_ref(&(row..row + 1)));
            let shown = model.panels().row(side, row);
            (
                !taken.is_empty(),
                matches!(shown, Some(Row::Entry(entry)) if entry.is_dimmed()),
            )
        };
        if taken {
            draw.draw_row_struck(&window.created().fonts.borrow().struck);
            DrawAnswer::NewFont
        } else if dimmed {
            draw.draw_row_grey();
            DrawAnswer::NewColour
        } else {
            DrawAnswer::Default
        }
    }

    /// The colors of a path box: the system highlight for the active
    /// panel's while it does not have the focus, as Total Commander shows
    /// its active path (DESIGN.md: A panel).
    pub(super) fn path_box_colors(
        &self,
        window: &MainWindow,
        colors: StaticColors,
        control: Hwnd,
    ) -> Option<LRESULT> {
        let side = self.panel_path(control)?;
        let active = window.model.borrow().panels().active() == side;
        (active && Hwnd::focused() != Some(control)).then(|| colors.highlight())
    }

    /// Windows asks to remove, or removed, the drive whose watch it names
    /// with `notification` (DESIGN.md: A panel).
    pub(super) fn on_device_removal(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        removal: Removal,
        notification: isize,
    ) -> bool {
        let (side, allowed) = window
            .created()
            .work
            .borrow_mut()
            .folders
            .removal(notification, removal);
        let Some(side) = side else {
            return allowed;
        };
        match removal {
            // The worker was signalled; resource cleanup never holds this callback.
            Removal::Query => {}
            // Another program kept the drive, so the reading that the query
            // stopped starts again, or the panel follows its folder again.
            Removal::Failed => {
                let ask = window.model.borrow_mut().panels_mut().restart_reading(side);
                self.read(window, side, ask);
            }
            Removal::Complete => {
                let shown = self.place(window, side);
                if window.model.borrow_mut().panels_mut().drive_removed(side) {
                    self.left_search(window, hwnd, side);
                }
                self.show_place(
                    window,
                    side,
                    &shown,
                    &Place::default(),
                    Scroll::ToFocus,
                    false,
                );
                self.show_panel(window, side);
                window.note_saved_state(hwnd);
            }
        }
        window.retire_model();
        allowed
    }

    /// Gives every list the taken entries `taken` and shows the panels
    /// after them (DESIGN.md: Taken entries). Returns what
    /// Search's list shows.
    pub(super) fn show_panels_taken(
        &self,
        window: &MainWindow,
        taken: std::sync::Arc<gozler_fileops::Taken>,
    ) -> TakenShown {
        let shown = Side::BOTH.map(|side| self.place(window, side));
        let taken = window
            .model
            .borrow_mut()
            .set_taken(taken, [shown[0].focused, shown[1].focused]);
        for (side, rows) in Side::BOTH.into_iter().zip(&taken.panels) {
            match rows {
                TakenRows::Pending => {}
                TakenRows::Folder(place) => {
                    self.show_place(
                        window,
                        side,
                        &shown[side.index()],
                        place,
                        Scroll::Keep,
                        true,
                    );
                }
                TakenRows::Search { deselect } => {
                    window.show_list_taken(List::Panel(side), *deselect);
                }
            }
            self.show_panel(window, side);
        }
        taken
    }

    /// The folder panels that operations ended in read their folders again,
    /// or ask for their sizes, as `follow` says.
    pub(super) fn follow_operations(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        follow: &[PanelFollow],
    ) {
        if window.model.borrow().mode() != Mode::Commander {
            return;
        }
        for follow in follow {
            let side = follow.side;
            if follow.read_again {
                let ask = window.model.borrow_mut().panels_mut().read_again(side);
                self.read(window, side, ask);
            }
            if follow.sizes {
                self.ask_panel_sizes(window, side);
                hwnd.set_timer(Timer::PanelAgain(side).id(), AGAIN_MS);
            }
        }
    }
}
