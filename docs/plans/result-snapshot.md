# Plan: a result snapshot in gozlerd

A result snapshot is the list of one search's matches, in result order, that
gozlerd keeps after it answers. The window then reads any rows of that list
later, without searching again. This plan says whether to build it, and how,
if the owner decides to.

"Result snapshot" is always written in full, in the docs and in code names
(`ResultSnapshot`, `ResultSnapshotId`), because `VolumePhase::Snapshot`
already names a volume without a journal.

## Schedule

**Not scheduled.** [An owner decision](../DESIGN.md#the-window) defers it
as new daemon work. This plan is the assessment for the owner, and the design
to build if the owner reopens that decision.

## Recommendation

**Defer.** Keep the owner decision that the list reaches the first 100,000
rows until all three of these are true:

1. The [visibility plan](visibility-plan.md) has landed.
2. The owner finds the 100,000-row limit or the wait for a chunk to be a
   problem in daily use.
3. A bench shows that the scan and full sort of the empty search take about
   0.5 s or less on the owner's machine (see "Not measured"). That is the
   wait for today's worst chunk, plus a little.

The reasons:

- **The user gains little.** 100,000 rows are thousands of screens of the
  list, so a person narrows the search instead, and the status bar already
  says "Showing the first 100,000 of N results". The 0.3 to 0.4 s wait comes
  only when the list shows a chunk that the cache does not hold. A search
  has at most 7 chunks (`LIST_LIMIT` 100,000, `CHUNK_ROWS` 16,384), and the
  cache holds 2. The selection limit
  reaches only rows that the user never saw. The reply that reads them also
  refreshes the visible rows, so no row that the user can see changes its
  meaning.
- **The cost is high.** It is the first daemon state that lives longer than a
  request. It needs ids, caps and expiry. It adds 32 to 64 MB to
  the daemon while a window holds a broad search. Each result snapshot sorts
  every match, though off the volume's read lock. It needs a protocol
  version.
  The window gets two new states and a new limit on the selection ("What
  changes in gozler-ui").
- **It must come after visibility.** A result snapshot must show each user
  only the rows they may see, and the visibility plan adds the caller's
  token to each request and the answers that filter its rows ("Who sees
  what" there). Built before, a result snapshot would let any user read the
  rows of another user once visibility lands. Built after, each read filters
  its rows as a page does.
- **Waiting costs no rework.** The window keeps the daemon behind one `Ask`
  per request, so the work it already has does not change. Only the items in
  "What changes in gozler-ui" are added.

## What the code does now

- **Every request for rows scans every match.** `query::ResultRows::search`
  keys each match as a 16-byte `SortRecord` under the volume's read lock,
  selects and sorts the first `offset + limit` of them without the lock, and
  makes those the reply's rows under the lock again: each row is the entry
  index and the file ID. `ResultRows::page` reads each page's rows from the
  index and skips the first `offset`, a skipped row costing as much as a sent
  one. The scan of the empty search takes about 0.3 s on the owner's machine,
  under the volume's read lock, for every chunk. The reply drops its rows when
  its last page is sent.
- **Several volumes merge by their current values.** A row keeps no sort key,
  only its entry index and file ID, and name keys and folder ranks belong to
  one volume. So a reply over several volumes compares each volume's next row
  by the values the index holds when the page reads it.
- **The daemon keeps nothing between requests.** A connection closes after 30
  s with no request (`IDLE_TIMEOUT`), the daemon serves at most 64 at once
  (`MAX_CONNECTIONS`), and the window opens a connection for each request
  ([window rule 3](../DESIGN.md#window-rules)).
  So a result snapshot cannot live in a connection. It needs a daemon-wide
  table.
- **An `EntryIdx` survives compaction but not a delete.**
  `VolumeIndex::apply_compaction` rewrites name ids only, so a slot keeps its
  index. A delete tombstones the slot and puts it on the free list, and
  `EntryTable::alloc` gives it to the next new entry. A rename rewrites the
  entry in its slot (`rewrite_entry`). The file ID tells a reused slot apart:
  on NTFS it holds the record's sequence number, and on ReFS it is the whole
  128-bit ID. A reply's pages already check each row this way.
- **The index instance checks a result snapshot.** `VolumeIndex::instance`
  is a number that no other index of the process has had: a rebuild's new
  index takes one, and so does `optimize_storage`, because it gives the
  entries new indexes. A reply's pages already stop a volume's rows when it
  changes. The generation cannot serve: `apply_batch` raises it for each
  journal batch, and a new index starts it at 0 again.
- **The wire.** `PROTO_VERSION` is 9. A results page carries a `generation`
  that is always 0, and item flag bits 0 to 3.

## What a result snapshot gives

| | Today | With a result snapshot |
| --- | --- | --- |
| Rows the list reaches | 100,000 | every match |
| Wait for a chunk | the scan (0.3 s), the sort of every match up to the chunk's end, and the chunk's rows | the chunk's rows only (see "Not measured") |
| Full scans for a search scrolled to its end | one for each chunk, up to 7 | two: the first chunk, then the result snapshot |
| A selected row the user never saw | the file at that position when the selection is read | the file at that position when the search ran |
| Rows of different chunks | each from its own moment | all from one moment |

## Design

### What a result snapshot holds

- **The reply's rows**, which a reply now drops when its last page is
  sent: for each volume, its `query::VolumeRows`, one row for each match in
  result order, the `EntryIdx` and the file ID at the index's own width
  (16 bytes a row on NTFS, 24 on ReFS), with the index instance that
  answered. The row has no path and no name, because the read builds them
  from the index, as a page does today.
- **The merged order, for a search over several volumes:** one byte for each
  row that names the volume of the next row, and every 4,096 rows a
  checkpoint of each volume's position. Each volume's rows are already in
  result order, so row k of the merge is the next unread row of the volume
  that byte k names. A read jumps to the checkpoint below its offset and
  replays at most 4,095 bytes without reading an index. A search over one
  volume, which includes every search with a folder scope, needs none: the
  offset indexes its rows directly. A single merged list was the
  alternative, but a search over both NTFS and ReFS volumes would make every
  row the 24-byte ReFS row.
- The total and the two coverage flags (`volumes_incomplete`,
  `metadata_pending`) at the moment of the search.
- Its id, the user SID of the request that made it, for the caps, and when
  it was last read.
- **Its volumes, held weakly** (`Weak<VolumeState>`), so that a removed
  volume's index is freed when its last owner drops it (see "Reading rows").

It never changes after it is made, so readers share it through an `Arc`
without a lock. `VolumeRows` changes during a reply today only in its
`past_cut` count, which a reply lowers as it ends a volume early. That count
moves into the reader's own state, so `VolumeRows` never changes after the
search and can be shared. Each read builds its own reader over the shared
rows: its position, its `FolderCopy` and its `ResultNames`, because the
folder ids a read hands out belong to that read's pages.

### When it is made

The first chunk of a search is asked for as today. The window asks for a
result snapshot only when it first needs rows outside that chunk: a scroll,
`End`, or a selection that reaches uncached rows. The reason is that search
as you type sends a search 150 ms after each keystroke, and a full sort for
each keystroke would multiply the daemon's work on the owner's 15 W laptop.
Most searches never scroll.

Making it costs what a search without a limit costs ("Rows on demand" in
`docs/DESIGN.md`): the scan and the keys under each volume's read lock, the
sort of every match without it, and the rows under the lock again. A merge of
several volumes reads under the read locks of all of them, taken in
`VolumeSet` order (the lock rules in `volume_state.rs`).

For several volumes, making the merged order walks every row once, a page at
a time, under the read locks of the volumes the page reads, because the merge
compares current values (see "What the code does now"). A row that a page
could not send at that moment, because its file was deleted or left the
scope, is dropped then, because a dead slot has no value to compare. So the
total is the count at that moment.

While a volume's read lock is held, its journal tail waits. A `parking_lot`
writer that waits also blocks new readers, so other clients' searches on that
volume wait too. That is the main cost to measure.

### Reading rows

A read gives the rows at `offset..offset + limit`. Under each volume's read
lock, each row is checked:

1. When the volume's index has another instance, or the volume is gone,
   the whole result snapshot is gone. A rebuilt index is better answered by
   a new search. A read upgrades each weak volume once, and the volume is
   gone when the upgrade fails or the upgraded volume is not the one in the
   request's `VolumeSet` (`Arc::ptr_eq`). The upgrade alone is not enough:
   a tail that the supervisor abandoned as wedged still owns its volume, and
   that volume's index keeps its instance, so without the check the result
   snapshot would keep serving a removed volume's rows. A volume attached
   again is a new `VolumeState`, so the check reports the old one gone.
2. When the slot is live and holds the row's file ID, the row is that file,
   with its current name, path, size and date, as a page reads it today. A
   renamed or moved file keeps its place in the order until `F5` sorts
   again. A hard link that was removed and added again in the same slot
   shows that link: the same file.
3. Otherwise the row is gone: the file was deleted, it moved out of the
   search's folder scope, or its folder has no full path now, the rows that
   "Index coherence" in `docs/DESIGN.md` has a reply leave out. A page today
   leaves such a row out and lowers the total; a result snapshot marks it
   gone, keeps its place and keeps the total, because the window holds rows
   by their place. Both make the same check, one `query` function that a
   reply's reader and a result snapshot's reader call: the row's folder when
   the row can be sent, or nothing. The reuse-mark check that
   `OrderedMatches::read_rows` makes before the rows exist stays apart,
   because a `SortRecord` has no room for the file ID that this check
   compares.
4. A row that the reading request's caller cannot see now is gone too, so a
   result snapshot never shows a name its user has lost the right to see.
   Each read is a request, so it builds its own answers from the caller's
   token, as every filtered request does, and tests each row with the same
   test a page uses (`VolumeIndex::is_seen`). The total a read reports counts
   only the rows the caller sees.

New files appear only after `F5`, as today.

### Lifetime

| Rule | Value | Why |
| --- | --- | --- |
| Where | one `Mutex<ResultSnapshots>` for the daemon | the table is needed beyond one connection |
| Id | 128 random bits, never reused | only the client that made it learns it, over its own pipe, so another user can neither guess nor read it, and a stale id can never name a newer result snapshot |
| Owner | whoever holds the id; each read filters the rows by the reader's own token | one user cannot read another user's rows |
| Per user | at most 2 for each user SID; a third drops the one read least recently | the window's and one other client's; a memory rule, so the user SID is enough |
| In total | at most twice the entries of all indexes, in rows; past it, the one read least recently is dropped, from any user | caps daemon memory at two empty searches |
| Expiry | 10 minutes since the last read, dropped whenever the table is used | a person who looks away for a few minutes keeps scrolling without a new search, and no timer is needed |
| Release | the request that makes one may name the one it `replaces` | the window frees its old one at once, with no extra request |
| Lock order | the table's mutex is never held while a volume lock is taken | a search never waits for the table |

A dropped result snapshot costs only a new search.

### Protocol

`PROTO_VERSION` rises by one: from 10, which the visibility plan sets, to
11.

- **Make one.** `SearchRequest` with a new `Returns::ResultSnapshot { rows:
  Rows, replaces: Option<ResultSnapshotId> }`. The reply is one JSON frame,
  `Response::ResultSnapshot { id, total }`, then the binary pages of
  `rows.offset..offset + limit`, as a `Rows` search sends them.
- **Read rows.** A new `Request::ResultSnapshotRows { id, offset, limit,
  want_size, want_mtime, want_attributes }`. The reply is binary pages. A
  gone row has a new item flag, bit 4 `gone`, an empty path and no other
  field. A result snapshot that is gone gives `Response::Error` with a new
  `ProtoError::ResultSnapshotGone`.
- **Older clients** never send these requests, so they never get the new
  flag or the new error, and every reply they get is unchanged.
- **An older daemon** refuses the window's `Hello`, and the window shows that
  the service is too old, as it already does.

### Memory

16 bytes for each NTFS row (24 on ReFS), the reply's rows. A search over
several volumes adds the merged order: 1 byte for each row and a checkpoint
every 4,096 rows, about 6 % of the NTFS rows.

| Search | Rows | Result snapshot | Beside |
| --- | --- | --- | --- |
| The empty search on the owner's `C:` | 2,015,690 | 32 MB | an index of 194.6 MB |
| The empty search over 3 million entries | 3,000,000 | 48 MB | |
| The empty search on the benchmark machine | 3,970,000 | 64 MB | 525 MB committed |
| The cap, on the owner's `C:` | 4,031,380 | 65 MB | |

On NTFS the records become the rows in place, so making it holds no second
list. A ReFS row is 24 bytes, larger than a record, so its rows take a second
list while they are made.

## What changes in gozler-ui

[The design](../DESIGN.md#how-normal-changes-land) lists these changes:

1. **A new `Ask`** that makes a result snapshot of the shown search and
   carries the chunk to read, and a chunk or selection read from it. When
   the result snapshot arrives, the window drops the chunks it cached
   before, because they are from an earlier moment (window rule 10).
2. **The list reaches every match.** `LIST_LIMIT` goes, and so does state
   `S4`'s text "Showing the first 100,000 of N results".
3. **A gone row** shows that the file no longer exists, and the selection
   leaves it out. This is a new state in `DESIGN.md`'s "States".
4. **A gone result snapshot** makes the window run the search again, as `F5`
   does.
5. **The selection needs its own limit.** Today the list's limit bounds it.
   With every row reachable, Select all on 3 million rows would hold about
   600 MB of files (20 MB for each 100,000), which breaks the memory budget.
   So the selection keeps 100,000 rows as its own limit: the window reads
   `LVM_GETSELECTEDCOUNT` first, and above the limit the commands that act
   on the selection are disabled and the status bar says why. The rule
   "No action refuses" in `DESIGN.md`'s "The selection" changes with it.

Export, the totals, a folder's size and status do not change.

## Not measured

- **The scan and full sort of the empty search on the owner's `C:`.** This
  decides recommendation item 3. On the 5.6 million entries of a tree 190
  folders deep, the scan takes 0.12 s under the read lock, the sort 0.06 s
  without it, and the rows 0.06 s under it again
  (`crates/gozler-daemon/examples/deep_export.rs`); `C:` is not measured.
- **A read of 16,384 rows from a result snapshot.** It reads its rows as a
  page does today, without the scan, and jumps to its offset through the
  merged order instead of walking every earlier row as a reply does. On the
  same tree, a page of 4,096 rows holds the read lock for at most 0.04 s, so
  four of them take about 0.16 s; on `C:` it is not measured.
- **Making the merged order of a search over several volumes.** It walks
  every row once under the volumes' read locks, as a full export does; not
  measured.

## Where the architecture work stopped

The architecture review of gozler-core and gozlerd moved the search's steps
from gozlerd into `query::ResultRows`, gave the page-time row check one
function (`row_folder`), and moved the journal read policy into
`ReadFailures`. One question is still open, and it comes first when work
resumes. No release happens before it is settled.

**Is the `path-term` search slower?** The criterion bench
`query_2M/path-term(group_007\bin_x64_03)` in
`crates/gozler-core/benches/query.rs` was slower with the new code in every
completed run:

| Code | Mcycles, criterion's middle estimate |
| --- | --- |
| Before the move (HEAD) | 350, 376, 396, 490, 676 |
| After the move | 571, 767, 843 |

- **Method.** Export HEAD with `git archive` into a temporary folder and give
  it its own `CARGO_TARGET_DIR`, so its bench binary cannot share a name with
  the tree's. Run `check.ps1 -Bench path-term` for HEAD, then for the tree,
  and repeat the pair. Run nothing else on the machine meanwhile.
- **Why it is undecided.** The same HEAD binary moved by up to 53 % between
  runs, and the tree's numbers lie inside HEAD's range. A review of the code
  found no extra work per entry or per match: `find_matches` gets the same
  arguments as before, and the new code adds only a few small allocations
  per search and a call through a closure in `run_held`.
- **If the tree stays slower in both pairs,** find what costs the time,
  starting with what the closure in `run_held` stops the compiler from
  inlining, and fix it, because the window must not get slower.

Two candidates of the review need no work of their own. Placing Dates
created in the service is in [service settings](service-settings.md). The
search totals and the window's sums now both add the known sizes, so the
candidate to give them one owner was dropped.
