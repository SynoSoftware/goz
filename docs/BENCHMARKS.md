# Benchmarks

goz vs Everything 1.4.1.1032 (`es.exe` 1.1.0.30), same machine, each warm. Single substring term, `-sort name` on both, full result set, median of 6 runs after 2 warmups, interleaved. Median milliseconds; **bold** is faster.

| Query                    | Matches   | Everything (file) | goz (file) | Everything (pipe) | goz (pipe) |
| ------------------------ | --------: | ----------------: | ---------: | ----------------: | ---------: |
| `kernel32.dll`           | 38        | 47.5              | **27.0**   | 32.0              | **14.0**   |
| `.pdf`                   | 424       | 51.0              | **27.0**   | 39.5              | **14.5**   |
| `.config`                | 4,287     | 71.5              | **35.0**   | 44.0              | **18.5**   |
| `.mui`                   | 21,789    | 148.0             | **39.0**   | 93.5              | **26.5**   |
| `.dll`                   | 141,666   | 724.0             | **139.0**  | 416.5             | **119.0**  |
| `e` (~73% of all files)  | 2,884,148 | 14,700            | **2,394**  | 7,511             | **2,470**  |

Faster in every cell: 1.8x to 6.1x to a file, 2.3x to 3.5x piped.

## Folder-scoped queries

For the file-manager case (`-path <folder> <term> -n 200`), goz walks the folder's subtree via per-entry child chains instead of scanning the volume, so cost tracks the folder, not the disk. Scoped to a ~12k-entry Downloads, every query, from `invoice` to a single letter, answers in ~17 ms end-to-end vs `es.exe`'s ~26 ms. Broad single-letter scoped queries previously spiked to ~300 ms; the subtree walk made breadth irrelevant.

## Setup

Ryzen 7 7800X3D (8C/16T), 31 GB RAM, Windows 11 Pro 26200, NVMe SSD. 4 live NTFS volumes, ~3.97M entries (C: 3.77M, F: 176k, D: 24k, E: 2.7k). Counts agreed exactly on five of six queries; `e` differed by 0.03% (live churn between the interleaved runs). `WindowsApps` queries were excluded (Everything under-reports them here). Process spawn (~18-21 ms) is included in every number. Inside the daemon, a selective query is single-digit milliseconds (`cargo bench -p gozler-core`).

## Memory footprint

Same box, each index warm at ~3.97M entries.

| Metric                                          | Everything | goz        |
| ----------------------------------------------- | ---------: | ---------: |
| Binaries shipped                                | **2.3 MB** | 4.1 MB     |
| Committed memory (private bytes)                | 527 MB     | **525 MB** |
| Resident RAM, idle after indexing               | 369 MB     | **12 MB**  |
| Resident RAM, steady after heavy querying       | 554 MB     | **215 MB** |
| Resident RAM, peak (2.88M-match sorted export)  | 1.2 GB     | 1.2 GB     |
| Committed memory, peak (same query)             | **1.2 GB** | 1.5 GB     |

goz wins the two resident-RAM rows that matter for a background daemon and edges out committed memory at rest. It loses two: a larger binary (a statically linked Rust build vs Everything's shared runtime), and ~0.3 GB more peak committed memory during a full sorted export, where sorting 2.88M matches materializes sort keys that Everything does not.

Why the resident footprint is small: filenames are interned (only ~31% are unique in the real world), entries carry a 4-byte name id, and paths are rebuilt from parent pointers. About 12 of the ~57 bytes per entry are the child chains that make folder-scoped queries cost the folder instead of the disk; this was a deliberate trade (~45 MB of committed memory for the folder-scoped speedup). After indexing, the daemon trims its working set once; set `GOZLER_PIN_WORKING_SET=1` to pin the index resident instead. Verify any of this with `gozler --status`, which prints the daemon's memory and a per-component breakdown of every index.

## Copy, move and delete engine

The engine's stage D measurement, run once on 2026-10-19 by `crates/gozler-fileops/examples/measure.rs`, elevated, on the owner's laptop: Intel Core i7-8550U (4C/8T), 16 GB RAM, Windows 11 build 26200, Toshiba KXG50ZNV512G NVMe SSD. The fixtures were on C: (NTFS); the last row is on V:, a ReFS Dev Drive that is a virtual disk file on the same SSD. Gozler's engine and the program were release builds.

**What is counted.** CPU cycles (`QueryProcessCycleTime`), at the owner's request, because other work on the machine changes wall time but not cycles: the measuring process's cycles, which hold Gozler's engine and the direct `CopyFile2` calls, plus the compared tool's process. Wall time is shown for reference. Time spent waiting for the disk costs no cycles, and the pages that Windows' lazy writer writes before a copy's closing flush count in the System process, which the program does not open.

**How each case runs.** The program waits 3 s, empties the standby list, so the case reads from the disk, and then runs the case. A copy ends with `FlushFileBuffers` on every copied file, so its count includes writing the data. The plan asked for a flush of the whole volume; Windows refused that with `ERROR_ACCESS_DENIED`, also with `SeManageVolumePrivilege` enabled. Each case runs once; a case within 10 % of its row's fewest cycles runs twice more, as does the fewest, and the middle run counts. robocopy and `rmdir` ran with the owner's approval; FastCopy and rmbrr were not compared. The "CopyFile2" rows call the Windows copy function directly, as the engine calls it for one file, because the speed settings cannot ask for a buffered 8 GiB copy or for `ioDesiredSize`; the small-file pair copies on 16 threads with and without the engine's temporary name (D10).

| Fixture | Case | CPU cycles (G) | Cycles | Runs | Wall time (s) |
| --- | --- | ---: | ---: | ---: | ---: |
| 30,000 files of 1 to 64 KB in 5,000 folders | Gozler delete, one worker per processor (8) | 27.08 | 903 k per file | 3 | 6.41 |
| | Gozler delete, 4 workers | **26.36** | 879 k per file | 3 | 7.35 |
| | Gozler delete, 16 workers | 31.04 | 1,035 k per file | 1 | 6.12 |
| | `rmdir /s /q` | 30.68 | 1,023 k per file | 1 | 19.07 |
| the same, copied within C: | Gozler copy, Windows engine, 16 workers | 140.35 | 139.7 M per MB | 1 | 90.92 |
| | `CopyFile2` on 16 threads, to the final name | **96.34** | 95.9 M per MB | 1 | 43.94 |
| | `CopyFile2` on 16 threads, to a temporary name, renamed (D10) | 137.70 | 137.1 M per MB | 1 | 95.79 |
| | `robocopy /e /mt:16` | 139.45 | 138.9 M per MB | 1 | 119.32 |
| one 8 GiB file | `CopyFile2`, buffered | 2.02 | 0.24 M per MB | 1 | 42.56 |
| | `CopyFile2`, unbuffered | 1.37 | 0.16 M per MB | 1 | 32.52 |
| | `CopyFile2`, unbuffered, `ioDesiredSize` 8 MiB | 1.42 | 0.17 M per MB | 1 | 29.22 |
| | `CopyFile2`, unbuffered, `ioDesiredSize` 32 MiB | **1.03** | 0.12 M per MB | 3 | 25.43 |
| | Gozler ring, 8 MiB x 8 | 1.13 | 0.13 M per MB | 3 | 23.77 |
| | Gozler ring, 2 MiB x 8 | 1.88 | 0.22 M per MB | 1 | 23.28 |
| | `robocopy /j` | 1.23 | 0.14 M per MB | 1 | 21.95 |
| 20 files of 32 MiB | Gozler, Windows engine buffered, 16 workers | 0.38 | 0.57 M per MB | 1 | 2.35 |
| | Gozler, Windows engine unbuffered, one at a time | **0.21** | 0.31 M per MB | 1 | 2.24 |
| 20 files of 64 MiB | Gozler, Windows engine buffered, 16 workers | 0.41 | 0.31 M per MB | 1 | 3.28 |
| | Gozler, Windows engine unbuffered, one at a time | **0.28** | 0.21 M per MB | 1 | 2.25 |
| 20 files of 128 MiB | Gozler, Windows engine buffered, 16 workers | 0.75 | 0.28 M per MB | 1 | 7.00 |
| | Gozler, Windows engine unbuffered, one at a time | **0.45** | 0.17 M per MB | 1 | 7.49 |
| 20 files of 64 MiB on the Dev Drive, within the volume | Gozler ring's clone (D20) | 0.36 | 0.27 M per MB | 1 | 0.38 |
| | Gozler, Windows engine (`CopyFile2`, unbuffered) | **0.22** | 0.16 M per MB | 1 | 0.14 |

What the rows decide, in the spec's terms:

- **D2, delete workers:** one per processor (8 here) and 4 differ by 3 % in cycles, within the rerun's noise, and 8 is 13 % faster in wall time; 16 costs 15 % more cycles. One per processor stays. Gozler's delete costs 12 % fewer cycles than `rmdir /s /q` and is 3 times faster.
- **D8, the large-file limit:** from 32 MiB to 128 MiB, the unbuffered copy one at a time costs 32 % to 45 % fewer cycles than the buffered copy on 16 workers, and its wall time is lower or at most 7 % higher. AngelCOPY's 32 MiB stays; BetterCopy's 256 MiB would copy these files buffered. A limit below 32 MiB was not measured.
- **D9, `ioDesiredSize`:** 32 MiB costs 25 % fewer cycles than leaving it to `CopyFile2` and is 22 % faster; 8 MiB gives nothing. The engine now passes 32 MiB for an unbuffered copy. Without it, the engine's large-file copy cost more cycles than `robocopy /j` (1.37 G against 1.23 G); with it, fewer (1.03 G).
- **D7, ring or Windows for large files between SSDs:** the Windows engine with `ioDesiredSize` 32 MiB costs 9 % fewer cycles than the ring's SSD profile, so Automatic keeps the Windows engine. Within the ring, 8 MiB blocks cost 40 % fewer cycles than 2 MiB blocks, so the profile keeps 8 MiB x 8. In wall time the ring (21.8 to 23.8 s) and `robocopy /j` (22.0 s) were faster than the Windows engine (24.6 to 28.6 s).
- **D10, the temporary name:** writing each small file under `.gozler-partial` and renaming it costs 43 % more cycles than writing it to its final name, and more than doubles the wall time; Gozler's engine costs the same as that pair, so the rename is its whole extra cost. Gozler's small-file copy costs the same cycles as `robocopy /e /mt:16` (140.35 G against 139.45 G, one run each) and is 24 % faster.
- **D20, the clone:** `CopyFile2` with `COPY_FILE_NO_BUFFERING` still clones within the Dev Drive: it copied 1.28 GB in 0.14 s. Its clone costs fewer cycles than the ring's (0.22 G against 0.36 G for 20 files, about 7 M cycles more per file in the ring); both take under 0.4 s.

Not measured here, and in the engine's checks of `docs/plans/ui-after-install.md`: two disks, a hard disk, a USB drive and a share, whose winners replace Automatic's hard-disk, USB and share values.

### Stage E: final names and resume

Run on 2026-10-20 by the same program, elevated, on the same laptop and SSD, with stage D's fixtures divided by 5 at the owner's request: 6,000 files of 1 to 64 KB in 1,000 folders (201 MB), and one 1,600 MiB file. Counted and run as in stage D. The totals do not compare with stage D's, so the comparison is per file and per MB.

| Fixture | Case | CPU cycles (G) | Cycles | Runs | Wall time (s) |
| --- | --- | ---: | ---: | ---: | ---: |
| 6,000 files of 1 to 64 KB in 1,000 folders | Gozler copy, Windows engine, 16 workers, to the final name | 19.20 | 3.20 M per file, 95.5 M per MB | 3 | 13.38 |
| | `CopyFile2` on 16 threads, to the final name | 19.95 | 3.33 M per file, 99.2 M per MB | 3 | 12.53 |
| one 1,600 MiB file | Gozler, Windows engine unbuffered, `ioDesiredSize` 32 MiB (Automatic) | 0.60 | 0.36 M per MB | 1 | 4.65 |
| | Gozler ring, 8 MiB x 8 | 0.39 | 0.23 M per MB | 1 | 4.25 |
| the 1,600 MiB file and a copy that holds all its bytes | Continue: the ring compares the whole file and writes nothing | 1.10 | 0.66 M per MB | 1 | 1.51 |

What the rows decide:

- **D10, the final name:** Gozler's small-file copy now costs what `CopyFile2` costs straight to the final name, 95.5 M cycles per MB against 139.7 M in stage D, when it wrote under a temporary name and renamed. Writing to the final name removed the rename's cost and added none.
- **D10, resume:** the compare read the two files at 1.11 GB/s (2.22 GB/s in total), against 0.41 GB/s for a copy by the Windows engine and 0.39 GB/s by the ring (medians of 4 runs). So a 50 GB file stopped at 49 GB resumes in about 47 s (49 GB compared, then 1 GB copied) against about 120 s to copy it again, and writes 1 GB instead of 49 GB. Computed from these rates; nothing near 50 GB was copied.
- **D7:** the Windows engine had the shorter wall time in 3 of 4 runs of the large file (median 4.09 s against 4.29 s), so Automatic keeps it for large files between SSDs. A file a fifth of stage D's size carries each copy's fixed cost over fewer MB, so both engines cost more per MB than in stage D.

### SSD overlap: small files beside one large file

Run on 2026-10-23 with `pwsh ./build.ps1 -Bench ssd_copy`, through the real
Queue and device admission, on the working-tree engine. The machine has
8 logical processors, Toshiba KXG50ZNV512G NVMe storage, Windows build
26200.9711, and about 110 GB free on C:. Sources and destination were on
that same physical SSD, on NTFS; the storage query reported no seek penalty,
NVMe command queueing and physical disk 0. No cloning was involved. Existing
security settings were preserved: Defender reported antivirus enabled and
real-time protection disabled. This differs from a test with synchronous
antivirus scanning enabled.

The disposable fixture contained 6,000 files of 1–64 KiB in 1,000 folders,
199,612,667 bytes in total, plus one 1,600 MiB file. Each case ran three
times, with case order rotated between rounds. Small source files were read
before each case to warm the system cache. The large file used Automatic's
unbuffered Windows engine. Preparation was outside timing. Timing began
before submission, ended first when the Queue reported all work complete,
and ended again after opening and flushing every destination file. Output
file count and total byte count were checked in every sample. Fixtures were
removed; [raw samples](research/ssd-copy-2026-10-23.csv) are retained.

Each table value is its own column's median, not necessarily the same sample.
The range is the minimum and maximum of time including the final flush.
Class completion times mean Queue-reported completion, before the final
flush; the mixed operation exposes no separate class completion event.
Process cycles include the final flush, but exclude System and other
processes. These wall-time results come from a shared interactive machine.

| Case | Copy complete (s) | Including flush (s) | Flush-inclusive range (s) | Small complete (s) | Large complete (s) | CPU cycles (G) |
| --- | ---: | ---: | --- | ---: | ---: | ---: |
| Separate transfers, limit 1, small first, 16 small workers | 7.24 | 14.74 | 14.68–15.06 | 2.32 | 7.24 | 21.66 |
| Separate transfers, limit 1, large first, 16 small workers | 6.76 | 16.83 | 15.30–17.23 | 6.76 | 4.53 | 20.63 |
| One mixed transfer, existing large lane beside 16 small workers | 6.75 | 14.17 | 14.04–15.46 | — | — | 19.01 |
| Separate transfers, limit 2, large beside 4 small workers | 6.29 | 13.43 | 12.16–14.56 | 6.29 | 5.55 | 17.87 |
| Separate transfers, limit 2, large beside 8 small workers | 5.99 | 13.12 | 12.30–13.90 | 5.25 | 5.13 | 18.99 |
| Separate transfers, limit 2, large beside 16 small workers | 5.59 | 12.46 | 9.22–13.36 | 5.30 | 5.48 | 20.90 |

What this establishes:

- Overlapping separate large-only and small-only transfers was useful in
  this workload. With 16 small workers, median time including the final
  flush fell 15.5% against small-first serialization. Every overlap sample
  was below every small-first serialized sample, although overlap's range
  was wide. Four and eight small workers also improved total time in each
  corresponding round.
- The improvement trades away small-file latency: the small transfer's
  median reported completion increased from 2.32 s to 5.30 s with 16
  workers. It does not make each class finish sooner.
- The existing mixed operation's median was 3.9% below small-first
  serialization, but its ranges overlap and one round was slower. This
  does not establish a reliable gain for that case or explain why it
  differed from the two-operation mixture.
- Queue admission is not an exact account of device I/O: small buffered
  copies can report completion while Windows still has writes pending.
  Omitting the final flush would overstate the elapsed-time benefit.

This is evidence to pursue SSD scheduling, not a new default or a universal
rule to keep a large file active. It tests one available large file, not
continuous replenishment, several large-file sizes, cold small-file reads,
two physical SSDs, sustained cache exhaustion or antivirus scanning. The
large file was known up front; no indexed candidate lookup was timed.
The next experiment should isolate whether indexed lookahead starts useful
large-file work earlier in a real selected tree, while sharing the Queue's
existing ownership and retaining the disk walk for correctness.

### SSD follow-up: repeatability and discovery

Run on 2026-10-23 with `pwsh ./build.ps1 -Bench ssd_copy_followup`, using
the same SSD, filesystem, fixture bytes and existing antivirus settings as
the first overlap experiment. One untabulated mixed warm-up took 14.485 s
including flush. Five rounds then rotated five cases so each case occupied
each position once. [The 25 raw samples](research/ssd-copy-followup-2026-10-23.csv)
also retain preparation and large-destination observation times.

The three control cases retain the original source layout. Both mixed and
separate-overlap controls receive the large file up front: recursive
discovery cannot explain a difference between them. The new wide fixture
puts the large file directly beside 1,000 immediate subfolders, each holding
six small files. Hard links reuse the original source data, so this fixture
has the same file count and byte sizes without another large-data write.
The copied output still contains independent files.

`wide-walk` selects that one root. `wide-listed` supplies its 1,001 children
up front, with the same destination layout. This is a diagnostic for earlier
knowledge, not an implementation of indexed promotion: it changes intake,
the selected roots and preparation. Child enumeration and destination-root
creation are included in its preparation time. Small-source warming remains
outside timing. The combined column sums each sample's preparation and
flush-inclusive copy times; it excludes harness setup and warming.

The harness observes destination-file existence between snapshots, waiting
up to 20 ms for a Queue wake between probes. It records the actual last-absent
and first-present interval rather than assuming a 20 ms timing guarantee.
This is a proxy for large-copy startup, not first data transfer or large-file
completion. All compared cases use the same observation. Table values are
independent column medians.

| Case | Prepare (s) | Large first observed (s after submit) | Copy complete (s) | Including flush (s) | Flush-inclusive range (s) | Prepare + copy + flush (s) |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| Serial, small first, 16 workers | 0.008 | 2.211 | 6.78 | 13.46 | 13.20–17.23 | 13.47 |
| One mixed transfer, 16 workers | 0.007 | 0.048 | 4.85 | 13.17 | 11.04–13.47 | 13.18 |
| Two overlapping transfers, 16 small workers | 0.008 | 0.028 | 5.25 | 12.97 | 9.79–13.22 | 12.97 |
| Wide folder, normal walk | 0.007 | 2.491 | 7.15 | 14.16 | 13.90–14.88 | 14.17 |
| Wide folder, children supplied up front | 0.198 | 0.033 | 5.38 | 13.47 | 11.71–14.52 | 13.67 |

The larger run narrows the first result. Overlap's median flush-inclusive
advantage over serialization is 3.7%, rather than the first run's 15.5%.
Separate overlap beats the mixed operation in only two of five paired
rounds; their ranges overlap substantially. There is no reliable evidence
here that splitting an already mixed workload into more operations helps.
Shared locks and startup differences remain hypotheses, not diagnosed costs.

The discovery delay is repeatable. Every wide-walk sample first observes
the large file after 2.15–2.80 s; every upfront sample does so after
0.024–0.044 s. `Walk::list` initializes all immediate subfolders before it
dispatches direct files, even though its listing already returned those
files. Upfront knowledge reduces median reported copy time by 1.78 s.
Including preparation and the final flush, the median reduction is only
0.50 s (3.5%), and the ranges overlap; two paired upfront cases are slower.
Earlier startup is established more strongly than a durable-copy speedup.

The simplest production experiment is to dispatch known direct large files
earlier through the existing lane, while preserving destination-name and
conflict-policy ownership. It should be tested before adding index hints for
this particular delay. Indexed hints could instead prioritize a large file's
not-yet-visited parent. [The source audit](research/ssd-copy-followup-audit.md)
records the ownership obligations and unmeasured alternatives. No production
ordering or admission default was changed by either experiment.

### Indexed size lookup cost

After the copy run, the installed `C:\Program Files\Gozler\gozler.exe`
reported version 0.4.0. Fifteen read-only probes measured that installed
client and its matching service, not the working-tree 0.5.0 copy benchmark.
Each used `-timeout 5000 -n 1 -size --json -path` scoped to this repository,
with `file:` and the byte range below as a `size:` filter. Five rounds
rotated the three ranges. Timings include a new CLI process, pipe exchange
and JSON output. Returned paths were not copied; live size checks happened
after timing. [Raw lookup samples](research/ssd-copy-index-lookup-2026-10-23.csv)
retain counts and sizes without paths.

| Size band | Median lookup (ms) | Range (ms) | Matches | Returned |
| --- | ---: | --- | ---: | ---: |
| 32–128 MiB | 61.14 | 60.19–73.82 | 182 | 1 |
| 128–512 MiB | 63.62 | 58.14–65.57 | 0 | 0 |
| 512–2,048 MiB | 62.08 | 60.42–71.24 | 0 | 0 |

The returned candidate's indexed and live size both measured 65,228,800
bytes. Every reply reported complete volumes and no pending metadata.
Exit 10 for the nonempty range means the requested limit omitted matches;
the empty ranges exited 0. A sandbox pipe-access refusal was resolved by
repeating the same read-only query outside the sandbox.

The measured lookup cost is small compared with the wide-folder startup
delay, which makes bounded background hints plausible. It does not measure
integrated scheduling, permission and identity checks, destination setup,
deduplication against the walk or a sustained speedup. Empty ranges also
show why a policy must keep ordinary discovery working when no suitable
candidate exists.

### SSD early dispatch: before and after

Run on 2026-10-23 with `pwsh ./build.ps1 -Bench ssd_copy_wide` once before
and once after changing `Walk::list`. Each run used an untabulated mixed
warm-up, then five rounds alternating which wide case ran first. The SSD,
NTFS volume, fixture bytes, engines, worker counts, warming, observation and
final flush procedure were the same as above. Warm-ups took 12.920 s before
and 13.929 s after. Both runs checked 6,001 copied files and their total
size in every sample and removed their disposable fixtures.
[Before samples](research/ssd-copy-wide-before-2026-10-23.csv),
[after samples](research/ssd-copy-wide-after-2026-10-23.csv).

The change dispatches a folder's direct large files through the existing
lane before opening and creating its subfolders, on plans whose large lane
runs beside the workers. Destination names, conflict decisions, ring routing
and folder lifetime accounting still use their existing owners. Hard-disk
and share plans retain their ordering. No index query or admission change
is involved. The children-supplied case is a control: its large file was
already selected up front, and its six-file subfolders contain no large file.

| Case | Large first observed (s) | Copy complete (s) | Including flush (s) | Flush-inclusive range (s) | Prepare + copy + flush (s) |
| --- | ---: | ---: | ---: | --- | ---: |
| Normal walk, before | 2.363 | 7.40 | 14.32 | 11.51–14.70 | 14.32 |
| Normal walk, after | 0.176 | 5.12 | 11.84 | 11.17–13.15 | 11.85 |
| Children supplied, before | 0.038 | 5.66 | 13.03 | 11.22–13.89 | 13.18 |
| Children supplied, after | 0.024 | 5.42 | 13.25 | 10.80–15.53 | 13.38 |

For the normal walk, median reported completion fell 30.8%, and every after
sample completed before the fastest before sample. Median time including
the final flush fell 17.3%; four of five corresponding rounds improved,
but ranges overlap. The control's median reported completion fell 4.1%,
while its flush-inclusive median increased 1.6%. This supports retaining
early dispatch for this startup delay; it does not establish a general
17% SSD throughput gain. The process-cycle medians fell in both cases
(walk 21.69 to 18.44 G; control 21.82 to 18.13 G), so attributing their
reduction to this change would be unwarranted.

Large-file existence was first observed at 0.162–0.180 s after submission
in every changed normal-walk sample, versus 2.261–2.637 s before. This
establishes earlier destination creation, not the time of its first byte
or its individual completion. Separate small/large completion times remain
unavailable for a mixed operation. The change is retained in the working
tree; it neither finds a large file deeper in an unvisited subtree nor
keeps the lane continuously occupied. Indexed hints still need their own
integrated benchmark, as do other SSDs, sustained writes and antivirus modes.
