# Spec: the copy, move and delete engine

`gozler-fileops` copies, moves and deletes files for the window's two modes,
Two panels and Search. This spec says what the engine does and why each
method was chosen. [The file manager spec](file-manager-spec.md) owns what
the person sees; [the plan](fast-copy-delete-plan.md) owns the order of work.

Every claim about another tool or a Windows API, with its source and its
numbers, is in [the research record](../research/fast-copy-delete.md). This
spec names the record's section beside each decision and does not repeat its
evidence.

## Owner decisions

| Date | Decision |
| --- | --- |
| 2026-10-19 | Copy, move and delete as fast as the disks allow. |
| 2026-10-19 | A Norton Commander style mode with two panels; operations run in the background, with a queue. |
| 2026-10-19 | The UI never waits for disk work: a command replies when the person's choice is recorded, and the progress shows on the operation. |
| 2026-10-19 | Where the research does not show which method is faster, Gozler implements both and the person chooses. |
| 2026-10-19 | The person chooses the speed settings: Automatic, which stage D's measurements fill, or a preset taken from the copiers that were read, shown without the copier's name, and can change each value. |
| 2026-10-19 | **Owner ruling: no Recycle Bin.** Every delete is permanent, with the fast method of D1. Gozler is the tool a person picks for speed; a person who wants the Recycle Bin uses File Explorer. |

## Decisions

Each row is settled unless it says otherwise. "Research" names the section of
the research record that holds the evidence.

| # | Question | Chosen | Not chosen | Why | Research |
| --- | --- | --- | --- | --- | --- |
| D1 | How to delete a file | Open the entry relative to its parent folder's handle, with `DELETE` and the open-reparse-point flag, and also `FILE_WRITE_ATTRIBUTES` only when the entry is read-only, because an open fails when it asks a right the file's permissions deny, while `DELETE` alone can come from the parent folder's `FILE_DELETE_CHILD`; set `FileDispositionInfoEx` with POSIX semantics and "ignore read-only". Where the file system refuses the flags (`ERROR_INVALID_PARAMETER`, `ERROR_NOT_SUPPORTED`, `ERROR_INVALID_FUNCTION`), clear the read-only attribute through `FileBasicInfo` on the same handle and set classic `FileDispositionInfo`. The first refusal on a volume is remembered for the rest of the operation, so every later entry on that volume goes straight to the classic delete | `DeleteFileW`; detecting support from the file system's name; `DeleteFileW` after `SetFileAttributesW`; `FILE_FLAG_DELETE_ON_CLOSE`; opening each entry by its full path, as rmbrr does | One call per file, the name leaves at once so the folder can go at once, and a read-only file needs no second call on NTFS, which made FastDelete 2× slower on read-only files. The open relative to the parent and the reparse-point flag are Rust's fix for CVE-2022-21658: a folder swapped for a link during the walk cannot redirect the delete, and a file symbolic link is deleted, not its target. The classic delete refuses a read-only file, which TinyTorrent2's `Payload.cpp` learned on FAT. Support depends on the Windows version and the driver, not only on the file system's name (ReFS refused it in 2020), so the refusal itself is the test; remembering it saves a failed call per file, 10,000 calls for a FAT drive with 10,000 files | Deleters (rmbrr, Rust's standard library, fs_at); Delete APIs |
| D2 | How many delete workers | A speed setting: one per logical processor in most presets; a work item is one folder, at most 256 files of it. Stage D measures this against 8 | One thread (`rd /s`); a fixed 8 (AngelCOPY) | rmbrr and FastDelete use one per processor; AngelCOPY found 16 no better than 8 on 10,000 files, which leaves the count open. The 256-file split lets one large folder use several workers, which FastDelete cannot | Deleters |
| D3 | When a folder is deleted | As soon as its last file and subfolder are gone, by the worker that removed the last one | One pass at the end, as AngelCOPY deletes folders deepest first after its files | A combination of two sources. The counter that deletes a folder when its last child goes is rmbrr's (`broker.rs`), but rmbrr first walks the whole tree on one thread and holds every path. The streaming walk in items of up to 256 files of one folder is AngelCOPY's (`Delete.cpp`). Together, folders go while the walk runs elsewhere, and no list of the whole tree is held; stage A's "partly deleted" test proves the combination | Deleters |
| D4 | How to list a folder | `GetFileInformationByHandleEx` with `FileFullDirectoryInfo` on a folder handle, parsed in `gozler_core::layout` | `FileIdExtdDirectoryInfo`, which gozlerd's ReFS walk parses; `FindFirstFileExW` (rmbrr, FastCopy); `NtQueryDirectoryFile` | The engine needs the name, attributes, size, date and reparse tag, not file IDs. `FileFullDirectoryInfo` gives all of them, the tag in `EaSize` for a reparse point (MS-FSCC), and every Windows file system answers it, FAT included. `FileIdExtdDirectoryInfo` is not handled by the FAT driver sample, so it would need this class as a fallback anyway | Listing APIs |
| D5 | Use gozlerd's index as the list of files to copy or delete | No. The disk is listed when the work starts | The index | The index lags the disk, hides names a person may not list, and holds no removable, network or FAT drive. A wrong list deletes or skips files. The index gives the panels their folder sizes only | — |
| D6 | The Recycle Bin | Not used (owner ruling) | `IFileOperation` with `FOFX_RECYCLEONDELETE`, as TinyTorrent2's `RecycleBin.cs` does | Recycling runs at File Explorer's speed and only through File Explorer's engine, which would also need its own thread, its own wait rules and its own questions | Delete APIs |
| D7 | Which copy engine | **Both, and the person chooses** (owner decisions 4 and 5): the Windows copy engine (`CopyFile2`) and Gozler's ring, combined for each case by the speed settings. The default preset, Automatic, starts as Parallel copy's values, and stage D and the After install checks replace each value they measure; the other presets are the configurations of the copiers that were read ("Speed settings") | One engine; a choice between whole engines only | The research does not settle it: no source compares a ring with `CopyFile2` and `COPY_FILE_NO_BUFFERING`, and no source measured a hard disk. The ring wins in theory on one hard disk with two partitions and measured +32 % on one large NVMe file; it also writes every byte itself, so a defect damages files with nothing to detect it | Copiers; What the research leaves open |
| D8 | The Windows engine's rules | Rename on one volume; `CopyFile2` across volumes; `COPY_FILE_NO_BUFFERING` from 32 MiB between local volumes; files under 32 MiB on 16 workers between volumes without a seek penalty, 2 on a USB volume or a share; one file at a time otherwise | Explorer's defaults; robocopy `/MT` | Copied from AngelCOPY's `NativeCopy.cpp`, through TinyTorrent2's `Payload::Move`: the rename, the 16 workers with 256-file items, and `CopyFile2` for shares, each measured by AngelCOPY. Not copied: for files of 32 MiB and more AngelCOPY uses its ring, and TinyTorrent2 put unbuffered `CopyFile2` there instead, with AngelCOPY's size limit, when it decided not to build a ring. Nobody measured that combination; BetterCopy does the same from 256 MiB, also unmeasured. BetterCopy's 2 workers for a USB bulk-only device and for a share | Copiers |
| D9 | `COPYFILE2_EXTENDED_PARAMETERS_V2::ioDesiredSize` | Not set until stage D measures it | Set it | Microsoft says it "may be reduced" and "may be ignored", and nothing has measured it | Copy APIs |
| D10 | Where a copy is written | Under a temporary name, `<name>.gozler-partial`, renamed when complete. A name that would pass 255 characters keeps a shorter start of its name and adds 8 lowercase hexadecimal digits of a hash of the full name: 32-bit FNV-1a over its UTF-16 units, each as its two little-endian bytes. The kept start is 232 units, one fewer when that would split a surrogate pair. Continue finds an earlier run's partial file by this name, so the hash is a saved format | The final name (File Explorer, robocopy) | A crash then never leaves a partial file that looks complete, and a move never deletes a source before its copy has its final name. The hash keeps two long names that start alike apart. Stage D measures the rename's cost on small files | Lessons from TinyTorrent2 |
| D11 | When the file list is made | When the work starts, by walking the disk | When the person asks | An operation can wait in the queue for minutes. TinyTorrent2 rejected a design that took the names at the request, because renames still running changed them | Lessons from TinyTorrent2 |
| D12 | How paths are compared | By identity: the final path (`GetFinalPathNameByHandleW`), component by component, with `VOLUME_NAME_GUID` on a local volume and `VOLUME_NAME_DOS` on a share, which has no volume GUID path | The path text | Two strings name one folder through case, short names, mount points and junctions; a text compare would miss an overlap or move a folder into itself | Lessons from TinyTorrent2 |
| D13 | What may be deleted | The entries of a delete; a moved source after its copy has its final name; an emptied source folder; a `.gozler-partial` file; the file that Replace replaces | Anything else, such as a folder a copy merged into | A path that an operation protects is never also a path it deletes | Lessons from TinyTorrent2 |
| D14 | Which operations wait, and who decides | The engine's queue: an operation waits for an earlier one that has not ended when one of them writes or deletes a path that the other reads or writes, a path counting as the entry and everything below it; copies that move bytes on one disk with a seek penalty wait for each other; nothing else waits | One queue for everything; one queue per disk; a queue in the window | Overlap protects files: with parallel workers nothing else orders work on one path, as TinyTorrent2's single payload worker did. The disk rule protects speed, so it applies only to a disk with a seek penalty: copies between SSDs run at once, as in File Explorer and robocopy. An ended operation makes nothing wait. A copy writes only `<destination>\<source name>` and its numbered names, not its whole destination folder, so a delete inside a backup folder does not wait for a copy into it. The wait is a file-safety rule, so it lives in the engine, where no caller can skip it, and never with the person | Lessons from TinyTorrent2 |
| D15 | Where the engine runs | In the window's process, with the person's token | In gozlerd | A pipe request to an elevated service would let any local user delete files as SYSTEM | — |
| D16 | Rename on one volume | Yes, for a move | Copy and delete, as FastCopy does | A rename changes only the folder entry, so 100 GB moves in milliseconds; AngelCOPY, BetterCopy and Ultracopier rename too | Copiers |
| D17 | Our own copy loop for the Windows engine | No: `CopyFile2` | A loop of `ReadFile`/`WriteFile` | BetterCopy deleted its own loop in favour of `CopyFileExW`; the ring (D7) is the one own loop, chosen by the person | Copiers |
| D18 | Verify a copy by reading it back | Not now | FastCopy's and TeraCopy's Verify | It reads every byte twice. The ring's own tests compare hashes; a Verify setting returns when the ring shows a defect in use | Copiers |
| D19 | How an operation survives a crash | The engine saves each operation before it changes a file, and offers it as Interrupted at the next start; the person chooses Continue or Dismiss | No record, as File Explorer; continue by itself, as TinyTorrent2 does once | Without the record, the person does not know which files moved. TinyTorrent2 continues a move once by itself because a torrent must be whole; a file manager that starts a large move at sign-in surprises the person. TinyTorrent2 never repeats an interrupted delete; Gozler offers it, because the person chooses Continue and the run walks the disk again, so it deletes only what is still there | Lessons from TinyTorrent2 |
| D20 | A copy within one volume that can clone blocks | A volume can clone when its flags (`GetVolumeInformationByHandleW`) include `FILE_SUPPORTS_BLOCK_REFCOUNTING`; a volume whose flags cannot be read, such as a share, cannot. There the ring clones the blocks itself (`FSCTL_DUPLICATE_EXTENTS_TO_FILE`), on every Windows version, then copies the metadata as for any file. The Windows engine leaves the clone to `CopyFile2` | The ring's full copy; an own clone in the Windows engine too; detecting ReFS by the file system's name, which says nothing about the volume's version or about another file system that clones | A clone writes no data, so it takes milliseconds for any size, and the ring would otherwise write every byte. `CopyFile2` clones by itself from Windows 11 23H2 with the February 2024 update (KB5034848); an own clone in the Windows engine would help only Windows before that, which has no Dev Drive. The owner's Dev Drive is ReFS | Copy APIs |
| D21 | A link inside a copied tree | Copied as a link and not entered: a junction or symbolic link becomes the same link at the destination | Follow it, as robocopy does without `/XJ` or `/SJ` | Following a link copies data from outside the selection and can loop forever; the copy then also differs from its source | Copy APIs |

## What an operation receives

Full paths: the active panel's selection, or Search's selection, which is
the files of one moment (window rule 10).

- **The engine reads each entry's kind itself.** It opens every selected
  entry with the open-reparse-point flag and reads its attributes and
  reparse tag (`FileAttributeTagInfo`), because Search's selection holds no
  reparse tag, and a junction taken for a folder would be entered.
- An entry below another selected folder is dropped; it goes with that
  folder. Two entries with one identity, such as one folder reached through
  two junctions, are one entry.
- For a copy or move, a destination inside a selected folder is refused, as
  File Explorer refuses it.
- An entry moved to the folder it is already in is left alone.
- **A selected entry that no longer exists** is not a refusal, because the
  selection is the files of one moment. A delete counts it as deleted. A
  move lists it as a failure, except at Continue, which counts it as moved
  when something exists at its final name, or for Keep both at one of its
  numbered names: after a crash Continue meets exactly these entries,
  because they already moved, while on a first run the engine knows it
  moved nothing, so a source gone meanwhile is never reported as moved. A
  copy lists it as a failure.
- A selected entry whose identity cannot be read, for example a file whose
  permissions deny the person every right, is listed as a failure with
  Windows' error and left out; the operation does not touch it, so the
  overlap test of D14 runs without it. Only a destination whose identity
  cannot be read refuses the operation.

Each test uses the identity comparison of D12.

**Which opens follow links.** A selected entry and every entry the walk
meets are opened with the open-reparse-point flag, so the engine never acts
on a link's target by accident. The destination is a folder the person
chose, so it is opened following links, and its identity is its target's:
a destination `C:\out` that is a junction to `C:\proj\build` is inside
`C:\proj`, and a copy of `C:\proj` there is refused.

**A folder with a reparse tag that is not a name surrogate,** such as a
OneDrive folder, is listed through a second handle opened without the
flag, after a check that both handles have the same volume serial number
and file ID (`FileIdInfo`), because a file ID is unique only on its own
volume. A folder of
Files On-Demand gets its children from the cloud filter, and a listing that
bypassed the filter could see an empty folder, which a move would then
delete. The check keeps a folder swapped for a junction between the two
opens from redirecting the walk.

**One volume or two** is decided at prepare, from the identities of the
sources and the resolved destination, because it decides whether the
operation moves bytes, its disk waits (D14) and its free-space check. A
rename that fails at run time with `ERROR_NOT_SAME_DEVICE` is a listed
failure, never a copy that nothing waited for or counted.

## Delete

1. A work item is one folder. A worker lists it (D4), deletes up to 256 of
   its files (D1), and queues its subfolders as new work items (D2). A work
   item carries its folder's open handle, not a path, so every open below
   it is relative to that handle (D1) and a swap of a parent for a link
   cannot redirect it.
2. Each folder counts what is still open below it: its own listing until
   the listing ends, each item of its files until the item is done, and
   each subfolder until it is deleted. A folder whose count reaches zero is
   deleted, and its parent's count goes down (D3); a folder that keeps a
   failed entry is not tried, so it and its parents stay. rmbrr's counter was
   complete before its first delete, because rmbrr walks first; with the
   streaming walk, a count of the subfolders alone would reach zero while
   other workers still delete the folder's files.
3. **A link is deleted, never entered.** A folder whose reparse tag is a name
   surrogate (`IsReparseTagNameSurrogate`: a junction, a symbolic link or a
   mount point) is deleted as a link, because entering it would delete files
   outside the selection. A cloud placeholder's tag is not a name surrogate,
   so a OneDrive folder is entered.
4. **Short retries.** A delete refused with a sharing violation or a lock
   violation, and a folder that is still not empty, are tried again, the
   first wait 1 ms and each wait 1.2 times longer than the one before,
   until the waits add up to 200 ms, as rimraf does; scanners and indexers hold files open for a
   moment. Access denied is a failure at once: D1 already deletes read-only
   files, so what remains is a permission, which does not change in 200 ms,
   and retrying it would cost 125 s on 10,000 files the person may not
   delete. rimraf and Rust's standard library do not retry it either.
5. **One failure does not stop the rest.** A file that cannot be deleted is
   recorded with Windows' error, and its folders stay. A file the walk
   listed that is already gone counts as deleted.

## Copy

1. **The walk streams,** as the delete does and as AngelCOPY's `WalkStream`
   does. It runs when the copy starts (D11), on the same workers as the
   files, so the worker count of the volume pair bounds both. A work item
   is one source folder. The worker that lists it (D4) creates its
   destination folder, reads that folder's existing names once (step 2),
   queues its subfolders as new work items, and hands its files on: up to
   256 files under the large-file limit as one item for the workers, and
   each large file to the one-at-a-time lane. Between volumes without a
   seek penalty the lane runs beside the workers, as AngelCOPY's does; when
   either volume has one, large files join the workers' queue, so one file
   moves at a time, because a second stream makes a hard disk seek.
   Copying starts while the walk still runs, and memory holds only the
   folders in flight. A name-surrogate entry is copied as a link and not
   entered (D21).
2. **Conflicts.** One listing of each destination folder that already
   exists gives the set of its names, folded as Windows compares them: each
   UTF-16 unit upper-cased one to one (`CharUpperW` on the unit, or a table
   in `gozler-fileops`), with no normalization. Neither
   `gozler_core::fold`, which normalizes, nor Rust's `to_uppercase`, which
   turns `ß` into `SS`, matches NTFS, and a false match with Skip would
   leave a file uncopied. Several workers hand files into one destination
   folder, for example the destination of a Search selection from many
   folders, so each set has its own lock. The worker that hands a file on
   decides its final name, its number included, under that lock and adds
   the name to the set, so two workers never give one number. A conflict,
   and Keep both's first free number, are lookups in that set, not one
   probe per name: a folder that holds `index (2).html` to
   `index (500).html` costs no 500 probes. A name taken after the listing is
   caught at the final rename (step 6). Two selected entries with one name
   are also a conflict. Folders with one name merge, as in File Explorer.
   Prepare counts conflicts the same way.
3. **Free space.** A copy that needs more bytes than the destination volume
   has free is refused at prepare, as File Explorer refuses it. The run
   compares prepare's byte count with the free space again before it
   starts, and does not count again. Within one volume that can clone
   blocks the check is skipped for both engines, because both clone there
   (D20). A disk that fills during the copy, also on a Windows that does
   not clone, stops it as step 9 says.
4. **Folders** are created with `CreateDirectoryExW` and the source folder
   as the template, which keeps its attributes, each by the one worker that
   lists its source folder, so no two workers create one folder. Creating a
   folder and opening a file use the delete's short retries (Delete, step
   4), and a folder create also retries `ERROR_PATH_NOT_FOUND` when this
   operation made its parent: AngelCOPY added that after about 30 folders
   on a file server returned it and their subtrees were lost.
5. **Files** are copied by the engine that the speed settings choose for
   their case (D7). Each is written under its temporary name (D10), after any
   `.gozler-partial` file of that name is deleted, because only an
   interrupted run of Gozler makes that name.
6. **The final rename** is `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`,
   except for Replace. When another file took the final name after the walk,
   the rename fails with `ERROR_FILE_EXISTS` or `ERROR_ALREADY_EXISTS`. It
   fails with `ERROR_ACCESS_DENIED` both for a hidden or read-only file and
   for a folder whose permissions refuse the rename, so on that code the
   engine asks whether the final name exists (`GetFileAttributesExW`). A
   final name that exists is a conflict with someone else's file, never
   deleted (D13): the operation's choice applies, and with no choice the
   copy is deleted and the file is listed as skipped. Otherwise the error is
   a failure.
7. **Replace** clears the read-only attribute of the file it replaces, then
   renames over it.
8. **A file and a folder with one name** are a listed failure whatever the
   choice, never replaced, as File Explorer refuses them.
9. **Failures.** A file that fails is recorded with Windows' error, and the
   copy goes on. A full destination disk stops the copy, because every later
   file would fail too.

Hard links in a copied tree become separate files, as with File Explorer and
robocopy.

### The Windows engine

| Case | Method |
| --- | --- |
| Both volumes local, file of 32 MiB or more | `CopyFile2` with `COPY_FILE_NO_BUFFERING`, one file at a time |
| Both volumes local without a seek penalty, file under 32 MiB | `CopyFile2` on 16 workers, 2 when either volume is on USB, each taking up to 256 files of one folder |
| A network share at either end | `CopyFile2`, buffered, on the share's workers (2 in Parallel copy, as BetterCopy uses), so the round trips of one file overlap another's. A share's seek penalty cannot be read, so it counts as a hard disk for D14: two copies to one server wait for each other |
| Every other case | `CopyFile2`, buffered, one file at a time. A volume whose seek penalty is unknown counts as a hard disk |

The 32 MiB limit and the worker counts are speed settings; the table shows
the values of the Parallel copy preset.

A link, an entry whose reparse tag is a name surrogate, is copied with
`COPY_FILE_OPEN_AND_COPY_REPARSE_POINT` (Windows 10 build 19041 and later):
"Always copy the reparse point regardless of type", junctions included
(D21). Every other file is copied without it, because a OneDrive
placeholder, a compressed (WOF) or a deduplicated file also has a reparse
point, and copying that point instead of reading through its filter gives
a file without its data. A sparse file stays sparse with
`COPY_FILE_ENABLE_SPARSE_COPY` on Windows build 22621 and later when the
destination supports sparse files. `CopyFile2` keeps alternate data streams
such as `Zone.Identifier`, attributes and dates, and tries ODX and SMB
offload by itself: its parameter page says that only its low free space
mode does not. Between shares on one server AngelCOPY measured
`CopyFileExW` 30 times faster than its ring.

Rules that TinyTorrent2's `Payload.cpp` and the copiers learned:

- **The 32 MiB limit is AngelCOPY's; BetterCopy uses 256 MiB** for
  `COPY_FILE_NO_BUFFERING`. Stage D measures both limits.
- **The seek penalty and the bus type** come from
  `IOCTL_STORAGE_QUERY_PROPERTY` on a volume handle opened with no access
  rights, as BetterCopy reads them, so no administrator right is needed.
- **A volume's class** is one of four, and the worker counts, the ring's
  profile and the disk wait (D14) all read it: a share, by its path; a
  hard disk, with a seek penalty or an unknown one; USB, without a seek
  penalty and a USB bulk-only device by BetterCopy's test in
  `profiler.rs`; otherwise an SSD.
- **`COPY_FILE_REQUEST_COMPRESSED_TRAFFIC` is not set:** compression helps
  only on a slow link and costs CPU on a fast one. Robocopy leaves it to its
  `/compress` switch.
- **The Windows build** comes from `RtlGetVersion`, because
  `GetVersionEx` reports the build that the executable's manifest declares,
  not the running one. The sparse flag is passed only where it exists:
  Microsoft does not document what an older Windows does with it.
- **The free space a copy needs** counts a sparse file that stays sparse at
  its allocated size (`GetCompressedFileSizeW`), and every other file at its
  full size, because a sparse file copied where it cannot stay sparse takes
  its full size on the destination.
- **Progress is limited to the file's size:** `CopyFile2`'s
  `uliTotalBytesTransferred` also counts the alternate data streams.
- **Long paths:** every path goes to Windows with the `\\?\` prefix, or
  `\\?\UNC\` for a share, because Windows takes a path longer than
  `MAX_PATH` only with it. A volume root goes without the prefix, because not
  every volume query accepts it.

### The ring engine

Gozler's own copy loop, from FastCopy's and AngelCOPY's designs (research,
"Copiers"). Their code is not used: FastCopy v3 is GPLv3 and AngelCOPY has no
licence.

| Setting | Hard-disk profile (FastCopy) | SSD profile (AngelCOPY) |
| --- | --- | --- |
| Applies to | every file, when either volume has a seek penalty | files of 32 MiB and more, when neither volume has one; smaller files use the Windows engine's workers |
| Block | 2 MB, FastCopy v5's default | 8 MiB |
| Blocks in flight | 8 reads and 8 writes, as FastCopy's help recommends | 8 in total, each reading or writing, as AngelCOPY's `RingCopyFile` |
| Buffer | 256 MB, shared by all files | the 8 blocks, 64 MiB |
| One physical disk | read until the buffer is full, then write all of it | not used: an SSD has no seeks |
| Two disks | reader and writer run at the same time | each block is written when it is read |
| Files opened ahead | 16 threads, up to 256 open files | one file at a time |

The 32 MiB limit, the blocks, the blocks in flight and the buffer are speed
settings; the table shows the values of the Ring copy preset.

Both profiles:

- **Unbuffered:** sources open with `FILE_FLAG_NO_BUFFERING`; destinations of
  64 KB and more too. Smaller destinations in the hard-disk profile go
  through the cache, because a small unbuffered write waits for the disk.
  Each block starts at an address aligned to the larger sector size of the
  two volumes.
- **Overlapped** I/O on a completion port for files of 512 KB and more;
  smaller files use synchronous I/O.
- **A ring error falls back.** Any error of the ring other than a cancel, a
  sharing violation or a full disk copies that file again with the Windows
  engine and counts it in the result, as AngelCOPY's `CopyOneBig` does, so
  a file the ring cannot copy, such as one on a disk with an unusual sector
  size, is still copied. A sharing violation stays a listed failure,
  because it is how the ring refuses a file another program is writing; a
  full disk stops the copy (Copy, step 9).
- **FAT destinations** are written unbuffered from 128 KB, not 64 KB,
  FastCopy's limit for FAT.
- **The end of file is set first,** to the size rounded up to a whole
  sector, as FastCopy and AngelCOPY do, and then set to the true size after
  the last block. A write that extends a file runs synchronously (Microsoft
  KB 156932), so growing the file with each write would serialize the
  writes in flight. Ultracopier reserves space with `FileAllocationInfo`
  instead, so that a partial file stays short; the temporary name (D10)
  already makes a partial file harmless.
- **Sources share read and delete, not write.** A file that another program
  has open for writing then fails with a sharing violation and is listed,
  instead of being copied as a mix of old and new bytes under its final
  name.
- **Sparse files:** the reader reads only the allocated ranges
  (`FSCTL_QUERY_ALLOCATED_RANGES`). On a destination that supports sparse
  files, the writer marks it sparse (`FSCTL_SET_SPARSE`), sets its size, and
  writes only those ranges; elsewhere the holes are written as zeros.
- **Metadata:** every alternate data stream and the extended attributes go
  through `BackupRead`/`BackupWrite`, skipping the main stream; attributes and
  the creation, access and write times through `FileBasicInfo`. Security
  descriptors are not copied, as `CopyFile2` does not copy them by default.
  This is FastCopy's method. AngelCOPY's ring copies only times and
  attributes and so loses `Zone.Identifier`, the mark that makes Windows
  warn before it runs a downloaded file.
- **The clone (D20):** within one volume that can clone blocks the ring sets the
  destination's end of file to the source's size, marks it sparse when the
  source is, and clones the data in ranges under 4 GB, the largest range
  Windows accepts. The data stream is then complete, and the metadata
  follows as for a copied file. A refused clone falls back to the ring's
  copy and is counted in the result, so a clone that never runs shows.
- **Handed to the Windows engine:** a network end, an unbuffered open that a
  file system filter refuses, any entry with a reparse tag, and an EFS file.
  The walk opens entries without following reparse points, so a ring read
  of a OneDrive placeholder or a compressed file would bypass its filter. AngelCOPY measured its
  ring 30 times slower than `CopyFileExW` between two shares on one server,
  and about 2 times slower from a share to a local disk, because buffered
  reads from a share read ahead.
- **Progress** counts each byte once, when it is written. On one hard disk
  the progress moves in steps of up to 256 MB.
- The buffer exists only while a copy runs.

### Speed settings

The two engines differ only for large files between SSDs and when a hard
disk is at either end; everywhere else both do the same thing. The speed
settings decide those cases and the numbers both engines use. Every copy,
move and delete receives them, and a saved operation keeps them, so
Continue runs with the same ones. They are one typed value whose
constructor refuses a value out of range, so a bad value from the Settings
page or from `ui.json` never reaches a copy.

| Setting | Range | What it decides |
| --- | --- | --- |
| Large-file limit | 1 to 1,024 MiB | from this size, a file between two local volumes is large: the Windows copy function copies it unbuffered and one at a time, and between SSDs the next row may give it to the ring |
| Large files between SSDs | Windows copy function, unbuffered; or the ring's SSD profile | the first case where the engines differ |
| A hard disk at either end | Windows copy function; or the ring's hard-disk profile, for every file | the second case |
| Workers for files that are not large | 1 to 64, set separately for SSD, USB, hard disk and share | how many files Windows' copy function copies at once |
| Ring, SSD profile | blocks of 64 KiB to 64 MiB, a power of two; 1 to 64 in flight | a power of two keeps every block aligned to any sector size |
| Ring, hard-disk profile | blocks as for the SSD profile; 1 to 64 reads in flight and as many writes; a buffer of 16 to 1,024 MiB | |
| Delete workers | 1 to 64, or one per logical processor | D2 |

**Presets.** The Settings page offers these. A preset sets every value;
changing one value makes the choice Custom. A preset is shown by what it
does, not by the copier it comes from, because Gozler uses none of their
code and a copier's name would promise that copier's exact behavior.

| Preset, as shown | Large-file limit | Large files between SSDs | Hard disk | Workers SSD / USB / hard disk / share | Ring SSD | Ring hard disk | Delete workers | From |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Automatic (default) | 32 MiB | Windows | Windows | 16 / 2 / 1 / 2 | 8 MiB × 8 | 2 MB × 8, 256 MB | one per processor | starts as Parallel copy's values; stage D replaces each value it measures, and the After install checks replace the hard-disk, USB and share values |
| Parallel copy | 32 MiB | Windows | Windows | 16 / 2 / 1 / 2 | — | — | one per processor | TinyTorrent2's `Payload::Move`; share workers from BetterCopy; delete from rmbrr |
| Ring copy | 32 MiB | ring | ring | 16 / 2 / 1 / 2 | 8 MiB × 8 | 2 MB × 8, 256 MB | one per processor | AngelCOPY's ring and FastCopy v5's hard-disk mode |
| Ring for large files, many small files at once | 32 MiB | ring | Windows | 16 / 16 / 16 / 16 | 8 MiB × 8 | — | 8 | AngelCOPY 1.6.0 |
| Windows copy function, unbuffered from 256 MiB | 256 MiB | Windows | Windows | 16 / 2 / 1 / 2 | — | — | one per processor | BetterCopy |

A dash is a value the preset does not use; it keeps the Automatic value.
Three copiers have no preset, because the engine cannot reproduce them:
FastCopy, which on an SSD runs its reader and writer at once over its
large buffer, a mode the ring has only for hard disks; Ultracopier's
buffered ring; and TeraCopy, whose method is not published. FastCopy's
hard-disk mode is in Ring copy.

## Move

- **On one volume** each selected entry is renamed (`MoveFileExW` without
  `MOVEFILE_COPY_ALLOWED`), a whole folder in one call. No walk runs, except
  into folders that merge: a folder of that name at the destination makes
  the move enter both, rename each child into the existing folder, merge
  child folders the same way, and delete each emptied source folder.
  Prepare counts the conflicts of those merged folders as it does for a
  copy.
- **Across volumes** it is a copy as the speed settings choose, and each source
  file is deleted when its copy has its final name. A source folder is
  deleted when it is empty (D3). A cancel takes effect before a file's
  final rename or after its source's delete, never between them, so a
  cancel or a failure leaves each file in one place.

## Conflicts

The person gives one answer for all conflicts that prepare found:

| Choice | A file that exists at the destination | Two selected entries with one name |
| --- | --- | --- |
| Replace | replaced | the second gets a number |
| Skip | left as it is; the source is not copied or moved | the second is skipped |
| Keep both | the new one gets the first free number: `report (2).pdf` | the second gets a number |

- **The run applies the choice to every conflict it meets,** also to a file
  that appeared after prepare. When prepare found no conflict, the person
  was not asked, and the run skips each conflict it meets and lists it in the
  result, because nobody chose to replace that file.
- Two selected entries never replace each other: on a move that would delete
  one of the person's files, and on a copy the result would depend on the
  order.
- A copy into the entry's own folder gets a number without a question, as
  in File Explorer, because Replace would replace the source with its own
  copy.

## What a stopped operation leaves

| Operation | Stopped by | Leaves |
| --- | --- | --- |
| Copy | Cancel or a failure | the files copied so far and the folders made; the files in progress are deleted |
| Copy | power loss or a crash | the files copied so far and the folders made, and the file in progress under its temporary name until Continue deletes it; Dismiss leaves it, and the Interrupted row says so |
| Move on one volume | anything | each entry in one place: a rename is complete or did not happen |
| Move across volumes | Cancel or a failure | each file in one place: moved, or still at its source |
| Move across volumes | power loss or a crash | each file in one place, except the file in progress, which is whole at its source and under its temporary name at the destination, and a file whose source delete had not run, which is at both |
| Delete | anything | the entries not yet deleted |

A file at its final name is always complete.

## The interface

The window talks to one object, the engine's queue. It creates the queue at
start with two things: the file where operations are saved, and a wake
function that the queue calls after each change of an operation's state.
The window's wake function only posts a message (window rule 5) and then
reads the states. It posts only while a flag that the window clears in
`WM_DESTROY` is set, because at Exit engine threads can outlive the window
by up to 3 seconds, and a message to a destroyed window handle can reach a
new window that reuses it.

1. **Prepare** takes the kind (copy, move, delete, rename or new folder), the
   speed settings, the sources and the destination, and returns an id at once.
   On a queue thread it applies "What an operation receives" and ends
   prepared, or ended with a typed refusal: a destination inside a source,
   a destination that does not exist, a destination whose identity cannot
   be read, or too little free space. A selected entry that no longer exists counts as
   "What an operation receives" says. A
   prepared operation holds the sources and the destination as identities
   (D12), the paths it reads and the paths it writes or deletes (D14),
   whether it moves bytes, and its disks. For a copy or a move it also counts
   the items, the bytes, the file conflicts and the selected entries that
   share a name, so the window can ask the person. A delete's prepare does
   not walk, so the delete starts at once; its progress is the count of
   items deleted, without a total. It keeps no list of files (D11).
2. **Submit** takes the id of a prepared operation and the conflict choice,
   if the person was asked. The queue starts the operation as soon as D14
   lets it.
3. **The operation runs** on its own thread: it checks the free space again
   (Copy, step 3), walks as it copies, applies "Conflicts", and ends complete, cancelled, or stopped
   by a full disk, with the counts of what was done and each failure or
   skipped file with its path and Windows error, in the order they happened.

**Each operation's state** is one of: preparing; prepared, waiting for the
person's answer; waiting, with the id of the operation it waits for and
whether for its paths or its disk; running, with its progress; ended, with
its result; or interrupted. An ended operation stays until the window
removes it. The queue is the one owner of which operations exist.

**Taken entries.** The queue answers whether a path is taken: it equals or
is below a source of a waiting, running or ended delete or move, compared
as Windows compares names, ignoring case. The window asks this, and nothing
else, to leave those entries out of the panels and show them blank in the
search results. The
window removes an ended operation from the queue when no list still needs
it: a panel releases it when the operation ends, because it lists the disk
again; a search list releases it when its search changes, because gozlerd
applies a large delete's journal records for some time after the delete.
This is a display rule on the path text the window gave; the safety rule
stays D14 on identities.

The test runs for every row Windows paints, about 50 rows per paint, so it
must not compare a row with every taken path: 100,000 deleted rows would
cost 5 million compares per paint. The queue keeps the taken sources in a
hash set of folded full paths, and a row is taken when its folded path or
one of its parent paths is in the set, a few dozen lookups per row. At each
wake the window takes a snapshot of the set (`Arc`), so a paint never waits
for the engine's lock while a worker holds it.

**The overlap test (D14)** compares the paths of two operations through a
trie of their identity components: each path of one operation walks the
other's trie once. Two selections of 100,000 entries then cost about a
million steps instead of 10 billion pairwise compares. The same trie
answers "an entry below another selected folder" and "a destination inside
a source".

**Cancel** takes an id. A preparing, prepared or waiting operation ends at
once. A running one stops within one `CopyFile2` chunk (its callback returns
`COPYFILE2_PROGRESS_CANCEL`), one ring block, or one delete and its 200 ms of
retries. A `CreateFileW` blocked on a share that stopped answering is ended
with `CancelSynchronousIo` on its thread, as `gozler_winfs`'s
`cancel_synchronous_io` does for gozler's other blocked calls. A read in
flight inside `CopyFile2` is overlapped (its page says only the low free
space mode uses no overlapped I/O), so it waits for the share's timeout.

**One queue per user session.** Only the window that owns the
single-instance mutex creates a queue. Another window, which Gozler opens
when "Allow only one Gozler window at a time" is off (DESIGN.md, "Window
lifetime"), offers no file operations, as it shows no tray icon: two queues
would replace each other's saved file, and an interrupted operation would
then be missing at the next start.

**Saved operations (D19).** Before a running operation changes its first
file, the queue writes its kind, its speed settings, its sources, its
destination and its conflict choice to the saved file: to a temporary file,
then renamed over the old one, as `ui.json` is written. It removes the
operation when it ends, cancelled included. A waiting operation is not
saved, because it has changed no file. When the queue is created, each
saved operation becomes an interrupted one.

- **Continue** prepares and submits it again with one more rule: a file at
  the destination's final name, or for Keep both at one of its numbered
  names, with the source's size and last write time is done. The times
  compare within 2 seconds, or within one hour and 2 seconds after a
  daylight saving change, because FAT keeps them to 2 seconds in local time,
  as TinyTorrent2's `Payload::IsCopy` does. Its prepare applies this rule
  while it counts, from the destination listing it reads for conflicts, so
  a file found done counts no bytes: otherwise a 100 GB copy that crashed
  at 90 GB on a drive with 120 GB free could not continue.
- **A move that continues keeps the source of a file found done** and lists
  it in the result. Equal size and time are a guess, and TinyTorrent2 trusts
  that guess only because a hash check of the torrent follows; Gozler checks
  no hash (D18), so it never deletes a source on it.
- **Dismiss** removes it.
- The file's keys are a saved format: an older entry that lacks a key keeps
  its old meaning, as `AGENTS.md` asks of the wire.

**Disks.** A prepared operation names the physical disks of every volume it
reads or writes (`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` on a volume handle
opened with no access rights; a volume can span disks). A network share
counts as one disk, named by its server. A volume whose disks cannot be read
counts as a disk of its own.

**Rename and new folder** go through the queue like the others. A rename
reads and writes its old and new names, and a new folder writes its own
name, so each waits only for an operation on those paths. A rename uses
`MoveFileExW` without a replace flag, and a new folder `CreateDirectoryW`,
so an existing name is refused, never replaced.

**Measure**, for Space on a folder in a volume that gozlerd does not index,
counts a folder's items and bytes outside the queue, because it changes
nothing; it can be cancelled.

**Progress** is atomic counters of bytes or entries, which the window reads
about twice a second.

**Threads.** The queue owns every engine thread: one for each preparing or
running operation, and the workers of each run, which are scoped threads
that the run creates and joins. Dropping the queue cancels every operation
and joins every thread, so no engine thread outlives it. At Exit the window
cancels every operation and waits at most 3 seconds for the threads; then
the process ends. An operation that had not ended stays in the saved file
and shows as Interrupted at the next start, which is the truth. The process
keeps its single-instance mutex until it ends, because a second Gozler
started earlier would read and rewrite the same saved file.

**Handles.** Every handle the engine opens shares delete, so its own handles
never make a delete fail. TinyTorrent2's mark writer opens without
`FILE_SHARE_DELETE`, and a delete of the same file then fails.

## Where the code goes

- **`gozler-winfs`:** every Win32 call, as safe functions: open an entry
  relative to a folder handle, list a folder, delete one entry, rename,
  `CopyFile2` with a progress closure that can cancel, clone a file's blocks,
  create a folder from a template, the final path, a volume's class and
  disks and free space, and the ring's unbuffered, overlapped reads and
  writes, sparse ranges and backup streams. An overlapped read or write owns
  its buffer until the I/O completes, and waits for that completion when it
  is dropped, because Windows writes into the buffer until then.
- **`gozler-core`:** the `FileFullDirectoryInfo` parser, and the parser of
  `BackupRead`'s stream headers (`WIN32_STREAM_ID`), each with a
  test that malformed bytes return an error, because `gozler-winfs` parses
  nothing. `deny.toml`'s list of `gozler-core`'s dependents gains
  `gozler-fileops`.
- **`gozler-fileops`:** everything in this spec. No `unsafe`, no window code.
  Its decisions (the numbers for Keep both, the temporary name, nested
  selections, a destination inside a source, the overlap test, taken paths,
  the method for a pair of volumes, the ring's block plan) build and test on
  every OS; the disk work is Windows only.
- **`gozler-ui`:** everything [the file manager spec](file-manager-spec.md)
  describes. It shows the queue's states and decides nothing about which
  operation waits.

The engine is a crate because `gozler-ui` is a binary only: the measurement
program and the engine's tests on real folders need a library.

## Not verified

A stage runs each Win32 case once and writes the result here, or the case
is in the plan's "After install".

- Whether the ring can set the true end of file on its unbuffered handle.
  AngelCOPY reopens the file without `FILE_FLAG_NO_BUFFERING` to do it.
  Stage C runs it once.
- What one rename per copied file (D10) and one listing per destination
  folder cost on a share. After install times a small-file copy to a share.

- ReFS accepts `FILE_DISPOSITION_FLAG_POSIX_SEMANTICS` (stage A, run once on
  2026-10-19, Windows 11 25H2 build 26200, the owner's Dev Drive): with
  "ignore read-only" it deleted a file, a read-only file and an empty
  folder, as NTFS did, so D1's fallback is not taken there. An older ReFS
  that refuses it takes the fallback.
- A share refuses `FILE_DISPOSITION_FLAG_POSIX_SEMANTICS` with
  `ERROR_INVALID_PARAMETER`, so a delete there takes D1's classic delete
  (stage A, run once on 2026-10-19 through `\\localhost\C$`). The delete
  opens ask for no `SYNCHRONIZE` and no synchronous I/O, so that a file
  whose own permissions grant nothing is still deleted through its
  folder's `FILE_DELETE_CHILD`. Over the share, on those handles, the
  refused POSIX call, the `FileBasicInfo` query and set, and the classic
  delete all completed: stage A's delete tests passed with their folders on
  the share, a read-only file, a held file and a file that grants nothing
  included.
- That `GetFinalPathNameByHandleW` with `VOLUME_NAME_DOS` gives one path for
  one share folder reached two ways (D12). After install.
- Whether `CopyFile2` copies a file that another program has open for
  writing. Stage B runs it once; the spec then says what the Windows engine
  does about it.
- How `CopyFile2` with `COPY_FILE_OPEN_AND_COPY_REPARSE_POINT` copies a
  folder junction, a folder symbolic link and a file symbolic link without
  Developer Mode. A link that Windows refuses to create is a failure with
  Windows' error (D21).
- Whether `COPY_FILE_NO_BUFFERING` stops `CopyFile2`'s own clone within one
  volume that can clone blocks. Stage D's Dev Drive row shows it: if it
  does, the Windows engine leaves the flag off within such a volume.
- How `FSCTL_DUPLICATE_EXTENTS_TO_FILE` takes a file whose size is not a
  whole number of clusters, and a file larger than 4 GB in several ranges
  (D20). Stage C runs it once on the Dev Drive.
- Whether the parallel walk is slower than one walker on a hard disk.
- Every speed claim of D2 and D7 to D10 on the owner's machine. Stage D
  measures them.

## Done when

- Every row of "Tests" in the plan passes, on folders the tests create.
- Stage D's measurements are in `docs/BENCHMARKS.md`, and each decision they
  test (D2, D7, D8, D9, D10) is confirmed or changed with the numbers beside
  it.
- `check.ps1` is green.
