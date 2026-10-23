# SSD copy scheduling

Researched on 2026-10-23 against the working tree and primary sources.
This note evaluates the copy-behaviour plan and records the tested early
dispatch change; the engine spec owns its execution rule.
The SSD experiments and lookup probes are recorded in
[BENCHMARKS.md](../BENCHMARKS.md#ssd-overlap-small-files-beside-one-large-file),
with [18 first-run samples](ssd-copy-2026-10-23.csv),
[25 follow-up samples](ssd-copy-followup-2026-10-23.csv) and
[15 lookup probes](ssd-copy-index-lookup-2026-10-23.csv), followed by
[10 before](ssd-copy-wide-before-2026-10-23.csv) and
[10 after](ssd-copy-wide-after-2026-10-23.csv) early-dispatch samples.

## Finding

The proposed combination, **one large file alongside several small files**,
already runs inside an admitted SSD transfer. The plan's default of one
transfer per physical device does not mean one file at a time. It can,
however, prevent a separately submitted large-file transfer from running
beside a small-file transfer on the same SSD. Those are different scheduling
decisions. [Device admission](../DESIGN.md#device-admission)
explicitly preserves the workers and ring inside each transfer.

The existing mixed-file design is reasonable. The first run favored two
overlapping transfers, but a five-round follow-up reduced their median
advantage over serialization to 3.7% and found no consistent advantage over
one mixed transfer. That does not justify increasing admission by default.

A clearer opportunity is discovery order: in a wide folder the normal walk
first observed the large destination after a median 2.49 s, compared with
0.033 s when children were supplied up front. Including preparation and
the final flush, the median difference was 0.50 s, with overlapping ranges.
Indexed size lookups through installed Gozler took about 61–64 ms including
CLI startup. These support testing earlier dispatch and bounded lookahead,
without establishing an always-active large-file policy or optimal size.

The subsequent production experiment removed the wide-folder startup
delay using the listing the walk already has. Direct large files now enter
the existing separate lane before subfolders are initialized. Across five
before and five after normal-walk samples, median large-destination
observation moved from 2.363 s to 0.176 s, reported copy completion from
7.40 s to 5.12 s, and time including the final flush from 14.32 s to 11.84 s.
The upfront-children control's flush-inclusive median stayed roughly flat.
These results justify keeping the small ordering change on this fixture;
the 17.3% flush-inclusive improvement is not a general SSD speed claim.
[Measurements and limitations](../BENCHMARKS.md#ssd-early-dispatch-before-and-after)

## What the code does now

These are working-tree findings, not claims inferred from drive specifications:

| Scope | Current behavior | Authority |
| --- | --- | --- |
| Admission | A transfer groups roots for one source/destination volume pair. Every distinct physical device is granted together. Default capacity is one transfer per device; Settings permits 1–3. | [Device admission](../DESIGN.md#device-admission) |
| Inside an SSD transfer | Automatic selects 16 small-file workers and a separate large-file lane, with a 32 MiB size boundary. Same-physical-SSD and separate-SSD copies both get the lane. | [CopyPlan::new](../../crates/gozler-fileops/src/volumes.rs), [Automatic preset](../plans/fast-copy-delete-spec.md#speed-settings) |
| Dispatch | The copy walk starts the small workers plus the large lane. A folder's known direct large files enter the separate lane before its subfolders are initialized; small files enter bounded batches on the workers. Plans without a separate lane retain their ordering. | [copy.rs](../../crates/gozler-fileops/src/copy.rs), [pool.rs](../../crates/gozler-fileops/src/pool.rs) |
| Multiple transfers | Each admitted transfer has its own plan and worker pool. A limit of three can therefore permit up to 48 small workers plus three large lanes on an SSD with Automatic settings. This is a capacity ceiling, not necessarily the active count. | The admission rule and worker creation above |

There is no second class of admission for a file or worker. Raising the
transfer limit is consequently a coarse way to obtain overlap: it can also
multiply the same kind of work already saturating a device.
"Small" here means below the configured large-file limit, not only tiny
files; with Automatic, it includes files just below 32 MiB.

## What primary sources establish

### File count is not device queue depth

Microsoft's DiskSpd controls target files, threads and outstanding requests
separately. One thread accessing one file can maintain several overlapped
requests. Consequently, neither one copying file nor 16 copying files tells
us the SSD's actual queue depth.
[DiskSpd threading and concurrency](https://github.com/microsoft/diskspd/wiki/Threading-and-concurrency)

`CopyFile2` recommends unbuffered I/O for very large files. Its low-free-space
mode explicitly disables overlapped I/O; **inference:** normal copying may
pipeline I/O, but this documentation supplies no fixed queue depth to assume.
The V2 `ioDesiredSize` is a requested I/O size that Windows may reduce; it is
not a file-concurrency or queue-depth setting.
[CopyFile2 flags](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-copyfile2_extended_parameters),
[V2 parameters](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-copyfile2_extended_parameters_v2)

### Small-file work can leave opportunities for overlap

Microsoft explains that file creation carries filesystem and SMB costs;
antivirus work can repeat for each transferred file. Parallel copies can
transfer data while other files are being created, but excessive threads can
hurt performance. This source discusses **SMB**, not a local SSD benchmark.
[Slow SMB transfers](https://learn.microsoft.com/en-us/troubleshoot/windows-server/networking/slow-smb-file-transfer)

There is also a local filter cost to investigate: Microsoft's Dev Drive
performance mode defers antivirus scans until after the open operation,
instead of scanning synchronously during it. Its performance analyzer can
attribute scan impact to files and processes.
[Dev Drive performance mode](https://learn.microsoft.com/en-us/defender-endpoint/microsoft-defender-endpoint-antivirus-performance-mode),
[Defender performance analyzer](https://learn.microsoft.com/en-us/defender-endpoint/performance-analyzer-reference)

**Inference:** a large copy might use bandwidth left idle by small-file
creation, metadata and filter delays. Several small-file workers may already
hide those delays, so adding the large copy is not guaranteed to help.
It does not eliminate any of the small files' required work.

### SSDs have concurrency and contention

Samsung's SATA 870 EVO specifies different random-read results at QD1 and
QD32: 13,000 versus 98,000 IOPS. That demonstrates that concurrency matters
even for SATA SSDs; it does not recommend 32 file-copy workers. The numbers
describe block workloads, not end-to-end filesystem copies.
[870 EVO data sheet, page 3](https://download.semiconductor.samsung.com/resources/data-sheet/Samsung_SSD_870_EVO_Data_Sheet_Rev1.1_230509_10129500053000.pdf)

NVMe supports many submission/completion queues, though implemented limits
are device dependent. This supplies room for parallel I/O, not a rule that
more active copies always improve throughput.
[NVM Express architectural overview](https://nvmexpress.org/base-nvm-express-part-one/)

Micron explains that reads, writes and internal garbage collection contend
for NAND resources.
Its evidence is from data-center SSD design, not this laptop's copy workload;
the useful conclusion is the mechanism, not its product performance figures.
[Micron on SSD latency](https://www.micron.com/about/blog/applications/data-center/why-latency-in-data-center-ssds-matters-and-how-micron-became-best-in-class)

**Inference:** on one physical SSD, source reads and destination writes
already compete within that device. A large stream may delay small-file I/O.
Two independent SSDs separate the source and destination media, though each
can still bottleneck, and shared host resources remain. Different volume
letters or a virtual disk do not establish independent physical devices.

### USB and sustained writes need separate treatment

Microsoft distinguishes UAS, which can process commands in parallel and use
native command queuing, from the older bulk-only transport. Therefore a USB
label alone cannot predict useful concurrency; the enclosure, transport and
link matter along with its SSD.
[Windows USB FAQ](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/usb-faq--introductory-level)

Samsung identifies its sequential-write headline as TurboWrite performance.
After the TurboWrite region, the 250/500 GB 870 EVO is specified at 300 MB/s,
compared with the 530 MB/s headline; larger capacities differ. A short run
is not enough to establish sustained behavior for every SSD.
[870 EVO data sheet](https://download.semiconductor.samsung.com/resources/data-sheet/Samsung_SSD_870_EVO_Data_Sheet_Rev1.1_230509_10129500053000.pdf)

## What our measurements leave unanswered

[BENCHMARKS.md](../BENCHMARKS.md#copy-move-and-delete-engine) originally
recorded separate small-only and large-only cases on one laptop's NVMe SSD.
The later SSD overlap experiment adds mixed large/small cases and compares
one versus two admitted transfers. There is still no two-physical-SSD case.
The earlier ReFS Dev Drive was a virtual disk on that same SSD.

The 32–128 MiB cases changed buffering and worker count together, so they
cannot isolate the benefit of either change. CPU cycles were the principal
metric, with wall time for reference. Waiting for storage costs no process
cycles, and some buffered writing is charged to System. Thus a CPU-cycle
winner does not establish the shortest completion time or least latency.

## Index-assisted lookahead

The owner's follow-up identifies a separate opportunity: Gozler's index can
find a suitably sized pending file before the copy walk reaches its folder.
The advantage to investigate is earlier knowledge of the accepted workload,
without a preliminary disk walk. Other copiers can learn sizes while listing
folders; this investigation does not establish that no other copier uses
an index or schedules by size.

The current engine does not use the index this way. Its `Index` interface
asks only for totals below selected folders. Selected entries receive their
metadata from disk, and the copy walk discovers nested files and dispatches
them to its lanes. The search engine can filter indexed sizes, but executes
queries by scanning candidates; it does not promise a constant-time lookup
for the file closest to a requested size.
[Index interface](../../crates/gozler-fileops/src/totals.rs),
[source intake](../../crates/gozler-fileops/src/receive.rs),
[copy dispatch](../../crates/gozler-fileops/src/copy.rs),
[query executor](../../crates/gozler-core/src/query/engine.rs)

**Hypothesis:** while small-file work runs, a bounded index query could
identify a large file still pending within the accepted selection and its
source/destination pair, making the existing large lane useful earlier.
This differs from increasing the number of whole transfers. An illustrative
case is a large file deep in a folder tree that the ordinary walk would
reach late. There is no benefit if the lane already has suitable work or
the device is already saturated.

The index supplies a scheduling hint, not a copy manifest. Missing or stale
metadata must not omit files, add files outside the accepted selection, or
replace the engine's live identity, permissions, reparse and destination
checks. Promoting a nested file would also have to share the existing
destination initialization and name ownership, and leave the later walk
unable to copy it twice. The normal walk remains the fallback when index
information is unavailable, incomplete or late. The lookup runs in the
background and never delays initial copying.
[intake and destination ownership](../../crates/gozler-fileops/src/prepare.rs),
[live copy checks](../../crates/gozler-fileops/src/copy.rs),
[index completeness checks](../../crates/gozler-fileops/src/totals.rs)

File size alone does not identify the best companion workload. Measured
throughput could estimate how long a candidate would occupy the large lane;
metadata/filter costs and SSD contention still decide whether overlap helps.
Begin with a size band rather than inventing an exact optimal byte count.
No target duration or size band has been established by this research.

After establishing the benefit of mixed copying, compare ordinary discovery
against index-assisted discovery on the same tree with a late-discovered
large file. Include lookup cost, time until the large lane starts, both
classes' completion times and total elapsed time. This isolates the index's
contribution from the benefit of the mixed lanes that already exist.

## Initial recommendation and experiment matrix

Keep the separate large lane and small-worker pool. The first overlap
experiment supports pursuing complementary transfer scheduling on SSDs.
Keep the present admission default provisionally; describe it as a
contention policy, not a measured SSD optimum. The existing mixed operation
showed a smaller, inconsistent improvement, so neither a universal default
change nor indexed lookahead's benefit follows from these samples.

First use the existing small fixture (6,000 files of 1–64 KB in 1,000
folders) plus one 1,600 MiB file, copying real data to fresh destinations.
Run the following on the same physical SSD first; repeat on two physical
SSDs before generalizing to that topology. This was the proposed matrix;
the completed subset and its exact fixture are in the benchmark record.

| Case | Purpose |
| --- | --- |
| Small set alone; large file alone | Establish each workload's baseline. |
| Small set then large file; reverse order | Establish elapsed time when the classes do not overlap; detect order/cache effects. |
| One mixed transfer, 16 small workers plus one large lane | Measure the user's proposed combination as already implemented. |
| Two submitted transfers, limit 1 then limit 2 | Isolate the admission rule using the same small-only and large-only transfers. |
| One mixed transfer, 4 or 8 small workers plus one large lane | Test whether reducing metadata/filter concurrency improves the mixture. |

Use total elapsed time until both classes finish and each class's completion
time as the primary measures. Record small files/s, large-file rate and CPU
cycles as supporting measures. Report each separately:
higher combined MB/s can merely mean more large-file bytes while the small
set finishes later. Record device queueing/latency and Defender scan cost
only if needed to explain the result, using installed tools.

Keep buffering, copy engine, verification and completion/flush semantics the
same between cases. Use at least three interleaved repetitions, reporting
median and range, with SSD model, connection, filesystem, free space,
antivirus mode and cache conditions. Preserve normal protection settings.
Avoid cloning/offload if the question is actual media-copy performance, or
report those cases separately. ReFS cloning can bypass the work being tested.
The existing benchmark records that behavior explicitly.

Only if the small experiment shows a useful difference, repeat its winner
and baseline for a sustained workload beyond the drive's write-cache region;
check SATA and USB/UAS separately when those devices are available. This
keeps the first investigation bounded without mistaking a short NVMe run
for a universal policy.

The first experiment supports overlapping a large-only transfer with a
small-only transfer on this SSD. The open policy question is whether that
gain is robust across workloads and worth the later completion of small
files. Indexed lookahead and continuously replenishing the large lane remain
unmeasured. If increasing slots causes contention, evaluate a shared device
budget within the existing Queue rather than multiplying full worker pools.

## First experiment verification

One `build.ps1 -Bench ssd_copy` run compiled the benchmark and completed all
18 samples successfully. Each sample checked the destination's file count
and total size, then its disposable output was removed. The final fixture
directory was also removed. The new benchmark source passes rustfmt.

One full `build.ps1` run encountered formatting errors in other sessions'
edits to `operation.rs`, `queue.rs`, synchronization and UI files. Clippy
then waited on another build's lock. The validation process and its children
were stopped at the repository's other-session error boundary; those files
were left unchanged. The run is not green. No `-Test` run or new regression
test was added: the change supplies a measurement harness, with no production
behavior change. Ubuntu and minimum-Rust checks for the new benchmark target
remain for CI. No dependency was added.

## Follow-up verification

One `build.ps1 -Bench ssd_copy_followup` run completed a warm-up and all
25 recorded samples, including the creation-time probes and wide-tree
diagnostic. Every sample checked copied file count and byte totals. Fifteen
subsequent read-only installed-client queries recorded size-band lookup cost;
the nonempty replies' live sizes matched the index. All disposable fixture
directories were removed, with the raw CSV results retained here.

One full `build.ps1` run for this follow-up is green: formatting, clippy,
workspace tests and documentation passed, including 95 fileops tests and
240 UI tests with 14 existing ignored timing checks. Together with the first
experiment's interrupted validation, this investigation has two full-build
attempts. No `-Test` run or new regression test was added; production code
and behavior were unchanged. Ubuntu and minimum-Rust validation of the new
benchmark remain for CI; there is no new dependency.

## Early-dispatch verification

Two `build.ps1 -Bench ssd_copy_wide` runs completed, one before and one
after the production change, with a warm-up and ten recorded samples each.
All copied counts and sizes matched; both disposable fixture directories
were removed. The benchmark adds only a focused filter to the existing
harness. The production change reuses one dispatch helper and the existing
name, copy and lifetime authorities, without new workers or index requests.

The added regression test,
`early_large_files_and_subfolders_keep_their_own_output`, watches a generated
Keep both name colliding with a later subfolder name: the original
destination, copied large file and child content must all survive. It pins
content preservation rather than timing or which entry gets a numbered name.
Ordinary distinct-name copy tests and the compiler do not watch that failure.

One full `build.ps1` run for this change passed formatting, then encountered
other sessions' compile errors: `totals.rs` still initializes the removed
`VolumeStatus.progress`, and UI callers still use that field, `IndexProgress`
and `StatusActivity`. Validation was stopped at that boundary without
editing those files. The new regression test has therefore not run, and
the change is not fully validated or installed. This investigation now has
three full-build attempts, of which the middle follow-up run was green.
There was no `-Test` run. Ubuntu and minimum-Rust checks for the benchmark
target remain for CI; no dependency or platform boundary was added.
