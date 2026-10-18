# Plan: a faster start

How to make an index complete sooner after gozlerd starts: first by building
it faster, and only if that is not enough, by saving it between runs. It
records what is known today, what is not known, and the order of the work.

## The problem

Every start of gozlerd builds each index from the MFT, because nothing is
saved between runs (`docs/DESIGN.md`, "Index coherence"). Until the build
ends, the volume is `bootstrapping` and every answer exits 2, "may be
incomplete". AI agents that search right after a restart get no answer they
can trust for that time.

## Schedule

**Scheduled:** not yet. The owner sets the date. Stage 0 comes first, and its
results decide whether stages 1 and 2 happen at all.

## What we know now

- **Start time.** On the owner's machine, gozlerd built C: (2,004,500
  entries) in 24.4 s and finished startup in 30.0 s (`gozlerd.log`,
  2026-10-18, "startup indexing finished ... elapsed_s=30.0161787").
- **Index size.** `gozler --status` reports 194.6 MB of index memory for C:
  with 2,015,690 entries.
- **Two passes over the MFT.** A build reads the MFT twice: once with
  `FSCTL_ENUM_USN_DATA` for names and parents, then with
  `FSCTL_QUERY_FILE_LAYOUT` for sizes, dates and attributes (`bootstrap.rs`,
  `bootstrap_volume`). Mustafa Ahci measured 16.0 s and 12.7 s for the two
  passes on his C: with 4.08 million entries (mustafaahci/goz#1).
- **Upstream proposal.** mustafaahci/goz#1 (ZJHSteven, 2026-08-28) proposes
  to save each volume's index with its journal cursor (the journal position
  the index is current to), check both at start, replay the journal from
  there, and fall back to a full build on any mismatch. The reporter measured
  105 s to build 4.9 million entries, and a restart that was still building
  after 170 s. Mustafa agreed with the direction. No pull request has
  arrived.
- **Journal size.** The change journal must cover the time gozlerd is down,
  or a saved cursor is useless. Mustafa's fixes that size the journal by
  volume (`f99fb35`, `1ba28b5`, `3b37318`) are in gozler. C: has a 250 MiB
  journal, which held about 20.5 hours of changes (stage 0 results).
- **Drift that a restart heals.** `--status` on C: shows 126 hard-link
  changes not reconciled and 44,770 placeholders. These do not start a
  rescan; a restart clears them, because it builds the index again.
- **The rule already set.** `docs/DESIGN.md` allows saving state between
  runs only when the index and its cursor are saved and replaced together.
  A cursor alone must never skip a build.

## What we do not know

- **Whether gozlerd survives a Fast Startup boot.** Windows Fast Startup
  normally hibernates services with the kernel at shutdown. On the owner's
  machine most boots are full starts anyway (stage 0 results), so this
  changes little there.
- **Whether one MFT pass can do the work of two.** `FSCTL_QUERY_FILE_LAYOUT`
  can return name entries with their parent folder. Whether those are
  complete enough to replace the enumeration pass is not checked.
- **How long a replay takes.** Replaying days of journal on a saved index
  runs the live update path for each change, including a Win32 walk of every
  hard-linked file's names. Not measured.

## Stage 0: measure

- Count gozlerd starts per week on the owner's machine.
- Try a build that reads the MFT once with `FSCTL_QUERY_FILE_LAYOUT` and
  compare its index with today's two-pass index over the same volume.
- Measure how many hours of changes the 250 MiB journal on C: holds.

**Done when:** the plan records the starts per week, the single-pass time
and whether its index equals the two-pass index, and the journal horizon in
hours. With these numbers the owner decides whether stage 1, stage 2 or
neither happens.

### Results so far (2026-10-18)

- **Starts.** gozlerd's log began on 2026-10-18, and Windows does not log
  service starts here (no Service Control Manager event 7036), so starts are
  counted from boots: the System log's Kernel-Boot event 27 records 32 boots
  since 2026-08-31, of which 26 were full starts, 4 Fast Startup boots and 2
  resumes from hibernation. Each full start rebuilds every index: about 3.8
  a week, plus each gozler upgrade. Fast Startup is on, but it rarely
  applies here. Whether gozlerd survives a Fast Startup boot is not yet
  seen in its log.
- **The two passes.** `gozlerd scan` on C: (2,016,090 entries), run while the
  service was also running: enumeration 12.47 s, file layout 10.46 s, 22.97 s
  in all. If the file layout alone builds the same index, one pass saves up
  to 12.5 s of the 23 s.
- **Why two passes today.** The file-layout pass is best-effort: a failed
  read stops it and leaves the remaining sizes and dates unknown, while the
  enumeration pass has already given every name (`enrich_from_layout`). A
  single pass keeps that guarantee by reading the file layout first and
  running the enumeration only when the file layout stopped part way.
- **Journal horizon.** The oldest change in the C: journal was from
  2026-10-17 20:36, read on 2026-10-18 17:05: about 20.5 hours on a
  development day. A saved index would therefore cover a night off, but not
  a weekend, after which a start builds from the MFT anyway.
- **The single-pass experiment.** An uncommitted `gozlerd scan
  --compare-single-pass` built C: with two passes, one pass and two passes
  again, and compared the single-pass index with the second two-pass index
  by full path, folder flag, size, date and attributes.
  - Time, with a warm disk cache: 16.09 s, single pass 11.58 s, 15.50 s. The
    enumeration took 5.5 s warm against 12.15 s cold, so at boot the single
    pass saves about half of the build: a cold single pass took 10.11 s
    against 22.14 s.
  - Entries: two passes 2,019,054 (60 without a path), single pass
    2,019,117 (none without a path). 38 entries were only in the two-pass
    index and 101 only in the single-pass index; nearly all were files that
    changed between the builds (browser and app caches, temp files).
  - The one real difference: the enumeration pass never returns the
    `$Extend` folder, so the two-pass build leaves the NTFS metadata files
    under it without a path, while the single pass gives them their path.
    They become findable, as `$Boot` and `$AttrDef` already are.
  - The file-layout pass also returns the root folder's own record, which
    names the root as its parent. Taken as a name, it made the root its own
    parent and left no entry a path; the single pass must only enrich the
    root.

**Stage 0 is done.** One pass builds the same index in about half the time
at boot. Stage 1 is worth doing; saving the index (stage 2) is not, for now:
after stage 1 a start takes about 10 s, and a saved index would win at most
that, only after a downtime shorter than the 20.5-hour journal.

## Stage 1: build with one MFT pass

Only if stage 0 shows that one pass builds the same index.

- Replace the enumeration pass with the file-layout pass, so a start reads
  the MFT once.

**Done when:** the corpus and the index tests pass; a fixture test shows that
the single pass builds the same entries, names, parents, sizes, dates and
attributes as the two passes; and the start time on C: is measured against
today's 30 s. The criterion benches show no slowdown the change does not
explain.

## Stage 2: save the index between runs

Only if a start is still too slow after stage 1, for example on HDDs or on
volumes with many millions of entries. Each requirement below names the
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

1. **After the single-pass experiment, before stage 1 lands.** A review of
   the stage 1 code change: its paths (a file-layout pass that stops part
   way, a rescan, a cancelled build) and its control flow, because the
   change alters how every index is built. The experiment itself needs no
   review: it compares two indexes, and the comparison is the evidence.
2. **Before stage 2 starts.** An independent plan review, with a second
   reviewer who challenges the first, of how stage 2 meets its seven
   requirements, because it adds a security boundary and a way to serve a
   stale index as `live`.
3. **When a measurement contradicts this plan.** For example, if the
   journal horizon is much longer than 20.5 hours or starts are much rarer
   than measured, the decisions above are reassessed with a review before
   work continues.

## When the plan is done

The rules of the stages that were built move into `docs/DESIGN.md`, "Index
coherence", and this file is deleted. If stage 0 decides that nothing is
built, its numbers move into `docs/DESIGN.md` as the reason, and this file is
deleted.
