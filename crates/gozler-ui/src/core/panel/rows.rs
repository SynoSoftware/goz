//! Folder rows own ordering, name reconciliation, taken-entry filtering and
//! retained operation evidence. Panel publishes these immutable snapshots;
//! only the row algorithms mutate their storage while preparing a candidate.

use super::calculation::End;
use super::{
    Awaited, Entry, FolderChange, Outcome, Place, Row, Totals, changes_sizes, ends_in, last_name,
};
use gozler_core::fold::fold_into;
use gozler_core::types::{SortDir, SortKey, SortSpec};
use gozler_core::wtf8;
use gozler_fileops::{OperationId, Taken};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hasher};
use std::ops::Range;
use std::sync::Arc;

/// The most entries a batch of changes inserts one by one
/// ([`FolderRows::insert_sorted`]); each insert moves the entries after it.
const INSERT_IN_PLACE: usize = 8;

/// A name in its folded form, which the name sort and every name
/// comparison use, as Search's name sort does.
pub(super) fn name_key(name: &[u16]) -> Vec<u8> {
    Folding::default().of(name).to_vec()
}

pub(super) fn name_hash(key: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write(key);
    hasher.finish()
}

/// Whether `a` and `b` are the same name without regard to case; folds
/// them only when they differ as written.
pub(super) fn same_name(a: &[u16], b: &[u16]) -> bool {
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
pub(super) struct Name {
    pub(super) hash: u64,
    pub(super) raw: Box<[u16]>,
}

impl Name {
    pub(super) fn of(raw: Box<[u16]>) -> Name {
        Name {
            hash: name_hash(&name_key(&raw)),
            raw,
        }
    }
}

/// Buffers that fold one name after another without an allocation for
/// each.
#[derive(Default)]
pub(super) struct Folding {
    bytes: Vec<u8>,
    folded: Vec<u8>,
}

impl Folding {
    pub(super) fn of(&mut self, name: &[u16]) -> &[u8] {
        self.bytes.clear();
        wtf8::from_utf16(name, &mut self.bytes);
        self.folded.clear();
        fold_into(&self.bytes, &mut self.folded);
        &self.folded
    }

    pub(super) fn of_str(&mut self, name: &str) -> &[u8] {
        self.folded.clear();
        fold_into(name.as_bytes(), &mut self.folded);
        &self.folded
    }
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
/// as [`super::Panels::accept_sizes`] matches gozlerd's.
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

#[derive(Clone, Debug)]
pub(super) struct FolderRows {
    /// Sorted by [`compare`] with `order`.
    entries: Vec<Entry>,
    /// The listing's sort, which differs from the chosen sort while its new
    /// listing is read.
    order: SortSpec,
    totals: Totals,
    /// Indexes in display order while entries are taken; `None` when all show.
    shown: Option<Vec<u32>>,
    /// [`FolderRows::filter`] reuses these hashes between change batches.
    taken_hashes: Option<Arc<TakenHashes>>,
    /// The ended operations that still take rows from this panel, with the
    /// moment their end was seen.
    kept: HashMap<OperationId, u64>,
}

/// The name hashes of the taken paths in one folder, for one snapshot of
/// the taken entries.
#[derive(Debug)]
struct TakenHashes {
    taken: Arc<Taken>,
    folder: String,
    hashes: HashSet<u64>,
}

impl FolderRows {
    pub(super) fn empty(order: SortSpec) -> Self {
        Self {
            entries: Vec::new(),
            order,
            totals: Totals::default(),
            shown: None,
            taken_hashes: None,
            kept: HashMap::new(),
        }
    }

    pub(super) fn replacement(&self) -> Self {
        let mut rows = Self::empty(self.order);
        rows.kept = self.kept.clone();
        rows
    }

    pub(super) fn replace_listing(&mut self, entries: Vec<Entry>, order: SortSpec) {
        self.entries = entries;
        self.order = order;
        self.totals = Totals::of(&self.entries);
    }

    pub(super) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub(super) fn order(&self) -> SortSpec {
        self.order
    }

    pub(super) fn keeps(&self, id: OperationId) -> bool {
        self.kept.contains_key(&id)
    }

    pub(super) fn counting_cost(&self) -> usize {
        self.shown.as_ref().map_or(0, Vec::len)
    }

    pub(super) fn shown_totals(&self) -> Totals {
        match &self.shown {
            None => self.totals,
            Some(shown) => Totals::of(shown.iter().map(|&i| &self.entries[i as usize])),
        }
    }

    pub(super) fn apply_sizes(&mut self, outcome: &Outcome) {
        let entries = match outcome {
            Outcome::Rows(chunk) => chunk.rows.as_ref(),
            _ => &[],
        };
        let sizes: Vec<(Box<[u16]>, u64)> = entries
            .iter()
            .filter(|row| row.is_dir)
            .filter_map(|row| Some((last_name(&row.path).encode_utf16().collect(), row.size?)))
            .collect();
        give_sizes(&mut self.entries, &sizes);
        if self.order.key == SortKey::Size {
            sort_listing(&mut self.entries, self.order);
        }
        self.totals = Totals::of(&self.entries);
    }

    pub(super) fn follow_ends(&mut self, ends: Vec<End>, folder: &str) -> (bool, bool) {
        let mut read_again = false;
        let mut sizes = false;
        for end in ends {
            for (id, request) in &end.evidence.operations {
                if ends_in(request, folder) {
                    self.kept.insert(*id, end.moment);
                    read_again = true;
                }
                sizes |= changes_sizes(request, folder);
            }
        }
        (read_again, sizes)
    }

    pub(super) fn items(&self) -> usize {
        self.entries
            .len()
            .saturating_add(self.kept.len())
            .saturating_add(
                self.taken_hashes
                    .as_ref()
                    .map_or(0, |known| known.taken.len() + known.hashes.len()),
            )
    }

    pub(super) fn row_named(&self, name: &Name, up: u64) -> Option<u64> {
        (up..self.count(up)).find(|row| {
            self.entry(*row, up)
                .is_some_and(|entry| entry.hash == name.hash && entry.name == name.raw)
        })
    }

    pub(super) fn awaited_row(&self, awaited: &Awaited, up: u64) -> Option<u64> {
        let renamed = awaited
            .from
            .as_ref()
            .is_some_and(|from| self.row_named(from, up).is_some());
        self.row_named(&awaited.name, up).filter(|_| !renamed)
    }
    pub(super) fn count(&self, up: u64) -> u64 {
        up + self.shown_len() as u64
    }

    /// The entries the list shows.
    pub(super) fn shown_len(&self) -> usize {
        self.shown.as_ref().map_or(self.entries.len(), Vec::len)
    }

    /// The index in `entries` of the `at`th shown entry.
    fn shown_at(&self, at: usize) -> Option<usize> {
        match &self.shown {
            None => (at < self.entries.len()).then_some(at),
            Some(shown) => shown.get(at).map(|&i| i as usize),
        }
    }

    /// The folder row at `index`; [`super::Panels::row`] gives a search panel's.
    pub(super) fn row(&self, index: u64, up: u64) -> Option<Row<'_>> {
        if up != 0 && index == 0 {
            return Some(Row::Up);
        }
        let at = usize::try_from(index - up).ok()?;
        Some(Row::Entry(&self.entries[self.shown_at(at)?]))
    }

    pub(super) fn entry(&self, index: u64, up: u64) -> Option<&Entry> {
        match self.row(index, up)? {
            Row::Entry(entry) => Some(entry),
            Row::Up | Row::Result(_) => None,
        }
    }

    /// The row that holds `name`: the one with exactly that name, else one
    /// whose name differs only in case, because a case-sensitive folder can
    /// hold both. `near` limits the search to a row and how far from it the
    /// name's row may have moved, so a change costs no pass over a large
    /// folder; `None` searches every row.
    pub(super) fn row_of(&self, name: &Name, near: Option<(u64, u64)>, up: u64) -> Option<u64> {
        let all = up..self.count(up);
        let rows = match near {
            Some((row, reach)) => {
                row.saturating_sub(reach).max(all.start)..(row + reach + 1).min(all.end)
            }
            None => all,
        };
        let exact = |row: &u64| {
            self.entry(*row, up)
                .is_some_and(|e| e.hash == name.hash && e.name == name.raw)
        };
        rows.clone().find(exact).or_else(|| {
            rows.into_iter()
                .find(|row| self.entry(*row, up).is_some_and(|e| e.is(name)))
        })
    }

    /// Keeps the stored names: a refresh or a replacement of that name
    /// preserves selection, but an external rename does not follow it.
    pub(super) fn remember(
        &self,
        focused: Option<u64>,
        selected: &[Range<u64>],
        up: u64,
    ) -> Remembered {
        let mut names: HashMap<u64, Vec<Box<[u16]>>> = HashMap::new();
        for row in selected.iter().flat_map(Clone::clone) {
            if let Some(entry) = self.entry(row, up) {
                names
                    .entry(entry.hash)
                    .or_default()
                    .push(entry.name.clone());
            }
        }
        let focused = focused.and_then(|row| match self.row(row, up)? {
            Row::Up => Some(Focus::Up),
            Row::Result(_) => None,
            Row::Entry(entry) => {
                // The rows after it, nearest first, for when it goes.
                let after = (row + 1..self.count(up))
                    .filter_map(|r| self.entry(r, up).map(Entry::name))
                    .take(64)
                    .collect();
                Some(Focus::Entry {
                    name: entry.name(),
                    after,
                    row,
                })
            }
        });
        Remembered {
            focused,
            selected: names,
        }
    }

    /// The rows of what `remembered` holds, now, which the list then shows.
    /// `reach` is how far a row may have moved, when the change says.
    pub(super) fn place_of(&self, remembered: Remembered, reach: Option<u64>, up: u64) -> Place {
        let count = self.count(up);
        let focused = match remembered.focused {
            None => None,
            Some(Focus::Up) => (count > 0).then_some(0),
            Some(Focus::Entry { name, after, row }) => {
                // A row moves at most `reach`, so a name of the rows after
                // it that is not near is gone. The focused name may be a
                // rename's new name, which can land anywhere.
                let near = reach.map(|reach| (row, reach));
                self.row_of(&name, near, up)
                    .or_else(|| near.and_then(|_| self.row_of(&name, None, up)))
                    .or_else(|| after.iter().find_map(|name| self.row_of(name, near, up)))
                    .or_else(|| count.checked_sub(1).map(|last| row.min(last)))
            }
        };
        let selected = if !remembered.selected.is_empty() {
            let selected: Vec<u64> = (0..self.shown_len())
                .filter(|&at| {
                    let entry = &self.entries[self.shown_at(at).expect("at is a shown entry")];
                    remembered
                        .selected
                        .get(&entry.hash)
                        .is_some_and(|names| names.contains(&entry.name))
                })
                .map(|at| at as u64 + up)
                .collect();
            ranges(&selected)
        } else {
            Vec::new()
        };
        Place {
            focused,
            selected: Arc::new(selected),
        }
    }

    /// Recomputes which entries show, after the entries or the taken
    /// entries changed. Only a folder that a delete or a move takes, or
    /// one that holds a taken entry, looks at its entries, so a change in
    /// any other folder costs nothing. In a folder that holds one, only an
    /// entry whose name has a taken name's hash is looked up, by its name
    /// as the operation gave it, so a change batch folds no name; its path
    /// is compared only when the operation gave its name in another case.
    pub(super) fn filter(&mut self, folder: &str, taken: &Arc<Taken>) {
        let kept = &self.kept;
        let keeps = |id| kept.contains_key(&id);
        if taken.is_empty() {
            self.shown = None;
            self.taken_hashes = None;
            return;
        }
        let wide: Vec<u16> = folder.encode_utf16().collect();
        let here = taken.in_folder(&wide);
        if here.is_none() {
            self.taken_hashes = None;
        }
        self.shown = if taken.contains(&wide, keeps) {
            Some(Vec::new())
        } else if let Some(here) = here {
            let known = (self.taken_hashes.as_ref())
                .is_some_and(|known| Arc::ptr_eq(&known.taken, taken) && known.folder == folder);
            if !known {
                let mut folding = Folding::default();
                self.taken_hashes = Some(Arc::new(TakenHashes {
                    taken: Arc::clone(taken),
                    folder: folder.to_owned(),
                    hashes: here
                        .names()
                        .map(|name| name_hash(folding.of(name)))
                        .collect(),
                }));
            }
            let hashes = &self.taken_hashes.as_ref().expect("filled above").hashes;
            let mut path = wide;
            if path.last() != Some(&u16::from(b'\\')) {
                path.push(u16::from(b'\\'));
            }
            let base = path.len();
            let mut shown = Vec::new();
            for (i, entry) in self.entries.iter().enumerate() {
                if hashes.contains(&entry.hash) {
                    let hidden = here.takes(&entry.name, keeps).unwrap_or_else(|| {
                        path.truncate(base);
                        path.extend_from_slice(&entry.name);
                        taken.contains(&path, keeps)
                    });
                    if hidden {
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
    pub(super) fn merge(&mut self, changes: Vec<FolderChange>) -> bool {
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
        let mut replaced: HashMap<Box<[u16]>, Option<u64>> = HashMap::new();
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
            replaced.insert(entry.name.clone(), entry.folder_size());
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
        let mut folded: HashMap<Box<[u16]>, Option<u64>> = HashMap::new();
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
                folded.insert((*name).into(), entry.folder_size());
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
                if let Some(&size) = replaced.get(&name) {
                    entry.set_folder_size(size);
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

    /// Releases the ended operations that ended before `started`, the
    /// moment the completed listing started. Returns whether it keeps one
    /// that ended after, whose rows only a newer listing can show.
    pub(super) fn release(&mut self, started: u64) -> bool {
        self.kept.retain(|_, ended| *ended >= started);
        !self.kept.is_empty()
    }
}

/// A changed name's last state in a batch, which decides how
/// [`FolderRows::merge`] matches it to an entry.
enum Now {
    Gone,
    Modified(Entry),
    /// Added, or a rename's new name.
    Added(Entry),
}

/// The focused row by name.
#[derive(Debug)]
pub(super) enum Focus {
    Up,
    Entry {
        name: Name,
        /// The names of the rows after it, for when it goes.
        after: Vec<Name>,
        row: u64,
    },
}

/// The list's place by stored names while its row indexes change.
#[derive(Debug, Default)]
pub(super) struct Remembered {
    pub(super) focused: Option<Focus>,
    selected: HashMap<u64, Vec<Box<[u16]>>>,
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
pub(super) fn ranges(rows: &[u64]) -> Vec<Range<u64>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::panel::{
        Panels, Side,
        tests::{file, names, showing},
    };
    use std::path::Path;

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
            let hashes = panels.panel(Side::Left).rows.taken_hashes.as_ref();
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
}
