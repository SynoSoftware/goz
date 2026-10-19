# Plan: the copy, move and delete engine

Builds [the engine spec](fast-copy-delete-spec.md) in four stages, A to D,
before any stage of [the file manager](file-manager-spec.md). The panels then
build on the engine's real interface and measured numbers.

## Schedule

**Not started.** The owner reviews this plan, the spec and the research
record first (2026-10-19).

## How every stage runs

These follow `AGENTS.md` ("Writing a plan"), which records why the window's
stages took about 2 hours each and the es superset's features about 25
minutes each.

- **One implementer** keeps its context through stages A to D and fixes the
  review's findings. The reviewer reads the diff against the spec and
  reports; the implementer fixes and builds once; the reviewer rechecks the
  fixes by reading.
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
- **Nothing waits for the owner,** except stage D's one elevated run. What
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
  class and its disks.
- `gozler-core`: the `FileFullDirectoryInfo` parser (D4).
- `gozler-fileops`: the crate, its `deny.toml` and CI entries; "What an
  operation receives"; "Delete"; the queue's skeleton from "The interface"
  (prepare, submit, the states, the threads, cancel and the wake function,
  without waits or the saved file), with the delete running in it, so stage
  B adds to the queue instead of restructuring a delete built without it.
- Run once: POSIX delete on the Dev Drive. Write the result into the spec.

Done when: the stage A rows of "Tests" pass, the run-once results are in the
spec, and `check.ps1` is green.

## Stage B: copy and move with the Windows engine

Build:

- `gozler-winfs`: rename; `CopyFile2` with a progress closure that can
  cancel; create a folder from a template; free space; seek penalty and bus
  type.
- `gozler-fileops`: "Copy" with "The Windows engine", "Move", "Conflicts",
  temporary names (D10), links (D21), and the rest of "The interface": the
  queue's waits (D14) and taken entries, saved operations with Continue and
  Dismiss (D19), rename, new folder and measure; the speed settings with
  their ranges and the presets that use only the Windows engine.
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
| one 8 GiB file | copy: Windows engine buffered, unbuffered, and unbuffered with a larger `ioDesiredSize` (D9); ring with 8 MiB × 8 and with 2 MB × 8 | `robocopy /j`, FastCopy |
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
  date;
- each of D2, D7, D8, D9 and D10 in the spec is confirmed or changed, with
  its numbers, and the Automatic preset holds the fastest measured values
  for each case;
- a case where the engine is slower than the Windows tool it is compared
  with has its cause found and fixed in stage A, B or C, or is written into
  the spec with the reason it stays.

## Tests

Each test watches a failure that the compiler and the other tests miss.

| Stage | Test | Failure it watches |
| --- | --- | --- |
| A | Delete a tree that holds a junction to a folder outside it, and a file symbolic link to a file outside it; the symbolic-link case is skipped when the test cannot create one | the delete enters the junction or deletes the link's target, outside the selection |
| A | Delete a selection whose only entry is a junction, given to the engine as a folder | the engine trusts the caller's mark, enters the junction and deletes its target's files |
| A | Delete a tree with a read-only file, a path longer than 260 characters and empty nested folders | the tree is left partly deleted |
| A | Delete a read-only file through the classic fallback of D1, called directly on NTFS | a FAT volume's read-only files cannot be deleted |
| A | Delete 100 files with the POSIX call made to refuse once, and count the POSIX calls (any OS, through the function that chooses the method) | every file on a FAT drive pays a failed call before the classic delete |
| A | Delete a tree in which one file is held open without `FILE_SHARE_DELETE` | one failure stops the delete, or the failure is lost and the delete reports success |
| A | Compare a folder with itself by other case, and by its short name; prepare a copy of a folder to a destination that is a junction into that folder; the short-name case is skipped when the volume makes no short names | two operations on one folder do not wait for each other, or a folder is copied or moved into itself |
| A | Nested selection, destination inside a source, the numbers for Keep both, the temporary name of a 250-character name (any OS) | an entry is handled twice, copied into itself, or a long name cannot be copied |
| B | Copy with each conflict choice onto existing files, and with Replace a file onto a folder of its name | Skip or Keep both replaces a file at the destination, or Replace deletes a folder |
| B | Create a file at the destination between prepare and run, when prepare found no conflict | the run replaces a file that nobody chose to replace |
| B | Create a hidden, read-only file at a final name after the walk, before the rename | the rename's refusal is taken for our own failure, and the person's file is replaced or deleted |
| B | Cancel a copy from its first progress report | a partial file stays at the final name and looks complete |
| B | Continue onto a destination that holds a `.gozler-partial` file, a complete copy, and a Keep both copy `report (2).pdf` | the complete copies are copied again, or the partial file is kept or taken as complete |
| B | Continue a move with the copy-and-delete method whose destination already holds a complete copy | the source is deleted on a size-and-time guess |
| B | Move with the rename method, and compare the file ID before and after; move into a folder of the same name that holds one conflicting file | a move on one volume copies instead of renaming, or a merge replaces a file without a choice |
| B | The queue's wait decision for: two operations on overlapping paths, two copies on one disk, a delete and a copy on one disk with separate paths, a delete inside a copy's destination folder but outside what it writes (any OS) | an operation changes files that another still uses, or a delete waits behind an unrelated copy |
| B | Build speed settings with each value just outside its range, and a block size that is not a power of two; reload a saved operation and compare its settings (any OS) | a bad value from `ui.json` reaches a copy, or Continue runs with other settings than the interrupted run |
| B | Write the saved file as a crash leaves it and create the queue; separately, drop a queue with a running operation and create it again | an interrupted operation is lost, or a cancelled one is offered as interrupted |
| C | Copy with the ring, in each profile and disk layout, then compare a hash of each source and destination: a sparse file with holes, a file whose size is not a multiple of the sector size, a file larger than the 256 MB buffer, a file of 0 bytes, a file with a `Zone.Identifier` stream, a read-only file, and 2,000 small files that cross the buffer's end | the ring writes a wrong byte, reuses a buffer still being written, loses a stream, a hole or an attribute, or pads a file |
| C | Copy with the ring a file that the test holds open for writing | a file another program writes is copied as a mix of bytes |
| C | Cancel a ring copy between two blocks | a partial file stays at the final name |

No test checks what `CopyFile2` keeps (streams, attributes, dates), because
that tests Windows.

## After install

The owner checks these, because they need a second disk or a share. No stage
waits for them.

- Stage D's copy cases between two disks, and on a hard disk with two
  partitions, where the ring's hard-disk profile should help most.
- A move across volumes, for example to a USB drive: the sources go, the
  progress shows bytes, and a cancel leaves each file in one place.
- A copy from a share whose server is switched off during the copy: the
  operation shows Cancelling until the share times out, and Exit ends Gozler
  within 3 seconds, leaving the copy Interrupted.
- A delete of a tree with read-only files on a FAT32 USB drive, which takes
  D1's classic fallback.
- One share folder reached through two drive mappings: a copy from one into
  the other is refused as a copy into itself (D12).

## When the plan is done

The engine's rules move into `docs/DESIGN.md` when the file manager's plan
closes, the research record stays in `docs/research/`, and this plan and the
spec are deleted then.
