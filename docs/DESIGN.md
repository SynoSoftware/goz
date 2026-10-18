# Design

## Workspace

gozler is four crates:

- **`goz-core`**: pure, OS-agnostic. USN/MFT parsing, the index, the query engine, the wire protocol, CSV output, and `es`-compatible argv parsing. Tests run on any OS.
- **`goz-winfs`**: the thin `unsafe` Win32 layer (`windows-sys` only). Buffers, typed structs, and opaque handles; it never parses an ioctl record payload. It depends on nothing else in the workspace, enforced by cargo-deny.
- **`goz-daemon`** (`gozlerd`): bootstrap, USN journal tail, the tokio named-pipe server, and Windows service integration.
- **`goz-cli`** (`gozler`): the `es`-compatible CLI and the blocking named-pipe client.

## Index coherence

gozler indexes fixed NTFS volumes; removable drives and non-NTFS filesystems (ReFS, exFAT, FAT32) are out of scope and are not tracked. The set of indexed volumes follows the machine: within a minute, a fixed NTFS volume that appears (an attached VHD, an unlocked BitLocker drive) is indexed, and a volume that is removed leaves the index. A `-path` scope on no indexed volume is an error, never an empty answer.

The daemon answers queries from the moment it starts. A volume is `bootstrapping` until its first build finishes, and like `rescanning` it marks results incomplete; a volume that cannot be opened is retried on the same budget as a tail that exits, then reported `failed`.

The index is a cache of the filesystem, and a cache that cannot prove it is coherent must rebuild itself. Every sync-loss path (journal wrap, a parse anomaly, a journal that cannot be read at startup, reads that stop answering, even a sick disk that leaves a journal read pending forever, which a supervisor cancels, or a tail thread that exits outright, which the same supervisor revives on a bounded budget) ends in one of three states:

- **in sync**, provably (phase `live`);
- **rescanning**, with a full rescan scheduled (phase `rescanning`); or
- **offline** (phase `offline`, or `failed` once retrying has stopped), reported as no longer tracking. A volume that can never have a journal is `snapshot`: complete as of bootstrap, and said so once in `--status` instead of on every result page.

There is never a silent fourth state.

Some irregularities do not lose sync and are not rescanned. A delete for a file ID the index does not hold has nothing to remove. A record number that NTFS has given to a new file has its stale entries removed in place. Both are normal while the journal replays changes made during the MFT scan, so a rescan would rebuild the same index; they are counted, and `--status` shows the counts. An entry whose walk to the root is longer than the hop limit has no full path and is left out of results. That happens briefly when the replay applies an old move on top of the newer scan, which can close a loop in the parent links until the later moves arrive, and permanently for a real folder tree that deep, which a rescan would rebuild the same way. Neither case starts a rescan. `--status`, the daemon handshake, and every result page decide a volume's state with a single shared rule, so a client can always tell whether the results it just got are complete.

## Hard links

A hard-linked file is one record with several names, and a USN record names only one of them.

- **Everything 1.4** never sees a hard link created after its index was built. Per the [Everything documentation](https://www.voidtools.com/support/everything/), *"changes made to a hard link will only update the first hard link entry."* Only a manual **Tools -> Options -> Indexes -> Force rebuild** picks it up. This is a long-standing, documented limitation.
- **gozler** reconciles hard links live. A hard-link-change record names only one link (and may name a dead one), so gozler walks the file's real link set over Win32 (`FindFirstFileNameW` / `FindNextFileNameW`) and reconciles the index to exactly that set: names added or removed are applied, not just size and dates. A walk that cannot complete (file gone, locked, or a parent not yet indexed) is skipped rather than applied as an empty set, counted, and surfaced by `--status`, and a rescan trues it up. The gap is visible, never silent.
- **Everything 1.5** (beta) does track new and renamed hard links.
