# Plan: the copy, move and delete engine

Builds [the engine spec](fast-copy-delete-spec.md) in stages A to F, before
any stage of [the file manager](file-manager-spec.md). The panels then build
on the engine's real interface and measured numbers.

## Schedule

Stages A to F are committed (3155fbc, 913eab6, 06ef84b, 155ab06, 262dc90,
and the commit that adds this line). The File operations window builds on
stage F in gozler-ui. "After install" waits for the owner.

## How every stage runs

These follow `AGENTS.md` ("Writing a plan"), which records why the window's
stages took about 2 hours each and the es superset's features about 25
minutes each.

- **One implementer** keeps its context through stages A to F and fixes the
  review's findings. The reviewer reads the diff against the spec and
  reports; the implementer fixes and builds once; the reviewer rechecks the
  fixes by reading.
- **An adversarial review only where a defect loses files:** stages A, B
  and C, where a second reviewer tries to make the stage delete, replace or
  damage a file it should not. Stage D measures and changes no rule, so it
  gets the one review.
- **Proof is a test** on a folder that the test creates under the system's
  temporary folder, which runs in seconds. No test reads or writes any other
  folder, and none needs a running gozlerd.
- **A Win32 behavior in doubt runs once,** and the result is written into
  the spec's "Not verified".
- **The engine's session owns its files:** `crates/gozler-fileops`, the new
  modules of `gozler-winfs`, and `gozler-core`'s `layout::listing` and the
  stream-header parser. A stage brief names them, and names the files other
  sessions own, because while the window's fixes ran a translation session
  edited the same crate.
- **A defect is reproduced before it is reported.** Paths are tested from
  PowerShell, because Git Bash rewrites `\\?\` paths, and two of the window's
  alarms were false for that kind of reason.
- **Nothing waits for the owner,** except in stage D: its one elevated run,
  and the owner's approval of the comparison with other tools. What
  needs a second disk, a hard disk, a USB drive or a share goes into "After
  install". A rule that depends on the disks, such as rename against copy
  and delete, is chosen by a function that the tests call with each value,
  so the test machine's one volume runs every path.
- **One `check.ps1` run** when the stage is reviewed, as `AGENTS.md` ("Before
  you finish") says.

## Stage A: delete

Build:

- `gozler-winfs`: open an entry relative to a folder handle; list a folder;
  delete one entry with its fallback (D1); the final path (D12); a volume's
  class and its disks. `NtOpenFile` needs the `windows-sys` features
  `Wdk_Foundation` and `Wdk_Storage_FileSystem`, and `RtlGetVersion`, in
  stage B, needs `Wdk_System_SystemServices`.
- `gozler-core`: the `FileFullDirectoryInfo` parser (D4).
- `gozler-fileops`: the crate, its `deny.toml` and CI entries; "What an
  operation receives"; "Delete"; the queue's skeleton from "The interface"
  (prepare, submit, the states, the threads, cancel and the wake function,
  without waits or the saved file), with the delete running in it, so stage
  B adds to the queue instead of restructuring a delete built without it;
  the speed settings as one typed value with their ranges, because prepare
  takes them and the delete reads its worker count from them.
- Run once: POSIX delete on the Dev Drive. Write the result into the spec.

Done when: the stage A rows of "Tests" pass, the run-once results are in the
spec, and `check.ps1` is green.

## Stage B: copy and move with the Windows engine

Build:

- `gozler-winfs`: rename; `CopyFile2` with a progress closure that can
  cancel; create a folder from a template; free space; seek penalty and bus
  type.
- `gozler-fileops`: "Copy" with "The Windows engine", "Move", "Conflicts",
  the final names (D10), links (D21), and the rest of "The interface": the
  queue's waits (D14) and taken entries, saved operations with Continue and
  Dismiss (D19), rename, new folder and measure; the presets that use only
  the Windows engine.
- Run once: `CopyFile2` with `COPY_FILE_OPEN_AND_COPY_REPARSE_POINT` on a
  folder junction, a folder symbolic link and a file symbolic link, without
  Developer Mode; `CopyFile2` on a file that another handle holds open for
  writing. Write the results and what the engine does about them into the
  spec.

Done when: the stage B rows of "Tests" pass, the run-once result is in the
spec, and `check.ps1` is green.

## Stage C: the ring engine and the clone

Build "The ring engine" of the spec: both profiles, the hand-offs to the
Windows engine, and the ring's Win32 calls in `gozler-winfs`, with the
`BackupRead` stream parser in `gozler-core`. Build the ring's clone (D20)
and the presets that use the ring. The profile and the disk layout (one disk or two) are chosen by a
function that the tests call with each value, so every path of the ring runs
on the test machine whatever its disks.

Run once: a clone on the Dev Drive of a file whose size is not a whole
number of clusters, of a sparse file, and of a 6 GB file in two ranges.
Write the result into the spec.

Done when: the stage C rows of "Tests" pass, the run-once result is in the
spec, and `check.ps1` is green.

## Stage D: measure

A program in `crates/gozler-fileops/examples/` creates the fixtures in a
folder it is given, runs each case once, and prints a table. It runs on the
owner's machine, elevated. Before each case it flushes the fixture volume
(`FlushFileBuffers` on the volume handle) and then empties the standby
list, so every case reads from the disk and not from memory. Each case,
the compared tools' included, ends with a flush of the destination volume,
so every time counts the data reaching the disk. Without both, a buffered
copy reads a fixture the program just wrote from the cache and returns
while its writes are still in memory, and wins against an unbuffered one by
construction. The fixtures are on an NTFS volume, because on the ReFS Dev
Drive Windows 11 clones a copy within the volume.

**Each case runs once.** A case runs again only when its time is within
10 % of the case it competes with, because only then can the owner's other
work decide the winner; both then run twice more and the middle time
counts. No preset runs on its own: a preset is a combination of values,
and each value is measured once, in the case where it matters.

| Fixture | Gozler's cases | Compared with |
| --- | --- | --- |
| 30,000 files of 1 to 64 KB in 5,000 folders | delete with one worker per processor, and with 8 | `rmdir /s /q`, rmbrr |
| the same | copy to another folder on the same volume, Windows engine, with and without the `.gozler-partial` rename (D10) | `robocopy /e /mt:16`, FastCopy |
| one 8 GiB file | copy: Windows engine buffered, unbuffered, and unbuffered with `ioDesiredSize` of 8 MiB and of 32 MiB (D9); ring with 8 MiB × 8 and with 2 MB × 8 | `robocopy /j`, FastCopy |
| 20 files each of 32 MiB, 64 MiB and 128 MiB | copy: Windows engine buffered and unbuffered | itself: the large-file limit, AngelCOPY's 32 MiB against BetterCopy's 256 MiB |
| 20 files of 64 MiB on the Dev Drive | copy within the volume: the ring's clone (D20) and `CopyFile2`'s own | itself |

On the owner's SSD the ring hands files under 32 MiB to the Windows
engine's workers, so the small-file row measures the Windows engine only.
The Dev Drive row's 64 MiB files get `COPY_FILE_NO_BUFFERING`, so a
`CopyFile2` that takes seconds there shows that the flag stops its clone.
The fastest value of each case goes into the Automatic preset.

**The other tools run only with the owner's approval.** FastCopy, in its
default settings, and rmbrr are the fastest copier and deleter that the
research found, and robocopy runs at 16 workers, a setting AngelCOPY's own
data shows is faster than the 64 it compared with. They are programs from
outside this repository: the owner approves the comparison run before it
starts, and installs FastCopy and rmbrr or approves each download.

The fixtures need about 25 GB free at once, and the run writes about
70 GB. The program prints both numbers and the free space before it
starts, and stops when the space is short.

Done when:

- the table is in `docs/BENCHMARKS.md` with the machine, the disk and the
  date; it holds Gozler's columns alone if the owner declines the
  comparison, and the compared tools' columns are added when the owner
  approves it;
- each of D2, D7, D8, D9 and D10 in the spec is confirmed or changed, with
  its numbers, and the Automatic preset holds the fastest measured values
  for each case;
- a case where the engine is slower than the Windows tool it is compared
  with has its cause found and fixed in stage A, B or C, or is written into
  the spec with the reason it stays.

## Stage E: final names and resume

Build D10 as the spec now has it (owner ruling, 2026-10-19): both engines
write straight to the final name, Replace on the ring writes over the file
with the compare, and Continue resumes an unfinished file on the ring from
the first block that differs. Which file is unfinished follows "Saved
operations". Remove the temporary name and everything only it used. A
normal copy gains no work.

Run once: end a process during a large `CopyFile2` copy and read what the
destination keeps; answer `CopyFile2`'s progress callback with STOP, CANCEL
and PAUSE. Measure with stage D's program, its fixtures divided by 5 (owner):
the small-file copy and the large file in both engines, against stage D per
file and per MB, and the compare's speed against a copy.

Done when: the stage E rows of "Tests" pass, each failing on the code
before its fix; the run-once results and the numbers are in the spec and
`docs/BENCHMARKS.md`; `check.ps1` is green outside crates other sessions
are editing.

## Stage F: progress and order for the File operations window

The window in gozler-ui shows the running operation's current file and
two bars, Pause and Resume, and the waiting operations in the order they
will run, which the person can change. The engine gives it, as agreed with
the window's session on 2026-10-20:

- `Queue::operations()`: every operation with its request and state, the
  running ones first, then the waiting ones in run order, then the rest; a
  running operation carries its counts and totals, whether it only renames,
  whether it is paused, and its current file with that file's bytes done
  and size (the file most recently started; for a delete, the entry being
  deleted).
- `Queue::set_paused`: the ring stops between blocks, delete workers
  between items, `CopyFile2` inside its progress callback. Cancel and
  `close()` work while paused, and a paused operation still makes others
  wait.
- `Queue::move_waiting` and `Queue::check_move`, one check: a waiting
  operation moves before another or last, refused with the operation it
  overlaps (D14) and the side it must stay on. Only the disk rule's order
  really changes; after a move every wait is decided again.
- The window's Exit calls `Queue::close()` and ends the process without
  dropping the queue, so `CopyFile2` keeps its file (What a stopped
  operation leaves).

Done when: the stage F rows of "Tests" pass and `check.ps1` is green.

## Tests

Each test watches a failure that the compiler and the other tests miss.

| Stage | Test | Failure it watches |
| --- | --- | --- |
| A | Delete a tree that holds a junction to a folder outside it, and a file symbolic link to a file outside it; the symbolic-link case is skipped when the test cannot create one | the delete enters the junction or deletes the link's target, outside the selection |
| A | Delete a selection whose only entry is a junction to a folder, given to the engine as a path | the engine takes the junction for a folder, enters it and deletes its target's files |
| A | Delete a tree with a read-only file, a path longer than 260 characters, empty nested folders, and a folder of 1,000 files beside one empty subfolder | the tree is left partly deleted, or a folder is tried before its last file item is done and fails as not empty |
| A | Delete a read-only file through the classic fallback of D1, called directly on NTFS | a FAT volume's read-only files cannot be deleted |
| A | Delete 100 files with the POSIX call made to refuse once, and count the POSIX calls (any OS, through the function that chooses the method) | every file on a FAT drive pays a failed call before the classic delete |
| A | Delete a tree in which one file is held open without `FILE_SHARE_DELETE` | one failure stops the delete, or the failure is lost and the delete reports success |
| A | Choose the retry for each refusal: sharing violation, lock violation, folder not empty, access denied (any OS) | a permission refusal waits through 200 ms of retries for every file |
| A | Compare a folder with itself by other case, and by its short name; prepare a copy of a folder to a destination that is a junction into that folder; the short-name case is skipped when the volume makes no short names | two spellings of one folder compare as two folders, so stage B's waits would not hold them apart, or a folder is copied or moved into itself |
| A | Build speed settings with each value just outside its range, and a block size that is not a power of two (any OS) | a bad value from `ui.json` reaches an operation |
| A | Nested selection, destination inside a source, the numbers for Keep both of a 250-character name (any OS) | an entry is handled twice, copied into itself, or a long name cannot be numbered |
| B | Copy with each conflict choice onto existing files, and with Replace a file onto a folder of its name | Skip or Keep both replaces a file at the destination, or Replace deletes a folder |
| B | Create a file at the destination between prepare and run, when prepare found no conflict | the run replaces a file that nobody chose to replace |
| B | Create a hidden, read-only file at a final name after the walk, before its create, in both engines and with each choice | the create's refusal is taken for our own failure, and the person's file is replaced or deleted against the choice |
| B | Cancel a copy from its first progress report | a partial file stays at the final name and looks complete |
| B | Continue onto a destination that holds an unfinished file, a complete copy, and a Keep both copy `report (2).pdf` | the complete copies are copied again, or the unfinished file is taken as complete |
| B | Continue a move with the copy-and-delete method whose destination already holds a complete copy | the source is deleted on a size-and-time guess |
| B | Continue a move with the rename method after half its entries were renamed; Continue a delete after half its entries were deleted; a first-run move whose source is deleted while it waits, onto a destination that holds a file of that name | entries that already moved or were deleted are reported as failures, or a source deleted by another program is reported as moved |
| B | Move with the rename method, and compare the file ID before and after; move into a folder of the same name that holds one conflicting file | a move on one volume copies instead of renaming, or a merge replaces a file without a choice |
| B | The queue's wait decision for: two operations on overlapping paths, two copies on one hard disk, two copies on one SSD, a delete and a copy on one disk with separate paths, a delete inside a copy's destination folder but outside what it writes, an operation after one that ended (any OS) | an operation changes files that another still uses, or waits behind an unrelated or ended one |
| B | The taken test for a row equal to a taken path, below it, and beside it with a longer name (`C:\a` taken, `C:\ab` not), in other case (any OS) | a deleted file's row stays visible, or a file that is not taken is hidden |
| B | Reload a saved operation and compare its speed settings (any OS) | Continue runs with other settings than the interrupted run |
| B | Write the saved file as a crash leaves it and create the queue; separately, drop a queue with a running operation and create it again | an interrupted operation is lost, or a cancelled one is offered as interrupted |
| C | Copy with the ring, in each profile and disk layout, then compare a hash of each source and destination: a sparse file with holes, a file whose size is not a multiple of the sector size, a file larger than the 256 MB buffer, a file of 0 bytes, a file with a `Zone.Identifier` stream, a read-only file, and 2,000 small files that cross the buffer's end | the ring writes a wrong byte, reuses a buffer still being written, loses a stream, a hole or an attribute, or pads a file |
| C | Copy with the ring a file that the test holds open for writing | a file another program writes is copied as a mix of bytes |
| C | Cancel a ring copy between two blocks | a partial file stays at the final name |
| E | Resume against a destination that holds: the source; a prefix then other bytes; a prefix then zeros; a zero block inside the prefix; and a source that changed inside the prefix, grew, or shrank mid-block | a byte that differs from the source is kept, or a block that equals it is written again |
| E | Continue onto unfinished files with each choice, also Skip and no choice; a file not done that the person created before the operation started and edited after | an unfinished file is skipped or numbered, or the person's file is written over |
| E | Continue two selected files with one name, one copy complete and one unfinished; two selected folders with one name that merge, with Replace | one source's complete copy is written over by the other source |
| E | Continue and Replace onto a file symbolic link whose target is outside the destination, in both engines | the link's target is written |
| E | Replace on the ring over a plain file and over a file with an extra stream; an interrupted Replace whose file looks older than the start | Replace leaves an old stream, or an interrupted Replace copies from 0 |
| E | A continued move whose resumed copy fails, and one that completes | the source is deleted before its copy is complete |
| E | Save an operation with its start time and load one saved without it (any OS) | Continue forgets the start, or an older saved operation no longer loads |
| F | Move a waiting operation past one it overlaps, and past one that waits only for the same hard disk (any OS) | a refused move changes the order or names the wrong operation, or an allowed move does not change which runs next |
| F | Pause a ring copy, a `CopyFile2` copy and a delete, then resume; cancel one while paused | a paused operation still writes or deletes, or a cancel while paused leaves more than a cancel leaves |

No test checks what `CopyFile2` keeps (streams, attributes, dates), because
that tests Windows.

## After install

The owner checks these, because they need a second disk or a share. No stage
waits for them.

- Stage D's copy cases between two disks, on a hard disk with two
  partitions, where the ring's hard-disk profile should help most, and to a
  USB drive, and a small-file copy to a share with 1, 2 and 16 workers,
  which also times D10's rename there. Their winners replace the Automatic
  preset's hard-disk, USB and share values.
- A move across volumes, for example to a USB drive: the sources go, the
  progress shows bytes, and a cancel leaves each file in one place.
- A copy from a share whose server is switched off during the copy: the
  operation shows Cancelling until the share times out, and Exit ends Gozler
  within 3 seconds, leaving the copy Interrupted.
- A delete of a tree with read-only files on a FAT32 USB drive, which takes
  D1's classic fallback.
- A copy of a folder symbolic link and of a file symbolic link by an
  elevated Gozler, which holds the right to create them, and by Gozler
  without elevation with Developer Mode off: each arrives as the same link,
  or is listed with `ERROR_PRIVILEGE_NOT_HELD` (the spec's "Not verified").
- One share folder reached through two drive mappings: a copy from one into
  the other is refused as a copy into itself (D12).
- A move to another drive of a OneDrive folder whose files are only in the
  cloud: every file arrives with its data, and the source folder goes only
  after its files ("What an operation receives", the second handle).

## When the plan is done

The engine's rules move into `docs/DESIGN.md` when the file manager's plan
closes, the research record stays in `docs/research/`, and this plan and the
spec are deleted then.
