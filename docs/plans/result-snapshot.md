# Plan: what is left of the result snapshot

gozlerd keeps result snapshots and the window reads its rows past the first
chunk from them (DESIGN.md: Result snapshots, Rows on demand); #118 built
them on 2026-10-21. What is left:

- the visibility filter of each read, which the [visibility
  plan](visibility-plan.md) adds with its filtered pages ("Who sees what"
  there), because until then every user may read every name;
- the costs below, which no bench has measured.

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
