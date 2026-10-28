//! The panels of Commander mode (DESIGN.md: A panel): what each panel's list shows,
//! and every decision about it that needs no window.
//!
//! A panel lists the disk itself, because gozlerd does not index USB,
//! network and FAT drives; gozlerd gives only the folder sizes. The reading
//! thread (`win::folder`) lists a folder and the watching thread follows
//! it; both answer with plain values that [`Panels`] applies, so the list
//! never waits for the disk and every rule here is tested without one.
//!
//! - **The rows** are the folder's entries, folders first and each group
//!   sorted by the panel's sort, less the entries that a delete or a move
//!   takes. The first row is `..` except at a drive's or a share's root.
//! - **The list owns the selection and the focus** (DESIGN.md: The
//!   selection). A change that moves rows takes the list's [`Place`] by row
//!   and gives it back by row, kept on the same names, so the person never
//!   loses their place.
//! - **Taken entries** (DESIGN.md: Taken entries): a panel
//!   keeps an ended delete or move whose end makes it read its folder
//!   again, until the first complete listing that it started after the end,
//!   because the change notifications and the end arrive in no fixed order.
//!   A moment counter orders the ends and the starts of the readings.

use super::fetch::{Ask, ChunkAsk, FromSnapshot, Generation, Outcome};
use super::model::Column;
use super::query::{self, Filters, Matching};
use super::search_list::{Deselect, SearchList};
use super::selection::{DragEntry, RowSelection, push_rows, wide_path};
use super::work::{self, Pass, Retirement};
use gozler_core::fold::fold;
use gozler_core::proto::ResultItem;
use gozler_core::query::{MaskScratch, NameMask, QueryStart};
use gozler_core::types::{SortDir, SortKey, SortSpec};
use gozler_core::winpath::{absolute, is_root, parent_of};
use gozler_core::wtf8;
use gozler_fileops::{OperationId, Request, Taken};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

mod calculation;
mod counting;
mod quick;
mod rows;
pub(crate) use calculation::{Calculated, Calculation, PanelUpdate, UpdatedKind};
use calculation::{Change as RowChange, End, Input, Interaction};
pub(crate) use counting::{Counted, Counting};
use quick::Quick;
pub(crate) use quick::{Finding, Found, QuickAction};
use rows::{FolderRows, Name, name_hash, name_key, same_name};
pub(crate) use rows::{give_sizes, sort_listing};

const PENDING_ROWS: usize = 32;

/// How many folders a panel's drive menu offers (DESIGN.md: The drive menu).
pub(crate) const HISTORY: usize = 10;

/// The most subfolders whose sizes one request asks gozlerd for.
const SIZE_ROWS: u64 = 100_000;

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// One of the two panels. Its names are `ui.json`'s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Side {
    #[default]
    Left,
    Right,
}

impl Side {
    pub(crate) const BOTH: [Side; 2] = [Side::Left, Side::Right];

    pub(crate) fn index(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }

    pub(crate) fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// One entry of a panel's folder, as a listing or a change read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The name as raw UTF-16, which may hold unpaired surrogates, because
    /// a file operation names the entry by it.
    pub(crate) name: Box<[u16]>,
    pub(crate) attributes: u32,
    /// A file's size in bytes; a folder's size from gozlerd while `sized`,
    /// else 0 ([`Entry::folder_size`]).
    pub(crate) size: u64,
    /// The last write time as a `FILETIME` value.
    pub(crate) modified: i64,
    /// The hash of the name's folded form, made once by the thread that read
    /// the entry. The window finds a name by it and folds a name only when
    /// the hash matches, because a stored folded name would take more than
    /// the 100 bytes an entry may (DESIGN.md: Memory budget).
    hash: u64,
    /// Whether `size` holds a folder's size from gozlerd: a flag, because an
    /// `Option` would add 8 bytes to each entry.
    sized: bool,
}

impl Entry {
    pub(crate) fn new(name: Box<[u16]>, attributes: u32, size: u64, modified: i64) -> Entry {
        Entry {
            hash: name_hash(&name_key(&name)),
            name,
            attributes,
            size,
            modified,
            sized: false,
        }
    }

    /// A folder's size from gozlerd; `None` for a file, or while gozlerd has
    /// not given it.
    pub(crate) fn folder_size(&self) -> Option<u64> {
        (self.is_folder() && self.sized).then_some(self.size)
    }

    fn set_folder_size(&mut self, size: Option<u64>) {
        if self.is_folder() {
            self.sized = size.is_some();
            self.size = size.unwrap_or(0);
        }
    }

    pub(crate) fn is_folder(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    /// A hidden or system entry, which the list shows in the gray text
    /// color, as File Explorer does.
    pub(crate) fn is_dimmed(&self) -> bool {
        self.attributes & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
    }

    pub(crate) fn display_name(&self) -> String {
        String::from_utf16_lossy(&self.name)
    }

    /// The extension without its dot, as Search's Extension column has it;
    /// empty for a folder.
    pub(crate) fn extension(&self) -> String {
        let name = self.display_name();
        gozler_core::query::extension(name.as_bytes(), self.is_folder())
            .map(|ext| String::from_utf8_lossy(ext).into_owned())
            .unwrap_or_default()
    }

    /// The size the status line adds: a file's, or a folder's from gozlerd.
    fn counted_size(&self) -> Option<u64> {
        if self.is_folder() {
            self.folder_size()
        } else {
            Some(self.size)
        }
    }

    /// Its name, to find it again after the rows change.
    fn name(&self) -> Name {
        Name {
            hash: self.hash,
            raw: self.name.clone(),
        }
    }

    /// Whether its name is `name`, without regard to case, as NTFS compares
    /// names; [`Panel::row_of`] looks for the exact name first.
    fn is(&self, name: &Name) -> bool {
        self.hash == name.hash && same_name(&self.name, &name.raw)
    }
}

/// A row of a panel's list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Row<'a> {
    /// `..`, which no command acts on.
    Up,
    Entry(&'a Entry),
    /// A search panel's fetched row that no operation has taken.
    Result(&'a ResultItem),
}

/// A change of the shown folder that the watching thread read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FolderChange {
    /// An added entry, under its name as stored on the disk.
    Added(Entry),
    /// A changed entry, under the name the program that wrote it opened it
    /// by, which may differ in case from the stored name.
    Modified(Entry),
    Gone(Box<[u16]>),
    /// `to` is under its name as stored on the disk.
    Renamed {
        from: Box<[u16]>,
        to: Entry,
    },
}

/// The list's focused row and selected rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Place {
    pub(crate) focused: Option<u64>,
    /// Sorted ranges that do not touch, as the list gives them.
    pub(crate) selected: Arc<Vec<Range<u64>>>,
}

/// What a panel's list area shows (DESIGN.md: A panel).
#[derive(Debug)]
pub(crate) enum View {
    /// The folder is open and its listing has not completed.
    Reading,
    Cancelled,
    Folder,
    /// The shown drive was removed or stopped answering; Retry reads the
    /// folder again.
    NotReady {
        drive: String,
    },
    /// The panel shows the results of a search (DESIGN.md: Search in a panel).
    /// Leaving the search drops it with its rows, its selection and its kept
    /// operations, so a reply that arrives later finds no list.
    Search(Box<PanelSearch>),
}

/// A search panel's search.
#[derive(Debug)]
pub(crate) struct PanelSearch {
    pub(crate) list: SearchList,
    pub(crate) look_in: LookIn,
    /// The name of the row that was focused before the search, which
    /// `Esc` and `Backspace` focus again.
    back: Option<Box<[u16]>>,
}

/// Where a search panel searches (DESIGN.md: Search in a panel).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LookIn {
    /// The panel's folder and its subfolders.
    #[default]
    ThisFolder,
    AllDrives,
}

impl LookIn {
    /// In the Look in list's order.
    pub(crate) const ALL: [LookIn; 2] = [LookIn::ThisFolder, LookIn::AllDrives];
}

pub(crate) use super::open::Selected as ChosenEntry;

/// What `Enter` on a panel row does ([`Panels::enter`]).
pub(crate) enum Entered {
    /// `..` or a folder, which the panel reads.
    Read(ReadAsk),
    /// A file, which the window opens as Windows does, so an `.exe` runs.
    Open(ChosenEntry),
    /// A blank row, or `..` of a drive's root.
    Nothing,
}

#[derive(Clone, Copy)]
pub(crate) struct CommandPlace {
    pub(crate) side: Side,
    epoch: u64,
    interaction: u64,
}

pub(crate) struct PanelEntries {
    rows: Arc<FolderRows>,
    selected: Arc<Vec<Range<u64>>>,
    focused: Option<u64>,
    folder: String,
    up: u64,
}

impl PanelEntries {
    pub(crate) fn items(&self) -> usize {
        self.rows.items().saturating_add(self.selected.len())
    }

    pub(crate) fn resolve(self) -> Vec<ChosenEntry> {
        self.map(|path, entry| ChosenEntry {
            path,
            is_folder: entry.is_folder(),
        })
    }

    pub(crate) fn dragged(self) -> Vec<DragEntry> {
        self.map(|path, entry| DragEntry {
            path,
            is_folder: entry.is_folder(),
            attributes: Some(entry.attributes),
            size: Some(entry.size),
            modified: Some(entry.modified),
        })
    }

    /// `make` of each chosen entry's full path and entry, in row order.
    fn map<T>(self, make: impl Fn(Vec<u16>, &Entry) -> T) -> Vec<T> {
        let mut base: Vec<u16> = self.folder.encode_utf16().collect();
        if base.last() != Some(&u16::from(b'\\')) {
            base.push(u16::from(b'\\'));
        }
        let entry = |row| {
            let entry = self.rows.entry(row, self.up)?;
            let mut path = Vec::with_capacity(base.len() + entry.name.len());
            path.extend_from_slice(&base);
            path.extend_from_slice(&entry.name);
            Some(make(path, entry))
        };
        if self.selected.is_empty() {
            self.focused.and_then(entry).into_iter().collect()
        } else {
            self.selected
                .iter()
                .flat_map(Clone::clone)
                .filter_map(entry)
                .collect()
        }
    }

    pub(crate) fn one(&self) -> Option<ChosenEntry> {
        let row = match self.selected.as_slice() {
            [] => self.focused?,
            [range] if range.end == range.start + 1 => range.start,
            _ => return None,
        };
        let entry = self.rows.entry(row, self.up)?;
        let mut path: Vec<u16> = self.folder.encode_utf16().collect();
        if path.last() != Some(&u16::from(b'\\')) {
            path.push(u16::from(b'\\'));
        }
        path.extend_from_slice(&entry.name);
        Some(ChosenEntry {
            path,
            is_folder: entry.is_folder(),
        })
    }
}

/// What a folder panel does after operations ended
/// ([`Panels::accept_operations`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PanelFollow {
    pub(crate) side: Side,
    /// It reads its folder again ([`ends_in`]).
    pub(crate) read_again: bool,
    /// It asks for its folder sizes again ([`changes_sizes`]).
    pub(crate) sizes: bool,
}

/// What new taken entries change in a panel's list ([`Panels::set_taken`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TakenRows {
    Pending,
    /// A folder panel's rows moved; the list shows this place.
    Folder(Place),
    /// A search panel's taken rows show blank in place, and its list
    /// unhighlights the rows `deselect` says.
    Search {
        deselect: Deselect,
    },
}

/// A column of a panel's list (DESIGN.md: A panel, Search in a panel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelColumn {
    Name,
    Ext,
    /// A search panel's: where each result is.
    Folder,
    Size,
    DateModified,
}

impl PanelColumn {
    /// A folder panel's columns, in their order.
    pub(crate) const FOLDER: [PanelColumn; 4] = [
        PanelColumn::Name,
        PanelColumn::Ext,
        PanelColumn::Size,
        PanelColumn::DateModified,
    ];
    /// A search panel's columns, in their order.
    pub(crate) const SEARCH: [PanelColumn; 5] = [
        PanelColumn::Name,
        PanelColumn::Ext,
        PanelColumn::Folder,
        PanelColumn::Size,
        PanelColumn::DateModified,
    ];

    /// What a click on its header sorts by; `None` for Folder, whose
    /// header does not sort (DESIGN.md: Search in a panel).
    pub(crate) fn sort_key(self) -> Option<SortKey> {
        match self {
            PanelColumn::Name => Some(SortKey::Name),
            PanelColumn::Ext => Some(SortKey::Extension),
            PanelColumn::Folder => None,
            PanelColumn::Size => Some(SortKey::Size),
            PanelColumn::DateModified => Some(SortKey::DateModified),
        }
    }

    /// Sizes are right-aligned, so their digits line up.
    pub(crate) fn right_aligned(self) -> bool {
        self == PanelColumn::Size
    }

    /// The Search column whose default width it takes, and whose text a
    /// result shows in it; Folder's is Path's.
    pub(crate) fn search_column(self) -> Column {
        match self {
            PanelColumn::Name => Column::Name,
            PanelColumn::Ext => Column::Extension,
            PanelColumn::Folder => Column::Path,
            PanelColumn::Size => Column::Size,
            PanelColumn::DateModified => Column::DateModified,
        }
    }
}

/// A listing the panel asks its reading thread for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReadAsk {
    pub(crate) generation: u64,
    pub(crate) path: String,
    /// A folder that does not exist is replaced by the nearest folder above
    /// it that exists, as for the shown folder and the saved one.
    pub(crate) nearest: bool,
    /// The panel's sort, which the reading thread sorts the listing by, so
    /// a large folder is never sorted on the window's thread.
    pub(crate) sort: SortSpec,
    /// The folder sizes the panel knows, when it reads its shown folder
    /// again: the reading thread gives them to the listing before it sorts,
    /// so a panel sorted by size keeps its order and its sizes.
    pub(crate) sizes: ReadSizes,
}

#[derive(Clone, Debug)]
pub(crate) struct ReadSizes(Option<Arc<FolderRows>>);

impl PartialEq for ReadSizes {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Eq for ReadSizes {}

impl ReadSizes {
    pub(crate) fn values(&self) -> Vec<(Box<[u16]>, u64)> {
        self.0
            .as_ref()
            .into_iter()
            .flat_map(|rows| rows.entries())
            .filter_map(|entry| Some((entry.name.clone(), entry.folder_size()?)))
            .collect()
    }
}

/// Why the reading thread could not list a folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadFailure {
    /// The path names nothing.
    Missing,
    /// The drive is not ready or is gone.
    NotReady,
    Other,
}

/// What the reading thread answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReadEvent {
    /// The folder opened, so its listing follows.
    Opened { generation: u64, folder: String },
    /// The complete listing of `folder`. `focus` names the file that a
    /// typed path named, whose folder the panel shows.
    Listed {
        generation: u64,
        folder: String,
        entries: Vec<Entry>,
        focus: Option<Box<[u16]>>,
    },
    Failed {
        generation: u64,
        path: String,
        failure: ReadFailure,
        /// Windows' reason, such as "Access is denied."
        reason: String,
    },
}

/// What a reading's answer means for the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReadChange {
    Pending,
    /// An older reading, which changes nothing.
    Stale,
    /// A Go's folder opened. The panel still shows the folder it leaves,
    /// until the listing replaces it at once, or the window shows the new
    /// folder reading ([`Panels::show_reading`]).
    Opened,
    /// The rows went: the folder a Go opened is read, and the list says
    /// "Reading…", or the shown drive is not ready.
    Cleared,
    /// The listing replaced the rows; the list shows `place`. `again` when
    /// it was the shown folder read again in the same order, whose rows
    /// stay where the person sees them. `stale` when an operation that
    /// ended while it was read still hides rows, so the panel reads its
    /// folder once more.
    Listed {
        place: Place,
        again: bool,
        stale: bool,
    },
    /// The folder could not be opened, so the panel stays where it was.
    /// The window says why in a message box; `typed` keeps the typed text
    /// in the path box.
    Refused {
        path: String,
        failure: ReadFailure,
        reason: String,
        typed: bool,
    },
}

/// What a reading's answer did ([`Panels::accept_read`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReadAccepted {
    pub(crate) change: ReadChange,
    /// The panel searched until this answer, which replaced its search, so
    /// the window ends the search's requests and timers.
    pub(crate) search_ended: bool,
}

/// How a reading started, which decides what its answer does.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ReadKind {
    /// Another folder, whose rows replace the shown ones once it opens;
    /// `focus` names the row to focus, such as the folder the panel came
    /// from.
    Go {
        focus: Option<Box<[u16]>>,
        typed: bool,
    },
    /// The shown folder again, whose rows stay until the listing applies
    /// by name.
    Again,
}

#[derive(Clone, Debug)]
struct Read {
    generation: u64,
    kind: ReadKind,
    path: String,
    /// Whether a missing folder gives way to the nearest one above it.
    nearest: bool,
    /// The sort its listing comes in.
    sort: SortSpec,
    reached: Reached,
}

/// How far a Go got. The panel keeps showing the folder it leaves until the
/// listing's rows replace it, or until [`Panels::show_reading`] shows the
/// opened folder reading.
#[derive(Clone, Debug)]
enum Reached {
    Asked,
    /// The folder opened, and its listing has not come.
    Opened(String),
    /// The listing came, and its rows are being worked out. Showing the
    /// folder reading now would drop them.
    Listed(String),
}

/// What the panel's status line counts (DESIGN.md: A panel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Counts {
    /// The entries, without `..`.
    pub(crate) items: u64,
    /// The selected entries, when any is.
    pub(crate) selected: Option<u64>,
    /// The counted entries' sizes; `None` in an empty folder.
    pub(crate) size: Option<Totals>,
}

/// A sum of entries' sizes. A panel keeps the one of all its entries as
/// entries come and go, so the status line never adds up a large folder
/// again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    /// The sum of the known sizes.
    pub(crate) bytes: u64,
    /// The folders whose size is unknown.
    pub(crate) unknown: u64,
}

impl Totals {
    fn of<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> Totals {
        let mut totals = Totals::default();
        for entry in entries {
            totals.add(entry);
        }
        totals
    }

    fn add(&mut self, entry: &Entry) {
        match entry.counted_size() {
            Some(bytes) => self.bytes += bytes,
            None => self.unknown += 1,
        }
    }

    fn remove(&mut self, entry: &Entry) {
        match entry.counted_size() {
            Some(bytes) => self.bytes -= bytes,
            None => self.unknown -= 1,
        }
    }
}

/// What a panel saves in `ui.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedPanel {
    /// `None` until the panel first shows a folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) folder: Option<String>,
    #[serde(default)]
    pub(crate) sort: SortSpec,
    /// The folders the panel showed last, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) history: Vec<String>,
}

/// One panel.
#[derive(Debug)]
pub(crate) struct Panel {
    /// The shown folder; empty before the first listing.
    folder: String,
    view: View,
    rows: Arc<FolderRows>,
    /// The sort the person chose, which the header shows.
    sort: SortSpec,
    history: Vec<String>,
    /// The folder the panel last showed on each drive in this session, by
    /// the drive's folded root.
    drive_folders: HashMap<Vec<u8>, String>,
    read: Option<Read>,
    sizes_asked: Option<Generation>,
    /// gozlerd answered a sizes request since the list was emptied, so a
    /// folder without a size is unknown, not yet asked.
    sized: bool,
    free: Option<u64>,
    /// The quick search box's text while it is open.
    quick: Quick,
    /// The list's selected rows as it last told them, which the status
    /// line counts and Clear selection follows.
    selected: Arc<Vec<Range<u64>>>,
    counted: CountState,
    selected_count: u64,
    /// The saved folder, which the first reading shows.
    saved: Option<String>,
    /// The focused name that a change took away, while the focus shows on
    /// a neighbouring row ([`Panels::apply_changes`]).
    gone_focus: Option<GoneFocus>,
    /// The list's focused row as it last told it, which the commands act
    /// on when no entry is selected.
    focused: Option<u64>,
    /// The name that Gozler's own New folder or rename gives an entry in
    /// the shown folder, which the focus goes to when a row shows it.
    awaited: Option<Awaited>,
    awaited_place: Option<(Place, RowSelection)>,
    interaction_revision: u64,
    epoch: u64,
    calculation: CalculationState,
    pending: std::collections::VecDeque<RowChange>,
    ends: Vec<End>,
    end_revision: u64,
    observing: bool,
    native_pending: bool,
    reread: bool,
    omitted_end: Option<u64>,
    ready_update: Option<PanelUpdate>,
    retired: Retirement,
}

#[derive(Debug)]
enum CalculationState {
    Idle,
    Pending(Box<Calculation>),
    Running,
    AwaitingSelection(Box<Calculated>),
}

#[derive(Debug)]
enum CountState {
    Needed,
    Running,
    Ready(Counts),
}

/// A name that an operation of this window creates in the shown folder
/// ([`Panels::await_name`]).
#[derive(Clone, Debug)]
struct Awaited {
    name: Name,
    /// The renamed entry was selected, so its new name is too.
    select: bool,
    /// The name of the entry it renames: while a row shows it, the rename
    /// has not happened, as when it waits for the answer to a conflict.
    from: Option<Name>,
    operation: OperationId,
    /// The moment the operation's end was seen: a listing started after it
    /// that lacks the name means the operation did not create it.
    ended: Option<u64>,
    /// Whether the name existed is decided from the command's original rows.
    origin: Option<(Arc<FolderRows>, u64)>,
}

impl Awaited {
    fn items(&self) -> usize {
        self.origin.as_ref().map_or(0, |(rows, _)| rows.items())
    }
}

/// A focused name that a change took away, to which the focus returns when
/// a later change brings it back, as an editor's save through a temporary
/// name does: the focus and the selection stay on the same names.
#[derive(Clone, Debug)]
struct GoneFocus {
    name: Name,
    /// The row the focus was put on; a list focused on another row means
    /// the person moved the focus, which forgets the name.
    row: Option<u64>,
}

impl Panel {
    fn set_awaited(&mut self, awaited: Option<Awaited>) {
        if let Some(old) = std::mem::replace(&mut self.awaited, awaited) {
            let items = old.items();
            self.retired.push(old, items);
        }
    }

    fn new(saved: &SavedPanel) -> Panel {
        Panel {
            folder: String::new(),
            view: View::Reading,
            rows: Arc::new(FolderRows::empty(saved.sort)),
            sort: saved.sort,
            history: saved.history.clone(),
            drive_folders: HashMap::new(),
            read: None,
            sizes_asked: None,
            sized: false,
            free: None,
            quick: Quick::default(),
            selected: Arc::default(),
            counted: CountState::Ready(Counts {
                items: 0,
                selected: None,
                size: None,
            }),
            selected_count: 0,
            saved: saved.folder.clone(),
            gone_focus: None,
            focused: None,
            awaited: None,
            awaited_place: None,
            interaction_revision: 0,
            epoch: 0,
            calculation: CalculationState::Idle,
            pending: Default::default(),
            ends: Vec::new(),
            end_revision: 0,
            observing: false,
            native_pending: false,
            reread: false,
            omitted_end: None,
            ready_update: None,
            retired: Retirement::default(),
        }
    }

    fn set_view(&mut self, view: View) {
        let mut old = std::mem::replace(&mut self.view, view);
        let items = match &mut old {
            View::Search(search) => {
                self.retired.append(search.list.take_retired());
                search.list.items()
            }
            _ => 0,
        };
        self.retired.push(old, items);
    }

    pub(crate) fn folder(&self) -> &str {
        &self.folder
    }

    pub(crate) fn view(&self) -> &View {
        &self.view
    }

    pub(crate) fn reading_generation(&self) -> Option<u64> {
        self.read.as_ref().map(|read| read.generation)
    }

    /// The folder a Go reads, while it reads: the one it opened, or else the
    /// path it asked for.
    fn going_to(&self) -> Option<&str> {
        let read = (self.read.as_ref()).filter(|read| matches!(read.kind, ReadKind::Go { .. }))?;
        Some(match &read.reached {
            Reached::Asked => &read.path,
            Reached::Opened(folder) | Reached::Listed(folder) => folder,
        })
    }

    /// The folder that `F5` and `F6` offer to copy or move to when this is
    /// the other panel: the open folder, also while it is still read; none
    /// for a search, a drive that is not ready or a folder not yet open.
    pub(crate) fn destination(&self) -> Option<&str> {
        if let Some(Read {
            reached: Reached::Opened(folder) | Reached::Listed(folder),
            ..
        }) = &self.read
        {
            return Some(folder);
        }
        match self.view {
            View::Reading | View::Folder if !self.folder.is_empty() => Some(&self.folder),
            _ => None,
        }
    }

    pub(crate) fn sort(&self) -> SortSpec {
        self.sort
    }

    /// The path box's text: the folder a reading goes to, from the moment
    /// it starts, or the shown folder.
    pub(crate) fn path_box(&self) -> &str {
        match &self.read {
            Some(Read {
                kind: ReadKind::Go { .. },
                path,
                ..
            }) => path,
            _ => &self.folder,
        }
    }

    /// The drive button's text: the drive letter, such as `C:`, or `\\` for
    /// a share.
    pub(crate) fn drive(&self) -> String {
        drive_of(self.path_box())
    }

    pub(crate) fn free(&self) -> Option<u64> {
        self.free
    }

    /// The root of the shown folder's drive or share, such as `E:\`.
    pub(crate) fn root(&self) -> String {
        drive_root(&self.folder)
    }

    pub(crate) fn quick_text(&self) -> Option<&str> {
        self.quick.text.as_deref()
    }

    /// The folders the drive menu offers: the last ones the panel showed,
    /// newest first, without the shown one.
    pub(crate) fn recent_folders(&self) -> Vec<&str> {
        let shown = folder_key(&self.folder);
        self.history
            .iter()
            .filter(|folder| folder_key(folder) != shown)
            .map(String::as_str)
            .take(HISTORY)
            .collect()
    }

    /// The panel's search, while it is a search panel.
    pub(crate) fn search(&self) -> Option<&PanelSearch> {
        match &self.view {
            View::Search(search) => Some(search),
            View::Reading | View::Cancelled | View::Folder | View::NotReady { .. } => None,
        }
    }

    fn search_mut(&mut self) -> Option<&mut PanelSearch> {
        match &mut self.view {
            View::Search(search) => Some(search),
            View::Reading | View::Cancelled | View::Folder | View::NotReady { .. } => None,
        }
    }

    /// Whether the list shows `..` first: a folder panel's folder that is
    /// not a drive's or a share's root; a search panel has none.
    fn has_up(&self) -> bool {
        matches!(self.view, View::Folder) && !self.folder.is_empty() && !is_root(&self.folder)
    }

    fn up_rows(&self) -> u64 {
        u64::from(self.has_up())
    }

    fn shown_len(&self) -> usize {
        self.rows.shown_len()
    }

    /// The number of rows the list shows.
    pub(crate) fn count(&self) -> u64 {
        match &self.view {
            View::Folder => self.up_rows() + self.shown_len() as u64,
            View::Search(search) => search.list.results().count(),
            View::Reading | View::Cancelled | View::NotReady { .. } => 0,
        }
    }

    fn row(&self, index: u64) -> Option<Row<'_>> {
        if !matches!(self.view, View::Folder) {
            return None;
        }
        self.rows.row(index, self.up_rows())
    }

    fn entry(&self, index: u64) -> Option<&Entry> {
        match self.row(index)? {
            Row::Entry(entry) => Some(entry),
            Row::Up | Row::Result(_) => None,
        }
    }

    /// The list's focused row as it last told it.
    pub(crate) fn focused(&self) -> Option<u64> {
        self.focused
    }

    /// The full path of `entry` in the shown folder.
    pub(crate) fn path_of(&self, name: &[u16]) -> String {
        join(&self.folder, &String::from_utf16_lossy(name))
    }

    /// The full path of the entry `name` in the shown folder as raw
    /// UTF-16, which a file operation opens, unlike [`Panel::path_of`]'s
    /// text.
    pub(crate) fn wide_path_of(&self, name: &[u16]) -> Vec<u16> {
        let mut path: Vec<u16> = self.folder.encode_utf16().collect();
        if path.last() != Some(&u16::from(b'\\')) {
            path.push(u16::from(b'\\'));
        }
        path.extend_from_slice(name);
        path
    }

    fn row_of(&self, name: &Name, near: Option<(u64, u64)>) -> Option<u64> {
        if !matches!(self.view, View::Folder) {
            return None;
        }
        self.rows.row_of(name, near, self.up_rows())
    }

    /// Whether an entry is selected.
    pub(crate) fn has_selection(&self) -> bool {
        !self.selected.is_empty()
    }

    /// How many rows the list selects, as it last told them.
    pub(crate) fn selected_rows(&self) -> u64 {
        self.selected_count
    }

    /// The folder entry at `row` as an entry a command acts on.
    fn chosen_entry(&self, row: u64) -> Option<ChosenEntry> {
        let entry = self.entry(row)?;
        Some(ChosenEntry {
            path: self.wide_path_of(&entry.name),
            is_folder: entry.is_folder(),
        })
    }

    /// The first row after the chosen entries, which the focus moves to
    /// when a delete takes them; `None` when they are the last rows.
    pub(crate) fn row_after_chosen(&self) -> Option<u64> {
        let last = match self.selected.last() {
            Some(range) => range.end - 1,
            None => self.focused.filter(|row| self.entry(*row).is_some())?,
        };
        (last + 1 < self.count()).then_some(last + 1)
    }

    /// The name of the row that shows `name`, without regard to case, as
    /// New folder checks it (DESIGN.md: New folder). A row that a delete or a
    /// move takes does not show.
    pub(crate) fn shown_name(&self, name: &str) -> Option<String> {
        let name = Name::of(name.encode_utf16().collect());
        if !matches!(self.view, View::Folder) {
            return None;
        }
        let row = self.row_of(&name, None)?;
        self.entry(row).map(Entry::display_name)
    }

    /// What the status line counts: from the kept totals while every entry
    /// shows and none is selected, so it costs nothing in a large folder.
    /// Until gozlerd first answers the sizes asked for the folder, a sum
    /// that lacks some has none, because it would call each folder unknown
    /// for the moment the reply takes.
    pub(crate) fn counts(&self) -> Counts {
        let mut counts = match self.counted {
            CountState::Ready(counts) => counts,
            _ => Counts {
                items: self.shown_len() as u64,
                selected: self.has_selection().then(|| self.selected_rows()),
                size: None,
            },
        };
        let waits = self.sizes_asked.is_some() && !self.sized;
        counts.size = counts.size.filter(|size| size.unknown == 0 || !waits);
        counts
    }

    fn select(&mut self, selected: Vec<Range<u64>>) {
        self.select_shared(selected.into());
    }

    fn select_shared(&mut self, selection: RowSelection) {
        self.selected_count = selection.count;
        let old = std::mem::replace(&mut self.selected, selection.positions);
        let items = old.len();
        self.retired.push(old, items);
        self.interaction_revision += 1;
        let input = Counting::new(self);
        self.counted = match work::pass(input.cost(), input, Counting::run) {
            Pass::Ready(answer) => CountState::Ready(answer.counts),
            Pass::Deferred(_) => CountState::Needed,
        };
        if let Some(old) = self.awaited_place.take() {
            let items = old.0.selected.len();
            self.retired.push(old, items);
        }
        if let Some(update) = self.ready_update.take() {
            let items = update.place.selected.len();
            self.retired.push(update, items);
        }
    }

    /// Empties the list, while it reads, its drive is not ready or it
    /// starts a search.
    fn clear(&mut self) {
        self.native_pending = false;
        self.invalidate();
        let old = std::mem::replace(&mut self.rows, Arc::new(FolderRows::empty(self.sort)));
        let items = old.items();
        self.retired.push(old, items);
        self.select(Vec::new());
        self.quick.close(&mut self.retired);
        self.gone_focus = None;
        let ends = std::mem::take(&mut self.ends);
        self.retired.push(ends, work::INLINE_ITEMS + 1);
        self.omitted_end = None;
        self.reread = false;
        self.sized = false;
    }

    /// The folder `ui.json` saves: the shown one, or the saved one while
    /// the first reading has not opened.
    fn saved_folder(&self) -> Option<&str> {
        match self.folder.is_empty() {
            true => self.saved.as_deref(),
            false => Some(&self.folder),
        }
    }

    /// Records the shown folder in the drive menu's history and as its
    /// drive's last folder.
    fn note_folder(&mut self) {
        let key = folder_key(&self.folder);
        self.history.retain(|folder| folder_key(folder) != key);
        self.history.insert(0, self.folder.clone());
        self.history.truncate(HISTORY + 1);
        self.drive_folders
            .insert(folder_key(&drive_root(&self.folder)), self.folder.clone());
    }

    #[cfg(test)]
    pub(crate) fn keeps(&self, id: OperationId) -> bool {
        self.observing || self.rows.keeps(id)
    }

    /// Whether the rows may still change before a command can act on them:
    /// they are being worked out, or a Go reads, whose listing replaces the
    /// rows the panel still shows. Nothing acts on them then.
    pub(crate) fn rows_pending(&self) -> bool {
        self.going_to().is_some() || self.is_calculating()
    }

    pub(crate) fn is_calculating(&self) -> bool {
        !matches!(self.calculation, CalculationState::Idle)
            || self.observing
            || self.native_pending
            || !self.pending.is_empty()
            || !self.ends.is_empty()
            || self.omitted_end.is_some()
    }

    fn interaction(&self) -> Interaction {
        Interaction {
            revision: self.interaction_revision,
            selected: Arc::clone(&self.selected),
            focused: self.focused,
            gone: self.gone_focus.clone(),
            awaited: self.awaited.clone(),
        }
    }

    fn invalidate(&mut self) {
        if self.observing && self.search().is_none()
            || !matches!(self.calculation, CalculationState::Idle)
            || !self.pending.is_empty()
        {
            self.omitted_end = Some(self.omitted_end.unwrap_or(0).max(self.end_revision));
        }
        self.epoch += 1;
        match std::mem::replace(&mut self.calculation, CalculationState::Idle) {
            CalculationState::Pending(input) => {
                let items = input.items();
                self.retired.push(input, items);
            }
            CalculationState::AwaitingSelection(answer) => {
                let items = answer.items();
                self.retired.push(answer, items);
            }
            CalculationState::Idle | CalculationState::Running => {}
        }
        let pending = std::mem::take(&mut self.pending);
        self.retired.push(pending, work::INLINE_ITEMS + 1);
        if let Some(old) = self.awaited_place.take() {
            let items = old.0.selected.len();
            self.retired.push(old, items);
        }
    }

    fn input(&mut self, change: RowChange, taken: &Arc<Taken>) -> Calculation {
        let folder = match &change {
            RowChange::Listed { folder, .. } => folder.clone(),
            _ => self.folder.clone(),
        };
        Calculation {
            epoch: self.epoch,
            folder,
            before: Arc::clone(&self.rows),
            before_up: self.up_rows(),
            interaction: self.interaction(),
            taken: Arc::clone(taken),
            ends: std::mem::take(&mut self.ends),
            end_revision: self.end_revision,
            input: Input::Change(change),
        }
    }

    fn submit(&mut self, change: RowChange, taken: &Arc<Taken>) -> Option<PanelUpdate> {
        if self.omitted_end.is_some() && !matches!(change, RowChange::Listed { .. }) {
            let items = change.items();
            self.retired.push(change, items);
            return None;
        }
        if self.native_pending
            || self.observing
            || !matches!(self.calculation, CalculationState::Idle)
            || !self.pending.is_empty()
        {
            if self.pending.len() == PENDING_ROWS {
                self.invalidate();
                self.omitted_end = Some(self.end_revision);
                self.reread = true;
                let items = change.items();
                self.retired.push(change, items);
                return None;
            }
            self.pending.push_back(change);
            return None;
        }
        let input = self.input(change, taken);
        match work::pass(input.items(), input, Calculation::run) {
            Pass::Ready(answer) => self.accept_calculated(answer, taken),
            Pass::Deferred(input) => {
                self.calculation = CalculationState::Pending(Box::new(input));
                None
            }
        }
    }

    fn accept_calculated(
        &mut self,
        mut answer: Calculated,
        taken: &Arc<Taken>,
    ) -> Option<PanelUpdate> {
        if answer.epoch != self.epoch || !Arc::ptr_eq(&answer.before, &self.rows) {
            let items = answer.items();
            self.retired.push(answer, items);
            return None;
        }
        if self.native_pending {
            self.calculation = CalculationState::AwaitingSelection(Box::new(answer));
            return None;
        }
        if self
            .omitted_end
            .is_some_and(|moment| answer.started().is_none_or(|started| started <= moment))
        {
            self.invalidate();
            let items = answer.items();
            self.retired.push(answer, items);
            self.reread = true;
            return None;
        }
        self.calculation = CalculationState::Idle;
        if self.observing
            || answer.interaction.revision != self.interaction_revision
            || answer.end_revision != self.end_revision
            || !Arc::ptr_eq(&answer.taken, taken)
        {
            let input = Calculation {
                epoch: self.epoch,
                folder: answer.folder.clone(),
                before: Arc::clone(&self.rows),
                before_up: self.up_rows(),
                interaction: self.interaction(),
                taken: Arc::clone(taken),
                ends: std::mem::take(&mut self.ends),
                end_revision: self.end_revision,
                input: Input::Remap(Box::new(answer)),
            };
            if self.observing {
                self.calculation = CalculationState::Pending(Box::new(input));
                return None;
            }
            match work::pass(input.items(), input, Calculation::run) {
                Pass::Ready(answer) => return self.accept_calculated(answer, taken),
                Pass::Deferred(input) => {
                    self.calculation = CalculationState::Pending(Box::new(input))
                }
            }
            return None;
        }
        let rows = answer
            .rows
            .take()
            .expect("a current answer owns its candidate");
        let before = Place {
            focused: self.focused,
            selected: Arc::clone(&self.selected),
        };
        let old = std::mem::replace(&mut self.rows, Arc::new(rows));
        let items = old.items();
        self.retired.push(old, items);
        let update = answer.update(before);
        if matches!(update.kind, UpdatedKind::Listed { .. }) {
            self.folder.clone_from(&answer.folder);
            self.set_view(View::Folder);
            self.read = None;
            self.saved = None;
            self.omitted_end = None;
            self.note_folder();
        }
        self.select_shared(answer.selected.clone());
        self.focused = update.place.focused;
        self.gone_focus = answer.interaction.gone.take();
        self.set_awaited(answer.interaction.awaited.take());
        self.awaited_place = answer.awaited_place.take();
        let items = answer.items();
        self.retired.push(answer, items);
        Some(update)
    }
}

/// The sort a click on the header of `key` asks for: the other direction of
/// the shown key, else the key's first direction, which puts the largest or
/// the newest first.
pub(crate) fn header_sort(shown: SortSpec, key: SortKey) -> SortSpec {
    if shown.key == key {
        SortSpec {
            key,
            dir: match shown.dir {
                SortDir::Asc => SortDir::Desc,
                SortDir::Desc => SortDir::Asc,
            },
        }
    } else {
        SortSpec::default_for(key)
    }
}

/// A folder's path in its folded form, without a trailing `\`, so two
/// spellings of one folder compare equal.
fn folder_key(folder: &str) -> Vec<u8> {
    fold(folder.trim_end_matches('\\').as_bytes())
}

/// Whether `a` and `b` name the same folder, as Windows compares names.
pub(crate) fn same_folder(a: &str, b: &str) -> bool {
    folder_key(a) == folder_key(b)
}

/// Where a panel shows a search result: the folder that holds it, with the
/// result's name to focus, or a folder result's own folder (DESIGN.md:
/// Search in a panel).
pub(crate) fn result_place(item: &ResultItem) -> (String, Option<Box<[u16]>>) {
    if item.is_dir {
        (item.path.clone(), None)
    } else {
        (parent_of(&item.path), Some(result_name(item)))
    }
}

/// A search result's name, after the last `\` of its full path.
fn result_name(item: &ResultItem) -> Box<[u16]> {
    let wide = wide_path(item);
    let start = (wide.iter().rposition(|&unit| unit == u16::from(b'\\')))
        .map_or(0, |separator| separator + 1);
    wide[start..].into()
}

/// Whether `path` is `folder` or below it.
fn is_within(path: &str, folder: &str) -> bool {
    let path = folder_key(path);
    let folder = folder_key(folder);
    path.starts_with(&folder) && (path.len() == folder.len() || path[folder.len()] == b'\\')
}

/// The folder of `path`, a result of a search scoped to `searched`, below
/// that folder: `Projects\Accounts` for `C:\Users\me\Projects\Accounts\a.txt`
/// searched in `C:\Users\me`, and empty for a result directly in it. It
/// skips as many components as `searched` has without comparing them,
/// because gozlerd answers a scoped search only with paths below the scope;
/// a parent with fewer components, which a correct reply never gives, comes
/// back whole.
pub(crate) fn relative_folder<'p>(searched: &str, path: &'p str) -> &'p str {
    let Some(end) = path.rfind('\\') else {
        return "";
    };
    let parent = &path[..end];
    let components = |path: &str| path.split('\\').filter(|c| !c.is_empty()).count();
    let skip = components(searched);
    if components(parent) < skip {
        return parent;
    }
    let mut rest = parent;
    for _ in 0..skip {
        let trimmed = rest.trim_start_matches('\\');
        rest = trimmed.find('\\').map_or("", |at| &trimmed[at..]);
    }
    rest.trim_start_matches('\\')
}

pub(crate) fn drive_root(folder: &str) -> String {
    let mut root = folder.to_owned();
    loop {
        let parent = parent_of(&root);
        if folder_key(&parent) == folder_key(&root) {
            return parent;
        }
        root = parent;
    }
}

/// The drive button's text for `path`.
fn drive_of(path: &str) -> String {
    match path.as_bytes() {
        [letter, b':', ..] if letter.is_ascii_alphabetic() => {
            format!("{}:", char::from(letter.to_ascii_uppercase()))
        }
        [b'\\', b'\\', ..] => r"\\".to_owned(),
        _ => String::new(),
    }
}

pub(crate) fn join(folder: &str, name: &str) -> String {
    if folder.ends_with('\\') {
        format!("{folder}{name}")
    } else {
        format!(r"{folder}\{name}")
    }
}

/// The name at the end of `path`.
pub(crate) fn last_name(path: &str) -> &str {
    path.trim_end_matches('\\')
        .rsplit_once('\\')
        .map_or(path, |(_, name)| name)
}

/// What the person typed in the path box, as a full path: environment
/// variables expanded ([`expand`]), and a relative path joined to `folder`
/// (DESIGN.md: A panel). `None` for an empty text.
pub(crate) fn typed_path(
    typed: &str,
    folder: &str,
    var: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    Some(absolute(&expand(typed, var), folder))
}

/// `typed` with each `%NAME%` that `var` knows replaced by its value, as
/// File Explorer expands environment variables.
pub(crate) fn expand(typed: &str, var: impl Fn(&str) -> Option<String>) -> String {
    let mut expanded = String::new();
    let mut rest = typed;
    while let Some(open) = rest.find('%') {
        let after = &rest[open + 1..];
        match after
            .find('%')
            .and_then(|close| var(&after[..close]).map(|value| (close, value)))
        {
            Some((close, value)) => {
                expanded.push_str(&rest[..open]);
                expanded.push_str(&value);
                rest = &after[close + 1..];
            }
            None => {
                expanded.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    expanded.push_str(rest);
    expanded
}

/// Whether `request`'s end makes a panel that shows `folder` read it again:
/// its destination is the folder or a path in it, such as a new name or a
/// created folder, also one created below a folder it created, or a source
/// is in the folder, is it or holds it (DESIGN.md: The person never waits for
/// the disk).
pub(crate) fn ends_in(request: &Request, folder: &str) -> bool {
    let text = |path: &Path| path.to_string_lossy().into_owned();
    let lands = |path: &Path| {
        let path = text(path);
        folder_key(&parent_of(&path)) == folder_key(folder) || is_within(&path, folder)
    };
    let takes = |path: &Path| {
        let path = text(path);
        folder_key(&parent_of(&path)) == folder_key(folder) || is_within(folder, &path)
    };
    match request {
        Request::Copy {
            sources,
            destination,
            ..
        }
        | Request::Move {
            sources,
            destination,
            ..
        } => {
            lands(&destination.path())
                || matches!(request, Request::Move { .. }) && sources.iter().any(|s| takes(s))
        }
        Request::Delete { sources } => sources.iter().any(|s| takes(s)),
        Request::NewFolder { path } => lands(path),
        Request::Synchronize { destination, .. } => lands(destination),
    }
}

/// Whether `request`'s end changes a folder size that a panel showing
/// `folder` shows: a source or the destination is in or below it
/// (DESIGN.md: Folder sizes).
pub(crate) fn changes_sizes(request: &Request, folder: &str) -> bool {
    let below = |path: &Path| is_within(&path.to_string_lossy(), folder);
    match request {
        Request::Copy {
            sources,
            destination,
            ..
        }
        | Request::Move {
            sources,
            destination,
            ..
        } => below(&destination.path()) || sources.iter().any(|s| below(s)),
        Request::Delete { sources } => sources.iter().any(|s| below(s)),
        Request::NewFolder { path } => below(path),
        Request::Synchronize { destination, .. } => below(destination),
    }
}

/// Both panels and what they share.
#[derive(Debug)]
pub(crate) struct Panels {
    panels: [Panel; 2],
    active: Side,
    /// Orders the ends of operations and the starts of readings, and
    /// names each reading.
    moment: u64,
    /// The entries that deletes and moves have taken, which the folder
    /// panels leave out, the search lists show struck through, and the
    /// selections leave out (DESIGN.md: The person never waits for the disk);
    /// one for all lists, Search's too, which each decide with their own kept
    /// operations.
    taken: Arc<Taken>,
    /// The drives' labels as the labels thread last read them, by letter.
    labels: HashMap<char, String>,
}

pub(crate) struct EndReceipt {
    moment: u64,
    awaited: [Option<OperationId>; 2],
}

pub(crate) struct PanelKeeps([Arc<FolderRows>; 2]);

impl PanelKeeps {
    pub(crate) fn contains(&self, id: OperationId) -> bool {
        self.0.iter().any(|rows| rows.keeps(id))
    }

    pub(crate) fn items(&self) -> usize {
        self.0.iter().map(|rows| rows.items()).sum()
    }

    pub(crate) fn matches(&self, panels: &Panels) -> bool {
        Side::BOTH
            .into_iter()
            .all(|side| Arc::ptr_eq(&self.0[side.index()], &panels.panel(side).rows))
    }
}

impl Panels {
    pub(crate) fn new(saved: &SavedPanels) -> Panels {
        Panels {
            panels: [Panel::new(&saved.left), Panel::new(&saved.right)],
            active: saved.active,
            moment: 0,
            taken: Arc::default(),
            labels: HashMap::new(),
        }
    }

    pub(crate) fn panel(&self, side: Side) -> &Panel {
        &self.panels[side.index()]
    }

    pub(crate) fn take_retired(&mut self) -> Retirement {
        let mut retired = Retirement::default();
        for panel in &mut self.panels {
            retired.append(std::mem::take(&mut panel.retired));
            if let Some(search) = panel.search_mut() {
                retired.append(search.list.take_retired());
            }
        }
        retired
    }

    pub(crate) fn take_calculation(&mut self, side: Side) -> Option<Calculation> {
        let taken = Arc::clone(&self.taken);
        let panel = self.panel_mut(side);
        if panel.observing || panel.native_pending {
            return None;
        }
        if matches!(panel.calculation, CalculationState::Idle)
            && let Some(change) = panel.pending.pop_front()
        {
            panel.calculation = CalculationState::Pending(Box::new(panel.input(change, &taken)));
        }
        match std::mem::replace(&mut panel.calculation, CalculationState::Running) {
            CalculationState::Pending(input) => Some(*input),
            CalculationState::AwaitingSelection(answer) => Some(Calculation {
                epoch: panel.epoch,
                folder: answer.folder.clone(),
                before: Arc::clone(&panel.rows),
                before_up: panel.up_rows(),
                interaction: panel.interaction(),
                taken,
                ends: std::mem::take(&mut panel.ends),
                end_revision: panel.end_revision,
                input: Input::Remap(answer),
            }),
            other => {
                panel.calculation = other;
                None
            }
        }
    }

    pub(crate) fn accept_calculation(
        &mut self,
        side: Side,
        answer: Calculated,
    ) -> Option<PanelUpdate> {
        let taken = Arc::clone(&self.taken);
        self.panel_mut(side).accept_calculated(answer, &taken)
    }

    pub(crate) fn take_count(&mut self, side: Side) -> Option<Counting> {
        let panel = self.panel_mut(side);
        if matches!(panel.view, View::Folder)
            && !panel.rows_pending()
            && matches!(panel.counted, CountState::Needed)
        {
            panel.counted = CountState::Running;
            Some(Counting::new(panel))
        } else {
            None
        }
    }

    pub(crate) fn accept_count(&mut self, side: Side, answer: Counted) -> bool {
        answer.accept(self.panel_mut(side))
    }

    pub(crate) fn take_reread(&mut self, side: Side) -> bool {
        std::mem::take(&mut self.panel_mut(side).reread)
    }

    pub(crate) fn take_update(&mut self, side: Side) -> Option<PanelUpdate> {
        self.panel_mut(side).ready_update.take()
    }

    fn panel_mut(&mut self, side: Side) -> &mut Panel {
        &mut self.panels[side.index()]
    }

    pub(crate) fn active(&self) -> Side {
        self.active
    }

    /// Makes `side` the active panel; whether it was not.
    pub(crate) fn activate(&mut self, side: Side) -> bool {
        std::mem::replace(&mut self.active, side) != side
    }

    /// What `ui.json` saves.
    pub(crate) fn saved(&self) -> SavedPanels {
        let saved = |panel: &Panel| SavedPanel {
            folder: panel.saved_folder().map(str::to_owned),
            sort: panel.sort,
            history: panel.history.clone(),
        };
        SavedPanels {
            left: saved(&self.panels[0]),
            right: saved(&self.panels[1]),
            active: self.active,
        }
    }

    /// Whether [`Panels::saved`] would give `saved`, compared in place.
    pub(crate) fn saved_as(&self, saved: &SavedPanels) -> bool {
        let same = |panel: &Panel, saved: &SavedPanel| {
            panel.saved_folder() == saved.folder.as_deref()
                && panel.sort == saved.sort
                && panel.history == saved.history
        };
        self.active == saved.active
            && same(&self.panels[0], &saved.left)
            && same(&self.panels[1], &saved.right)
    }

    fn start(&mut self, side: Side, path: String, kind: ReadKind, nearest: bool) -> ReadAsk {
        self.moment += 1;
        let generation = self.moment;
        let panel = self.panel_mut(side);
        // Another folder closes the quick search box and forgets the
        // awaited name; the shown one read again keeps them, so a change
        // does not end the person's typing.
        if matches!(kind, ReadKind::Go { .. }) {
            panel.quick.close(&mut panel.retired);
            panel.set_awaited(None);
        }
        let sort = panel.sort;
        let sizes = if folder_key(&path) == folder_key(&panel.folder) {
            ReadSizes(Some(Arc::clone(&panel.rows)))
        } else {
            ReadSizes(None)
        };
        panel.invalidate();
        panel.read = Some(Read {
            generation,
            kind,
            path: path.clone(),
            nearest,
            sort,
            reached: Reached::Asked,
        });
        ReadAsk {
            generation,
            path,
            nearest,
            sort,
            sizes,
        }
    }

    /// Forgets the readings that run, whose threads the window stopped, as
    /// when Search shows.
    pub(crate) fn stop_reading(&mut self) {
        for panel in &mut self.panels {
            panel.read = None;
        }
    }

    pub(crate) fn cancel_read(&mut self, side: Side, generation: u64) -> bool {
        let panel = self.panel_mut(side);
        if panel.reading_generation() != Some(generation) {
            return false;
        }
        let read = panel.read.take().expect("the matching reading is present");
        panel.invalidate();
        let retained = matches!(panel.view, View::Folder);
        // Navigation ended the accepted folder's watch; a fresh listing restores
        // it and clears omitted end evidence. A dirty same-folder read needs Retry.
        panel.reread = retained
            && matches!(read.kind, ReadKind::Go { .. })
            && !same_folder(&read.path, &panel.folder);
        if matches!(panel.view, View::Reading)
            || retained && panel.omitted_end.is_some() && !panel.reread
        {
            if panel.folder.is_empty() {
                panel.folder = read.path;
            }
            panel.set_view(View::Cancelled);
            panel.select(Vec::new());
            panel.focused = None;
            panel.quick.close(&mut panel.retired);
        }
        true
    }

    /// The reading of `side` when Commander mode shows: its folder again, or
    /// the first time its saved folder, or the nearest folder above it that
    /// exists; `first` is the folder of a panel that saved none, Documents
    /// or Downloads. `None` for a search panel, whose search runs again
    /// instead.
    pub(crate) fn start_reading(
        &mut self,
        side: Side,
        first: impl FnOnce() -> String,
    ) -> Option<ReadAsk> {
        if self.panel(side).search().is_some() {
            return None;
        }
        if let Some(ask) = self.read_again(side) {
            return Some(ask);
        }
        let path = self.panel(side).saved.clone().unwrap_or_else(first);
        let kind = ReadKind::Go {
            focus: None,
            typed: false,
        };
        Some(self.start(side, path, kind, true))
    }

    /// Reads the shown folder again, keeping its rows until the listing
    /// applies by name; a shown folder that was deleted gives way to the
    /// nearest folder above it. Nothing while the panel goes to another
    /// folder, whose listing is newer than any reading of this one, and
    /// nothing for a search panel.
    pub(crate) fn read_again(&mut self, side: Side) -> Option<ReadAsk> {
        let panel = self.panel(side);
        let folder = panel.folder.clone();
        if folder.is_empty() || panel.going_to().is_some() {
            return None;
        }
        let kind = match self.panel(side).view {
            View::Folder => ReadKind::Again,
            View::Reading | View::Cancelled | View::NotReady { .. } => ReadKind::Go {
                focus: None,
                typed: false,
            },
            View::Search(_) => return None,
        };
        Some(self.start(side, folder, kind, true))
    }

    /// Starts again the reading of `side` that runs, with a new generation
    /// and the panel's sort, as when a drive's removal stopped it or the
    /// person sorted again; with none, reads the shown folder again.
    pub(crate) fn restart_reading(&mut self, side: Side) -> Option<ReadAsk> {
        match self.panel(side).read.clone() {
            Some(read) => Some(self.start(side, read.path, read.kind, read.nearest)),
            None => self.read_again(side),
        }
    }

    /// Sorts `side` by `sort`: the reading thread lists the folder again in
    /// that order, as it lists every folder, so a large folder is never
    /// sorted on the window's thread. The rows keep their order until the
    /// listing comes. `None` while the panel shows no folder, also for a
    /// search panel, whose search the window runs again in the new sort.
    pub(crate) fn sort(&mut self, side: Side, sort: SortSpec) -> Option<ReadAsk> {
        let panel = self.panel_mut(side);
        panel.sort = sort;
        if panel.read.is_none() && !matches!(panel.view, View::Folder) {
            return None;
        }
        self.restart_reading(side)
    }

    /// Shows `path` in `side`, focusing the entry `focus` when the listing
    /// has it.
    pub(crate) fn go(&mut self, side: Side, path: String, focus: Option<Box<[u16]>>) -> ReadAsk {
        let kind = ReadKind::Go {
            focus,
            typed: false,
        };
        self.start(side, path, kind, false)
    }

    /// The path the person typed in `side`'s path box; `None` for an empty
    /// text.
    pub(crate) fn go_typed(
        &mut self,
        side: Side,
        typed: &str,
        var: impl Fn(&str) -> Option<String>,
    ) -> Option<ReadAsk> {
        let path = typed_path(typed, &self.panel(side).folder, var)?;
        let kind = ReadKind::Go {
            focus: None,
            typed: true,
        };
        Some(self.start(side, path, kind, false))
    }

    /// The parent folder of `side`, with the folder it came from focused.
    pub(crate) fn go_up(&mut self, side: Side) -> Option<ReadAsk> {
        // While a Go reads, the panel still shows the folder it leaves, and
        // `Backspace` goes up from the one the person goes to.
        let panel = self.panel(side);
        let folder = panel.going_to().unwrap_or(&panel.folder).to_owned();
        if folder.is_empty() || is_root(&folder) {
            return None;
        }
        let name: Box<[u16]> = last_name(&folder).encode_utf16().collect();
        Some(self.go(side, parent_of(&folder), Some(name)))
    }

    /// `Enter` or a double-click on `row` of `side`, as in Total Commander:
    /// `..` shows the parent folder with the folder the panel came from
    /// focused, and a folder, also a search panel's, shows that folder.
    /// Settings > Context menu's open commands do not apply (DESIGN.md:
    /// Searching, "where Open goes").
    pub(crate) fn enter(&mut self, side: Side, row: u64) -> Entered {
        enum Target {
            Up,
            Folder(String),
            File(ChosenEntry),
        }
        let panel = self.panel(side);
        // The rows still shown while a Go reads belong to the folder it
        // leaves.
        if panel.going_to().is_some() {
            return Entered::Nothing;
        }
        let target = match self.row(side, row) {
            None => return Entered::Nothing,
            Some(Row::Up) => Target::Up,
            Some(Row::Entry(entry)) if entry.is_folder() => {
                Target::Folder(panel.path_of(&entry.name))
            }
            Some(Row::Entry(entry)) => Target::File(ChosenEntry {
                path: panel.wide_path_of(&entry.name),
                is_folder: false,
            }),
            Some(Row::Result(item)) if item.is_dir => Target::Folder(item.path.clone()),
            Some(Row::Result(item)) => Target::File(ChosenEntry::of_result(item)),
        };
        match target {
            Target::Up => self.go_up(side).map_or(Entered::Nothing, Entered::Read),
            Target::Folder(path) => Entered::Read(self.go(side, path, None)),
            Target::File(entry) => Entered::Open(entry),
        }
    }

    /// `Ctrl+Left` or `Ctrl+Right` on `row` of `side`, as in Total
    /// Commander: the panel `to` shows the row's folder, the parent folder
    /// for `..`, with the folder it came from focused, or for a file the
    /// folder that holds it, with the file focused; with no row, `side`'s
    /// folder. `None` when `to` already
    /// shows that folder, so a file in the same panel keeps its selection.
    pub(crate) fn show_in(&mut self, side: Side, row: Option<u64>, to: Side) -> Option<ReadAsk> {
        let panel = self.panel(side);
        let (path, focus) = match row.and_then(|row| self.row(side, row)) {
            None => (panel.folder.clone(), None),
            Some(Row::Up) => (
                parent_of(&panel.folder),
                Some(last_name(&panel.folder).encode_utf16().collect()),
            ),
            Some(Row::Entry(entry)) if entry.is_folder() => (panel.path_of(&entry.name), None),
            Some(Row::Entry(entry)) => (panel.folder.clone(), Some(entry.name.clone())),
            Some(Row::Result(item)) => result_place(item),
        };
        let target = self.panel(to);
        if path.is_empty() || target.search().is_none() && same_folder(&path, &target.folder) {
            return None;
        }
        Some(self.go(to, path, focus))
    }

    /// The row at `index` of `side`'s list: a search panel's fetched row
    /// that no operation has taken, or a folder panel's row. `None` for a
    /// blank row.
    pub(crate) fn row(&self, side: Side, index: u64) -> Option<Row<'_>> {
        let panel = self.panel(side);
        match panel.search() {
            Some(search) => (search.list).shown_row(index, &self.taken).map(Row::Result),
            None => panel.row(index),
        }
    }

    pub(crate) fn search(&self, side: Side) -> Option<&PanelSearch> {
        self.panel(side).search()
    }

    pub(crate) fn search_mut(&mut self, side: Side) -> Option<&mut PanelSearch> {
        self.panel_mut(side).search_mut()
    }

    /// Makes `side` a search panel of its folder, with no text yet
    /// (DESIGN.md: Search in a panel). The folder's rows, its kept operations
    /// and its reading go; the focused name stays for the way back. Returns
    /// whether it was not a search panel.
    pub(crate) fn enter_search(&mut self, side: Side) -> bool {
        let panel = self.panel_mut(side);
        if panel.search().is_some() {
            return false;
        }
        let back = (panel.focused)
            .and_then(|row| panel.entry(row))
            .map(|entry| entry.name.clone());
        if panel.folder.is_empty() {
            // Before the first listing: the folder being read.
            if let Some(path) =
                (panel.read.as_ref().map(|read| read.path.clone())).or_else(|| panel.saved.clone())
            {
                panel.folder = path;
            }
        }
        panel.clear();
        panel.read = None;
        panel.sizes_asked = None;
        panel.set_awaited(None);
        panel.focused = None;
        panel.set_view(View::Search(Box::new(PanelSearch {
            list: SearchList::new(),
            look_in: LookIn::ThisFolder,
            back,
        })));
        true
    }

    /// Leaves `side`'s search: the searched folder opens again, with the
    /// row focused that was focused before the search, or the nearest
    /// folder above it that exists. The results show until it opens.
    /// `None` when the panel does not search.
    pub(crate) fn leave_search(&mut self, side: Side) -> Option<ReadAsk> {
        let panel = self.panel(side);
        let search = panel.search()?;
        let kind = ReadKind::Go {
            focus: search.back.clone(),
            typed: false,
        };
        let folder = panel.folder.clone();
        Some(self.start(side, folder, kind, true))
    }

    /// `Esc` in `side`'s list, after the quick search box was closed: it
    /// clears a selection first, and only with nothing selected leaves the
    /// search (DESIGN.md: Search in a panel).
    pub(crate) fn leave_on_escape(&mut self, side: Side, selected: bool) -> Option<ReadAsk> {
        if selected {
            return None;
        }
        self.leave_search(side)
    }

    /// The drive menu's choice of `root`, such as `D:\`: the folder the
    /// panel last showed on that drive in this session, or its root; the
    /// drive the panel already shows goes to its root.
    pub(crate) fn go_drive(&mut self, side: Side, root: &str) -> ReadAsk {
        let panel = self.panel(side);
        let key = folder_key(root);
        let path = if folder_key(&drive_root(&panel.folder)) == key {
            root.to_owned()
        } else {
            panel
                .drive_folders
                .get(&key)
                .cloned()
                .unwrap_or_else(|| root.to_owned())
        };
        self.go(side, path, None)
    }

    /// Applies the reading thread's `event` for `side`, whose list has the
    /// focus on `focused`.
    pub(crate) fn accept_read(
        &mut self,
        side: Side,
        event: ReadEvent,
        focused: Option<u64>,
    ) -> ReadAccepted {
        self.accepted(side, |panels| panels.apply_read(side, event, focused))
    }

    /// The listing of the folder that `side`'s Go opened has not come soon
    /// after it opened: the panel shows that folder, reading, without rows
    /// (DESIGN.md: A panel).
    pub(crate) fn show_reading(&mut self, side: Side) -> ReadAccepted {
        self.accepted(side, |panels| {
            let panel = panels.panel_mut(side);
            let Some(Read {
                reached: Reached::Opened(folder),
                ..
            }) = panel.read.clone()
            else {
                return ReadChange::Stale;
            };
            panel.folder = folder;
            panel.set_view(View::Reading);
            panel.clear();
            panel.free = None;
            ReadChange::Cleared
        })
    }

    /// The change `apply` makes to `side`, and whether it ended the panel's
    /// search.
    fn accepted(
        &mut self,
        side: Side,
        apply: impl FnOnce(&mut Self) -> ReadChange,
    ) -> ReadAccepted {
        let searched = self.search(side).is_some();
        let change = apply(self);
        ReadAccepted {
            change,
            search_ended: searched && self.search(side).is_none(),
        }
    }

    fn apply_read(&mut self, side: Side, event: ReadEvent, focused: Option<u64>) -> ReadChange {
        let generation = match &event {
            ReadEvent::Opened { generation, .. }
            | ReadEvent::Listed { generation, .. }
            | ReadEvent::Failed { generation, .. } => *generation,
        };
        let taken = Arc::clone(&self.taken);
        let panel = self.panel_mut(side);
        let Some(read) = panel.read.clone().filter(|r| r.generation == generation) else {
            let items = match &event {
                ReadEvent::Listed { entries, .. } => entries.len(),
                _ => 0,
            };
            panel.retired.push(event, items);
            return ReadChange::Stale;
        };
        match event {
            ReadEvent::Opened { folder, .. } => match read.kind {
                ReadKind::Go { .. } => {
                    if let Some(read) = &mut panel.read {
                        read.reached = Reached::Opened(folder);
                    }
                    ReadChange::Opened
                }
                ReadKind::Again => ReadChange::Stale,
            },
            ReadEvent::Listed {
                folder,
                entries,
                focus,
                ..
            } => {
                if panel
                    .omitted_end
                    .is_some_and(|moment| read.generation <= moment)
                {
                    let items = entries.len();
                    panel.retired.push(entries, items);
                    panel.reread = true;
                    return ReadChange::Pending;
                }
                if let Some(read) = &mut panel.read
                    && matches!(read.kind, ReadKind::Go { .. })
                {
                    read.reached = Reached::Listed(folder.clone());
                }
                let same = matches!((&read.kind, &panel.view), (ReadKind::Again, View::Folder))
                    && folder_key(&folder) == folder_key(&panel.folder);
                let focus = match &read.kind {
                    ReadKind::Go { focus: wanted, .. } => focus.or_else(|| wanted.clone()),
                    ReadKind::Again => focus,
                };
                panel.focused = focused;
                let change = RowChange::Listed {
                    folder,
                    entries,
                    read,
                    same,
                    focus,
                };
                match panel.submit(change, &taken) {
                    Some(update) => ReadChange::Listed {
                        place: update.place,
                        again: matches!(update.kind, UpdatedKind::Listed { again: true }),
                        stale: update.read_again,
                    },
                    None => ReadChange::Pending,
                }
            }
            ReadEvent::Failed {
                path,
                failure,
                reason,
                ..
            } => {
                panel.read = None;
                // A folder that cannot be opened is no state of the panel
                // (DESIGN.md: A panel): it stays where it was, with its rows,
                // its search, or another folder that reads or is not ready,
                // which the window reads again. Only the shown folder itself,
                // or the first one, is not ready.
                let elsewhere = matches!(panel.view, View::Folder | View::Search(_))
                    || !panel.folder.is_empty() && read.path != panel.folder;
                match read.kind {
                    ReadKind::Go { typed, .. } if elsewhere => ReadChange::Refused {
                        path,
                        failure,
                        reason,
                        typed,
                    },
                    _ => {
                        panel.set_view(View::NotReady {
                            drive: drive_root(&read.path),
                        });
                        if panel.folder.is_empty() {
                            panel.folder = read.path;
                        }
                        panel.clear();
                        ReadChange::Cleared
                    }
                }
            }
        }
    }

    /// The shown drive of `side` is gone, as Windows said when it removed
    /// it. A panel that was going to another folder shows that folder's
    /// drive, whose watch Windows named, and Retry reads that folder.
    /// Returns whether the panel searched until now, which ends its search,
    /// as [`ReadAccepted::search_ended`] says.
    pub(crate) fn drive_removed(&mut self, side: Side) -> bool {
        let panel = self.panel_mut(side);
        let searched = panel.search().is_some();
        if let Some(Read {
            kind: ReadKind::Go { .. },
            path,
            ..
        }) = panel.read.take()
        {
            panel.folder = path;
        }
        panel.set_view(View::NotReady {
            drive: drive_root(&panel.folder),
        });
        panel.clear();
        searched
    }

    /// Select files… and Unselect files…: `side`'s place with every file
    /// whose name `mask` matches selected, or unselected when not `select`;
    /// folders keep their state, as in Total Commander. `None` when the
    /// panel lists no folder.
    pub(crate) fn select_matching(
        &mut self,
        side: Side,
        mask: &NameMask,
        select: bool,
    ) -> Option<Place> {
        let panel = self.panel(side);
        if !matches!(panel.view, View::Folder) {
            return None;
        }
        let mut scratch = MaskScratch::default();
        let mut name = Vec::new();
        let mut was = panel.selected.iter().peekable();
        let mut selected = Vec::new();
        for row in 0..panel.count() {
            while was.next_if(|rows| rows.end <= row).is_some() {}
            let mut is = was.peek().is_some_and(|rows| rows.contains(&row));
            if let Some(entry) = panel.entry(row)
                && !entry.is_folder()
            {
                name.clear();
                wtf8::from_utf16(&entry.name, &mut name);
                if mask.matches(&name, &mut scratch) {
                    is = select;
                }
            }
            if is {
                push_rows(&mut selected, row..row + 1);
            }
        }
        self.set_selected(side, RowSelection::from(selected));
        Some(self.place(side))
    }

    /// The list's selected rows, as it tells them; whether they changed.
    pub(crate) fn set_selected(&mut self, side: Side, selected: RowSelection) -> bool {
        let panel = self.panel_mut(side);
        let changed = panel.selected_count != selected.count
            || selected.positions.len() > work::INLINE_ITEMS
            || panel.selected.as_ref() != selected.positions.as_ref();
        if changed {
            panel.select_shared(selected);
            panel.quick.invalidate();
        }
        changed
    }

    pub(crate) fn native_selection_pending(&mut self, side: Side, pending: bool) {
        self.panel_mut(side).native_pending = pending;
    }

    pub(crate) fn selected_count(&self, side: Side) -> u64 {
        self.panel(side).selected_count
    }

    pub(crate) fn place(&self, side: Side) -> Place {
        let panel = self.panel(side);
        Place {
            focused: panel.focused,
            selected: Arc::clone(&panel.selected),
        }
    }

    /// The list's focused row, as it tells it.
    pub(crate) fn set_focused(&mut self, side: Side, focused: Option<u64>) {
        let panel = self.panel_mut(side);
        if panel.focused != focused {
            panel.focused = focused;
            panel.interaction_revision += 1;
            panel.quick.invalidate();
            if let Some(old) = panel.awaited_place.take() {
                let items = old.0.selected.len();
                panel.retired.push(old, items);
            }
        }
    }

    /// How many entries `side`'s file commands act on
    /// ([`Panels::chosen_source`]), without a pass over them: none while a search
    /// panel's selection is read.
    pub(crate) fn chosen_count(&self, side: Side) -> usize {
        let panel = self.panel(side);
        if panel.rows_pending() {
            return 0;
        }
        match (panel.search(), panel.selected.as_slice()) {
            (None, []) => usize::from(self.focused(side).is_some()),
            (None, _) => panel.selected_count as usize,
            (Some(search), _) => search.list.selection().files().map_or(0, <[_]>::len),
        }
    }

    /// The entries a file command of `side` acts on: the selected ones,
    /// else the focused one; never `..` (DESIGN.md: Selecting entries). A
    /// search panel's selected results once they are read, and none while they
    /// are.
    pub(crate) fn chosen_source(&self, side: Side) -> Option<super::open::Entries> {
        let panel = self.panel(side);
        if panel.rows_pending() {
            return None;
        }
        match panel.search() {
            None => Some(super::open::Entries::Panel(PanelEntries {
                rows: Arc::clone(&panel.rows),
                selected: Arc::clone(&panel.selected),
                focused: panel.focused,
                folder: panel.folder.clone(),
                up: panel.up_rows(),
            })),
            Some(search) => {
                (search.list.selection().shared_files()).map(super::open::Entries::Results)
            }
        }
    }

    pub(crate) fn command_place(&self, side: Side) -> CommandPlace {
        let panel = self.panel(side);
        CommandPlace {
            side,
            epoch: panel.epoch,
            interaction: panel.interaction_revision,
        }
    }

    pub(crate) fn place_is_current(&self, place: CommandPlace) -> bool {
        let panel = self.panel(place.side);
        place.epoch == panel.epoch && place.interaction == panel.interaction_revision
    }

    /// The focused entry of `side`, which View opens; never `..` or a blank
    /// row.
    pub(crate) fn focused(&self, side: Side) -> Option<ChosenEntry> {
        let panel = self.panel(side);
        if panel.rows_pending() {
            return None;
        }
        let row = panel.focused?;
        match self.row(side, row)? {
            Row::Up => None,
            Row::Entry(_) => panel.chosen_entry(row),
            Row::Result(item) => Some(ChosenEntry::of_result(item)),
        }
    }

    /// Whether `row` of `side` can be selected: a folder panel's entry,
    /// not `..`; any search panel row that no delete or move took. A row
    /// whose chunk has not arrived is selectable, because the selection is
    /// its position.
    pub(crate) fn selectable(&self, side: Side, row: u64) -> bool {
        let panel = self.panel(side);
        match panel.search() {
            None => panel.entry(row).is_some(),
            Some(search) => row < panel.count() && !search.list.row_taken(row, &self.taken),
        }
    }

    /// The operation `operation` gives an entry of `side`'s folder the name
    /// `name`, renaming the entry `from` if it is a rename: the focus goes
    /// to it when a row shows it and none shows `from`, and with `select` it
    /// is selected, because Gozler's own rename keeps the selection on the
    /// renamed entry, which a rename by another program does not. Another
    /// folder, or a listing after the operation's end without the name or
    /// still with `from`, forgets it. A name that a row shows exactly now
    /// is awaited only for a rename that may replace it: otherwise the
    /// operation does not create it, as when a move goes into a folder of
    /// that name.
    pub(crate) fn await_name(
        &mut self,
        side: Side,
        name: &str,
        from: Option<&str>,
        select: bool,
        operation: OperationId,
    ) {
        // A search panel shows no folder in which the name could show.
        if self.panel(side).search().is_some() {
            return;
        }
        let name = Name::of(name.encode_utf16().collect());
        let from = from.map(|from| Name::of(from.encode_utf16().collect()));
        let taken = Arc::clone(&self.taken);
        let panel = self.panel_mut(side);
        panel.set_awaited(Some(Awaited {
            name,
            select,
            from,
            operation,
            ended: None,
            origin: Some((Arc::clone(&panel.rows), panel.up_rows())),
        }));
        panel.interaction_revision += 1;
        panel.quick.invalidate();
        panel.submit(RowChange::Await, &taken);
    }

    /// The place that shows `side`'s awaited name, once a row shows it,
    /// after its list showed a change or a listing; it is then forgotten.
    /// `None` while no row shows it.
    pub(crate) fn show_awaited(&mut self, side: Side) -> Option<Place> {
        let panel = self.panel_mut(side);
        if panel.rows_pending() {
            return None;
        }
        let (place, selected) = panel.awaited_place.take()?;
        panel.set_awaited(None);
        panel.gone_focus = None;
        panel.select_shared(selected);
        panel.focused = place.focused;
        panel.quick.invalidate();
        Some(place)
    }

    /// Applies the watching thread's `changes` to `side`, whose list has
    /// the focus on `focused`, by name: the focus and the selection stay on
    /// their names, and when the focused entry goes, also by a rename, the
    /// focus moves to the next row. The panel remembers the name that went
    /// until the person moves the focus or the panel reads a folder, and the
    /// focus returns to it when a change brings it back. Returns the new
    /// place, and whether the panel must read its folder again, because a
    /// change named an entry by a short name that no row has. `None` while
    /// the panel shows no folder.
    pub(crate) fn apply_changes(
        &mut self,
        side: Side,
        changes: Vec<FolderChange>,
        focused: Option<u64>,
    ) -> Option<(Place, bool)> {
        let taken = Arc::clone(&self.taken);
        let panel = self.panel_mut(side);
        if !matches!(panel.view, View::Folder) {
            let items = changes.len();
            panel.retired.push(changes, items);
            return None;
        }
        panel.focused = focused;
        panel
            .submit(RowChange::Watch(changes), &taken)
            .map(|update| (update.place, update.read_again))
    }

    /// The ended operations `ended`, each with its request, which each
    /// folder panel keeps when its end makes it read its folder again; a
    /// search panel's list decides for itself ([`SearchList::accept_end`]).
    /// Returns what each panel that they changed does.
    #[cfg(test)]
    pub(crate) fn operations_ended(
        &mut self,
        ended: Arc<super::search_list::Ended>,
        has_ended: impl Fn(OperationId) -> bool,
    ) -> Vec<PanelFollow> {
        let receipt = self.observe_operations();
        self.accept_operations(receipt, Some(ended), has_ended)
    }

    pub(crate) fn kept_rows(&self) -> PanelKeeps {
        PanelKeeps(std::array::from_fn(|at| Arc::clone(&self.panels[at].rows)))
    }

    pub(crate) fn observe_operations(&mut self) -> EndReceipt {
        self.moment += 1;
        let moment = self.moment;
        let awaited = std::array::from_fn(|at| {
            self.panels[at]
                .awaited
                .as_ref()
                .map(|awaited| awaited.operation)
        });
        for panel in &mut self.panels {
            panel.observing = true;
            panel.end_revision = moment;
        }
        EndReceipt { moment, awaited }
    }

    pub(crate) fn accept_operations(
        &mut self,
        receipt: EndReceipt,
        ended: Option<Arc<super::search_list::Ended>>,
        has_ended: impl Fn(OperationId) -> bool,
    ) -> Vec<PanelFollow> {
        let moment = receipt.moment;
        let taken = Arc::clone(&self.taken);
        let mut out = Vec::new();
        for side in Side::BOTH {
            let panel = self.panel_mut(side);
            panel.observing = false;
            let Some(ended) = &ended else {
                continue;
            };
            // Command intent must keep its acknowledgment when a row candidate is discarded.
            if let Some(awaited) = &mut panel.awaited
                && awaited.ended.is_none()
                && receipt.awaited[side.index()] == Some(awaited.operation)
                && has_ended(awaited.operation)
            {
                awaited.ended = Some(moment);
                panel.interaction_revision += 1;
            }
            if panel.folder.is_empty() || panel.search().is_some() {
                continue;
            }
            panel.end_revision = moment;
            let cancelled = matches!(panel.view, View::Cancelled) && panel.read.is_none();
            if panel.omitted_end.is_some() || cancelled {
                panel.omitted_end = Some(moment);
                panel.reread = !cancelled;
                continue;
            }
            if panel.ends.len() == PENDING_ROWS {
                panel.invalidate();
                let ends = std::mem::take(&mut panel.ends);
                panel.retired.push(ends, work::INLINE_ITEMS + 1);
                panel.omitted_end = Some(moment);
                panel.reread = true;
                continue;
            }
            panel.ends.push(End {
                moment,
                evidence: Arc::clone(ended),
            });
            if !matches!(panel.calculation, CalculationState::Idle) || !panel.pending.is_empty() {
                continue;
            }
            if let Some(update) = panel.submit(RowChange::Ended, &taken)
                && (update.read_again || update.sizes)
            {
                out.push(PanelFollow {
                    side,
                    read_again: update.read_again,
                    sizes: update.sizes,
                });
            }
        }
        out
    }

    /// The entries that deletes and moves have taken, for every list.
    pub(crate) fn taken(&self) -> &Arc<Taken> {
        &self.taken
    }

    /// Whether either panel keeps the ended operation `id`.
    #[cfg(test)]
    pub(crate) fn keep(&self, id: OperationId) -> bool {
        self.panels.iter().any(|panel| panel.keeps(id))
    }

    /// The taken entries changed: each folder panel's rows show without
    /// them, and each search panel's selection loses them. `focused` are
    /// the lists' focused rows, by side.
    pub(super) fn set_taken(
        &mut self,
        taken: Arc<Taken>,
        focused: [Option<u64>; 2],
    ) -> [TakenRows; 2] {
        self.taken = taken;
        let taken = Arc::clone(&self.taken);
        Side::BOTH.map(|side| {
            let panel = self.panel_mut(side);
            if let Some(search) = panel.search_mut() {
                return TakenRows::Search {
                    deselect: search.list.deselect(),
                };
            }
            if !matches!(panel.view, View::Folder) {
                return TakenRows::Folder(Place::default());
            }
            panel.focused = focused[side.index()];
            if !matches!(panel.calculation, CalculationState::Idle) || !panel.pending.is_empty() {
                return TakenRows::Pending;
            }
            match panel.submit(RowChange::Filter, &taken) {
                Some(update) => TakenRows::Folder(update.place),
                None => TakenRows::Pending,
            }
        })
    }

    /// The request for `side`'s folder sizes: every direct subfolder of
    /// its folder, from gozlerd (DESIGN.md: Folder sizes).
    pub(crate) fn ask_sizes(&mut self, side: Side, generation: Generation) -> Option<Ask> {
        let panel = self.panel_mut(side);
        if !matches!(panel.view, View::Folder)
            || !panel.rows.entries().first().is_some_and(Entry::is_folder)
        {
            panel.sizes_asked = None;
            return None;
        }
        let filters = Filters {
            location: Some(panel.folder.clone()),
            only_directly: true,
            ..Filters::default()
        };
        let search = query::search(
            "folder:",
            Matching::default(),
            &filters,
            &QueryStart {
                now: jiff::Timestamp::UNIX_EPOCH,
                time_zone: jiff::tz::TimeZone::UTC,
            },
        )
        .ok()?;
        panel.sizes_asked = Some(generation);
        Some(Ask::FolderSizes(
            side,
            ChunkAsk {
                search,
                sort: SortSpec::default(),
                start: 0,
                len: SIZE_ROWS,
                snapshot: FromSnapshot::No,
            },
        ))
    }

    /// gozlerd's answer to `side`'s folder sizes: each subfolder's size,
    /// matched to the rows by name, exactly first, then without regard to
    /// case, as NTFS compares names. Sizes and their resulting order publish
    /// together, so a later change inserts by the new order.
    /// Whether it was the answer the panel waits for.
    pub(crate) fn accept_sizes(
        &mut self,
        side: Side,
        generation: Generation,
        outcome: Outcome,
    ) -> bool {
        let taken = Arc::clone(&self.taken);
        let panel = self.panel_mut(side);
        if panel.sizes_asked != Some(generation) || !matches!(panel.view, View::Folder) {
            let items = outcome.items();
            panel.retired.push(outcome, items);
            return false;
        }
        panel.sizes_asked = None;
        panel.sized = true;
        if let Some(update) = panel.submit(RowChange::Sizes(Box::new(outcome)), &taken) {
            panel.ready_update = Some(update);
        }
        true
    }

    pub(crate) fn expire_sizes(&mut self, side: Side, generation: Generation) -> bool {
        let panel = self.panel_mut(side);
        if panel.sizes_asked != Some(generation) {
            return false;
        }
        panel.sizes_asked = None;
        true
    }

    pub(crate) fn set_free(&mut self, side: Side, folder: &str, free: Option<u64>) -> bool {
        let panel = self.panel_mut(side);
        if folder_key(folder) != folder_key(&panel.folder) {
            return false;
        }
        panel.free = free;
        true
    }

    pub(crate) fn set_label(&mut self, letter: char, label: String) {
        self.labels.insert(letter, label);
    }

    /// The drive menu's drive item for `letter`: "&C:  Windows", or "&C:"
    /// while its label is unknown or empty.
    pub(crate) fn drive_label(&self, letter: char) -> String {
        match self.labels.get(&letter).filter(|label| !label.is_empty()) {
            Some(label) => format!("&{letter}:  {label}"),
            None => format!("&{letter}:"),
        }
    }

    /// Closes the quick search box; whether it was open.
    pub(crate) fn close_quick(&mut self, side: Side) -> bool {
        let panel = self.panel_mut(side);
        panel.quick.close(&mut panel.retired)
    }
}

/// What `ui.json` saves of the panels.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SavedPanels {
    #[serde(default)]
    pub(crate) left: SavedPanel,
    #[serde(default)]
    pub(crate) right: SavedPanel,
    #[serde(default)]
    pub(crate) active: Side,
}

#[cfg(test)]
#[allow(
    clippy::single_range_in_vec_init,
    reason = "a selection is a list of row ranges"
)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn wide(name: &str) -> Box<[u16]> {
        name.encode_utf16().collect()
    }

    pub(crate) fn file(name: &str, size: u64, modified: i64) -> Entry {
        Entry::new(wide(name), 0x20, size, modified)
    }

    fn folder(name: &str) -> Entry {
        Entry::new(wide(name), FILE_ATTRIBUTE_DIRECTORY, 0, 0)
    }

    pub(super) fn names(panels: &Panels, side: Side) -> Vec<String> {
        let panel = panels.panel(side);
        (0..panel.count())
            .map(|row| match panel.row(row).expect("a row") {
                Row::Up => "..".to_owned(),
                Row::Entry(entry) => entry.display_name(),
                Row::Result(item) => item.path.clone(),
            })
            .collect()
    }

    /// Panels whose left panel shows `folder` with `entries`.
    pub(super) fn showing(folder: &str, entries: Vec<Entry>) -> Panels {
        let mut panels = Panels::new(&SavedPanels::default());
        list(&mut panels, folder, entries);
        panels
    }

    /// Reads `folder` again in the left panel and lists `entries`, as the
    /// reading thread answers; returns the list's new place.
    pub(crate) fn list(panels: &mut Panels, folder: &str, entries: Vec<Entry>) -> Place {
        let ask = panels.go(Side::Left, folder.to_owned(), None);
        complete(panels, &ask, folder, entries)
    }

    /// Answers `ask` with the listing of `folder`, with the sizes and in
    /// the sort the reading thread gives it.
    fn complete(
        panels: &mut Panels,
        ask: &ReadAsk,
        folder: &str,
        mut entries: Vec<Entry>,
    ) -> Place {
        if folder == ask.path {
            super::give_sizes(&mut entries, &ask.sizes.values());
        }
        sort_listing(&mut entries, ask.sort);
        let event = ReadEvent::Listed {
            generation: ask.generation,
            folder: folder.to_owned(),
            entries,
            focus: None,
        };
        match panels.accept_read(Side::Left, event, None).change {
            ReadChange::Listed { place, .. } => place,
            ReadChange::Pending => loop {
                let input = panels
                    .take_calculation(Side::Left)
                    .expect("a fixture has pending work");
                if let Some(update) = panels.accept_calculation(Side::Left, input.run()) {
                    break update.place;
                }
            },
            other => panic!("not listed: {other:?}"),
        }
    }

    fn delete(sources: &[&str]) -> Arc<Request> {
        Arc::new(Request::Delete {
            sources: sources.iter().map(PathBuf::from).collect(),
        })
    }

    #[test]
    fn a_large_row_answer_remaps_current_selection_and_taken_without_replaying_the_change() {
        let entries = (0..1_100)
            .map(|i| file(&format!("item {i:04}"), 1, 0))
            .collect();
        let mut panels = showing(r"C:\f", entries);
        panels.set_selected(Side::Left, vec![1..2].into());
        panels.set_focused(Side::Left, Some(1));
        assert!(
            panels
                .apply_changes(
                    Side::Left,
                    vec![FolderChange::Added(file("000-new", 7, 0))],
                    Some(1)
                )
                .is_none()
        );
        assert_eq!(panels.panel(Side::Left).count(), 1_101);
        assert_eq!(panels.chosen_count(Side::Left), 0);
        let input = panels
            .take_calculation(Side::Left)
            .expect("large changes defer");
        let answer = input.run();
        let candidate = answer
            .rows
            .as_ref()
            .expect("an answer owns rows")
            .entries()
            .as_ptr();

        panels.native_selection_pending(Side::Left, true);
        assert!(panels.accept_calculation(Side::Left, answer).is_none());
        assert!(panels.take_calculation(Side::Left).is_none());
        assert_eq!(panels.panel(Side::Left).selected.as_ref(), &[1..2]);
        assert_eq!(panels.panel(Side::Left).count(), 1_101);

        panels.set_selected(Side::Left, vec![2..4].into());
        panels.set_focused(Side::Left, Some(2));
        let mut taken = Taken::default();
        taken.insert(
            OperationId::for_tests(51),
            Path::new(r"C:\f\item 0001"),
            false,
        );
        assert!(matches!(
            panels.set_taken(Arc::new(taken), [Some(2), None])[0],
            TakenRows::Pending
        ));
        panels.native_selection_pending(Side::Left, false);
        assert_eq!(panels.panel(Side::Left).selected.as_ref(), &[2..4]);
        assert_eq!(panels.panel(Side::Left).count(), 1_101);

        let remap = panels
            .take_calculation(Side::Left)
            .expect("new interaction requires a remap");
        let answer = remap.run();
        let selected = Arc::clone(&answer.selected.positions);
        let update = panels
            .accept_calculation(Side::Left, answer)
            .expect("the remap is current");
        assert_eq!(update.place.selected.as_slice(), [3..4]);
        assert!(Arc::ptr_eq(&panels.panel(Side::Left).selected, &selected));
        assert_eq!(panels.panel(Side::Left).rows.entries().as_ptr(), candidate);
        assert_eq!(
            panels.panel(Side::Left).shown_name("000-new").as_deref(),
            Some("000-new")
        );
        assert_eq!(panels.panel(Side::Left).shown_name("item 0001"), None);
    }

    #[test]
    fn an_overflow_keeps_the_end_barrier_and_awaited_acknowledgment_until_a_new_listing() {
        let entries = || {
            (0..1_100)
                .map(|i| file(&format!("item {i:04}"), 1, 0))
                .collect()
        };
        let mut panels = showing(r"C:\f", entries());
        let older = panels.read_again(Side::Left).expect("a folder shows");
        let id = OperationId::for_tests(52);
        panels.await_name(Side::Left, "created", None, false, id);
        let input = panels
            .take_calculation(Side::Left)
            .expect("a large awaited lookup defers");
        let request = Arc::new(Request::NewFolder {
            path: PathBuf::from(r"C:\f\created"),
        });
        end(&mut panels, &[(id, request)]);
        let moment = panels
            .panel(Side::Left)
            .awaited
            .as_ref()
            .expect("the command awaits its name")
            .ended;
        assert!(moment.is_some());
        for _ in 0..=PENDING_ROWS {
            panels.apply_changes(
                Side::Left,
                vec![FolderChange::Modified(file("item 0000", 2, 0))],
                None,
            );
        }
        assert!(panels.panel(Side::Left).is_calculating());
        let event = ReadEvent::Listed {
            generation: older.generation,
            folder: r"C:\f".to_owned(),
            entries: entries(),
            focus: None,
        };
        assert_eq!(
            panels.accept_read(Side::Left, event, None).change,
            ReadChange::Pending
        );
        assert!(panels.take_reread(Side::Left));
        assert!(panels.accept_calculation(Side::Left, input.run()).is_none());
        assert_eq!(
            panels
                .panel(Side::Left)
                .awaited
                .as_ref()
                .expect("invalidation keeps command intent")
                .ended,
            moment
        );

        let newer = panels
            .read_again(Side::Left)
            .expect("overflow requests a fresh listing");
        complete(&mut panels, &newer, r"C:\f", entries());
        assert!(!panels.panel(Side::Left).is_calculating());
        assert!(panels.panel(Side::Left).awaited.is_none());
        panels.apply_changes(
            Side::Left,
            vec![FolderChange::Added(folder("created"))],
            None,
        );
        let input = panels
            .take_calculation(Side::Left)
            .expect("the folder change defers");
        panels
            .accept_calculation(Side::Left, input.run())
            .expect("the change is current");
        assert_eq!(panels.show_awaited(Side::Left), None);
    }

    /// `Ctrl+Right` shows the focused folder in the right panel, and for
    /// `..` or a file the folder above or around it, as Total Commander
    /// does. `Ctrl+Left` on a file of the left panel reads nothing, because
    /// showing its own folder again would drop the panel's selection.
    #[test]
    fn ctrl_arrow_shows_the_focused_folder_in_that_panel() {
        let mut panels = showing(r"C:\f", vec![folder("sub"), file("a.txt", 1, 0)]);
        assert_eq!(names(&panels, Side::Left), ["..", "sub", "a.txt"]);
        let path = |ask: Option<ReadAsk>| ask.map(|ask| ask.path);
        let mut right = |row| path(panels.show_in(Side::Left, Some(row), Side::Right));
        assert_eq!(right(0).as_deref(), Some(r"C:\"));
        assert_eq!(right(1).as_deref(), Some(r"C:\f\sub"));
        assert_eq!(right(2).as_deref(), Some(r"C:\f"));
        assert_eq!(panels.show_in(Side::Left, Some(2), Side::Left), None);
    }

    /// An ended delete hides its rows until a listing that started after
    /// its end arrives, whichever of the change and the end comes first; a
    /// listing that started before the end releases nothing. Then the
    /// entry that the delete could not remove shows again, because the
    /// panel would otherwise hide a file that is still on the disk.
    #[test]
    fn an_ended_delete_hides_its_rows_until_a_listing_after_its_end() {
        for change_first in [true, false] {
            let id = OperationId::for_tests(1);
            let mut panels = showing(
                r"C:\f",
                vec![
                    file("a.txt", 1, 0),
                    file("b.txt", 1, 0),
                    file("c.txt", 1, 0),
                ],
            );
            let running = |ended| {
                let mut taken = Taken::default();
                taken.insert(id, Path::new(r"C:\f\a.txt"), ended);
                taken.insert(id, Path::new(r"C:\f\b.txt"), ended);
                Arc::new(taken)
            };
            panels.set_taken(running(false), [None; 2]);
            assert_eq!(names(&panels, Side::Left), ["..", "c.txt"]);
            // A reading that started while the delete ran.
            let before = panels.read_again(Side::Left).expect("a folder shows");
            // b.txt went; a.txt was held by another program.
            let gone = || vec![FolderChange::Gone(wide("b.txt"))];
            if change_first {
                panels.apply_changes(Side::Left, gone(), None);
            }
            let again = end(
                &mut panels,
                &[(id, delete(&[r"C:\f\a.txt", r"C:\f\b.txt"]))],
            );
            assert_eq!(
                again,
                [PanelFollow {
                    side: Side::Left,
                    read_again: true,
                    sizes: true
                }]
            );
            panels.set_taken(running(true), [None; 2]);
            if !change_first {
                panels.apply_changes(Side::Left, gone(), None);
            }
            assert_eq!(
                names(&panels, Side::Left),
                ["..", "c.txt"],
                "change first: {change_first}"
            );
            complete(
                &mut panels,
                &before,
                r"C:\f",
                vec![file("a.txt", 1, 0), file("c.txt", 1, 0)],
            );
            assert!(panels.keep(id));
            assert_eq!(names(&panels, Side::Left), ["..", "c.txt"]);
            let after = panels.read_again(Side::Left).expect("a folder shows");
            complete(
                &mut panels,
                &after,
                r"C:\f",
                vec![file("a.txt", 1, 0), file("c.txt", 1, 0)],
            );
            assert!(!panels.keep(id));
            assert_eq!(names(&panels, Side::Left), ["..", "a.txt", "c.txt"]);
        }
    }

    /// Folders come first, each group in the sort's order, so a sort by
    /// size never mixes a large file among the folders.
    #[test]
    fn folders_sort_first() {
        let entries = || {
            vec![
                file("b.txt", 5, 2),
                folder("Zeta"),
                file("A.txt", 9, 1),
                folder("alpha"),
            ]
        };
        let mut panels = showing(r"C:\", entries());
        assert_eq!(
            names(&panels, Side::Left),
            ["alpha", "Zeta", "A.txt", "b.txt"]
        );
        let sort = header_sort(SortSpec::default(), SortKey::Size);
        let ask = panels.sort(Side::Left, sort).expect("a folder shows");
        complete(&mut panels, &ask, r"C:\", entries());
        assert_eq!(
            names(&panels, Side::Left),
            ["alpha", "Zeta", "A.txt", "b.txt"]
        );
        let sort = header_sort(sort, SortKey::Size);
        let ask = panels.sort(Side::Left, sort).expect("a folder shows");
        complete(&mut panels, &ask, r"C:\", entries());
        assert_eq!(
            names(&panels, Side::Left),
            ["alpha", "Zeta", "b.txt", "A.txt"]
        );
    }

    /// A header click sorts through a new listing, so the rows keep their
    /// order until it comes, and then the list scrolls to the focus,
    /// because the rows moved.
    #[test]
    fn a_new_sort_comes_with_a_listing() {
        let entries = || vec![file("a", 1, 0), file("b", 9, 0)];
        let mut panels = showing(r"C:\f", entries());
        let sort = SortSpec {
            key: SortKey::Size,
            dir: SortDir::Desc,
        };
        let ask = panels.sort(Side::Left, sort).expect("a folder shows");
        assert_eq!(ask.sort, sort);
        assert_eq!(panels.panel(Side::Left).sort(), sort);
        assert_eq!(names(&panels, Side::Left), ["..", "a", "b"]);
        let mut listed = entries();
        sort_listing(&mut listed, ask.sort);
        let event = ReadEvent::Listed {
            generation: ask.generation,
            folder: r"C:\f".to_owned(),
            entries: listed,
            focus: None,
        };
        let change = panels.accept_read(Side::Left, event, Some(1)).change;
        assert!(matches!(change, ReadChange::Listed { again: false, .. }));
        assert_eq!(names(&panels, Side::Left), ["..", "b", "a"]);
    }

    /// When Safely Remove is refused during a move to another folder, that
    /// reading starts again; and when the drive goes instead, the panel
    /// names the folder it was going to, whose drive went.
    #[test]
    fn a_stopped_reading_starts_again() {
        let mut panels = showing(r"C:\a", vec![]);
        let first = panels.go(Side::Left, r"E:\x".to_owned(), None);
        let again = panels.restart_reading(Side::Left).expect("a reading runs");
        assert_eq!(again.path, r"E:\x");
        assert!(again.generation > first.generation);
        let stale = ReadEvent::Opened {
            generation: first.generation,
            folder: r"E:\x".to_owned(),
        };
        assert_eq!(
            panels.accept_read(Side::Left, stale, None).change,
            ReadChange::Stale
        );
        panels.drive_removed(Side::Left);
        assert!(matches!(
            panels.panel(Side::Left).view(),
            View::NotReady { drive } if drive == r"E:\"
        ));
        assert_eq!(
            panels.read_again(Side::Left).expect("a folder").path,
            r"E:\x"
        );
    }

    /// An operation that ended while another folder was read keeps its
    /// rows hidden, so that listing asks for one more.
    #[test]
    fn a_listing_older_than_an_end_asks_for_another() {
        let mut panels = showing(r"C:\f", vec![file("a.txt", 1, 0)]);
        let ask = panels.go(Side::Left, r"C:\f".to_owned(), None);
        let id = OperationId::for_tests(1);
        end(&mut panels, &[(id, delete(&[r"C:\f\a.txt"]))]);
        let event = ReadEvent::Listed {
            generation: ask.generation,
            folder: r"C:\f".to_owned(),
            entries: vec![],
            focus: None,
        };
        let change = panels.accept_read(Side::Left, event, None).change;
        assert!(matches!(change, ReadChange::Listed { stale: true, .. }));
    }

    /// A case-sensitive folder, as WSL makes one, can hold `Makefile` and
    /// `makefile`. A name that is there now is the name on the disk, so an
    /// added `makefile` leaves `Makefile`, reading the folder again keeps
    /// the selection on the one selected, and a removed `makefile` leaves
    /// `Makefile`; a name that went and that no entry has exactly still
    /// takes its one case-twin.
    #[test]
    fn a_change_keeps_a_name_that_differs_only_in_case() {
        let mut panels = showing(r"C:\src", vec![file("Makefile", 1, 0)]);
        let changes = vec![FolderChange::Added(file("makefile", 2, 0))];
        panels.apply_changes(Side::Left, changes, None);
        assert_eq!(names(&panels, Side::Left), ["..", "Makefile", "makefile"]);
        panels.set_selected(Side::Left, vec![1..2].into());
        let ask = panels.read_again(Side::Left).expect("a folder shows");
        let listed = vec![file("Makefile", 1, 0), file("makefile", 2, 0)];
        assert_eq!(
            complete(&mut panels, &ask, r"C:\src", listed)
                .selected
                .as_slice(),
            [1..2]
        );
        let changes = vec![FolderChange::Gone(wide("makefile"))];
        panels.apply_changes(Side::Left, changes, None);
        assert_eq!(names(&panels, Side::Left), ["..", "Makefile"]);
        let changes = vec![FolderChange::Gone(wide("MAKEFILE"))];
        panels.apply_changes(Side::Left, changes, None);
        assert_eq!(names(&panels, Side::Left), [".."]);
    }

    /// An entry that a delete hides loses its selection: when the delete
    /// fails and it shows again, the next change must not select it beside
    /// the row the person selected since. Also when the delete takes the
    /// whole folder.
    #[test]
    fn a_hidden_entry_comes_back_unselected() {
        for taken_path in [r"C:\f\a.txt", r"C:\f"] {
            let mut panels = showing(r"C:\f", vec![file("a.txt", 1, 0), file("b.txt", 1, 0)]);
            // `..`, a.txt, b.txt: a.txt selected.
            panels.set_selected(Side::Left, vec![1..2].into());
            let mut taken = Taken::default();
            taken.insert(OperationId::for_tests(1), Path::new(taken_path), false);
            panels.set_taken(Arc::new(taken), [None; 2]);
            panels.set_taken(Arc::default(), [None; 2]);
            assert_eq!(names(&panels, Side::Left), ["..", "a.txt", "b.txt"]);
            panels.set_selected(Side::Left, vec![2..3].into());
            let changes = vec![FolderChange::Modified(file("b.txt", 2, 0))];
            let (place, _) = panels
                .apply_changes(Side::Left, changes, None)
                .expect("a folder shows");
            assert_eq!(place.selected.as_slice(), [2..3], "taken: {taken_path}");
        }
    }

    #[test]
    fn late_quick_search_does_not_reopen_after_escape_or_steal_newer_interaction() {
        for (escape, select) in [(false, false), (false, true), (true, false)] {
            let mut panels = showing(
                r"C:\docs",
                vec![file("report.pdf", 1, 0), file("tax.pdf", 1, 0)],
            );
            panels.find_quick(Side::Left, QuickAction::Character('r'));
            let pending = panels.take_quick(Side::Left).unwrap();
            if escape {
                assert!(panels.close_quick(Side::Left));
                panels.find_quick(Side::Left, QuickAction::Character('t'));
            } else if select {
                panels.set_selected(Side::Left, vec![2..3].into());
            } else {
                panels.set_focused(Side::Left, Some(2));
            }
            assert_eq!(panels.accept_quick(Side::Left, pending.run()), None);
            assert_eq!(panels.panel(Side::Left).quick_text(), None);
            if escape {
                let current = panels.take_quick(Side::Left).unwrap();
                assert_eq!(panels.accept_quick(Side::Left, current.run()), Some(2));
                assert_eq!(panels.panel(Side::Left).quick_text(), Some("t"));
            }
        }
    }

    #[test]
    fn a_watch_update_retries_pending_quick_search_instead_of_losing_the_character() {
        let mut panels = showing(
            r"C:\docs",
            vec![file("report.pdf", 1, 0), file("tax.pdf", 1, 0)],
        );
        panels.find_quick(Side::Left, QuickAction::Character('r'));
        let pending = panels.take_quick(Side::Left).unwrap();
        panels.apply_changes(
            Side::Left,
            vec![FolderChange::Added(file("aaa.txt", 1, 0))],
            None,
        );
        assert_eq!(panels.accept_quick(Side::Left, pending.run()), None);
        let current = panels
            .take_quick(Side::Left)
            .expect("a row change retries the pending character");
        assert_eq!(panels.accept_quick(Side::Left, current.run()), Some(2));
        assert_eq!(panels.panel(Side::Left).quick_text(), Some("r"));
    }

    #[test]
    fn quick_search_goes_to_the_first_match() {
        fn quick(panels: &mut Panels, action: QuickAction) -> Option<u64> {
            panels.find_quick(Side::Left, action);
            let input = panels.take_quick(Side::Left).unwrap();
            let row = panels.accept_quick(Side::Left, input.run());
            if row.is_some() {
                panels.set_focused(Side::Left, row);
            }
            row
        }
        let mut panels = showing(
            r"C:\docs",
            vec![
                file("notes.txt", 1, 0),
                file("report.pdf", 1, 0),
                file("Report-draft.pdf", 1, 0),
                file("tax.pdf", 1, 0),
            ],
        );
        // A first character that matches nothing opens no box.
        assert_eq!(quick(&mut panels, QuickAction::Character('z')), None);
        assert_eq!(panels.panel(Side::Left).quick_text(), None);
        // `..`, notes, Report-draft, report, tax.
        assert_eq!(quick(&mut panels, QuickAction::Character('r')), Some(2));
        assert_eq!(quick(&mut panels, QuickAction::Character('E')), Some(2));
        assert_eq!(quick(&mut panels, QuickAction::Character('x')), None);
        assert_eq!(panels.panel(Side::Left).quick_text(), Some("rE"));
        assert_eq!(
            quick(&mut panels, QuickAction::Step { down: true }),
            Some(3)
        );
        assert_eq!(quick(&mut panels, QuickAction::Step { down: true }), None);
        assert_eq!(
            quick(&mut panels, QuickAction::Step { down: false }),
            Some(2)
        );
        assert_eq!(quick(&mut panels, QuickAction::Back), Some(2));
        assert_eq!(quick(&mut panels, QuickAction::Character('.')), None);
        assert!(panels.close_quick(Side::Left));
    }

    /// A change applies by name: a new file takes its sorted place, the
    /// selection stays on its names, and when the focused entry goes, the
    /// focus moves to the row after it.
    #[test]
    fn a_change_applies_by_name() {
        let mut panels = showing(
            r"C:\f",
            vec![file("a", 1, 0), file("c", 1, 0), file("d", 1, 0)],
        );
        // `..`, a, c, d: c focused, a and d selected.
        panels.set_selected(Side::Left, vec![1..2, 3..4].into());
        let (place, again) = panels
            .apply_changes(
                Side::Left,
                vec![
                    FolderChange::Added(file("b", 1, 0)),
                    FolderChange::Gone(wide("c")),
                ],
                Some(2),
            )
            .expect("a folder shows");
        assert!(!again);
        assert_eq!(names(&panels, Side::Left), ["..", "a", "b", "d"]);
        assert_eq!(
            place,
            Place {
                focused: Some(3),
                selected: Arc::new(vec![1..2, 3..4]),
            }
        );
    }

    /// Word saves `doc.docx` by renaming it to a `~WRL` name, renaming its
    /// temporary file to `doc.docx`, and deleting the `~WRL` file, in one
    /// batch or several. The focus shows on a neighbouring row while
    /// `doc.docx` is away and returns to it, so the next command acts on
    /// the file the person focused.
    #[test]
    fn the_focus_returns_to_a_name_that_comes_back() {
        let mut panels = showing(
            r"C:\f",
            vec![
                file("doc.docx", 1, 0),
                file("x.txt", 1, 0),
                file("~WRD0001.tmp", 2, 0),
            ],
        );
        // `..`, doc.docx, x.txt, ~WRD0001.tmp: doc.docx focused.
        let batches = [
            FolderChange::Renamed {
                from: wide("doc.docx"),
                to: file("~WRL0001.tmp", 1, 0),
            },
            FolderChange::Renamed {
                from: wide("~WRD0001.tmp"),
                to: file("doc.docx", 2, 0),
            },
            FolderChange::Gone(wide("~WRL0001.tmp")),
        ];
        let mut focused = Some(1);
        for change in batches {
            let (place, _) = panels
                .apply_changes(Side::Left, vec![change], focused)
                .expect("a folder shows");
            focused = place.focused;
        }
        assert_eq!(names(&panels, Side::Left), ["..", "doc.docx", "x.txt"]);
        assert_eq!(focused, Some(1));

        // A save by delete and create, when the file is the folder's only
        // entry: the focus shows on `..`, or at a root on no row, meanwhile.
        for (folder, first) in [(r"C:\g", 1), (r"C:\", 0)] {
            let mut panels = showing(folder, vec![file("doc.docx", 1, 0)]);
            let mut focused = Some(first);
            for change in [
                FolderChange::Gone(wide("doc.docx")),
                FolderChange::Added(file("doc.docx", 2, 0)),
            ] {
                let (place, _) = panels
                    .apply_changes(Side::Left, vec![change], focused)
                    .expect("a folder shows");
                focused = place.focused;
            }
            assert_eq!(focused, Some(first), "{folder}");
        }
    }

    /// A batch of changes keeps a replaced folder's size until gozlerd
    /// gives it again, and the selection on its names, because the merge
    /// takes the entries out and puts them back. The status line's sum,
    /// which the panel keeps as entries come and go, follows.
    #[test]
    fn a_batch_keeps_sizes_and_the_selection() {
        let mut panels = showing(r"C:\d", vec![folder("Photos"), file("a", 5, 0)]);
        give_sizes(&mut panels, &[(r"C:\d\Photos", 40)]);
        // `..`, Photos, a: both selected.
        panels.set_selected(Side::Left, vec![1..3].into());
        let (place, _) = panels
            .apply_changes(
                Side::Left,
                vec![
                    FolderChange::Modified(folder("Photos")),
                    FolderChange::Renamed {
                        from: wide("a"),
                        to: file("b", 5, 0),
                    },
                    FolderChange::Added(file("c", 1, 0)),
                ],
                Some(1),
            )
            .expect("a folder shows");
        assert_eq!(names(&panels, Side::Left), ["..", "Photos", "b", "c"]);
        assert_eq!(
            panels
                .panel(Side::Left)
                .entry(1)
                .expect("Photos")
                .folder_size(),
            Some(40)
        );
        assert_eq!(
            place,
            Place {
                focused: Some(1),
                selected: Arc::new(vec![1..2]),
            }
        );
        panels.set_selected(Side::Left, Vec::new().into());
        let size = panels.panel(Side::Left).counts().size;
        assert_eq!(size.map(|size| size.bytes), Some(46));
    }

    /// An editor that saves through a temporary name and keeps a backup
    /// renames the selected file away and another file to its name: the
    /// selection stays on the name, so the backup is not selected, and a
    /// Delete takes only the one file the person selected.
    #[test]
    fn a_save_with_a_backup_keeps_one_selected() {
        let mut panels = showing(r"C:\f", vec![file("doc.txt", 1, 0)]);
        // `..`, doc.txt: doc.txt selected.
        panels.set_selected(Side::Left, vec![1..2].into());
        let changes = vec![
            FolderChange::Added(file("doc.tmp", 2, 0)),
            FolderChange::Renamed {
                from: wide("doc.txt"),
                to: file("doc.bak", 1, 0),
            },
            FolderChange::Renamed {
                from: wide("doc.tmp"),
                to: file("doc.txt", 2, 0),
            },
        ];
        let (place, _) = panels
            .apply_changes(Side::Left, changes, None)
            .expect("a folder shows");
        assert_eq!(names(&panels, Side::Left), ["..", "doc.bak", "doc.txt"]);
        assert_eq!(place.selected.as_slice(), [2..3]);
    }

    /// A program can write a file under another spelling of its name, and
    /// the watch reports the change under that spelling: the change applies
    /// to the entry, which keeps the name stored on the disk.
    #[test]
    fn a_change_keeps_the_stored_spelling() {
        let mut panels = showing(r"C:\f", vec![file("Readme.txt", 1, 0)]);
        let changes = vec![FolderChange::Modified(file("README.TXT", 2, 0))];
        panels.apply_changes(Side::Left, changes, None);
        assert_eq!(names(&panels, Side::Left), ["..", "Readme.txt"]);
    }

    /// A rename by another program carries neither the focus nor the
    /// selection, so Gozler's own `F6` rename brings both to the new name
    /// itself. A listing after an end that did not create the name forgets
    /// it, so a later file of that name does not take the focus. A name
    /// that a row shows already, such as a folder that `F6` moves the entry
    /// into, is not awaited, so that row never takes the focus.
    #[test]
    fn gozler_s_own_rename_keeps_the_focus_and_the_selection() {
        let mut panels = showing(r"C:\f", vec![file("a.txt", 1, 0), file("b.txt", 1, 0)]);
        // `..`, a.txt, b.txt: a.txt selected and focused.
        panels.set_selected(Side::Left, vec![1..2].into());
        panels.await_name(
            Side::Left,
            "c.txt",
            Some("a.txt"),
            true,
            OperationId::for_tests(1),
        );
        let renamed = FolderChange::Renamed {
            from: wide("a.txt"),
            to: file("c.txt", 1, 0),
        };
        let (place, _) = panels
            .apply_changes(Side::Left, vec![renamed], Some(1))
            .expect("a folder shows");
        assert!(place.selected.is_empty());
        let place = panels.show_awaited(Side::Left).expect("c.txt shows");
        assert_eq!(names(&panels, Side::Left), ["..", "b.txt", "c.txt"]);
        assert_eq!(
            (place.focused, place.selected),
            (Some(2), Arc::new(vec![2..3]))
        );
        assert_eq!(panels.show_awaited(Side::Left), None, "it is shown once");

        let id = OperationId::for_tests(2);
        panels.await_name(Side::Left, "d.txt", None, false, id);
        end(&mut panels, &[(id, delete(&[r"C:\other\x"]))]);
        let ask = panels.read_again(Side::Left).expect("a folder shows");
        complete(&mut panels, &ask, r"C:\f", vec![file("b.txt", 1, 0)]);
        let ask = panels.read_again(Side::Left).expect("a folder shows");
        complete(&mut panels, &ask, r"C:\f", vec![file("d.txt", 1, 0)]);
        assert_eq!(panels.show_awaited(Side::Left), None);

        panels.await_name(Side::Left, "d.txt", None, true, OperationId::for_tests(3));
        assert_eq!(panels.show_awaited(Side::Left), None);
    }

    /// A watch can name an entry by its short name, which no row shows, so
    /// a rename from one must read the folder again, or the row under its
    /// long name would stay (#111).
    #[test]
    fn a_rename_from_a_short_name_reads_the_folder_again() {
        let mut panels = showing(r"C:\f", vec![file("Long report.txt", 1, 0)]);
        let renamed = FolderChange::Renamed {
            from: wide("LONGRE~1.TXT"),
            to: file("new.txt", 1, 0),
        };
        let (_, again) =
            (panels.apply_changes(Side::Left, vec![renamed], None)).expect("a folder shows");
        assert!(again);
    }

    /// `F6` to a name a row shows and Replace brings the focus to that row
    /// once the renamed entry is gone (#111); Skip, which keeps the entry,
    /// forgets the name at the listing after the end, so the row never
    /// takes the focus.
    #[test]
    fn a_rename_that_replaces_takes_the_focus_once_done() {
        let entries = vec![file("a.txt", 1, 0), file("b.txt", 1, 0)];
        let mut panels = showing(r"C:\f", entries.clone());
        panels.set_selected(Side::Left, vec![1..2].into());
        panels.await_name(
            Side::Left,
            "b.txt",
            Some("a.txt"),
            true,
            OperationId::for_tests(1),
        );
        assert_eq!(panels.show_awaited(Side::Left), None, "the rename waits");
        let renamed = FolderChange::Renamed {
            from: wide("a.txt"),
            to: file("b.txt", 1, 0),
        };
        panels.apply_changes(Side::Left, vec![renamed], Some(1));
        let place = panels.show_awaited(Side::Left).expect("b.txt replaced");
        assert_eq!(
            (place.focused, place.selected),
            (Some(1), Arc::new(vec![1..2]))
        );

        let mut panels = showing(r"C:\f", entries.clone());
        let id = OperationId::for_tests(2);
        panels.await_name(Side::Left, "b.txt", Some("a.txt"), false, id);
        end(&mut panels, &[(id, delete(&[r"C:\other\x"]))]);
        let ask = panels.read_again(Side::Left).expect("a folder shows");
        complete(&mut panels, &ask, r"C:\f", entries);
        panels.apply_changes(Side::Left, vec![FolderChange::Gone(wide("a.txt"))], None);
        assert_eq!(panels.show_awaited(Side::Left), None);
    }

    /// New folder refuses a name that a row shows in any case, and names
    /// the row's own spelling, but not one that only a row taken by a
    /// delete shows, because the person does not see that row
    /// (DESIGN.md: New folder).
    #[test]
    fn new_folder_checks_the_names_the_rows_show() {
        let mut panels = showing(
            r"C:\f",
            vec![folder("photos"), file("Photos", 1, 0), file("x.txt", 1, 0)],
        );
        let shown = |panels: &Panels, name| panels.panel(Side::Left).shown_name(name);
        assert_eq!(shown(&panels, "Photos").as_deref(), Some("Photos"));
        assert_eq!(shown(&panels, "PHOTOS").as_deref(), Some("photos"));
        let mut taken = Taken::default();
        taken.insert(OperationId::for_tests(1), Path::new(r"C:\f\x.txt"), false);
        panels.set_taken(Arc::new(taken), [None; 2]);
        assert_eq!(shown(&panels, "x.txt"), None);
        assert_eq!(shown(&panels, "Photos").as_deref(), Some("Photos"));

        panels.apply_changes(
            Side::Left,
            vec![FolderChange::Renamed {
                from: wide("Photos"),
                to: file("Images", 1, 0),
            }],
            None,
        );
        assert_eq!(shown(&panels, "Photos").as_deref(), Some("photos"));
        assert_eq!(shown(&panels, "IMAGES").as_deref(), Some("Images"));
        panels.set_taken(Arc::new(Taken::default()), [None; 2]);
        assert_eq!(shown(&panels, "x.txt").as_deref(), Some("x.txt"));

        list(&mut panels, r"C:\f", vec![folder("Photos")]);
        assert_eq!(shown(&panels, "PHOTOS").as_deref(), Some("Photos"));
        assert_eq!(shown(&panels, "Images"), None);
    }

    #[test]
    fn cancelling_navigation_preserves_the_retained_view_and_recovers_its_watch() {
        let mut panels = showing(r"C:\old", vec![file("keep.txt", 1, 0)]);
        let navigate = panels.go(Side::Left, r"E:\blocked".to_owned(), None);
        assert!(panels.cancel_read(Side::Left, navigate.generation));
        assert!(panels.take_reread(Side::Left));
        let recovery = panels
            .read_again(Side::Left)
            .expect("the retained folder needs its watch restored");
        assert_eq!(recovery.path, r"C:\old");
        complete(
            &mut panels,
            &recovery,
            r"C:\old",
            vec![file("keep.txt", 1, 0)],
        );
        assert!(panels.enter_search(Side::Left));
        let receipt = panels.observe_operations();
        let navigate = panels.go(Side::Left, r"E:\blocked".to_owned(), None);
        assert!(panels.cancel_read(Side::Left, navigate.generation));
        assert!(
            panels
                .accept_operations(receipt, None, |_| false)
                .is_empty()
        );
        assert!(panels.panel(Side::Left).search().is_some());
        assert!(!panels.panel(Side::Left).is_calculating());
        assert!(!panels.take_reread(Side::Left));

        let entries = || {
            (0..work::INLINE_ITEMS + 1)
                .map(|i| file(&format!("item {i:04}"), 1, 0))
                .collect()
        };
        let mut panels = showing(r"C:\old", entries());
        panels.set_selected(Side::Left, vec![1..2].into());
        panels.set_focused(Side::Left, Some(1));
        panels.apply_changes(
            Side::Left,
            vec![FolderChange::Added(file("discarded", 2, 0))],
            Some(1),
        );
        let candidate = panels
            .take_calculation(Side::Left)
            .expect("a large change defers");
        end(
            &mut panels,
            &[(OperationId::for_tests(91), delete(&[r"C:\old\item 0001"]))],
        );
        let navigate = panels.go(Side::Left, r"E:\blocked".to_owned(), None);
        assert!(panels.cancel_read(Side::Left, navigate.generation));
        assert!(panels.panel(Side::Left).omitted_end.is_some());
        assert!(panels.take_reread(Side::Left));
        let recovery = panels
            .read_again(Side::Left)
            .expect("the retained folder needs a fresh listing");
        assert_eq!(recovery.path, r"C:\old");
        assert!(
            panels
                .accept_calculation(Side::Left, candidate.run())
                .is_none()
        );
        assert!(panels.panel(Side::Left).is_calculating());
        assert_eq!(panels.chosen_count(Side::Left), 0);
        complete(&mut panels, &recovery, r"C:\old", entries());
        assert!(!panels.panel(Side::Left).is_calculating());
        assert_eq!(panels.chosen_count(Side::Left), 1);
        assert!(panels.chosen_source(Side::Left).is_some());
        assert!(panels.panel(Side::Left).shown_name("discarded").is_none());
    }

    #[test]
    fn cancelling_same_folder_recovery_waits_for_explicit_retry() {
        let entries = || {
            (0..work::INLINE_ITEMS + 1)
                .map(|i| file(&format!("item {i:04}"), 1, 0))
                .collect()
        };
        let mut panels = showing(r"C:\old", entries());
        panels.set_selected(Side::Left, vec![1..2].into());
        panels.apply_changes(
            Side::Left,
            vec![FolderChange::Added(file("discarded", 2, 0))],
            Some(1),
        );
        let candidate = panels
            .take_calculation(Side::Left)
            .expect("a large change defers");
        let recovery = panels.read_again(Side::Left).expect("a folder shows");
        assert!(panels.cancel_read(Side::Left, recovery.generation));
        assert!(panels.panel(Side::Left).omitted_end.is_some());
        assert!(!panels.take_reread(Side::Left));
        assert!(matches!(panels.panel(Side::Left).view(), View::Cancelled));
        assert_eq!(panels.panel(Side::Left).count(), 0);
        assert_eq!(panels.selected_count(Side::Left), 0);
        assert!(
            panels
                .accept_calculation(Side::Left, candidate.run())
                .is_none()
        );
        let follow = end(
            &mut panels,
            &[(OperationId::for_tests(92), delete(&[r"C:\old\item 0001"]))],
        );
        assert!(!follow.iter().any(|follow| follow.side == Side::Left));
        assert!(!panels.take_reread(Side::Left));
        assert!(panels.panel(Side::Left).omitted_end.is_some());
        assert!(matches!(panels.panel(Side::Left).view(), View::Cancelled));
        let retry = panels
            .read_again(Side::Left)
            .expect("Retry starts a new listing explicitly");
        complete(&mut panels, &retry, r"C:\old", entries());
        assert!(!panels.panel(Side::Left).is_calculating());
        assert!(matches!(panels.panel(Side::Left).view(), View::Folder));
    }

    #[test]
    fn a_cancelled_read_keeps_accepted_rows_and_rejects_its_late_candidate() {
        let mut panels = showing(r"C:\old", vec![file("keep.txt", 1, 0)]);
        let other = panels.go(Side::Right, r"D:\other".to_owned(), None);
        let ask = panels.go(Side::Left, r"E:\blocked".to_owned(), None);
        assert!(!panels.cancel_read(Side::Left, ask.generation + 1));
        assert!(panels.cancel_read(Side::Left, ask.generation));
        assert_eq!(names(&panels, Side::Left), ["..", "keep.txt"]);
        assert_eq!(panels.panel(Side::Left).path_box(), r"C:\old");
        assert_eq!(
            panels
                .accept_read(
                    Side::Left,
                    ReadEvent::Opened {
                        generation: ask.generation,
                        folder: ask.path,
                    },
                    None
                )
                .change,
            ReadChange::Stale
        );

        let ask = panels.go(Side::Left, r"E:\large".to_owned(), None);
        panels.accept_read(
            Side::Left,
            ReadEvent::Opened {
                generation: ask.generation,
                folder: ask.path.clone(),
            },
            None,
        );
        let listed = ReadEvent::Listed {
            generation: ask.generation,
            folder: ask.path.clone(),
            entries: (0..work::INLINE_ITEMS + 1)
                .map(|i| file(&format!("file {i}"), 1, 0))
                .collect(),
            focus: None,
        };
        assert_eq!(
            panels.accept_read(Side::Left, listed, None).change,
            ReadChange::Pending
        );
        let calculating = panels
            .take_calculation(Side::Left)
            .expect("a large listing is deferred");
        assert!(panels.cancel_read(Side::Left, ask.generation));
        assert!(
            panels
                .accept_calculation(Side::Left, calculating.run())
                .is_none()
        );
        // The folder opened but its listing never applied, so the panel
        // still shows the folder it was leaving.
        assert!(matches!(panels.panel(Side::Left).view(), View::Folder));
        assert_eq!(panels.panel(Side::Left).path_box(), r"C:\old");
        assert_eq!(names(&panels, Side::Left), ["..", "keep.txt"]);
        assert_eq!(
            panels.panel(Side::Right).reading_generation(),
            Some(other.generation)
        );
        let retry = panels
            .read_again(Side::Left)
            .expect("a cancelled reading can be retried");
        assert!(!panels.cancel_read(Side::Left, ask.generation));
        assert_eq!(
            panels.panel(Side::Left).reading_generation(),
            Some(retry.generation)
        );
    }

    /// A folder that cannot be opened while the panel reads another folder,
    /// or shows a drive that is not ready, is refused: the panel stays as it
    /// was instead of showing the failed folder's drive as not ready.
    #[test]
    fn a_failed_move_from_reading_or_not_ready_is_refused() {
        let fail = |panels: &mut Panels, path: &str| {
            let ask = panels.go(Side::Left, path.to_owned(), None);
            let event = ReadEvent::Failed {
                generation: ask.generation,
                path: path.to_owned(),
                failure: ReadFailure::NotReady,
                reason: String::new(),
            };
            panels.accept_read(Side::Left, event, None).change
        };
        let mut panels = Panels::new(&SavedPanels::default());
        assert_eq!(fail(&mut panels, r"D:\"), ReadChange::Cleared);
        assert!(matches!(
            fail(&mut panels, r"E:\x"),
            ReadChange::Refused { .. }
        ));
        assert!(matches!(
            panels.panel(Side::Left).view(),
            View::NotReady { drive } if drive == r"D:\"
        ));

        let mut panels = Panels::new(&SavedPanels::default());
        let ask = panels.go(Side::Left, r"C:\a".to_owned(), None);
        let opened = ReadEvent::Opened {
            generation: ask.generation,
            folder: r"C:\a".to_owned(),
        };
        panels.accept_read(Side::Left, opened, None);
        panels.show_reading(Side::Left);
        assert!(matches!(
            fail(&mut panels, r"E:\x"),
            ReadChange::Refused { .. }
        ));
        assert!(matches!(panels.panel(Side::Left).view(), View::Reading));
    }

    /// A Go keeps the folder it leaves on screen until the listing replaces
    /// it, so the list never shows empty in between, and commands wait for
    /// the new rows (DESIGN.md: A panel).
    #[test]
    fn a_go_shows_the_old_rows_until_its_listing() {
        let mut panels = showing(r"C:\old", vec![file("keep.txt", 1, 0)]);
        let ask = panels.go(Side::Left, r"C:\new".to_owned(), None);
        let opened = ReadEvent::Opened {
            generation: ask.generation,
            folder: ask.path.clone(),
        };
        assert_eq!(
            panels.accept_read(Side::Left, opened, None).change,
            ReadChange::Opened
        );
        assert_eq!(names(&panels, Side::Left), ["..", "keep.txt"]);
        assert_eq!(panels.chosen_count(Side::Left), 0);
        complete(&mut panels, &ask, r"C:\new", vec![file("new.txt", 1, 0)]);
        assert_eq!(names(&panels, Side::Left), ["..", "new.txt"]);
    }

    /// `F5` and `F6` offer the other panel's folder as soon as it opens,
    /// not only once its listing, which takes long on a large folder or a
    /// share, has completed (#111).
    #[test]
    fn a_folder_still_read_is_the_destination() {
        let mut panels = Panels::new(&SavedPanels::default());
        let ask = panels.go(Side::Left, r"C:\a".to_owned(), None);
        assert_eq!(panels.panel(Side::Left).destination(), None);
        let opened = ReadEvent::Opened {
            generation: ask.generation,
            folder: r"C:\a".to_owned(),
        };
        panels.accept_read(Side::Left, opened, None);
        assert_eq!(panels.panel(Side::Left).destination(), Some(r"C:\a"));
    }

    /// A panel holds about 100 bytes for each entry (DESIGN.md: Memory budget):
    /// the entry itself, its row's index, and its name on the heap. A field
    /// that grows the entry would pass every other test.
    #[test]
    fn an_entry_fits_the_memory_budget() {
        assert!(size_of::<Entry>() <= 48, "{} bytes", size_of::<Entry>());
    }

    /// A change notification during a move to another folder reads
    /// nothing, because reading the old folder again would cancel the move.
    #[test]
    fn reading_again_waits_for_a_move_to_another_folder() {
        let mut panels = showing(r"C:\a", vec![]);
        let ask = panels.go(Side::Left, r"C:\b".to_owned(), None);
        assert_eq!(panels.read_again(Side::Left), None);
        complete(&mut panels, &ask, r"C:\b", vec![]);
        assert!(panels.read_again(Side::Left).is_some());
    }

    /// A panel closed before its first listing keeps its saved folder, so
    /// the next start shows it again.
    #[test]
    fn the_saved_folder_lasts_until_the_first_listing() {
        let saved = SavedPanels {
            left: SavedPanel {
                folder: Some(r"D:\x".to_owned()),
                ..SavedPanel::default()
            },
            ..SavedPanels::default()
        };
        let mut panels = Panels::new(&saved);
        let ask = panels.start_reading(Side::Left, || unreachable!("a saved folder"));
        assert_eq!(ask.expect("a folder panel reads").path, r"D:\x");
        assert_eq!(panels.saved().left.folder.as_deref(), Some(r"D:\x"));
    }

    /// The path box expands environment variables, joins a relative path to
    /// the panel's folder, and leaves a `%` that names nothing as typed.
    #[test]
    fn a_typed_path_is_a_full_path() {
        let var = |name: &str| {
            (name.eq_ignore_ascii_case("USERPROFILE")).then(|| r"C:\Users\me".to_owned())
        };
        assert_eq!(
            typed_path(r"%USERPROFILE%\Documents", r"D:\x", var).as_deref(),
            Some(r"C:\Users\me\Documents")
        );
        assert_eq!(
            typed_path(r"..\Other", r"D:\x\y", var).as_deref(),
            Some(r"D:\x\Other")
        );
        assert_eq!(
            typed_path(r"100%\a", r"D:\", var).as_deref(),
            Some(r"D:\100%\a")
        );
        assert_eq!(typed_path("  ", r"D:\", var), None);
    }

    /// A typed path that names a file shows its folder with the file
    /// focused.
    #[test]
    fn a_file_path_focuses_the_file() {
        let mut panels = showing(r"C:\", vec![]);
        let ask = panels
            .go_typed(Side::Left, r"D:\x\b.txt", |_| None)
            .expect("a path");
        let event = ReadEvent::Listed {
            generation: ask.generation,
            folder: r"D:\x".to_owned(),
            entries: vec![file("a.txt", 1, 0), file("b.txt", 1, 0)],
            focus: Some(wide("b.txt")),
        };
        let change = panels.accept_read(Side::Left, event, None).change;
        assert_eq!(
            change,
            ReadChange::Listed {
                place: Place {
                    focused: Some(2),
                    selected: Arc::default(),
                },
                again: false,
                stale: false,
            }
        );
    }

    fn end(panels: &mut Panels, operations: &[(OperationId, Arc<Request>)]) -> Vec<PanelFollow> {
        panels.operations_ended(
            Arc::new(super::super::search_list::Ended {
                operations: operations.to_vec(),
                in_queue: HashSet::new(),
                taken: Arc::clone(panels.taken()),
            }),
            |id| operations.iter().any(|(ended, _)| *ended == id),
        )
    }

    #[test]
    fn a_listing_after_a_received_end_cannot_revive_an_awaited_name() {
        let mut panels = showing(r"C:\f", vec![file("a.txt", 1, 0)]);
        let id = OperationId::for_tests(53);
        panels.await_name(Side::Left, "created", None, false, id);
        let receipt = panels.observe_operations();
        let ask = panels.read_again(Side::Left).expect("a folder shows");
        let event = ReadEvent::Listed {
            generation: ask.generation,
            folder: r"C:\f".to_owned(),
            entries: vec![file("a.txt", 1, 0)],
            focus: None,
        };
        assert_eq!(
            panels.accept_read(Side::Left, event, None).change,
            ReadChange::Pending
        );
        assert!(panels.take_calculation(Side::Left).is_none());
        let ended = Arc::new(super::super::search_list::Ended {
            operations: vec![(
                id,
                Arc::new(Request::NewFolder {
                    path: PathBuf::from(r"C:\f\created"),
                }),
            )],
            in_queue: HashSet::new(),
            taken: Arc::clone(panels.taken()),
        });
        panels.accept_operations(receipt, Some(ended), |ended| ended == id);
        let input = panels
            .take_calculation(Side::Left)
            .expect("the listing waits for the end evidence");
        panels
            .accept_calculation(Side::Left, input.run())
            .expect("the post-end listing is current");
        assert!(!panels.panel(Side::Left).is_calculating());
        assert!(panels.panel(Side::Left).awaited.is_none());
        panels.apply_changes(
            Side::Left,
            vec![FolderChange::Added(folder("created"))],
            None,
        );
        assert_eq!(panels.show_awaited(Side::Left), None);
    }

    /// gozlerd's sizes match the rows by name without regard to case; a
    /// folder it does not name stays unknown.
    #[test]
    fn folder_sizes_match_rows_by_name() {
        let mut panels = showing(
            r"C:\d",
            vec![folder("Photos"), folder("Old"), file("a", 5, 0)],
        );
        give_sizes(&mut panels, &[(r"C:\d\PHOTOS", 40)]);
        let panel = panels.panel(Side::Left);
        let sizes: Vec<Option<u64>> = (1..panel.count())
            .map(|row| panel.entry(row).expect("an entry").folder_size())
            .collect();
        // Old, Photos, a.
        assert_eq!(sizes, [None, Some(40), None]);
    }

    /// The folder sizes of the left panel, by row.
    fn folder_sizes(panels: &Panels) -> Vec<Option<u64>> {
        let panel = panels.panel(Side::Left);
        (1..panel.count())
            .map(|row| panel.entry(row).expect("an entry").folder_size())
            .collect()
    }

    #[test]
    fn sizes_by_size_come_with_their_order() {
        let entries = || vec![folder("a"), folder("b"), file("f", 1, 0)];
        let mut panels = showing(r"C:\d", entries());
        let sort = SortSpec {
            key: SortKey::Size,
            dir: SortDir::Desc,
        };
        let ask = panels.sort(Side::Left, sort).expect("a folder shows");
        complete(&mut panels, &ask, r"C:\d", entries());
        panels.set_focused(Side::Left, Some(1));
        give_sizes(&mut panels, &[(r"C:\d\a", 10), (r"C:\d\b", 90)]);
        let update = panels
            .take_update(Side::Left)
            .expect("sizes and order publish together");
        assert_eq!(update.place.focused, Some(2));
        assert_eq!(names(&panels, Side::Left), ["..", "b", "a", "f"]);
        assert_eq!(folder_sizes(&panels), [Some(90), Some(10), None]);

        panels.apply_changes(Side::Left, vec![FolderChange::Added(folder("c"))], None);
        assert_eq!(names(&panels, Side::Left), ["..", "b", "a", "c", "f"]);
    }

    /// Reading the shown folder again keeps the folder sizes the panel
    /// knows, which the reading thread gives the listing before it sorts;
    /// another folder gets none.
    #[test]
    fn reading_again_keeps_the_folder_sizes() {
        let entries = || vec![folder("Photos"), folder("Old")];
        let mut panels = showing(r"C:\d", entries());
        give_sizes(&mut panels, &[(r"C:\d\Photos", 40)]);
        let ask = panels.read_again(Side::Left).expect("a folder shows");
        complete(&mut panels, &ask, r"C:\d", entries());
        // Old, Photos.
        assert_eq!(folder_sizes(&panels), [None, Some(40)]);
        let ask = panels.go(Side::Left, r"C:\e".to_owned(), None);
        assert!(ask.sizes.values().is_empty());
    }

    /// Asks for the left panel's folder sizes, and answers with gozlerd's
    /// `rows`, each a folder's path and size.
    fn give_sizes(panels: &mut Panels, rows: &[(&str, u64)]) {
        let ask = panels.ask_sizes(Side::Left, Generation(7));
        assert!(matches!(ask, Some(Ask::FolderSizes(Side::Left, _))));
        let rows = rows
            .iter()
            .map(|(path, size)| gozler_core::proto::ResultItem {
                path: (*path).to_owned(),
                path_u16: None,
                size: Some(*size),
                mtime_ft: None,
                created_ft: None,
                is_dir: true,
                attributes: None,
            })
            .collect();
        let chunk = super::super::fetch::Chunk {
            start: 0,
            rows,
            total: 1,
            volumes_incomplete: false,
            metadata_pending: false,
            sums: None,
            rest_follows: false,
            made: None,
        };
        assert!(panels.accept_sizes(Side::Left, Generation(7), Outcome::Rows(chunk)));
    }

    /// An empty folder's status line says "0 items" without a sum.
    #[test]
    fn an_empty_folder_counts_no_bytes() {
        let panels = showing(r"C:\e", vec![]);
        assert_eq!(
            panels.panel(Side::Left).counts(),
            Counts {
                items: 0,
                selected: None,
                size: None,
            }
        );
    }

    /// Until a folder's first sizes arrive, its status line shows no sum,
    /// which would flash "(5 bytes, 1 unknown)" on each move into a folder
    /// with a subfolder (#114); once they did, a later ask, as after a
    /// change, keeps the sum.
    #[test]
    fn the_first_size_wait_ends_only_on_its_reply_or_its_own_expiry() {
        let mut panels = showing(r"C:\d", vec![folder("Photos"), file("a", 5, 0)]);
        panels.ask_sizes(Side::Left, Generation(6));
        assert_eq!(panels.panel(Side::Left).counts().size, None);
        assert!(!panels.expire_sizes(Side::Left, Generation(5)));
        assert_eq!(panels.panel(Side::Left).counts().size, None);
        assert!(panels.expire_sizes(Side::Left, Generation(6)));
        assert_eq!(
            panels
                .panel(Side::Left)
                .counts()
                .size
                .map(|size| size.unknown),
            Some(1)
        );
        give_sizes(&mut panels, &[]);
        panels.ask_sizes(Side::Left, Generation(8));
        let size = panels.panel(Side::Left).counts().size;
        assert_eq!(size.map(|size| size.unknown), Some(1));
    }

    /// The status line sums the known sizes of the counted entries, and
    /// counts the folders whose size is unknown beside the sum.
    #[test]
    fn the_sum_counts_an_unknown_folder() {
        let mut panels = showing(
            r"C:\d",
            vec![folder("Photos"), folder("Old"), file("a", 5, 0)],
        );
        give_sizes(&mut panels, &[(r"C:\d\Photos", 100)]);
        // `..`, Old, Photos, a.
        assert_eq!(
            panels.panel(Side::Left).counts(),
            Counts {
                items: 3,
                selected: None,
                size: Some(Totals {
                    bytes: 105,
                    unknown: 1,
                }),
            }
        );
        panels.set_selected(Side::Left, vec![2..4].into());
        assert_eq!(
            panels.panel(Side::Left).counts(),
            Counts {
                items: 3,
                selected: Some(2),
                size: Some(Totals {
                    bytes: 105,
                    unknown: 0,
                }),
            }
        );
    }

    /// `Esc` and `Backspace` in a search panel must show the searched folder
    /// again with the row that was focused (DESIGN.md: Search in a panel).
    #[test]
    fn leaving_a_search_shows_the_folder_with_its_row_focused() {
        let mut panels = Panels::new(&SavedPanels::default());
        let entries = || vec![file("a.txt", 1, 0), file("b.txt", 1, 0)];
        list(&mut panels, r"C:\a", entries());
        // `..` is row 0.
        panels.set_focused(Side::Left, Some(2));
        assert!(panels.enter_search(Side::Left));
        let ask = panels.leave_search(Side::Left).expect("the panel searches");
        assert_eq!(ask.path, r"C:\a");
        let opened = ReadEvent::Opened {
            generation: ask.generation,
            folder: ask.path.clone(),
        };
        panels.accept_read(Side::Left, opened, None);
        let place = complete(&mut panels, &ask, r"C:\a", entries());
        assert!(matches!(panels.panel(Side::Left).view(), View::Folder));
        assert_eq!(place.focused, Some(2));
    }

    /// T3: the first `Esc` must never drop a search whose results are
    /// selected.
    #[test]
    fn the_first_esc_clears_the_selection_and_keeps_the_search() {
        let mut panels = Panels::new(&SavedPanels::default());
        list(&mut panels, r"C:\a", Vec::new());
        panels.enter_search(Side::Left);
        assert_eq!(panels.leave_on_escape(Side::Left, true), None);
        assert!(panels.leave_on_escape(Side::Left, false).is_some());
    }

    /// T7: a drive's root ends in a separator, which shifts the count of
    /// components by one, and a malformed reply must not slice past the end.
    #[test]
    fn the_folder_cell_is_the_path_below_the_searched_folder() {
        let searched = r"C:\Users\me";
        assert_eq!(
            relative_folder(searched, r"C:\Users\me\Projects\Accounts\a.txt"),
            r"Projects\Accounts"
        );
        assert_eq!(relative_folder(searched, r"C:\Users\me\a.txt"), "");
        assert_eq!(relative_folder(r"C:\", r"C:\a\b.txt"), "a");
        assert_eq!(relative_folder(searched, r"C:\x.txt"), "C:");
    }
}
