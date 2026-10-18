# Plan: index ReFS volumes

How gozler indexes fixed ReFS volumes, such as a Windows 11 Dev Drive, beside
NTFS volumes. Stage 0 measured how ReFS answers the requests gozler sends to
NTFS. Stage 1 builds a ReFS index by walking its folders, then keeps it
current with the same journal tail, index and queries as NTFS.

## The problem

gozlerd skips every volume that is not NTFS (`skip_reason` in
`bootstrap.rs`). Windows 11 creates every Dev Drive as ReFS, and developers
keep source trees and package caches there: the files they most often search
for by name. Today a search does not find them, and a `-path` search on a Dev
Drive exits 4 because the folder is on no indexed volume.

## Schedule

- **Stage 0** is done (2026-10-18).
- **Stage 1** starts when the owner schedules it.

## What we know

`docs/research/refs-indexing.md` collects what Microsoft and voidtools
document, with sources. The points that shape this plan:

- **No file layout on ReFS.** The `FSCTL_QUERY_FILE_LAYOUT` page says "This
  code is not supported by ReFS."
- **The ReFS journal input.** Microsoft describes `READ_USN_JOURNAL_DATA_V1`
  as the journal input added for ReFS's 128-bit file IDs.
- **The 64-bit ID is unreliable on ReFS.** ReFS maps only some 128-bit IDs to
  a 64-bit value and gives -1 for the others, so a V2 record or any 64-bit ID
  API can lose the file's identity.
- **A folder query gives names, IDs, sizes and dates.**
  `FileIdExtdDirectoryInfo` returns, for each name in one folder, the 128-bit
  file ID, the size, the dates and the attributes, many names per call.
- **Everything** indexes fixed ReFS volumes and follows the journal. Its
  author says the first index is slow because ReFS has no MFT, and that
  Everything does not follow junctions.

Each part of gozler that assumes NTFS:

| Part | Where | What assumes NTFS |
|---|---|---|
| Which volumes are indexed | `skip_reason`, `bootstrap.rs` | It accepts only `NTFS`. |
| The root folder | `NTFS_ROOT_FRN` and `VolumeIndex::new`, `index/volume.rs` | Every index starts with the NTFS root, record 5 with sequence number 5. An entry's full path ends when its parent chain reaches that entry, so a volume with another root ID gives no entry a full path. |
| The build | `build_index`, `bootstrap.rs` | It reads the MFT with the file layout, and falls back to the MFT enumeration. ReFS answers neither (stage 0). |
| Journal reads | `read_usn_journal`, `journal.rs` | It sends `READ_USN_JOURNAL_DATA_V0`, which asks for V2 records with 64-bit IDs. |
| Record parsing | `usn/record.rs` | `parse_v3` keeps bytes 0..7 of a 128-bit ID and drops bytes 8..15 without an error. |
| File IDs in the index | `Frn`, `types.rs`; `EntryTable::frn` and `FrnMap`, `index/store.rs`; the apply rules, `index/volume.rs` | `Frn` is 64 bits, pinned by a size assertion. `FrnMap` keys on the 48-bit MFT record number. The apply rules in `volume.rs` look entries up by that record number in six places. When the entry under a record number has another full ID, NTFS has given that record to a new file, and the apply rules reclaim the old entries; the 16-bit sequence number is the part of the ID that changes. |
| Sizes, dates and hard links after the build | `stat_file`, `file_stat.rs`; `link_paths`, `links.rs` | Both open a file with `OpenFileById` and a 64-bit `FileIdType`. |
| Query, result order, output, pipe, CLI | `gozler-core`, `gozler-cli` | Nothing. File IDs never cross the pipe; the result order compares them inside the daemon only. |
| Text that says "NTFS" | `escompat/help.rs`, `overview.rs`, `failure.rs`, `README.md`, `CONTEXT.md`, `docs/DESIGN.md`, `docs/SECURITY.md` | They name NTFS as the only filesystem. |

Both the start (`bootstrap.rs`) and a rescan (`tail.rs`) call
`ensure_journal` before `build_index`, so the journal replay covers every
change made while a build runs.

**Memory.** Widening `Frn` to 128 bits for every volume adds 8 bytes to each
entry. On the owner's C: (2,015,690 entries, 194.6 MB of index memory) that
is 16.1 MB, about 8%, paid on every NTFS volume for a filesystem most users
do not have.

## Stage 0: measure (done)

Results on V:, a fixed 64 GB ReFS volume of the owner's, and on C:, Windows
build 26200, 2026-10-18. The probe is `target\probe\refs-probe.ps1`; its
last elevated results are in `target\probe\refs-probe-results.txt`.

- **No enumeration on ReFS.** `FSCTL_ENUM_USN_DATA` fails on V: with error 1
  (`ERROR_INVALID_FUNCTION`) for `MFT_ENUM_DATA_V0` and for `MFT_ENUM_DATA_V1`
  with record versions 2 to 3 or 3 only. With no file layout either, a ReFS
  build must walk the folders.
- **The walk.** Opening each folder by its 128-bit ID and listing it with
  `FileIdExtdDirectoryInfo`, from the root, listed 250,210 entries in 46,703
  folders. With a mostly cold cache it took 29.6 s (118 s per million
  entries); warm, 3.2 s (12.7 s per million). A cold NTFS build reads about
  5 s per million entries, so a ReFS build is about 24 times slower per
  entry. Without elevation and without `SeBackupPrivilege`, one folder
  refused access.
- **File IDs use both halves.** `FileIdInfo` gives each folder its own number
  in bytes 8..15 of the `FILE_ID_128` and 0 in bytes 0..7: `0x600` for the
  root, `0x702` for `V:\temp`. A file has the number of the folder it was
  created in, in bytes 8..15, and its own number in bytes 0..7, such as
  `0x84`. `parse_v3` keeps bytes 0..7, so today it would give every ReFS
  folder the ID 0. A 64-bit `Frn` cannot hold these IDs.
- **Journal records.** `READ_USN_JOURNAL_DATA_V0` gets V2 records on V:;
  `READ_USN_JOURNAL_DATA_V1` with record versions 2 to 3, or 3 only, gets V3
  records with the full 128-bit ID. On C:, the V1 input with versions 2 to 3
  switches NTFS to V3 records, so NTFS keeps the V0 input.
- **Moves keep the ID.** A rename in place, a file moved to another folder
  and a folder moved with its children kept every ID. The journal writes a
  rename as an old-name and a new-name record with the same ID, and a move as
  a new-name record with the new parent, as on NTFS, so the journal tail
  applies them unchanged.
- **Hard links work.** A hard link has the file's ID, and
  `fsutil hardlink list`, which calls `FindFirstFileNameW`, lists both names.
- **One way to open a file by ID.** `OpenFileById` with `ExtendedFileIdType`
  opens a ReFS folder from its 128-bit ID, and opens `C:\Windows` from its
  NTFS ID zero-extended to 128 bits.
- **The journal.** V:'s journal is active, 32 MB, with record versions 2 to
  3. Whether `ensure_journal` can grow it to its 64 MB minimum was not
  tested; a grow that fails is already logged and the journal used at its
  size.

## Stage 1: index ReFS

ReFS gets its own build, because ReFS answers neither way NTFS reads the
MFT. Everything after the build is shared: one journal tail, one index and
one set of coherence rules in `docs/DESIGN.md`, because two copies of those
rules would drift apart. The volume's filesystem is one value, set when the
volume is found, and each part that differs reads it.

1. **Discovery.** `skip_reason` accepts fixed ReFS volumes. A ReFS volume
   follows the same lifecycle as an NTFS volume: it is indexed within a
   minute of appearing and leaves the index when removed.
2. **The root.** The build reads the ID of the volume's root folder and
   starts the index from it, on every volume, instead of `NTFS_ROOT_FRN`. On
   NTFS this reads the same ID as the constant, so one rule serves both
   filesystems, and no ReFS root needs to be assumed from one volume.
3. **The walk.** On ReFS, `build_index` walks the volume from the root, so
   every start and every rescan does. It opens each folder by its 128-bit ID
   with `FILE_FLAG_BACKUP_SEMANTICS` and `SeBackupPrivilege` enabled, so a
   folder's permissions do not hide its names, and lists it with
   `FileIdExtdDirectoryInfo`. Each name becomes an entry with its ID, its
   folder, its size, its date and its attributes, so the walk is also the
   size and date pass. A folder reparse point, such as a junction, is indexed
   but not entered, as Everything does. The query call lives in
   `gozler-winfs`, and the parser of its output buffer lives in
   `gozler-core`, because the buffer is untrusted input.
   - **Names the walk meets again.** The walk meets each hard link of a
     file in its own folder, and the index's upsert treats a second name for
     a known ID as a rename, so the walk inserts through its own rule. A
     name the entry already has, with the same folder, is skipped: a folder
     moved during the walk into a part not yet listed is listed twice, with
     the same children. Another name for a known file ID is added as another
     hard link. Another name for a known folder ID replaces the first,
     because a folder has one name, and a second one means it moved during
     the walk. A file moved during the walk can so keep a stale second name;
     the replayed rename record for it reaches the hard-link reconcile,
     which sets the file's names to the real set, as it does on NTFS.
   - **Folders the walk never lists.** A folder moved during the walk, from
     a part not yet listed into one already listed, is never listed. The
     journal replay adds the folder from its rename record, but not its
     unchanged children. So a rename that brings in a folder the index does
     not hold walks that folder, during the replay and in the live tail. The
     apply reports the folder in `ApplyOutcome`, as it reports
     `needs_link_reconcile`, and the tail walks it as `reconcile_links` walks
     links: with the index lock released, beating the tail's pulse for each
     folder, and applying the listing under one short lock. A walk in the
     tail that fails starts a rescan, as other sync losses do. On NTFS the
     build reads every record, so a renamed folder is already in the index
     and the rule does not fire.
   - **Failure and cancellation.** A folder that is gone when the walk opens
     it is skipped; the journal replay removes it. A walk that fails
     otherwise fails the build, as a failed MFT enumeration does today. The
     walk checks the build's stop flag between folders, as the MFT read
     checks it between pages, so a rescan or a shutdown does not wait for
     the whole walk.
   - **Time.** A ReFS volume stays `bootstrapping` for the walk: about 30 s
     for V: after a boot, by the stage 0 measurement.
4. **Journal reads.** ReFS volumes send `READ_USN_JOURNAL_DATA_V1` with
   record versions 2 to 3, which returns V3 records. NTFS volumes keep
   `READ_USN_JOURNAL_DATA_V0`, so their records do not change.
5. **Record parsing.** `parse_v3` keeps all 128 bits of each ID.
6. **File IDs in the index.** NTFS entries keep their 8-byte ID, so NTFS
   index memory does not grow; only ReFS volumes store 128-bit IDs. On ReFS,
   lookups key on the full ID, and the NTFS rule for a reused record number
   does not apply. Stage 1 starts by sketching two forms in
   `index/store.rs` and `index/volume.rs`: this one, and a simpler one with a
   128-bit `Frn` for all volumes. If the sketches show that the ReFS-only
   form makes the apply rules materially harder to follow, the choice goes to
   the owner with both sketches and the 16.1 MB cost.
7. **Opening files by ID.** `stat_file` and `link_paths` use
   `ExtendedFileIdType` on every volume, with the NTFS ID zero-extended.
8. **Text.** The help, the overview, the failure messages, `README.md`,
   `CONTEXT.md` ("Indexed volume", "File ID"), `docs/DESIGN.md` and
   `docs/SECURITY.md` name both filesystems. On ReFS too the index holds names
   that file permissions hide, so the trust model in `docs/SECURITY.md` covers
   it unchanged.

Tests, each with the failure it watches:

- **The whole ID survives parsing.** A V3 fixture with non-zero bytes 8..15
  keeps all 128 bits. It replaces `v3_nonzero_high_half_is_truncated_not_fatal`,
  which pins the old behavior. Without it, every ReFS folder gets the ID 0,
  and no error shows.
- **A malformed folder buffer returns an error.** A proptest over arbitrary
  bytes for the `FileIdExtdDirectoryInfo` parser, as `AGENTS.md` requires for
  a new parser. Without it, one malformed entry can panic the daemon or
  allocate without a cap.
- **The build converges on ReFS IDs.** `bootstrap_race_converges` in
  `index/tests.rs` gains ReFS-shaped IDs and a ReFS root as a random choice,
  so the full-ID lookups are tested through the same shuffled build and
  journal replay as the NTFS ones. Without it, the only test that carries the
  bootstrap's correctness covers NTFS alone, and index code that still
  assumes `NTFS_ROOT_FRN` fails no test. Whether the real root ID is right is
  shown by the full paths on V: (see "Done when").
- **The walk race converges.** A new proptest beside
  `bootstrap_race_converges`: the walk lists one folder at a time while the
  model's changes keep running between listings, then the journal replays
  with the rule for folders the walk never lists. The result must equal the
  final model. `bootstrap_race_converges` cannot watch this, because it
  reads one snapshot taken at a single moment, and a walk is not one.
  Without it, a folder that a package manager renames into place during a
  build can lose its children, or a folder listed twice can duplicate them,
  and no error shows.
- **The walk keeps hard links.** One fixture, because the model has no hard
  links: a file listed in two folders keeps both names, and a folder listed
  twice under the same name adds no entry. Without it, the walk's insert
  rule can drop a hard link or duplicate an entry.

**Done when:**

- `check.ps1` passes, with the tests above.
- The code review that "Expert reviews" names is done and its findings are
  resolved.
- `gozler --status` shows V: `live`. Its entry count is compared with a
  directory listing of V:, and each difference is explained, such as a
  folder that the listing could not open.
- Files and folders created, renamed, moved and deleted in a scratch folder
  on V: show in results under their new full paths within a second. The same
  holds on C:, because items 2 and 7 change code that NTFS volumes run.
- A size or date search on V:, such as `size:>1mb`, returns files without
  the "may be incomplete" warning, which shows that the walk gave sizes and
  dates.
- `gozlerd.log` shows V:'s journal grown to gozlerd's target, or the error
  that `ensure_journal` logs when it cannot grow it.
- `gozler --status` shows C:'s index memory within 1% of the figure before
  the change, for the same entry count.

## Expert reviews

**Before stage 1 lands,** a code review of the changes to `index/volume.rs`,
`index/store.rs` and the journal tail, because every query and every journal
record goes through that code, and of the walk, its insert rule, its parser
and the tail's folder walk, because they read untrusted bytes and decide
which names a ReFS index holds.

## When the plan is done

When stage 1 lands, its rules move into `docs/DESIGN.md` ("Index
coherence", "Sizes and dates", "Hard links") and `CONTEXT.md`, and this file
is deleted. The probe in `target\probe\` is deleted with it.
