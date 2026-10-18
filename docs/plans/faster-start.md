# Plan: a faster start

How to make an index complete sooner after gozlerd starts. Stage 0 measured
where the time goes. Stage 1 builds the index with one pass over the MFT
instead of two. Stage 2, saving the index between runs, is deferred, and this
plan keeps its requirements for the day it is needed.

## The problem

Every start of gozlerd builds each index from the MFT, because nothing is
saved between runs (`docs/DESIGN.md`, "Index coherence"). Until the build
ends, the volume is `bootstrapping` and every answer exits 2, "may be
incomplete". AI agents that search right after a restart get no answer they
can trust for that time. A rescan after a journal fault runs the same build.

## Schedule

- **Stage 0** is done (2026-10-18).
- **Stage 1** is next. The owner sets the date.
- **Stage 2** is deferred. It starts only when a user with an HDD or with many
  millions of entries reports that a start is still too slow after stage 1,
  and then only after the review that "Expert reviews" names.

## What we know

- **Start time.** On the owner's machine gozlerd built C: (2,004,500 entries)
  in 24.4 s and finished startup in 30.0 s (`gozlerd.log`, 2026-10-18,
  "startup indexing finished ... elapsed_s=30.0161787").
- **Index size.** `gozler --status` reports 194.6 MB of index memory for C:
  with 2,015,690 entries.
- **Two passes over the MFT.** `build_index` in `bootstrap.rs` reads the MFT
  twice: `FSCTL_ENUM_USN_DATA` gives one name, the parent and the attributes
  of each file; `FSCTL_QUERY_FILE_LAYOUT` then adds sizes, dates and every
  hard-link name. The file-layout pass is best-effort: a failed read stops it
  and leaves the remaining sizes and dates unknown, while the enumeration has
  already given every name (`enrich_from_layout`).
- **Upstream proposal.** mustafaahci/goz#1 (ZJHSteven, 2026-08-28) proposes
  to save each volume's index with its journal cursor (the journal position
  the index is current to), check both at start, replay the journal from
  there, and fall back to a full build on any mismatch. The reporter measured
  105 s to build 4.9 million entries, and a restart still building after
  170 s. Mustafa agreed with the direction and measured 16.0 s and 12.7 s for
  the two passes on his C: with 4.08 million entries. No pull request has
  arrived.
- **Journal size.** Mustafa's fixes that size the journal by volume
  (`f99fb35`, `1ba28b5`, `3b37318`) are in gozler. C: has a 250 MiB journal.
- **Drift that a restart heals.** `--status` on C: shows 126 hard-link
  changes not reconciled and 44,770 placeholders. These do not start a
  rescan; a restart clears them, because it builds the index again.
- **The rule already set.** `docs/DESIGN.md` allows saving state between
  runs only when the index and its cursor are saved and replaced together.
  A cursor alone must never skip a build.

## What we do not know

- **Whether gozlerd survives a Fast Startup boot.** Windows Fast Startup
  normally hibernates services with the kernel at shutdown. On the owner's
  machine most boots are full starts anyway, so this changes little there.
- **How long a journal replay takes** on a saved index. It matters only to
  stage 2.

## Stage 0: measure (done)

Results from the owner's machine, 2026-10-18:

- **Starts.** gozlerd's log began on 2026-10-18, and Windows does not log
  service starts here (no Service Control Manager event 7036), so starts are
  counted from boots: the System log's Kernel-Boot event 27 records 32 boots
  since 2026-08-31, of which 26 were full starts, 4 Fast Startup boots and 2
  resumes from hibernation. Each full start rebuilds every index: about 3.8 a
  week, plus each gozler upgrade.
- **The two passes.** `gozlerd scan` on C: (2,016,090 entries): enumeration
  12.47 s, file layout 10.46 s, 22.97 s in all.
- **Journal horizon.** The oldest change in the C: journal was from
  2026-10-17 20:36, read on 2026-10-18 17:05: about 20.5 hours on a
  development day. A saved index would cover a night off, but not a weekend,
  after which a start builds from the MFT anyway.
- **The single-pass experiment.** An uncommitted `gozlerd scan
  --compare-single-pass` built C: with two passes, one pass and two passes
  again, and compared the single-pass index with the second two-pass index
  by full path, folder flag, size, date and attributes.
  - Time with a warm disk cache: 16.09 s, single pass 11.58 s, 15.50 s. The
    enumeration took 5.5 s warm against 12.15 s cold, so at boot one pass
    saves about half of the build: a cold single pass took 10.11 s against
    22.14 s.
  - Entries: two passes 2,019,054 (60 without a path), single pass 2,019,117
    (none without a path). 38 entries were only in the two-pass index and
    101 only in the single-pass index; nearly all were files that changed
    between the builds (browser and app caches, temp files).
  - The enumeration never returns the `$Extend` folder, so the two-pass
    build leaves the NTFS metadata files under it without a path; the single
    pass gives them their path.
  - The file-layout pass returns the root folder's own record, which names
    the root as its parent. Taken as a name, it made the root its own parent
    and left no entry a path.

**Decision:** stage 1 is worth doing. Stage 2 is not, for now: after stage 1
a cold start of C: takes about 10 s, and a saved index would win at most that,
and only after a downtime shorter than the journal horizon.

## Stage 1: build with one MFT pass

`build_index` reads the file layout first and runs the enumeration only when
the file-layout pass stops part way. Every start and every rescan uses it,
because both call `build_index` today.

1. **The file-layout pass builds the index.** Each file-layout record goes
   through `VolumeIndex::insert_layout`: its first name that is not a DOS
   8.3 alias stands in for the enumeration record (which also fills a folder
   that an earlier child left as a placeholder), then `enrich` adds the other
   hard-link names, the size, the date and the attributes.
2. **The root only enriches.** The root folder's own record names itself as
   its parent. `insert_layout` gives it no name and no parent, so the root
   that `VolumeIndex::new` made stays the root. Without this rule no entry
   has a path (stage 0 results).
3. **A stopped file-layout pass falls back to the enumeration** on the same
   index, so a failed read never loses names, as today. A name the file
   layout already gave is rewritten to itself; a hard-linked file keeps the
   names the file layout gave it; a file the file layout never reached gets
   its name. Its size and date stay unknown, and the volume reports
   `metadata_pending`, as a stopped pass does today.
4. **A cancelled build stops between pages**, as today, and returns nothing.
5. **`$Extend` becomes searchable.** Its metadata files (`$Extend\$Deleted`,
   `$Extend\$RmMetadata` and the others) get their path, as `$Boot` and
   `$AttrDef` already have theirs. Today the same entries are in the index
   without a path, so no query finds them. Showing them is the default
   because gozler indexes every name on the volume; `docs/DESIGN.md` says
   so.
6. **The timings say what ran.** `BuiltIndex` and `gozlerd scan` report the
   file-layout time, and the enumeration time only when the fallback ran.
7. **The experiment goes.** The `--compare-single-pass` mode is removed; the
   fixture test below keeps its comparison.
8. **Comments and docs follow the code.** `bootstrap.rs`'s module comment
   and `build_index`'s comment describe the new order, and `enrich`'s comment
   about a rename between the two passes names the fallback as the only case
   where both passes run.

Tests, each with the failure it watches:

- **The bootstrap race covers both builds.** The property test
  `bootstrap_race_converges` in `index/tests.rs` ingests a shuffled snapshot
  through `insert_enum`, then replays the journal, and checks the paths and
  the index's internal chains. It gains a third random choice, the build:
  the enumeration alone as today, the file layout alone, or the file layout
  stopped at a random record followed by the enumeration. Without this the
  test that carries the bootstrap's correctness would exercise only the
  fallback, and a folder that arrives after its child, a replayed journal on
  a file-layout index and the fallback over a partly built index would have
  no test.
- **The root and hard links.** One fixture test, because the model in that
  property test has no root record and no hard links: a file-layout pass
  with the root's own record, a hard-linked file and a DOS-only alias gives
  every entry its path, the file both names, and the alias none. It watches
  the root loop that removed every path in the experiment.
- `layout_pass_reports_a_failed_read_as_stopped` stays as it is: it already
  shows that a failed read stops the pass.

**Done when:**

- `check.ps1` passes, with the tests above.
- The code review that "Expert reviews" names for stage 1 is done and its
  findings are resolved.
- After the owner installs the build and restarts Windows, the "startup
  indexing finished" line in `gozlerd.log` shows the cold start of C: against
  the 30.0 s baseline, and `gozler --status` shows the entry count of C:
  within a few hundred of the count before.

## Stage 2: save the index between runs (deferred)

Starts only on the condition in "Schedule". Each requirement below names the
failure a user would see without it.

1. **Writes the journal never saw.** A writer that skips the journal, such
   as Linux's ntfs-3g on a dual-boot machine, changes files while Windows is
   down. The saved cursor still validates, so without a rule the stale index
   goes `live` and lists deleted files with exit 0. That is the silent state
   `docs/DESIGN.md` forbids. The stage needs a check that detects this, or
   the owner's decision to accept the gap and say so in `docs/DESIGN.md` and
   `--status`.
2. **Who can read and write the file.** The saved index holds every name,
   size and date on the volume, read past NTFS permissions. Without
   protection, a standard user reads names they cannot list, or plants a
   crafted file that gozlerd serves as `live`. The file lives in a folder
   that only SYSTEM and Administrators can write and read, the same rule the
   service log follows since `8f0d03c`. A CRC detects damage, not a crafted
   file.
3. **Drift.** Without a rule, the drift that a restart heals today is saved
   and reloaded indefinitely, and stale hard-link names stay in results. The
   saved file carries the drift counts and its age; past a limit, the start
   builds from the MFT.
4. **When to write.** Windows gives a service only seconds at system
   shutdown, too little to write about 200 MB. gozlerd writes the file while
   it runs, at an interval, and replaces it atomically, which needs twice
   the space during the write.
5. **Reading the file.** The file is untrusted input under `AGENTS.md`:
   checked reads, every count read from it capped, and a proptest that every
   malformed file returns an error. Without the caps, one damaged file
   aborts gozlerd on every boot.
6. **Replay or rebuild.** When the journal span to replay is large enough
   that replay would take longer than a build, the start builds from the
   MFT instead.
7. **Format.** The file stores entries, not the store's memory layout, and
   carries a version. A version gozlerd does not know means a full build.
   Every change to what an entry holds raises the version.

**Done when:** fixture tests show that each mismatch (another volume, another
journal ID, a purged cursor, a bad CRC, an unknown version, drift past the
limit, a replay too long) ends in a full build and never in `live` with a
stale index; the proptest passes; the file's folder refuses a standard user;
and a restart of C: is measured against stage 1's time.

## Expert reviews

An independent review is worth its cost only where evidence cannot settle
the question, so the plan asks for one at these points and not before:

1. **Before stage 1 lands.** A review of the stage 1 code change: its paths
   (a file-layout pass that stops part way, a rescan, a cancelled build) and
   its control flow, because the change alters how every index is built.
2. **Before stage 2 starts.** An independent plan review, with a second
   reviewer who challenges the first, of how stage 2 meets its seven
   requirements, because it adds a security boundary and a way to serve a
   stale index as `live`.
3. **When a measurement contradicts this plan.** For example, if a stage 1
   start is not near 10 s cold, or the journal horizon is much longer than
   20.5 hours, the decisions above are reassessed with a review before work
   continues.

## When the plan is done

When stage 1 lands, its rules move into `docs/DESIGN.md`, "Index coherence",
and stage 0 and stage 1 leave this file. Stage 2's requirements stay here
until stage 2 is built or the owner drops it; then they move into
`docs/DESIGN.md` as the reason, and this file is deleted.
