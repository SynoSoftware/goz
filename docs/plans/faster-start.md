# Plan: a faster start by saving the index

How to make an index complete sooner after gozlerd starts, beyond what one
MFT pass gives. Every start builds each index from the MFT, and until the
build ends the volume is `bootstrapping`: every answer exits 2, "may be
incomplete" (`docs/DESIGN.md`, "Index coherence"). Saving each index with
its journal cursor would let a start replay the journal instead. This plan
keeps the requirements for that, for the day it is needed.

## Schedule

Deferred. It starts only when a user with an HDD or with many millions of
entries reports that a start is still too slow, and then only after the
review that "Expert reviews" names.

## What we know

- **The build reads the MFT once** (`d7f9451`; `docs/DESIGN.md`, "Index
  coherence"). On the owner's C: (2 million entries) it took 15.4 s after a
  service restart, against 22.5 s for the two passes before it
  (`gozlerd.log`, 2026-10-18, from "indexing volume" to "index built;
  tailing the journal"). A cold single pass took 10.1 s against 22.1 s in
  the experiment that preceded it.
- **The "startup indexing finished" line does not measure the build.** The
  supervisor writes it on its first 30-second check (`SCAN_INTERVAL`) after
  the build, so it always reads about 30 s.
- **Starts.** Windows does not log service starts on the owner's machine,
  so they were counted from boots: the System log's Kernel-Boot event 27
  records 32 boots from 2026-08-31 to 2026-10-18, of which 26 were full
  starts. Each full start builds every index: about 3.8 a week, plus each
  gozler upgrade.
- **Journal horizon.** The oldest change in the C: journal (250 MiB) was
  from 2026-10-17 20:36, read on 2026-10-18 17:05: about 20.5 hours on a
  development day. A saved index would cover a night off, but not a
  weekend, after which a start builds from the MFT anyway.
- **Why stage 2 is deferred.** A saved index would save at most the build,
  about 10 s cold on the owner's machine, and only after a downtime shorter
  than the journal horizon. It costs a security boundary and a new way to
  serve a stale index as `live` (the requirements below).
- **Upstream proposal.** mustafaahci/goz#1 (ZJHSteven, 2026-08-28) proposes
  to save each volume's index with its journal cursor (the journal position
  the index is current to), check both at start, replay the journal from
  there, and fall back to a full build on any mismatch. The reporter
  measured 105 s to build 4.9 million entries with two passes. No pull
  request has arrived.
- **Drift that a restart heals.** `--status` on C: showed 126 hard-link
  changes not reconciled before a restart. These do not start a rescan; a
  restart clears them, because it builds the index again.
- **The rule already set.** `docs/DESIGN.md` allows saving state between
  runs only when the index and its cursor are saved and replaced together.
  A cursor alone must never skip a build.

## What we do not know

- **The start time after a boot with one pass.** Read it from `gozlerd.log`
  after the owner's next ordinary reboot. If it is far from 10 s, the
  decision above is reassessed.
- **Whether gozlerd survives a Fast Startup boot.** Windows Fast Startup
  normally hibernates services with the kernel at shutdown. On the owner's
  machine most boots are full starts anyway.
- **How long a journal replay takes** on a saved index.

## Requirements

Each requirement names the failure a user would see without it.

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
and a restart of C: is measured against the one-pass build.

## Expert reviews

An independent review is worth its cost only where evidence cannot settle
the question, so the plan asks for one at these points:

1. **Before this work starts.** An independent plan review, with a second
   reviewer who challenges the first, of how the work meets the seven
   requirements, because it adds a security boundary and a way to serve a
   stale index as `live`.
2. **When a measurement contradicts this plan.** For example, if a start
   after a boot is far from 10 s, or the journal horizon is much longer than
   20.5 hours, the deferral is reassessed with a review.

## When the plan is done

When the work is built, or the owner drops it, its rules move into
`docs/DESIGN.md` as the reason, and this file is deleted.
