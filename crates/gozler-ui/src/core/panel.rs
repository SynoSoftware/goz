//! The panels of Two panels (DESIGN.md: A panel): what each panel's list shows,
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
//! - **Taken entries** (fast-copy-delete-spec.md: Taken entries): a panel
//!   keeps an ended delete or move whose end makes it read its folder
//!   again, until the first complete listing that it started after the end,
//!   because the change notifications and the end arrive in no fixed order.
//!   A moment counter orders the ends and the starts of the readings.

use super::fetch::{Ask, ChunkAsk, FromSnapshot, Generation, Outcome};
use super::model::Column;
use super::query::{self, Filters, Matching};
use super::search_list::{Deselect, SearchList, is_taken};
use super::selection::{Selection, wide_path};
use gozler_core::fold::{fold, fold_into};
use gozler_core::proto::ResultItem;
use gozler_core::query::QueryStart;
use gozler_core::types::{SortDir, SortKey, SortSpec};
use gozler_core::winpath::{absolute, is_root, parent_of};
use gozler_core::wtf8;
use gozler_fileops::{OperationId, Request, Taken};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hasher};
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

/// How many folders a panel's drive menu offers (DESIGN.md: The drive menu).
pub(crate) const HISTORY: usize = 10;

/// The most subfolders whose sizes one request asks gozlerd for.
const SIZE_ROWS: u64 = 100_000;

/// The most entries a batch of changes inserts one by one
/// ([`Panel::insert_sorted`]); each insert moves the entries after it.
const INSERT_IN_PLACE: usize = 8;

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
    /// Whether the list selected it, while a change moves the rows
    /// ([`Panel::remember`]).
    selected: bool,
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
            selected: false,
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

/// A name in its folded form, which the name sort and every name
/// comparison use, as Search's name sort does.
fn name_key(name: &[u16]) -> Vec<u8> {
    Folding::default().of(name).to_vec()
}

fn name_hash(key: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write(key);
    hasher.finish()
}

/// Whether `a` and `b` are the same name without regard to case; folds
/// them only when they differ as written.
fn same_name(a: &[u16], b: &[u16]) -> bool {
    a == b || name_key(a) == name_key(b)
}

/// The one of `candidates` whose name is `name` exactly, else one whose
/// name differs only in case, because a case-sensitive folder can hold
/// both.
fn best_match<'c, T>(
    candidates: &'c [T],
    name: &[u16],
    name_of: impl Fn(&T) -> &[u16],
) -> Option<&'c T> {
    (candidates.iter().find(|c| name_of(c) == name))
        .or_else(|| candidates.iter().find(|c| same_name(name_of(c), name)))
}

/// A name the window looks for among the entries.
#[derive(Clone, Debug)]
struct Name {
    hash: u64,
    raw: Box<[u16]>,
}

impl Name {
    fn of(raw: Box<[u16]>) -> Name {
        Name {
            hash: name_hash(&name_key(&raw)),
            raw,
        }
    }
}

/// Buffers that fold one name after another without an allocation for
/// each.
#[derive(Default)]
struct Folding {
    bytes: Vec<u8>,
    folded: Vec<u8>,
}

impl Folding {
    fn of(&mut self, name: &[u16]) -> &[u8] {
        self.bytes.clear();
        wtf8::from_utf16(name, &mut self.bytes);
        self.folded.clear();
        fold_into(&self.bytes, &mut self.folded);
        &self.folded
    }

    fn of_str(&mut self, name: &str) -> &[u8] {
        self.folded.clear();
        fold_into(name.as_bytes(), &mut self.folded);
        &self.folded
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
    pub(crate) selected: Vec<Range<u64>>,
}

/// What a panel's list area shows (DESIGN.md: A panel).
#[derive(Debug)]
pub(crate) enum View {
    /// The folder is open and its listing has not completed.
    Reading,
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

/// An entry that a file command acts on, in a folder panel or a search
/// panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChosenEntry {
    /// The full path as Windows names it.
    pub(crate) path: Vec<u16>,
    pub(crate) is_folder: bool,
}

impl ChosenEntry {
    fn of_result(item: &ResultItem) -> ChosenEntry {
        ChosenEntry {
            path: wide_path(item),
            is_folder: item.is_dir,
        }
    }
}

/// What a folder panel does after operations ended
/// ([`Panels::operations_ended`]).
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
    /// A folder panel's rows moved; the list shows this place.
    Folder(Place),
    /// A search panel's taken rows show blank in place, and its list
    /// unhighlights the rows `deselect` says.
    Search { deselect: Deselect },
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
    pub(crate) sizes: Vec<(Box<[u16]>, u64)>,
}

/// Sorts a listing for a panel sorted by `sort`, as its rows show:
/// folders first, then each group by the sort. The folded names it
/// compares live only while it sorts.
pub(crate) fn sort_listing(entries: &mut [Entry], sort: SortSpec) {
    let order = sorted_order(entries, sort);
    apply_order(entries, order);
}

/// The order that sorts `entries` by `sort`: the index of the entry that
/// goes first, then the next.
fn sorted_order(entries: &[Entry], sort: SortSpec) -> Vec<usize> {
    let keys: Vec<Vec<u8>> = entries.iter().map(|entry| name_key(&entry.name)).collect();
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by(|&a, &b| compare(sort, &entries[a], &keys[a], &entries[b], &keys[b]));
    order
}

/// Moves the entry `order[i]` to `i`, one cycle of the order at a time, so
/// each entry moves once.
fn apply_order(entries: &mut [Entry], mut order: Vec<usize>) {
    for start in 0..order.len() {
        let mut at = start;
        while order[at] != at {
            let next = order[at];
            order[at] = at;
            if next == start {
                break;
            }
            entries.swap(at, next);
            at = next;
        }
    }
}

/// Gives each folder of a listing its size from `sizes`, matched by name
/// as [`Panels::accept_sizes`] matches gozlerd's.
pub(crate) fn give_sizes(entries: &mut [Entry], sizes: &[(Box<[u16]>, u64)]) {
    if sizes.is_empty() {
        return;
    }
    // By the hash of the folded name.
    let mut by_hash = HashMap::<u64, Vec<_>>::new();
    for named in sizes {
        by_hash
            .entry(name_hash(&name_key(&named.0)))
            .or_default()
            .push(named);
    }
    for entry in entries.iter_mut().filter(|e| e.is_folder()) {
        let size = (by_hash.get(&entry.hash))
            .and_then(|named| best_match(named, &entry.name, |(name, _)| name))
            .map(|(_, size)| *size);
        entry.set_folder_size(size);
    }
}

/// gozlerd's folder sizes for a panel sorted by size, which a thread sorts
/// the folders by before the panel shows them, because sorting a folder of
/// many subfolders would hold the window ([`Panels::take_folder_sort`]).
#[derive(Debug)]
pub(crate) struct FolderSort {
    /// The panel's folder, whose subfolders they are.
    folder: String,
    /// The sort the panel's entries are in.
    sort: SortSpec,
    /// Copies of the panel's folders, in their order, with the new sizes.
    folders: Vec<Entry>,
}

impl FolderSort {
    /// Sorts the folders, on the thread that runs it.
    pub(crate) fn sort(self) -> SortedFolders {
        let order = sorted_order(&self.folders, self.sort);
        SortedFolders {
            folder: self.folder,
            sort: self.sort,
            folders: self.folders,
            order,
        }
    }
}

/// A [`FolderSort`]'s answer, which [`Panels::accept_folder_order`]
/// applies.
#[derive(Debug)]
pub(crate) struct SortedFolders {
    folder: String,
    sort: SortSpec,
    folders: Vec<Entry>,
    order: Vec<usize>,
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
    /// An older reading, which changes nothing.
    Stale,
    /// The rows went: a new folder opened, and the list says "Reading…",
    /// or the shown drive is not ready.
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

/// What `Enter` on a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EnterAction {
    Read(ReadAsk),
    /// Opens the file with Settings' "Open files" command.
    Open(String),
    Nothing,
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
    /// Sorted by [`compare`] with `order`.
    entries: Vec<Entry>,
    /// The sort `entries` are in: the sort of the listing that gave them,
    /// which differs from `sort` while a listing in the new sort is read.
    order: SortSpec,
    totals: Totals,
    /// The indexes of the entries that the list shows, in order, while a
    /// delete or a move takes some of them; `None` when all show.
    shown: Option<Vec<u32>>,
    /// The sort the person chose, which the header shows.
    sort: SortSpec,
    history: Vec<String>,
    /// The folder the panel last showed on each drive in this session, by
    /// the drive's folded root.
    drive_folders: HashMap<Vec<u8>, String>,
    /// The ended operations that still take rows from this panel, with the
    /// moment their end was seen.
    kept: HashMap<OperationId, u64>,
    read: Option<Read>,
    sizes_asked: Option<Generation>,
    /// gozlerd answered a sizes request since the list was emptied, so a
    /// folder without a size is unknown, not yet asked.
    sized: bool,
    free: Option<u64>,
    /// The quick search box's text while it is open.
    quick: Option<String>,
    /// The list's selected rows as it last told them, which the status
    /// line counts and Clear selection follows. Each selected entry also
    /// carries the mark, which stays with it when a change moves the rows.
    selected: Vec<Range<u64>>,
    /// The saved folder, which the first reading shows.
    saved: Option<String>,
    /// gozlerd's sizes for a panel sorted by size, which wait for a thread
    /// to sort them.
    folder_sort: Option<FolderSort>,
    /// The focused name that a change took away, while the focus shows on
    /// a neighbouring row ([`Panels::apply_changes`]).
    gone_focus: Option<GoneFocus>,
    /// The list's focused row as it last told it, which the commands act
    /// on when no entry is selected.
    focused: Option<u64>,
    /// The name that Gozler's own New folder or rename gives an entry in
    /// the shown folder, which the focus goes to when a row shows it.
    awaited: Option<Awaited>,
    /// The name hashes of the taken entries in the shown folder, which
    /// [`Panel::filter`] keeps from one change batch to the next.
    taken_hashes: Option<TakenHashes>,
}

/// The name hashes of the taken paths in one folder, for one snapshot of
/// the taken entries.
#[derive(Debug)]
struct TakenHashes {
    taken: Arc<Taken>,
    folder: String,
    hashes: HashSet<u64>,
}

/// A name that an operation of this window creates in the shown folder
/// ([`Panels::await_name`]).
#[derive(Debug)]
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
}

/// A focused name that a change took away, to which the focus returns when
/// a later change brings it back, as an editor's save through a temporary
/// name does: the focus and the selection stay on the same names.
#[derive(Debug)]
struct GoneFocus {
    name: Name,
    /// The row the focus was put on; a list focused on another row means
    /// the person moved the focus, which forgets the name.
    row: Option<u64>,
}

impl Panel {
    fn new(saved: &SavedPanel) -> Panel {
        Panel {
            folder: String::new(),
            view: View::Reading,
            entries: Vec::new(),
            order: saved.sort,
            totals: Totals::default(),
            shown: None,
            sort: saved.sort,
            history: saved.history.clone(),
            drive_folders: HashMap::new(),
            kept: HashMap::new(),
            read: None,
            sizes_asked: None,
            sized: false,
            free: None,
            quick: None,
            selected: Vec::new(),
            saved: saved.folder.clone(),
            folder_sort: None,
            gone_focus: None,
            focused: None,
            awaited: None,
            taken_hashes: None,
        }
    }

    pub(crate) fn folder(&self) -> &str {
        &self.folder
    }

    pub(crate) fn view(&self) -> &View {
        &self.view
    }

    /// The folder that `F5` and `F6` offer to copy or move to when this is
    /// the other panel: the open folder, also while it is still read; none
    /// for a search, a drive that is not ready or a folder not yet open.
    pub(crate) fn destination(&self) -> Option<&str> {
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
        self.quick.as_deref()
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
            View::Reading | View::Folder | View::NotReady { .. } => None,
        }
    }

    fn search_mut(&mut self) -> Option<&mut PanelSearch> {
        match &mut self.view {
            View::Search(search) => Some(search),
            View::Reading | View::Folder | View::NotReady { .. } => None,
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

    /// The entries the list shows.
    fn shown_len(&self) -> usize {
        self.shown.as_ref().map_or(self.entries.len(), Vec::len)
    }

    /// The index in `entries` of the `at`th shown entry.
    fn shown_at(&self, at: usize) -> Option<usize> {
        match &self.shown {
            None => (at < self.entries.len()).then_some(at),
            Some(shown) => shown.get(at).map(|&i| i as usize),
        }
    }

    /// The number of rows the list shows.
    pub(crate) fn count(&self) -> u64 {
        match &self.view {
            View::Folder => self.up_rows() + self.shown_len() as u64,
            View::Search(search) => search.list.results().count(),
            View::Reading | View::NotReady { .. } => 0,
        }
    }

    /// The folder row at `index`; [`Panels::row`] gives a search panel's.
    fn row(&self, index: u64) -> Option<Row<'_>> {
        if !matches!(self.view, View::Folder) {
            return None;
        }
        if self.has_up() && index == 0 {
            return Some(Row::Up);
        }
        let at = usize::try_from(index - self.up_rows()).ok()?;
        Some(Row::Entry(&self.entries[self.shown_at(at)?]))
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

    /// The row that holds `name`: the one with exactly that name, else one
    /// whose name differs only in case, because a case-sensitive folder can
    /// hold both. `near` limits the search to a row and how far from it the
    /// name's row may have moved, so a change costs no pass over a large
    /// folder; `None` searches every row.
    fn row_of(&self, name: &Name, near: Option<(u64, u64)>) -> Option<u64> {
        let all = self.up_rows()..self.count();
        let rows = match near {
            Some((row, reach)) => {
                row.saturating_sub(reach).max(all.start)..(row + reach + 1).min(all.end)
            }
            None => all,
        };
        let exact = |row: &u64| {
            self.entry(*row)
                .is_some_and(|e| e.hash == name.hash && e.name == name.raw)
        };
        rows.clone().find(exact).or_else(|| {
            rows.into_iter()
                .find(|row| self.entry(*row).is_some_and(|e| e.is(name)))
        })
    }

    /// The row with exactly `name`, as an operation of this window
    /// creates it.
    /// The row that shows `awaited`'s name, once no row shows the entry it
    /// renames.
    fn awaited_row(&self, awaited: &Awaited) -> Option<u64> {
        let renamed = (awaited.from.as_ref()).is_some_and(|from| self.row_named(from).is_some());
        self.row_named(&awaited.name).filter(|_| !renamed)
    }

    fn row_named(&self, name: &Name) -> Option<u64> {
        (self.up_rows()..self.count()).find(|row| {
            self.entry(*row)
                .is_some_and(|e| e.hash == name.hash && e.name == name.raw)
        })
    }

    /// Whether an entry is selected.
    pub(crate) fn has_selection(&self) -> bool {
        !self.selected.is_empty()
    }

    /// How many rows the list selects, as it last told them.
    pub(crate) fn selected_rows(&self) -> u64 {
        self.selected
            .iter()
            .map(|range| range.end - range.start)
            .sum()
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
        let items = self.shown_len() as u64;
        let mut picked = 0;
        let chosen = Totals::of(
            (self.selected.iter().flat_map(|range| range.clone()))
                .filter_map(|row| self.entry(row))
                .inspect(|_| picked += 1),
        );
        let (selected, size) = if picked == 0 {
            let size = match &self.shown {
                None => self.totals,
                Some(shown) => Totals::of(shown.iter().map(|&i| &self.entries[i as usize])),
            };
            // An empty folder's line says "0 items" alone.
            (None, (items > 0).then_some(size))
        } else {
            (Some(picked), Some(chosen))
        };
        let waits = self.sizes_asked.is_some() && !self.sized;
        Counts {
            items,
            selected,
            size: size.filter(|size| size.unknown == 0 || !waits),
        }
    }

    /// The list's selected rows changed to `selected`: the marks move from
    /// the old rows' entries to the new ones', which costs the selection,
    /// not the folder.
    fn select(&mut self, selected: Vec<Range<u64>>) {
        // A search panel has no entries; its list holds its selection.
        if self.search().is_none() {
            let up = self.up_rows();
            for (ranges, mark) in [(&self.selected, false), (&selected, true)] {
                for row in ranges.iter().flat_map(Clone::clone) {
                    let at = row.checked_sub(up).and_then(|at| usize::try_from(at).ok());
                    if let Some(i) = at.and_then(|at| self.shown_at(at)) {
                        self.entries[i].selected = mark;
                    }
                }
            }
        }
        self.selected = selected;
    }

    /// The focused row by name, so it can be found again after the rows
    /// change; the selected entries carry their own marks
    /// ([`Panel::select`]).
    fn remember(&self, focused: Option<u64>) -> Remembered {
        let focused = focused.and_then(|row| match self.row(row)? {
            Row::Up => Some(Focus::Up),
            Row::Result(_) => None,
            Row::Entry(entry) => {
                // The rows after it, nearest first, for when it goes.
                let after = (row + 1..self.count())
                    .filter_map(|r| self.entry(r).map(Entry::name))
                    .take(64)
                    .collect();
                Some(Focus::Entry {
                    name: entry.name(),
                    after,
                    row,
                })
            }
        });
        Remembered { focused }
    }

    /// The rows of what `remembered` holds, now, which the list then shows.
    /// `reach` is how far a row may have moved, when the change says. The
    /// marks are looked for only when an entry was selected.
    fn place_of(&mut self, remembered: Remembered, reach: Option<u64>) -> Place {
        let count = self.count();
        let focused = match remembered.focused {
            None => None,
            Some(Focus::Up) => (count > 0).then_some(0),
            Some(Focus::Entry { name, after, row }) => {
                // A row moves at most `reach`, so a name of the rows after
                // it that is not near is gone. The focused name may be a
                // rename's new name, which can land anywhere.
                let near = reach.map(|reach| (row, reach));
                self.row_of(&name, near)
                    .or_else(|| near.and_then(|_| self.row_of(&name, None)))
                    .or_else(|| after.iter().find_map(|name| self.row_of(name, near)))
                    .or_else(|| count.checked_sub(1).map(|last| row.min(last)))
            }
        };
        if !self.selected.is_empty() {
            let up = self.up_rows();
            let selected: Vec<u64> = (0..self.shown_len())
                .filter(|&at| self.shown_at(at).is_some_and(|i| self.entries[i].selected))
                .map(|at| at as u64 + up)
                .collect();
            self.selected = ranges(&selected);
        }
        Place {
            focused,
            selected: self.selected.clone(),
        }
    }

    /// Empties the list, while it reads, its drive is not ready or it
    /// starts a search.
    fn clear(&mut self) {
        self.entries.clear();
        self.totals = Totals::default();
        self.shown = None;
        self.selected.clear();
        self.quick = None;
        self.gone_focus = None;
        // A search panel never filters, so it would keep a taken snapshot,
        // which holds every source of a large delete, until it leaves.
        self.taken_hashes = None;
        self.sized = false;
    }

    /// Recomputes which entries show, after the entries or the taken
    /// entries changed. Only a folder that a delete or a move takes, or
    /// one that holds a taken entry, looks at its entries, so a change in
    /// any other folder costs nothing. In a folder that holds one, only an
    /// entry whose name has a taken name's hash is looked up, by its name
    /// as the operation gave it, so a change batch folds no name; its path
    /// is compared only when the operation gave its name in another case.
    /// An entry it hides loses its selection mark, because the list's
    /// selection loses its row, and a mark kept would select it again,
    /// unasked, when it shows again.
    fn filter(&mut self, taken: &Arc<Taken>) {
        let kept = &self.kept;
        let keeps = |id| kept.contains_key(&id);
        if taken.is_empty() {
            self.shown = None;
            self.taken_hashes = None;
            return;
        }
        let folder: Vec<u16> = self.folder.encode_utf16().collect();
        let here = taken.in_folder(&folder);
        if here.is_none() {
            self.taken_hashes = None;
        }
        self.shown = if taken.contains(&folder, keeps) {
            for entry in &mut self.entries {
                entry.selected = false;
            }
            Some(Vec::new())
        } else if let Some(here) = here {
            let known = (self.taken_hashes.as_ref()).is_some_and(|known| {
                Arc::ptr_eq(&known.taken, taken) && known.folder == self.folder
            });
            if !known {
                let mut folding = Folding::default();
                self.taken_hashes = Some(TakenHashes {
                    taken: Arc::clone(taken),
                    folder: self.folder.clone(),
                    hashes: here
                        .names()
                        .map(|name| name_hash(folding.of(name)))
                        .collect(),
                });
            }
            let hashes = &self.taken_hashes.as_ref().expect("filled above").hashes;
            let mut path = folder.clone();
            if path.last() != Some(&u16::from(b'\\')) {
                path.push(u16::from(b'\\'));
            }
            let base = path.len();
            let mut shown = Vec::new();
            for (i, entry) in self.entries.iter_mut().enumerate() {
                if hashes.contains(&entry.hash) {
                    let hidden = here.takes(&entry.name, keeps).unwrap_or_else(|| {
                        path.truncate(base);
                        path.extend_from_slice(&entry.name);
                        taken.contains(&path, keeps)
                    });
                    if hidden {
                        entry.selected = false;
                        continue;
                    }
                }
                shown.push(i as u32);
            }
            Some(shown)
        } else {
            None
        };
    }

    /// Applies `changes` in their order: the entries of every changed name
    /// leave in one pass, which compares only the names whose hash matches
    /// a changed one, and those that are there now go in at their sorted
    /// places. An added name and a rename's new name are the names as
    /// stored on the disk, so each takes only the entry with exactly that
    /// name, and a case-sensitive folder that holds `Makefile` keeps it
    /// beside a new `makefile`. A name that went or changed takes the entry
    /// with exactly that name, else one whose name differs only in case; a
    /// changed entry keeps its stored name. A replaced folder keeps its
    /// size until gozlerd gives it again, and a replaced entry stays
    /// selected. The selection keeps its names, so a renamed entry is not
    /// selected: an editor that saves through a temporary name and keeps a
    /// backup would otherwise leave two entries selected where the person
    /// selected one, and the next Delete would take both. Returns whether a
    /// change named a short name (8.3) that no entry has, which only a new
    /// listing can place.
    fn merge(&mut self, changes: Vec<FolderChange>) -> bool {
        // The last state of each changed name, as Windows reported it.
        let mut last: HashMap<Box<[u16]>, Now> = HashMap::new();
        let mut gone_short = Vec::new();
        for change in changes {
            match change {
                FolderChange::Added(entry) => {
                    last.insert(entry.name.clone(), Now::Added(entry));
                }
                FolderChange::Modified(entry) => {
                    let name = entry.name.clone();
                    // A name added in this batch is still the stored one.
                    let now = match last.get(&name) {
                        Some(Now::Added(_)) => Now::Added(entry),
                        _ => Now::Modified(entry),
                    };
                    last.insert(name, now);
                }
                FolderChange::Gone(name) => {
                    if is_short_name(&name) {
                        gone_short.push(name.clone());
                    }
                    last.insert(name, Now::Gone);
                }
                FolderChange::Renamed { from, to } => {
                    if is_short_name(&from) {
                        gone_short.push(from.clone());
                    }
                    last.insert(from, Now::Gone);
                    last.insert(to.name.clone(), Now::Added(to));
                }
            }
        }
        // What each changed name's entry was: its folder size and its mark.
        let mut replaced: HashMap<Box<[u16]>, (Option<u64>, bool)> = HashMap::new();
        let mut totals = self.totals;
        // The changed names' hashes, sorted: a binary search for each entry
        // costs less than hashing its name.
        let mut hashes: Vec<u64> = last.keys().map(|name| name_hash(&name_key(name))).collect();
        hashes.sort_unstable();
        self.entries.retain(|entry| {
            if hashes.binary_search(&entry.hash).is_err() || !last.contains_key(&entry.name) {
                return true;
            }
            totals.remove(entry);
            replaced.insert(entry.name.clone(), (entry.folder_size(), entry.selected));
            false
        });
        // The names that went or changed and that no entry has exactly,
        // which then match without regard to case.
        let mut unmatched: HashMap<u64, Vec<&[u16]>> = HashMap::new();
        for (name, _) in last
            .iter()
            .filter(|(name, now)| !matches!(now, Now::Added(_)) && !replaced.contains_key(*name))
        {
            let hash = name_hash(&name_key(name));
            unmatched.entry(hash).or_default().push(name);
        }
        let mut folded: HashMap<Box<[u16]>, (Option<u64>, bool)> = HashMap::new();
        // The stored name of the entry each of them took.
        let mut stored = HashMap::new();
        if !unmatched.is_empty() {
            self.entries.retain(|entry| {
                // Each name takes one entry.
                let Some(name) = (unmatched.get(&entry.hash)).and_then(|names| {
                    names
                        .iter()
                        .find(|name| !folded.contains_key(**name) && same_name(name, &entry.name))
                }) else {
                    return true;
                };
                totals.remove(entry);
                folded.insert((*name).into(), (entry.folder_size(), entry.selected));
                stored.insert(Box::<[u16]>::from(*name), entry.name.clone());
                false
            });
        }
        drop(unmatched);
        replaced.extend(folded);
        let unplaced_short = gone_short.iter().any(|name| !replaced.contains_key(name));
        let mut added: Vec<(Vec<u8>, Entry)> = last
            .into_iter()
            .filter_map(|(name, now)| {
                let mut entry = match now {
                    Now::Gone => return None,
                    Now::Added(entry) => entry,
                    Now::Modified(mut entry) => {
                        if let Some(stored_name) = stored.remove(&name) {
                            entry.name = stored_name;
                        }
                        entry
                    }
                };
                if let Some(&(size, selected)) = replaced.get(&name) {
                    entry.set_folder_size(size);
                    entry.selected = selected;
                }
                totals.add(&entry);
                Some((name_key(&entry.name), entry))
            })
            .collect();
        self.totals = totals;
        let order = self.order;
        added.sort_by(|(a_key, a), (b_key, b)| compare(order, a, a_key, b, b_key));
        self.insert_sorted(added);
        unplaced_short
    }

    /// Puts `added`, which is sorted, at its sorted places among the
    /// entries. A binary search finds each place, so only the entries it
    /// probes are folded. A few entries go in where they belong; more
    /// rebuild the entries once, which moves each entry once instead of
    /// once for each entry that goes in before it.
    fn insert_sorted(&mut self, added: Vec<(Vec<u8>, Entry)>) {
        let order = self.order;
        let mut folding = Folding::default();
        let mut from = 0;
        let places: Vec<usize> = added
            .iter()
            .map(|(key, entry)| {
                from += self.entries[from..].partition_point(|old| {
                    compare(order, old, folding.of(&old.name), entry, key).is_lt()
                });
                from
            })
            .collect();
        if added.len() <= INSERT_IN_PLACE {
            // From the last, so the places before it stay where they are.
            for ((_, entry), at) in added.into_iter().zip(places).rev() {
                self.entries.insert(at, entry);
            }
            return;
        }
        let mut old = std::mem::take(&mut self.entries).into_iter();
        let mut merged = Vec::with_capacity(old.len() + added.len());
        let mut moved = 0;
        for ((_, entry), at) in added.into_iter().zip(places) {
            merged.extend(old.by_ref().take(at - moved));
            moved = at;
            merged.push(entry);
        }
        merged.extend(old);
        self.entries = merged;
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

    /// Releases the ended operations that ended before `started`, the
    /// moment the completed listing started. Returns whether it keeps one
    /// that ended after, whose rows only a newer listing can show.
    fn release(&mut self, started: u64) -> bool {
        self.kept.retain(|_, ended| *ended >= started);
        !self.kept.is_empty()
    }

    pub(crate) fn keeps(&self, id: OperationId) -> bool {
        self.kept.contains_key(&id)
    }
}

/// A changed name's last state in a batch, which decides how
/// [`Panel::merge`] matches it to an entry.
enum Now {
    Gone,
    Modified(Entry),
    /// Added, or a rename's new name.
    Added(Entry),
}

/// The focused row by name.
#[derive(Debug)]
enum Focus {
    Up,
    Entry {
        name: Name,
        /// The names of the rows after it, for when it goes.
        after: Vec<Name>,
        row: u64,
    },
}

/// The list's place while the rows change; the selected entries carry
/// their own mark.
#[derive(Debug)]
struct Remembered {
    focused: Option<Focus>,
}

/// The rows of `a` that are not in `b`, both sorted ranges that do not
/// touch, as such ranges.
pub(crate) fn difference(a: &[Range<u64>], b: &[Range<u64>]) -> Vec<Range<u64>> {
    let mut out = Vec::new();
    let mut b = b.iter().peekable();
    for range in a {
        let mut start = range.start;
        while start < range.end {
            while b.next_if(|cut| cut.end <= start).is_some() {}
            match b.peek() {
                Some(cut) if cut.start < range.end => {
                    if start < cut.start {
                        out.push(start..cut.start);
                    }
                    start = cut.end.max(start);
                }
                _ => {
                    out.push(start..range.end);
                    start = range.end;
                }
            }
        }
    }
    out
}

/// Whether `name` is a short (8.3) name, as a watch can report one: a `~`
/// and a digit, at most 8 and 3 characters, and no lowercase letter.
fn is_short_name(name: &[u16]) -> bool {
    let name = String::from_utf16_lossy(name);
    let (base, ext) = name.rsplit_once('.').unwrap_or((&name, ""));
    base.len() <= 8
        && ext.len() <= 3
        && base.contains('~')
        && base.chars().any(|c| c.is_ascii_digit())
        && !name.chars().any(char::is_lowercase)
}

/// Sorted rows as ranges that do not touch.
fn ranges(rows: &[u64]) -> Vec<Range<u64>> {
    let mut out: Vec<Range<u64>> = Vec::new();
    for &row in rows {
        match out.last_mut() {
            Some(last) if last.end == row => last.end = row + 1,
            Some(last) if last.end > row => {}
            _ => out.push(row..row + 1),
        }
    }
    out
}

/// The order of a panel's entries: folders first, then each group by
/// `sort`, and by name where the sort's key is equal. `a_key` and `b_key`
/// are their names' folded forms; two names that differ only in case, as
/// a case-sensitive folder holds, are ordered as written, so a merged
/// entry lands where a listing puts it.
fn compare(sort: SortSpec, a: &Entry, a_key: &[u8], b: &Entry, b_key: &[u8]) -> Ordering {
    let by_name = || a_key.cmp(b_key).then_with(|| a.name.cmp(&b.name));
    let directed = |ordering: Ordering| match sort.dir {
        SortDir::Asc => ordering,
        SortDir::Desc => ordering.reverse(),
    };
    b.is_folder()
        .cmp(&a.is_folder())
        .then_with(|| match sort.key {
            // A panel lists no date created, so it sorts by name.
            SortKey::Name | SortKey::Path | SortKey::DateCreated => directed(by_name()),
            SortKey::Extension => {
                directed(extension_key(a_key, a).cmp(extension_key(b_key, b))).then_with(by_name)
            }
            SortKey::Size => directed(a.counted_size().cmp(&b.counted_size())).then_with(by_name),
            SortKey::DateModified => directed(a.modified.cmp(&b.modified)).then_with(by_name),
        })
}

fn extension_key<'k>(key: &'k [u8], entry: &Entry) -> &'k [u8] {
    gozler_core::query::extension(key, entry.is_folder()).unwrap_or(b"")
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
        }
        | Request::Move {
            sources,
            destination,
        } => {
            lands(&destination.path())
                || matches!(request, Request::Move { .. }) && sources.iter().any(|s| takes(s))
        }
        Request::Delete { sources } => sources.iter().any(|s| takes(s)),
        Request::NewFolder { path } => lands(path),
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
        }
        | Request::Move {
            sources,
            destination,
        } => below(&destination.path()) || sources.iter().any(|s| below(s)),
        Request::Delete { sources } => sources.iter().any(|s| below(s)),
        Request::NewFolder { path } => below(path),
    }
}

/// How a key in a panel's list moves the focus (DESIGN.md: Moving around).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

/// The row the focus moves to from `focused`, in a list of `count` rows
/// that shows `page` rows at a time.
pub(crate) fn step(step: Step, focused: Option<u64>, count: u64, page: u64) -> Option<u64> {
    let last = count.checked_sub(1)?;
    let page = page.max(2) - 1;
    let Some(from) = focused else {
        return Some(match step {
            Step::End => last,
            _ => 0,
        });
    };
    Some(match step {
        Step::Up => from.saturating_sub(1),
        Step::Down => (from + 1).min(last),
        Step::PageUp => from.saturating_sub(page),
        Step::PageDown => (from + page).min(last),
        Step::Home => 0,
        Step::End => last,
    })
    .map(|row| row.min(last))
}

/// The rows that `Shift` with `step`, which moves the focus from `from` to
/// `to`, selects: each row the focus passes, as in Total Commander, so the
/// row it lands on is selected only when it cannot move on, or when `step`
/// goes to the first or last row, which `Shift+Home` and `Shift+End` select.
pub(crate) fn passed(step: Step, from: u64, to: u64, count: u64) -> Range<u64> {
    let to_edge = matches!(step, Step::Home | Step::End);
    match from.cmp(&to) {
        Ordering::Less if to_edge => from..to + 1,
        Ordering::Less => from..to,
        Ordering::Greater if to_edge => to..from + 1,
        Ordering::Greater => to + 1..from + 1,
        Ordering::Equal => from..(from + 1).min(count),
    }
}

/// How a list adds rows that `Shift` passed to the rows it selects, with
/// the fewest writes, because each write notifies the window
/// ([`add_selected`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AddSelected {
    /// Selects each added row.
    EachRow,
    /// Selects every row in one write, then clears each row that is not
    /// added and was not selected ([`clears_after_all`]); the list reads
    /// only its selected rows outside `span`, the added rows' span.
    AllThenClear { span: Range<u64> },
}

/// How a list of `count` rows adds `adding`, sorted ranges that do not
/// touch, to the rows it selects: Shift+End from the top of a large list
/// passes every row, which one write and a few clears select.
pub(crate) fn add_selected(adding: &[Range<u64>], count: u64) -> AddSelected {
    let (Some(first), Some(last)) = (adding.first(), adding.last()) else {
        return AddSelected::EachRow;
    };
    let span = first.start..last.end;
    let added: u64 = adding.iter().map(|range| range.end - range.start).sum();
    // Each row outside the span is read when selected or cleared when not,
    // and each row of the span that is not added is cleared.
    let after_all = 1 + count - (span.end - span.start) + (span.end - span.start - added);
    if after_all < added {
        AddSelected::AllThenClear { span }
    } else {
        AddSelected::EachRow
    }
}

/// The rows to clear after a list of `count` rows selected every row, so
/// that it selects `adding` and the rows `selected` it selected before.
#[allow(
    clippy::single_range_in_vec_init,
    reason = "a selection is a list of row ranges"
)]
pub(crate) fn clears_after_all(
    selected: &[Range<u64>],
    adding: &[Range<u64>],
    count: u64,
) -> Vec<Range<u64>> {
    difference(&difference(&[0..count], selected), adding)
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
            panel.quick = None;
            panel.awaited = None;
        }
        let sort = panel.sort;
        let sizes = if folder_key(&path) == folder_key(&panel.folder) {
            let folders = panel.entries.iter().filter(|e| e.is_folder());
            (folders.filter_map(|e| Some((e.name.clone(), e.folder_size()?)))).collect()
        } else {
            Vec::new()
        };
        panel.read = Some(Read {
            generation,
            kind,
            path: path.clone(),
            nearest,
            sort,
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

    /// The reading of `side` when Two panels shows: its folder again, or
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
        let going = matches!(
            &panel.read,
            Some(Read {
                kind: ReadKind::Go { .. },
                ..
            })
        );
        let folder = panel.folder.clone();
        if folder.is_empty() || going {
            return None;
        }
        let kind = match self.panel(side).view {
            View::Folder => ReadKind::Again,
            View::Reading | View::NotReady { .. } => ReadKind::Go {
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
        let folder = self.panel(side).folder.clone();
        if folder.is_empty() || is_root(&folder) {
            return None;
        }
        let name: Box<[u16]> = last_name(&folder).encode_utf16().collect();
        Some(self.go(side, parent_of(&folder), Some(name)))
    }

    /// `Enter` on `row` of `side`. A search panel's result goes to its
    /// folder with it focused, and a folder result into that folder
    /// (DESIGN.md: Search in a panel).
    pub(crate) fn enter(&mut self, side: Side, row: u64) -> EnterAction {
        let panel = self.panel(side);
        match self.row(side, row) {
            None => EnterAction::Nothing,
            Some(Row::Up) => self
                .go_up(side)
                .map_or(EnterAction::Nothing, EnterAction::Read),
            Some(Row::Entry(entry)) => {
                let path = panel.path_of(&entry.name);
                if entry.is_folder() {
                    EnterAction::Read(self.go(side, path, None))
                } else {
                    EnterAction::Open(path)
                }
            }
            Some(Row::Result(item)) => {
                let (path, focus) = result_place(item);
                EnterAction::Read(self.go(side, path, focus))
            }
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
        panel.order = panel.sort;
        panel.kept.clear();
        panel.read = None;
        panel.sizes_asked = None;
        panel.folder_sort = None;
        panel.awaited = None;
        panel.focused = None;
        panel.view = View::Search(Box::new(PanelSearch {
            list: SearchList::new(),
            look_in: LookIn::ThisFolder,
            back,
        }));
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
    pub(crate) fn leave_on_escape(&mut self, side: Side) -> Option<ReadAsk> {
        if self.panel(side).has_selection() {
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
        let searched = self.search(side).is_some();
        let change = self.apply_read(side, event, focused);
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
            return ReadChange::Stale;
        };
        match event {
            ReadEvent::Opened { folder, .. } => match read.kind {
                ReadKind::Go { .. } => {
                    panel.folder = folder;
                    panel.view = View::Reading;
                    panel.clear();
                    panel.free = None;
                    ReadChange::Cleared
                }
                ReadKind::Again => ReadChange::Stale,
            },
            ReadEvent::Listed {
                folder,
                entries,
                focus,
                ..
            } => {
                panel.read = None;
                panel.gone_focus = None;
                let remembered = match (&read.kind, &panel.view) {
                    (ReadKind::Again, View::Folder)
                        if folder_key(&folder) == folder_key(&panel.folder) =>
                    {
                        Some(panel.remember(focused))
                    }
                    _ => None,
                };
                let focus = match read.kind {
                    ReadKind::Go { focus: wanted, .. } => focus.or(wanted),
                    ReadKind::Again => focus,
                };
                panel.folder = folder;
                panel.view = View::Folder;
                panel.saved = None;
                // The same folder keeps its selection, by exact name, because
                // both listings hold the names as stored on the disk; the
                // reading thread gave it the folder sizes the panel knew
                // (`ReadAsk::sizes`).
                let same = remembered.is_some();
                // By hash, each with the entries of that hash, whose names
                // are compared only when it matches.
                let mut old: HashMap<u64, Vec<Entry>> = HashMap::new();
                if same {
                    for entry in panel.entries.drain(..).filter(|e| e.selected) {
                        old.entry(entry.hash).or_default().push(entry);
                    }
                }
                panel.entries = entries;
                if !old.is_empty() {
                    for entry in &mut panel.entries {
                        entry.selected = (old.get(&entry.hash))
                            .is_some_and(|olds| olds.iter().any(|old| old.name == entry.name));
                    }
                }
                panel.totals = Totals::of(&panel.entries);
                // The rows stay where the person sees them only in the
                // order they had.
                let again = same && read.sort == panel.order;
                panel.order = read.sort;
                let stale = panel.release(read.generation);
                panel.filter(&taken);
                panel.note_folder();
                if let Some(awaited) = &panel.awaited
                    && awaited.ended.is_some_and(|ended| ended < read.generation)
                    && panel.awaited_row(awaited).is_none()
                {
                    panel.awaited = None;
                }
                let place = match remembered {
                    Some(remembered) => panel.place_of(remembered, None),
                    None => {
                        panel.selected.clear();
                        let focused = focus
                            .and_then(|name| panel.row_of(&Name::of(name), None))
                            .or_else(|| (panel.count() > 0).then_some(0));
                        Place {
                            focused,
                            selected: Vec::new(),
                        }
                    }
                };
                ReadChange::Listed {
                    place,
                    again,
                    stale,
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
                        panel.view = View::NotReady {
                            drive: drive_root(&read.path),
                        };
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
        panel.view = View::NotReady {
            drive: drive_root(&panel.folder),
        };
        panel.clear();
        searched
    }

    /// The list's selected rows, as it tells them; whether they changed.
    pub(crate) fn set_selected(&mut self, side: Side, selected: Vec<Range<u64>>) -> bool {
        let panel = self.panel_mut(side);
        let changed = panel.selected != selected;
        if changed {
            panel.select(selected);
        }
        changed
    }

    /// Whether the active panel has a selected entry.
    pub(crate) fn has_selection(&self) -> bool {
        self.panel(self.active).has_selection()
    }

    /// The list's focused row, as it tells it.
    pub(crate) fn set_focused(&mut self, side: Side, focused: Option<u64>) {
        self.panel_mut(side).focused = focused;
    }

    /// How many entries the active panel's file commands act on
    /// ([`Panels::chosen`]), without a pass over them: none while a search
    /// panel's selection is read.
    pub(crate) fn chosen_count(&self) -> usize {
        let side = self.active;
        let panel = self.panel(side);
        let focused = || usize::from(self.focused(side).is_some());
        match (panel.search(), panel.selected.as_slice()) {
            (None, []) => focused(),
            (None, selected) => selected.iter().map(|r| (r.end - r.start) as usize).sum(),
            (Some(search), selected) => match search.list.selection() {
                Selection::None if selected.is_empty() => focused(),
                selection => selection.files().map_or(0, <[_]>::len),
            },
        }
    }

    /// The entries a file command of `side` acts on: the selected ones,
    /// else the focused one; never `..` (DESIGN.md: Selecting entries). A
    /// search panel's selected results once they are read, and none while they
    /// are.
    pub(crate) fn chosen(&self, side: Side) -> Vec<ChosenEntry> {
        let panel = self.panel(side);
        match panel.search() {
            None if panel.selected.is_empty() => self.focused(side).into_iter().collect(),
            None => (panel.selected.iter())
                .flat_map(Clone::clone)
                .filter_map(|row| panel.chosen_entry(row))
                .collect(),
            Some(search) => match search.list.selection() {
                Selection::None if panel.selected.is_empty() => {
                    self.focused(side).into_iter().collect()
                }
                selection => (selection.files().unwrap_or_default().iter())
                    .map(ChosenEntry::of_result)
                    .collect(),
            },
        }
    }

    /// The focused entry of `side`, which View opens; never `..` or a blank
    /// row.
    pub(crate) fn focused(&self, side: Side) -> Option<ChosenEntry> {
        let panel = self.panel(side);
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
    /// The rows of `rows` that [`Panels::selectable`] says `side` can select,
    /// as sorted ranges that do not touch.
    pub(crate) fn selectable_ranges(&self, side: Side, rows: Range<u64>) -> Vec<Range<u64>> {
        let mut ranges: Vec<Range<u64>> = Vec::new();
        for row in rows.filter(|row| self.selectable(side, *row)) {
            match ranges.last_mut() {
                Some(last) if last.end == row => last.end = row + 1,
                _ => ranges.push(row..row + 1),
            }
        }
        ranges
    }

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
        let panel = self.panel_mut(side);
        let replaces = (from.as_ref()).is_some_and(|from| panel.row_named(from).is_some());
        panel.awaited = (replaces || panel.row_named(&name).is_none()).then_some(Awaited {
            name,
            select,
            from,
            operation,
            ended: None,
        });
    }

    /// The place that shows `side`'s awaited name, once a row shows it,
    /// after its list showed a change or a listing; it is then forgotten.
    /// `None` while no row shows it.
    pub(crate) fn show_awaited(&mut self, side: Side) -> Option<Place> {
        let panel = self.panel_mut(side);
        let awaited = panel.awaited.as_ref()?;
        let row = panel.awaited_row(awaited)?;
        let select = awaited.select;
        panel.awaited = None;
        panel.gone_focus = None;
        if select {
            let mut rows: Vec<u64> = panel.selected.iter().flat_map(Clone::clone).collect();
            rows.push(row);
            rows.sort_unstable();
            rows.dedup();
            panel.select(ranges(&rows));
        }
        Some(Place {
            focused: Some(row),
            selected: panel.selected.clone(),
        })
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
            return None;
        }
        // Each change moves a row by one at most.
        let reach = changes.len() as u64 + 1;
        let mut remembered = panel.remember(focused);
        let gone = (panel.gone_focus.take()).filter(|gone| gone.row == focused);
        let back = gone.as_ref().is_some_and(|gone| {
            changes.iter().any(|change| match change {
                FolderChange::Added(entry)
                | FolderChange::Modified(entry)
                | FolderChange::Renamed { to: entry, .. } => entry.name == gone.name.raw,
                FolderChange::Gone(_) => false,
            })
        });
        // When the name was the folder's only entry, the focus shows on
        // `..`, or on no row at a drive's root.
        if back && let Some(gone) = &gone {
            let name = gone.name.clone();
            remembered.focused = Some(match remembered.focused.take() {
                Some(Focus::Entry { after, row, .. }) => Focus::Entry { name, after, row },
                _ => Focus::Entry {
                    name,
                    after: Vec::new(),
                    row: focused.unwrap_or(0),
                },
            });
        }
        let wanted = match &remembered.focused {
            Some(Focus::Entry { name, .. }) => Some(name.clone()),
            _ => None,
        };
        let again = panel.merge(changes);
        panel.filter(&taken);
        let place = panel.place_of(remembered, Some(reach));
        let shows = |name: &Name| {
            (place.focused.and_then(|row| panel.entry(row))).is_some_and(|e| e.name == name.raw)
        };
        let gone_name = match wanted {
            Some(name) if !shows(&name) => Some(gone.map_or(name, |gone| gone.name)),
            _ if back => None,
            _ => gone.map(|gone| gone.name),
        };
        panel.gone_focus = gone_name.map(|name| GoneFocus {
            name,
            row: place.focused,
        });
        Some((place, again))
    }

    /// The folder sizes of `side` that wait to be sorted, which
    /// [`Panels::accept_sizes`] left for a panel sorted by size.
    pub(crate) fn take_folder_sort(&mut self, side: Side) -> Option<FolderSort> {
        self.panel_mut(side).folder_sort.take()
    }

    /// The folders of `side`, sorted by their new sizes: the sizes apply in
    /// that order, keeping the focus on `focused`'s name. Returns the new
    /// place; `None` when the shown folder, its folders or the sort changed
    /// since the sizes came, and then the change that did it asks gozlerd
    /// for the sizes again, so the panel's entries stay in their order.
    pub(crate) fn accept_folder_order(
        &mut self,
        side: Side,
        sorted: SortedFolders,
        focused: Option<u64>,
    ) -> Option<Place> {
        let taken = Arc::clone(&self.taken);
        let panel = self.panel_mut(side);
        let count = panel.entries.partition_point(Entry::is_folder);
        let folders = &panel.entries[..count];
        if folder_key(&panel.folder) != folder_key(&sorted.folder)
            || panel.order != sorted.sort
            || folders.len() != sorted.folders.len()
            || !folders
                .iter()
                .zip(&sorted.folders)
                .all(|(a, b)| a.name == b.name)
        {
            return None;
        }
        let remembered = panel.remember(focused);
        let folders = &mut panel.entries[..count];
        for (entry, sized) in folders.iter_mut().zip(&sorted.folders) {
            panel.totals.remove(entry);
            entry.set_folder_size(sized.folder_size());
            panel.totals.add(entry);
        }
        apply_order(folders, sorted.order);
        panel.filter(&taken);
        Some(panel.place_of(remembered, None))
    }

    /// The ended operations `ended`, each with its request, which each
    /// folder panel keeps when its end makes it read its folder again; a
    /// search panel's list decides for itself ([`SearchList::ended`]).
    /// Returns what each panel that they changed does.
    pub(crate) fn operations_ended(
        &mut self,
        ended: &[(OperationId, Arc<Request>)],
    ) -> Vec<PanelFollow> {
        self.moment += 1;
        let moment = self.moment;
        let mut out = Vec::new();
        for side in Side::BOTH {
            let panel = self.panel_mut(side);
            let mut again = false;
            let mut sizes = false;
            for (id, request) in ended {
                if let Some(awaited) = &mut panel.awaited
                    && awaited.operation == *id
                {
                    awaited.ended = Some(moment);
                }
                if panel.folder.is_empty() || panel.search().is_some() {
                    continue;
                }
                if ends_in(request, &panel.folder) {
                    panel.kept.insert(*id, moment);
                    again = true;
                }
                sizes |= changes_sizes(request, &panel.folder);
            }
            if again || sizes {
                out.push(PanelFollow {
                    side,
                    read_again: again,
                    sizes,
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
                let lost = search.list.drop_taken(&taken);
                if lost {
                    search.list.count_chosen_bytes();
                }
                return TakenRows::Search {
                    deselect: search.list.deselect(lost),
                };
            }
            let remembered = panel.remember(focused[side.index()]);
            panel.filter(&taken);
            TakenRows::Folder(panel.place_of(remembered, None))
        })
    }

    /// The request for `side`'s folder sizes: every direct subfolder of
    /// its folder, from gozlerd (DESIGN.md: Folder sizes).
    pub(crate) fn ask_sizes(&mut self, side: Side, generation: Generation) -> Option<Ask> {
        let panel = self.panel_mut(side);
        if !matches!(panel.view, View::Folder) || !panel.entries.iter().any(Entry::is_folder) {
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
    /// case, as NTFS compares names. A folder gozlerd does not know keeps
    /// no size. A panel sorted by size keeps its sizes until its folders
    /// are sorted by them ([`Panels::take_folder_sort`]), because a merge
    /// inserts by the order the entries are in.
    /// Whether it was the answer the panel waits for.
    pub(crate) fn accept_sizes(
        &mut self,
        side: Side,
        generation: Generation,
        outcome: Outcome,
    ) -> bool {
        let panel = self.panel_mut(side);
        if panel.sizes_asked != Some(generation) {
            return false;
        }
        panel.sizes_asked = None;
        panel.sized = true;
        let rows = match &outcome {
            Outcome::Rows(chunk) => chunk.rows.as_slice(),
            _ => &[],
        };
        // By the hash of the folded name, each with the names of that hash
        // as gozlerd wrote them.
        let mut sizes: HashMap<u64, Vec<(Vec<u16>, u64)>> = HashMap::new();
        for row in rows.iter().filter(|row| row.is_dir) {
            if let Some(size) = row.size {
                let name = last_name(&row.path);
                let hash = name_hash(&fold(name.as_bytes()));
                let entry = (name.encode_utf16().collect(), size);
                sizes.entry(hash).or_default().push(entry);
            }
        }
        let size_of = |entry: &Entry| {
            (sizes.get(&entry.hash))
                .and_then(|named| best_match(named, &entry.name, |(name, _)| name))
                .map(|(_, size)| *size)
        };
        if panel.order.key == SortKey::Size {
            let folders = panel.entries.iter().take_while(|e| e.is_folder());
            let folders = folders.map(|entry| {
                let mut sized = entry.clone();
                sized.set_folder_size(size_of(entry));
                sized
            });
            panel.folder_sort = Some(FolderSort {
                folder: panel.folder.clone(),
                sort: panel.order,
                folders: folders.collect(),
            });
            return true;
        }
        for entry in panel.entries.iter_mut().filter(|e| e.is_folder()) {
            let size = size_of(entry);
            entry.set_folder_size(size);
        }
        panel.totals = Totals::of(&panel.entries);
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

    /// A character typed in `side`'s list, which opens the quick search
    /// box or extends its text (DESIGN.md: Quick search). The focus goes to the
    /// first row whose name starts with the text; a character after which no
    /// name would match is not added. Returns the row to focus.
    pub(crate) fn quick_char(&mut self, side: Side, c: char) -> Option<u64> {
        let taken = Arc::clone(&self.taken);
        let panel = self.panel_mut(side);
        if !matches!(panel.view, View::Folder | View::Search(_)) {
            return None;
        }
        // A first character that matches nothing does not open the box.
        let mut wanted = panel.quick.clone().unwrap_or_default();
        wanted.push(c);
        let found = first_match(panel, &taken, &wanted, 0, true);
        if found.is_some() {
            panel.quick = Some(wanted);
        }
        found
    }

    /// `Backspace` in the quick search box: the text loses its last
    /// character and the focus goes to the first row that matches it.
    pub(crate) fn quick_back(&mut self, side: Side) -> Option<u64> {
        let taken = Arc::clone(&self.taken);
        let panel = self.panel_mut(side);
        let text = panel.quick.as_mut()?;
        text.pop();
        let text = text.clone();
        first_match(panel, &taken, &text, 0, true)
    }

    /// `Up` or `Down` in the quick search box: the previous or the next
    /// row that matches.
    pub(crate) fn quick_step(
        &mut self,
        side: Side,
        focused: Option<u64>,
        down: bool,
    ) -> Option<u64> {
        let panel = self.panel(side);
        let text = panel.quick.as_deref()?;
        let from = focused?;
        if down {
            first_match(panel, &self.taken, text, from + 1, true)
        } else {
            first_match(panel, &self.taken, text, from, false)
        }
    }

    /// Closes the quick search box; whether it was open.
    pub(crate) fn close_quick(&mut self, side: Side) -> bool {
        self.panel_mut(side).quick.take().is_some()
    }
}

/// The first row from `from` down, or the last before `from` up, whose
/// name starts with `text`, without regard to case. `..` never matches. A
/// search panel looks only at its fetched rows that no delete or move has
/// taken, so the walk never passes the rows the list cannot show
/// (DESIGN.md: Quick search).
fn first_match(panel: &Panel, taken: &Taken, text: &str, from: u64, down: bool) -> Option<u64> {
    let wanted = fold(text.as_bytes());
    let mut folding = Folding::default();
    if let Some(search) = panel.search() {
        let list = &search.list;
        return (list.results().fetched_from(from, down))
            .find(|(_, item)| {
                folding.of_str(last_name(&item.path)).starts_with(&wanted)
                    && !is_taken(taken, &list.kept, item)
            })
            .map(|(row, _)| row);
    }
    let matches = |row: &u64| {
        panel
            .entry(*row)
            .is_some_and(|entry| folding.of(&entry.name).starts_with(&wanted))
    };
    if down {
        (from..panel.count()).find(matches)
    } else {
        (0..from).rev().find(matches)
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

    fn names(panels: &Panels, side: Side) -> Vec<String> {
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
    fn showing(folder: &str, entries: Vec<Entry>) -> Panels {
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
            super::give_sizes(&mut entries, &ask.sizes);
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
            other => panic!("not listed: {other:?}"),
        }
    }

    fn delete(sources: &[&str]) -> Arc<Request> {
        Arc::new(Request::Delete {
            sources: sources.iter().map(PathBuf::from).collect(),
        })
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
            let again = panels.operations_ended(&[(id, delete(&[r"C:\f\a.txt", r"C:\f\b.txt"]))]);
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
        panels.operations_ended(&[(id, delete(&[r"C:\f\a.txt"]))]);
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
        panels.set_selected(Side::Left, vec![1..2]);
        let ask = panels.read_again(Side::Left).expect("a folder shows");
        let listed = vec![file("Makefile", 1, 0), file("makefile", 2, 0)];
        assert_eq!(
            complete(&mut panels, &ask, r"C:\src", listed).selected,
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
            panels.set_selected(Side::Left, vec![1..2]);
            let mut taken = Taken::default();
            taken.insert(OperationId::for_tests(1), Path::new(taken_path), false);
            panels.set_taken(Arc::new(taken), [None; 2]);
            panels.set_taken(Arc::default(), [None; 2]);
            assert_eq!(names(&panels, Side::Left), ["..", "a.txt", "b.txt"]);
            panels.set_selected(Side::Left, vec![2..3]);
            let changes = vec![FolderChange::Modified(file("b.txt", 2, 0))];
            let (place, _) = panels
                .apply_changes(Side::Left, changes, None)
                .expect("a folder shows");
            assert_eq!(place.selected, [2..3], "taken: {taken_path}");
        }
    }

    /// The arrow keys move only the focus; `Shift` selects the rows the
    /// focus passes, and the last row once the focus cannot move on;
    /// `Shift+End` and `Shift+Home` also select the row they land on.
    #[test]
    fn the_focus_moves_without_the_selection() {
        assert_eq!(step(Step::Down, Some(3), 10, 5), Some(4));
        assert_eq!(step(Step::PageDown, Some(3), 10, 5), Some(7));
        assert_eq!(step(Step::End, Some(3), 10, 5), Some(9));
        assert_eq!(step(Step::Up, None, 10, 5), Some(0));
        assert_eq!(step(Step::Down, Some(0), 0, 5), None);
        assert_eq!(passed(Step::Down, 3, 4, 10), 3..4);
        assert_eq!(passed(Step::Up, 4, 3, 10), 4..5);
        assert_eq!(passed(Step::Down, 9, 9, 10), 9..10);
        assert_eq!(passed(Step::End, 3, 9, 10), 3..10);
        assert_eq!(passed(Step::Home, 4, 0, 10), 0..5);
    }

    /// Shift+End from the top of 100,000 rows selects every row in one
    /// write instead of 99,999, and keeps the rows the person selected
    /// before the span; a short step still selects row by row.
    #[test]
    fn shift_across_most_rows_selects_every_row_then_clears() {
        let count = 100_000;
        let adding = [1..count];
        assert_eq!(
            add_selected(&adding, count),
            AddSelected::AllThenClear { span: 1..count }
        );
        assert_eq!(clears_after_all(&[], &adding, count), [0..1]);
        let span_inside = [10..20, 30..count];
        assert_eq!(
            clears_after_all(&[2..3], &span_inside, count),
            [0..2, 3..10, 20..30]
        );
        assert_eq!(add_selected(&[5..6], count), AddSelected::EachRow);
    }

    /// Quick search goes to the first name that starts with the text,
    /// refuses a character after which nothing matches, and `Up` and
    /// `Down` step between the matches.
    #[test]
    fn quick_search_goes_to_the_first_match() {
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
        assert_eq!(panels.quick_char(Side::Left, 'z'), None);
        assert_eq!(panels.panel(Side::Left).quick_text(), None);
        // `..`, notes, Report-draft, report, tax.
        assert_eq!(panels.quick_char(Side::Left, 'r'), Some(2));
        assert_eq!(panels.quick_char(Side::Left, 'E'), Some(2));
        assert_eq!(panels.quick_char(Side::Left, 'x'), None);
        assert_eq!(panels.panel(Side::Left).quick_text(), Some("rE"));
        assert_eq!(panels.quick_step(Side::Left, Some(2), true), Some(3));
        assert_eq!(panels.quick_step(Side::Left, Some(3), true), None);
        assert_eq!(panels.quick_step(Side::Left, Some(3), false), Some(2));
        assert_eq!(panels.quick_back(Side::Left), Some(2));
        assert_eq!(panels.quick_char(Side::Left, '.'), None);
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
        panels.set_selected(Side::Left, vec![1..2, 3..4]);
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
                selected: vec![1..2, 3..4],
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
        panels.set_selected(Side::Left, vec![1..3]);
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
                selected: vec![1..2],
            }
        );
        panels.set_selected(Side::Left, Vec::new());
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
        panels.set_selected(Side::Left, vec![1..2]);
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
        assert_eq!(place.selected, [2..3]);
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
        panels.set_selected(Side::Left, vec![1..2]);
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
        assert_eq!((place.focused, place.selected), (Some(2), vec![2..3]));
        assert_eq!(panels.show_awaited(Side::Left), None, "it is shown once");

        let id = OperationId::for_tests(2);
        panels.await_name(Side::Left, "d.txt", None, false, id);
        panels.operations_ended(&[(id, delete(&[r"C:\other\x"]))]);
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
        panels.set_selected(Side::Left, vec![1..2]);
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
        assert_eq!((place.focused, place.selected), (Some(1), vec![1..2]));

        let mut panels = showing(r"C:\f", entries.clone());
        let id = OperationId::for_tests(2);
        panels.await_name(Side::Left, "b.txt", Some("a.txt"), false, id);
        panels.operations_ended(&[(id, delete(&[r"C:\other\x"]))]);
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
        let mut panels = showing(r"C:\f", vec![folder("Photos"), file("x.txt", 1, 0)]);
        let shown = |panels: &Panels, name| panels.panel(Side::Left).shown_name(name);
        assert_eq!(shown(&panels, "PHOTOS").as_deref(), Some("Photos"));
        let mut taken = Taken::default();
        taken.insert(OperationId::for_tests(1), Path::new(r"C:\f\x.txt"), false);
        panels.set_taken(Arc::new(taken), [None; 2]);
        assert_eq!(shown(&panels, "x.txt"), None);
    }

    /// A change batch in a folder that a delete takes from finds the taken
    /// entries by the names the delete gave, and one given in another case
    /// by its path. The panel folds the taken names once for each snapshot,
    /// so a batch costs nothing per taken name.
    #[test]
    fn a_change_batch_keeps_the_taken_names_folded_once() {
        let mut panels = showing(
            r"C:\f",
            vec![
                file("a.txt", 1, 0),
                file("B.txt", 1, 0),
                file("c.txt", 1, 0),
            ],
        );
        let mut taken = Taken::default();
        taken.insert(OperationId::for_tests(1), Path::new(r"C:\f\a.txt"), false);
        taken.insert(OperationId::for_tests(1), Path::new(r"C:\f\b.txt"), false);
        panels.set_taken(Arc::new(taken), [None; 2]);
        assert_eq!(names(&panels, Side::Left), ["..", "c.txt"]);
        let folded = |panels: &Panels| {
            let hashes = panels.panel(Side::Left).taken_hashes.as_ref();
            hashes
                .expect("the folder holds taken entries")
                .folder
                .as_ptr()
        };
        let before = folded(&panels);
        let changes = vec![FolderChange::Added(file("d.txt", 1, 0))];
        panels.apply_changes(Side::Left, changes, None);
        assert_eq!(names(&panels, Side::Left), ["..", "c.txt", "d.txt"]);
        assert_eq!(folded(&panels), before);
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
        assert!(matches!(
            fail(&mut panels, r"E:\x"),
            ReadChange::Refused { .. }
        ));
        assert!(matches!(panels.panel(Side::Left).view(), View::Reading));
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

    /// More changes than go in one by one rebuild the entries, which must
    /// put each new entry at its sorted place among the old ones.
    #[test]
    fn a_large_batch_goes_in_sorted() {
        let old = ["b", "d", "f", "h"];
        let mut panels = showing(r"C:\f", old.map(|name| file(name, 1, 0)).to_vec());
        let new = ["a", "c", "e", "g", "i", "j", "k", "l", "m"];
        assert!(new.len() > INSERT_IN_PLACE);
        let changes = new
            .map(|name| FolderChange::Added(file(name, 1, 0)))
            .to_vec();
        panels.apply_changes(Side::Left, changes, None);
        let mut all = [old.as_slice(), new.as_slice()].concat();
        all.sort_unstable();
        assert_eq!(names(&panels, Side::Left)[1..], all);
    }

    /// A panel holds about 100 bytes for each entry (DESIGN.md: Memory budget):
    /// the entry itself, its row's index, and its name on the heap. A field
    /// that grows the entry would pass every other test.
    #[test]
    fn an_entry_fits_the_memory_budget() {
        assert!(size_of::<Entry>() <= 48, "{} bytes", size_of::<Entry>());
    }

    /// The list writes only the rows whose selection changed, so the
    /// difference must keep every row of `a` that `b` lacks.
    #[test]
    fn a_difference_keeps_the_rows_the_other_lacks() {
        assert_eq!(difference(&[0..10], &[2..3, 5..7]), [0..2, 3..5, 7..10]);
        assert_eq!(difference(&[0..3, 5..8], &[2..6]), [0..2, 6..8]);
        assert!(difference(&[1..2], &[0..4]).is_empty());
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
                    selected: Vec::new(),
                },
                again: false,
                stale: false,
            }
        );
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

    /// A panel sorted by size gets gozlerd's sizes only with the order a
    /// thread sorted them in, because a change that comes first inserts by
    /// the order the entries are in; an order its folders no longer match,
    /// or one sorted for another folder, is refused, and the entries keep
    /// theirs.
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
        give_sizes(&mut panels, &[(r"C:\d\a", 10), (r"C:\d\b", 90)]);
        assert_eq!(folder_sizes(&panels), [None, None, None]);
        let job = panels.take_folder_sort(Side::Left).expect("sizes wait");
        let place = panels.accept_folder_order(Side::Left, job.sort(), Some(1));
        assert_eq!(place.expect("the folders match").focused, Some(2));
        assert_eq!(names(&panels, Side::Left), ["..", "b", "a", "f"]);
        assert_eq!(folder_sizes(&panels), [Some(90), Some(10), None]);

        give_sizes(&mut panels, &[(r"C:\d\a", 95), (r"C:\d\b", 90)]);
        let sorted = panels
            .take_folder_sort(Side::Left)
            .expect("sizes wait")
            .sort();
        let changes = vec![FolderChange::Added(folder("c"))];
        panels.apply_changes(Side::Left, changes, None);
        assert_eq!(panels.accept_folder_order(Side::Left, sorted, None), None);
        assert_eq!(names(&panels, Side::Left), ["..", "b", "a", "c", "f"]);

        // Another folder with the same names refuses them too.
        let mut panels = showing(r"C:\d", entries());
        let ask = panels.sort(Side::Left, sort).expect("a folder shows");
        complete(&mut panels, &ask, r"C:\d", entries());
        give_sizes(&mut panels, &[(r"C:\d\a", 10), (r"C:\d\b", 90)]);
        let sorted = panels
            .take_folder_sort(Side::Left)
            .expect("sizes wait")
            .sort();
        list(&mut panels, r"C:\e", entries());
        assert_eq!(panels.accept_folder_order(Side::Left, sorted, None), None);
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
        assert!(ask.sizes.is_empty());
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
    fn a_sum_waits_for_the_first_sizes() {
        let mut panels = showing(r"C:\d", vec![folder("Photos"), file("a", 5, 0)]);
        panels.ask_sizes(Side::Left, Generation(6));
        assert_eq!(panels.panel(Side::Left).counts().size, None);
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
        panels.set_selected(Side::Left, vec![2..4]);
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
        panels.set_selected(Side::Left, vec![0..2]);
        assert_eq!(panels.leave_on_escape(Side::Left), None);
        panels.set_selected(Side::Left, Vec::new());
        assert!(panels.leave_on_escape(Side::Left).is_some());
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
