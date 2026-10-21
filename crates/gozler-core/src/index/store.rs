//! Low-level index storage: the interned name store, the struct-of-arrays
//! entry table, and the FRN → entry map. These carry no policy: [`super::volume`]
//! owns the apply/rename/link rules; this file just stores bytes and columns
//! compactly.
//!
//! Names are INTERNED: most names on a real Windows volume repeat
//! (node_modules alone contributes tens of thousands of copies of
//! `index.js`), so each distinct name is stored once and every entry carries a
//! 4-byte [`NameId`]. Entries sharing a name are linked through a doubly-linked
//! same-name chain (`next_same` / `prev_same` columns, head in the store), which
//! is how a query scan hit over the deduplicated haystack expands back into
//! entries. The chains are kept EXACT (eager O(1) unlink on tombstone), never
//! lazily repaired: a stale singly-linked chain can cycle through reused slots,
//! and an exact chain needs no per-candidate staleness guard at query time.

use crate::attributes::PackedAttributes;
use crate::types::{EntryIdx, Frn, NIL};
use core::num::NonZeroU32;
use rustc_hash::FxHashMap;

/// Interned-name handle: an index into the name store's tables.
pub(crate) type NameId = u32;

/// A slice of a `NameStore` byte buffer: byte offset + byte length. Names are
/// capped at 64 KiB (an NTFS component is ≤ 255 UTF-16 units, so ≤ 765 WTF-8
/// bytes) and the arenas are assumed to stay under 2 GiB (bit 31 of `off` is
/// a buffer selector; nothing enforces the bound, `push` truncates if it ever
/// did not hold), so `u32`/`u16` are ample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct NameRef {
    /// Byte offset into the arena. Bit 31 (`RAW_IN_FOLDED`) on a RAW ref means
    /// the raw bytes live in the FOLDED arena (the name folds to itself, so
    /// one copy serves both).
    pub off: u32,
    /// Byte length of the name (excludes the NUL separator).
    pub len: u16,
}

/// Bit 31 of `NameRef::off` on a raw ref: the bytes live in the folded arena.
const RAW_IN_FOLDED: u32 = 1 << 31;

/// One interned name: a handle into the raw and folded buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct NamePair {
    /// Original-case bytes for display and path reconstruction. May carry
    /// [`RAW_IN_FOLDED`] in `off`.
    pub raw: NameRef,
    /// Case-folded bytes for the query scan, always in the folded arena.
    pub folded: NameRef,
}

// Per-entry SoA columns are multiplied by millions of files and per-name
// tables by millions of unique names: a silent layout regression (a widened
// column, `EntryFlags` past a `u8`) costs hundreds of MB on a 10M-file volume
// with no other signal. Pin the sizes.
const _: () = assert!(size_of::<NameRef>() == 8);
const _: () = assert!(size_of::<NamePair>() == 16);
const _: () = assert!(size_of::<EntryFlags>() == 1);
const _: () = assert!(size_of::<PackedAttributes>() == 2);
const _: () = assert!(size_of::<EntryIdx>() == 4);
const _: () = assert!(size_of::<NameId>() == 4);

/// Freshly compacted name storage, with its intern table and ranks, plus the
/// id remap for every kept name. Built by [`NameStore::plan_compact`], applied
/// with [`NameStore::install_compacted`].
pub(crate) struct CompactedNames {
    raw: Vec<u8>,
    folded: Vec<u8>,
    pairs: Vec<NamePair>,
    refcount: Vec<u32>,
    head: Vec<EntryIdx>,
    by_bytes: hashbrown::HashTable<NameId>,
    ranks: NameRanks,
    /// `remap[old_id]` = new id, or [`NIL`] for a dropped (refcount-0) name.
    remap: Vec<NameId>,
}

/// The place of each name's folded bytes among the folded bytes of every
/// name, so a sort compares numbers instead of names. Names whose folded
/// bytes are equal share a rank, because the result order compares folded
/// names.
#[derive(Debug, Default)]
pub(crate) struct NameRanks {
    /// The rank of each name id, from 1; `None` for a name added after the
    /// ranks were made, which [`NameStore::name_key`] places by a binary
    /// search.
    rank: Vec<Option<NonZeroU32>>,
    /// The ranked name ids in rank order. A ranked name that died stays here,
    /// with its bytes zero-filled, until the next merge or compaction.
    by_rank: Vec<NameId>,
    /// Live names without a rank. With `dead_ranked`, the cue to merge the
    /// ranks again ([`NameStore::ranks_stale`]).
    unranked: usize,
    /// Ranked names that died since the ranks were made.
    dead_ranked: usize,
}

/// The interned name store: every distinct name once, in two NUL-separated
/// byte buffers (original-case `raw`, case-folded `folded`), plus per-name
/// tables indexed by [`NameId`].
///
/// NUL cannot occur in an NTFS name, so the separators, and the zero-fill
/// written over a dead name's bytes when its last reference drops, mean a
/// substring search over the contiguous `folded` buffer can never match
/// across a boundary or inside a dead name. `pairs[id].folded.off` is
/// ascending in `id` (appends only grow the buffer and compaction preserves
/// order), which is what lets a scan map hit offsets back to ids with a
/// forward merge-walk instead of a per-hit binary search.
///
/// When a name case-folds to itself (most real-world names do), the raw bytes
/// are NOT stored: the raw ref points into the folded arena via
/// [`RAW_IN_FOLDED`].
#[derive(Debug, Default)]
pub(crate) struct NameStore {
    raw: Vec<u8>,
    folded: Vec<u8>,
    pairs: Vec<NamePair>,
    /// Live references per name; 0 = dead, awaiting compaction.
    refcount: Vec<u32>,
    /// Head of the same-name entry chain (see `EntryTable::next_same`).
    head: Vec<EntryIdx>,
    /// raw bytes → id. Stores only the id (4 bytes/name); key bytes live in
    /// the arenas and are compared through `raw_bytes`.
    by_bytes: hashbrown::HashTable<NameId>,
    /// Dead bytes in `raw` + `folded`, the compaction cue.
    garbage: usize,
    ranks: NameRanks,
}

pub(crate) fn hash_bytes(b: &[u8]) -> u64 {
    use core::hash::{Hash, Hasher};
    let mut h = rustc_hash::FxHasher::default();
    b.hash(&mut h);
    h.finish()
}

/// The capacity a column of `len` elements keeps: its length plus ~1.6%
/// headroom (see `NameStore::shrink_with_headroom` for why never to
/// exact-fit).
fn with_headroom(len: usize) -> usize {
    len + len / 64 + 64
}

/// Shrinks a Vec to [`with_headroom`] of its length.
fn shrink_vec<T>(v: &mut Vec<T>) {
    v.shrink_to(with_headroom(v.len()));
}

/// Makes room for `additional` more elements, growing by an eighth instead of
/// `Vec`'s doubling: once live growth passes the headroom
/// [`shrink_vec`] leaves, doubling would pin up to twice the column's size for
/// the daemon's lifetime. Growth stays amortized O(1).
fn reserve_eighth<T>(v: &mut Vec<T>, additional: usize) {
    if v.capacity() - v.len() < additional {
        v.reserve_exact(additional.max(v.len() / 8 + 64));
    }
}

/// [`Vec::push`] that grows by [`reserve_eighth`].
fn push_eighth<T>(v: &mut Vec<T>, value: T) {
    reserve_eighth(v, 1);
    v.push(value);
}

/// Where an entry compaction's `remap` moved `idx`; [`NIL`] stays [`NIL`].
pub(crate) fn moved(remap: &[EntryIdx], idx: EntryIdx) -> EntryIdx {
    if idx == NIL { NIL } else { remap[idx as usize] }
}

/// Moves each live slot of `column` to its place in `remap`, through `map`,
/// and drops the rest. A slot only moves down, so one forward pass is safe.
fn compact_column<T: Copy>(
    column: &mut Vec<T>,
    remap: &[EntryIdx],
    live: usize,
    map: impl Fn(T) -> T,
) {
    for (old, &new) in remap.iter().enumerate() {
        if new != NIL {
            column[new as usize] = map(column[old]);
        }
    }
    column.truncate(live);
}

/// `NameStore::raw_bytes` over explicit tables: the intern table's rehash
/// closures need to hash names while `by_bytes` is mutably borrowed, so they
/// borrow the byte tables disjointly and go through this.
fn raw_bytes_in<'a>(
    pairs: &'a [NamePair],
    raw: &'a [u8],
    folded: &'a [u8],
    id: NameId,
) -> &'a [u8] {
    let r = pairs[id as usize].raw;
    let buf = if r.off & RAW_IN_FOLDED != 0 {
        folded
    } else {
        raw
    };
    let off = (r.off & !RAW_IN_FOLDED) as usize;
    &buf[off..off + r.len as usize]
}

impl NameStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Number of name ids ever allocated (live + dead-awaiting-compaction).
    #[cfg(test)]
    pub(crate) fn ids(&self) -> usize {
        self.pairs.len()
    }

    /// Interns `name`, bumping its refcount (creating it on first sight).
    /// Panics only if `name` exceeds 64 KiB, which no NTFS component can.
    pub(crate) fn intern(&mut self, name: &[u8]) -> NameId {
        let hash = hash_bytes(name);
        if let Some(&id) = self.by_bytes.find(hash, |&id| self.raw_bytes(id) == name) {
            self.refcount[id as usize] += 1;
            return id;
        }

        let folded_bytes = crate::fold::fold(name);
        let folded = Self::push(&mut self.folded, &folded_bytes);
        let raw = if folded_bytes.as_slice() == name {
            NameRef {
                off: folded.off | RAW_IN_FOLDED,
                len: folded.len,
            }
        } else {
            Self::push(&mut self.raw, name)
        };

        let id = self.pairs.len() as NameId;
        push_eighth(&mut self.pairs, NamePair { raw, folded });
        push_eighth(&mut self.refcount, 1);
        push_eighth(&mut self.head, NIL);
        push_eighth(&mut self.ranks.rank, None);
        self.ranks.unranked += 1;
        let Self {
            by_bytes,
            pairs,
            raw: raw_buf,
            folded: folded_buf,
            ..
        } = self;
        by_bytes.insert_unique(hash, id, |&i| {
            hash_bytes(raw_bytes_in(pairs, raw_buf, folded_buf, i))
        });
        id
    }

    fn push(buf: &mut Vec<u8>, bytes: &[u8]) -> NameRef {
        let len = u16::try_from(bytes.len())
            .expect("every name source caps a name at 255 UTF-16 units, far below 64 KiB");
        let off = buf.len() as u32;
        reserve_eighth(buf, bytes.len() + 1);
        buf.extend_from_slice(bytes);
        buf.push(0);
        NameRef { off, len }
    }

    /// The raw (original-case) bytes of `id`.
    #[inline]
    pub(crate) fn raw_bytes(&self, id: NameId) -> &[u8] {
        raw_bytes_in(&self.pairs, &self.raw, &self.folded, id)
    }

    /// The folded bytes of `id`.
    #[inline]
    pub(crate) fn folded_bytes_of(&self, id: NameId) -> &[u8] {
        let r = self.pairs[id as usize].folded;
        &self.folded[r.off as usize..r.off as usize + r.len as usize]
    }

    /// The whole folded buffer, for a contiguous query scan.
    #[inline]
    pub(crate) fn folded_haystack(&self) -> &[u8] {
        &self.folded
    }

    /// All interned pairs, indexed by id; `folded.off` ascending. The scan's
    /// merge-walk indexes this directly.
    #[inline]
    pub(crate) fn pairs(&self) -> &[NamePair] {
        &self.pairs
    }

    #[inline]
    pub(crate) fn chain_head(&self, id: NameId) -> EntryIdx {
        self.head[id as usize]
    }

    #[inline]
    pub(crate) fn set_chain_head(&mut self, id: NameId, e: EntryIdx) {
        self.head[id as usize] = e;
    }

    /// Drops one reference to `id`. When the last reference goes, the name's
    /// bytes are zero-filled (so the scan can never match them), the intern
    /// entry is removed (so the id can never be returned again), and the
    /// bytes are accounted as garbage for the compaction trigger.
    pub(crate) fn release(&mut self, id: NameId) {
        let i = id as usize;
        debug_assert!(self.refcount[i] > 0, "release of a dead name");
        self.refcount[i] -= 1;
        if self.refcount[i] > 0 {
            return;
        }
        debug_assert!(
            self.head[i] == NIL,
            "a name died while entries still chain to it"
        );
        let pair = self.pairs[i];
        let hash = hash_bytes(self.raw_bytes(id));
        if let Ok(entry) = self.by_bytes.find_entry(hash, |&cand| cand == id) {
            entry.remove();
        }
        match self.ranks.rank[i] {
            None => self.ranks.unranked -= 1,
            Some(_) => self.ranks.dead_ranked += 1,
        }
        // Zero the folded bytes (the scan haystack) and, if separately
        // stored, the raw bytes. Garbage counts both plus separators.
        let f = pair.folded;
        for b in &mut self.folded[f.off as usize..f.off as usize + f.len as usize] {
            *b = 0;
        }
        self.garbage += f.len as usize + 1;
        if pair.raw.off & RAW_IN_FOLDED == 0 {
            let r = pair.raw;
            for b in &mut self.raw[r.off as usize..r.off as usize + r.len as usize] {
                *b = 0;
            }
            self.garbage += r.len as usize + 1;
        }
    }

    /// `true` once the buffers exceed 8 KiB and dead bytes exceed 25% of
    /// them, the compaction cue.
    pub(crate) fn should_compact(&self) -> bool {
        let total = self.raw.len() + self.folded.len();
        total > 8192 && self.garbage * 4 > total
    }

    /// Builds compacted storage keeping only live (refcount > 0) names, in id
    /// order, WITHOUT mutating `self`: the copy and the intern table are the
    /// whole cost of a compaction and must not run while queries are blocked.
    /// `ranks` are this store's ranks or a merge of them from
    /// [`Self::plan_ranks`]; the compaction moves them to the new ids and
    /// drops the dead names from their order. The caller applies the result
    /// with [`Self::install_compacted`].
    pub(crate) fn plan_compact(&self, ranks: &NameRanks) -> CompactedNames {
        let live = self.refcount.iter().filter(|&&c| c > 0).count();
        let mut c = CompactedNames {
            raw: Vec::with_capacity(self.raw.len().saturating_sub(self.garbage)),
            folded: Vec::with_capacity(self.folded.len()),
            pairs: Vec::with_capacity(live),
            refcount: Vec::with_capacity(live),
            head: Vec::with_capacity(live),
            by_bytes: hashbrown::HashTable::with_capacity(live),
            ranks: NameRanks {
                rank: Vec::with_capacity(live),
                by_rank: Vec::with_capacity(ranks.by_rank.len() - ranks.dead_ranked),
                unranked: 0,
                dead_ranked: 0,
            },
            remap: vec![NIL; self.pairs.len()],
        };
        for (i, &rc) in self.refcount.iter().enumerate() {
            if rc == 0 {
                continue;
            }
            let pair = self.pairs[i];
            let folded = Self::push(&mut c.folded, self.folded_bytes_of(i as NameId));
            let raw = if pair.raw.off & RAW_IN_FOLDED != 0 {
                NameRef {
                    off: folded.off | RAW_IN_FOLDED,
                    len: folded.len,
                }
            } else {
                Self::push(&mut c.raw, self.raw_bytes(i as NameId))
            };
            let id = c.pairs.len() as NameId;
            c.remap[i] = id;
            c.pairs.push(NamePair { raw, folded });
            c.refcount.push(rc);
            c.head.push(self.head[i]);
            c.ranks.rank.push(ranks.rank[i]);
            c.ranks.unranked += usize::from(ranks.rank[i].is_none());
            let hash = hash_bytes(self.raw_bytes(i as NameId));
            let (pairs, raw, folded) = (&c.pairs, &c.raw, &c.folded);
            c.by_bytes.insert_unique(hash, id, |&i| {
                hash_bytes(raw_bytes_in(pairs, raw, folded, i))
            });
        }
        c.ranks.by_rank.extend(
            ranks
                .by_rank
                .iter()
                .map(|&id| c.remap[id as usize])
                .filter(|&id| id != NIL),
        );
        c
    }

    /// Installs storage built by [`Self::plan_compact`], returning the id
    /// remap (`remap[old_id]` = new id or [`NIL`]). The caller must rewrite
    /// every entry's name id through it. Only buffer swaps. The plan is valid
    /// only if the store has not changed since it was built (see
    /// [`super::volume::VolumeIndex::plan_compaction`]).
    pub(crate) fn install_compacted(&mut self, c: CompactedNames) -> Vec<NameId> {
        self.raw = c.raw;
        self.folded = c.folded;
        self.pairs = c.pairs;
        self.refcount = c.refcount;
        self.head = c.head;
        self.by_bytes = c.by_bytes;
        self.garbage = 0;
        self.ranks = c.ranks;
        c.remap
    }

    /// Whether names added since the ranks were made, or ranked names that
    /// died since, pass 0.1 % of the names. Each unranked name costs a search
    /// a binary search, which steps over each dead name it meets.
    pub(crate) fn ranks_stale(&self) -> bool {
        let NameRanks {
            by_rank,
            unranked,
            dead_ranked,
            ..
        } = &self.ranks;
        let names = by_rank.len() - dead_ranked + unranked;
        unranked.max(dead_ranked) * 1000 > names
    }

    /// Ranks every live name, WITHOUT mutating `self`, so searches keep
    /// reading while it runs: sorts only the unranked names, and merges them
    /// into the rank order without the dead names. The caller installs the
    /// result with [`Self::install_ranks`], or through
    /// [`Self::plan_compact`], while the store has not changed.
    pub(crate) fn plan_ranks(&self) -> NameRanks {
        use rayon::slice::ParallelSliceMut;
        let live = |id: NameId| self.refcount[id as usize] > 0;
        let folded = |id: NameId| self.folded_bytes_of(id);
        let mut added: Vec<NameId> = (0..self.pairs.len() as NameId)
            .filter(|&id| self.ranks.rank[id as usize].is_none() && live(id))
            .collect();
        added.par_sort_unstable_by(|&a, &b| folded(a).cmp(folded(b)));

        let live_ranked = self.ranks.by_rank.len() - self.ranks.dead_ranked;
        let mut by_rank = Vec::with_capacity(live_ranked + added.len());
        let mut ranked = self
            .ranks
            .by_rank
            .iter()
            .copied()
            .filter(|&id| live(id))
            .peekable();
        for id in added {
            while let Some(below) = ranked.next_if(|&r| folded(r) <= folded(id)) {
                by_rank.push(below);
            }
            by_rank.push(id);
        }
        by_rank.extend(ranked);

        // With the headroom `shrink_vec` leaves, so the next intern does not
        // copy the whole column under the write lock.
        let names = self.pairs.len();
        let mut rank = Vec::with_capacity(with_headroom(names));
        rank.resize(names, None);
        let mut current = 0u32;
        let mut previous: Option<&[u8]> = None;
        for &id in &by_rank {
            if previous != Some(folded(id)) {
                current += 1;
                previous = Some(folded(id));
            }
            rank[id as usize] = NonZeroU32::new(current);
        }
        NameRanks {
            rank,
            by_rank,
            ..NameRanks::default()
        }
    }

    /// Installs ranks built by [`Self::plan_ranks`]. Only buffer swaps. The
    /// ranks are valid only if the store has not changed since they were
    /// built.
    pub(crate) fn install_ranks(&mut self, ranks: NameRanks) {
        self.ranks = ranks;
    }

    /// The ranks as they are, for a compaction that does not merge them.
    pub(crate) fn ranks(&self) -> &NameRanks {
        &self.ranks
    }

    /// Where the folded bytes of live name `id` sort among every name's, as
    /// one number: 2 × its rank for a ranked name. An unranked name takes
    /// 2 × r when its folded bytes equal those of the nearest live ranked name
    /// not greater than them, whose rank is r, and 2 × r + 1 when they are
    /// greater, with r = 0 when there is no such name. So a smaller key always
    /// means smaller folded bytes, and unranked names between the same two
    /// ranked names share a key.
    ///
    /// A key fits in a `u32`: every name takes at least its NUL in the folded
    /// arena, which stays under 2 GiB, so a rank stays under 2^31.
    pub(crate) fn name_key(&self, id: NameId) -> u32 {
        if let Some(rank) = self.ranks.rank[id as usize] {
            return 2 * rank.get();
        }
        let folded = self.folded_bytes_of(id);
        let order = &self.ranks.by_rank;
        let live = |at: usize| self.refcount[order[at] as usize] > 0;
        // Every live name before `lo` is not greater than `folded`, and every
        // live name from `hi` on is greater. A dead name's bytes are zero, so
        // the search steps over it to the next live name.
        let (mut lo, mut hi) = (0, order.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match (mid..hi).find(|&at| live(at)) {
                Some(at) if self.folded_bytes_of(order[at]) <= folded => lo = at + 1,
                _ => hi = mid,
            }
        }
        match (0..lo).rev().find(|&at| live(at)).map(|at| order[at]) {
            Some(below) => {
                let equal = self.folded_bytes_of(below) == folded;
                let rank =
                    self.ranks.rank[below as usize].expect("the rank order holds ranked names");
                2 * rank.get() + u32::from(!equal)
            }
            None => 1,
        }
    }

    pub(crate) fn raw_mem(&self) -> ComponentBytes {
        ComponentBytes::of_vec(&self.raw)
    }

    pub(crate) fn folded_mem(&self) -> ComponentBytes {
        ComponentBytes::of_vec(&self.folded)
    }

    /// Per-name table bytes (pairs + refcounts + chain heads + ranks + intern
    /// table).
    pub(crate) fn tables_mem(&self) -> ComponentBytes {
        let intern_per = (size_of::<NameId>() + 1) as u64;
        let intern_buckets = ((self.by_bytes.capacity() as u64 * 8) / 7).next_power_of_two();
        ComponentBytes::of_vec(&self.pairs)
            + ComponentBytes::of_vec(&self.refcount)
            + ComponentBytes::of_vec(&self.head)
            + ComponentBytes::of_vec(&self.ranks.rank)
            + ComponentBytes::of_vec(&self.ranks.by_rank)
            + ComponentBytes {
                used: self.by_bytes.len() as u64 * intern_per,
                allocated: intern_buckets * intern_per,
            }
    }

    /// Rewrites every chain head through an entry compaction's `remap`
    /// ([`EntryTable::compact`]).
    pub(crate) fn remap_heads(&mut self, remap: &[EntryIdx]) {
        for head in &mut self.head {
            *head = moved(remap, *head);
        }
    }

    /// Returns growth slack to the OS, keeping ~1.6% headroom. Called once
    /// post-bootstrap: the buffers grew by [`reserve_eighth`], so up to a
    /// ninth of their allocation, and all of the intern table's power-of-two
    /// overshoot, can be slack. NOT an exact-fit shrink: a Vec shrunk to
    /// exactly its length reallocates, copying the whole buffer, on the very
    /// next journal append. The headroom absorbs live churn instead.
    pub(crate) fn shrink_with_headroom(&mut self) {
        shrink_vec(&mut self.raw);
        shrink_vec(&mut self.folded);
        shrink_vec(&mut self.pairs);
        shrink_vec(&mut self.refcount);
        shrink_vec(&mut self.head);
        shrink_vec(&mut self.ranks.rank);
        let Self {
            by_bytes,
            pairs,
            raw,
            folded,
            ..
        } = self;
        by_bytes.shrink_to_fit(|&id| hash_bytes(raw_bytes_in(pairs, raw, folded, id)));
    }
}

/// Heap bytes one index component holds. `used` is what the live data needs;
/// `allocated` is what the component actually reserved (capacity), so
/// `allocated - used` is reclaimable slack. Both count heap payload only, not
/// the struct headers (a few dozen bytes against hundreds of MB).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ComponentBytes {
    pub used: u64,
    pub allocated: u64,
}

impl ComponentBytes {
    pub(crate) fn of_vec<T>(v: &Vec<T>) -> Self {
        let w = size_of::<T>() as u64;
        ComponentBytes {
            used: v.len() as u64 * w,
            allocated: v.capacity() as u64 * w,
        }
    }
}

impl core::ops::Add for ComponentBytes {
    type Output = ComponentBytes;
    fn add(self, other: ComponentBytes) -> ComponentBytes {
        ComponentBytes {
            used: self.used + other.used,
            allocated: self.allocated + other.allocated,
        }
    }
}

/// Per-entry flag bits.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct EntryFlags(u8);

impl EntryFlags {
    pub(crate) const DIR: EntryFlags = EntryFlags(1 << 0);
    /// The slot is dead and on the free list.
    pub(crate) const TOMBSTONE: EntryFlags = EntryFlags(1 << 1);
    /// A parent stand-in created before its real record was seen; carries a
    /// synthetic name until the real record fills it in.
    pub(crate) const PLACEHOLDER: EntryFlags = EntryFlags(1 << 2);

    pub(crate) const EMPTY: EntryFlags = EntryFlags(0);

    #[inline]
    pub(crate) fn contains(self, other: EntryFlags) -> bool {
        self.0 & other.0 == other.0
    }

    #[inline]
    #[must_use]
    pub(crate) fn with(self, other: EntryFlags, on: bool) -> EntryFlags {
        if on {
            EntryFlags(self.0 | other.0)
        } else {
            EntryFlags(self.0 & !other.0)
        }
    }
}

impl core::fmt::Debug for EntryFlags {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut parts = Vec::new();
        for (bit, name) in [
            (Self::DIR, "DIR"),
            (Self::TOMBSTONE, "TOMBSTONE"),
            (Self::PLACEHOLDER, "PLACEHOLDER"),
        ] {
            if self.contains(bit) {
                parts.push(name);
            }
        }
        write!(f, "EntryFlags({})", parts.join("|"))
    }
}

/// Sentinel size meaning "unknown" (not yet enriched from FILE_LAYOUT / stat).
pub(crate) const SIZE_UNKNOWN: u64 = u64::MAX;
/// Sentinel mtime meaning "unknown".
pub(crate) const MTIME_UNKNOWN: i64 = i64::MIN;

/// The width of a volume's file IDs, which its filesystem decides: NTFS IDs
/// fit in 64 bits, keyed by their record number, and ReFS IDs take 128 bits,
/// whose low bytes repeat across folders, so they are keyed whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdWidth {
    Narrow,
    Wide,
}

/// The entries' file IDs: `u64`s for an index of [`IdWidth::Narrow`] IDs,
/// so an NTFS index never pays for the wider ID, and `u128`s for one of
/// [`IdWidth::Wide`] IDs. A narrow column that is given a wide ID, which
/// only corrupt data holds, widens rather than lose its high half.
#[derive(Debug)]
enum FrnColumn {
    Narrow(Vec<u64>),
    Wide(Vec<u128>),
}

impl Default for FrnColumn {
    fn default() -> Self {
        FrnColumn::Narrow(Vec::new())
    }
}

impl FrnColumn {
    fn len(&self) -> usize {
        match self {
            FrnColumn::Narrow(v) => v.len(),
            FrnColumn::Wide(v) => v.len(),
        }
    }

    #[inline]
    fn get(&self, i: usize) -> Frn {
        match self {
            FrnColumn::Narrow(v) => Frn(v[i].into()),
            FrnColumn::Wide(v) => Frn(v[i]),
        }
    }

    fn set(&mut self, i: usize, frn: Frn) {
        self.widen_for(frn);
        match self {
            FrnColumn::Narrow(v) => v[i] = narrow(frn),
            FrnColumn::Wide(v) => v[i] = frn.0,
        }
    }

    fn push(&mut self, frn: Frn) {
        self.widen_for(frn);
        match self {
            FrnColumn::Narrow(v) => push_eighth(v, narrow(frn)),
            FrnColumn::Wide(v) => push_eighth(v, frn.0),
        }
    }

    fn compact(&mut self, remap: &[EntryIdx], live: usize) {
        match self {
            FrnColumn::Narrow(v) => compact_column(v, remap, live, |id| id),
            FrnColumn::Wide(v) => compact_column(v, remap, live, |id| id),
        }
    }

    fn widen_for(&mut self, frn: Frn) {
        if let FrnColumn::Narrow(v) = self
            && frn.is_wide()
        {
            *self = FrnColumn::Wide(v.iter().map(|&id| id.into()).collect());
        }
    }

    fn memory(&self) -> ComponentBytes {
        match self {
            FrnColumn::Narrow(v) => ComponentBytes::of_vec(v),
            FrnColumn::Wide(v) => ComponentBytes::of_vec(v),
        }
    }

    fn shrink_with_headroom(&mut self) {
        match self {
            FrnColumn::Narrow(v) => shrink_vec(v),
            FrnColumn::Wide(v) => shrink_vec(v),
        }
    }
}

/// `frn` as a narrow column stores it.
fn narrow(frn: Frn) -> u64 {
    u64::try_from(frn.0).expect("widen_for widens the column before it stores a wide ID")
}

/// Struct-of-arrays entry storage. Every column is indexed by [`EntryIdx`];
/// one entry exists per link name, so a hard-linked file has several entries
/// sharing an [`Frn`], chained through `next_link`. Entries sharing an
/// interned name are chained through `next_same`/`prev_same` (head in
/// [`NameStore`]); [`super::volume`] keeps those chains exact.
#[derive(Debug, Default)]
pub(crate) struct EntryTable {
    frn: FrnColumn,
    parent: Vec<EntryIdx>,
    name_id: Vec<NameId>,
    flags: Vec<EntryFlags>,
    size: Vec<u64>,
    mtime: Vec<i64>,
    attributes: Vec<PackedAttributes>,
    next_link: Vec<EntryIdx>,
    next_same: Vec<EntryIdx>,
    prev_same: Vec<EntryIdx>,
    /// Head of this entry's CHILD chain (meaningful for directories; NIL for
    /// files). With `next_child`/`prev_child` this makes a scope subtree
    /// enumerable, which turns a folder-scoped query from an O(volume) scan
    /// with a per-candidate ancestry check into an O(subtree) walk.
    first_child: Vec<EntryIdx>,
    /// Doubly-linked sibling chain, kept exact like `next_same`.
    next_child: Vec<EntryIdx>,
    prev_child: Vec<EntryIdx>,
    free: Vec<EntryIdx>,
    live: usize,
    reused: ReusedSlots,
}

/// How many of the latest reused slots [`ReusedSlots`] keeps. They cover the
/// time between a search's two holds of the lock, in which the journal
/// applies the changes queued during the first hold and those that arrive
/// during the sort. Past them, the search loses the volume's answer, so the
/// note keeps a burst of deletes and creates, such as `npm install` over an
/// old `node_modules`; DESIGN.md ("Rows on demand") gives the figures.
pub(crate) const REUSED_SLOTS_KEPT: usize = 65_536;

/// The slots [`EntryTable::alloc`] gave to a new entry after another had
/// died there, the latest last, so a search that released the index can
/// tell which of its matches' slots hold another entry since.
#[derive(Debug, Default)]
struct ReusedSlots {
    /// Every reuse since the table was made.
    count: u64,
    latest: std::collections::VecDeque<EntryIdx>,
}

impl EntryTable {
    pub(crate) fn new(ids: IdWidth) -> Self {
        Self {
            frn: match ids {
                IdWidth::Narrow => FrnColumn::Narrow(Vec::new()),
                IdWidth::Wide => FrnColumn::Wide(Vec::new()),
            },
            ..Self::default()
        }
    }

    /// Allocates a slot (reusing a freed one when available). The caller
    /// links the entry into its name's chain.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn alloc(
        &mut self,
        frn: Frn,
        parent: EntryIdx,
        name_id: NameId,
        flags: EntryFlags,
        size: u64,
        mtime: i64,
        attributes: PackedAttributes,
        next_link: EntryIdx,
    ) -> EntryIdx {
        self.live += 1;
        if let Some(idx) = self.free.pop() {
            if self.reused.latest.len() == REUSED_SLOTS_KEPT {
                self.reused.latest.pop_front();
            }
            self.reused.latest.push_back(idx);
            self.reused.count += 1;
            let i = idx as usize;
            self.frn.set(i, frn);
            self.parent[i] = parent;
            self.name_id[i] = name_id;
            self.flags[i] = flags;
            self.size[i] = size;
            self.mtime[i] = mtime;
            self.attributes[i] = attributes;
            self.next_link[i] = next_link;
            self.next_same[i] = NIL;
            self.prev_same[i] = NIL;
            self.first_child[i] = NIL;
            self.next_child[i] = NIL;
            self.prev_child[i] = NIL;
            idx
        } else {
            let idx = self.frn.len() as EntryIdx;
            self.frn.push(frn);
            push_eighth(&mut self.parent, parent);
            push_eighth(&mut self.name_id, name_id);
            push_eighth(&mut self.flags, flags);
            push_eighth(&mut self.size, size);
            push_eighth(&mut self.mtime, mtime);
            push_eighth(&mut self.attributes, attributes);
            push_eighth(&mut self.next_link, next_link);
            push_eighth(&mut self.next_same, NIL);
            push_eighth(&mut self.prev_same, NIL);
            push_eighth(&mut self.first_child, NIL);
            push_eighth(&mut self.next_child, NIL);
            push_eighth(&mut self.prev_child, NIL);
            idx
        }
    }

    /// Marks a slot dead and pushes it on the free list. The caller must have
    /// already unlinked the entry from its same-name chain and released its
    /// name.
    pub(crate) fn tombstone(&mut self, idx: EntryIdx) {
        let i = idx as usize;
        debug_assert!(!self.flags[i].contains(EntryFlags::TOMBSTONE));
        self.flags[i] = EntryFlags::TOMBSTONE;
        // A dead slot keeps no link into the tree or the name store: its old
        // parent may be dead too, and a compaction gives its old name id to
        // another name.
        self.parent[i] = NIL;
        self.name_id[i] = NIL;
        self.next_link[i] = NIL;
        self.next_same[i] = NIL;
        self.prev_same[i] = NIL;
        debug_assert!(
            self.first_child[i] == NIL,
            "tombstoning a directory that still has chained children"
        );
        self.first_child[i] = NIL;
        self.next_child[i] = NIL;
        self.prev_child[i] = NIL;
        self.free.push(idx);
        self.live -= 1;
    }

    #[inline]
    pub(crate) fn is_tombstone(&self, idx: EntryIdx) -> bool {
        self.flags[idx as usize].contains(EntryFlags::TOMBSTONE)
    }

    /// Number of live (non-tombstoned) entries, including root and lost+found.
    pub(crate) fn live(&self) -> usize {
        self.live
    }

    /// Total allocated slots (live + tombstoned-awaiting-reuse).
    pub(crate) fn capacity(&self) -> usize {
        self.frn.len()
    }

    /// Whether some file ID is wider than 64 bits, as every ReFS ID is.
    pub(crate) fn has_wide_ids(&self) -> bool {
        matches!(self.frn, FrnColumn::Wide(_))
    }

    /// Marks the slots reused so far, for [`Self::reused_since`].
    pub(crate) fn reuse_mark(&self) -> u64 {
        self.reused.count
    }

    /// The slots reused since `mark`, or `None` when they are too many to
    /// have been kept.
    pub(crate) fn reused_since(&self, mark: u64) -> Option<impl Iterator<Item = EntryIdx> + '_> {
        let since = usize::try_from(self.reused.count - mark).ok()?;
        (since <= self.reused.latest.len())
            .then(|| self.reused.latest.iter().rev().take(since).copied())
    }

    #[inline]
    pub(crate) fn frn(&self, idx: EntryIdx) -> Frn {
        self.frn.get(idx as usize)
    }
    #[inline]
    pub(crate) fn parent(&self, idx: EntryIdx) -> EntryIdx {
        self.parent[idx as usize]
    }
    #[inline]
    pub(crate) fn name_id(&self, idx: EntryIdx) -> NameId {
        self.name_id[idx as usize]
    }
    #[inline]
    pub(crate) fn flags(&self, idx: EntryIdx) -> EntryFlags {
        self.flags[idx as usize]
    }
    #[inline]
    pub(crate) fn size(&self, idx: EntryIdx) -> u64 {
        self.size[idx as usize]
    }
    #[inline]
    pub(crate) fn mtime(&self, idx: EntryIdx) -> i64 {
        self.mtime[idx as usize]
    }
    #[inline]
    pub(crate) fn attributes(&self, idx: EntryIdx) -> PackedAttributes {
        self.attributes[idx as usize]
    }
    #[inline]
    pub(crate) fn next_link(&self, idx: EntryIdx) -> EntryIdx {
        self.next_link[idx as usize]
    }
    #[inline]
    pub(crate) fn next_same(&self, idx: EntryIdx) -> EntryIdx {
        self.next_same[idx as usize]
    }
    #[inline]
    pub(crate) fn prev_same(&self, idx: EntryIdx) -> EntryIdx {
        self.prev_same[idx as usize]
    }
    #[inline]
    pub(crate) fn first_child(&self, idx: EntryIdx) -> EntryIdx {
        self.first_child[idx as usize]
    }
    #[inline]
    pub(crate) fn next_child(&self, idx: EntryIdx) -> EntryIdx {
        self.next_child[idx as usize]
    }
    #[inline]
    pub(crate) fn prev_child(&self, idx: EntryIdx) -> EntryIdx {
        self.prev_child[idx as usize]
    }

    #[inline]
    pub(crate) fn set_parent(&mut self, idx: EntryIdx, parent: EntryIdx) {
        self.parent[idx as usize] = parent;
    }
    #[inline]
    pub(crate) fn set_name_id(&mut self, idx: EntryIdx, id: NameId) {
        self.name_id[idx as usize] = id;
    }
    #[inline]
    pub(crate) fn set_flags(&mut self, idx: EntryIdx, flags: EntryFlags) {
        self.flags[idx as usize] = flags;
    }
    #[inline]
    pub(crate) fn set_size(&mut self, idx: EntryIdx, size: u64) {
        self.size[idx as usize] = size;
    }
    #[inline]
    pub(crate) fn set_mtime(&mut self, idx: EntryIdx, mtime: i64) {
        self.mtime[idx as usize] = mtime;
    }
    #[inline]
    pub(crate) fn set_attributes(&mut self, idx: EntryIdx, attributes: PackedAttributes) {
        self.attributes[idx as usize] = attributes;
    }
    #[inline]
    pub(crate) fn set_next_link(&mut self, idx: EntryIdx, next: EntryIdx) {
        self.next_link[idx as usize] = next;
    }
    #[inline]
    pub(crate) fn set_next_same(&mut self, idx: EntryIdx, next: EntryIdx) {
        self.next_same[idx as usize] = next;
    }
    #[inline]
    pub(crate) fn set_prev_same(&mut self, idx: EntryIdx, prev: EntryIdx) {
        self.prev_same[idx as usize] = prev;
    }
    #[inline]
    pub(crate) fn set_first_child(&mut self, idx: EntryIdx, child: EntryIdx) {
        self.first_child[idx as usize] = child;
    }
    #[inline]
    pub(crate) fn set_next_child(&mut self, idx: EntryIdx, next: EntryIdx) {
        self.next_child[idx as usize] = next;
    }
    #[inline]
    pub(crate) fn set_prev_child(&mut self, idx: EntryIdx, prev: EntryIdx) {
        self.prev_child[idx as usize] = prev;
    }

    /// Whether a removed entry left a slot that [`Self::compact`] would
    /// return.
    pub(crate) fn has_free_slots(&self) -> bool {
        !self.free.is_empty()
    }

    /// Moves every live entry down over the free slots, in order, and returns
    /// where each slot went: `remap[old]` is the new index, or [`NIL`] for a
    /// free slot. The columns that hold entry indexes are rewritten through
    /// it; the caller rewrites the indexes it holds elsewhere.
    pub(crate) fn compact(&mut self) -> Vec<EntryIdx> {
        let mut remap = vec![NIL; self.capacity()];
        let mut live = 0;
        for (old, new) in remap.iter_mut().enumerate() {
            if !self.is_tombstone(old as EntryIdx) {
                *new = live as EntryIdx;
                live += 1;
            }
        }
        let link = |idx| moved(&remap, idx);
        self.frn.compact(&remap, live);
        compact_column(&mut self.parent, &remap, live, link);
        compact_column(&mut self.name_id, &remap, live, |id| id);
        compact_column(&mut self.flags, &remap, live, |flags| flags);
        compact_column(&mut self.size, &remap, live, |size| size);
        compact_column(&mut self.mtime, &remap, live, |mtime| mtime);
        compact_column(&mut self.attributes, &remap, live, |a| a);
        compact_column(&mut self.next_link, &remap, live, link);
        compact_column(&mut self.next_same, &remap, live, link);
        compact_column(&mut self.prev_same, &remap, live, link);
        compact_column(&mut self.first_child, &remap, live, link);
        compact_column(&mut self.next_child, &remap, live, link);
        compact_column(&mut self.prev_child, &remap, live, link);
        self.free.clear();
        // The kept slots name slots that moved; no mark from before outlives
        // the compaction, because it gives the index a new instance.
        self.reused.latest.clear();
        remap
    }

    pub(crate) fn iter_live(&self) -> impl Iterator<Item = EntryIdx> + '_ {
        (0..self.frn.len() as EntryIdx).filter(|&i| !self.is_tombstone(i))
    }

    pub(crate) fn memory(&self) -> ComponentBytes {
        self.frn.memory()
            + ComponentBytes::of_vec(&self.parent)
            + ComponentBytes::of_vec(&self.name_id)
            + ComponentBytes::of_vec(&self.flags)
            + ComponentBytes::of_vec(&self.size)
            + ComponentBytes::of_vec(&self.mtime)
            + ComponentBytes::of_vec(&self.attributes)
            + ComponentBytes::of_vec(&self.next_link)
            + ComponentBytes::of_vec(&self.next_same)
            + ComponentBytes::of_vec(&self.prev_same)
            + ComponentBytes::of_vec(&self.first_child)
            + ComponentBytes::of_vec(&self.next_child)
            + ComponentBytes::of_vec(&self.prev_child)
            + ComponentBytes::of_vec(&self.free)
    }

    /// Returns growth slack to the OS, keeping headroom so live churn does not
    /// reallocate every column at once (see `NameStore::shrink_with_headroom`).
    /// Called once post-bootstrap.
    pub(crate) fn shrink_with_headroom(&mut self) {
        self.frn.shrink_with_headroom();
        shrink_vec(&mut self.parent);
        shrink_vec(&mut self.name_id);
        shrink_vec(&mut self.flags);
        shrink_vec(&mut self.size);
        shrink_vec(&mut self.mtime);
        shrink_vec(&mut self.attributes);
        shrink_vec(&mut self.next_link);
        shrink_vec(&mut self.next_same);
        shrink_vec(&mut self.prev_same);
        shrink_vec(&mut self.first_child);
        shrink_vec(&mut self.next_child);
        shrink_vec(&mut self.prev_child);
        shrink_vec(&mut self.free);
    }
}

/// How many records past twice its length a dense [`FrnMap`] still grows to
/// before [`FrnMap::set`] switches it to the sparse backing.
const DENSE_GROWTH_SLACK: u64 = 1 << 16;

/// Maps a file ID to its chain-head [`EntryIdx`].
///
/// On NTFS the map is keyed by the 48-bit MFT record number, not the full
/// ID, so slot reuse is detectable: the caller re-checks the stored entry's
/// full ID and, on a sequence mismatch, treats the slot as free. Bootstrap
/// builds the `Sparse` variant (record numbers arrive before their density is
/// known); [`FrnMap::densify_if_smaller`] swaps to `Dense` at publish when the
/// record space is compact. A ReFS ID has no record number, so a ReFS
/// volume's map is `Wide`, keyed by the whole ID, and the re-check always
/// finds the same ID.
#[derive(Debug)]
pub(crate) enum FrnMap {
    /// `Vec` indexed by record number; [`NIL`] marks an empty slot.
    Dense(Vec<EntryIdx>),
    /// Fallback for sparse / arbitrary record-number spaces.
    Sparse(FxHashMap<u64, EntryIdx>),
    /// Keyed by the whole 128-bit ID.
    Wide(FxHashMap<u128, EntryIdx>),
}

impl FrnMap {
    /// An empty map for IDs `ids` wide: `Wide` for wide IDs, and otherwise a
    /// record-number map, which starts sparse because it is safe for any
    /// record-number space.
    pub(crate) fn new(ids: IdWidth) -> Self {
        match ids {
            IdWidth::Wide => FrnMap::Wide(FxHashMap::default()),
            IdWidth::Narrow => FrnMap::Sparse(FxHashMap::default()),
        }
    }

    #[inline]
    pub(crate) fn get(&self, frn: Frn) -> Option<EntryIdx> {
        match self {
            FrnMap::Dense(v) => v
                .get(frn.record_number() as usize)
                .copied()
                .filter(|&i| i != NIL),
            FrnMap::Sparse(m) => m.get(&frn.record_number()).copied(),
            FrnMap::Wide(m) => m.get(&frn.0).copied(),
        }
    }

    /// Maps `frn` to `idx`. A dense map switches to the sparse backing for
    /// a record number far past its end: a real MFT grows a little at a time,
    /// but a corrupt journal record can carry any 48-bit number, and growing
    /// the `Vec` to it would abort the daemon on a failed allocation.
    #[inline]
    pub(crate) fn set(&mut self, frn: Frn, idx: EntryIdx) {
        let record = frn.record_number();
        if let FrnMap::Dense(v) = self
            && record > v.len() as u64 * 2 + DENSE_GROWTH_SLACK
        {
            let records = v.iter().enumerate().filter(|&(_, &i)| i != NIL);
            *self = FrnMap::Sparse(records.map(|(r, &i)| (r as u64, i)).collect());
        }
        match self {
            FrnMap::Dense(v) => {
                let r = record as usize;
                if r >= v.len() {
                    reserve_eighth(v, r + 1 - v.len());
                    v.resize(r + 1, NIL);
                }
                v[r] = idx;
            }
            FrnMap::Sparse(m) => {
                m.insert(record, idx);
            }
            FrnMap::Wide(m) => {
                m.insert(frn.0, idx);
            }
        }
    }

    /// Whether `a` and `b` take one place in the map: the same record
    /// number in a record-number map, the same ID in a wide one.
    pub(crate) fn same_key(&self, a: Frn, b: Frn) -> bool {
        match self {
            FrnMap::Dense(_) | FrnMap::Sparse(_) => a.record_number() == b.record_number(),
            FrnMap::Wide(_) => a == b,
        }
    }

    #[inline]
    pub(crate) fn remove(&mut self, frn: Frn) {
        match self {
            FrnMap::Dense(v) => {
                if let Some(slot) = v.get_mut(frn.record_number() as usize) {
                    *slot = NIL;
                }
            }
            FrnMap::Sparse(m) => {
                m.remove(&frn.record_number());
            }
            FrnMap::Wide(m) => {
                m.remove(&frn.0);
            }
        }
    }

    /// Switches a sparse map to the dense backing when a `Vec` indexed by
    /// record number would cost less than the hash map's bucket array.
    ///
    /// MFT record numbers are dense by construction (record N sits at byte
    /// N x 1024 of the MFT, and NTFS reuses freed records before extending),
    /// so on a real volume the record space is barely larger than the live
    /// file count and the dense form wins: 4 bytes per record versus 17 per
    /// hash bucket at power-of-two capacity. The size check keeps a
    /// pathological space (sparse snapshots, non-NTFS IDs) on the hash map.
    ///
    /// Called once per volume when it goes live: bootstrap has seen every
    /// record by then, so `max key` is the true bound, and `set` still grows
    /// the Vec if the journal later mints a higher record number.
    pub(crate) fn densify_if_smaller(&mut self) {
        let FrnMap::Sparse(m) = self else { return };
        let Some(&max) = m.keys().max() else { return };
        let dense_bytes = (u128::from(max) + 1) * size_of::<EntryIdx>() as u128;
        if dense_bytes < u128::from(hash_map_memory(m).allocated) {
            let mut v = vec![NIL; max as usize + 1];
            for (&record, &idx) in m.iter() {
                v[record as usize] = idx;
            }
            *self = FrnMap::Dense(v);
        }
    }

    /// Rewrites every entry through an entry compaction's `remap`
    /// ([`EntryTable::compact`]).
    pub(crate) fn remap(&mut self, remap: &[EntryIdx]) {
        match self {
            FrnMap::Dense(v) => v.iter_mut().for_each(|idx| *idx = moved(remap, *idx)),
            FrnMap::Sparse(m) => m.values_mut().for_each(|idx| *idx = moved(remap, *idx)),
            FrnMap::Wide(m) => m.values_mut().for_each(|idx| *idx = moved(remap, *idx)),
        }
    }

    /// A short label for diagnostics ("dense" / "sparse" / "wide").
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            FrnMap::Dense(_) => "dense",
            FrnMap::Sparse(_) => "sparse",
            FrnMap::Wide(_) => "wide",
        }
    }

    pub(crate) fn memory(&self) -> ComponentBytes {
        match self {
            FrnMap::Dense(v) => ComponentBytes::of_vec(v),
            FrnMap::Sparse(m) => hash_map_memory(m),
            FrnMap::Wide(m) => hash_map_memory(m),
        }
    }
}

/// Estimates a hash map's bytes. hashbrown allocates power-of-two bucket
/// arrays at 7/8 max load; estimate from the usable capacity it reports. Each
/// bucket holds a (key, value) pair, padded, plus 1 control byte. An estimate,
/// but within ~2x of truth is enough to rank the map against the other
/// components.
fn hash_map_memory<K>(m: &FxHashMap<K, EntryIdx>) -> ComponentBytes {
    let buckets = ((m.capacity() as u64 * 8) / 7).next_power_of_two();
    let per_bucket = (size_of::<(K, EntryIdx)>() + 1) as u64;
    ComponentBytes {
        used: m.len() as u64 * per_bucket,
        allocated: buckets * per_bucket,
    }
}
