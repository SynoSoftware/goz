# Plan: show each user only what they may see

How to build what [the spec](visibility-spec.md) describes, in what order,
and how each stage proves that it is done. The design is here too. This plan
changes as the work progresses; the spec changes only when a requirement
changes.

## Schedule

**Scheduled:** in the queue, not started. Stage 0 item 8 already passed, on
2026-10-18.

- **Stage 0** starts when the owner takes the plan from the queue. It runs
  on the owner's machine, elevated, and changes no product code.
- **Stage 1** starts after stage 0. It changes no answer.
- **Stage 2** starts after stage 1. It changes how `gozler-client` opens the
  pipe, and the window uses the same client (`docs/plans/ui-plan.md`), so it
  lands between two window stages, not during one.
- Stages 1 and 2 land in one release. Stage 1 alone shows nothing new. Within
  stage 2, filtered rows and unknown folder sizes land together, because
  filtered rows with full folder sizes still show how much a hidden folder
  holds.

Each stage is one or more commits, each of which passes `check.ps1`
(`AGENTS.md`, "Before you finish"), with the tests that "Tests" names for it.

## What the code does now

- **NTFS gives each file's `SecurityId`, and gozler drops it.**
  `FILE_LAYOUT_INFO_ENTRY` starts with the 40-byte `BasicInformation`, then
  `OwnerId` (offset 40), `SecurityId` (offset 44, 4 bytes) and `Usn`
  (offset 48, 8 bytes); `INFO_ENTRY_LEGACY_LEN` is 56 (`layout/parse.rs`).
  `parse_file` reads only `RawInfoPrefix`, the first 24 bytes. NTFS gives one
  `SecurityId` to all files that have the same security descriptor, and keeps
  each descriptor once.
- **The journal records have a `SecurityId` too** (`_security_id` in
  `usn/record.rs`). This plan does not use it, so NTFS and ReFS follow one
  rule (see "Reading descriptors").
- **ReFS gives none.** `FileIdExtdDirectoryInfo` has no security field. The
  ReFS build and the tail's `settle_batch` both list folders through
  `walk_folders` (`gozler-core/src/index/settle.rs`), whose reads,
  `bootstrap::Disk`, open each folder with `open_folder`
  (`gozler-winfs/src/file_id.rs`): `OpenFileById` with backup semantics and
  `FILE_LIST_DIRECTORY`.
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
- **Every query result passes one function.** `scan` in `query/engine.rs`
  calls `Scan::visit`, then `Scan::evaluate`, for every candidate, on all
  three paths: the scope subtree walk, the folded arena and the full scan.
  `run_query`, `run_query_deferrable` and `run_totals` all use `scan`, and
  `total` is counted after `evaluate`.
- **A folder's size is read in three places**, all through `EntryView::size`:
  `Check::Size` in `Check::test`, the `QueryHit` built in `run_query_impl`, and
  `OrderFields::size` for `Ranked`. `run_totals` adds file sizes only, so
  `-get-folder-size` and `-get-total-size` sum the files a scan passes.
  `EntryView::size` already returns `None` for a folder while a volume's
  folder sizes are unknown, and every one of those readers handles `None`.
- **The index has a column only folders use**: `first_child` in `EntryTable`
  (`index/store.rs`).
- **A `-path` scope never returns the scope folder itself**: the subtree walk
  starts at its first child, and `in_scope` is false for the scope root.
- **The client hides who it is.** `gozler-client/src/client.rs` opens the
  pipe with `SECURITY_SQOS_PRESENT` and the anonymous level.
- **The daemon reads each request on the async side.** `handle_connection`
  in `server.rs` reads a frame, then runs `serve(volumes, frame, emit)` with
  `spawn_blocking`. The server tests and `server::tests::corpus` call `serve`
  directly. `run_on_volumes` reads each target volume through
  `VolumeState::read`, which holds the volume's `parking_lot` read lock on
  its index. The daemon runs as LocalSystem and refuses remote clients.
- **`gozlerd` has subcommands** (`Command` in `gozler-daemon/src/main.rs`,
  with clap): `scan`, `run`, `install`, `uninstall`, `status`.
- **No crate reads the registry yet**, and the workspace's `windows-sys`
  features lack `Win32_System_Registry` (root `Cargo.toml`).
- **Memory.** The owner's `C:` has 2,015,690 entries in 194.6 MB of index
  (ReFS plan measurement). A 4-byte column adds 8.1 MB, 4.1 %.

From Microsoft's documentation:

- `ImpersonateNamedPipeClient` takes the security context of the last
  message read from the pipe. It succeeds at a level below
  `SecurityImpersonation`, so an identification-level client is enough. If it
  fails, "no client requests should be executed".
- `AccessCheck` compares a security descriptor with an impersonation token
  that has `TOKEN_QUERY` access. The descriptor must contain an owner and a
  group, or the call fails with `ERROR_INVALID_SECURITY_DESCR`.

## Design

### Parts

| Part | Where | Owns |
|---|---|---|
| Descriptor IDs | `gozler-core/src/index/descriptors.rs` (new) | `DescriptorId`, `DescriptorTable`: each distinct descriptor once |
| The descriptor column | `EntryTable` in `index/store.rs` | `descriptor: Vec<Option<DescriptorId>>`, set for folders only |
| Views in the index | `gozler-core/src/index/views.rs` (new), used by `index/volume.rs` | `ViewId`, `Visibility`, `SeenBy`, `MayList`, `ViewAnswers` |
| The filter | `Scan::evaluate` in `query/engine.rs` | which candidates a request sees |
| Reading descriptors | `gozler-winfs/src/file_id.rs` | `read_folder_security`, `Folder::descriptor`, `descriptor_sddl` |
| A record's USN | `gozler-core/src/usn/record.rs` | `record_usn` |
| The NTFS build | `gozler-daemon/src/bootstrap.rs` | `NtfsReads`, `build_ntfs`, `read_build_descriptors` |
| The caller | `gozler-winfs/src/caller.rs` (new) | `Sid`, `ClientToken`, `pipe_client_token`, the access check |
| The stored list | `gozler-winfs/src/parameters.rs` (new) | reading and writing "Users and groups who see every file", account names |
| Who sees what | `gozler-daemon/src/views.rs` (new) | `ViewRegistry`, `Access`, `sees_everything` |
| The journal | `usn/ops.rs`, `index/settle.rs`, `tail.rs` | `UsnOp::SecurityDirty`, reading changed descriptors |
| The commands | `gozler-daemon/src/main.rs`, `service.rs` | `gozlerd see-every-file`, `gozlerd signed-in-user`, `gozlerd install --see-every-file` |
| The setup's choice | `installer/Setup.iss` | the page that offers the signed-in user |

`gozler-core` stays OS-agnostic: it compares descriptor bytes and calls a
closure that answers "may this view list a folder with this descriptor". It
never parses a descriptor and never calls Windows.

### Descriptor IDs

- `DescriptorId(NonZeroU32)`, so `Option<DescriptorId>` takes 4 bytes. The
  column holds `None` for a file and for a folder whose descriptor gozlerd
  has not read. One value means "no descriptor", with no sentinel.
- `DescriptorTable` holds each distinct descriptor's bytes once, in a `Vec`
  indexed by the ID, and a map from the bytes' hash to the IDs with that hash,
  so equal bytes always get the same ID. IDs are dense from 1, so a view keeps
  its answers in a `Vec<bool>` indexed by the ID.
- `VolumeIndex::set_folder_descriptor(&mut self, frn: Frn, descriptor:
  Option<&[u8]>)` is the one way a live entry's descriptor changes. It finds
  or adds the ID and sets it on the folder. A `frn` that is not a folder, or
  not in the index, is ignored.
- `EntryTable` owns the column like its others: `alloc` sets the slot to
  `None` when it reuses a freed slot, as it resets `first_child`, and
  `tombstone` sets it to `None`. Without this, a folder created in a deleted
  folder's slot inherits that folder's descriptor, `apply_batch` sees a
  descriptor that is not `None` and never reads the new one, and a user who
  could list the deleted folder sees every name in the new one.
- `VolumeIndex::is_metadata_folder(&self, idx: EntryIdx) -> bool` is true for
  NTFS's `$Extend` folder and every folder below it. On a volume whose root
  FRN is not wide (`Frn::is_wide`), which is how `FrnMap::for_root` tells
  NTFS from ReFS, `$Extend` is the root's child named `$Extend` with record
  number 11 (`Frn::record_number`), the number NTFS fixes for it; the test
  walks up with `chain_up`. Windows lets no caller list these folders, so `None`, which hides their entries,
  is their right answer for every view. This one predicate is the rule:
  `set_folder_descriptor` stores `None` for such a folder whatever bytes it is
  given, so the build's shared bytes and the tail agree; `apply_batch` does
  not push it to `needs_descriptor`; and `read_build_descriptors` drops it
  from its lists before reading, so no read is made and no failure counted.
  A ReFS volume has no `$Extend`, and its root FRN is wide, so the predicate
  is false for every folder there, including one a user names `$Extend`.
- A descriptor that no folder uses any more stays until the next rescan.
  Volumes have few distinct descriptors (stage 0, item 1), and a rescan builds
  a new table.
- When `rewrite_entry` turns a folder into a file, it calls
  `set_folder_descriptor(frn, None)` first, because a file has no
  descriptor.
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
allocate without bound. A descriptor larger than that fails the read, which
is counted, and its folder stays hidden.

Every failed read is counted, `ERROR_ACCESS_DENIED` included. gozlerd opens
with backup semantics, and its backup privilege grants `READ_CONTROL`
whatever the DACL says, so a refusal means that something else stands in the
way, such as an antivirus filter driver, and `--status` must show it. The
NTFS metadata folders, which refuse every caller, are never read
(`VolumeIndex::is_metadata_folder` in "Descriptor IDs"). Stage 0, item 1,
checks that no other folder refuses.

- `pub fn read_folder_security(volume: &VolumeHandle, id: u128) ->
  Result<Option<RawFolderSecurity>, WinError>`, with `pub struct
  RawFolderSecurity { pub descriptor: Vec<u8>, pub usn_record: Vec<u8> }`,
  opens by ID once, with `READ_CONTROL | FILE_READ_ATTRIBUTES`, reads the
  descriptor, then reads the raw record of `FSCTL_READ_FILE_USN_DATA`, in that
  order. `Ok(None)` means the folder is gone or being deleted, by the same
  rule `open_folder` uses; no caller counts it, because the folder's delete
  is in the journal. The NTFS build uses both fields; the tail uses
  `descriptor` and pays one more `DeviceIoControl` per changed folder, which
  keeps one reader. `gozler-winfs` never parses a record (its crate doc), so
  `usn/record.rs` gains `pub fn record_usn(rec: &[u8]) -> Result<i64,
  WalkError>`, which reads the version and the `Usn` field through the same
  `RawV2` and `RawV3` views as `parse_v2` and `parse_v3`, with checked lengths.
- `open_folder` asks for `FILE_LIST_DIRECTORY | READ_CONTROL`, and
  `Folder::descriptor(&self) -> Result<Vec<u8>, WinError>` reads from the
  open handle. A folder that is open cannot be gone, so it has no `None`.
- `pub fn descriptor_sddl(descriptor: &[u8]) -> Option<String>` converts with
  `ConvertSecurityDescriptorToStringSecurityDescriptorW`, for the debug log
  line that stage 1 check 4 reads.

`LayoutFile` gains `security_id: Option<u32>` and `usn: Option<i64>`.
`parse_file` reads them at offsets 44 and 48 of the info entry when the
available length is at least 56, through `zerocopy` like the rest, so a short
entry gives `None`, never a panic. `walk_listing_buffer` sets both to `None`.

**The NTFS build.** Stage 1 rewrites the NTFS half of `build_index`, so the
same change puts it behind a volume-reading seam. Today its rules run only
against a real volume: a stopped layout pass falls back to the MFT
enumeration, sets `metadata_pending`, and skips `compute_folder_sizes`, and
no test watches them. `build_index` keeps reading the root folder and the
ReFS branch, and calls a new `build_ntfs` for the rest:

```rust
// bootstrap.rs, daemon-private
trait NtfsReads: VolumeReads {
    fn layout_page(&mut self, restart: bool, buf: &mut [u8]) -> Result<Option<usize>, WinError>;
    fn enum_page(&mut self, start: u64, buf: &mut [u8]) -> Result<Option<usize>, Self::Error>;
}
fn build_ntfs(index: &mut VolumeIndex, reads: &mut impl NtfsReads, cancelled: impl Fn() -> bool)
    -> anyhow::Result<Option<NtfsBuild>>; // NtfsBuild { read_secs, enum_secs, metadata_pending }
```

`Disk` implements `NtfsReads` with the FILE_LAYOUT and ENUM reads. A trait
replaces `layout_pass`'s `read_page` closure because the build makes three
different reads (layout pages, ENUM pages and folder descriptors), and a test
fake answers all three. It is a supertrait of `VolumeReads` because every
NTFS build also reads descriptors through `VolumeReads::folder_security`, so
the build needs one bound and one error type. `layout_page` keeps `WinError`,
because a failed layout read stops the pass instead of failing the build.
The inline enumeration loop becomes an `enum_page` loop with the same
stuck-cursor guard. The build stays in the daemon, not in core, because core
has no logging and the build has one real adapter.

Only `layout_pass` sees each `LayoutFile`, so it gains a parameter
`security: &mut FxHashMap<u32, Vec<(Frn, i64)>>` and adds to it, for each
`LayoutFile` that is a folder and has both `security_id` and `usn`, the
`(Frn, Usn)` under its `SecurityId`. `build_ntfs` owns the map, which lives
only for the build. After the layout pass, `build_ntfs` calls
`read_build_descriptors(index: &mut VolumeIndex, folders: FxHashMap<u32,
Vec<(Frn, i64)>>, reads: &mut impl VolumeReads)` (new, `bootstrap.rs`). It
reads each folder through `VolumeReads::folder_security(&mut self, frn: Frn)
-> Result<Option<FolderSecurity>, Self::Error>`, where `FolderSecurity {
descriptor: Vec<u8>, usn: i64 }` is a struct in
`gozler-core/src/index/settle.rs`, beside `FileStat`. `Disk` answers it with
`read_folder_security` and `record_usn`, and with `Ok(None)` when the folder
is gone. The settle step calls the same method and ignores `usn`, so one read
serves both. `read_build_descriptors` first drops every folder for which
`is_metadata_folder` is true from its lists, then handles each `SecurityId`:

1. `SecurityId` 0 means the folder keeps its own descriptor, as files from
   before NTFS 3.0 can, so 0 is no group: each folder with it is read on its
   own (step 3).
2. Read the first folder of the list. If its `usn` equals the `Usn` from the
   pass, its permissions have not changed since the pass, so every folder of
   the list gets those bytes. Reading the descriptor before the USN makes
   this safe: a change before the USN read changes the USN.
3. If the USN differs, read each folder of the list on its own.
4. If the read of the first folder gives `Ok(None)` or fails, try the next
   folder of the list. A folder that no read reaches stays `None`. It adds 1
   to `descriptor_reads_failed` when its own read failed; a folder that is
   gone adds nothing, because its delete is in the journal and replays.

A change after the read is in the journal, whose cursor was taken before the
build, so the replay reads that folder again. Without step 2's USN check, one
folder whose permissions changed during the build would give its new
permissions to every folder that shared its old `SecurityId`.

After a stopped layout pass, the MFT enumeration gives no `SecurityId`, so
`build_ntfs` reads, on its own, every folder that still has `None` and is
not a metadata folder. This path is rare and slow.

**The ReFS build** and **the missed-folder walk**: `walk_folders` and
`Listed` live in `gozler-core/src/index/settle.rs`. The callbacks of
`VolumeReads::list_folder` and of `walk_folders` both take a
`Listed<'_, E>`, where `E` is the reads' error type, and `Disk` in
`bootstrap.rs` reads each folder's descriptor from the handle it opens:

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
`Folder`, and `set_folder_descriptor` ignores an entry not yet in the index,
which would leave the subfolder hidden with no failure counted. A failed read
does not fail the walk.

**The journal tail.** See "The journal".

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
  because spec `V1` needs no file descriptors, and neither does any record
  for a metadata folder (`is_metadata_folder`).
- For `SecurityDirty` on a folder, `apply_batch` also calls
  `set_folder_descriptor(frn, None)` at once, under the batch's write lock.
  The folder's entries are then hidden from every view until the new
  descriptor is read (spec `V9`). Without this, after `icacls` denies `ben`
  a folder and 5,000 files are copied into it in the same batch, `ben` sees
  their names until the read, which comes after the batch's other work.
- `settle_batch` gains a descriptor step, shaped like its stat step: with
  the lock released and `cancelled` asked before each folder, it reads each
  folder's descriptor through a new `VolumeReads::folder_security`, which
  `Disk` answers with `read_folder_security`, then applies the batch with
  `set_folder_descriptor` under one write. A failed read adds 1 to
  `descriptor_reads_failed` through
  `VolumeIndex::note_descriptor_reads_failed`; a folder that is gone is not
  counted, because its delete is in the journal. It runs between the apply
  and the missed-folder walk, so before the walk, the stat step and the link
  step, which can take seconds (one open per file), and a folder stays hidden
  only for the reads themselves (spec `V9`). It needs nothing those
  steps produce: the walk reads the descriptors of the folders it lists
  itself.
- An inherited permission change gives each changed folder its own record
  (stage 0, item 3, checks this), so the tail reads each one. If stage 0 shows
  otherwise, `SecurityDirty` on a folder also reads every folder below it,
  through the child chain.

### The caller

`gozler-client` opens the pipe with `SECURITY_SQOS_PRESENT |
SECURITY_IDENTIFICATION` (`0x0010_0000 | 0x0001_0000`). The comment on the
constant says why: gozlerd can identify the caller but cannot act as it, and
the client still checks the pipe's owner before it sends anything.

`gozler-winfs/src/caller.rs`:

- `pub struct Sid(Box<[u8]>)`: a binary SID, compared by its bytes.
- `pub struct ClientToken` owns an identification-level token handle with
  `TOKEN_QUERY`. It is `Send` and `Sync`: `AccessCheck` and
  `GetTokenInformation` only read the token.
- `pub fn pipe_client_token(pipe: RawHandle) -> Result<Option<ClientToken>,
  WinError>` calls `ImpersonateNamedPipeClient`, then `OpenThreadToken` with
  `TOKEN_QUERY | TOKEN_DUPLICATE` and `OpenAsSelf`, then `RevertToSelf`, in
  one function that never yields. `RevertToSelf` runs on every path after a
  successful impersonation. If `RevertToSelf` fails, the function aborts the
  process, because the thread would serve the next client as this user.
  `Ok(None)` means that the client is anonymous: `OpenThreadToken` fails with
  `ERROR_CANT_OPEN_ANONYMOUS`, or the token's level is `SecurityAnonymous`.
  `OpenThreadToken` returns the token at the level the client chose, and a
  client that opens the pipe with a plain `CreateFile` chooses
  impersonation. So the function then calls `DuplicateTokenEx` with
  `TOKEN_QUERY`, `SecurityIdentification` and `TokenImpersonation`, keeps
  the copy, and closes the original. A token gozlerd keeps can then never act
  as the client.
- `ClientToken::enabled_sids(&self) -> Result<Vec<Sid>, WinError>`: the
  token's user first, then each group whose attributes have
  `SE_GROUP_ENABLED` and not `SE_GROUP_USE_FOR_DENY_ONLY`.
- `ClientToken::rights_key(&self) -> Result<Vec<u8>, WinError>`: the SIDs and
  attributes of `TokenUser`, `TokenGroups` and `TokenRestrictedSids`, the SID
  of `TokenIntegrityLevel`, the `Flags` of `TokenAccessInformation`, and the
  token's security attributes except `TSA://ProcUnique` (each one's name,
  type, flags and values, sorted by name), written out as bytes with no
  pointers. Two tokens with the same key get the same answer for every
  descriptor.
  - The security attributes come from AuthZ, because
    `GetTokenInformation`'s `TokenSecurityAttributes` is documented as
    reserved, with no output structure. `caller.rs` creates one resource
    manager with `AuthzInitializeResourceManager(AUTHZ_RM_FLAG_NO_AUDIT, ...)`
    for the process, in a `OnceLock`. `rights_key` calls
    `AuthzInitializeContextFromToken` with the token, then
    `AuthzGetInformationFromContext` with
    `AuthzContextInfoSecurityAttributes`, which returns
    `AUTHZ_SECURITY_ATTRIBUTES_INFORMATION`, and frees the context with
    `AuthzFreeContext`. The workspace already enables
    `Win32_Security_Authorization`. A failure is an `Err`, which refuses the
    search (see "Who sees what"). Stage 0, item 9, checks that an
    identification-level copy of a packaged process's token returns
    `WIN://SYSAPPID` this way.
  - The flags are in the key because a write-restricted token checks its
    restricted SIDs only for writes, and a plain restricted token checks them
    for listing too.
  - The security attributes are in the key because conditional ACEs read
    them: `C:\Program Files\WindowsApps` grants Users listing only when the
    token has `WIN://SYSAPPID`, which a process of a packaged app has and
    another process of the same user has not.
  - `TSA://ProcUnique` is left out because Windows gives it a different value
    in each process, so with it every `gozler` run would build a new view.
    Stage 2 check 4 shows that two runs share one view.

  What the key leaves out does not change a listing answer for a token that
  reaches the pipe:
  - an AppContainer token, with its package SID and capabilities, cannot open
    the pipe, because its DACL (`pipe_sd.rs`) has no ACE for `ALL
    APPLICATION PACKAGES`, and a low-integrity token cannot open it for
    writing under the default medium label;
  - user claims, device claims and device groups come only from a domain
    with compound identity, which spec "Not covered" leaves out with central
    access policies.
- `ClientToken::may_list(&self, descriptor: &[u8]) -> Result<bool, WinError>`:
  `AccessCheck` for `FILE_LIST_DIRECTORY` with the file generic mapping
  (`FILE_GENERIC_READ`, `FILE_GENERIC_WRITE`, `FILE_GENERIC_EXECUTE`,
  `FILE_ALL_ACCESS`).

### The stored list

"Users and groups who see every file" (spec `V16`) is the `REG_MULTI_SZ` value
`SeeEveryFile` under `HKLM\SYSTEM\CurrentControlSet\Services\gozler\Parameters`,
one SID string per line (`S-1-1-0` for Everyone). Windows keeps service
settings under the service's key, and by default only Administrators and
SYSTEM may write there while Users may read, which is the access `V16`
needs. A missing key or value is an empty list. The service settings plan
must keep reading this value, or move it and carry what is saved.
`DeleteService` removes the service's key with its subkeys, so `gozlerd
uninstall` drops the list and the filter is on again after a new install,
which is the safe direction; an upgrade that replaces only the executables
keeps it. `docs/DESIGN.md` says so.

`gozler-winfs/src/parameters.rs` (the root `Cargo.toml` adds the
`Win32_System_Registry` feature):

- `pub fn see_every_file() -> Result<SeeEveryFile, WinError>`, where
  `pub struct SeeEveryFile { pub sids: Vec<Sid>, pub rejected: Vec<String> }`,
  reads the value with `RegGetValueW` and converts each line with
  `ConvertStringSidToSidW`. A line that does not convert goes to `rejected`,
  which the daemon logs as a warning. The value is capped at 64 KiB.
- `pub fn set_see_every_file(sids: &[Sid]) -> Result<(), WinError>` opens the
  existing service key, creates its `Parameters` subkey if needed, and writes
  the value. It never creates the service key itself, which belongs to the
  Service Control Manager.
- `pub fn account_sid(name: &str) -> Result<Option<Sid>, WinError>`: a name
  that starts with `S-1-` converts with `ConvertStringSidToSidW`; `Everyone`,
  in any case, is `S-1-1-0`; any other name goes to `LookupAccountNameW`.
  Windows names well-known accounts in the system's language ("Jeder" on
  German Windows), so without the first two rules `add Everyone` would fail
  there.
- `pub fn account_name(sid: &Sid) -> Option<String>` with `LookupAccountSidW`
  (`DOMAIN\name`).

`gozlerd see-every-file` (`Command::SeeEveryFile` in `main.rs`):

- With no argument, it prints each entry's name, or its SID string when it
  has no name, one per line; for an empty list it prints `The list is empty:
  each user sees the files in folders they can open.` Either way it ends with
  the line `To show every file to every user, run as administrator: gozlerd
  see-every-file add Everyone`, so
  the off switch is found where the list is. It needs no elevation.
  `docs/WRITING.md` governs the final wording.
- `add <account>` and `remove <account>` change the list. Not elevated, they
  fail with `gozlerd: changing who sees every file needs an administrator
  terminal`. When `service_state` reports that the gozler service is not
  installed, they fail with `gozlerd: install the gozler service first`, so
  no key appears under `Services` that the Service Control Manager does not
  own. An unknown name fails with `gozlerd: no user or group is named
  <account>`. `add` of an entry already on the list, or `remove` of one not on
  it, changes nothing and says so.

The daemon reads the list for each search request: one registry read, a few
microseconds, so a change applies at the next search with no restart and no
notification code. It logs `rejected` lines only when they differ from the
last read's, kept in a `last_rejected: Vec<String>` field of `ViewRegistry`
under the lock `access` already takes, so one bad line written with `regedit`
gives one warning, not one per search.

### The setup's choice

Spec `V17`. Three parts, each with one job:

- **Who is signed in** is decided once, in `gozler-winfs/src/parameters.rs`:
  `pub fn signed_in_user() -> Result<Option<String>, WinError>` calls
  `ProcessIdToSessionId` for the current process, then
  `WTSQuerySessionInformationW` with `WTSDomainName` and `WTSUserName` for
  that session, and returns `DOMAIN\name`. Session 0, or an empty user name,
  gives `None`. The session's user does not depend on the process's account
  (`WTS_INFO_CLASS`: "the name of the user associated with the session"), so
  an elevated process that another administrator approved still gets the
  standard user who started it, if that process runs in the same session
  (stage 0, item 8, checks this). The root `Cargo.toml` adds the
  `Win32_System_RemoteDesktop` feature.
- **`gozlerd signed-in-user`** (`Command::SignedInUser` in `main.rs`) prints
  that name and exits 0, administrator or not, because an administrator also
  sees every file only through the list (spec `V3`). With no user it prints
  `gozlerd: no user is signed in to this session` and exits 1. It needs no
  elevation.
- **`gozlerd install --see-every-file <account>`**, or `--see-every-file
  none`, stores the list with that one account, or empty, but only when no
  `SeeEveryFile` value exists; otherwise it prints `Kept the existing list of
  users who see every file.` It writes the list after it creates or updates
  the service and before it starts it, so the first search already follows
  the choice. Without the option, `install` leaves the list as it is, which
  is how the window's "Install the service" button and a hand install behave.
  `account` resolves with `account_sid`, as in `see-every-file add`, before
  the service is touched, so an unknown name fails with the same message and
  changes nothing. A PC upgrading from a version without this feature has no
  value yet, so its setup shows the page once, which is the first time the
  choice exists there.

`installer/Setup.iss`:

- In `InitializeWizard`, when `WizardSilent` is false and
  `RegValueExists(HKLM64, 'SYSTEM\CurrentControlSet\Services\gozler\Parameters',
  'SeeEveryFile')` is false, the setup extracts `gozlerd.exe` with
  `ExtractTemporaryFile` and runs `gozlerd.exe signed-in-user` with
  `ExecAndCaptureOutput`. When it exits 0, the setup keeps the trimmed first
  line as `<name>` and adds a page with
  `CreateInputOptionPage`, after the license page, with one check box,
  checked: `Let <name> see the names of every file on this PC`, and the text:
  `gozler shows each user only the files in folders they can open. Checked,
  <name> sees every file name, also without running as administrator. This
  does not let them open any file they cannot open now.` The texts are `[CustomMessages]` entries, and
  `docs/WRITING.md` governs their final wording.
- In `CurStepChanged(ssPostInstall)`, the `gozlerd install` call gains
  `--see-every-file "<name>"` when the page was shown and the box is checked,
  `--see-every-file none` when it was shown and cleared, and nothing when the
  page was not shown.

The registry test in `InitializeWizard` and the "only when no value exists"
rule in `gozlerd install` agree by design: the setup only decides whether to
ask, and `gozlerd install` is the one place that decides whether to write.

### Who sees what

`gozler-core/src/index/views.rs`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewId(u32);

/// Whether a caller may list a folder with this descriptor.
pub type MayList = Arc<dyn Fn(&[u8]) -> bool + Send + Sync>;

/// What a request may see. A view carries its check, because every volume
/// that lacks the view needs both to add it.
#[derive(Clone)]
pub enum Visibility {
    /// Spec V3 and V16: every entry.
    Everything,
    /// Spec V4: what this view may list.
    View { id: ViewId, may_list: MayList },
}

/// A `Visibility` resolved against one volume's index for one scan.
#[derive(Clone, Copy)]
pub enum SeenBy<'a> {
    Everything,
    View(&'a ViewAnswers),
}
```

`gozler-daemon/src/views.rs`:

```rust
/// What the daemon decided for one request, before it runs.
pub enum Access {
    /// Spec V11: searches are refused with this message.
    Refused(&'static str),
    Sees(Visibility),
}

/// Spec V3 and V16: whether the caller's enabled SIDs, user first, hold
/// SYSTEM (`S-1-5-18`) or meet the stored list.
fn sees_everything(caller: &[Sid], list: &[Sid]) -> bool;
```

`server::run` creates an `Arc<ViewRegistry>` and passes it to `accept_loop`
and `handle_connection`. For each frame it decodes, before `spawn_blocking`,
`handle_connection` calls `pipe_client_token` and `ViewRegistry::access`,
which returns an `Access`:

- `Ok(None)` gives `Refused` with the anonymous message of `V11`; `Err` gives
  `Refused` with the other message of `V11`, and logs the error once per
  connection.
- A token whose `enabled_sids` hold SYSTEM or meet the stored list
  (`sees_everything`) gives `Sees(Visibility::Everything)`. When reading the
  stored list fails, only the SYSTEM test counts, and every other caller is
  filtered by its own token, which is the safe answer.
- Any other token gives `Sees(Visibility::View { .. })` for its `rights_key`,
  created on first use. The view keeps the token in an `Arc`; its `MayList`
  closure captures it and maps an `AccessCheck` error to `false`, with a debug
  log, so an error hides rather than shows.
- When `enabled_sids` or `rights_key` fails, the result is `Refused` with the
  second `V11` message, logged once per connection. Without the groups no view
  can be built, and a guessed view could show names.

Calling `pipe_client_token` once per frame, not once per connection, keeps
"the last message read" exact; it costs a few microseconds.

`serve(volumes, access: &Access, frame, emit)` answers `Hello` and `Status` for
any `Access`. For `Search` and `Query`, `Refused` gives `Response::Error` with
`ProtoError::Unsupported` and its message; `Sees` runs with its
`Visibility`. Tests and `server::tests::corpus` pass
`Access::Sees(Visibility::Everything)`, or a `Visibility::View` with a test
closure.

The registry keeps at most 32 views, because a PC has few users and each view
adds an `AccessCheck` to every new descriptor. A 33rd key replaces the least
recently used view. `ViewId`s are never reused.

**Views in each index.** Each volume's `VolumeIndex` keeps a `ViewAnswers`
for each view added to it: its `MayList` and `listable: Vec<bool>` indexed
by `DescriptorId`. `VolumeIndex` gains:

- `add_view(&mut self, id: ViewId, may_list: MayList)`: calls `may_list` for
  every descriptor, with no pass over the entries;
- `view_ids(&self) -> impl Iterator<Item = ViewId>` and
  `retain_views(&mut self, keep: &[ViewId])`, for the sweep;
- `seen_by(&self, visibility: &Visibility) -> Option<SeenBy<'_>>`: `None`
  when the index has no such view.

When `set_folder_descriptor` adds a new ID, the index calls each view's
`MayList` for it at once, under the same write lock, so no view meets a
descriptor without an answer, and no other lock is involved.

`run_on_volumes` in `server.rs` gains the request's `&Visibility` and reads
each volume through a new `VolumeState::request(&Visibility)` instead of
`VolumeState::read`. `request` takes the read lock as `read` does. When
`seen_by` returns `None`, it drops the read lock, takes the write lock,
checks again, calls `add_view` if the view is still missing, then turns the
guard into a read guard with `RwLockWriteGuard::downgrade` and returns it,
so the query runs under it and no rescan and no sweep can remove the view in
between. A new volume or a rescanned index so
gets the view on its first request. After `add_view` it logs, at info level,
`view added` with the volume, the `ViewId`, the view's memory and the time
it took. It never uses `upgradable_read`:
`compact_index` holds the one upgradable read for its whole planning pass,
about 184 ms, and every request would queue behind it.

If `seen_by` still returns `None`, the scan returns no candidates. This
cannot happen with the downgrade, but the `Option` makes the type force an
answer, and the answer hides.

**The sweep.** Once a minute, `accept_loop` runs `ViewRegistry::sweep(&self,
volumes: &VolumeSet)` with `spawn_blocking`. It drops from the registry each
view unused for 24 hours, copies the IDs that remain, and releases the
registry's lock. The key holds the token's logon SID, which changes at each
sign-in, so each sign-in leaves the last session's view unused; without the
expiry, a user who signs in daily would leave about 30 dead views per volume
within a month, each answering every new descriptor. 24 hours keeps the
view of a user who searches once an hour, who would otherwise rebuild it,
under each volume's write lock, at each search. Then, for each
volume, it reads `view_ids` under the read lock, and only when the volume holds
a view the registry no longer holds does it take the write lock and call
`retain_views` with the registry's IDs. Removing by "not in the registry" also
cleans a view that a request added to a volume while the sweep ran. Taking the
write lock only when something must go matters, because a `parking_lot`
writer that waits behind a long export blocks every new reader. A dropped view
costs only its rebuild on the next request.

**Lock order.** The registry's `Mutex` is never held while a volume lock is
taken: `access` returns before any volume is locked, and the sweep releases
the registry before it locks volumes. `MayList` closures take no lock.

### The filter

- `run_query`, `run_query_deferrable` and `run_totals` gain a `seen_by:
  SeenBy<'_>` parameter, and `Scan` stores it.
- `Scan::evaluate` returns `None` when the view cannot list the candidate's
  parent folder: the parent's descriptor is `None`, or `listable` is `false`
  for it. The root has no parent and is visible. For `SeenBy::Everything` the
  test is skipped. The test runs after `entry_clauses` and the scope test and
  before the path clauses: the cheap name tests reject most candidates of a
  full scan first, so most entries never cost the random read of their
  parent's descriptor, and no hidden entry pays for a path. `evaluate` is the
  one place every
  candidate passes, so rows, `total`, totals, every query form and the scope
  walk follow it (spec `V5`), and filtering comes before the sort and the
  limit (spec `V7`).
- `VolumeIndex::entry(idx)` becomes `entry(idx, seen_by: SeenBy<'_>)`, and
  `EntryView` keeps it. `EntryView::size` returns `None` for a folder and a
  `SeenBy::View` (spec `V6`), the value it already gives while a volume's
  folder sizes are unknown, so `Check::Size`, the `QueryHit` and the size
  sort need no change. The compiler thus makes every caller of `entry`
  choose, so no answer path reads a folder sum by mistake. The callers that are not answers pass
  `SeenBy::Everything`: `query/paths.rs`, `in_scope` and `resolve_scope` in
  `query/engine.rs`, `index/volume.rs` itself, `index/tests.rs`, and
  `run_demo_query` in `gozler-daemon/src/main.rs` (`gozlerd scan`).
- The golden corpora run with `Access::Sees(Visibility::Everything)`, so
  their rows do not change.

### Folder sizes

Spec `V6` is the owner's rule: a user learns only about files they may see,
and a user who does not see every file gets no folder sizes when
visible-only sizes would slow searching. They would:

- Each view would keep a sum for every folder above a hidden file:
  megabytes per view on `C:`.
- Every size change, create, delete and move would update every view's sums
  under the tail's write lock, which every search waits behind; it would be
  most of stage 2's work in the index.
- A new view would need a pass over all 2 million entries of `C:` under the
  volume's write lock, so the first search of each user, and of each new
  sign-in, would stall every other search on that volume.

So the index keeps no per-view sizes. `EntryView::size` returns `None` for a
folder under a `SeenBy::View` ("The filter"), the folder sums stay as they
are for `SeenBy::Everything`, and `run_totals` already adds only the files a
scan passes, so `-get-folder-size` and `-get-total-size` give the visible
total.

### Status and memory

- `IndexStats` gains `descriptor_reads_failed`, added to by
  `VolumeIndex::note_descriptor_reads_failed(n)`.
- `VolumeStatus` gains `descriptor_reads_failed: u64`, and `VolumeMemory`
  gains `descriptors: MemPair`, both `#[serde(default)]`. `descriptors`
  counts all memory permissions use (spec `V12`): the descriptor table plus
  each view's `listable` vector. `VolumeMemory::total` and
  `IndexMemory::total` add the new component, so every reader of the total,
  including the window's Index page, follows. `build_status` fills both. The
  struct literals in `gozler-cli/src/main.rs` and the test in
  `proto/types.rs` are updated.
- `gozler --status` (`gozler-cli/src/main.rs`) adds `permissions <bytes>` to
  the second memory line, and, when the count is not 0, a line
  `Permissions   N folder permission reads failed since the last scan; such a
  folder's contents stay hidden from most users until the folder changes`.
  The count only rises until the next scan, as `IndexStats`' other counts do,
  so the text says "since the last scan".

### Text

- `escompat/help.rs`: one entry in `NOTES`, as spec `V13` gives it.
- `docs/DESIGN.md`: a new section "Visibility" after "Sizes and dates" with
  the rules of spec `V1` to `V17`; "Sizes and dates" says that a folder has a
  size only for a caller who sees every file, and why (spec `V6`); "Paths"
  changes "a folder the user cannot
  read still searches, because gozlerd answers from its index" to say that it
  searches and shows what the user may see; the "Not planned" line about
  filtering by permissions is removed.
- `gozler-client/src/lib.rs`: the doc comment of `folder_is_missing` says the
  same as "Paths".
- `docs/plans/ui-spec.md`: "Folder sizes", the Size column and the details
  pane's Size field say that a user who does not see every file gets no
  folder sizes (spec `V6`), and that the window leaves a folder's size empty,
  in the column and in the pane, whenever it is unknown. A result carries no reason for an unknown size, and spec `V12`
  adds none, so the window cannot tell this rule from a size pass still
  running; "Unknown" would read as data still loading. The volume's state
  already says when a pass is running. A file's "Unknown" stays as it is.
- `docs/SECURITY.md`: the trust model says which names each user sees, who
  sees every name, who may change the list, and why the client connects at the
  identification level. `README.md`'s security paragraph says the same in one
  sentence.
- `CONTEXT.md`: gains **Visible entry**: "An entry a user may see: one in a
  folder the user may list, or any entry for a user who sees every file. A
  search shows only visible entries."

## Stage 0: measure

The implementer writes `target\probe\visibility-probe.ps1`; the owner runs
it elevated on `C:` and on the ReFS volume `V:`. Each item names what changes
if the answer is not the expected one.

1. **Descriptor count and build cost.** Count folders, files, distinct folder
   `SecurityId` values and distinct `SecurityId` values for all entries on
   `C:`, from `FSCTL_QUERY_FILE_LAYOUT`, and the average descriptor size.
   With a cold cache (after a reboot), time `read_folder_security`'s steps
   (open by file ID with `READ_CONTROL | FILE_READ_ATTRIBUTES`, read the
   descriptor, read the USN record) for one folder per distinct folder
   `SecurityId`, and count how often the USN equals the layout's `Usn`. Also
   time the same read for every folder, which is the build's cost when the
   USNs do not match and after a stopped layout pass, and list every folder
   whose open or read fails, with the error.
   Expected: a few thousand descriptors, under 2 s, a match for nearly every
   `SecurityId`, and no failure. If it takes longer, every boot of `C:` waits
   that long before it is live, and the owner decides whether that is
   acceptable. Every failure must be a folder for which
   `is_metadata_folder` is true (`$Extend` or below it), which gozlerd never
   reads. Any other folder that fails on every PC would make every NTFS
   volume show a `Permissions` line and stage 1 check 2 fail, so the plan is
   changed for it before stage 1.
2. **The ReFS cost.** On `V:`, time the folder walk with and without reading
   each folder's descriptor, and count distinct descriptors. Expected: under
   10 % added to the 30 s walk.
3. **The journal and an inherited change.** On `C:` and on `V:`, in a scratch
   tree three folders deep, change the top folder's permissions with `icacls`
   so that the change is inherited, and read the journal with `fsutil usn
   readjournal`. Expected: one `SECURITY_CHANGE` record for each changed
   folder, and each folder's USN, from `fsutil usn readdata`, changed. If
   folders below get no record, the tail reads the subtree (see "The
   journal"). If the USN does not change, step 2 of the NTFS build cannot be
   trusted, and the build reads each folder on its own instead. If ReFS
   writes no `SECURITY_CHANGE` record even for the top folder, the tail never
   learns of a permission change there, spec `V8` and `V15` are false on
   ReFS, and no design in this plan replaces the record: the plan stops and
   goes to the owner.
4. **The check agrees with Windows.** As a standard local test account
   `gozlertest`, which `target\probe\setup-gozlertest.ps1` creates (item
   8), run the probe's check part with
   `runas`. For `C:\Users\<owner>`, `C:\System Volume Information`,
   `C:\Windows\System32\config`, `C:\Program Files\WindowsApps`, the scratch
   tree, and a scratch folder with a no-read-up integrity label
   (`S:(ML;OICI;NR;;;HI)`), compare `AccessCheck` for `FILE_LIST_DIRECTORY`,
   using an identification-level copy of the account's token and the
   descriptor read elevated, with whether `Get-ChildItem` can list the folder
   as that account. They must agree for every folder. A disagreement stops the
   plan and goes to the owner, because spec `V4` would be false.
5. **The access check's cost.** Time 10,000 `AccessCheck` calls over the
   distinct descriptors of `C:`. Expected: a few microseconds each, so a new
   view costs a few milliseconds under the write lock.
6. **What a filtered user loses.** Count the `C:` folders and entries that
   `gozlertest`'s token hides. Stage 2 check 2's bench view hides the same
   share of folders, so the bench measures a real filter, not a guess.
7. **The baseline.** From `gozlerd.log`, record the time from the service's
   start to live for `C:` and `V:` after a reboot, before any change, for
   stage 1 check 3.
8. **The signed-in user when another account runs the setup.**
   `target\probe\setup-gozlertest.ps1` creates `gozlertest` with a random
   password, saved for the owner's account only, and runs this check; only
   the run that creates the account asks for a UAC approval. The steps it
   automates are: in the owner's own session, without signing out, run
   `runas /user:gozlertest "pwsh -NoExit -File
   <repo>\target\probe\signed-in-user.ps1"` and type `gozlertest`'s
   password. Expected: `process account` names `gozlertest`, and `signed-in
   user` names the owner. The probe asks Windows exactly as `signed_in_user`
   will, so this shows that the answer follows the session, not the account
   the process runs as. An elevation approved by another administrator is the
   same case: Windows starts the elevated process with that administrator's
   token, and its window opens on the standard user's desktop, which belongs
   to that user's session. The probe ran on 2026-10-18 in the owner's own
   session and named the same account on both lines, which shows that the
   calls work. If the signed-in user follows the process account, the setup
   cannot find the intended person, and the owner decides whether the page
   asks for a name instead.
9. **The token's security attributes.** The probe, elevated, opens the
   token of its own process and of a packaged app's process (Notepad or
   Windows Terminal, started from the Start menu, found by process ID). It
   copies each with `DuplicateTokenEx` at `SecurityIdentification`, as
   `pipe_client_token` does, and reads the copy's attributes through AuthZ,
   as `rights_key` will. Expected: the packaged process's copy lists
   `WIN://SYSAPPID`, and the plain process's copy does not. If the packaged
   copy lacks it, `rights_key` cannot tell the two processes apart, a plain
   process could share the packaged process's view and see the names in
   `C:\Program Files\WindowsApps`, and the plan stops and goes to the owner.

**Done when:** the results are in `target\probe\visibility-probe-results.txt`,
this plan is updated with them, and each design point that an answer changes
is changed here.

## Stage 1: keep each folder's descriptor

Builds "Descriptor IDs", "Reading descriptors", "The journal" and "Status and
memory". No answer changes: nothing reads the column yet.

**Done when:**

1. `check.ps1` passes, with tests T1 to T4 and T10 to T12.
2. `gozler --status` shows `C:` and `V:` live, with the new `permissions`
   memory part. Index memory is up by 4 bytes per entry plus the table, and
   the table is within 20 % of stage 0's descriptor count times its average
   size. No `Permissions` line shows, so no read failed. (Spec `V9`, `V12`.)
3. After a reboot, `gozlerd.log` shows `C:` live no later than stage 0's
   baseline plus its item 1 time plus 10 %, and `V:` no later than its
   baseline plus its item 2 time plus 10 %.
4. In a scratch folder on `C:` and on `V:`, a new folder and a folder whose
   permissions `icacls` changed each get the folder's current descriptor
   within a second. The proof is a `debug`-level log line from
   `Disk::folder_security` that prints the folder and its descriptor in
   SDDL, from `descriptor_sddl`, compared with `(Get-Acl <folder>).Sddl`. A temporary
   folder that is deleted at once adds no failed read. (Spec `V8`, `V9`,
   `V15`.)

## Stage 2: filter every answer

Builds "The caller", "The stored list", "The setup's choice", "Who sees
what", "The filter", "Folder sizes" and "Text".

**Done when:**

1. `check.ps1` passes, with tests T5 to T9.
2. `pwsh ./check.ps1 -Bench query` runs once before the stage and once after
   it. The `query` bench gains one case with a view that hides the share of
   folders stage 0 item 6 measured. For `SeenBy::Everything`, every case is
   within noise of the run before. For the view, a broad query costs at most
   10 % more. The 10 % is an estimate for one parent lookup and one
   `Vec<bool>` read per candidate; if it does not hold, the numbers go to the
   owner.
3. `C:\Program Files\WindowsApps` lets elevated administrators list it but
   not a non-elevated one, whose Administrators group is deny-only. With
   `count` the result of `gozler -path "C:\Program Files\WindowsApps"
   -get-result-count` (spec `V3`), after `gozlerd see-every-file add
   <owner>` in an administrator terminal, because a hand install stores no
   list (the setup does that in check 11):
   - as the owner, `count` is the same non-elevated and elevated, and not 0;
   - after `gozlerd see-every-file remove <owner>`, the owner's `count` is 0
     non-elevated and not 0 elevated, because no administrator sees every
     file without the list and an elevated one sees what their token may
     list; `add <owner>` restores the full count;
   - as SYSTEM, from a one-time scheduled task, the same command writes the
     full count to a file.
4. As `gozlertest`:
   - `gozler -path C:\Users\<owner> -n all` lists no row, and
     `gozler -parent C:\Users <owner>` lists the `C:\Users\<owner>` row.
   - `gozler -path "C:\Program Files\WindowsApps" -get-result-count` gives 0.
   - `gozler -path C:\Users\<owner>\Documents x` exits 0 with no rows and no
     warning, and with `-no-result-error` exits 9. (Spec `V5`, `V7`.)
   - These runs, each a new `gozler.exe` process, add one view: `gozlerd.log`
     shows one `view added` line for `C:`, and none for a volume the runs
     did not search.
5. In the stage 0 scratch tree, with `gozlertest` denied listing on the top
   folder and allowed on a folder two levels below it:
   - the names `gozler -path <tree> -n all` lists as `gozlertest` equal the
     names `Get-ChildItem -Recurse -Force` lists as that account, plus the
     entries of the allowed folder below the denied one, which gozler shows
     with their full path (spec `V1`, `V2`);
   - after `icacls` takes away `gozlertest`'s right to list the allowed
     folder, a search stops finding its entries within a second; after the
     right comes back, it finds them again (spec `V8`).
6. In the scratch tree, which holds files of known sizes, as `gozlertest`:
   `gozler -parent <tree> /ad -size <top folder name>` shows the top folder
   with no size, `gozler -path <tree> /ad size:>0` lists no folder, and
   `gozler -get-folder-size <tree>\<top folder>` gives the sum of the files
   in the allowed folder only. The owner sees the top folder's size as the
   sum of every file. (Spec `V6`.)
7. A `gozler.exe` built before stage 2, which opens the pipe anonymously,
   prints `gozler: gozlerd could not tell which user is searching; open the
   pipe at the identification level` for a search and exits 7, and its
   `--status` still works. The new `gozler.exe` searches as `gozlertest`.
   (Spec `V10`, `V11`.)
8. `gozler --help` shows the new note, and the help tests pass. (Spec
   `V13`.)
9. `gozlerd see-every-file add gozlertest`, in an administrator terminal,
   makes check 4's `WindowsApps` count for `gozlertest` equal the owner's at
   the next search, with no restart. `remove gozlertest` brings back 0. `add
   Everyone` does the same for every account. Run without elevation, `add`
   fails with the message in "The stored list", and the bare command lists the
   entries. `add S-1-1-0` gives the same entry as `add Everyone`. After
   `gozlerd uninstall` and `gozlerd install`, the bare command prints `The
   list is empty: each user sees the files in folders they can open.` (Spec
   `V16`.)
10. The text in "Text" is changed.
11. The setup, built with `installer\Build.ps1`, on a PC where `gozlerd
    uninstall` has run (spec `V17`):
    - Signed in as `gozlertest`, run the setup and approve it with the owner's
      administrator password. The page names `gozlertest`, not the owner.
      Leave it checked: after the setup, check 4's `WindowsApps` count for
      `gozlertest` equals the owner's.
    - Run the setup again: no page shows, and `gozlerd see-every-file` still
      lists `gozlertest`.
    - Remove it with `gozlerd see-every-file remove gozlertest`, run the setup
      again: no page shows, and the list stays empty.
    - Uninstall, run the setup, and clear the box: `gozlerd see-every-file`
      prints `The list is empty: each user sees the files in folders they can
      open.`
    - Uninstall, run the setup with `/VERYSILENT`: no page, and the list is
      empty.
    - Uninstall, and run the setup in the owner's own session, as an
      administrator: the page names the owner. Leave it checked: check 3's
      first case holds.

## Tests

Each test names the failure it watches.

- **T1. The layout keeps the `SecurityId` and the `Usn`.**
  `LayoutInfoFixture` gains `security_id` and `usn`, written at offsets 44 and
  48, and `arb_info` draws both at random, so
  `roundtrip_fixture_build_then_parse` checks them. A wrong offset would give
  folders each other's descriptors, which no other test and no error shows.
- **T2. A security change reaches the index.** `security_only_emits_nothing`
  pins the old behavior, so it becomes
  `security_change_marks_the_descriptor_dirty`, and
  `unrelated_bits_do_not_mask_matches` expects `SecurityDirty` with the other
  ops. A delete with the bit still yields only `Delete`. The tail's existing
  `reason_mask_covers_every_bit_that_drives_an_index_op` fails until
  `REASON_MASK` has the bit, which watches the kernel request.
- **T3. The build shares a descriptor only when it is safe.** A
  `bootstrap.rs` test of `read_build_descriptors` with a fake reader: three
  folders on one `SecurityId` whose first read returns the pass's USN all get
  one ID; when the first read returns another USN, each folder gets its own
  read; when the first folder is gone, the second is read; folders on
  `SecurityId` 0 are each read on their own. Without it, one
  folder whose permissions changed during the build gives its new permissions
  to every folder that shared its old `SecurityId`, with no error.
  `read_build_descriptors` takes `&mut impl VolumeReads`, so the test's fake
  needs no volume; it answers only `folder_security`.
- **T4. A folder that needs its descriptor is reported.** An `apply_batch`
  test: a new folder, a folder with `SECURITY_CHANGE`, and a file that becomes
  a folder each appear once in `needs_descriptor`; a file with
  `SECURITY_CHANGE` does not. A folder that has a descriptor and gets
  `SECURITY_CHANGE` has `None` after the batch. A folder is given a
  descriptor and deleted, and a new folder created in the same batch takes
  its freed slot: the new folder is in `needs_descriptor`. A new folder below
  a root folder `$Extend` with record number 11 is not in
  `needs_descriptor`, and `set_folder_descriptor` with bytes leaves it
  `None`. Without it, a new folder's entries stay hidden from filtered users
  until the next rescan; a folder that loses a user's permission shows that
  user the names added to it in the same batch; a folder in a reused slot
  shows its names to every user who could list the deleted one; and the
  build can give `$Extend` another folder's bytes, which shows NTFS's
  metadata names to users Windows never lists them for.
- **T5. Every view answers every descriptor.** A `VolumeIndex` test: after
  `add_view`, `set_folder_descriptor` with bytes no folder had before adds a
  `DescriptorId` past the end of the view's `listable`, and `seen_by` for
  that view then answers it, as its `MayList` says. Without it, a new
  descriptor after a view exists reads past `listable` and panics the
  daemon, or a missing answer shows or hides a folder by accident.
- **T6. Every answer leaves out hidden entries.** One engine test over a
  fixture index with a view that cannot list one folder, and one folder whose
  descriptor is `None`. For a query on each scan path (scope walk, folded
  arena, full scan), the rows, `total`, and `run_totals` count and size leave
  out the entries of both folders; under the view every folder has no size,
  `size:` matches no folder, and under `SeenBy::Everything` the same folders
  keep their sums. Without it, one path that skips `Scan::evaluate`, or one
  size read that skips the view, shows hidden data, and nothing else fails.
- **T7. `Everything` changes nothing.** The golden corpora run unchanged with
  `Access::Sees(Visibility::Everything)`. No new test: the existing rows are
  the check.
- **T8. The daemon refuses an unknown caller and filters a view.** A
  `server.rs` test of `serve` with `Access::Refused`: `Search` and `Query`
  get `ProtoError::Unsupported` with the message; `Hello` and `Status` answer.
  A second case runs `serve` with a `Visibility::View` over a fixture index and
  gets only the visible rows. Without it, a change to `serve` can answer an
  unknown caller's search unfiltered, which would let any program bypass the
  filter.
- **T9. Who sees every file.** A `views.rs` test of `sees_everything`: SYSTEM
  as the user with an empty list, the caller's user on the list, an enabled
  group on the list, and Everyone on the list give `true`; Administrators
  among the caller's SIDs with an empty list, and a list that names only a
  group the caller does not hold, give `false`. Without it, a wrong match
  turns the filter off for users the administrator did not list, or leaves it
  on for one they did. Deny-only groups are left out by `enabled_sids`, which
  stage 2 check 9 covers with a real token.

- **T10. A malformed USN record returns an error.** A proptest over arbitrary
  bytes for `record_usn`, as `AGENTS.md` asks of a new parser, and one fixture
  each of a V2 and a V3 record whose `Usn` it returns. Without it, a short or
  odd record from `FSCTL_READ_FILE_USN_DATA` could panic the daemon during a
  build, or a wrong offset would make every USN differ, which silently turns
  the build's sharing off and makes every boot read each folder.
- **T11. A settle reads descriptors first.** A `settle.rs` test with a fake
  `VolumeReads` that records the order of its calls: over a batch that has a
  folder with `SECURITY_CHANGE`, a missed folder, a changed file and a file
  whose links changed, every `folder_security` call comes before any
  `list_folder`, `stat` or `links` call. Without it, a change that moves the
  descriptor step after the walk or the link step keeps a folder's names
  visible, for seconds, to a user who just lost permission to list it.
- **T12. A stopped build pass leaves its gaps visible.** A `bootstrap.rs`
  test of `build_ntfs` with a fake `NtfsReads` that records its calls and
  needs no layout or ENUM bytes. When `layout_page` fails at once,
  `metadata_pending` is true, `enum_page` is called, and the root's folder
  size stays unknown. When `layout_page` gives `Ok(None)` at once,
  `metadata_pending` is false, `enum_page` is not called, and the root's
  folder size is known. Without it, a change that sums folder sizes after a
  stopped pass reports short sums as exact. It does not watch that ENUM
  records reach the index, which core's parser tests cover. The existing
  `layout_pass_reports_a_failed_read_as_stopped` moves to the same fake.

Not tested in code: `pipe_client_token`, `enabled_sids`, `rights_key`, `may_list` and `parameters.rs`. They only call
Windows, and a test would need a second account or a registry write. Stage 0
item 4 and stage 2 checks 3 to 9 cover them.

## Costs

| Cost | Size | Source |
|---|---|---|
| Descriptor column | 4 bytes per entry, 8.1 MB on `C:` | computed from the measured entry count |
| Descriptor table | distinct descriptors times their size; estimate under 2 MB | stage 0, item 1 |
| One view | 1 byte per descriptor | stage 2 logs the size when a view is added |
| Boot time | one open per distinct `SecurityId` on NTFS; one more call per folder on ReFS | stage 0, items 1, 2 and 7 |
| Adding a view | one `AccessCheck` per descriptor, under the write lock | stage 0, item 5; stage 2 logs the time |
| A query | one parent lookup and one `Vec<bool>` read per candidate for a view; nothing for `Everything` | stage 2, check 2 |
| A search request | one registry read and one token read | estimate: microseconds; stage 2, check 2 includes it |

## Not verified

- **The descriptor count on volumes unlike the owner's.** A volume with
  explicit permissions on each of millions of folders would make the table
  and each view large. `--status` shows the table's memory. Checked: stage 0,
  item 1, on the owner's volumes only.
- **Integrity labels.** That `AccessCheck` applies a no-read-up label as the
  file system does. Checked: stage 0, item 4.
- **Inherited changes in the journal, and the USN after a permission
  change.** Checked: stage 0, item 3.
- **The stored list across a reinstall** (that `DeleteService` drops it).
  Checked: stage 2, check 9.
- **The token's security attributes through AuthZ.** Microsoft documents
  `AuthzContextInfoSecurityAttributes`, but not which attributes a process
  token carries. Checked: stage 0, item 9.
- **The signed-in user when another administrator approves the UAC prompt.**
  Microsoft documents that `WTSUserName` is the session's user, but no page
  found says that the approved process stays in the standard user's session;
  that it does follows from its window opening on that user's desktop.
  On 2026-10-18 `target\probe\setup-gozlertest.ps1` ran the probe as
  `gozlertest` in the owner's session 1: `signed-in user` named the owner,
  so the answer follows the session, not the process account (stage 0, item
  8, passed). That the UAC-approved process runs in the same session is still
  inferred. Checked: stage 2, check 11.
- **Central access policies** (spec, "Not covered"): not checked, because the
  owner's PCs have no domain.

## Expert reviews

- **Before stage 1 lands:** a code review of `index/volume.rs`,
  `index/store.rs`, `index/descriptors.rs`, `read_build_descriptors` and the
  tail, because every journal record goes through that code.
- **Before stage 2 lands:** a security review of `caller.rs`,
  `parameters.rs`, `views.rs`, the refusal in `serve`, `Scan::evaluate` and
  `EntryView::size`. These decide which names each user reads, and one wrong
  path shows every name to every user with no error.

## When the plan is done

When stage 2 lands, its rules move into `docs/DESIGN.md`,
`docs/SECURITY.md`, `README.md` and `CONTEXT.md`, as "Text" says, and this
plan and its spec are deleted. The probe in `target\probe\` is deleted with
them.
