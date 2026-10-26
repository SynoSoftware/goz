# Plan: show each user only what they may see

How to build what [the spec](visibility-spec.md) describes, in what order,
and how each stage proves that it is done. The design is here too. This plan
changes as the work progresses; the spec changes only when a requirement
changes.

## Schedule

**Scheduled:** started 2026-10-26. Stage 0 item 5 passed on 2026-10-18.
Item 3 passed on 2026-10-26 as test W1. Item 4 on 2026-10-26: 1.88 µs per
check, 10,000 checks over the 114 distinct descriptors of 20,000 folders
under Program Files, ProgramData, Windows and Users (`build.ps1 -Timing
may_list_cost`). Items 1 and 2 ran on 2026-10-26 (owner's script, warm
cache, 19 hours after boot): `C:` has 274,208 folders, 1,499 distinct folder
`SecurityId`s and 1,444 distinct descriptors of 327 bytes on average, so a
filtered request pays about 2.7 ms and the answers stay per request. One read
per `SecurityId` took 0.11 s against 27 s for every folder; the record's USN
equalled the layout's for 1,498 of 1,499 and no `SecurityId` gave two
descriptors, so the NTFS build shares descriptors. The only failed reads were
four folders below `$Extend` (`$RmMetadata`, `$Deleted`, `$TxfLog`, `$Txf`),
all `ERROR_ACCESS_DENIED`, so the metadata predicate stays. ReFS on `V:`
writes `SECURITY_CHANGE` for a changed folder and for a child that inherits
the change. The owner's script needs
`target\release\examples\visibility_stage0.exe` from `cargo build --release
--workspace --examples`.

**Stage 1a** is committed as `16d88c3` (2026-10-26). **Stage 1 review of
1a**: pass after one fix round. Fixed: the
descriptor table grew without bound when a user changed a folder's
permissions in a loop (now rebuilt from the column, with a new instance,
when it holds over twice what the last rebuild kept plus 1 MiB); a missed
folder's descriptor was read and counted twice; failed reads were counted
for folders the Include rules remove; a refused `READ_CONTROL` failed the
whole walk; two doc comments. Accepted risk: each rebuild gives the volume a
new instance, so a user who changes their own folder's permissions
thousands of times can make other users' running searches and result
snapshots on that volume start again; a separate descriptor generation
would add a check on every path that now checks the instance. Left for the
2a round: a new subfolder of a walked folder is read and counted twice when
its read keeps failing; one comment's wrapping.

**Execution** (the lead keeps this current; agents read it and report to the
lead):

| Piece | Owner | Files it alone edits | State |
|---|---|---|---|
| Windows layer: "The caller", "Reading descriptors" (`read_folder_descriptor`, `Folder::descriptor`, `read_folder_security`, the owner-and-group rule), the stored list's reader and writer, W1 to W3, T10, stage 0 item 4 as an ignored timing test; the layout's `security_id` and `usn` with T1; the owner's stage 0 script and its example program | implementer W (Opus) | `gozler-winfs`, `gozler-core/src/layout/` (`parse.rs`, `listing.rs`, `fixtures.rs`), the stage 0 example, `target\probe\visibility-stage0.ps1` | started |
| Stage 1a and 2a in core and daemon: descriptor IDs, the journal, status and memory, the filter, the answers, who sees what, the refusals, the client's identification level; `record_usn` with T9 first, because the stage 0 program needs it; later 1b's `read_build_descriptors` and T3 | implementer C (Opus) | `gozler-core` except `layout/parse.rs`, `gozler-daemon`, `gozler-client`, `gozler-cli`, the `VolumeMemory` literals in `gozler-fileops/src/totals.rs` and `gozler-ui/src/core/model.rs` | started |
| Stage 2a's empty Size cell (T11) and stage 3, the Visibility page with the list | implementer U (Opus) | `gozler-ui` except the `model.rs` literal | started |
| Stage 2b: `gozlerd see-every-file`, `install --see-every-file`, the setup's page; the owner's after-install script | implementer W, after C hands over `gozler-daemon/src/main.rs` and `service.rs` | those two files, `installer/Setup.iss`, `target\probe\visibility-check.ps1` | waiting |
| Stage 2c, "Text" | a Sonnet writer, after 2a and 2b settle | the documents "Text" names, `escompat/help.rs` `NOTES` | waiting |

- **Builds.** The lead alone runs `build.ps1`, when every implementer has
  reported a consistent point, because the build compiles the whole
  workspace and one half-edited file fails every other piece's check.
- **Reviews.** The stage 1 code review reads 1a when it is built, and 1b's
  change later, because 1b waits for the owner's script. The security review
  runs when 2a and 2b are built. Stage reviewers do not run the code-review
  skill; it runs once, as the final deep review.
- **Skills.** `rust-unsafe-ffi` for every `unsafe` Win32 or COM call;
  `rust-api-design` for new public types; `codebase-design` for the types and
  seams of core and daemon; `rust-win32-ui`
  (`C:\Users\user\.agents\skills\rust-win32-ui\SKILL.md`) for the window;
  `domain-modeling` for names and `CONTEXT.md`.
- **Decisions made during the work.** `pipe_client_token` takes a
  `BorrowedHandle`, as `pipe_client_user` does. The owner-and-group rule is
  `check_descriptor`, built on `RtlValidRelativeSecurityDescriptor`, which
  also checks that every part lies inside the bytes; `may_list` calls it
  too. `Listed::Page` owns its `Vec<LayoutFile>`. One owned `SeenBy {
  Everything, Caller(Listable) }` is passed by reference, and the searches
  take a `Viewer` (everything, or the `may_list` closure). A refused caller
  also gets no `ResultSnapshotRows` and no `SortResultSnapshot`. The list
  has its own Settings page, Visibility (service settings plan).
- **Stage 1 review of 1b** (2026-10-26): pass, no findings. Departures
  accepted: `std::collections::HashMap` for the SecurityId map, because the
  daemon does not depend on rustc-hash; `is_metadata_folder` stays private
  and the daemon asks `wants_descriptor` and `unread_folders`, so one owner
  in core decides which folders are read; `read_build_descriptors` returns
  its failures in `failed` and stops when the build is cancelled; when the
  first folder's USN differs it keeps its own new bytes, which the journal
  replay corrects if they changed; `VolumeIndex::folder_descriptor` is public
  for the daemon's T3.
- **Security review of 2a, 2b and 3** (2026-10-26): pass. No sequence
  showed a filtered user a hidden name, count or size. Fixed: a stored list
  that does not read logged a warning on every request; an empty SID from
  the picker reached `slice::from_raw_parts` with a null pointer; `serve`
  handled the refusal four times; `StoredList` was a one-caller wrapper;
  `show_list` parsed each line twice. The re-check passed those fixes and
  found one issue in 1b, fixed: on an NTFS volume without an active journal
  NTFS does not update a folder's USN, so the build shares descriptors only
  when the journal is active and otherwise reads each folder on its own. The
  documents say only that a program that took the pipe name gets no token
  for the user, because `GetNamedPipeClientSessionId` may still give it the
  session. Accepted risk: the elevated save reads
  a file the standard user can write, so a UAC approval approves the
  program, not the list it saves; the Include save works the same way, and
  the administrator still approves the prompt.
- **Found during the work.** A search panel's selection showed no size at
  all when one selected folder's size was unknown (`chosen_bytes` in
  `core/search_list.rs`), which a filtered user would see on every folder
  selection; it now shows the known sum and how many sizes are unknown,
  with stage 3.
- **Commits.** Each stage is committed on `main` after its review passes,
  in the order 1a, 1b, then 2a and 2b together, then 2c and 3. Nothing is
  pushed.

- **Stage 0** has two parts. An agent runs items 3 and 4 without elevation
  as soon as the plan leaves the queue. The owner runs one elevated script,
  for items 1 and 2, after his next reboot.
- **Stages 1a, 2a, 2b, 2c and 3** start at once. **Stage 1b**, the NTFS
  build, is the one stage that waits for the owner's script: stage 0 item 1
  decides whether it shares descriptors and how it treats the NTFS metadata
  folders. It is scheduled last in stage 1, so the wait costs nothing.
- **Landing rule.** A build that holds stage 2a also holds stages 1a, 1b and
  2b. Without 1b, no NTFS folder has a descriptor, so a filtered search finds
  nothing on `C:`; without 2b, nobody can be put on the list. So the stages
  land on `main` in the order 1a, 1b, 2a, 2b: stage 2a may be written before
  the owner's script has run, but it lands only after 1b. The owner installs
  builds that follow this rule on his own PCs as soon as they exist.
- **Release rule.** The public release that Check for updates offers carries
  stages 1 to 3 together: the release that turns the filter on for the other
  users of a shared PC also carries the page where an administrator changes
  who sees every file.

Each stage is one or more commits, each of which passes `build.ps1`
(`AGENTS.md`, "Before you finish"), with the tests that "Tests" names for it.

## What the code does now

- **NTFS gives each file's `SecurityId`, and gozler drops it.**
  `FILE_LAYOUT_INFO_ENTRY` starts with the 40-byte `BasicInformation`, then
  `OwnerId` (offset 40), `SecurityId` (offset 44, 4 bytes) and `Usn`
  (offset 48, 8 bytes); `INFO_ENTRY_LEGACY_LEN` is 56 (`layout/parse.rs`).
  `parse_file` reads only `RawInfoPrefix`, the first 24 bytes. NTFS gives one
  `SecurityId` to all files that have the same security descriptor, keeps
  each descriptor once, and never changes the bytes of an ID.
- **The journal records have a `SecurityId` and a `Usn` too** (`_security_id`
  and `_usn` in `RawV2` and `RawV3`, `usn/record.rs`), which `parse_v2` and
  `parse_v3` ignore.
- **ReFS gives none.** `FileIdExtdDirectoryInfo` has no security field. The
  ReFS build and the tail's `settle_batch` both list folders through
  `walk_folders` (`gozler-core/src/index/settle.rs`), whose callback gets
  each page of `LayoutFile`s. Its reads, `bootstrap::Disk`, open each folder
  with `open_folder` (`gozler-winfs/src/file_id.rs`): `OpenFileById` with
  backup semantics and `FILE_LIST_DIRECTORY`.
- **A build reads the whole volume, then applies the Include rules.**
  `build_index` (`gozler-daemon/src/bootstrap.rs`) runs the layout pass, or
  the ReFS walk, then `apply_rules`, which removes what Include leaves out,
  then `compute_folder_sizes`, `optimize_storage` and `rank_names`. A
  folder's path is known only after every folder is read.
- **The tail never hears of a security change.** `REASON_MASK` in
  `gozler-daemon/src/tail.rs` omits `USN_REASON_SECURITY_CHANGE`, so the
  kernel does not report a change that has only that reason. `ops_for`
  (`usn/ops.rs`) maps that reason to no operation; its doc table says so in
  row 5, and `security_only_emits_nothing` and
  `unrelated_bits_do_not_mask_matches` test it. `ops_for` returns
  `SmallVec<[UsnOp; 3]>`.
- **A new folder is already stat'd.** A record with `FILE_CREATE` yields
  `UsnOp::StatDirty` for files and folders alike, and `settle_batch` reads
  each one with the index lock released.
- **Every candidate passes one function.** `scan` in `query/engine.rs` has
  four paths: the scope subtree walk, the folded arena, the component walk
  (`visit_components`, for case-insensitive `-p` terms) and the full scan.
  Each calls `Scan::visit`, which calls `Scan::evaluate`. `evaluate` tests,
  in order: the entry clauses, the scope, the orphan check, the component
  bits and the path clauses. `find_matches` and `run_totals` use `scan`, and
  `total` is counted after `evaluate`.
- **A search reads each volume in several holds of its read lock.**
  `ResultRows::search` (`query/rows.rs`) runs `find_matches` under one hold
  of each volume's `IndexedVolume::read` (in gozlerd, `VolumeState::read`),
  orders the matches with no lock, and reads the rows with `read_rows` in a
  second hold. `ResultRows::page` reads each page in another hold
  (`Reply::fill`, which calls `find_head`). Both first holds run in
  `run_held`. `read_rows` refuses an index with another instance.
  `find_head` already leaves out a row that is gone and takes it off the
  total. `search_totals` runs in one hold. gozlerd's `stream_rows` only
  loops over `proto::encode_page`.
- **The index is compacted.** `compact_entries` moves live entries over free
  slots (`EntryTable::compact`, `compact_column`) and gives the index a new
  instance, so a running search sees `IndexChanged`. `EntryTable`'s columns
  also join `alloc`, `tombstone` and `push_eighth` growth
  (`index/store.rs`); `ReusedSlots` records the indexes of reused slots for
  running searches and holds no column. `first_child` is a column only
  folders use.
- **Six places read a folder's size**, all through `EntryView::size`:
  `Check::Size` in `Check::test`, the size key in `key_records`, the row a
  page reads in `Reply::fill`, `OrderFields::size` for `Ranked` and for the
  merge's `Live`, and `match_sums`, whose `folder_bytes` the window's status
  bar shows. `run_totals` adds file sizes only. `EntryView::size` already
  returns `None` for a folder while a volume's folder sizes are unknown, and
  every reader handles `None`. The window's Size column shows "Unknown" for
  `None`. The inspector's folder size is a Totals request scoped to the
  folder (`Ask::FolderSize`), not a row's size.
- **A `-path` scope never returns the scope folder itself**: the subtree walk
  starts at its first child, and `in_scope` is false for the scope root.
- **The client hides who it is.** `gozler-client/src/client.rs` opens the
  pipe with `SECURITY_SQOS_PRESENT` and the anonymous level. The window
  opens one connection for each request (`app/work/requests.rs`).
- **The daemon serves seven requests**: `Hello`, `Status`, `Search`,
  `SettingsSaved`, `CountIncluded`, `ResultSnapshotRows` and
  `SortResultSnapshot`. `PROTO_VERSION` is 14 and equals
  `OLDEST_PROTO_VERSION`. `handle_connection` reads a frame, then runs
  `serve(volumes, include, frame, emit)` with `spawn_blocking`.
  `CountIncluded` answers any caller through `count_included` and
  `VolumeIndex::count_kept`, which applies the request's folder choices, name
  patterns and attribute rules to every name in the index. The daemon runs
  as LocalSystem and refuses remote clients.
- **`gozlerd` has subcommands** (`Command` in `gozler-daemon/src/main.rs`,
  with clap): `scan`, `run`, `install`, `uninstall`, `status`, `protocol`.
- **Service settings live in `HKLM\SOFTWARE\Gozler`.** The value `Include`
  (`REG_SZ`, JSON; `SETTINGS_KEY` and `INCLUDE_VALUE` in
  `gozler-core/src/include.rs`) is written by an elevated copy of the window
  (`gozler-ui --save-include`), and the daemon reads it again on
  `SettingsSaved`. `gozler-winfs/src/registry.rs` reads `REG_SZ` values with
  `machine_string`. The uninstaller removes only HKCU keys, so the settings
  survive an uninstall. A new subkey of `HKLM\SOFTWARE` inherits: Users and
  `ALL APPLICATION PACKAGES` read; SYSTEM, Administrators and `CREATOR
  OWNER` full control.
- **The service log is readable by every user.**
  `%SystemRoot%\Logs\Gozler\gozlerd.log` inherits `BUILTIN\Users:(GR,GE)`.
  `init_service_tracing` fixes its level at `info`, and no line names a file
  today.
- **File operations take their totals from gozlerd.**
  `gozler-fileops/src/totals.rs` asks for Totals on each selected folder and
  treats them as exact when `!volumes_incomplete && !metadata_pending`; they
  drive progress and the free-space refusal (`check_space` in
  `prepare.rs`).
- **The setup** (`installer/Setup.iss`) adds `CleanupPage` after the license
  page when it finds an earlier install, already calls four Win32 functions
  through `external` declarations, and runs `gozlerd install` at
  `ssPostInstall`.
- **Measurements.** The owner's `C:` has 2,015,690 entries in 194.6 MB of
  index (ReFS plan measurement), so a 4-byte column adds 8.1 MB, 4.1 %. On
  2026-10-21, a warm restart built `C:` (1,983,552 entries) in 22 s and
  `V:` (255,645 entries, ReFS) in 24 s (`gozlerd.log`).

From Microsoft's documentation:

- `ImpersonateNamedPipeClient` takes the security context of the last
  message read from the pipe. It succeeds at a level below
  `SecurityImpersonation`, so an identification-level client is enough. If it
  fails, "no client requests should be executed".
- `AccessCheck` compares a security descriptor with an impersonation token
  that has `TOKEN_QUERY` access, identification level or higher. The
  descriptor must contain an owner and a group, or the call fails with
  `ERROR_INVALID_SECURITY_DESCR`.
- `SetNamedSecurityInfo` propagates inherited permissions by writing each
  child's descriptor, so each changed child gets its own journal record. A
  file's USN is the USN of its last journal record.

## Design

### Parts

| Part | Where | Owns |
|---|---|---|
| Descriptor IDs | `gozler-core/src/index/descriptors.rs` (new) | `DescriptorId`, `DescriptorTable`, `Listable`, `SeenBy` |
| The descriptor column | `EntryTable` in `index/store.rs` | `descriptor: Vec<Option<DescriptorId>>`, set for folders only |
| The filter | `Scan::evaluate` in `query/engine.rs`, `find_head` in `query/rows.rs` | which entries a request sees |
| Reading descriptors | `gozler-winfs/src/file_id.rs` | `read_folder_descriptor`, `read_folder_security`, `Folder::descriptor` |
| A record's USN | `gozler-core/src/usn/record.rs` | `record_usn` |
| The NTFS build | `gozler-daemon/src/bootstrap.rs` | `read_build_descriptors` |
| The caller | `gozler-winfs/src/caller.rs` (new) | `Sid`, `ClientToken`, `pipe_client_token`, the access check, account names |
| The stored list | `gozler-winfs/src/registry.rs`, `gozler-core/src/include.rs` | `machine_strings`, `set_machine_strings`, `SEE_EVERY_FILE_VALUE` |
| Who sees what | `gozler-daemon/src/access.rs` (new) | `Access`, `Visibility`, `sees_everything`, `stored_sids` |
| The journal | `usn/ops.rs`, `index/settle.rs`, `tail.rs` | `UsnOp::SecurityDirty`, reading changed descriptors |
| The commands | `gozler-daemon/src/main.rs`, `service.rs` | `gozlerd see-every-file`, `gozlerd install --see-every-file` |
| The setup's choice | `installer/Setup.iss` | the page that offers the signed-in user |

`gozler-core` stays OS-agnostic: it compares descriptor bytes and calls a
closure that answers "may this caller list a folder with this descriptor".
It never parses a descriptor and never calls Windows.

### Descriptor IDs

- `DescriptorId(NonZeroU32)`, so `Option<DescriptorId>` takes 4 bytes. The
  column holds `None` for a file and for a folder whose descriptor gozlerd
  has not read. One value means "no descriptor", with no sentinel.
- `DescriptorTable` holds each distinct descriptor's bytes once, in a `Vec`
  indexed by the ID, and a map from the bytes' hash to the IDs with that hash,
  so equal bytes always get the same ID. IDs are dense from 1 and only ever
  added within one index instance; compaction moves the column with the
  entries and leaves the table alone. A rescan builds a new table with a new
  instance.
- `VolumeIndex::set_folder_descriptor(&mut self, frn: Frn, descriptor:
  Option<&[u8]>)` is the one way a live entry's descriptor changes. It finds
  or adds the ID and sets it on the folder. A `frn` that is not a folder, or
  not in the index, is ignored.
- `EntryTable` owns the column like its others: it joins `alloc`, which sets
  the slot to `None` when it reuses a freed slot, as it resets `first_child`,
  `tombstone`, which sets it to `None`, `compact_column` and `push_eighth`
  growth. `ReusedSlots` needs nothing: it records slot indexes, and `alloc`
  resets the reused slot's descriptor. Without this, a folder created in a
  deleted folder's slot, or moved by compaction, gets another folder's
  descriptor, and a user who could list that folder sees every name in this
  one.
- When `rewrite_entry` turns a folder into a file, it calls
  `set_folder_descriptor(frn, None)` first, because a file has no
  descriptor.
- A descriptor that no folder uses any more stays until the next rescan.
  Volumes have few distinct descriptors (stage 0, item 1).
- The table's memory is a new `IndexMemory::descriptors` component; the
  column is part of `entries`.

### Reading descriptors

`gozler-winfs` reads a descriptor with `GetKernelObjectSecurity` on a handle
opened with `READ_CONTROL`, which backup semantics grant to gozlerd. It asks
for `OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION |
DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION`: `AccessCheck` needs
the owner and the group, and the label carries the integrity level. It does
not ask for the SACL, because the SACL does not decide listing. The length
Windows asks for is capped at 64 KiB, so a bad answer cannot make the daemon
allocate without bound. A descriptor larger than that fails the read, and so
does one without an owner or a group (`GetSecurityDescriptorOwner` and
`GetSecurityDescriptorGroup`), a rule that one function over the descriptor's
bytes owns and every reader calls: `AccessCheck` refuses such a descriptor, and
without this rule its folders would be hidden from every filtered user with
nothing in `--status`. Any other `AccessCheck` error is then not expected,
and is logged at debug level only.

Every failed read is counted, `ERROR_ACCESS_DENIED` included, and its folder
stays `None`, which hides its entries from every filtered user (spec `V9`).
gozlerd's backup privilege grants `READ_CONTROL` whatever the DACL says, so
a refusal means that something else stands in the way, such as an antivirus
filter driver, and `--status` must show it.

- Stage 1a: `pub fn read_folder_descriptor(volume: &VolumeHandle, id: u128)
  -> Result<Option<Vec<u8>>, WinError>` opens by ID with `READ_CONTROL` and
  reads the descriptor. `Ok(None)` means the folder is gone or being
  deleted, by the same rule `open_folder` uses; no caller counts it, because
  the folder's delete is in the journal. `VolumeReads` gains
  `folder_descriptor(&mut self, frn: Frn) -> Result<Option<Vec<u8>>,
  Self::Error>`, which `Disk` answers with it, for the tail on NTFS and ReFS
  alike.
- Stage 1b, only with the sharing: `pub fn read_folder_security(volume:
  &VolumeHandle, id: u128) -> Result<Option<RawFolderSecurity>, WinError>`,
  with `pub struct RawFolderSecurity { pub descriptor: Vec<u8>, pub
  usn_record: Vec<u8> }`, opens by ID once, with `READ_CONTROL |
  FILE_READ_ATTRIBUTES`, reads the descriptor, then the raw record of
  `FSCTL_READ_FILE_USN_DATA`, in that order. Only the NTFS build makes this
  call, so the tail never depends on `FSCTL_READ_FILE_USN_DATA`, which no
  test runs on ReFS. `gozler-winfs` never parses a record (its crate doc), so
  `usn/record.rs` gains `pub fn record_usn(rec: &[u8]) -> Result<i64,
  WalkError>`, which reads the version and the `Usn` field through the same
  `RawV2` and `RawV3` views as `parse_v2` and `parse_v3`, with checked
  lengths. `VolumeReads` gains `folder_security(&mut self, frn: Frn) ->
  Result<Option<FolderSecurity>, Self::Error>`, where `FolderSecurity {
  descriptor: Vec<u8>, usn: i64 }` sits in `index/settle.rs`, beside
  `FileStat`.
- Stage 1a: `open_folder` asks for `FILE_LIST_DIRECTORY | READ_CONTROL`, and
  `Folder::descriptor(&self) -> Result<Vec<u8>, WinError>` reads from the
  open handle. A folder that is open cannot be gone, so it has no `None`.

**NTFS metadata folders** (stage 1b). `$Extend` and the folders below it
hold NTFS's own files. Stage 0 item 1 lists every folder whose read fails,
and decides:

- every read succeeds: no special case; their descriptors decide;
- they fail with an error that no other folder gives: `Disk` treats that
  error as `Ok(None)`, not as a failure;
- they fail with `ERROR_ACCESS_DENIED`: `VolumeIndex::is_metadata_folder`
  (the root's child with record number 11 on a volume whose root FRN is not
  wide, and every folder below it, through `chain_up`) keeps them out of the
  reads and the count.

Any other folder that fails on a normal PC would show a `Permissions` line
on every PC, so the plan changes for it before stage 1b.

**The NTFS build** (stage 1b). Until stage 1b, an NTFS build reads no
descriptors, and every NTFS folder is `None`. `LayoutFile` gains `security_id: Option<u32>` and
`usn: Option<i64>`. `parse_file` reads them at offsets 44 and 48 of the info
entry when the available length is at least 56, through `zerocopy` like the
rest, so a short entry gives `None`, never a panic. `walk_listing_buffer`
sets both to `None`.

`layout_pass` gains a parameter `security: &mut FxHashMap<u32, Vec<(Frn,
i64)>>` and adds to it, for each `LayoutFile` that is a folder and has both
fields, the `(Frn, Usn)` under its `SecurityId`. Right after `apply_rules`,
`build_index` calls `read_build_descriptors(index: &mut VolumeIndex,
folders: FxHashMap<u32, Vec<(Frn, i64)>>, reads: &mut impl VolumeReads)`,
which skips each folder no longer in the index, so no folder that Include
removed is read. For each `SecurityId`:

1. `SecurityId` 0 means the folder keeps its own descriptor, as files from
   before NTFS 3.0 can, so each folder with it is read on its own.
2. Read the first folder of the list. If the read gives bytes and its USN
   equals the `Usn` from the pass, its permissions have not changed since the
   pass, so every folder of the list gets those bytes. Reading the
   descriptor before the USN makes this safe: a change before the USN read
   changes the USN.
3. Otherwise, read each folder of the list on its own. A folder whose own
   read fails adds 1 to `descriptor_reads_failed`; a folder that is gone
   adds nothing, because its delete is in the journal and replays.

A change after a read is in the journal, whose cursor was taken before the
build, so the replay reads that folder again. Without step 2's USN check,
one folder whose permissions changed during the build would give its new
permissions to every folder that shared its old `SecurityId`, until the next
rescan.

Then the build reads, on its own, every folder in the index that the layout
pass gave no `SecurityId`: a folder whose layout entry had no extra
information, which `parse_file` accepts, and, after a stopped layout pass,
every folder the MFT enumeration added. One rule covers both, so no folder of
a complete build stays hidden uncounted, and a folder whose read in step 3
failed is not read or counted a second time. The second path is rare and
slow.

Stage 0 item 1 decides whether the sharing earns its place: when reading
every folder on its own adds under about 1 s to `C:`'s build with a cold
cache, the build reads every folder with `folder_descriptor`, as the ReFS
walk does, and the map, `read_folder_security`, `record_usn`, the layout's
`usn` field and tests T3 and T9 go.

**The ReFS build** and **the missed-folder walk**: the callbacks of
`VolumeReads::list_folder` and of `walk_folders` take a `Listed<'_, E>`,
where `E` is the reads' error type, and `Disk` reads each folder's descriptor
from the handle it opens:

```rust
enum Listed<'a, E> {
    /// Once per folder the walk opens, before its first page. The folder
    /// comes from the walk, not from a listing, because an empty folder lists
    /// no page.
    Folder { frn: Frn, descriptor: Result<Vec<u8>, E> },
    /// Each page of that folder's listing.
    Page(&'a [LayoutFile]),
}
```

For `Folder`, the callback calls `set_folder_descriptor` with `Ok` bytes, and
for `Err` it adds 1 to a failure count and leaves the folder `None`. For
`Page` it calls `insert_listed`, as today. The ReFS build applies each event
at once; `settle_batch` collects the events in one `Vec`, in walk order, and
applies them in that order under one lock, as today, with the count through
`note_descriptor_reads_failed`. The order matters: a subfolder's entry comes
from its parent's `Page`, which the walk gives before the subfolder's
`Folder`, and `set_folder_descriptor` ignores an entry not yet in the index.
A failed read does not fail the walk. The ReFS build reads the descriptors of
folders that `apply_rules` then removes, because the open handle makes that
read nearly free.

### The journal

- `REASON_MASK` gains `USN_REASON_SECURITY_CHANGE`, and `usn/record.rs` gains
  that constant (`0x0000_0800`).
- `ops_for` gains a row after row 4: `SECURITY_CHANGE` pushes
  `UsnOp::SecurityDirty`. Row 1 still wins: a delete pushes nothing else. The
  doc table's row 5 drops "security-only". The return type becomes
  `SmallVec<[UsnOp; 4]>`, because a record can now yield four operations.
- `apply_batch` pushes the record's ID to a new
  `ApplyOutcome::needs_descriptor` when the entry is a folder and either the op
  is `SecurityDirty` or the folder's descriptor is `None`. The second
  condition covers a new folder, a folder a rename brings in, a placeholder
  that gets its real record, a file that becomes a folder, and a folder whose
  last read failed, with one rule. A file's `SecurityDirty` does nothing,
  because spec `V1` needs no file descriptors.
- For `SecurityDirty` on a folder, `apply_batch` also calls
  `set_folder_descriptor(frn, None)` at once, under the batch's write lock.
  The folder's entries are then hidden from every filtered user until the new
  descriptor is read (spec `V9`). Without this, after `icacls` denies `ben`
  a folder and 5,000 files are copied into it in the same batch, `ben` sees
  their names until the read.
- `settle_batch` gains a descriptor step, shaped like its stat step: with
  the lock released and `cancelled` asked before each folder, it reads each
  folder through `VolumeReads::folder_descriptor`, then applies the batch with
  `set_folder_descriptor` under one write. A failed read adds 1 to
  `descriptor_reads_failed` through
  `VolumeIndex::note_descriptor_reads_failed`; a folder that is gone is not
  counted. The step runs right after the apply, before the missed-folder
  walk, the stat step and the link step, which can take seconds, so a new
  folder's entries stay hidden only for the reads themselves. The order
  decides only how long entries stay hidden, never whether a name shows,
  because the folder is `None` from the apply on.
- An inherited permission change writes a record for each changed folder
  (see Microsoft's documentation above), so the tail reads each one. Owner
  check S3 changes a top folder and finds the change in a folder below it.

### The caller

`gozler-client` opens the pipe with `SECURITY_SQOS_PRESENT |
SECURITY_IDENTIFICATION` (`0x0010_0000 | 0x0001_0000`). The comment on the
constant says why: gozlerd can identify the caller but cannot act as it, and
the client still checks the pipe's owner before it sends anything.

`gozler-winfs/src/caller.rs`:

- `pub struct Sid(Box<[u8]>)`: a binary SID, compared by its bytes, with
  conversion to and from its string form.
- `pub struct ClientToken` owns an identification-level token handle with
  `TOKEN_QUERY`. It is `Send` and `Sync`: `AccessCheck` and
  `GetTokenInformation` only read the token.
- `pub fn pipe_client_token(pipe: BorrowedHandle<'_>) -> Result<Option<ClientToken>,
  WinError>` calls `ImpersonateNamedPipeClient`, then `OpenThreadToken` with
  `TOKEN_QUERY | TOKEN_DUPLICATE` and `OpenAsSelf`, then `RevertToSelf`, in
  one function that never yields. `RevertToSelf` runs on every path after a
  successful impersonation. If `RevertToSelf` fails, the function aborts the
  process, because the thread would serve the next client as this user.
  `Ok(None)` means that the client is anonymous: `OpenThreadToken` fails with
  `ERROR_CANT_OPEN_ANONYMOUS`, or the token's level is `SecurityAnonymous`.
  A client that opens the pipe with a plain `CreateFile` chooses
  impersonation, so the function then calls `DuplicateTokenEx` with
  `TOKEN_QUERY`, `SecurityIdentification` and `TokenImpersonation`, keeps
  the copy, and closes the original. A token gozlerd keeps can then never act
  as the client.
- `ClientToken::enabled_sids(&self) -> Result<Option<Vec<Sid>>, WinError>`:
  `None` for a restricted token (`IsTokenRestricted`: one with restricting
  SIDs, write-restricted included), which the list never lets see every
  file, because the program that restricted it meant it to reach less than
  its user. Otherwise the token's user first, unless its attributes have
  `SE_GROUP_USE_FOR_DENY_ONLY`, then each group whose attributes have
  `SE_GROUP_ENABLED` and not `SE_GROUP_USE_FOR_DENY_ONLY`. Such a token is
  filtered by its own `AccessCheck`, which applies its restrictions.
- `ClientToken::may_list(&self, descriptor: &[u8]) -> Result<bool, WinError>`:
  `AccessCheck` for `FILE_LIST_DIRECTORY` with the file generic mapping
  (`FILE_GENERIC_READ`, `FILE_GENERIC_WRITE`, `FILE_GENERIC_EXECUTE`,
  `FILE_ALL_ACCESS`). The check reads every token input itself: groups,
  restricted SIDs, the integrity level and the security attributes that
  conditional ACEs test, such as `WIN://SYSAPPID` for
  `C:\Program Files\WindowsApps`.
- `pub fn account_sid(name: &str) -> Result<Option<Sid>, WinError>`: a name
  that starts with `S-1-` converts with `ConvertStringSidToSidW`; `Everyone`,
  in any case, is `S-1-1-0`; any other name goes to `LookupAccountNameW`.
  Windows names well-known accounts in the system's language ("Jeder" on
  German Windows), so without the first two rules `add Everyone` would fail
  there.
- `pub fn account_name(sid: &Sid) -> Option<String>` with `LookupAccountSidW`
  (`DOMAIN\name`).

### Who sees what

**Result snapshots exist now** (DESIGN.md: Result snapshots), built before
this plan by an owner decision, and their reads do not filter rows yet. This
plan's stage that filters a page also filters each read of a result
snapshot: the read is a request, so it builds its own answers from the
caller's token, tests each row with `VolumeIndex::is_seen` as a page does,
and reports a row the caller cannot see as gone. A sort of a result
snapshot (`ResultSnapshot::sort_again`) takes the same input as a search,
because a size sort reads folder sizes: it builds the sorting caller's
`Listable` under its first hold, so the new order shows nothing hidden.

`gozler-core/src/index/descriptors.rs`:

```rust
/// One request's answers for one volume: whether the caller may list a
/// folder with each descriptor. An ID past the end was added after the
/// answers were made, and is hidden.
pub struct Listable(Vec<bool>);

/// What one request sees of one volume's index.
#[derive(Clone, Copy)]
pub enum SeenBy<'a> {
    /// Spec V3 and V16: every entry, with folder sizes.
    Everything,
    /// Spec V4: the entries in folders these answers let the caller list.
    Caller(&'a Listable),
}
```

`VolumeIndex::listable(&self, may_list: &dyn Fn(&[u8]) -> bool) ->
Listable` calls `may_list` once for every descriptor in the table.
`VolumeIndex::is_seen(&self, idx: EntryIdx, seen_by: SeenBy<'_>) -> bool` is
the one row test: true for `Everything`, for the root, and for an entry whose
parent's descriptor is listed as listable.

`gozler-daemon/src/access.rs`:

```rust
/// What the daemon decided for one request, before it runs.
pub enum Access {
    /// Spec V11: searches and Include counts are refused with this message.
    Refused(&'static str),
    Sees(Visibility),
}

pub enum Visibility {
    Everything,
    Caller(Arc<ClientToken>),
}

/// Spec V3 and V16: whether the caller's enabled SIDs, user first, hold
/// SYSTEM (`S-1-5-18`) or meet the stored list.
fn sees_everything(caller: &[Sid], list: &[Sid]) -> bool;
```

For each frame it decodes, before `spawn_blocking`, `handle_connection`
calls `pipe_client_token` and `access`:

- `Ok(None)` gives `Refused` with the anonymous message of `V11`; `Err` gives
  `Refused` with the other message of `V11`, and logs the error once per
  connection.
- A token whose `enabled_sids` hold SYSTEM or meet the stored list gives
  `Sees(Visibility::Everything)`. When reading the stored list fails, only
  the SYSTEM test counts, and every other caller is filtered by its own
  token, which is the safe answer.
- Any other token, a restricted one included, gives
  `Sees(Visibility::Caller(token))`.
- When `enabled_sids` fails, the result is `Refused` with the second `V11`
  message, logged once per connection.

Calling `pipe_client_token` once per frame, not once per connection, keeps
"the last message read" exact; it costs a few microseconds.

`serve(volumes, include, access: &Access, frame, emit)` answers `Hello`,
`Status` and `SettingsSaved` for any `Access`. `Search` and `CountIncluded`
under `Refused` get `Response::Error` with `ProtoError::Unsupported` and its
message. `CountIncluded` under `Visibility::Caller` gets the same error with
a message that says only a user who sees every file gets the count (spec
`V14`), because its folder and name rules over the whole index would let any
user count, and so name, hidden entries. The window's preview line shows
that refusal's text, as `preview_counted` in `gozler-ui/src/app/settings.rs`
already shows every refused count, so the window needs no change.

**The answers.** `ResultRows::search` and `search_totals` gain one input
for what the request sees: everything, or a `may_list` closure, which
gozlerd makes from `Visibility::Caller` by calling `ClientToken::may_list`
and mapping an error to `false`, so an error hides rather than shows. The
input is a closure, not `Visibility`, because `gozler-core` holds no token.
`run_held`, right after it takes a volume's read guard and before the scan,
builds that volume's `Listable` with it. The request keeps each volume's `Listable` to its end:
`read_rows` and every page's `find_head` use the same answers in their later
holds. They stay valid there, because IDs are only added within an instance,
`read_rows` refuses a new instance, compaction makes one, and an ID past the
end is hidden. Nothing outlives the request, so no answer can go stale and
no cache needs a key, a cap or a sweep.

The answers reach every reader of the later holds the same way the index
does: `OrderedMatches::read_rows(index, seen_by)` takes the volume's
`SeenBy` beside its index, and keeps it in `Ranked` for `order_ties`;
`Reply::fill(indexes, seen_by, page_rows, push)` takes one `SeenBy` per
volume, beside `indexes`, and passes it to `find_head`, `Live` and the
`entry` call that reads each row. `ResultRows` holds the `Listable`s for the
whole reply, beside the volumes it reads. So under a caller every folder's size is `None` in the
sort's tie runs and in the merge too: folders of equal key are ordered by
name, never by the real size the caller may not learn.

The table only grows until a rescan, so D, and with it the cost of a
filtered request, grows with uptime, by the descriptors that new explicit
permissions add, such as a package update's folders. A rescan starts it
again; no mechanism trims it.

A filtered request pays one `AccessCheck` per descriptor of each volume it
reads: D × c, where D is the number of descriptors and c the cost of one
check, about 6 ms for D = 3,000 and c = 2 µs (stage 0 items 1 and 4). The
window sends a request about 150 ms after the last keystroke, for each
scrolled chunk, for each selection change and for the selected folder's
size, so a filtered user's busiest moment is about 5 requests a second. A
request that sees everything pays nothing, so the owner pays nothing.
Computing the answers before the scan, and not when the scan first meets a
descriptor, keeps the time a search takes from depending on whether a hidden
name matches it.

**If the answers are too slow.** When stage 0 shows D × c over about 10 ms
on the owner's machine, the plan returns to cached answers: one `Listable`
per distinct token, kept across requests, answered for each new descriptor
under the tail's write lock, with an LRU cap and an expiry. Its key must hold
every token input `AccessCheck` reads (`TokenUser`, `TokenGroups`,
`TokenRestrictedSids`, the integrity level, `TokenAccessInformation` flags
and the security attributes from AuthZ except `TSA://ProcUnique`), and stage
0 then checks that AuthZ returns `WIN://SYSAPPID` for a packaged process. A
shorter key is wrong: a packaged process and the user's other processes share
the user and logon SIDs, and so do an administrator's elevated and filtered
tokens, yet each pair lists different folders. This plan's history before
the change that adds this paragraph holds that design.

**For result snapshots.** `VolumeIndex::listable`, IDs that stay
stable within an instance, `Visibility::Caller` carrying the token, and the
one row test `is_seen` are what the reads of result snapshots (DESIGN.md:
Result snapshots) build on.

### The filter

- `find_matches` and `run_totals` gain a `seen_by: SeenBy<'_>` parameter, and
  `Scan` stores it.
- `Scan::evaluate` returns `None` when `is_seen` is false, right after the
  scope test and before the orphan check, the component bits and the path
  clauses. The cheap name tests reject most candidates of a full scan first,
  so most entries never cost the read of their parent's descriptor, and no
  hidden entry pays for a path. `evaluate` is the one place every candidate
  passes, on all four scan paths, so rows, `total`, totals, every query form
  and the scope walk follow it (spec `V5`), and filtering comes before the
  sort and the limit (spec `V7`).
- `find_head` leaves out a row for which `is_seen` is false now, because a
  file can move into a folder the caller cannot list after the scan, and an
  export's pages can be read minutes later. It takes the row off the total,
  as it does for a row that is gone.
- `VolumeIndex::entry(idx)` becomes `entry(idx, seen_by: SeenBy<'_>)`.
  `EntryView::size` returns `None` for a folder under `SeenBy::Caller` (spec
  `V6`), the value it already gives while a volume's folder sizes are
  unknown, so the six readers need no change of their own, `match_sums`'
  `folder_bytes` included. The compiler makes every caller of `entry` choose.
  The callers that are not answers pass `SeenBy::Everything`:
  `query/paths.rs`, `in_scope` and `resolve_scope` in `query/engine.rs`,
  `index/volume.rs` itself, `index/tests.rs`, and `run_demo_query` in
  `gozler-daemon/src/main.rs` (`gozlerd scan`).
- The golden corpora see everything, so their rows do not change: the search
  corpus (`gozler-core`'s `escompat::corpus`) calls `ResultRows::search`
  with the "everything" input, and gozlerd's `serve` tests run with
  `Access::Sees(Visibility::Everything)`.

### Folder sizes

Spec `V6` is the owner's rule. A visible-only size for each folder would
need, for each filtered user, a sum for every folder above a hidden file
(megabytes per user on `C:`), an update of every such sum under the tail's
write lock on every size change, and a pass over all entries when such a
user first searches, while searches on that volume wait. So the index keeps
no per-user sizes. `run_totals` already adds only the files a scan passes,
so `-get-folder-size`, `-get-total-size` and the inspector's folder size give
the visible total. The window's Size column leaves a folder's cell empty
whenever its size is unknown, for a filtered user and while a size pass runs
alike, as File Explorer leaves every folder's Size cell empty; the volume's
state already says when a pass runs. Without this, every folder row of a
filtered user reads "Unknown", which reads as "still loading". `text::cell`
in `gozler-ui` gains one arm for a folder with no size, in stage 2a; a
file's unknown size still reads "Unknown".

### The stored list

"Users and groups who see every file" (spec `V16`) is the `REG_MULTI_SZ`
value `SeeEveryFile` under `HKLM\SOFTWARE\Gozler`, beside `Include`, one SID
string per line (`S-1-1-0` for Everyone). `gozler-core/src/include.rs` gains
`SEE_EVERY_FILE_VALUE` beside `SETTINGS_KEY` and `INCLUDE_VALUE`. A missing
key or value is an empty list. The key's inherited permissions let Users read
it and only administrators and SYSTEM write it, which is the access `V16`
needs. The uninstaller keeps it, as it keeps `Include`.

- `gozler-winfs/src/registry.rs` gains `machine_strings(key, name) ->
  Result<Option<Vec<String>>, WinError>`, which reads a `REG_MULTI_SZ` value
  capped at 64 KiB, and `set_machine_strings`, which creates the key when it
  is missing. The writer lives in `gozler-winfs`, not beside the window's
  `set_machine_string` (`gozler-ui/src/win/shell.rs`), because `gozlerd
  see-every-file` writes the list too and `gozler-daemon` does not depend on
  `gozler-ui`; stage 3's elevated save uses the same writer.
- `stored_sids` in `access.rs` reads the value for each frame and converts
  each line with `Sid`'s string form. A line that does not convert is
  skipped without a log line, because only a hand edit with `regedit` writes
  one, and a warning for each search would fill the log; the bare `gozlerd
  see-every-file` prints each such line, where the person who edits the list
  looks. The read costs a few microseconds, so a change applies at the next
  search with no restart and no notification code.

`gozlerd see-every-file` (`Command::SeeEveryFile` in `main.rs`):

- With no argument, it prints each entry's name, or its SID string when it
  has no name, one per line, and each line of the value that is not a SID,
  marked as ignored; for an empty list it prints `The list is empty: each
  user sees the files in folders they can open.` Either way it ends with
  the line `To show every file to every user, run as administrator: gozlerd
  see-every-file add Everyone`, so the off switch is found where the list
  is. It needs no elevation.
- `add <account>` and `remove <account>` change the list. They check
  elevation first, before they resolve the name: not elevated, they fail
  with `gozlerd: changing who sees every file needs an administrator
  terminal`, whatever the name. Then an unknown name fails with `gozlerd: no
  user or group is named <account>`. `add` of an entry already on the list,
  or `remove` of one not on it, changes nothing and says so.

`docs/WRITING.md` governs the final wording of every message.

### The setup's choice

Spec `V17`. Two parts, each with one job.

- **`gozlerd install --see-every-file <account>`**, or `--see-every-file
  none`, stores the list with that one account, or empty, but only when no
  `SeeEveryFile` value exists; otherwise it prints `Kept the existing list of
  users who see every file.` It writes the list after it creates or updates
  the service and before it starts it, so the first search already follows
  the choice. `account` resolves with `account_sid` before the service is
  touched, so an unknown name fails with the `see-every-file add` message and
  changes nothing. Without the option, `install` leaves the list as it is,
  which is how the window's "Install the service" button and a hand install
  behave. This is the one place that decides whether to write.
- **`installer/Setup.iss`** finds the signed-in user itself, with the same
  `external` declarations it already uses for `user32.dll`:
  `WTSQuerySessionInformationW` for `WTS_CURRENT_SESSION` with `WTSUserName`
  and `WTSDomainName`, each buffer copied into a `String` with `lstrlenW`
  and `lstrcpynW` and freed with `WTSFreeMemory`. The buffer is declared as
  a pointer-sized integer, and a comment says so. The session's user does
  not depend on the process's account (`WTS_INFO_CLASS`: "the name of the
  user associated with the session"), so an elevated setup that another
  administrator approved still names the standard user who started it
  (stage 0 item 5). An empty name, as in session 0, gives no page.
- In `InitializeWizard`, when `WizardSilent` is false, `RegValueExists(HKLM64,
  'SOFTWARE\Gozler', 'SeeEveryFile')` is false, and a name was found, the
  setup adds a page with `CreateInputOptionPage`: after `CleanupPage` when
  that page exists, otherwise after the license page. The two pages stay
  apart, because removing earlier programs and granting a user are unrelated
  choices, and the grant is then the last choice before Ready. The page has
  one check box, checked: `Let <name> see the names of every file on this
  PC`, and the text: `Gozler shows each user only the files in folders they
  can open. Checked, <name> sees every file name, also without running as
  administrator. This does not let them open any file they cannot open now.
  To change who sees every file later, open Settings > Visibility.`
  The texts are `[CustomMessages]` entries, and `docs/WRITING.md` governs
  their final wording.
- In `CurStepChanged(ssPostInstall)`, the `gozlerd install` call gains
  `--see-every-file "<name>"` when the page was shown and the box is checked,
  `--see-every-file none` when it was shown and cleared, and nothing when the
  page was not shown. The setup only decides whether to ask.

### Status and memory

- `IndexStats` gains `descriptor_reads_failed`, added to by
  `VolumeIndex::note_descriptor_reads_failed(n)`.
- `VolumeStatus` gains `descriptor_reads_failed: u64`, and `VolumeMemory`
  gains `descriptors: MemPair`, the descriptor table's memory. Neither has
  `#[serde(default)]`, and `PROTO_VERSION` rises by one, from 14 to 15 (spec `V12`).
  `VolumeMemory::total` and `IndexMemory::total` add the new component, so
  every reader of the total, including the window's Drives and service page,
  follows. `build_status` in `gozler-daemon/src/server.rs` fills both. The
  other struct literals are updated: the tests in `gozler-core/src/proto/
  types.rs`, `gozler-core/src/scope.rs`, `gozler-cli/src/main.rs`,
  `gozler-fileops/src/totals.rs` and `gozler-ui/src/core/model.rs`.
- `gozler --status` (`gozler-cli/src/main.rs`) adds `permissions <bytes>` to
  the second memory line, and, when the count is not 0, a line
  `Permissions   N folder permission reads failed since the last scan; such a
  folder's contents stay hidden from most users until the folder changes`.

### Logging

No log line names a file or a folder, because every user may read the
service log. A build logs, in its `index built` line, the number of
descriptor reads it made and of those that failed, not the folders. An `AccessCheck` error is logged at debug level with the descriptor
ID only.

### Text

- `escompat/help.rs`: one entry in `NOTES`, as spec `V13` gives it.
- `docs/DESIGN.md`: a new section "Visibility" after "Sizes and dates" with
  the rules of spec `V1` to `V17` and where the list is stored; "Sizes and
  dates" says that a folder has a size only for a caller who sees every
  file, and why (spec `V6`); "Paths" changes "a folder the user cannot read
  still searches, because gozlerd answers from its index" to say that it
  searches and shows what the user may see; "Results list" says that a
  folder's Size cell is empty whenever its size is unknown, and a file's
  reads "Unknown", and "Inspector pane" that a filtered user's folder size is
  the sum of the visible files (spec `V6`); "The
  Include page" says that only a user who sees every file gets the preview
  count; the "Not planned" line about filtering by permissions is removed.
- `gozler-client/src/lib.rs`: the doc comment of `folder_is_missing` says the
  same as "Paths".
- `docs/SECURITY.md`: the trust model says which names each user sees, who
  sees every name, who may change the list, why the client connects at the
  identification level, and that `CountIncluded` answers only a user who sees
  every file. `README.md`'s security paragraph says the same in one sentence.
- `CONTEXT.md` already holds **Visible entry**, **Users and groups who see
  every file** and **Filtered user**.

## Stage 0: measure

Each item names what changes when the answer is not the expected one.

1. **Descriptor count, build cost and failures** (owner's script). On `C:`,
   from `FSCTL_QUERY_FILE_LAYOUT`, count folders, distinct folder
   `SecurityId` values and distinct descriptors, and their average size. With
   a cold cache, after a reboot, time `read_folder_security`'s steps for one
   folder per `SecurityId`, and for every folder, and list every folder whose
   open or read fails, with its error. Expected: a few thousand descriptors;
   one read per `SecurityId` under 0.3 s; every folder about 6 to 15 s. The
   answers decide three things: whether the build shares descriptors ("The
   NTFS build"), how metadata folders are handled ("Reading descriptors"),
   and, with item 4, whether the answers stay per request ("Who sees what").
2. **ReFS reports a permission change** (owner's script). On `V:`, in a
   scratch folder, change a folder's permissions with `icacls` and read the
   journal with `fsutil usn readjournal`. Expected: a `SECURITY_CHANGE`
   record for the folder. Without it, the tail never learns of a permission
   change on ReFS, spec `V8` and `V15` are false there, and the ReFS case
   goes to the owner. The NTFS work does not wait for it.
3. **The check agrees with Windows** (agent, no elevation; later test W1).
   Compare `ClientToken::may_list` with a real open of the folder with
   `FILE_LIST_DIRECTORY`, made while impersonating the same token, for the
   agent's own token against a scratch folder that denies the agent's user,
   a duplicate lowered to Low integrity against a folder with a Medium
   no-read-up label, a restricted token, and `C:\Program Files\WindowsApps`
   when the agent may read its descriptor. They must agree for every case.
   A disagreement stops the plan and goes to the owner, because spec `V4`
   would be false.
4. **The check's cost** (agent, no elevation). Time 10,000 `AccessCheck`
   calls over the distinct descriptors of the folders the agent's account
   may read, with its own token duplicated at the identification level.
   Expected: about 2 µs each. With item 1's D, this decides whether the
   answers stay per request.
5. **The signed-in user when another account runs the setup.** Passed on
   2026-10-18: `target\probe\setup-gozlertest.ps1` ran
   `target\probe\signed-in-user.ps1` as `gozlertest` in the owner's session
   1, and `signed-in user` named the owner, so the answer follows the
   session, not the process account.

The owner's script is `target\probe\visibility-stage0.ps1`. It runs
elevated, with one prompt, after a reboot, and writes
`target\probe\visibility-stage0-results.txt`.

**Done when:** the results are in that file, this plan is updated with them,
and each design point that an answer changes is changed here.

## Stages

| Stage | Builds | Size | Starts | Done when |
|---|---|---|---|---|
| 1a | "Descriptor IDs", "The journal", `read_folder_descriptor` and `Folder::descriptor`, the ReFS build's reads, "Status and memory", `PROTO_VERSION` 15 | about 500 lines | at once | `build.ps1` passes with T2 and T4 |
| 1b | "The NTFS build", "NTFS metadata folders", and with the sharing `read_folder_security` and `record_usn` | about 350 lines, or 150 without the sharing | after stage 0 item 1, scheduled last in stage 1 | `build.ps1` passes with T1, T3 and T9 when the sharing stays, and with T4's metadata case when the predicate stays |
| 2a | "The caller" without the account names, "Who sees what", "The filter", "Folder sizes" with the window's empty Size cell, "Logging", the client's identification level, the stored list's reader | about 850 lines | at once | `build.ps1` passes with T5 to T8, T11 and W1 to W3; agent check A4 |
| 2b | `gozlerd see-every-file`, `install --see-every-file`, the account names, the writer, the setup's page, the help note | about 300 lines | at once | `build.ps1` passes with T10 and the help tests |
| 2c | "Text" | about 150 lines of prose | at once | the text is changed |
| 3 | the list on its own page, Settings > Visibility ([service settings plan](service-settings.md)) | about 400 lines | at once | as that plan says |

Stage 1 alone changes no answer: nothing reads the column yet.

**Reviews.** Each reviewer reads and reports; the implementer fixes the
findings and runs `build.ps1` again.

- **Stage 1 code review**, when 1a and 1b are complete: `index/volume.rs`,
  `index/store.rs`, `index/descriptors.rs`, `read_build_descriptors` and the
  tail, because every journal record passes through that code. It runs
  beside stage 2's work.
- **Security review**, when 2a and 2b are complete and stage 0 items 1 and 4
  have confirmed per-request answers: `caller.rs`, `access.rs`, the stored
  list's reader and writer, the refusals in `serve`, `Scan::evaluate`,
  `find_head`, `EntryView::size`, the answers' lifetime across a request's
  holds, and the setup's grant. These decide which names each user reads,
  and one wrong path shows every name to every user with no error.

**Release candidate.** Built once with `build.ps1 -Release` after both
reviews. The owner installs it, runs his after-install script and the After
install list ("Checks"); an agent then runs its checks. Only the public
release waits for these.

## Checks

Each check names the failure it watches. A check reads the state when it
tests it and never waits a fixed time; one that waits for a change polls
until a deadline.

### Tests in `build.ps1`

T1 to T11 and W1 to W3 ("Tests"), and the existing help tests.

### Agent checks, without elevation

- **A1.** After the owner installs: `gozler --status` shows `C:` and `V:`
  live, with a `permissions` memory part, and no `Permissions` line.
  Watches: a read that fails on every PC (spec `V9`, `V12`). It compares no
  memory figure, because the table's map and growth room, and descriptors no
  folder uses any more, make any expected size a guess.
- **A2.** Only when the build shares descriptors. After the owner installs:
  the build's log line for `C:` gives a number of descriptor reads close to
  stage 0 item 1's count of distinct folder `SecurityId` values, not to its
  folder count. Watches: sharing that silently never happens on a real
  volume, because the layout's `Usn` and the record's differ there, which
  makes every boot read each folder and which T3 and T9, on fixtures, cannot
  see. It counts reads instead of timing the boot, so the machine's speed
  does not decide it.
- **A3.** After the owner installs: `gozlerd see-every-file` lists the
  entries and ends with the off-switch line; `gozlerd see-every-file add x`
  fails with the elevation message. Watches: a list that needs elevation to
  read, or a change made without it (spec `V16`).
- **A4.** `pwsh ./build.ps1 -Bench query` once before stage 2a and once after
  it. The `query` bench gains one case with a caller that cannot list half
  the folders. For `SeenBy::Everything`, every case is within noise of the
  run before; for the caller, a broad query costs at most 10 % more. If it
  does not hold, the numbers go to the owner.

### The owner's after-install script

`target\probe\visibility-check.ps1` runs elevated, with one prompt, and
writes `target\probe\visibility-check-results.txt`. It runs `gozler` as
`gozlertest` with `Start-Process -Credential` and the credential that
`target\probe\gozlertest.credential.xml` keeps for the owner's account, so
nobody types a password; as SYSTEM through a one-time scheduled task; and as
the owner without elevation through a task with RunLevel Limited. It leaves
the list as it found it.

- **S1. Who sees every file** (spec `V3`, `V16`). With `count` the result of
  `gozler -parent "C:\Program Files\WindowsApps" -get-result-count`: the
  package folders directly in `WindowsApps`, which lists for elevated
  administrators only. The check counts only these direct children, because
  each package folder grants Users listing, so a filtered user rightly sees
  the entries below them (spec `V1`, `V2`). With the owner on the list,
  `count` is the same without and with elevation, and not 0; after `gozlerd
  see-every-file remove <owner>`, it is 0 without elevation and not 0
  elevated; `add <owner>` restores it; as SYSTEM it is not 0. Watches: an
  administrator exception, SYSTEM filtered, and a list change that does not
  apply at the next search.
- **S2. A filtered user's answers** (spec `V1`, `V5`, `V7`). As `gozlertest`:
  `gozler -path C:\Users\<owner> -n all` lists no row, and `gozler -parent
  C:\Users <owner>` lists the `C:\Users\<owner>` row; S1's `WindowsApps`
  count is 0; `gozler -path C:\Users\<owner>\Documents x` exits 0 with no
  rows and no warning, and with `-no-result-error` exits 9.
- **S3. Folders and changes** (spec `V1`, `V2`, `V4`, `V8`, `V9`, `V15`). In a
  scratch tree on `C:` and one on `V:`, with `gozlertest` denied listing on
  the top folder and allowed on a folder two levels below it: the names
  `gozler -path <tree> -n all` lists as `gozlertest` equal the names
  `Get-ChildItem -Recurse -Force` lists as that account, plus the entries of
  the allowed folder, shown with their full path. After `icacls` takes away
  the allowed folder's right, a search stops finding its entries within a
  second, and finds them again within a second after the right comes back.
  A new folder created in the allowed folder, with a file in it, is found
  within a second. A change to the top folder that its subfolders inherit
  shows in a subfolder's answer.
- **S4. Folder sizes** (spec `V6`). In the scratch tree, which holds files of
  known sizes, as `gozlertest`: `gozler -parent <tree> /ad -size <top folder
  name>` shows the top folder with no size, `gozler -path <tree> /ad
  size:>0` lists no folder, and `gozler -get-folder-size <tree>\<top
  folder>` gives the sum of the allowed folder's files only. As the owner,
  the top folder's size is the sum of every file.
- **S5. The list's commands** (spec `V16`, `V17`). `see-every-file add
  gozlertest` makes S1's `WindowsApps` count for `gozlertest` equal the
  owner's at the next search; `remove gozlertest` brings back 0; `add S-1-1-0`
  gives the same entry as `add Everyone`. With the value deleted, `gozlerd
  install --see-every-file none` stores an empty list; with a value present,
  `install --see-every-file gozlertest` prints that it kept the list and
  changes nothing.
- **S6. A silent setup grants nobody** (spec `V17`). With the value deleted,
  the setup built with `installer\Build.ps1` runs with `/VERYSILENT`; then
  no value exists.

### The After install list

The owner checks these when he installs the release candidate with the
setup, which he does anyway.

- **L1.** On a PC with no stored list, the setup's page names the owner, is
  checked, and comes after the cleanup page when that page shows; after the
  install, `gozlerd see-every-file` lists the owner.
- **L2.** The setup run again shows no page.
- **L3.** Stage 3's screenshots, as the service settings plan lists them.

### Not checked by a run

- That a setup approved by another administrator runs in the requesting
  session: Windows shows the elevated setup's window on that session's
  desktop, and stage 0 item 5 showed that the API names the session's user.
- `gozler.exe` from other builds. One from before stage 1a speaks protocol
  9, so gozlerd refuses it at `Hello` (spec `V12`). One from stage 1a or 1b
  speaks protocol 10 but still opens the pipe anonymously, so a stage 2a
  gozlerd refuses its searches with the anonymous message of spec `V11`.
  Only development builds are of the second kind, because the release
  carries every stage.
- The volume states in which permission changes wait for a rescan (spec
  `V8`): a volume without a journal shows as `snapshot`, and a stopped tail
  as offline or failed, in `--status` already.

## Tests

Each test names the failure it watches.

- **T1. The layout keeps the `SecurityId` and the `Usn`** (stage 1b, only
  with the sharing, which alone uses them). `LayoutInfoFixture` gains
  `security_id` and `usn`, written at offsets 44 and 48, and `arb_info`
  draws both at random, so `roundtrip_fixture_build_then_parse` checks them.
  A wrong offset would give folders each other's descriptors, which no other
  test and no error shows.
- **T2. A security change reaches the index** (stage 1a).
  `security_only_emits_nothing` pins the old behavior, so it becomes
  `security_change_marks_the_descriptor_dirty`, and
  `unrelated_bits_do_not_mask_matches` expects `SecurityDirty` with the other
  ops. A delete with the bit still yields only `Delete`. The tail's existing
  `reason_mask_covers_every_bit_that_drives_an_index_op` fails until
  `REASON_MASK` has the bit. Without it, permission changes are never seen
  and spec `V8` is silently false.
- **T3. The build shares a descriptor only when it is safe** (stage 1b, only
  with the sharing). A `bootstrap.rs` test of `read_build_descriptors` with
  a fake `VolumeReads`: three folders on one `SecurityId` whose first read
  returns the pass's USN all get one ID; when the first read returns another
  USN, fails, or finds the folder gone, each folder gets its own read;
  folders on `SecurityId` 0 are each read on their own; a folder no longer in
  the index is not read. Without it, one folder whose permissions changed
  during the build gives its new permissions to every folder that shared its
  old `SecurityId`, with no error.
- **T4. The column follows each folder** (stage 1a). An `apply_batch` test:
  a new folder, a folder with `SECURITY_CHANGE`, and a file that becomes a
  folder each appear once in `needs_descriptor`; a file with
  `SECURITY_CHANGE` does not. A folder that has a descriptor and gets
  `SECURITY_CHANGE` has `None` after the batch. A folder is given a
  descriptor and deleted, and a new folder created in the same batch takes
  its freed slot: the new folder is in `needs_descriptor`. After
  `compact_entries`, every folder keeps its own descriptor. In stage 1b, and
  only when stage 0 keeps the metadata predicate, the test gains one case: a
  new folder below `$Extend` is not in `needs_descriptor`. Without it, a new folder's entries stay hidden until
  the next rescan, a folder that loses a user's permission shows that user
  the names added to it in the same batch, and a folder in a reused or
  compacted slot shows its names to every user who could list another one.
- **T5. Every answer leaves out hidden entries** (stage 2a). One engine test
  over a fixture index with a caller that cannot list one folder, and one
  folder whose descriptor is `None`. On each scan path (scope walk, folded
  arena, component walk, full scan), the rows, `total`, `run_totals`' count
  and size, and `match_sums`' counts and sizes leave out the entries of both
  folders; under the caller every folder has no size and `size:` matches no
  folder; under `SeenBy::Everything` the same folders keep their sums. Sorted
  by size under the caller, on one volume and over two, two visible folders
  whose names and real sizes run in opposite orders come in name order.
  Across a reply's later holds: a row whose parent gets a descriptor that
  was new after the answers were made is left out, with no panic, and the
  total drops by one; a file moved into the folder the caller cannot list
  between the scan and the page is left out. Without it, one path or size
  reader that skips the filter shows hidden data, the size sort's tie runs
  or the merge show which visible folder is larger, an ID past the answers'
  end panics the daemon, or a long export shows a file moved into a hidden
  folder.
- **T6. `Everything` changes nothing** (stage 2a). The golden corpora run
  unchanged, seeing everything (see "The filter"). No new test: the existing
  rows are the check.
- **T7. The daemon refuses an unknown caller and filters a caller** (stage
  2a). A `server.rs` test of `serve` with `Access::Refused`: `Search` and
  `CountIncluded` get `ProtoError::Unsupported` with the message; `Hello`,
  `Status` and `SettingsSaved` answer. With a `Visibility::Caller` over a
  fixture index and a test check, `Search` gets only the visible rows and
  `CountIncluded` is refused. Without it, a change to `serve` can answer an
  unknown caller unfiltered, which lets any program bypass the filter, or
  give any user the Include count, whose rules can spell hidden names.
- **T8. Who sees every file** (stage 2a). A test of `sees_everything`:
  SYSTEM as the user with an empty list, the caller's user on the list, an
  enabled group on the list, and Everyone on the list give `true`;
  Administrators among the caller's SIDs with an empty list, and a list that
  names only a group the caller does not hold, give `false`. Without it, a
  wrong match turns the filter off for users the administrator did not list,
  or leaves it on for one they did.
- **T9. A malformed USN record returns an error** (stage 1b, only with the
  sharing). A proptest over arbitrary bytes for `record_usn`, as `AGENTS.md`
  asks of a new parser, and one fixture each of a V2 and a V3 record whose
  `Usn` it returns. Without it, a short or odd record could panic the daemon
  during a build, or a wrong offset would make every USN differ, which
  silently turns the sharing off.
- **T10. Account names work in any language** (stage 2b). `account_sid`
  gives `S-1-1-0` for `Everyone` and `EVERYONE`, and converts an `S-1-…`
  string, without calling `LookupAccountNameW`. Without it, a plausible
  simplification ("just look the name up") makes `add Everyone` fail on
  Windows in another language.
- **W1. The check agrees with Windows** (stage 2a). Stage 0 item 3 as a
  `#[cfg(windows)]` test in `gozler-winfs`, with the folders it makes in a
  temporary folder. It also passes the function that owns the owner-and-group
  rule a descriptor built from SDDL with no group (`O:BAD:...`, through
  `ConvertStringSecurityDescriptorToSecurityDescriptorW`), and checks that it
  is refused. Without it, a wrong
  access mask, a missing label in the descriptor read, or the wrong token
  type makes spec `V4` false with no error, and a descriptor `AccessCheck`
  cannot use hides its folders with nothing in `--status`.
- **W2. The pipe gives the caller's token, and never one that acts as the
  caller** (stage 2a). An in-process pipe with a unique name: a client at the
  identification level gives a token for the current user at the
  identification level; an anonymous client gives `Ok(None)`. Without it, a
  kept token that can impersonate, or an anonymous client served as
  somebody, breaks spec `V10` or `V11` with no error.
- **W3. Deny-only SIDs and restricted tokens do not count** (stage 2a).
  `enabled_sids` of a token made with `CreateRestrictedToken` whose
  `SidsToDisable` holds a group leaves that group out, and one whose
  `SidsToDisable` holds the user leaves the user out; for a token with a
  restricting SID it gives `None`. Without it, a non-elevated administrator
  sees every file when Administrators is on the list (spec `V16`), and a
  program that runs a listed user's work with a restricted token reads every
  name through gozlerd.
- **T11. A folder's unknown size is an empty cell** (stage 2a). A
  `gozler-ui` test of `text::cell` for the Size column: a folder with no
  size gives an empty text, and a file with no size gives "Unknown". Without
  it, a plausible simplification back to one arm makes every folder row of a
  filtered user read "Unknown" again (spec `V6`).

Not tested in code: the stored list's registry reader and writer, because a
test that writes `HKLM` needs elevation; owner check S5 covers them.

## Costs

| Cost | Size | Source |
|---|---|---|
| Descriptor column | 4 bytes per entry, 8.1 MB on `C:` | computed from the measured entry count |
| Descriptor table | distinct descriptors times their size; estimate under 2 MB | stage 0, item 1 |
| Boot time | NTFS: one read per distinct folder `SecurityId`, or per folder without the sharing; ReFS: one call per folder on a handle the walk opens | stage 0, item 1; agent check A2 counts the reads |
| A filtered request | one `AccessCheck` per descriptor of each volume it reads, D × c, estimate 6 ms, and D bytes per volume; D grows with uptime until a rescan | stage 0, items 1 and 4 |
| A query | one parent lookup and one `Vec<bool>` read per candidate for a filtered request; nothing for `Everything` | agent check A4 |
| Every request | one token read and one registry read | estimate: microseconds |

## Not verified

- **The descriptor count on volumes unlike the owner's.** A volume with
  explicit permissions on each of millions of folders would make the table
  large and every filtered request slow. `--status` shows the table's memory.
  Checked: stage 0, item 1, on the owner's volumes only.
- **The check for another user's token.** W1 uses tokens derived from the
  agent's own; owner check S3 uses `gozlertest`.
- **Inherited changes in the journal.** From Microsoft's description of
  `SetNamedSecurityInfo`. Checked: owner check S3.
- **ReFS writes `SECURITY_CHANGE`.** Checked: stage 0, item 2.
- **The NTFS metadata folders' reads.** Checked: stage 0, item 1.
- **The setup's buffer copy in the Inno Setup version that builds it.**
  Checked: After install item L1.
- **A RunLevel Limited task as the signed-in owner without a password.**
  Owner check S1 relies on it; if it fails, S1's non-elevated half moves to
  the After install list.
- **Central access policies** (spec, "Not covered"): not checked, because the
  owner's PCs have no domain.

## When the plan is done

When stage 3 lands, the rules move into `docs/DESIGN.md`,
`docs/SECURITY.md` and `README.md`, as "Text" says, and this plan and its
spec are deleted. Before the deletion, every decision this plan holds moves,
with its reason, into `docs/DESIGN.md` or into the doc comment of the
module that owns it: the "Decisions made during the work" list, the
choices in "Design" (such as the per-request answers instead of cached
views, the descriptor table's rebuild, and why the NTFS build shares
descriptors or not), and the accepted risks, not only what "Text" lists,
because a decision that leaves with the plan cannot be checked later. The
probes in `target\probe\` are deleted with them.
