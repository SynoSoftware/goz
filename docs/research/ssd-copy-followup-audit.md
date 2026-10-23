# SSD copy follow-up: source audit

2026-10-23. Read-only audit of the working tree after the first overlap
experiment. No build, benchmark or production change was made for this note.
The earlier [research](ssd-copy-scheduling.md) owns the external storage
sources; this note follows the local implementation.
Subsequent measurements are in
[the benchmark record](../BENCHMARKS.md#ssd-follow-up-repeatability-and-discovery).
They confirm the wide-folder startup delay and do not reproduce a
consistent separate-overlap advantage over the mixed operation.
The later [early-dispatch experiment](../BENCHMARKS.md#ssd-early-dispatch-before-and-after)
changed the ordering audited below and measured its result; this note
preserves the original diagnosis.

## What the first experiment establishes

The [recorded samples](ssd-copy-2026-10-23.csv) give `mixed-16` a median
flushed time of 14.17 s and `overlap-16` 12.46 s. Median Queue completion
times, before the final flush phase, are 6.75 s and 5.59 s respectively.
These are observations on this fixture, not an identified cause.

The [harness](../../crates/gozler-fileops/benches/ssd_copy.rs),
`sample` / `Order::Mixed`, supplies **two selected roots** in one operation:
`[small-folder, large.bin]`. The large file is not hidden inside a subtree.
Both roots enter `Work::Selected`, and the pool takes the last queued item
first. Therefore late recursive discovery cannot explain this mixed case's
gap: the large file was available at the start and was initially at the top
of its queue. Thread scheduling can change which worker takes it, but it
does not require discovering a nested folder.
[copy::copy and Walk::selected](../../crates/gozler-fileops/src/copy.rs),
[Pool::next](../../crates/gozler-fileops/src/pool.rs)

There is material variation. Flushed times span 14.04–15.46 s for mixed and
9.22–13.36 s for overlap. Subtracting Queue completion from flushed time
gives tails of 6.74/8.71/8.24 s for mixed and 7.31/7.78/3.63 s for overlap.
The unusually short third overlap tail is visible in the data. Neither
"the whole difference is flushing" nor "the pool caused it" follows:
there is also an earlier completion difference, but no mixed per-class
completion time was recorded. Repeatability and phase attribution remain
open. [Samples](ssd-copy-2026-10-23.csv)

## Concrete differences between the two execution shapes

| Area | Source fact | What remains unmeasured |
| --- | --- | --- |
| Work pool | Mixed has one `Pool` mutex for both lanes. Separate transfers have separate pools. Each `next`, `push` and `finish` briefly takes its pool lock; the copy itself runs outside it. | Lock wait and wake cost. The large lane handles one file here, so pool-lock sharing alone is not evidence of a seconds-long cost. |
| Progress | Mixed shares `Progress.bytes/items` atomics and the mutex protecting the most recently started file. Separate operations have separate counters and slots. Chunk callbacks increment bytes; file starts replace the current-file slot. | Cache-line contention or mutex wait. Sixteen small workers already share these fields; one added large copy may or may not matter. |
| Pause | Active work items enter one `Pause` state mutex and leave through the activity guard. Unpaused `wait` has an atomic fast path. Separate transfers have independent gates. | Additional gate contention. A large-file callback reads the pause atomic; it does not lock the gate for each chunk. |
| Destination | Mixed initializes one transfer destination and shares its root `Target`. Separate operations initialize independently and read destination names independently. Child folders normally have their own name locks. | Whether root name activity competes meaningfully in this fixture. Destination initialization is once per transfer, not one global lock around all copies. |
| Threads | A copy starts its configured small workers plus one large-lane thread even when it has only one large selected file. Two operations create two sets; unused workers wait on the pool condition variable. | Thread startup, scheduler placement and idle-thread cost. This is not 32 workers actively copying small files. |
| Queue/recovery | Each admitted part starts on a queue thread after saved recovery state is written. `write_saved` has one serialization mutex and may find the file already current. Separate operations can incur more startup/completion work. | Startup timing and whether the extra work shifts overlap rather than directly making it faster. |

Authorities: [pool.rs](../../crates/gozler-fileops/src/pool.rs),
[Progress and Pause](../../crates/gozler-fileops/src/operation.rs),
[Walk::copy_file](../../crates/gozler-fileops/src/copy.rs),
[Target](../../crates/gozler-fileops/src/place.rs),
[Shared::start, write_saved, run_transfer](../../crates/gozler-fileops/src/queue.rs).

These are candidate costs and timing differences, not diagnoses. Changing
production locks before measuring their wait time would be speculative.

## A real discovery opportunity, on a different tree

`Walk::list` first drains the current source folder's listing into `files`
and `subfolders`. It then processes **all immediate subfolders** before
iterating the direct files. Each subfolder step opens and checks the source,
creates or merges its destination, and queues its listing. Only afterward
can a direct large file enter the large lane. Other workers may be copying
or discovering those child trees in parallel.
[Walk::list, subfolder, folder_target](../../crates/gozler-fileops/src/copy.rs),
[listing::list](../../crates/gozler-fileops/src/listing.rs)

With one selected root containing a large file beside 1,000 immediate
subfolders, the large lane therefore waits through those immediate
subfolder-initialization steps. This sequencing follows from code; its
elapsed cost was unmeasured at audit time. The subsequent benchmark linked
above measures the startup delay. It does not require a deep large file.

Later discovery can also wait for small-file batches: the same workers take
`List` and `Files`, the latter with up to 256 files copied sequentially.
The pool is LIFO. These facts permit delayed discovery for some trees;
they do not establish starvation or a bottleneck in the measured fixture.
[Walk::work](../../crates/gozler-fileops/src/copy.rs),
[FILES_PER_ITEM](../../crates/gozler-fileops/src/listing.rs),
[Pool::next](../../crates/gozler-fileops/src/pool.rs)

Before building index-assisted promotion, test the simpler alternative:
classify or dispatch direct large files before initializing every immediate
subfolder. It uses information already returned by the required listing.
Any production reordering still needs conflict-policy and merged/shared-tree
review, because final-name decisions have one authority and ordering can
matter. This alternative cannot find a large file whose parent has not yet
been listed; an index hint might help in that separate case.

## What an index hint can and cannot supply

The file-operation `Index` trait currently exposes **only totals** below
selected folders. Prepare reads selected metadata, destination names,
volumes and totals; it does not recursively discover the source tree.
[totals.rs](../../crates/gozler-fileops/src/totals.rs),
[prepare.rs](../../crates/gozler-fileops/src/prepare.rs),
[survey.rs](../../crates/gozler-fileops/src/survey.rs)

The existing query API can ask for file-only matches within a folder scope,
filter by a size range, sort by size and limit returned rows. Client rows
include the reconstructed path, optional exact UTF-16 path, size and
attributes, and the reply reports incomplete volumes and pending metadata.
An ordinary result item does not expose a stable file ID to fileops.
The query still scans candidate entries before selecting/sorting its
limited result; a one-row reply is not a constant-time size lookup.
[SearchRequest, Returns, Rows, ResultItem](../../crates/gozler-core/src/proto/types.rs),
[query grammar](../../crates/gozler-core/src/query/parse.rs),
[find_matches](../../crates/gozler-core/src/query/engine.rs),
[Client::query](../../crates/gozler-client/src/client.rs)

A future bounded background query could hint which accepted descendant's
parent to visit earlier. Directly manufacturing a copy task from an indexed
path is a larger change. It would need all of these existing obligations:

- Stay inside the accepted roots and source/destination volume pair, with
  their policy, cancellation, admission, recovery and progress owner.
- Recheck live source kind and metadata, respect relative no-follow traversal
  and special filter views, and avoid descending through a name surrogate.
- Establish destination ancestors through the same initialization and
  name authority used by the normal walk. `Target::child` caches shared
  destinations only when `shared` requests it; two arbitrary construction
  paths cannot assume they automatically get the same `Target`.
- Claim the accepted source name once, so the later normal walk cannot copy
  it again. Hard-linked names are separate selected entries; deduplicating
  by file ID alone would discard a name that must be copied.
- Retain ordinary discovery as fallback. An unavailable, incomplete or stale
  hint must never omit work or grant permission to read a different tree.

Authorities: [receive.rs](../../crates/gozler-fileops/src/receive.rs),
[tree.rs](../../crates/gozler-fileops/src/tree.rs),
[copy.rs](../../crates/gozler-fileops/src/copy.rs),
[place.rs](../../crates/gozler-fileops/src/place.rs),
[name rules](../../crates/gozler-fileops/src/names.rs),
[entry vocabulary](../../CONTEXT.md).

That favors hinting the existing walk over creating a second traversal or
copy authority. Whether it earns its lookup and coordination cost is an
unmeasured hypothesis.

## Distinguishing measurements

Repeat the original two-root mixed and separate-overlap cases alongside a
serial baseline, with the same cache, completion and flush rules. Split
timestamps into preparation, submit/start, destination large-file creation,
large completion, small completion, Queue completion and final flush.
If large creation is equally early but the copies diverge afterward, early
discovery does not explain the difference. Then inspect storage/CPU activity
and only measure lock waits if the evidence points to synchronization.

For discovery, use the **new** wide-tree scenario above. Compare normal
one-root traversal with upfront selection of that root's children as a
diagnostic upper bound. Disclose that the latter changes intake, identity
checks, preparation cost and selected-root scheduling; it is not an index
implementation and does not preserve every production semantic. Time both
end-to-end and after preparation.

`Progress.current` is the most recently started file, so small files can
overwrite the large file immediately. Production byte/item reports do not
wake the Queue: `reported()` is empty outside tests. Queue wakes alone are
therefore unsuitable for observing a large-file start.
[Progress::start_file and reported](../../crates/gozler-fileops/src/operation.rs),
[Queue wake sites](../../crates/gozler-fileops/src/queue.rs)

A background harness observer may poll whether the destination large file
exists at a bounded interval, recording the last-absent/first-present
interval as an **approximate creation time**. Creation is not first useful
data transfer and preallocation can make length misleading. Use identical
observation in compared cases; these metadata probes themselves add work.
Do not make a test depend on a precise 20 ms scheduling deadline.

Only if the large lane measurably starts late and that delay affects total
completion should an index-hint experiment follow. Compare its lookup and
ownership costs against ordinary traversal and the simpler direct-file
ordering change. No exact ideal companion size or adaptive policy follows
from this audit.

All disk and daemon waits traced here run on preparation/transfer workers.
No new UI-thread disk or daemon wait was found in this scoped audit; it is
not a general UI responsiveness review.
