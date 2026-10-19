# Fast copy, move and delete on Windows

The evidence behind [the engine spec](../plans/fast-copy-delete-spec.md):
which tools and Windows APIs were read, what each one does, what each one
measured, and how the work got here. The spec holds the decisions; this file
holds why a reviewer can trust them.

**How to read this file.** Each fact is marked:

- **Confirmed**: the primary source says it, at the link given.
- **Corrected**: an earlier claim was wrong; the row gives what the source
  says.
- **Not found**: no primary source says it.

A number a tool's author measured on their own machine is that author's
measurement, not ours. Stage D of [the plan](../plans/fast-copy-delete-plan.md)
measures on the owner's machine.

**Reading limits.** TinyTorrent2 read the tools on 2026-10-18. gozler checked
every claim against its primary source on 2026-10-19, through an agent that
read the GitHub repositories at their default-branch heads and the
Microsoft Learn source markdown. The commit given is the head that was read.
Where a README and the code disagree, the code wins, and the row says so.
No code from these tools is copied into gozler.

## How the work got here

| Date | Where | What happened |
| --- | --- | --- |
| 2026-10-18 | TinyTorrent2, commit `2a1bdd5` | A plan for moving torrent files in the engine read the copiers and deleters below and chose a method for each pair of volumes, with an own ring engine. |
| 2026-10-18 | TinyTorrent2, commit `5ef86e7` | The ring engine was recorded as "won't implement" (`docs/plans/ring-copy-plan.md`): a ring defect would make torrents download data again, and the ring wins only in rare cases. |
| 2026-10-18 | TinyTorrent2, commit `0d6e588` | `Payload::Move` and `Payload::Delete` were built with `CopyFile2` and parallel POSIX delete. The plan (`git show 0d6e588^:docs/plans/payload-move-plan.md`) was deleted into the engine contract. |
| 2026-10-19 | TinyTorrent2, `docs/plans/14-removed-torrents-plan.md` (issue SynoSoftware/TinyTorrent2#540) | Five models redesigned how a removed torrent leaves the list while its files are busy. Its lessons are in "Lessons from TinyTorrent2" below. |
| 2026-10-19 | gozler | The owner asked for copy, move and delete "as fast as possible", then for a Norton Commander style mode, operations in the background with a queue, and a UI that never waits for disk work. Where the research does not show which method is faster, Gozler builds both and the person chooses. |
| 2026-10-19 | gozler | The window's coordinator session reported why the window's plan took about 18.5 hours against about 5.6 hours for the es superset; `AGENTS.md` ("Writing a plan") and [the file manager spec](../plans/file-manager-spec.md) hold the lessons. |
| 2026-10-19 | gozler | This record. Then an adversarial review of the specs by the window's session, and the code-review skill's `plan2` review. |
| 2026-10-19 | gozler | Owner ruling: no Recycle Bin. Gozler is the tool for speed, every delete is permanent, and a person who wants the Recycle Bin uses File Explorer. The `IFileOperation` rows below stay as evidence of what was not used. |

## Copiers

### FastCopy

Shirouzu Hiroaki. v3.63 source: <https://github.com/shirouzu/FastCopy> (head
`72655443`, last push 2019-02-25), GPLv3 (`doc/readme_eng.txt`). Current
help, v5: <https://fastcopy.jp/help/fastcopy_eng.htm>; v5 is under an EULA,
and no v5 source was found.

| Fact | Verdict | Source |
| --- | --- | --- |
| Its own unbuffered, overlapped `ReadFile`/`WriteFile`; sources open with `FILE_FLAG_NO_BUFFERING`, `FILE_FLAG_SEQUENTIAL_SCAN` and `FILE_FLAG_OVERLAPPED` | Confirmed | help, "I/O Settings"; `src/fastcopyr.cpp` `FastCopy::OpenFileProcCore`; `src/fastcopyw.cpp` `FastCopy::WriteFileProcCore` |
| One big buffer, 256 MB by default on 64-bit (128 MB on 32-bit), with used and free offsets | Confirmed for v3.63 (`DEFAULT_BUFSIZE 256`). v5's help gives no default. | `src/cfg.cpp` `Cfg::ReadIni`; `src/fastcopy.h` |
| I/O size | Corrected: v3.63 used 1 MB (`DEFAULT_MAXOVLSIZE 1`) with 20 in flight (`DEFAULT_MAXOVLNUM 20`). v5.11.0 changed the default to 2 MB, and the current help recommends 2 MB and 8 overlapped I/Os. | `src/cfg.cpp`; help, history v5.11.0 and "OverLap I/O Num" |
| One physical disk ("Same HDD"): read until the buffer is full, then write it in bulk. Two disks ("Diff HDD"): reader and writer threads at once. An SSD is always "Diff". Auto by default; `/disk_mode=auto\|same\|diff` | Confirmed: "Reading is processed until the big buffer fills. When the big buffer is filled, writing is started and processed in bulk." | help, "About" and the HDD mode option; `src/fastcopy.cpp` `FastCopy::InitSrcPath` |
| The physical disk comes from `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`, first extent only, then `\\.\PhysicalDriveN` | Confirmed | `src/utility.cpp` `DriveMng::SetDriveID` |
| Verify with a hash: xxHash3 by default, also MD5, SHA-1, SHA-256 and others; "Perfect verify", which reads both again, is Pro only | Confirmed | help, "Verify" |
| Never renames on one volume; does not keep sparse files sparse | From TinyTorrent2's reading on 2026-10-18; not rechecked | `ring-copy-plan.md` |
| Its documentation: parallel reads and writes on one hard disk "will be slow because many HDD head seeks will occur" | From TinyTorrent2's reading; no published measurement | `ring-copy-plan.md` |

### AngelCOPY

<https://github.com/vangel76/AngelCOPY>, head `b5c63a72`, v1.6.0 of
2026-10-03. No licence file; the GitHub API reports none. The README is stale:
it still names `robocopy /MT:64` as the engine, which changelog 1.5.2
removed. Cite `CHANGELOG.md` and the source.

| Fact | Verdict | Source |
| --- | --- | --- |
| A native Win32 engine replaced robocopy in 1.0.0 (2026-07-18) | Confirmed | `CHANGELOG.md` [1.0.0] |
| A move on one volume is `MoveFileExW`; copy and delete only on `ERROR_NOT_SAME_DEVICE`. 2 GiB: 0.7 ms against about 1 s | Confirmed | `CHANGELOG.md` [1.0.0]; `AngelCopyRunner/NativeCopy.cpp` |
| Files of 32 MiB and more: an unbuffered, overlapped ring on a completion port, 8 MiB blocks, 8 in flight (`kBigFileBytes`, `kRingChunk`, `kRingDepth`) | Confirmed | `NativeCopy.cpp` `RingCopyFile` |
| Preallocation sets the end of file to the sector-aligned size (`FileEndOfFileInfo`), then truncates to the true size at the end | Confirmed; it does not use `FileAllocationInfo` | `NativeCopy.cpp` `RingCopyFile` |
| Smaller files: `CopyFileExW` on 16 workers (`PoolThreads`, overridable), items of up to 256 files of one folder (`kChunkFiles`) | Confirmed | `NativeCopy.cpp` |
| A network end (UNC or `DRIVE_REMOTE`) always uses `CopyFileExW` | Confirmed | `NativeCopy.cpp` `IsRemotePath`, `CopyOneBig` |
| Deletion: `DeleteFileW` and `RemoveDirectoryW` on 8 workers (`kDelThreads`) | Confirmed | `AngelCopyRunner/Delete.cpp` |
| One 8 GiB cold file on NVMe: 3,200 MB/s against robocopy's 2,430 MB/s (+32 %), by `bench/bench.cpp` | Confirmed | `CHANGELOG.md` [1.0.0] |
| 10,000 files of 64 KiB in a nested tree: 33 % faster than `robocopy /MT:64`; equal on one flat folder | Confirmed | `CHANGELOG.md` [1.0.0] |
| 64 workers: 130 MB/s; 16 workers by folder: 428 MB/s | Confirmed | `CHANGELOG.md` [1.0.0] |
| A 2 GiB file between shares on one server: `CopyFileExW` 0.2 s, the ring 6–7 s. From a share to local: unbuffered 4.9–5.1 s, buffered 2.0–2.7 s | Confirmed (July 2026) | `NativeCopy.cpp`, comment above `IsRemotePath` |
| Deleting 10,000 files of 64 KiB: 1,358–1,656 ms on one thread, 330–380 ms on 8 (3.9×); 16 threads were no better than 8 | Confirmed | `Delete.cpp`, comment above `kDelThreads` |
| Its ring copies only times and attributes, so it loses `Zone.Identifier` | From TinyTorrent2's reading; not rechecked | `ring-copy-plan.md` |

### BetterCopy

<https://github.com/articulite/BetterCopy>, head `a3e239bb`, MIT, Rust.

| Fact | Verdict | Source |
| --- | --- | --- |
| `CopyFileExW` for every file | Confirmed | `better_copy_core/src/engine.rs` `copy_file_win32` |
| `COPY_FILE_NO_BUFFERING` from 256 MiB | Confirmed. The README's "Large Files (>=1MB) (Unbuffered I/O)" is the large-file queue's limit, not the flag's | `engine.rs` |
| Workers by device: 16 for an SSD or NVMe and for an unknown device, 2 for a USB bulk-only device, 1 for a hard disk; large files on at most 2 | Confirmed | `better_copy_core/src/profiler.rs` `default_concurrency`; `engine.rs` |
| Its own copy loop was removed for `CopyFileExW`: "Remove copy_file_direct entirely … 2.2x speedup" | Confirmed | commit `ea7b08da` (2026-08-07); merge `a3e239bb` |
| The seek penalty is read on a volume handle opened with no access rights | Confirmed | `profiler.rs` `open_volume` |

### Ultracopier

<https://github.com/alphaonex86/Ultracopier>, head `6f80b901`, GPL-3.0. The
Windows engine is
`plugins/CopyEngine/Ultracopier-Spec/win-iocp/TransferThreadWin.cpp`.

| Fact | Verdict | Source |
| --- | --- | --- |
| Its own buffered, overlapped I/O on a completion port | Confirmed | `TransferThreadWin.cpp` |
| 8 buffers (`NUM_BUFFERS`) of 256 KB by default, up to 16 MB, set by the person | Confirmed | `TransferThreadWin.h`; `CopyEngineUltracopier-SpecVariable.h` |
| `FILE_FLAG_SEQUENTIAL_SCAN` on both handles, behind a check box that is on by default | Confirmed | `TransferThreadWin.cpp`; `CopyEngine.cpp` |
| Destination space reserved with `FileAllocationInfo`, for a new file of 1 MiB or more, "so an interrupted copy stays short", rather than setting the end of file | Confirmed | `TransferThreadWin.cpp`; `pipeline/TransferThreadPipelined.h` |

### TeraCopy

Closed source. Official pages: <https://www.codesector.com/teracopy>,
<https://support.codesector.com/en/articles/8789942-copying-and-verifying-files>,
<https://blog.codesector.com/2023/12/03/teracopy-3-17/>.

| Fact | Verdict |
| --- | --- |
| Copy method | Not found |
| Buffer up to 256 MB (3.17) | Confirmed |
| Verify: checksums computed while copying, then the destination is read back and compared; 17 algorithms, xxHash3 and BLAKE3 among them | Confirmed |
| Threads: up to 2 in the free version, more in Pro | Confirmed |

### Robocopy and File Explorer

Closed source and not read. Robocopy's switches (`/j`, `/sparse`, `/nooffload`,
`/noclone`, `/compress`) name features of the Windows copy engine, so it uses
`CopyFile2` or `CopyFileEx`. Every comparison above against robocopy is a
comparison against the Windows copy engine.

## Deleters

### rmbrr

<https://github.com/mtopolski/rmbrr>, head `8e1808d1`, MIT or Apache-2.0,
Rust. Its library depends on `clap` and `windows` 0.58.

| Fact | Verdict | Source |
| --- | --- | --- |
| Each file: `SetFileInformationByHandle(FileDispositionInfoEx)` with delete, POSIX semantics and "ignore read-only" | Confirmed | `src/winapi.rs` `posix_delete_file` |
| Each folder: the same, without "ignore read-only" | Confirmed | `winapi.rs` `posix_delete_dir` |
| Opens each entry by its full path; no fallback when POSIX semantics are refused | Confirmed | `winapi.rs` |
| Lists with `FindFirstFileExW`, `FindExInfoBasic`, no large fetch | Confirmed | `winapi.rs` |
| A dependency graph: a folder is deleted when its last child is gone; threads default to the processor count | Confirmed | `src/tree.rs`; README |
| 28,434 files in 5,122 folders (a 350 MB `node_modules`): rmbrr 1,780 ms, rimraf 3,175 ms, PowerShell 6,824 ms, `rmdir` 6,422 ms, `del` and `rmdir` 7,175 ms, `robocopy /MIR` 9,528 ms. No hardware given | Confirmed | README |

### FastDelete (shaggie76)

<https://github.com/shaggie76/FastDelete>, head `3c680edd`. No licence.

| Fact | Verdict | Source |
| --- | --- | --- |
| One thread per logical processor (`dwNumberOfProcessors`) | Confirmed in the code | `FastDelete.cpp` |
| One folder per work item; it deletes that folder's files and queues its subfolders | Confirmed | `FastDelete.cpp` |
| `SetFileAttributes` to clear read-only, then `DeleteFile`; `FindFirstFileEx`; `RemoveDirectory` | Confirmed | `FastDelete.cpp` |
| 1 million files: 32 s against `rd /s /q`'s 84 s (i7-9750H laptop); 39 s against 283 s (Ryzen 9 3950X) | Confirmed. Corrected: the README names the processors, not "6 cores" and "16 cores" | README |
| Read-only files on the laptop: 149 s against 65 s | Confirmed | README |

### rimraf

<https://github.com/isaacs/rimraf>, head `051e6c72`, BlueOak-1.0.0.

| Fact | Verdict | Source |
| --- | --- | --- |
| Retries `EBUSY`, `EMFILE` and `ENFILE`; each wait 1.2 times longer (`RATE`), starting at 1 ms | Confirmed | `src/retry-busy.ts` |
| The 200 ms (`MAXBACKOFF`) is a total of the waits, not a wall-clock limit; then up to 10 immediate retries (`MAXRETRIES`) | Corrected | `retry-busy.ts`; README |
| `EPERM`: makes the file writable (`chmod 0o666`) and tries once more | Corrected: not a retry loop | `src/fix-eperm.ts` |
| `ENOTEMPTY` on a folder: moves it away, then removes it | Confirmed | `src/rimraf-windows.ts` |

### rmx

<https://github.com/zerx-lab/rmx>, head `a9b862ec`, MIT, Rust.

| Fact | Verdict | Source |
| --- | --- | --- |
| `CreateFileW` sharing read, write and delete; `FileDispositionInfoEx` with delete, POSIX semantics, "ignore read-only" and `FORCE_IMAGE_SECTION_CHECK`; `\\?\` paths; `FindFirstFileExW` with large fetch; threads = processors | Confirmed | `src/winapi.rs` |
| Retries: the README says 10 with backoff from 10 to 100 ms | Corrected: the code retries 4 times (waits 0, 1, 5, 10 ms) on errors 5, 32, 33 and 145; for 145 (folder not empty) up to 5 more rounds of 1, 10, 50, 100 and 200 ms | `winapi.rs` |
| 476 MB, 51,705 items, mean of 3 runs: rmx 4,120 ms, `rmdir` 7,496 ms, PowerShell 7,623 ms, `rm -rf` 8,394 ms. No hardware given | Confirmed | README |

### remove_dir_all and fs_at

<https://github.com/XAMPPRocky/remove_dir_all>, head `386e1f56`, v1.0.0, MIT
or Apache-2.0. Its Windows deletion is in <https://github.com/rbtcollins/fs_at>
(head `8443962e`, Apache-2.0).

| Fact | Verdict | Source |
| --- | --- | --- |
| A `parallel` feature with rayon, off by default; the crate says it helps on Windows and network file systems | Confirmed | `Cargo.toml`; `src/lib.rs` |
| Opens each child relative to its parent folder's handle (`NtCreateFile` with `RootDirectory`), deletes with `FileDispositionInfoEx` (POSIX semantics, "ignore read-only"), falls back to `FileDispositionInfo` | Confirmed | `fs_at/src/win.rs` `delete_with_posix` |

### Rust's standard library

<https://github.com/rust-lang/rust>, master at `bd4c6cc4`:
`library/std/src/sys/fs/windows.rs` and
`library/std/src/sys/fs/windows/remove_dir_all.rs`.

| Fact | Verdict | Source |
| --- | --- | --- |
| `remove_dir_all` deletes with POSIX semantics and "ignore read-only", and falls back to `FileDispositionInfo` on `ERROR_INVALID_PARAMETER`, `ERROR_NOT_SUPPORTED` or `ERROR_INVALID_FUNCTION` | Confirmed | `windows.rs` `File::delete`, `File::posix_delete` |
| It opens each entry relative to its parent folder (`NtOpenFile` with `RootDirectory` and `FILE_OPEN_REPARSE_POINT`), the fix for CVE-2022-21658, in which a folder swapped for a link during the walk made `remove_dir_all` delete outside its tree | Confirmed; corrected from `NtCreateFile` to `NtOpenFile` | `remove_dir_all.rs` module doc, `nt_open_file` |
| It retries a sharing violation and a folder that is not empty up to 50 times, yielding the thread between tries | Confirmed | `remove_dir_all.rs` |
| `remove_file` is `DeleteFileW`; only on access denied does it reopen with delete access and use POSIX semantics | Corrected | `windows.rs` `unlink` |

### Found, not used

| Tool | What it is | Why it is not used |
| --- | --- | --- |
| <https://github.com/berkkarabacak/FastDelete> | A one-pass `FindFirstFileEx` walker, `NtSetInformationFile(FileDispositionInformationEx)` with a `DeleteFile` fallback, a Recycle Bin option through `SHFileOperation` in chunks. README: about 5,000 items/s against about 1,000 for File Explorer; 10,100 tiny files in 2.2 s against PowerShell's 5.6 s | No licence found, no hardware given, nothing that rmbrr and FastDelete do not already show |
| <https://github.com/eoffermann/b2cFastCopy> | Unbuffered overlapped copy with a preallocated memory arena, benchmarked against `robocopy /E /J /MT:32` | Numbers not checked; the same design family as FastCopy's ring |

## Windows APIs

### Copy APIs

| Fact | Verdict | Source |
| --- | --- | --- |
| `COPY_FILE_NO_BUFFERING`: "recommended for very large file copies"; do not pause such a copy | Confirmed | <https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-copyfile2_extended_parameters>; The Old New Thing, 2026-08-10, <https://devblogs.microsoft.com/oldnewthing/20260810-00/?p=112600> |
| `COPY_FILE_ENABLE_SPARSE_COPY`: Windows 11 22H2 and later | Confirmed | same page |
| `COPY_FILE_REQUEST_COMPRESSED_TRAFFIC`: Windows 10 1903; only SMB 3.1.1 shares on Windows 10; ignored where not supported | Confirmed | same page |
| `ioDesiredSize` exists only in `COPYFILE2_EXTENDED_PARAMETERS_V2`: "may be reduced if insufficient memory is available … may be ignored if ioDesiredRate is also provided" | Confirmed. The page's minimum versions (client Windows 11, server 2012) contradict each other | <https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-copyfile2_extended_parameters_v2> |
| `COPYFILE2_PROGRESS_CANCEL` fails the copy with `ERROR_REQUEST_ABORTED` and deletes "any partially copied fragments"; `COPYFILE2_PROGRESS_STOP` leaves them | Confirmed | <https://learn.microsoft.com/en-us/windows/win32/api/winbase/nc-winbase-pcopyfile2_progress_routine> |
| ODX: from Windows 8 the copy engine tries an offloaded transfer first in `CopyFile`, `CopyFileEx`, `MoveFile`, `MoveFileEx` and `CopyFile2`; NTFS only; not for compressed, sparse, EFS, BitLocker, deduplicated or resident files; after a failure the engine treats the volume as unable to offload for the rest of the process | Confirmed | <https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/offloaded-data-transfers> |
| `CopyFile2` tries SMB offload by itself | Corrected (found in round 2): `COPY_FILE_ENABLE_LOW_FREE_SPACE_MODE` (build 19041): "No overlapped I/Os are used. ODX and SMB offload are not attempted", so the normal mode attempts both and uses overlapped I/O. Which SMB mechanism (`FSCTL_SRV_COPYCHUNK`) is not named. AngelCOPY's 0.2 s between shares agrees | <https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-copyfile2_extended_parameters>; <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/eaa129d1-fca4-46b1-8ccd-b91f3e1d2d80> |
| `COPY_FILE_OPEN_AND_COPY_REPARSE_POINT` (build 19041): "Always copy the reparse point regardless of type"; `COPY_FILE_COPY_SYMLINK` covers symbolic links only; `COPY_FILE_DIRECTORY` (build 19041) copies a folder's streams, reparse data and EAs | Confirmed | same page |
| `FSCTL_DUPLICATE_EXTENTS_TO_FILE` clones a range of a file into another file on the same ReFS volume; the ranges are cluster-aligned and both files must have the same integrity setting | Confirmed. Corrected: the page gives Windows Server 2016 as the minimum server and "None supported" as the minimum client, though ReFS volumes and Dev Drives on Windows 11 accept it | <https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_duplicate_extents_to_file> |
| `FILE_SUPPORTS_BLOCK_REFCOUNTING` (0x08000000) in a volume's flags: "Indicates that FSCTL_DUPLICATE_EXTENTS_TO_FILE is a supported operation". `GetVolumeInformationByHandleW` is not supported over SMB | Confirmed | <https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationbyhandlew> |
| A cloned region "must be less than 4GB in length"; the destination region must not extend past the end of file, so the end of file is set first; a sparse source needs a sparse destination | Confirmed (found by the challenger review) | <https://learn.microsoft.com/en-us/windows/win32/fileio/block-cloning> |
| Block cloning happens natively in Windows' copy operations on ReFS, within one volume; Dev Drive copies clone too. No page names `CopyFile2` | Corrected: the win32 page says from the Windows 11 Moment 5 update (KB5034848, February 2024, for 23H2); the windows-server page says Windows 11 24H2 and Windows Server 2025. The earlier date holds for clients | <https://learn.microsoft.com/en-us/windows/win32/fileio/block-cloning>; <https://learn.microsoft.com/en-us/windows-server/storage/refs/block-cloning>; <https://learn.microsoft.com/en-us/windows/dev-drive/> |

### Delete APIs

| Fact | Verdict | Source |
| --- | --- | --- |
| `FILE_DISPOSITION_FLAG_POSIX_SEMANTICS`: the name leaves when the deleting handle closes, while other handles keep their data. `FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE`: without it a read-only file returns `STATUS_CANNOT_DELETE` | Confirmed | <https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddk/ns-ntddk-_file_disposition_information_ex>; <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/2e860264-018a-47b3-8555-565a13b35a45> |
| POSIX semantics from Windows 10 1607; "ignore read-only" from Windows 10 1809 | Not found in Microsoft's pages. Rust's std says 1607 for POSIX semantics; Microsoft's STL says 1809 for "ignore read-only" | Rust `windows.rs`; <https://github.com/microsoft/STL/blob/main/stl/src/filesystem.cpp> |
| File systems: NTFS supports POSIX semantics; FAT32 does not (Rust's std); ReFS refused it with `ERROR_NOT_SUPPORTED` in January 2020 | Not found in Microsoft's pages; ReFS from Microsoft's STL PR #407, which added the fallback and tested ReFS by hand | <https://github.com/microsoft/STL/pull/407> |
| `IFileOperation` with `FOFX_RECYCLEONDELETE` (Windows 8) sends deleted items to the Recycle Bin | Confirmed | <https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperation-setoperationflags> |
| `GetAnyOperationsAborted` must be called after `PerformOperations` whatever it returned: "A success code can be returned even if the operation was stopped by the user or the system" | Confirmed | <https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperation-getanyoperationsaborted> |
| `FOF_NOCONFIRMATION` answers "Yes to all" to every question, including "this item cannot be recycled, delete it permanently?"; `FOF_WANTNUKEWARNING` makes that one question appear anyway | From the window session's and the plan reviewer's reviews; not checked, because the Recycle Bin is not used | `SetOperationFlags` page above |
| A classic `FileDispositionInfo` delete refuses a read-only file | Confirmed in TinyTorrent2's `Payload.cpp` (`Remove` clears the attribute first, "which FAT volumes use") | TinyTorrent2 `engine/src/Payload.cpp` |

### Listing APIs

| Fact | Verdict | Source |
| --- | --- | --- |
| `FindExInfoBasic` skips the short name, "improving overall enumeration speed" | Confirmed | <https://learn.microsoft.com/en-us/windows/win32/api/minwinbase/ne-minwinbase-findex_info_levels> |
| `FIND_FIRST_EX_LARGE_FETCH` uses a larger buffer per directory query | Confirmed | <https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-findfirstfileexw> |
| `GetFileInformationByHandleEx` with `FileIdExtdDirectoryInfo` gives name, 128-bit file ID, attributes, size, dates and reparse tag in one call; gozler already uses it on NTFS and ReFS | Confirmed in gozler (`gozler_winfs::Folder::read_listing`, `gozler_core::layout::listing`) | <https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_extd_dir_info> |
| `FileIdExtdDirectoryInfo` on FAT, exFAT and SMB shares | Not found. The fastfat driver sample's `FatQueryDirectory` does not handle it | <https://github.com/microsoft/Windows-driver-samples/tree/main/filesys/fastfat> |
| `FileFullDirectoryInfo` gives name, attributes, size and dates, and for a reparse point the tag in `EaSize`: "If FILE_ATTRIBUTE_REPARSE_POINT is set in the FileAttributes field, this field MUST contain a reparse tag"; the fastfat sample handles it | Confirmed (found by the challenger review). Whether every SMB server answers it is not checked | MS-FSCC, FileFullDirectoryInformation; fastfat sample above |

## What the research leaves open

- **No source compares an own ring with `CopyFile2` and
  `COPY_FILE_NO_BUFFERING`.** AngelCOPY measured its ring against robocopy,
  and its benchmark ran `CopyFileEx` with that flag without publishing the
  result. BetterCopy measured the opposite direction: its own loop was 2.2×
  slower than `CopyFileExW` for small files.
- **No source measured a hard disk.** FastCopy's one-disk mode rests on its
  design reasoning. The seek arithmetic: a 1 MB cycle at 150 MB/s transfers
  for about 7 ms, then seeks twice for about 20 ms, so about 25 % of the
  time copies data; a 64 MB cycle copies about 95 % of the time. 5,000 files
  of 200 KB on one disk cost about 100 s of seeks for 7 s of data. The
  Windows cache and the disk's command queue hide part of that, by an amount
  nobody has measured.
- **The no-buffering limit:** AngelCOPY uses 32 MiB, BetterCopy 256 MiB.
- **The delete worker count:** rmbrr, FastDelete and rmx use one per
  processor; AngelCOPY measured 16 no better than 8 on 10,000 files.
- **`ioDesiredSize`:** Microsoft says it may be reduced or ignored, and
  nothing has measured it.
- **All authors' numbers** are their own, mostly without hardware. Stage D
  of the plan measures on the owner's machine.

## Lessons from TinyTorrent2

### From its removal redesign

`docs/plans/14-removed-torrents-plan.md` (SynoSoftware/TinyTorrent2#540).
Each row names where the engine spec applies it.

| Lesson | Applied in |
| --- | --- |
| An item being deleted leaves the visible list at once. A "deleted" flag on the item made 44 readers remember to skip it, and one forgot. | File manager spec, "The person never waits for the disk" |
| Never delete a file that something may still write; the wait belongs inside the engine, not with the person | Engine spec, D14 |
| Make the file list when the deletion runs: a design that copied names at the request was rejected, because renames still running changed them | Engine spec, D11 |
| Protecting a path is not permission to delete it | Engine spec, D13 |
| Narrow each wait to files that overlap | Engine spec, D14 |
| One payload worker ordered all file work there; parallel workers must order work on one path themselves | Engine spec, D14 |
| A handle opened without `FILE_SHARE_DELETE` makes a delete of the same file fail | Engine spec, "Handles" |
| Its own copy cannot be cancelled from outside, so a delete during a long copy waits for the whole copy | Engine spec, "The interface", Cancel |
| A move on one drive is a rename; across drives it is a copy and a delete, testable only with a second drive | Engine spec, "Move"; plan, "After install" |
| An interrupted move needs a marker saved before any file moves | Engine spec, D19 |
| The Recycle Bin: `FOFX_RECYCLEONDELETE`; a missing file counts as deleted; `GetAnyOperationsAborted`, because a cancelled operation returns no error | Not used: owner ruling, engine spec D6 |
| Compare paths by identity, not text | Engine spec, D12 |
| The UI never waits for disk work | Owner decision in both specs |

### From `Payload.cpp`

TinyTorrent2's `engine/src/Payload.cpp`, as of commit `b033d3d`:

| Lesson | Applied in |
| --- | --- |
| Paths longer than `MAX_PATH` need `\\?\` (`\\?\UNC\` for a share); a volume root goes without it, because not every volume query accepts it | Engine spec, "The Windows engine" |
| `GetVersionEx` reports the manifest's build; `RtlGetVersion` reports the running one | Engine spec, "The Windows engine" |
| A sparse file that stays sparse needs its allocated size in free space (`GetCompressedFileSizeW`), any other file its full size | Engine spec, "The Windows engine" |
| `CopyFile2`'s `uliTotalBytesTransferred` also counts alternate data streams; progress is limited to the file's size | Engine spec, "The Windows engine" |
| A destination that holds the name is refused as `ERROR_FILE_EXISTS`, `ERROR_ALREADY_EXISTS`, or `ERROR_ACCESS_DENIED` when it is hidden or read-only, and belongs to someone else | Engine spec, "Conflicts" |
| Workers creating one folder at the same time fail; create the folders first | Engine spec, "Copy" |
| FAT keeps times to 2 seconds in local time, so a copy's time differs by up to 2 seconds, or by an hour after a daylight saving change | Engine spec, Continue |
