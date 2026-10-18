# How to index and track a ReFS volume

Read on 2026-10-18 for gozler's ReFS indexing, which `docs/DESIGN.md`
("Index coherence") now describes. The sources are Microsoft Learn
(Win32, driver and open-spec pages) and the voidtools site and forum. Nothing
was run against a volume.

**How to read this file.** "Documented" means a primary page states it.
"Not documented" means the pages read are silent or contradict each other. The
elevated probe has to measure those points. A label such as *secondary* marks a
source that is not Microsoft's own.

**Reading limits.** The pages were read through a tool that summarises some of
them. Where a quote is marked as paraphrase, the page was not available word
for word.

## Summary

| # | Question | Answer |
|---|---|---|
| 1 | What does ReFS support? | The Win32 pages for `FSCTL_ENUM_USN_DATA`, `FSCTL_READ_USN_JOURNAL`, `FSCTL_QUERY_USN_JOURNAL` and `FSCTL_CREATE_USN_JOURNAL` say "NTFS" and have no ReFS row. Other Microsoft pages say ReFS has a USN journal, and `MFT_ENUM_DATA_V1` is described as the ReFS input form. `FSCTL_QUERY_FILE_LAYOUT` is documented as not supported. `OpenFileById` with `ExtendedFileIdType`, `FileIdInfo` and `FileIdExtdDirectoryInfo` are documented as supported. |
| 2 | Enumeration on ReFS | The `MFT_ENUM_DATA_V1` page says it is for ReFS. With `MaxMajorVersion` 3 the output holds V2 or V3 records. Its cursor is still 64 bits. How a 64-bit cursor walks 128-bit IDs is not documented. |
| 3 | `FSCTL_QUERY_FILE_LAYOUT` | Documented as not supported by ReFS. |
| 4 | Fastest size and date pass | No page names a fastest way or gives timings. One directory query that returns `FILE_ID_EXTD_DIR_INFORMATION` gives name, 128-bit ID, size and dates together. The USN record carries no size and no file date. |
| 5 | Everything and ReFS | Native ReFS support. It reads the USN journal for changes. The first index is slow "as there's no Master File Table". No timings or method are published. |
| 6 | `MaxMajorVersion` 3 on NTFS | Not documented for the journal and enumeration. The pages say "V2 or V3". They say that range tracking makes NTFS produce V3 only. |
| 7 | Other ReFS rules | The 64-bit file ID can be -1. The documented ID layout disagrees with the measurement on V:. ID reuse is not documented. Hard-link support depends on the ReFS version. |

## 1. What ReFS supports

### Table

"Since" is the minimum Windows version that the page names.

| Item | ReFS support, as documented | Since | Source |
|---|---|---|---|
| `FSCTL_ENUM_USN_DATA` | The Win32 page says the volume "must be NTFS" and has no ReFS row. It lists CsvFS only. The `MFT_ENUM_DATA_V1` pages call V1 the input for ReFS. | Server 2012 for V1 | [Win32 page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data), [V1 page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-mft_enum_data_v1) |
| `MFT_ENUM_DATA_V0` | No ReFS statement. | Windows XP | [page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-mft_enum_data_v0) |
| `MFT_ENUM_DATA_V1` | Yes: "for ReFS volumes". Fields: start number, `LowUsn`, `HighUsn`, `MinMajorVersion`, `MaxMajorVersion`. | Server 2012 (Win32 page says no client). The WDK page says Windows XP, which cannot be right. | [Win32](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-mft_enum_data_v1), [WDK](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-mft_enum_data_v1) |
| `FSCTL_READ_USN_JOURNAL` | The page says the volume "must be NTFS". No ReFS row. CsvFS "See comment". | Windows XP | [page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_read_usn_journal) |
| `READ_USN_JOURNAL_DATA_V0` | No ReFS statement. | Windows XP | [page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-read_usn_journal_data_v0) |
| `READ_USN_JOURNAL_DATA_V1` | The page says V1 was introduced "to support 128-bit file identifiers used by ReFS". On the live Win32 page the `MinMajorVersion` and `MaxMajorVersion` entries are empty. The archived page says they are the versions "supported by the calling application". | Windows 8, Server 2012 (archived page) | [live](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-read_usn_journal_data_v1), [archived](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/legacy/hh802706(v=vs.85)) |
| `USN_RECORD_V2` | 64-bit `FileReferenceNumber` and parent. No ReFS statement. | Windows XP | [page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2) |
| `USN_RECORD_V3` | 128-bit `FILE_ID_128` for the file and the parent. The only ReFS mention is the integrity-stream reason flag. | Windows 8, Server 2012 | [page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v3) |
| `USN_RECORD_V4` | Output only when range tracking is on and the file is at least the size threshold. It is always followed by a V3 record. No ReFS statement. | Windows 8.1, Server 2012 R2 | [page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v4) |
| `FSCTL_QUERY_FILE_LAYOUT` | "This code is not supported by ReFS." | Windows 8.1 Update (NTFS) | [IFS page](https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/fsctl-query-file-layout) |
| `FSCTL_CREATE_USN_JOURNAL` | The page says "NTFS 3.0 or later". No ReFS row. | Windows XP | [page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_create_usn_journal) |
| `FSCTL_QUERY_USN_JOURNAL` | The page says the volume "must be formatted with the NTFS filesystem". No ReFS row. `USN_JOURNAL_DATA_V1` adds `MinSupportedMajorVersion` and `MaxSupportedMajorVersion`. `USN_JOURNAL_DATA_V2` adds `Flags` (range tracking). | V2 structure: Windows 8.1 | [page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_query_usn_journal), [V1](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_journal_data_v1), [V2](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_journal_data_v2) |
| `OpenFileById` | ReFS row: "Yes". | Windows 8, Server 2012 (table); function since Vista | [page](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-openfilebyid) |
| `ExtendedFileIdType` in `FILE_ID_DESCRIPTOR` | `ExtendedFileId` is a `FILE_ID_128` and "is used on ReFS file systems". | Windows 8, Server 2012 | [page](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_descriptor) |
| `GetFileInformationByHandleEx` | ReFS row: "Yes" for the function. This covers `FileIdInfo` and `FileIdExtdDirectoryInfo`. | Windows 8, Server 2012 (table) | [page](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getfileinformationbyhandleex) |
| `FileIdInfo` / `FILE_ID_INFO` | The MS-FSCC product notes say `FileIdInformation` is supported on NTFS and ReFS. The structure holds a volume serial number and a `FILE_ID_128`. | Windows 8, Server 2012 | [MS-FSCC Appendix B](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/d4bc551b-7aaf-4b4f-ba0e-3a75e7c528f0) (note 94), [FILE_ID_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info) |
| `FileIdExtdDirectoryInfo` / `FILE_ID_EXTD_DIR_INFO` | Covered by the function's ReFS row. The WDK structure page names Windows 8.1. | Server 2012 (Win32 page), Windows 8.1 (WDK page) | [Win32](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_extd_dir_info), [WDK](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-file_id_extd_dir_information) |
| `FindFirstFileNameW` | ReFS row: "Yes". | Windows 8, Server 2012 (table) | [page](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-findfirstfilenamew) |

### The USN journal exists on ReFS

Other Microsoft pages state this in general terms. They do not name the
individual FSCTLs.

- The ReFS overview lists "USN journal", "File IDs" and "Hard links" as
  available on both ReFS and NTFS.
  [ReFS overview](https://learn.microsoft.com/en-us/windows-server/storage/refs/refs-overview)
- MS-FSA says "The USN journal is supported on ReFS all versions", and that it
  is active by default on Windows Vista and later, not on servers.
  [MS-FSA Appendix A](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/4e3695bd-7574-4f24-a223-b4679c065b63)
  (note 9)
- MS-FSCC says ReFS "supports reparse points and the USN change journal".
  [MS-FSCC Appendix B](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/d4bc551b-7aaf-4b4f-ba0e-3a75e7c528f0)
  (note 14)

### Not documented

- The Win32 pages for the enumeration, journal read, query and create FSCTLs
  say "NTFS" and show no ReFS row, but the `MFT_ENUM_DATA_V1` page and the
  general pages say ReFS has the journal. The pages disagree. They do not say
  which FSCTL accepts which input form on ReFS.
- Whether `MFT_ENUM_DATA_V0` or `READ_USN_JOURNAL_DATA_V0` is accepted on
  ReFS. The V1 pages say V1 is for ReFS. No page says that V0 fails.
- Whether `FSCTL_CREATE_USN_JOURNAL` can create or grow a ReFS journal. The
  Win32 page says NTFS only. Everything's options page offers "Enable USN
  Journal" and a maximum size for ReFS volumes (see question 5, *secondary*
  evidence for the vendor's use of the call).
- Whether `FSCTL_QUERY_USN_JOURNAL` returns `USN_JOURNAL_DATA_V2` on ReFS and
  whether range tracking exists there.

## 2. Enumeration on ReFS

**Measured, 2026-10-18, Windows build 26200:** on the ReFS volume V:,
`FSCTL_ENUM_USN_DATA` fails with error 1 (`ERROR_INVALID_FUNCTION`) for
`MFT_ENUM_DATA_V0` and for `MFT_ENUM_DATA_V1` with record versions 2 to 3 or
3 only, despite the documentation below. So a ReFS index is built by
walking its folders.

### Documented

- The `MFT_ENUM_DATA_V1` page describes the structure as the "starting place
  of an enumeration" of USN records "for ReFS volumes". It is the input of
  `FSCTL_ENUM_USN_DATA`. The WDK page for the same structure says the same.
  [Win32](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-mft_enum_data_v1),
  [WDK](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-mft_enum_data_v1)
- `MaxMajorVersion` 2: the output holds `USN_RECORD_V2`. `MaxMajorVersion` 3:
  the output holds V2 or V3 records. The V2 and V3 pages list major version 4
  too, but the V1 enumeration page does not list it.
- `StartFileReferenceNumber` is `DWORDLONG` (Win32) or `ULONGLONG` (WDK): 64
  bits. The first call must pass 0. "Each call ... retrieves the starting
  point for the subsequent call as the first entry in the output buffer."
  Later calls must pass that value. The page calls it "the ordinal position
  within the files on the current volume".
- The Win32 `FSCTL_ENUM_USN_DATA` page also says the volume "must be NTFS".
  The two pages disagree.
  [FSCTL page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data)
- The `USN_RECORD_V3` page defines the file and parent numbers as 128-bit.
  MS-FSA says ReFS sets the record length from the V3 layout (note 41), and
  NTFS from the V2 layout.
  [MS-FSA Appendix A](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/4e3695bd-7574-4f24-a223-b4679c065b63)

### Not documented

- Whether ReFS needs V1, or also accepts V0. No page says that V0 fails on
  ReFS.
- What the 8-byte cursor holds when the file IDs are 128-bit. The structure
  has only a 64-bit cursor. No page says how it maps to a 128-bit ID, or
  whether it is the low half, the high half, or an opaque position.
- Which record versions ReFS returns for `MaxMajorVersion` 2. MS-FSA suggests
  ReFS records are V3 by their length rule, but that rule is for recording a
  change, not for this FSCTL.
- Whether the enumeration returns the root folder's own record, and whether it
  returns a record for every hard link.

## 3. `FSCTL_QUERY_FILE_LAYOUT`

**Documented as not supported on ReFS.** The IFS page says: "This code is not
supported by ReFS."
[IFS page](https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/fsctl-query-file-layout)

- The page last changed in 2024 and names Windows 8.1 Update as the minimum.
- It lists `STATUS_INVALID_PARAMETER` for "The file system volume is not an
  open user volume". It names no status for a file system that lacks the code.
- Not documented: the error code that ReFS returns.
- The USN record itself carries no size and no file modification date. The
  `TimeStamp` member is the record's own time. So a ReFS build needs another
  source of size and date (question 4).

## 4. Size and date of every file on ReFS

### Documented

No Microsoft page names a fastest method for ReFS or gives timings. These
facts are documented:

| Method | What one call returns | ReFS | Source |
|---|---|---|---|
| `GetFileInformationByHandleEx` with `FileIdExtdDirectoryInfo` (0x13), then `FileIdExtdDirectoryRestartInfo` (0x14) for a restart | Per entry: name, 128-bit file ID, `EndOfFile`, `AllocationSize`, four times, attributes, reparse tag, `EaSize` | Function row: Yes | [function](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getfileinformationbyhandleex), [structure](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_extd_dir_info) |
| `NtQueryDirectoryFile` or `NtQueryDirectoryFileEx` with `FileIdExtdDirectoryInformation` (60) | The same fields | Not stated per call | [Ex](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntquerydirectoryfileex), [structure](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-file_id_extd_dir_information) |
| `FindFirstFileExW` with `FIND_FIRST_EX_LARGE_FETCH` | `WIN32_FIND_DATA`: name, size, dates, attributes. No file ID. | ReFS row: "Yes" | [page](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-findfirstfileexw) |
| `OpenFileById` per file, then a handle query | One file per call | Yes | [page](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-openfilebyid) |

- The `FIND_FIRST_EX_LARGE_FETCH` flag "can increase performance of the find
  operation". The page gives no figure.
- `NtQueryDirectoryFileEx` takes `SL_RETURN_SINGLE_ENTRY` and says that flag
  makes the query "less efficient". Without it, the buffer is packed with as
  many entries as fit. The minimum client is Windows 10 version 1709.
- A directory query returns zero in any field that the file system does not
  support (NtQueryDirectoryFile remarks).
- On ReFS and NTFS, `FileIndex` is zero. The ".." entry has file ID 0.
  [MS-FSCC Appendix B](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/d4bc551b-7aaf-4b4f-ba0e-3a75e7c528f0)
  (notes 117, 118)
- Newer classes `FileId64ExtdDirectoryInformation` and
  `FileIdAllExtdDirectoryInformation` exist on NTFS and ReFS from Windows 11
  version 23H2 and Windows Server 2022 23H2.
  [same page](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/d4bc551b-7aaf-4b4f-ba0e-3a75e7c528f0)
  (notes 90 to 93)

### Not documented

- Any speed ranking of these methods on ReFS.
- Whether a directory query with `FileIdExtdDirectoryInformation` lists each
  hard link as its own entry on ReFS. A directory query lists names, so it
  should, but no page says so.
- A buffer size that works best.

## 5. How Everything indexes ReFS

voidtools publishes no method. These points are from its author or its
documentation.

- **Native support.** void, 2024: "Everything has native support for ReFS."
  The options are under Tools, Options, ReFS. "The initial index is slow as
  there's no Master File Table." "Like NTFS, ReFS has a USN Change Journal so
  Everything will pickup all changes."
  [forum thread](https://www.voidtools.com/forum/viewtopic.php?t=15630)
- **Slow first index, earlier statement.** void, 2019: "ReFS does not have a
  Master File Table (MFT)." and "Indexing ReFS volumes is slow." He suggested
  zipping folders that hold very many subfolders. He also said that log
  messages about failing to open a directory are normal, because ReFS volumes
  "usually start at a particular FileID". He added that a finished ReFS index
  logs the number of seconds it took (paraphrase).
  [forum thread](https://www.voidtools.com/forum/viewtopic.php?t=8495)
- **Volumes.** Everything indexes all fixed ReFS volumes automatically. A
  removable ReFS volume is added by hand on the ReFS tab. It does not follow
  junctions on NTFS or ReFS. Hard links are all indexed, but a change to a hard
  link updates only the first entry (paraphrase).
  [Indexes page](https://www.voidtools.com/support/everything/indexes/)
- **The USN journal.** The options page (paraphrase) says Everything uses the
  journal to index and monitor changes, that "the USN Journal is managed by the
  ReFS driver (not Everything)", that the user can enable the journal and set
  its maximum size, that at least 32768 KB is suggested if the index rebuilds
  after a restart, and that the allocation delta is currently ignored for ReFS
  volumes.
  [Options page](https://www.voidtools.com/support/everything/options/)
- **Journal gaps.** A 2017 thread describes a ReFS index that rebuilt at every
  boot. The log showed "refs monitor out of date" and "read REFS usn journal
  failed 87" (paraphrase). A larger journal did not fix it. The fix was to run
  the Everything main application at startup. void confirmed that the main
  application, not the service, maintains the database, and that the service
  only lets the client read ReFS volume journals.
  [forum thread](https://www.voidtools.com/forum/viewtopic.php?t=6394)
- **Dev Drive.** voidtools has no Dev Drive page that I found. A 2024 post on a
  virtual-disk thread asks about Dev Drive on a VHD and gets no definite
  answer.
  [forum thread](https://www.voidtools.com/forum/viewtopic.php?t=7088)

### Not documented

- Which calls Everything uses to build the first ReFS index: a directory walk
  by name, a walk by file ID, `FSCTL_ENUM_USN_DATA`, or a mix. The
  "particular FileID" remark hints at opening directories by ID, but void does
  not say so.
- Any timing or file count for a ReFS build.
- Whether error 87 on the ReFS journal read is a wrong input structure or
  another cause. The thread does not say.

## 6. `MaxMajorVersion` 3 on NTFS

### Documented

- For the enumeration, the page says `MaxMajorVersion` 3 means the output
  holds "V2 or V3" records. It does not say which one NTFS picks.
  [MFT_ENUM_DATA_V1](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-mft_enum_data_v1)
- For the journal read, the live `READ_USN_JOURNAL_DATA_V1` page has no text
  for the version fields.
- The `USN_RECORD_V3` page says: "When range tracking is turned on, NTFS
  switches to producing only USN_RECORD_V3 records as output."
  [page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v3)
  This says the answer depends on range tracking. It does not say that NTFS
  returns V2 when range tracking is off, although it points that way.
- `USN_RECORD_V4` is output only with range tracking on and a large enough
  file, and always before a V3 record.
  [page](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v4)
- For a different FSCTL, `FSCTL_READ_FILE_USN_DATA` (one record for one open
  file), MS-FSA gives exact rules. `MaxMajorVersion` of 3 or more selects V3.
  A V3 record gets the 128-bit file ID. A V2 record gets the 64-bit file ID.
  [MS-FSA](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/dbeecd40-87d1-4992-87c3-41a72abeeb4f),
  [READ_FILE_USN_DATA](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-read_file_usn_data)
  (without an input buffer the maximum defaults to 2; "To support 128-bit file
  identifiers used by ReFS this must be 3 or higher"). These rules are for that
  one FSCTL and are not shown to apply to the journal read or the enumeration.
- `fsutil usn readjournal` defaults to `minver=2` and `maxver=4`.
  [fsutil usn](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/fsutil-usn)

### Not documented

- What NTFS returns for `MaxMajorVersion` 3 with range tracking off, for the
  enumeration and for the journal read.
- Whether a NTFS V3 record zero-extends the 64-bit number to 128 bits. The
  128-bit file ID rule for NTFS says the high 64 bits "MUST be zero".
  [MS-FSCC Appendix B](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/d4bc551b-7aaf-4b4f-ba0e-3a75e7c528f0)
  (note 11)

## 7. Other rules a ReFS indexer must handle

### The 128-bit ID layout: the docs and the measurement disagree

MS-FSCC and MS-FSA both say: the low 64 bits of a ReFS `FILE_ID_128` are "an
index uniquely identifying the file's parent directory on the volume", and the
high 64 bits are "an index uniquely identifying the file within that
directory".
[MS-FSCC Appendix B](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/d4bc551b-7aaf-4b4f-ba0e-3a75e7c528f0)
(note 11),
[MS-FSA Appendix A](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/4e3695bd-7574-4f24-a223-b4679c065b63)
(note 48).

The raw bytes on V: are the other way round. `GetFileInformationByHandleEx`
with `FileIdInfo`, read on 2026-10-18 (Windows build 26200), gave these
`FILE_ID_128` halves, each read as a little-endian `u64`:

| Item | Bytes 0..7 | Bytes 8..15 |
|---|---|---|
| NTFS root, `C:\` | `0x0005000000000005` | 0 |
| ReFS root, `V:\` | 0 | `0x600` |
| ReFS folder, `V:\temp` | 0 | `0x702` |
| ReFS file in `V:\temp` | `0x84` | `0x702` |

So on this volume bytes 8..15 hold the folder's number and bytes 0..7 the
file's number within it, which is 0 for every folder. `fsutil file
queryfileid` prints bytes 15..0, so its output agrees. The NTFS row shows the
64-bit file ID in bytes 0..7, as the NTFS rule above requires. Why the
specification's wording differs is not documented. An indexer must keep the
whole 128 bits and not split them.

### Other points

1. **The 64-bit ID can be -1.** ReFS "maps a subset of the possible 128-bit
   file ID values to a 64-bit value using a reversible algorithm"; for other
   values the 64-bit ID is -1. So every API that returns the 64-bit ID is
   unreliable on ReFS. Use the 128-bit forms.
   [MS-FSCC Appendix B](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/d4bc551b-7aaf-4b4f-ba0e-3a75e7c528f0)
   (note 10),
   [MS-FSA Appendix A](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/4e3695bd-7574-4f24-a223-b4679c065b63)
   (note 49)
2. **No file ID can mean all ones.** "For files for which a unique 128-bit file
   ID cannot be established, this field MUST be set to
   0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF, and MUST be ignored."
   [MS-FSCC 128-bit file ID](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/98860416-1caf-4c80-a9ab-8d61e1ccf5a5)
3. **ID stability.** The same page says the ID "SHOULD be unique to the volume
   and stable until the file is deleted". For ReFS, the 128-bit ID is listed as
   generated, stable and unique. The ReFS layout has no sequence number. NTFS
   has one, and the NTFS page says a "different file can have the same
   128-bit file ID ... in the past". Whether ReFS reuses an ID after a delete
   is not documented.
4. **Opening by ID on NTFS with a 128-bit ID.** Documented. The driver page for
   object IDs says NTFS "simultaneously supports opening by 128-bit ID: if the
   high 64-bits are zero, the low 64-bits are treated as a file ID; otherwise,
   the 128-bit value is treated as an object ID". So `ExtendedFileIdType` with
   a zero-extended NTFS ID should open the NTFS file. ReFS has no object IDs,
   so it reads every 128-bit value as a file ID.
   [FILE_OBJECTID_INFORMATION](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_objectid_information)
5. **No object IDs on ReFS.** The overview lists object IDs as unavailable. The
   `USN_REASON_OBJECT_ID_CHANGE` flag does not apply there.
   [ReFS overview](https://learn.microsoft.com/en-us/windows-server/storage/refs/refs-overview)
6. **No 8.3 short names.** The overview says "Short names aren't officially
   supported". MS-FSA says ReFS does not implement short names. A ReFS name
   has one form only.
7. **Hard links depend on the ReFS version, and the docs disagree.** MS-FSA
   says hard links are supported on "ReFS volumes formatted version 3.5 or
   later (Windows Server 2022 and later)" (note 1), and elsewhere that on ReFS
   "there will always be exactly one link per file or directory" (note 26).
   The ReFS overview lists hard links as available. `USN_REASON_HARD_LINK_CHANGE`
   is worded for NTFS only. The measurement on V: showed that hard links work.
   [MS-FSA Appendix A](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/4e3695bd-7574-4f24-a223-b4679c065b63),
   [ReFS overview](https://learn.microsoft.com/en-us/windows-server/storage/refs/refs-overview)
8. **A Dev Drive is a ReFS volume with filters.** It needs Windows 11 build
   10.0.22621.2338 or later. By default only antivirus filters attach to it.
   The designation is set at format time. A removable volume cannot be a Dev
   Drive. Volumes can be partitions or VHDs. ReFS uses "slightly more memory
   than NTFS".
   [Dev Drive](https://learn.microsoft.com/en-us/windows/dev-drive/)
9. **The journal is not always on.** MS-FSA says the journal is active by
   default on client Windows and not on servers (note 9). The `fsutil usn
   createjournal` page speaks only of NTFS. Everything offers an "Enable USN
   Journal" box for ReFS, which implies a ReFS volume can lack a journal.
   [fsutil usn](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/fsutil-usn),
   [Everything options](https://www.voidtools.com/support/everything/options/)
10. **Other ReFS version differences.** The pages name these: the default
    cluster size is 4 KB on Windows 10 and later, and 64 KB on Windows 8 and
    Server 2012 (MS-FSA note 2); alternate data streams over 128 KB are not
    allowed (MS-FSA note 54). No page lists a USN or file ID difference between
    ReFS versions.

## Gaps the docs leave open

These are the facts that the pages read do not give. Each needs a measurement.

- Which input form each of `FSCTL_ENUM_USN_DATA`, `FSCTL_READ_USN_JOURNAL`,
  `FSCTL_QUERY_USN_JOURNAL` and `FSCTL_CREATE_USN_JOURNAL` accepts on ReFS, and
  the error code for a refused form.
- What the enumeration cursor and the leading 8 bytes hold on ReFS, and which
  record versions come back for `MaxMajorVersion` 2 and 3.
- Whether the enumeration returns the ReFS root folder's own record.
- Whether `FSCTL_CREATE_USN_JOURNAL` can grow a ReFS journal, and whether a
  ReFS volume may have no journal.
- What NTFS returns for `MaxMajorVersion` 3 without range tracking.
- The error code of `FSCTL_QUERY_FILE_LAYOUT` on ReFS.
- Whether the specs or the measurement on V: has the ReFS file ID halves the
  right way round, as the raw `FILE_ID_128` bytes would show.
- Whether ReFS reuses a file ID after a delete.
- How a directory query lists hard links on ReFS, and its speed against the
  other methods.
