# Plan: the window reads result snapshots

A result snapshot is the list of one search's matches, in result order, that
gozlerd keeps after it answers, so the window reads any rows of that list
later without searching again. gozlerd and the protocol keep and serve them
(DESIGN.md: Result snapshots); this plan holds what is left.

## Schedule

**Scheduled.** The owner lifted the deferral on 2026-10-21 and asked for the
result snapshot to be built. `gozler-core`, protocol 11 and gozlerd were
built first (#118). What is left:

- the window's changes below;
- the visibility filter of each read, which the [visibility
  plan](visibility-plan.md) adds with its filtered pages ("Who sees what"
  there), because until then every user may read every name;
- the costs that "Not measured" lists.

## What changes in gozler-ui

[The design](../DESIGN.md#how-normal-changes-land) lists these changes. The
first chunk of a search is asked for as today; the window asks for a result
snapshot only when it first needs rows outside that chunk (a scroll, `End`,
or a selection that reaches uncached rows), because search as you type sends
a search 150 ms after each keystroke, and a full sort for each keystroke
would multiply the daemon's work on the owner's 15 W laptop.

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
  is the wait for a scroll past the first chunk. On the 5.6 million entries of a tree 190
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
