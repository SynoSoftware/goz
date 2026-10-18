//! The query executor: compile a [`ParsedQuery`] into finders, scan the
//! index, apply scope and sort, and return a page of hits.
//!
//! The scan matches each candidate against the index's precomputed folded name
//! (case-insensitive queries) and tests the cheapest predicates first: kind and
//! size, then name substrings and wildcards, then ext, then (only for
//! survivors) scope ancestry and the full path. Full paths come from the
//! `paths` module, which builds each folder's path once per query, so a path
//! term costs one copy per candidate instead of a walk to the root.

use super::order::{OrderFields, compare};
use super::parse::{CodePointBuf, Filters, Kind, ParsedQuery, SizeRange};
use super::paths::{DirPaths, NameForm, PathMatcher};
use crate::index::VolumeIndex;
use crate::types::{EntryIdx, Frn, SortKey, SortSpec};
use memchr::memmem;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::borrow::Cow;

/// One matched entry. The daemon formats `mtime` for display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryHit {
    pub idx: EntryIdx,
    /// Full path: the volume's mount prefix, then every folder and the name
    /// (WTF-8).
    pub path: Vec<u8>,
    /// File ID, shared by every hard link of the file.
    pub frn: Frn,
    /// File ID of the folder that holds this name.
    pub parent_frn: Frn,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
}

/// Result of a query: the requested page plus the full match count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryOutcome {
    /// Total matches before offset/limit (es `totitems` semantics).
    pub total: u64,
    /// The requested page of hits, ordered per [`Self::sorted`].
    pub hits: Vec<QueryHit>,
    /// Whether [`Self::hits`] is already in result order.
    ///
    /// `false` means the executor declined to order under the index lock and
    /// the CALLER must order the hits with [`super::order_hits`].
    /// Always `true` from [`run_query`], always `false` from
    /// [`run_query_unsorted`]; only [`run_query_deferrable`] varies.
    pub sorted: bool,
}

/// A [`ParsedQuery`]'s name needles prebuilt into finders once (rather than
/// per candidate). The remaining name predicates (wildcards, filters) are read
/// straight off the `ParsedQuery` in the scan loop.
pub struct CompiledQuery<'a> {
    name_finders: Vec<memmem::Finder<'a>>,
    /// The longest literal substring guaranteed present in any match's name,
    /// drawn from the name terms, from name wildcard literals (e.g. `.dll` of
    /// `*.dll`), and from a single `ext:` value (`.ext`). Drives the
    /// folded-arena prefilter scan. `None` when nothing offers a usable
    /// literal (then the query iterates every entry).
    ///
    /// Path terms and path wildcards never supply it: their text can lie in
    /// any folder of the path, and the literal of `*\docs\*` is `\docs\`,
    /// which no name contains.
    prefilter: Option<Vec<u8>>,
}

impl<'a> CompiledQuery<'a> {
    pub fn new(parsed: &'a ParsedQuery) -> Self {
        // The prefilter is the longest literal from any name term, wildcard, or
        // single `ext:` value;
        // every match must contain it, so scanning for it yields a superset of
        // matches that `evaluate` then verifies.
        // Borrow name terms and move owned wildcard/ext literals, materializing
        // exactly one owned Vec at the end instead of cloning every name term
        // and discarding all but the longest.
        let mut prefilter: Option<Cow<'a, [u8]>> = None;
        let mut consider = |cand: Cow<'a, [u8]>| {
            if cand.len() > prefilter.as_ref().map_or(0, |c| c.len()) {
                prefilter = Some(cand);
            }
        };
        for term in &parsed.name_terms {
            consider(Cow::Borrowed(term.as_slice()));
        }
        for w in &parsed.wildcards {
            if let Some(lit) = w.longest_literal() {
                consider(Cow::Owned(lit));
            }
        }
        // A single `ext:` value contributes `.ext`, which every match's name
        // contains, so an ext-only query scans the folded arena for `.ext`
        // instead of iterating every entry. Multiple extensions share no single
        // literal, so they keep the slow path.
        if let Some(exts) = &parsed.filters.ext
            && exts.len() == 1
            && !exts[0].is_empty()
        {
            let mut lit = Vec::with_capacity(exts[0].len() + 1);
            lit.push(b'.');
            lit.extend_from_slice(&exts[0]);
            consider(Cow::Owned(lit));
        }

        Self {
            name_finders: parsed
                .name_terms
                .iter()
                .map(|t| memmem::Finder::new(t.as_slice()))
                .collect(),
            prefilter: prefilter.map(Cow::into_owned),
        }
    }
}

/// A matched entry before its path is built (deferred to the final page).
struct Candidate {
    idx: EntryIdx,
    is_dir: bool,
    size: Option<u64>,
    mtime: Option<i64>,
    /// Where the folded path of the parent folder lies in the path sort's
    /// buffer; filled only for a path sort.
    folded_parent: (usize, usize),
}

/// Runs a query over `index`, the volume mounted at `mount_prefix` (`C:\`).
///
/// `scope` (if any) restricts results to descendants of that directory entry;
/// the daemon resolves the `-path` folder via [`resolve_scope`]. Results are
/// sorted per `sort`, then `offset`/`limit` select the page; `total` reports
/// the full match count.
///
/// The common case, a case-insensitive query with at least one substring
/// term, scans the index's contiguous folded-name arena with a single
/// prebuilt `memmem` finder (SIMD over the whole buffer), maps each hit back to
/// its entry, and defers path reconstruction to the final page. That keeps the
/// hot path off per-query folding and off per-match path building.
pub fn run_query(
    index: &VolumeIndex,
    mount_prefix: &str,
    parsed: &ParsedQuery,
    scope: Option<EntryIdx>,
    sort: SortSpec,
    offset: u32,
    limit: Option<u32>,
) -> QueryOutcome {
    run_query_impl(
        index,
        mount_prefix,
        parsed,
        scope,
        Some(sort),
        offset,
        limit,
        false,
    )
}

/// Like [`run_query`], but allowed to hand back an UNSORTED page when ordering
/// under the index lock would buy nothing.
///
/// Ordering inside the lock earns its keep only when the page is a strict slice
/// of the match set: picking which `limit` matches survive needs the index, and
/// only that many paths are then reconstructed. When the page already covers
/// every match there is nothing to pick, so this returns the matches in scan
/// order with [`QueryOutcome::sorted`] set to `false` and leaves the ordering to
/// the caller, which can do it after releasing the lock.
///
/// That distinction is not knowable before the scan (it depends on the match
/// count), which is why it lives here rather than in the caller's dispatch.
///
/// The caller MUST check [`QueryOutcome::sorted`] and pass the hits through
/// [`super::order_hits`] when it is `false`.
pub fn run_query_deferrable(
    index: &VolumeIndex,
    mount_prefix: &str,
    parsed: &ParsedQuery,
    scope: Option<EntryIdx>,
    sort: SortSpec,
    offset: u32,
    limit: Option<u32>,
) -> QueryOutcome {
    run_query_impl(
        index,
        mount_prefix,
        parsed,
        scope,
        Some(sort),
        offset,
        limit,
        true,
    )
}

/// Every match, with paths built, in scan order. The CALLER sorts.
///
/// For a full export (`limit: None`) the caller receives every match anyway, so
/// ordering is pure post-processing that needs no index at all: every key of
/// the result order is in the returned [`QueryHit`].
///
/// It matters because of who is waiting. The caller holds the volume's index
/// READ lock for as long as this runs, and the volume's tail thread cannot apply
/// a single journal record until that lock is released. Sorting a million rows
/// inside the lock stalled live updates for ~1.7 s; sorting them after it is
/// released stalls nothing.
///
/// The caller MUST pass the result through [`super::order_hits`].
pub fn run_query_unsorted(
    index: &VolumeIndex,
    mount_prefix: &str,
    parsed: &ParsedQuery,
    scope: Option<EntryIdx>,
) -> QueryOutcome {
    run_query_impl(index, mount_prefix, parsed, scope, None, 0, None, false)
}

/// A folder-scoped query walks the scope's subtree instead of scanning the
/// whole volume, up to this many visited entries. Past it (a near-volume
/// scope), the walk aborts and the ordinary scan+ancestry-filter path runs:
/// pointer-chasing millions of entries is slower than the SIMD arena scan.
/// The cutoff makes a Downloads-sized scope independent of disk size: the
/// old path cost ~300 ms on a broad query over 3.9M entries because every
/// candidate on the volume paid an ancestry check; the walk touches only
/// the folder.
const SUBTREE_WALK_MAX: usize = 200_000;

/// `sort: None` skips ordering entirely and returns every match.
///
/// `allow_defer` lets the executor drop the ordering when the page turns out to
/// cover every match (see [`run_query_deferrable`]); the reported
/// [`QueryOutcome::sorted`] says which happened.
#[allow(clippy::too_many_arguments)]
fn run_query_impl(
    index: &VolumeIndex,
    mount_prefix: &str,
    parsed: &ParsedQuery,
    scope: Option<EntryIdx>,
    sort: Option<SortSpec>,
    offset: u32,
    limit: Option<u32>,
    allow_defer: bool,
) -> QueryOutcome {
    let mut scan = Scan::new(index, mount_prefix, parsed);

    // Folder scope: walk the scope subtree via the child chains, so cost
    // tracks the folder, not the volume. Membership is by construction, so
    // the candidates are tested without the scope (its ancestry walk is the
    // expensive part this path exists to avoid); every other predicate is
    // identical to the scan paths below. Oversized scopes abort to those paths.
    let mut subtree_done = false;
    if let Some(root) = scope {
        let mut visited = 0usize;
        subtree_done = true;
        for idx in index.subtree_entries(root) {
            visited += 1;
            if visited > SUBTREE_WALK_MAX {
                scan.candidates.clear();
                subtree_done = false;
                break;
            }
            if index.is_synthetic(idx) {
                continue;
            }
            scan.visit(idx, None);
        }
    }

    // Fast path: case-insensitive query with a usable literal → scan the
    // folded arena for the prefilter literal and verify the rest per hit.
    let prefilter = scan.compiled.prefilter.take();
    if subtree_done {
        // Scoped walk answered the query; the scan paths are skipped.
    } else if !parsed.match_case
        && let Some(needle) = prefilter.as_deref()
    {
        let driver = memmem::Finder::new(needle);
        // The haystack holds each distinct name ONCE (names are interned), so
        // the scan cost is proportional to the unique-name bytes (~1/3 of the
        // total on a real volume) and a hit fans out to every entry bearing
        // the name through its same-name chain. The chains are exact, so no
        // per-candidate staleness guard is needed.
        let hay = index.folded_bytes();
        let pairs = index.name_pairs();
        // Merge-walk: both the `memmem` hits and `pairs[id].folded.off` are
        // ascending, so a forward cursor maps each hit to its name id in
        // amortized O(1) with no per-hit binary search (which cache-thrashes
        // at scale). The first hit seeds the cursor with one binary search so
        // a rare term whose matches start deep in the arena doesn't pay a
        // full-length forward scan.
        let mut cursor = 0usize;
        let mut seeded = false;
        // A name is a contiguous NUL-separated region, so repeated hits in
        // one name are adjacent: a single "same as last" check dedups them.
        let mut last_id = u32::MAX;
        for pos in driver.find_iter(hay) {
            let pos = pos as u32;
            if pairs.is_empty() {
                break;
            }
            if !seeded {
                cursor = pairs
                    .partition_point(|p| p.folded.off <= pos)
                    .saturating_sub(1);
                seeded = true;
            } else {
                while cursor + 1 < pairs.len() && pairs[cursor + 1].folded.off <= pos {
                    cursor += 1;
                }
            }
            let id = cursor as u32;
            if id == last_id {
                continue; // driver term appears twice in one name
            }
            last_id = id;
            for idx in index.name_chain(id) {
                if index.is_synthetic(idx) {
                    continue;
                }
                scan.visit(idx, scope);
            }
        }
    } else {
        // Slow path: case-sensitive, or no positive substring term (only
        // wildcards / filters / path terms). Iterate every live entry.
        for idx in index.live_entries() {
            if index.is_synthetic(idx) {
                continue;
            }
            scan.visit(idx, scope);
        }
    }

    let Scan {
        mut candidates,
        mut dirs,
        ..
    } = scan;
    let total = candidates.len() as u64;

    let start = (offset as usize).min(candidates.len());
    let end = match limit {
        Some(n) => (start + n as usize).min(candidates.len()),
        None => candidates.len(),
    };

    // A page that covers every match has nothing to select, so ordering it here
    // buys nothing the caller cannot do itself once it has released the index
    // lock. Dropping the sort in that case is the whole point of
    // `run_query_deferrable`: see `run_query_unsorted` for who is waiting on
    // that lock. `end` is already clamped to the match count, so this is exactly
    // "the page is the whole set".
    //
    // Everything below keys off the rebound `sort`, so a deferred query also
    // skips the path-key precompute it would never consult.
    let sort = match sort {
        Some(s) if !(allow_defer && end == candidates.len()) => Some(s),
        _ => None,
    };
    let sorted = sort.is_some();

    // Sort-by-path compares each candidate's folded parent path. Siblings share
    // it, so each parent folder's path is built and stored once.
    let mut folded_parents = Vec::new();
    if sort.is_some_and(|s| matches!(s.key, SortKey::Path)) {
        let mut folded_dirs = DirPaths::new(index, NameForm::Folded, mount_prefix);
        let mut stored: FxHashMap<EntryIdx, (usize, usize)> = FxHashMap::default();
        let mut path = Vec::new();
        for c in &mut candidates {
            let parent = index.entry(c.idx).parent();
            c.folded_parent = *stored.entry(parent).or_insert_with(|| {
                // A parent with no path leaves the key empty; the page drops
                // the candidate.
                folded_dirs.folder_path_into(index, parent, &mut path);
                let start = folded_parents.len();
                folded_parents.extend_from_slice(&path);
                (start, folded_parents.len())
            });
        }
    }

    // Order only enough to fill the page: quickselect the top `end` by the
    // result order (O(M)), then sort just that prefix (O(end log end)). This
    // avoids an O(M log M) sort of the entire match set on common queries.
    if let Some(sort) = sort {
        order_top(index, &folded_parents, &mut candidates, sort, end);
    }

    // Build paths only for the requested page. A broad result set holds
    // millions of files under far fewer distinct folders, and `dirs` builds
    // each folder's path once.
    let mut hits = Vec::with_capacity(end - start);
    for c in &candidates[start..end] {
        let mut path = Vec::new();
        if !dirs.full_path_into(index, c.idx, &mut path) {
            continue; // no path; only possible when `has_orphans` was false
        }
        let entry = index.entry(c.idx);
        hits.push(QueryHit {
            idx: c.idx,
            path,
            frn: entry.frn(),
            parent_frn: index.entry(entry.parent()).frn(),
            is_dir: c.is_dir,
            size: c.size,
            mtime: c.mtime,
        });
    }

    QueryOutcome {
        total,
        hits,
        sorted,
    }
}

/// The state one scan shares across its candidates: finders, memos and scratch
/// buffers, built once per query instead of once per candidate.
struct Scan<'q, 'i> {
    index: &'i VolumeIndex,
    parsed: &'q ParsedQuery,
    compiled: CompiledQuery<'q>,
    /// `None` when the query has no path term.
    paths: Option<PathMatcher<'q>>,
    /// Raw folder paths. ONE memo, shared by the presentability check and the
    /// page builder: both need exactly these paths.
    dirs: DirPaths,
    scope_memo: FxHashMap<EntryIdx, bool>,
    code_points: CodePointBuf,
    ext_scratch: Vec<u8>,
    candidates: Vec<Candidate>,
}

impl<'q, 'i> Scan<'q, 'i> {
    fn new(index: &'i VolumeIndex, mount_prefix: &str, parsed: &'q ParsedQuery) -> Self {
        Self {
            index,
            parsed,
            compiled: CompiledQuery::new(parsed),
            paths: PathMatcher::new(index, parsed, mount_prefix),
            dirs: DirPaths::new(index, NameForm::Raw, mount_prefix),
            scope_memo: FxHashMap::default(),
            code_points: CodePointBuf::new(),
            ext_scratch: Vec::new(),
            candidates: Vec::new(),
        }
    }

    /// Keeps entry `idx` as a candidate if it passes every predicate.
    fn visit(&mut self, idx: EntryIdx, scope: Option<EntryIdx>) {
        if let Some(c) = self.evaluate(idx, scope) {
            self.candidates.push(c);
        }
    }

    fn evaluate(&mut self, idx: EntryIdx, scope: Option<EntryIdx>) -> Option<Candidate> {
        let index = self.index;
        let parsed = self.parsed;
        let entry = index.entry(idx);
        if !kind_matches(parsed.filters.kind, entry.is_dir()) {
            return None;
        }
        if !size_matches(parsed.filters.size, entry.size()) {
            return None;
        }

        // Name/wildcard predicates run against the precomputed folded name
        // (case-insensitive) or the raw name (case-sensitive), matching how the
        // needles were folded at parse time.
        let hay_name = if parsed.match_case {
            entry.name()
        } else {
            index.folded_name(idx)
        };
        if !self
            .compiled
            .name_finders
            .iter()
            .all(|f| f.find(hay_name).is_some())
        {
            return None;
        }
        if !parsed
            .wildcards
            .iter()
            .all(|w| w.matches_into(hay_name, &mut self.code_points))
        {
            return None;
        }
        // The ext check folds the candidate extension, so it runs after the
        // allocation-free name/wildcard predicates that reject most candidates.
        if !ext_matches(&parsed.filters, entry.name(), &mut self.ext_scratch) {
            return None;
        }

        if let Some(root) = scope
            && !in_scope(index, idx, root, &mut self.scope_memo)
        {
            return None;
        }

        // An entry whose parent has no path (ancestry ends in lost+found) is
        // dropped by the page loop. Reject it here so it never counts toward
        // `total` and never occupies a page slot: otherwise `gozler -n 10 report`
        // can print nothing while reporting a non-zero total, and the same
        // query without `-n` prints the real hits.
        //
        // Asks the very memo the page loop uses rather than re-deriving its
        // rules, so the two cannot drift. Gated on `has_orphans` because an
        // index with nothing under lost+found cannot contain an unpresentable
        // entry: `has_orphans` is a single load, while the walk it guards is
        // one path build per distinct parent folder, which on a broad limited
        // query is tens of thousands of builds the page (100 rows) would never
        // have needed. Paying it unconditionally cost a broad limited query
        // ~20%.
        if index.has_orphans() && !self.dirs.entry_has_path(index, idx) {
            return None;
        }

        if let Some(paths) = &mut self.paths
            && !paths.matches(index, idx)
        {
            return None;
        }

        Some(Candidate {
            idx,
            is_dir: entry.is_dir(),
            size: entry.size(),
            mtime: entry.mtime(),
            folded_parent: (0, 0),
        })
    }
}

fn kind_matches(kind: Option<Kind>, is_dir: bool) -> bool {
    match kind {
        None => true,
        Some(Kind::File) => !is_dir,
        Some(Kind::Folder) => is_dir,
    }
}

/// A size filter excludes entries whose size is unknown (not yet enriched):
/// we cannot confirm the predicate, so we do not claim a match.
fn size_matches(filter: Option<SizeRange>, size: Option<u64>) -> bool {
    match filter {
        None => true,
        Some(range) => size.is_some_and(|s| range.contains(s)),
    }
}

/// Extension matching is always case-insensitive (es semantics): the filter
/// values are folded at parse time, so the candidate extension is folded here
/// unconditionally, even under a case-sensitive query.
fn ext_matches(filters: &Filters, name: &[u8], scratch: &mut Vec<u8>) -> bool {
    let Some(exts) = &filters.ext else {
        return true;
    };
    let Some(ext) = extension(name) else {
        return false; // no extension → cannot match an ext: filter
    };
    scratch.clear();
    crate::fold::fold_into(ext, scratch);
    exts.iter().any(|e| e.as_slice() == scratch.as_slice())
}

/// The extension of an entry's name, folder or file: what follows its last
/// `.`, or `None` when the name has no dot. `ext:` matches it, and the
/// Extension column prints it.
pub fn extension(name: &[u8]) -> Option<&[u8]> {
    let dot = name.iter().rposition(|&b| b == b'.')?;
    Some(&name[dot + 1..])
}

fn fold(bytes: &[u8]) -> Vec<u8> {
    crate::fold::fold(bytes)
}

/// Is `node` a descendant of `root`? Memoized parent walk.
fn in_scope(
    index: &VolumeIndex,
    node: EntryIdx,
    root: EntryIdx,
    memo: &mut FxHashMap<EntryIdx, bool>,
) -> bool {
    // A scope excludes the folder itself (es: descendants only).
    if node == root {
        return false;
    }
    // Mirrors `VolumeIndex::path_of`: directory depths under 32 stay on the
    // stack, so the per-candidate ancestry walk doesn't heap-allocate.
    let mut chain: SmallVec<[EntryIdx; 32]> = SmallVec::new();
    let mut cur = node;
    let verdict = loop {
        if cur == root {
            break true;
        }
        if let Some(&v) = memo.get(&cur) {
            break v;
        }
        let parent = index.entry(cur).parent();
        if parent == crate::types::NIL {
            break false;
        }
        chain.push(cur);
        cur = parent;
        if chain.len() > 4096 {
            break false; // cycle guard
        }
    };
    for e in chain {
        memo.insert(e, verdict);
    }
    verdict
}

/// A candidate as the result order reads it: straight from the index, so a
/// tie-break key is loaded only when a comparison reaches it.
struct Ranked<'a> {
    index: &'a VolumeIndex,
    folded_parents: &'a [u8],
    candidate: &'a Candidate,
}

impl OrderFields for Ranked<'_> {
    fn folded_parent(&self) -> &[u8] {
        let (start, end) = self.candidate.folded_parent;
        &self.folded_parents[start..end]
    }
    fn folded_name(&self) -> &[u8] {
        self.index.folded_name(self.candidate.idx)
    }
    fn size(&self) -> Option<u64> {
        self.candidate.size
    }
    fn mtime(&self) -> Option<i64> {
        self.candidate.mtime
    }
    fn volume(&self) -> &[u8] {
        b"" // one volume per query run
    }
    fn frn(&self) -> Frn {
        self.index.entry(self.candidate.idx).frn()
    }
    fn parent_frn(&self) -> Frn {
        let parent = self.index.entry(self.candidate.idx).parent();
        self.index.entry(parent).frn()
    }
    fn name(&self) -> &[u8] {
        self.index.entry(self.candidate.idx).name()
    }
}

/// Places the top `end` candidates (by `sort`) in `cands[..end]`, sorted. Uses
/// quickselect to partition when `end` is a small slice of a large match set.
fn order_top(
    index: &VolumeIndex,
    folded_parents: &[u8],
    cands: &mut [Candidate],
    sort: SortSpec,
    end: usize,
) {
    if end == 0 {
        return;
    }
    let cmp = |a: &Candidate, b: &Candidate| {
        let ranked = |candidate| Ranked {
            index,
            folded_parents,
            candidate,
        };
        compare(sort, &ranked(a), &ranked(b))
    };
    // The result order is total, so stability is irrelevant: unstable sort
    // avoids driftsort's scratch buffer and uses the faster ipnsort.
    //
    // A broad query with no limit sorts the whole match set (millions of
    // candidates); that is done in parallel across cores. Quickselect + small
    // sorts stay serial: they touch far less data and the rayon fan-out is not
    // worth it there.
    use rayon::slice::ParallelSliceMut;
    const PARALLEL_SORT_MIN: usize = 100_000;
    if end < cands.len() {
        cands.select_nth_unstable_by(end - 1, cmp);
        // The selected prefix is not always small: a page asking for all but a
        // handful of matches selects nearly everything, and ordering that
        // serially held the index read lock ~6x longer than the parallel branch
        // below does for the same row count, stalling the volume's tail thread
        // for the difference. Same threshold, same work, just not on one core.
        if end >= PARALLEL_SORT_MIN {
            cands[..end].par_sort_unstable_by(cmp);
        } else {
            cands[..end].sort_unstable_by(cmp);
        }
    } else if cands.len() >= PARALLEL_SORT_MIN {
        cands.par_sort_unstable_by(cmp);
    } else {
        cands.sort_unstable_by(cmp);
    }
}

/// Resolves a scope folder path (WTF-8, no volume prefix) to its entry,
/// walking components from the volume root: case-insensitively via the folded
/// directory index, or case-sensitively against raw names when `match_case`. Returns `None`
/// if any component is missing.
pub fn resolve_scope(index: &VolumeIndex, path: &[u8], match_case: bool) -> Option<EntryIdx> {
    let components: Vec<&[u8]> = path
        .split(|&b| b == b'\\' || b == b'/')
        .filter(|c| !c.is_empty())
        .collect();
    if components.is_empty() {
        return Some(index.root());
    }

    // Common case: walk the persistent directory index, O(components). It is
    // keyed by folded name, so it serves case-insensitive scopes directly with
    // no per-query scan of the volume.
    if !match_case {
        let mut cur = index.root();
        for comp in components {
            // Move the folded bytes into the lookup key instead of re-cloning
            // them inside `child_dir` (which previously took `&[u8]`).
            cur = index.child_dir(cur, fold(comp))?;
        }
        return Some(cur);
    }

    // Case-sensitive scopes are rare (the persistent index is folded), so we do
    // not build a whole-volume raw-name map (one key Vec per directory) per
    // query. Walk each component with a linear scan of the live entries,
    // comparing raw names directly (no allocation), stopping at the first
    // matching sibling. NOTE: do not reuse the folded `child_dir` walk with a
    // post-hoc raw-name check: `dir_children` keeps a single entry per
    // (parent, folded name), so it cannot represent case-variant siblings and
    // would miss a legitimately case-matching sibling that folding collapsed.
    let mut cur = index.root();
    'components: for comp in components {
        for idx in index.live_entries() {
            if index.is_synthetic(idx) {
                continue;
            }
            let e = index.entry(idx);
            if e.is_dir() && e.parent() == cur && e.name() == comp {
                cur = idx;
                continue 'components;
            }
        }
        return None;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::store::FrnMap;
    use crate::index::{NTFS_ROOT_FRN, VolumeIndex};
    use crate::query::order::{VolumeRun, order_hits};
    use crate::query::parse::parse_query;
    use crate::types::{Frn, SortDir};
    use crate::usn::record::{FILE_ATTRIBUTE_DIRECTORY, ParsedUsnRecord, USN_REASON_FILE_CREATE};

    fn frn(rec: u64) -> Frn {
        Frn(rec | (1u64 << 48))
    }

    fn enum_rec(f: Frn, parent: Frn, name: &str, is_dir: bool) -> ParsedUsnRecord {
        ParsedUsnRecord {
            major_version: 2,
            frn: f,
            parent_frn: parent,
            usn: 0,
            timestamp_ft: 0,
            reason: USN_REASON_FILE_CREATE,
            attributes: if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
            name: name.as_bytes().to_vec(),
            name_lossy: false,
        }
    }

    /// Builds: docs/ (100), docs/sub/ (101), docs/sub/Report.PDF (102, 4KB),
    /// docs/notes.txt (103, 200B), photo.JPG (104, 2MB) at root.
    fn sample_index() -> VolumeIndex {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
        let root = NTFS_ROOT_FRN;
        for r in [
            enum_rec(frn(100), root, "docs", true),
            enum_rec(frn(101), frn(100), "sub", true),
            enum_rec(frn(102), frn(101), "Report.PDF", false),
            enum_rec(frn(103), frn(100), "notes.txt", false),
            enum_rec(frn(104), root, "photo.JPG", false),
        ] {
            idx.insert_enum(&r);
        }
        // Enrich sizes via the layout path.
        set_size(&mut idx, frn(102), 4096);
        set_size(&mut idx, frn(103), 200);
        set_size(&mut idx, frn(104), 2 * 1024 * 1024);
        idx
    }

    /// An orphan: a real file whose parent was never seen, so the parent is a
    /// placeholder under lost+found and the file has no presentable path.
    /// Reachable whenever a genuine gap exists (journal wrap, missed parent
    /// record, FRN sequence mismatch), and densest during bootstrap and rescan,
    /// which are exactly the windows the server still queries.
    fn index_with_orphans() -> VolumeIndex {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
        let root = NTFS_ROOT_FRN;
        // Two presentable hits.
        idx.insert_enum(&enum_rec(frn(100), root, "real1.log", false));
        idx.insert_enum(&enum_rec(frn(101), root, "real2.log", false));
        // Three orphans: parent 999 is never inserted, so it is synthesized as a
        // placeholder under lost+found and path_of on it is not Ok.
        for (f, n) in [
            (200, "ghost1.log"),
            (201, "ghost2.log"),
            (202, "ghost3.log"),
        ] {
            idx.insert_enum(&enum_rec(frn(f), frn(999), n, false));
        }
        idx
    }

    /// `total` and the page must count the same things.
    ///
    /// The page loop drops any hit whose parent has no presentable path, but
    /// `total` was counted before that and the orphans still occupied page slots.
    /// So an unlimited query returned the 2 real hits while a limited one spent
    /// its whole page on orphans and returned NOTHING, with total claiming 5.
    /// Silent: the user sees an empty result for a file that exists.
    #[test]
    fn orphans_are_excluded_from_total_and_never_eat_the_page() {
        let idx = index_with_orphans();
        let parsed = parse_query("log").unwrap();

        let all = run_query(
            &idx,
            "",
            &parsed,
            None,
            SortSpec::default_for(SortKey::Name),
            0,
            None,
        );
        assert_eq!(all.hits.len(), 2, "only the presentable files are hits");
        assert_eq!(
            all.total, 2,
            "total must count what the page can actually return, not orphans"
        );

        // The failing case: a limit small enough that orphans would have filled it.
        for key in [SortKey::Name, SortKey::Path] {
            let page = run_query(
                &idx,
                "",
                &parsed,
                None,
                SortSpec::default_for(key),
                0,
                Some(3),
            );
            assert_eq!(
                page.hits.len(),
                2,
                "{key:?}: a limited query must still return the real hits"
            );
            assert_eq!(
                page.total, 2,
                "{key:?}: total must not count dropped orphans"
            );
        }
    }

    fn set_size(idx: &mut VolumeIndex, f: Frn, size: u64) {
        use crate::layout::{LayoutFile, LayoutName};
        // Reuse the existing name/parent by reading them back.
        let head = idx.head_of(f).unwrap();
        let e = idx.entry(head);
        let name = e.name().to_vec();
        let parent_frn = idx.entry(e.parent()).frn();
        idx.enrich(&LayoutFile {
            frn: f,
            attributes: 0x20,
            size: Some(size),
            mtime_ft: Some(1000),
            names: vec![LayoutName {
                parent_frn,
                name,
                name_lossy: false,
                dos_only: false,
            }],
        });
    }

    fn run(idx: &VolumeIndex, query: &str) -> Vec<String> {
        let parsed = parse_query(query).unwrap();
        let out = run_query(idx, "", &parsed, None, SortSpec::default(), 0, None);
        out.hits
            .iter()
            .map(|h| crate::wtf8::to_string_lossy(&h.path))
            .collect()
    }

    #[test]
    fn case_insensitive_substring_over_names() {
        let idx = sample_index();
        assert_eq!(run(&idx, "report"), vec!["docs\\sub\\Report.PDF"]);
        assert_eq!(run(&idx, "REPORT"), vec!["docs\\sub\\Report.PDF"]);
    }

    #[test]
    fn multi_term_and() {
        let idx = sample_index();
        // Both terms must be substrings of the same name.
        assert!(run(&idx, "notes zzz").is_empty()); // "zzz" is in no name
        assert_eq!(run(&idx, "notes txt"), vec!["docs\\notes.txt"]);
    }

    #[test]
    fn ext_filter() {
        let idx = sample_index();
        assert_eq!(run(&idx, "ext:pdf"), vec!["docs\\sub\\Report.PDF"]);
        let mut jpg = run(&idx, "ext:jpg");
        jpg.sort();
        assert_eq!(jpg, vec!["photo.JPG"]);
    }

    #[test]
    fn size_filter_excludes_unknown_and_out_of_range() {
        let idx = sample_index();
        // > 1MB → only the 2MB photo.
        assert_eq!(run(&idx, "size:>1mb"), vec!["photo.JPG"]);
        // tiny (≤10KB) → notes (200B) and report (4KB), not the 2MB photo.
        let mut tiny = run(&idx, "size:tiny");
        tiny.sort();
        assert_eq!(tiny, vec!["docs\\notes.txt", "docs\\sub\\Report.PDF"]);
    }

    #[test]
    fn kind_filters() {
        let idx = sample_index();
        let mut folders = run(&idx, "folder:");
        folders.sort();
        assert_eq!(folders, vec!["docs", "docs\\sub"]);
    }

    #[test]
    fn wildcard_whole_name() {
        let idx = sample_index();
        assert_eq!(run(&idx, "*.pdf"), vec!["docs\\sub\\Report.PDF"]);
        assert!(run(&idx, "*.zip").is_empty());
    }

    #[test]
    fn path_term_matches_full_path() {
        let idx = sample_index();
        let mut hits = run(&idx, "docs\\sub");
        hits.sort();
        // Every entry whose full path contains "docs\sub".
        assert_eq!(hits, vec!["docs\\sub", "docs\\sub\\Report.PDF"]);
    }

    #[test]
    fn scope_restricts_to_descendants() {
        let idx = sample_index();
        let parsed = parse_query("").unwrap();
        let scope = resolve_scope(&idx, b"docs\\sub", false).unwrap();
        let out = run_query(&idx, "", &parsed, Some(scope), SortSpec::default(), 0, None);
        let paths: Vec<String> = out
            .hits
            .iter()
            .map(|h| crate::wtf8::to_string_lossy(&h.path))
            .collect();
        assert_eq!(paths, vec!["docs\\sub\\Report.PDF"]); // descendants only, not "docs\sub" itself
    }

    #[test]
    fn sort_path_orders_by_parent_then_name() {
        let idx = sample_index();
        let parsed = parse_query("").unwrap();
        let out = run_query(
            &idx,
            "",
            &parsed,
            None,
            SortSpec {
                key: SortKey::Path,
                dir: SortDir::Asc,
            },
            0,
            None,
        );
        let paths: Vec<String> = out
            .hits
            .iter()
            .map(|h| crate::wtf8::to_string_lossy(&h.path))
            .collect();
        // Root-level entries (empty parent) sort first, then docs\, then docs\sub\.
        assert_eq!(
            paths,
            vec![
                "docs",
                "photo.JPG",
                "docs\\notes.txt",
                "docs\\sub",
                "docs\\sub\\Report.PDF",
            ]
        );
    }

    #[test]
    fn offset_and_limit_paginate_but_total_is_full_count() {
        let idx = sample_index();
        let parsed = parse_query("").unwrap();
        let out = run_query(
            &idx,
            "",
            &parsed,
            None,
            SortSpec {
                key: SortKey::Name,
                dir: SortDir::Asc,
            },
            1,
            Some(2),
        );
        assert_eq!(out.total, 5);
        assert_eq!(out.hits.len(), 2);
    }

    /// Ordering under the index lock earns its keep only when the limit really
    /// slices the match set: picking the survivors needs the index, and only
    /// that many paths get reconstructed. A page that already covers every
    /// match has nothing to pick, so the executor must hand the ordering back
    /// rather than do it while the caller holds the lock.
    ///
    /// Regression: the daemon chose by `limit.is_none()`, so a client asking for
    /// everything as a NUMBER (`-n 4294967295`, or any limit at or above the
    /// match count) took the in-lock path and sorted the entire set with the
    /// volume's tail thread blocked behind it. The protection was one number
    /// away from being skipped.
    #[test]
    fn a_page_covering_every_match_defers_the_sort() {
        let idx = sample_index();
        let parsed = parse_query("").unwrap();
        let sort = SortSpec::default_for(SortKey::Name);
        let total = run_query(&idx, "", &parsed, None, sort, 0, None).total as u32;
        assert!(total > 1, "fixture must have several matches to slice");

        for limit in [None, Some(u32::MAX), Some(total), Some(total + 1)] {
            let out = run_query_deferrable(&idx, "", &parsed, None, sort, 0, limit);
            assert!(
                !out.sorted,
                "limit {limit:?} covers all {total} matches; ordering must be deferred off-lock"
            );
            assert_eq!(
                out.hits.len(),
                total as usize,
                "limit {limit:?}: a covering page still returns every match"
            );
        }

        // A limit that genuinely slices still orders here, where the index is
        // available to pick which matches survive.
        let sliced = run_query_deferrable(&idx, "", &parsed, None, sort, 0, Some(total - 1));
        assert!(sliced.sorted, "a real slice must still order in-lock");
        assert_eq!(sliced.hits.len(), total as usize - 1);

        // `run_query` keeps its unconditional promise for direct callers.
        assert!(
            run_query(&idx, "", &parsed, None, sort, 0, Some(u32::MAX)).sorted,
            "run_query must always return sorted hits"
        );
    }

    #[test]
    fn case_sensitive_query_respects_case() {
        let idx = sample_index();
        // Report.PDF has capital R; case: makes "report" not match.
        assert!(run(&idx, "report case:").is_empty());
        assert_eq!(run(&idx, "Report case:"), vec!["docs\\sub\\Report.PDF"]);
    }

    #[test]
    fn ext_filter_is_case_insensitive_even_under_case_directive() {
        // Regression: Report.PDF has an uppercase extension; ext: is always
        // case-insensitive, even when the query is otherwise case-sensitive.
        let idx = sample_index();
        assert_eq!(
            run(&idx, "Report ext:pdf case:"),
            vec!["docs\\sub\\Report.PDF"]
        );
        assert_eq!(
            run(&idx, "Report ext:PDF case:"),
            vec!["docs\\sub\\Report.PDF"]
        );
    }

    // -- path terms and path wildcards --------------------------------------

    /// `docs\` at the root and deeper, so a path wildcard is tested at both.
    /// `readme.md` repeats at every level, so a name term drives the arena
    /// scan to every one of them and the path wildcard must pick.
    fn docs_index() -> VolumeIndex {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
        let root = NTFS_ROOT_FRN;
        for r in [
            enum_rec(frn(100), root, "docs", true),
            enum_rec(frn(101), root, "repo", true),
            enum_rec(frn(102), frn(101), "docs", true),
            enum_rec(frn(103), frn(101), "src", true),
            enum_rec(frn(200), frn(100), "readme.md", false),
            enum_rec(frn(201), frn(102), "readme.md", false),
            enum_rec(frn(202), frn(102), "guide.MD", false),
            enum_rec(frn(203), frn(103), "readme.md", false),
            enum_rec(frn(204), frn(103), "main.cs", false),
            enum_rec(frn(205), frn(101), "readme.md", false),
        ] {
            idx.insert_enum(&r);
        }
        idx
    }

    fn paths_under(
        idx: &VolumeIndex,
        prefix: &str,
        query: &str,
        scope: Option<&[u8]>,
    ) -> Vec<String> {
        let parsed = parse_query(query).unwrap();
        let scope = scope.map(|s| resolve_scope(idx, s, false).expect("scope exists"));
        let out = run_query(
            idx,
            prefix,
            &parsed,
            scope,
            SortSpec::default_for(SortKey::Path),
            0,
            None,
        );
        out.hits
            .iter()
            .map(|h| crate::wtf8::to_string_lossy(&h.path))
            .collect()
    }

    /// A wildcard with a separator matches the whole full path, on every scan
    /// path: the full scan, the folded-arena scan (a name term drives it), and
    /// the scoped subtree walk. The wildcard's literal `\docs\` is in no name,
    /// so feeding it to the arena prefilter would find nothing.
    #[test]
    fn a_wildcard_with_a_separator_matches_the_full_path() {
        let idx = docs_index();
        let docs = [
            r"C:\docs\readme.md",
            r"C:\repo\docs\guide.MD",
            r"C:\repo\docs\readme.md",
        ];

        // Full scan: no name term, so every entry is tested.
        assert_eq!(paths_under(&idx, r"C:\", r"*\docs\*.md", None), docs);
        // Folded-arena scan: `readme` drives it.
        assert_eq!(
            paths_under(&idx, r"C:\", r"*\docs\*.md readme", None),
            [r"C:\docs\readme.md", r"C:\repo\docs\readme.md"]
        );
        // Scoped walk.
        assert_eq!(
            paths_under(&idx, r"C:\", r"*\docs\*.md", Some(b"repo")),
            [r"C:\repo\docs\guide.MD", r"C:\repo\docs\readme.md"]
        );
        // `path:` with a wildcard is a path wildcard too.
        assert_eq!(paths_under(&idx, r"C:\", r"path:*\docs\*.md", None), docs);
    }

    /// The full path starts at the mount prefix, so an anchored wildcard needs
    /// the drive, as in es: `C:\repo\*.cs` finds the `.cs` files under `repo`,
    /// and `repo\*.cs`, which no full path starts with, finds nothing.
    #[test]
    fn a_path_wildcard_is_anchored_to_the_whole_full_path() {
        let idx = docs_index();
        assert_eq!(
            paths_under(&idx, r"C:\", r"C:\repo\*.cs", None),
            [r"C:\repo\src\main.cs"]
        );
        assert!(paths_under(&idx, r"C:\", r"repo\*.cs", None).is_empty());
        // `?` stands for one character, a separator included.
        assert_eq!(
            paths_under(&idx, r"C:\", r"C:\repo?src\main.c?", None),
            [r"C:\repo\src\main.cs"]
        );
    }

    /// A case-sensitive path wildcard matches the raw full path.
    #[test]
    fn a_case_sensitive_path_wildcard_matches_the_raw_path() {
        let idx = docs_index();
        assert_eq!(
            paths_under(&idx, r"C:\", r"*\docs\*.MD case:", None),
            [r"C:\repo\docs\guide.MD"]
        );
    }

    /// Path terms see the same full path, mount prefix included.
    #[test]
    fn a_path_term_matches_anywhere_in_the_full_path() {
        let idx = docs_index();
        assert_eq!(
            paths_under(&idx, r"C:\", r"c:\docs", None),
            [r"C:\docs", r"C:\docs\readme.md"]
        );
        assert_eq!(
            paths_under(&idx, r"C:\", r"repo\src", None),
            [
                r"C:\repo\src",
                r"C:\repo\src\main.cs",
                r"C:\repo\src\readme.md"
            ]
        );
    }

    // -- result order -------------------------------------------------------

    fn add_file(
        idx: &mut VolumeIndex,
        f: Frn,
        links: &[(Frn, &str)],
        size: Option<u64>,
        mtime: Option<i64>,
    ) {
        use crate::layout::{LayoutFile, LayoutName};
        idx.enrich(&LayoutFile {
            frn: f,
            attributes: 0x20,
            size,
            mtime_ft: mtime,
            names: links
                .iter()
                .map(|&(parent_frn, name)| LayoutName {
                    parent_frn,
                    name: name.as_bytes().to_vec(),
                    name_lossy: false,
                    dos_only: false,
                })
                .collect(),
        });
    }

    /// Every way two hits can tie or nearly tie: es path order against a byte
    /// compare (`a~qx7zv` against `a\qx7zv`, `a` against `a b`), equal sizes and
    /// dates, an unknown size, names equal after folding, and one file with
    /// two links of the same name.
    fn order_index() -> VolumeIndex {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
        let root = NTFS_ROOT_FRN;
        for r in [
            enum_rec(frn(100), root, "P", true),
            enum_rec(frn(101), frn(100), "a", true),
            enum_rec(frn(102), frn(100), "a b", true),
            enum_rec(frn(103), frn(101), "x", true),
        ] {
            idx.insert_enum(&r);
        }
        add_file(
            &mut idx,
            frn(200),
            &[(frn(100), "a~qx7zv")],
            Some(10),
            Some(5),
        );
        add_file(
            &mut idx,
            frn(201),
            &[(frn(101), "qx7zv")],
            Some(10),
            Some(5),
        );
        add_file(
            &mut idx,
            frn(202),
            &[(frn(102), "QX7ZV")],
            Some(30),
            Some(7),
        );
        add_file(&mut idx, frn(203), &[(frn(103), "qx7zv")], None, None);
        add_file(
            &mut idx,
            frn(204),
            &[(frn(101), "link.qx7zv"), (frn(102), "link.qx7zv")],
            Some(10),
            Some(5),
        );
        add_file(
            &mut idx,
            frn(205),
            &[(frn(102), "b.qx7zv")],
            Some(10),
            Some(5),
        );
        idx
    }

    fn all_sorts() -> Vec<SortSpec> {
        let mut sorts = Vec::new();
        for key in [
            SortKey::Name,
            SortKey::Path,
            SortKey::Size,
            SortKey::DateModified,
        ] {
            for dir in [SortDir::Asc, SortDir::Desc] {
                sorts.push(SortSpec { key, dir });
            }
        }
        sorts
    }

    fn texts(hits: &[QueryHit]) -> Vec<String> {
        hits.iter()
            .map(|h| crate::wtf8::to_string_lossy(&h.path))
            .collect()
    }

    /// `-sort path` orders by parent folder, then name, as
    /// [`SortKey::Path`] documents. `P\a~qx7zv` sits in `P`, so it comes before
    /// everything in `P\a`, though `\` sorts before `~` in a byte compare of
    /// the whole path.
    #[test]
    fn path_order_is_parent_folder_then_name() {
        let idx = order_index();
        let parsed = parse_query("qx7zv").unwrap();
        let out = run_query(
            &idx,
            r"C:\",
            &parsed,
            None,
            SortSpec::default_for(SortKey::Path),
            0,
            None,
        );
        assert_eq!(
            texts(&out.hits),
            [
                r"C:\P\a~qx7zv",
                r"C:\P\a\link.qx7zv",
                r"C:\P\a\qx7zv",
                r"C:\P\a b\b.qx7zv",
                r"C:\P\a b\link.qx7zv",
                r"C:\P\a b\QX7ZV",
                r"C:\P\a\x\qx7zv",
            ]
        );
    }

    /// An unknown size or date sorts last whichever way the key runs.
    #[test]
    fn unknown_sizes_and_dates_sort_last() {
        let idx = order_index();
        let parsed = parse_query("qx7zv").unwrap();
        for sort in all_sorts() {
            if matches!(sort.key, SortKey::Size | SortKey::DateModified) {
                let out = run_query(&idx, r"C:\", &parsed, None, sort, 0, None);
                assert_eq!(
                    texts(&out.hits).last().unwrap(),
                    r"C:\P\a\x\qx7zv",
                    "{sort:?}"
                );
            }
        }
    }

    /// A limited query is a prefix of the same query without a limit, under
    /// every sort, whether the engine orders the page under the lock or
    /// [`order_hits`] orders the deferred set after it.
    #[test]
    fn a_limited_query_is_a_prefix_of_the_unlimited_one() {
        let idx = order_index();
        for query in ["qx7zv", ""] {
            let parsed = parse_query(query).unwrap();
            for sort in all_sorts() {
                let deferred = run_query_unsorted(&idx, r"C:\", &parsed, None);
                let unlimited = order_hits(
                    sort,
                    vec![VolumeRun {
                        mount_prefix: r"C:\",
                        hits: deferred.hits,
                        sorted: false,
                    }],
                );
                assert_eq!(
                    run_query(&idx, r"C:\", &parsed, None, sort, 0, None).hits,
                    unlimited,
                    "{query:?} {sort:?}: the engine and order_hits disagree"
                );
                for k in 0..=unlimited.len() {
                    let page = run_query(&idx, r"C:\", &parsed, None, sort, 0, Some(k as u32));
                    assert_eq!(page.hits, unlimited[..k], "{query:?} {sort:?} -n {k}");
                }
            }
        }
    }

    /// The same holds across volumes. Each volume picks its own top `k`, and
    /// [`order_hits`] merges them; the result equals the first `k` of every
    /// match merged. The two prefixes interleave in path order (`C:\mnt\P`
    /// sorts between `C:` and `C:\P`), and the identical fixtures make every
    /// name tie across volumes.
    #[test]
    fn a_limited_query_is_a_prefix_across_volumes() {
        let volumes = [(r"C:\", order_index()), (r"C:\mnt\", order_index())];
        let parsed = parse_query("qx7zv").unwrap();
        for sort in all_sorts() {
            let merged = |limit: Option<u32>| {
                let runs = volumes
                    .iter()
                    .map(|(prefix, idx)| {
                        let out = run_query_deferrable(idx, prefix, &parsed, None, sort, 0, limit);
                        VolumeRun {
                            mount_prefix: prefix,
                            hits: out.hits,
                            sorted: out.sorted,
                        }
                    })
                    .collect();
                let mut hits = order_hits(sort, runs);
                hits.truncate(limit.map_or(usize::MAX, |k| k as usize));
                hits
            };
            let unlimited = merged(None);
            assert_eq!(unlimited.len(), 14);
            for k in 0..=unlimited.len() {
                assert_eq!(merged(Some(k as u32)), unlimited[..k], "{sort:?} -n {k}");
            }
        }
    }

    // -- oracle property test --------------------------------------------

    use proptest::prelude::*;

    /// A flat index of random-named files at the root.
    fn flat_index(names: &[String]) -> VolumeIndex {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
        for (i, n) in names.iter().enumerate() {
            idx.insert_enum(&enum_rec(frn(1000 + i as u64), NTFS_ROOT_FRN, n, false));
        }
        idx
    }

    /// Naive oracle: a file matches iff its folded name contains every folded
    /// term as a substring.
    fn oracle(names: &[String], terms: &[String]) -> BTreeSet<String> {
        names
            .iter()
            .filter(|n| {
                let fn_ = crate::fold::fold(n.as_bytes());
                terms.iter().all(|t| {
                    let ft = crate::fold::fold(t.as_bytes());
                    fn_.windows(ft.len().max(1)).any(|w| w == ft.as_slice()) || ft.is_empty()
                })
            })
            .cloned()
            .collect()
    }

    use std::collections::BTreeSet;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        /// The engine's name-term matching agrees with the naive oracle over
        /// random corpora and needles (mixed case, unicode letters).
        #[test]
        fn matcher_agrees_with_oracle(
            names in prop::collection::vec("[a-zA-Zçğ0-9._]{1,10}", 1..30),
            terms in prop::collection::vec("[a-zA-Zçğ]{1,4}", 0..3),
        ) {
            // Names must be unique so a path set is a faithful comparison.
            let mut uniq: Vec<String> = Vec::new();
            for (i, n) in names.iter().enumerate() {
                uniq.push(format!("{n}_{i}"));
            }
            let idx = flat_index(&uniq);
            let query = terms.join(" ");
            let parsed = parse_query(&query).unwrap();
            let out = run_query(&idx, "", &parsed, None, SortSpec::default(), 0, None);
            let engine: BTreeSet<String> = out
                .hits
                .iter()
                .map(|h| crate::wtf8::to_string_lossy(&h.path))
                .collect();
            prop_assert_eq!(engine, oracle(&uniq, &terms));
        }

        /// A path term finds exactly the entries whose full path, built by
        /// walking to the root, contains it: over random trees that include
        /// orphans, which have no path.
        #[test]
        fn path_terms_agree_with_a_naive_full_path_search(
            parents in prop::collection::vec(0usize..14, 1..40),
            names in prop::collection::vec("[a-cA-C]{1,3}", 40),
            before in "[a-cA-C]{0,2}",
            after in "[a-cA-C]{0,2}",
        ) {
            let mut idx = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
            for (i, &p) in parents.iter().enumerate() {
                let parent = match p {
                    p if p < i => frn(1000 + p as u64),
                    12 => frn(999), // never inserted: an orphan's parent
                    _ => NTFS_ROOT_FRN,
                };
                idx.insert_enum(&enum_rec(frn(1000 + i as u64), parent, &names[i], i % 3 != 0));
            }
            let term = format!("{before}\\{after}");
            let parsed = parse_query(&term).unwrap();
            let out = run_query(&idx, r"C:\", &parsed, None, SortSpec::default(), 0, None);
            let engine: BTreeSet<String> = out
                .hits
                .iter()
                .map(|h| crate::wtf8::to_string_lossy(&h.path))
                .collect();

            let needle = crate::fold::fold(term.as_bytes());
            let mut naive = BTreeSet::new();
            for e in idx.live_entries().filter(|&e| !idx.is_synthetic(e)) {
                let mut rel = Vec::new();
                if idx.path_of(e, &mut rel) != crate::index::PathStatus::Ok {
                    continue;
                }
                let full = [b"C:\\".as_slice(), &rel].concat();
                let folded = crate::fold::fold(&full);
                if folded.windows(needle.len()).any(|w| w == needle.as_slice()) {
                    naive.insert(crate::wtf8::to_string_lossy(&full));
                }
            }
            prop_assert_eq!(engine, naive);
        }
    }
}
