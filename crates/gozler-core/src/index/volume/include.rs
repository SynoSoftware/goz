//! How the index follows the Include rules ([`crate::include`]): it removes
//! the entries they leave out, counts the entries they would keep, and checks
//! each journal record against them.
//!
//! The index keeps the file ID of every folder it left out. A record inside
//! one names a parent the index does not hold, and without that set it would
//! become a placeholder under lost+found, so a change below an unchecked
//! folder would cost memory instead of nothing.

use super::*;
use crate::fold::fold;
use crate::include::{Place, Verdict};
use crate::query::wildcard::CodePointBuf;
use crate::usn::record::{USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE};

/// The rules one journal batch is checked against, with the scratch buffer
/// their name patterns match in.
pub(super) struct Screening {
    rules: Arc<VolumeRules>,
    buf: CodePointBuf,
}

impl Screening {
    pub(super) fn new(rules: Arc<VolumeRules>) -> Self {
        Screening {
            rules,
            buf: CodePointBuf::new(),
        }
    }
}

/// What [`VolumeIndex::screen`] decides for one journal record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Screen {
    /// Apply the record as it is.
    Apply,
    /// The rules keep the record's entry, which the index lacks because the
    /// rules left it out before: insert it as an entry the build missed.
    Missed,
    /// The rules leave the record's entry out, and the screen removed it.
    Drop,
    /// The rules leave out a name of a file with several, so reconcile its
    /// names, which keeps only those the rules keep.
    Relink,
}

/// Where an indexed entry stands under the rules.
enum Standing {
    /// Kept, as an entry or as the path to a checked folder; what is below
    /// it stands at this place.
    At(Place),
    /// The rules leave it out.
    LeftOut,
    /// It has no full path, so the rules cannot place it.
    Orphan,
}

impl VolumeIndex {
    /// Removes every entry that `rules` leave out, and checks every later
    /// journal batch against them. It reads nothing from the volume, so it
    /// cannot bring back an entry that earlier rules removed: rules that add
    /// anything back need a rebuild ([`VolumeRules::adds_to`]). Call
    /// [`Self::optimize_storage`] after it to return the removed entries'
    /// memory. Rules without Dates created drop the dates created.
    pub fn apply_rules(&mut self, rules: Arc<VolumeRules>) {
        if !rules.dates_created {
            self.keep_created(false);
        }
        let mut buf = CodePointBuf::new();
        for top in self.left_out_tops(&rules, self.root, rules.root(), &mut buf) {
            self.leave_out(top);
        }
        self.include = (!rules.keeps_everything()).then_some(rules);
    }

    /// Whether `rules` keep anything the index lacks: an entry that the
    /// applied rules left out, or the dates created. Applying them then
    /// needs a read of the volume.
    pub fn lacks_for(&self, rules: &VolumeRules) -> bool {
        (rules.dates_created && !self.keeps_created())
            || self
                .include
                .as_ref()
                .is_some_and(|applied| rules.adds_to(applied))
    }

    /// The rules the index was cut down to; `None` while it keeps every
    /// entry.
    pub fn include_rules(&self) -> Option<&Arc<VolumeRules>> {
        self.include.as_ref()
    }

    /// How many entries would stay if `rules` were applied. Changes nothing.
    pub fn count_kept(&self, rules: &VolumeRules) -> u64 {
        let tops = self.left_out_tops(rules, self.root, rules.root(), &mut CodePointBuf::new());
        let removed: usize = tops
            .iter()
            .map(|&top| 1 + self.subtree_entries(top).count())
            .sum();
        (self.len() - removed) as u64
    }

    /// Whether the rules left out the folder `folder`, so a link in it is no
    /// name the index keeps.
    pub fn is_left_out(&self, folder: Frn) -> bool {
        self.head_of(folder).is_none() && self.left_out.contains(&folder)
    }

    /// Checks what `folders`, which a walk listed, hold against the rules,
    /// and removes what they leave out.
    pub fn recheck_listed(&mut self, folders: &[Frn]) {
        if let Some(rules) = self.include.clone() {
            let mut screening = Screening::new(rules);
            for &folder in folders {
                self.recheck_below(&mut screening, folder);
            }
        }
    }

    /// Checks one journal record against the rules before it is applied. A
    /// record whose entry the rules leave out is dropped, and its entry
    /// leaves the index when it is still there, as after a rename into a
    /// left-out folder or to a left-out name.
    pub(super) fn screen(&mut self, screening: &mut Screening, rec: &ParsedUsnRecord) -> Screen {
        let Screening { rules, buf } = screening;
        let deleted = rec.reason & USN_REASON_FILE_DELETE != 0;
        let place = match self.head_of(rec.parent_frn) {
            Some(parent) => match self.standing(rules, parent, buf) {
                Standing::At(place) => Some(place),
                Standing::LeftOut => None,
                Standing::Orphan => return Screen::Apply,
            },
            None if self.left_out.contains(&rec.parent_frn) => None,
            // A folder the index has not seen yet, which the record makes a
            // placeholder until its own record arrives, as without rules.
            None => return Screen::Apply,
        };
        let verdict = place.map_or(Verdict::LeftOut, |place| {
            let attributes = PackedAttributes::from_record(rec.attributes);
            rules.verdict(place, &fold(&rec.name), attributes, rec.is_dir(), buf)
        });
        let head = self.head_of(rec.frn);
        if verdict == Verdict::LeftOut {
            if rec.is_dir() {
                if deleted {
                    self.left_out.remove(&rec.frn);
                } else {
                    self.left_out.insert(rec.frn);
                }
            }
            return match head {
                Some(_) if deleted => Screen::Apply,
                Some(head) if self.entries.next_link(head) != NIL => Screen::Relink,
                Some(head) => {
                    self.leave_out(head);
                    Screen::Drop
                }
                None => Screen::Drop,
            };
        }
        if rec.is_dir() {
            self.left_out.remove(&rec.frn);
        }
        // An entry whose attributes change so that the rules keep it again
        // comes back with a record that neither creates nor renames it.
        let names_it = rec.reason & (USN_REASON_FILE_CREATE | USN_REASON_RENAME_NEW_NAME) != 0;
        if head.is_none() && !deleted && !names_it {
            Screen::Missed
        } else {
            Screen::Apply
        }
    }

    /// Removes what the rules leave out below `folder`, after it moved or a
    /// walk listed it.
    pub(super) fn recheck_below(&mut self, screening: &mut Screening, folder: Frn) {
        let Some(idx) = self.head_of(folder) else {
            return;
        };
        if self.entries.first_child(idx) == NIL {
            return;
        }
        let Screening { rules, buf } = screening;
        if let Standing::At(place) = self.standing(rules, idx, buf) {
            for top in self.left_out_tops(rules, idx, place, buf) {
                self.leave_out(top);
            }
        }
    }

    /// The names of `actual`, the real names of the file `frn`, that
    /// `rules` keep.
    pub(super) fn kept_links(
        &self,
        rules: &VolumeRules,
        frn: Frn,
        actual: &[LinkTarget],
    ) -> Vec<LinkTarget> {
        let mut buf = CodePointBuf::new();
        let attributes = self
            .head_of(frn)
            .map_or(PackedAttributes::default(), |head| {
                self.entries.attributes(head)
            });
        actual
            .iter()
            .filter(|link| match self.standing(rules, link.parent, &mut buf) {
                Standing::At(place) => matches!(
                    rules.verdict(place, &fold(&link.name), attributes, false, &mut buf),
                    Verdict::Kept(_)
                ),
                Standing::LeftOut => false,
                Standing::Orphan => true,
            })
            .cloned()
            .collect()
    }

    /// Where the indexed entry `idx` stands, decided from the root down.
    fn standing(&self, rules: &VolumeRules, idx: EntryIdx, buf: &mut CodePointBuf) -> Standing {
        let chain = self.chain_up(idx);
        if chain.last() != Some(&self.root) {
            return Standing::Orphan;
        }
        let mut place = rules.root();
        for &e in chain.iter().rev().skip(1) {
            let is_dir = self.entries.flags(e).contains(EntryFlags::DIR);
            match rules.verdict(
                place,
                self.folded_name(e),
                self.entries.attributes(e),
                is_dir,
                buf,
            ) {
                Verdict::Kept(p) | Verdict::Path(p) => place = p,
                Verdict::LeftOut => return Standing::LeftOut,
            }
        }
        Standing::At(place)
    }

    /// The highest entries below `start`, a folder at `place`, that the
    /// rules leave out; each takes everything below it.
    fn left_out_tops(
        &self,
        rules: &VolumeRules,
        start: EntryIdx,
        place: Place,
        buf: &mut CodePointBuf,
    ) -> Vec<EntryIdx> {
        let mut tops = Vec::new();
        let mut folders = vec![(start, place)];
        while let Some((folder, place)) = folders.pop() {
            let mut child = self.entries.first_child(folder);
            while child != NIL {
                let is_dir = self.entries.flags(child).contains(EntryFlags::DIR);
                let attributes = self.entries.attributes(child);
                match rules.verdict(place, self.folded_name(child), attributes, is_dir, buf) {
                    Verdict::Kept(p) | Verdict::Path(p) if is_dir => folders.push((child, p)),
                    Verdict::Kept(_) | Verdict::Path(_) => {}
                    Verdict::LeftOut => tops.push(child),
                }
                child = self.entries.next_child(child);
            }
        }
        tops
    }

    /// Removes `top` and everything below it, remembering each folder.
    fn leave_out(&mut self, top: EntryIdx) {
        let below: Vec<EntryIdx> = self.subtree_entries(top).collect();
        // Children before their folder, so no entry is orphaned.
        for e in below.into_iter().rev().chain([top]) {
            if self.entries.flags(e).contains(EntryFlags::DIR) {
                self.left_out.insert(self.entries.frn(e));
            }
            self.remove_link(e);
        }
    }

    /// Removes one entry, one name of its file, and keeps the file's other
    /// names.
    fn remove_link(&mut self, e: EntryIdx) {
        let frn = self.entries.frn(e);
        let next = self.entries.next_link(e);
        match self.head_of(frn) {
            Some(head) if head == e && next == NIL => self.by_frn.remove(frn),
            Some(head) if head == e => self.by_frn.set(frn, next),
            Some(head) => {
                let mut prev = head;
                while self.entries.next_link(prev) != e && self.entries.next_link(prev) != NIL {
                    prev = self.entries.next_link(prev);
                }
                if self.entries.next_link(prev) == e {
                    self.entries.set_next_link(prev, next);
                }
            }
            None => {}
        }
        self.remove_entry(e);
    }
}
