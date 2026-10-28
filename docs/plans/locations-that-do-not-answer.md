# A location that does not answer

When a network share, a removable drive or a failing disk stops answering or
goes away, nothing waits for it. All its work moves to the end of the queue
with a list of what is not done, and resumes by itself when it answers
again. A file that cannot be read, such as one on a bad sector, fails once
and is listed; it is not tried again. The window, the other panel, the other
folders and the other transfers go on, and Exit still ends.

This is not only about shares. A local NTFS disk with a bad sector hangs the
same way: Windows and the disk retry the sector, often for a minute or more,
and every read on that disk can wait meanwhile. Commander mode will also
work on file systems other than NTFS, such as FAT and exFAT drives and
shares, so every rule here holds for any file system.

This plan takes what applies to Gozler from TinyTorrent2's torrent control
work: its `docs/plans/torrent-control-plan.md`, and "Drive turns", "Where a
folder really is" and "Closing and shutdown" in its
`docs/contracts/engine/`. Most of it is already in Gozler. Three places
still let one location that does not answer stop work that does not touch
it.

## Schedule

**Scheduled:** a todo for when Gozler adds shares. Shares are not planned
now (owner, 2026-10-28), so nothing here starts before then. Until then this
plan records what that work must design for. On a local volume that answers, the copy engine already
finds where a path really goes from the final path of its open (D12), so a
copy into `C:\temp`, a junction to `V:\temp`, already counts `V:`'s disk.
Stage 0 first, because stages 1 and 2 use it; it
touches only `gozler-core` and `gozler-winfs`. Stage 2 changes the queue and
follows stage 0. Stages 1 and 3 come after stage 2 of `maintainability.md`,
because that stage rewrites `win::work::Task` and the work owner's `close`,
which they change too. Not started.

Each stage is its own commit. One implementer keeps its context through the
stages and fixes the review's findings; the reviewer reads and reports.

## Words

**Location:** the drive or the server that a path's work really reaches. A
local drive is its volume; `\\nas\media` is the server `nas`; a mapped letter
`Z:` for `\\nas\media` is `nas`; a `subst` letter is the location of its
folder; `C:\Media`, a junction to `\\nas\media`, is `nas`. Stage 0 adds the
word to `CONTEXT.md`.

**Does not answer:** a location whose root Gozler has started to open, on a
thread of its own, and the open has not returned. It answers again the
moment that open returns.

**Went away:** a location whose file failed with an error that says the
location is gone, such as a network name that no longer exists or a device
that is gone.

**Cannot be read:** a file whose read or write failed with an error about
its data, not its location, such as a bad sector (`ERROR_CRC`,
`ERROR_SECTOR_NOT_FOUND`, `ERROR_READ_FAULT`, `ERROR_WRITE_FAULT`,
`ERROR_IO_DEVICE`, `ERROR_FILE_CORRUPT`). The implementer confirms the list
against what the copy engines return. Trying the file again would hang on
the same sector, so the failure is final.

## What Gozler already does

| TinyTorrent2 | Gozler today |
|---|---|
| One worker per drive or server | The queue's device admission (D14) counts each physical disk once and each share by its server. A copy from `C:` to `D:` never waits for a share. |
| Jobs on one disk take turns | The same: Transfers per device, default 1. |
| Find where a folder really is, without opening through a link (step 7) | Partly. A transfer's devices come from the final path of each end (`identity`, D12), so a junction to a share counts as the share, but only after an open through the link returns. While the share hangs, that open hangs on the operation's own prepare thread (stage 0). |
| Save the work before it changes a file | `operations.json`, Interrupted, Continue and Dismiss (D19). |
| A notice that names the share, and work that resumes when it returns | A panel says "Waiting for \\\\nas\\media…" after 3 seconds (`core::work::SLOW`). A transfer has neither yet (stage 2). |
| Exit never waits for file work | The queue is never dropped at exit, and its saved file is local. Exit's cleanup still waits without a limit (stage 3). |

Prepare, Delete and New folder each run on a thread of their own, so a hang
in one of them stops only that operation. The drive menu reads each drive's
label with a task for each drive letter.

## Stage 0: where a path really goes

This is TinyTorrent2's step 7. Both the copy engine (stage 2) and the
window (stage 1) need it: they must know a path's location before they open
it, because the open hangs when that location does not answer. Today the
engine learns the real destination only from the final path of an open
handle (D12), so a copy into a link to a hung share sits in "Preparing…"
and cannot move to the end of the queue.

A location is found without opening through a link:

1. A drive letter is looked up with `QueryDosDevice`, which asks Windows'
   object names and touches no disk or network. A mapped letter gives its
   server and share; a `subst` letter gives its folder, which is looked up
   again.
2. A `\\server\share` path is that server.
3. Each folder on the path that is a link (junction, symbolic link, mount
   point) is opened as the link itself, with `FILE_FLAG_OPEN_REPARSE_POINT`,
   and its target is read with `FSCTL_GET_REPARSE_POINT`. That open is on the
   drive that holds the link, not on its target. The walk stops at a network
   path.
4. A name the walk cannot read, or a form of `QueryDosDevice` it does not
   know, ends the walk. The location found so far is the answer, which is
   today's behavior for that path.

Step 3 opens folders, so a caller runs the walk off the window's thread. The
Win32 calls go in `gozler-winfs`. The parsing of a reparse buffer and of a
`QueryDosDevice` answer goes in `gozler-core`, because it reads bytes, with a
test that malformed bytes return an error. The walk only finds the location;
D12's final path stays the identity that compares paths.

**Proof.** Parsing tests for a junction, a symbolic link, a mapped letter, a
`subst` letter and malformed input. The rest is code review.

## Stage 1: a panel leaves a location that does not answer

**What the person sees now (read from code, not measured).** The left panel
opens `\\nas\media`, and the NAS hangs. The person presses `Alt+F1` and picks
`C:`. The panel keeps "Waiting for \\\\nas\\media…" and does not open `C:` until
Windows returns the call on the share.

**Why.** `core::work::Slot` keeps a cancelled call's slot until the call
returns (the test `cancelled_native_work_keeps_its_slot_and_only_the_latest_replacement`
pins it), and the slots are per purpose, not per location:

- `app::work::folders`: one reading per panel. A new reading also waits until
  the old folder's change watch closes, and closing a handle on a hung share
  can hang too.
- `app::work::folders`: one free-space read per panel.
- `app::work::lists`: the one check of Search's Location.
- `app::work::shell`: one call for each list and command, such as Open in
  Search's list, and Show in Commander. Open on a file on the hung share
  makes the next Open in that list wait.

The implementer first lists every `Task` whose call opens a path, because
this list comes from reading the constructors, and adds any that it missed.

**Change.** A slot keeps its calls per location, not one in total. New input
for another location starts at once. A call that the person has left keeps
running on its location until Windows returns it, and its result is
dropped. New input for the same location also starts at once beside one
left call, because a bad sector hangs one folder, not the whole disk: the
person who leaves `C:\Broken` for `C:\Users` must not wait for the sector's
retries. Only when a left call and the current call on one location both
have not returned does newer input there wait, because then the location
itself does not answer. So each purpose has at most two hung threads for
each location the person has used, which keeps the bound that the slot
exists for (window rule 5). Each call finds its location
with stage 0's walk, off the window's thread, on the location that holds
the path's first link.

**Proof.** Unit tests in `core::work`: a call on one location that has not
returned does not delay new input for another location or for its own; a
second call there that has not returned delays new input for that location
only. The rest is code review and the checks after install.

## Stage 2: a location that does not answer goes to the end of the queue

**What the person sees now (read from code, not measured).**

- **A share that hangs.** A copy from `\\nas\media` to `C:` runs, and the
  NAS hangs in the middle of a file. With the default of 1 transfer per
  device, every other copy or move that touches `C:`'s disk shows "Waiting
  for the copy to C:\…" until Windows returns. Pause and Run now do not
  help, because both wait until the stuck transfer is quiet, and it is
  quiet only when Windows returns.
- **A share that goes away.** A share that disconnects, or a USB drive that
  is removed, returns errors at once instead of hanging. No step ends a
  running transfer when its location goes away, so each remaining file
  fails on its own and the report lists them all: 10,000 failures for one
  unplugged cable.
- **A bad sector.** A copy reads a file on `C:` with a bad sector. While
  Windows and the disk retry it, the copy keeps `C:`'s device, so every
  other transfer on that disk waits. When the read fails, the file is
  listed as failed and the copy goes on, which is right.

**Why.** `waits::waits` counts each admitted transfer's devices until the
transfer ends or pauses, and a pause releases them only after the transfer's
Windows calls have returned (DESIGN.md "Device admission"). Nothing in the
queue knows that a location stopped answering.

**Change.** Nothing waits for a location that does not answer. All the work
on it moves to the end of the queue and is tried again later, and the rest
of the queue goes on.

- **A device is held only by a transfer that moves.** A transfer whose byte
  and item counts have not changed for 3 seconds, the window's existing
  slow limit, holds no device, so the next transfer on that device starts
  at once. When the still transfer moves again, it finishes its current file
  without admission and asks for admission before its next file, as Resume
  does. Two transfers then share a device for at most one file. That costs
  speed, not files, because the transfer limit protects speed. So a disk
  that spins up, or a share that is only slow, costs a moment of shared
  speed, and no time limit has to tell them apart from a hang.
- **Which location does not answer.** When a transfer goes still, the queue
  opens the root of its source and of its destination, each on a thread of
  its own. A location does not answer while that open has not returned. A
  local root returns in milliseconds, so the hung end is known almost at
  once. A file that fails with an error that says its location went away,
  such as a network name that no longer exists or a device that is gone,
  marks that location at once, without a check first. These opens block
  nothing.
- **The whole location moves to the end of the queue.** Every transfer that
  needs a location that does not answer or went away moves to the end of
  the queue, in its order, and holds no device: the still transfer, the
  transfers that wait, and new ones as they arrive. Each starts no more
  files. The file it stopped in does not count as failed, and its report
  lists what is not done: that file, and the files and folders it has not
  reached. The other transfers of the same operation go on.
- **A hung call's result still decides its file.** The call stays on its
  thread, and when Windows returns it, its result counts: a file it
  finished is done; a file that cannot be read is failed and listed; a
  location that went away leaves the file not done. So a bad sector costs
  one failed file, and nothing reads that sector again.
- **A file that cannot be read fails once.** Its transfer lists it with the
  reason, such as "The disk cannot read this file", and goes on with the
  next file. The resume after a location answers again does not try it
  again. A Continue after a restart walks the disk again and meets it once
  more, as any new run would. A copy never fills an unreadable block with zeros,
  because a file with a silent hole is worse than a listed failure. Its
  unfinished output stays as "What a stopped operation leaves on disk"
  says, and a move keeps its source.
- **What the person sees.** Each of those transfers names the location in
  its row, its selected progress and the status bar, such as "Copying to
  \\\\nas\\media is waiting: the share does not answer", as a panel already
  says "Waiting for \\\\nas\\media…". The words follow `docs/WRITING.md` and
  go in every language catalogue.
- **Tried again later.** A location that does not answer answers again the
  moment its root open returns, such as when a disk ends its retries of a
  bad sector. A location that went away is tried again every 30 seconds,
  on a thread of its own. Then the location's transfers are ready again,
  from their place at the end of the queue, and each
  resumes as Continue does: the files it finished count as done, and its
  unfinished file resumes at its last confirmed byte (D10). A transfer whose
  hung call has not returned yet waits for that call alone, because both
  would write the same file. The person can also choose Continue, which
  tries the location at once, move the transfer up the queue, or Cancel it.
  A transfer that the person paused stays paused.
- **Pause, Cancel and Run now** act at once on a still transfer: the queue
  records the choice and the row shows it. The call that hangs ends when
  Windows returns it.
- **New work.** Prepare finds the location of the sources and the
  destination with stage 0's walk before its first open. An operation that
  needs a location that does not answer or went away does not open it: it
  goes to the end of the queue with the same notice. An open that hangs on
  a location that nothing has tried yet stays on prepare's own thread, and
  its operation goes still as a transfer does. So a copy into `C:\Media`, a
  junction to a hung `\\nas\media`, names the NAS instead of showing
  "Preparing…" until Windows gives up.
- **Exit.** Work on a location that does not answer or went away stays in
  `operations.json`, so the next start offers it as Interrupted, as D19
  decides for every cut operation.

The stillness clock and the root opens live in the queue, beside device
admission. Which transfers hold devices, and which errors mean that a
location went away or that a file cannot be read, are pure functions beside
`waits::waits`, so they are tested on every OS.

**Proof.** Unit tests: a still transfer from `\\nas\media` to `C:` holds no
device, so a transfer from `C:` to `D:` starts; when it moves again, it asks
for admission before its next file; a location that does not answer moves
every transfer that needs it to the end of the queue, in its order, while
the operation's other transfers go on; a location-gone error does the same
without failing the file; when the root opens, those transfers are ready
again from the end of the queue; a hung call that returns a bad-sector
error fails its file once, and the resumed transfer does not try it again.
The rest is code review and the checks after install.

## Stage 3: Exit never waits for a location

**What the person sees now (read from code, not measured).** The person
exports results to a file on `\\nas\media`, the NAS hangs, and the person
chooses Exit. The window closes, but the process stays, because Exit's
cleanup waits for the export without a limit (`requests::Work::close` calls
`Task::finish`). The old process still holds the instance mutex, so Gozler
started again opens a window that offers no file operations. When only one
window is allowed, it first waits 10 seconds for the old process
(`FIRST_SETTLES`).

**Change.** Exit waits only for work that moves, by stage 2's rule: each
wait in Exit's cleanup ends after 3 seconds without progress, and then the
process ends. An export's progress is the rows it writes. A wait with no
progress to measure, such as the service fix and the elevated helper that
`app::work::close` joins today, gets 3 seconds; the helper is a process of
its own and goes on without Gozler. The
settings file and `operations.json` are written first, as now, so nothing
the next start needs is lost. An export cut this way leaves the part it
wrote, as an export cut by a crash does.

**Proof.** Code review of every wait in `app::work::close` and
`finish_shutdown`, and check M8 after install.

## Checks after install

These need a share that can be disconnected, made to hang and reconnected,
a USB drive, and for M7 a disk with a known bad sector, if one is at hand.
They go into `ui-after-install.md` when stage 1 starts.

| # | Do | Expect |
|---|---|---|
| M1 | Open `\\nas\media` in a panel and make the NAS hang. Go to `C:`, then to `C:\Media` when it is a junction to the share | `C:` opens at once. `C:\Media` waits with "Waiting for \\\\nas\\media…". |
| M2 | Queue a copy from the share to `C:`, then a copy from `C:` to `D:`, and make the NAS hang during the first | Within about 3 seconds the second copy runs. The first moves to the end of the queue, names `\\nas\media`, and its report lists what is not done. Within 30 seconds of the NAS answering, the first resumes by itself and finishes. |
| M3 | During M2, Pause and then Cancel the stuck copy | Each shows at once. |
| M4 | During M2, start another copy to the share, and one to `C:\Media` when it is a junction to the share | Each goes to the end of the queue at once, names `\\nas\media`, and resumes when the NAS answers. |
| M5 | Disconnect the share during a copy to it; reconnect after 3 minutes | The copy goes to the end of the queue with no failures listed, and resumes by itself from the file it stopped at. |
| M6 | Remove the USB drive during a copy to it; plug it back in | The same as M5. |
| M7 | On a disk with a bad sector, copy a folder that holds the unreadable file, start a second copy on that disk, and open another folder of that disk in a panel | The second copy and the panel go on within about 3 seconds. The unreadable file fails once, with its reason, and the first copy finishes the rest of the folder without reading that file again. |
| M8 | Choose Exit during M2, and again during an export to the hung share. Start Gozler again | Each process ends within about 5 seconds. The next start opens its window at once with file operations, and offers the cut copy as Interrupted. |

## Not in this plan

- TinyTorrent2's libtorrent fork work (`lt::storage_root_key`, its re-keying
  job): libtorrent assigns its own storage roots; Gozler's queue does not.
- Physical-disk turns: device admission already uses the volume's disk
  extents (`gozler_winfs::volume_storage`).
- Continuing interrupted work by itself at start: D19 lets the person
  choose, and stays.
- A probe of every location each minute while nothing runs: the window
  reads a location only when the person opens it, so there is nothing to
  watch.
