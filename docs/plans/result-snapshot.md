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
  request. It needs ids, caps, expiry and a sweep. It adds 26 to 52 MB to
  the daemon while a window holds a broad search. Each result snapshot sorts
  every match under the volume's read lock. It needs a protocol version.
  The window gets two new states and a new limit on the selection ("What
  changes in gozler-ui").
- **It must come after visibility.** A result snapshot must belong to the
  user who made it, and the visibility plan adds that ownership: a view for
  each user, in a registry with an LRU cap, a sweep once a minute and a lock
  order. Built before, a result snapshot would either let any user read the
  rows of another user once visibility lands, or invent a second registry.
  Built after, it uses the same patterns.
- **Waiting costs no rework.** The window keeps the daemon behind one `Ask`
  per request, so the work it already has does not change. Only the items in
  "What changes in gozler-ui" are added.

## What the code does now

- **Every request for rows scans every match.** `run_query_impl` collects
  each match as an 8-byte `Candidate`, then `order_top` selects and sorts the
  first `offset + limit` of them, and paths are built only for those. Then
  `stream_rows` drops the first `offset` (`per_vol_limit`). The scan of the
  empty search takes about 0.3 s on the owner's machine, under the volume's
  read lock, for every chunk.
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
  128-bit ID.
- **The generation cannot check a result snapshot.** `apply_batch` raises
  it once for each journal batch, so it changes whenever a file on the
  volume changes. A rebuild installs a new `VolumeIndex` (`Building::finish`), and its
  generation starts at 0 again, so the generation cannot tell that the index
  was replaced.
- **The wire.** `PROTO_VERSION` is 7. A results page carries a `generation`
  that is always 0, and item flag bits 0 to 3.

## What a result snapshot gives

| | Today | With a result snapshot |
| --- | --- | --- |
| Rows the list reaches | 100,000 | every match |
| Wait for a chunk | the scan (0.3 s) and the sort and paths of every row up to the chunk's end (0.4 s near row 100,000) | the paths of the chunk only (not measured, see below) |
| Full scans for a search scrolled to its end | one for each chunk, up to 7 | two: the first chunk, then the result snapshot |
| A selected row the user never saw | the file at that position when the selection is read | the file at that position when the search ran |
| Rows of different chunks | each from its own moment | all from one moment |

## Design

### What a result snapshot holds

- **One row for each match, in result order:** the volume (1 byte), the
  `EntryIdx` (4 bytes) and the file ID at the index's own width (8 bytes on
  NTFS, 16 on ReFS). The row has no path and no name, because the read builds
  them from the index, as a page does today.
- The total and the two coverage flags (`volumes_incomplete`,
  `metadata_pending`) at the moment of the search.
- For each volume, the build number of the index it read. `Indexed` gains
  `build: u64`, from a daemon-wide counter that `Building::finish` raises.
- The view that made it (after visibility), and when it was last read.

It never changes after it is made, so readers share it through an `Arc`
without a lock.

### When it is made

The first chunk of a search is asked for as today. The window asks for a
result snapshot only when it first needs rows outside that chunk: a scroll,
`End`, or a selection that reaches uncached rows. The reason is that search
as you type sends a search 150 ms after each keystroke, and a full sort for
each keystroke would multiply the daemon's work on the owner's 15 W laptop.
Most searches never scroll.

Making it costs the scan, a full sort of every match on each volume under
that volume's read lock, and, when the search covers several volumes, a merge
under the read locks of all of them. The sort cannot move off the lock, as
`run_query_deferrable` moves today's export sort, because its keys are the
folded names in the index. Copying the keys out is why a full sorted export
peaks 0.3 GB above Everything's committed memory (`docs/BENCHMARKS.md`). `order_top` already sorts in
parallel from 100,000 rows. Holding several read locks cannot deadlock,
because no thread that holds a volume's write lock waits for another
volume's lock.

While the sort holds the read lock, the volume's journal tail waits. A
`parking_lot` writer that waits also blocks new readers, so other clients'
searches on that volume wait too. That is the main cost to measure.

### Reading rows

A read gives the rows at `offset..offset + limit`. Under each volume's read
lock, each row is checked:

1. When the volume's index has another build number, or the volume is gone,
   the whole result snapshot is gone. A rebuilt index is better answered by
   a new search.
2. When the slot is live and holds the row's file ID, the row is that file,
   with its current name, path, size and date. A renamed or moved file keeps
   its place in the order until `F5` sorts again. A hard link that was
   removed and added again in the same slot shows that link: the same file.
3. Otherwise the row is gone: the file was deleted.
4. After visibility, a row the view can no longer see is gone too, so a
   result snapshot never shows a name its user has lost the right to see.

New files appear only after `F5`, as today.

### Lifetime

| Rule | Value | Why |
| --- | --- | --- |
| Where | one `Mutex<ResultSnapshots>` for the daemon | the table is needed beyond one connection |
| Id | a `u64` from a counter, never reused | a stale id can never name a newer result snapshot |
| Owner | the view of the request that made it; a request from another view gets "gone" | one user cannot read another user's rows |
| Per view | at most 2; a third drops the one read least recently | the window's and one other client's |
| In total | at most twice the entries of all indexes, in rows; past it, the one read least recently is dropped, from any view | caps daemon memory at two empty searches |
| Expiry | 10 minutes since the last read, dropped by the sweep | a person who looks away for a few minutes keeps scrolling without a new search |
| Release | the request that makes one may name the one it `replaces`, from the same view | the window frees its old one at once, with no extra request |
| Lock order | the table's mutex is never held while a volume lock is taken | as the visibility plan's view registry |

The sweep is the visibility plan's once-a-minute task in `accept_loop`. A
dropped result snapshot costs only a new search.

### Protocol

`PROTO_VERSION` rises by one (from 7 today).

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

13 bytes for each NTFS row (21 on ReFS), in columns without padding.

| Search | Rows | Result snapshot | Beside |
| --- | --- | --- | --- |
| The empty search on the owner's `C:` | 2,015,690 | 26 MB | an index of 194.6 MB |
| The empty search over 3 million entries | 3,000,000 | 39 MB (36 MB without the volume byte) | |
| The empty search on the benchmark machine | 3,970,000 | 52 MB | 525 MB committed |
| The cap, on the owner's `C:` | 4,031,380 | 52 MB | |

While it is made, the daemon also holds the scan's candidates, 8 bytes a
match, as every request does today: 63 MB at the peak for 3 million rows,
freed when the reply starts.

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

- **The scan and full sort of the empty search under the read lock.** This
  decides recommendation item 3. Measure it with a case in the `query_2M`
  bench group that orders every candidate and builds no path, run through
  `pwsh ./check.ps1 -Bench query_2M`.
- **A read of 16,384 rows from a result snapshot.** It builds the same paths
  as a page does today, without the scan, so it is expected to take a small
  part of today's 0.3 s.
