# Paths through links: historical handoff

This records the paused implementation before the replacement. Current code,
review and verification are recorded in [the chosen plan](paths-through-links.md).

Status: paused at the owner's request. Nothing is committed. The code below
has **never compiled**. The one build started was stopped while it compiled
`gozler-core`, so no compile error or test result exists.

The chosen design is [paths-through-links.md](paths-through-links.md). It
replaces the matching described here ("design A", in
[paths-through-links-design.md](paths-through-links-design.md)). Keep the
link sources (sections 1 and 2 below). Replace the matching (section 3), and
review the protocol, the window and DESIGN (sections 4 to 6) against the plan.

Design A has a known defect that the plan removes: past `MAX_REACHES` (4,096
paths) it stops adding paths, logs a warning and still answers as if the
results were complete.

## 1. Reading link targets (keep)

- `crates/gozler-winfs/src/ioctl.rs`: adds `FSCTL_GET_REPARSE_POINT`
  (`0x000900A8`, asserted in the existing test) and `ioctl_out_bytes`.
- `crates/gozler-winfs/src/file_id.rs`: adds `read_reparse_point(volume, id)`.
  It opens the file by ID with `FILE_FLAG_OPEN_REPARSE_POINT` and reads up to
  16 KiB. It gives `Ok(None)` for a file that is gone or is not a reparse
  point. `lib.rs` re-exports it.
- `crates/gozler-core/src/links.rs` (new, `pub mod links` in `lib.rs`):
  - `parse_reparse_data` reads a `REPARSE_DATA_BUFFER` through `zerocopy` and
    gives a `ReparseTarget`: `Absolute("V:\x")` or `Relative("..\x")`.
    `\??\Volume{…}`, `\??\UNC\…` and other tags give `None`.
  - `FolderLink { link, target }`: both are full paths, without a trailing
    separator, and a root is written as `V:`.
  - `Links`: the sorted table. `new` writes each target as its real path.
    `real_path(path)` follows links from the left, at most `MAX_LINK_HOPS`
    (63) times, comparing folded names. `lead_out_of(folder)` says whether a
    link below a real folder leads outside it. `fingerprint()` is an FxHash
    of the table.
  - Tests: the parser on junction, symlink, relative and refused targets; a
    proptest that any bytes parse without a panic; and `real_path`.
- `crates/gozler-core/src/index/volume.rs`: the field
  `folder_links: FxHashMap<Frn, ReparseTarget>`, and `reparse_folders()`,
  `is_folder_link()`, `set_folder_link()` and
  `folder_links(mount_prefix, may_list)`. The last gives full link paths and
  resolves relative targets against the link's parent. With `may_list`, a
  filtered user's check of a folder's descriptor, it gives only the links in
  folders that user may list.
- `crates/gozler-daemon/src/tail.rs`:
  - `rescan` reads every reparse folder's target before the new index is
    shared (`read_folder_links`).
  - After each journal batch, `refresh_folder_links` reads again each folder
    record that has the reparse attribute or is a known link. It reads
    without the lock and writes under `index_mut()`.

Measured on this machine, through PowerShell on the reparse folders that
`gozler attrib:dl` lists: 253 links (93 junctions, 160 symbolic links, most
of them boost's relative symlinks). As JSON they take about 32 KB.

## 2. Collecting the table (keep, adapt)

`crates/gozler-daemon/src/server.rs`, `folder_links(volumes, viewer) -> Links`:
each volume's `folder_links`, plus each mount of a volume after its first, as
a link to the mount its paths print with. It is built on every search and
every status request. `Links::new` folds each link once (`folded`/`follow`),
so building the table costs no allocation per pair of links.

## 3. Matching, design A (replace)

- `crates/gozler-core/src/query/through.rs` (new): `Through` keeps, for each
  target, the paths that reach it (`Reach`). Paths are deduplicated by a
  `Crossing` key for each test. A worklist expands them, a wildcard or a
  regex passes each link at most once, and the total is capped by
  `MAX_REACHES`.
- `crates/gozler-core/src/query/engine.rs`:
  - `CompiledQuery::with_links(parsed, links, scope)`, plus `search_scope`
    and `links_truncated`.
  - `CompiledTerm::through` and `CompiledClause::passes_through`.
  - `VolumeTargets`, the interned set of targets above each folder, used in
    `Scan::evaluate` only when the real path fails.
  - `MatchedParts::with_links`.
  - Several items became `pub(super)`, and `last_code_point` was split out of
    `occurrences`.
  - The test `a_search_through_a_junction_finds_what_is_below_its_target`.
- `crates/gozler-core/src/query/rows.rs`: `targets(..)` takes
  `query.search_scope(scope)`.
- `crates/gozler-core/benches/query.rs`: the case
  "path-term(group_007\bin_x64_03) one link".

## 4. Protocol (review against the plan)

- `PROTO_VERSION` is 17.
- `Request::Status { links_held: Option<u64> }`. `None` is sent by a client
  that does not use links: `gozler` and fileops.
- `DaemonStatus.links: Option<Links>` is sent only when the fingerprint
  differs, and is skipped in JSON when it is `None`.
- `Client::status(links_held)`. Callers are updated in the CLI and fileops,
  and the test literals get `links: None`.

## 5. Window (review against the plan)

- `core/model.rs` keeps `links: Arc<Links>` and asks for status with its
  fingerprint. When the table changes, it marks the matched parts again and
  redraws the Search list.
- `core/fetch.rs`: the variant is now `Ask::Status { links_held }`.
- `core/results.rs`: `mark_matches(links)`.
- `app/work/requests.rs`: passes the fingerprint.

## 6. Other

- `crates/gozler-client`: the first fix (`follow_links`, which rewrote paths
  in the client) is fully removed. Only the `status` signature changed there.
- `docs/DESIGN.md`: the "Paths" bullet describes design A, with loops, the
  one-word rule, the cap and the filtered user. Rewrite it for the chosen
  design. One line in the visibility section no longer says "PROTO_VERSION
  is 16".
- `rustfmt --check` passes on every file I touched. I ran rustfmt only on
  files whose every reported diff was mine.

## What a new session must know

- Other sessions' uncommitted work shares this tree: the `gozler-ui` `win/`
  painting and viewer files, `present.rs`, `folder_tree.rs` and others.
  Commit only this work's files, and only when the owner asks.
- The tree has not compiled. Read the plan first, then expect compile errors
  in `through.rs`, `engine.rs` and the window's status path.
- Never stop or replace the installed goz service. gozler's own service
  installs beside it.
