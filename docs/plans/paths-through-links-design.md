# Design: searching through junctions and folder links

Historical design A, replaced by [the chosen plan](paths-through-links.md).

Goal (owner): `artifacts\evidence` finds what is under
`C:\SynoSoftware\TinyTorrent2\artifacts`, a junction to
`V:\artifacts\TinyTorrent2`. The same must hold for every folder a person can
`cd` into.

Recommendation in one sentence: gozlerd keeps a small table of the links on its
volumes, and a path test passes when the real path of an entry, or one of the
paths that reach it through a link, passes it. This one mechanism in the
daemon also covers the full-path case, so the client rewrite
(`gozler_client::follow_links`) is deleted.

## 1. Where gozler learns about links and their targets

- The index already knows which entries are links. Every entry keeps its
  attributes, and L (reparse point), H and S are among them
  (`PackedAttributes`). The index does not keep a link's target.
- Recommended source of the target: after a volume's build, and for each
  journal record of a reparse-point change, create, rename or delete of a
  folder with L, the daemon opens that folder by file ID with
  `FILE_FLAG_OPEN_REPARSE_POINT` and reads `FSCTL_GET_REPARSE_POINT`. It works
  the same on NTFS and on ReFS: the ReFS walk already lists a link with its
  reparse tag and does not enter it. A machine has tens to a few hundred
  folder links, so this is a few hundred opens per build, on the volume's
  tail thread, never in a query. The Win32 call goes in `gozler-winfs`, and
  the reparse buffer parser goes in `gozler-core` (untrusted bytes:
  `zerocopy`, checked lengths, a proptest that bad bytes give an error).
- Rejected alternative: `FSCTL_QUERY_FILE_LAYOUT` with
  `QUERY_FILE_LAYOUT_INCLUDE_STREAM_INFORMATION |
  ..._FOR_REPARSE_ATTRIBUTE` (winioctl.h) may put the reparse data in the
  build pass that the index already runs. Nobody has checked that it returns
  the buffer or on which Windows builds, and it covers NTFS only, so ReFS
  would still need the reads above. It is two ways to do one thing.
- Which links count (revised for the owner's rule "if I can cd to it, it
  must find it"): every folder link a person can `cd` through.
  - A junction (`IO_REPARSE_TAG_MOUNT_POINT` to a drive path) and a folder
    symbolic link (`IO_REPARSE_TAG_SYMLINK`; a relative target is resolved
    against the link's folder).
  - A folder that mounts a volume. Its target is `\??\Volume{...}\`, and
    the daemon's volume list (`VolumeMounts`) already names every mount of
    each volume, so each mount other than the one the volume's paths print
    with becomes a path to that volume's root. Today only a scope can use
    such a mount; a path term cannot.
  - Hidden and system links too, such as Windows' compatibility junctions
    (`Documents and Settings`, `Application Data`), because
    `cd "C:\Documents and Settings\user"` works. Section 2 says why this
    does not flood a one-word search.
  - Not followed: a target on a share or on a volume the index does not
    hold, because the index has nothing to find there.

## 2. Where matching happens: one mechanism, one owner

- The owner is `gozler-core`'s query engine, run in gozlerd. The daemon gives
  each query the link table it holds for all volumes, because a link on C:
  can point into V:.
- At compile time, each link becomes a reach entry: its target folder, found
  in the index by its path, and the prefixes that reach it, the link's own
  real path and the paths that reach the link through other links (see 5).
- `DirPaths` already builds each folder's path once per query from its
  parent's, and `ComponentBits` passes component results down the same way.
  Each folder also inherits its parent's list of reach entries, and the
  target folder of a link adds that link's. For almost every folder the list
  is empty.
- An entry passes when one of its paths passes every component clause and
  every path clause: first its real path, as today; only when that fails and
  its folder's list is not empty, each path through a link (the prefix, then
  the real path below the target). One path must pass all of them, as a
  folder index in Everything would test each path on its own. This covers
  every path matcher alike: substring, whole word, word start, start,
  wildcard and regular expression.
- Which tests use the paths through links (revised): every test that holds
  a separator or a drive, which is text that names a path a person can
  open, and the scope, Location and `parent:`. A one-word test that Match
  path or `-p` makes a path test (`ComponentBits`) matches real paths only.
  A single word names no folder to open. Through links, a word that names a
  link folder would list every entry below its target: with Windows'
  compatibility junctions, `settings` would list all of
  `C:\Users\…\AppData\Local` through `Local Settings`. This also keeps the
  component walk and the `startwith:` volume skip as they are.
- Scope, Location and `parent:`: `resolve_scope` looks up each component as a
  child folder. When it reaches a folder that is a link in the table, it goes
  on at the link's target, also on another volume. The table answers this
  from memory, so the daemon opens nothing on disk.
- The full-path case `C:\…\artifacts\evidence\` is a path through a link, so
  the same test finds it. `follow_links` in `gozler-client`, its two calls in
  `Client::rows` and `Client::totals`, its test and its DESIGN bullet are
  removed in the same change.

## 3. How it stays fast

- A query never opens a file. The link table is in memory and changes only
  when the journal reports a link change.
- A query without a path term or `-p` pays nothing: name terms, the folded
  arena prefilter and the filters do not read the reach lists.
- A query with a path term pays, once per query, one index lookup per link
  to find its target folder, and per folder one inherited list, almost always
  empty. Only entries below a link target pay one more path copy per reach
  prefix, and only when their real path failed.
- I will measure with the criterion benches before and after
  (`-p kernel32`, a path term with a separator, and a name-only query) and
  report the cycles.

## 4. What the results show, and the bold match

- Recommended: the real path, as today, and each entry once, even when
  several paths reach it. The index, the selection, the sort by path and the
  file operations all use the real path, so the rows stay consistent with
  them. The owner's first screenshot shows this form
  (`V:\artifacts\TinyTorrent2\evidence\…`).
- Bold: the window computes the matched parts itself from the shown search
  (`Results::matched_parts`). It cannot know the links today. Recommended:
  the status reply that the window already asks for carries the reach table,
  and `MatchedParts` tests the same paths the engine tested. A part that
  falls below the link target is bold in the real path: for
  `artifacts\evidence`, `\evidence` is bold in
  `V:\artifacts\TinyTorrent2\evidence\Stall-logs`. A part that falls inside
  the link's own path, such as `artifacts`, has no characters in the real
  path, so it is not bold.
- Alternative, not recommended: show the path through the link
  (`C:\SynoSoftware\TinyTorrent2\artifacts\evidence\…`). The bold would then
  be complete. But every row would need to carry its link, the sort by path
  would no longer follow the order of the shown paths, and Open location and
  the file operations would use a path that the index does not hold.

## 5. Loops and links inside link targets

(Revised for the owner's rule.)

- A link inside another link's target gets the paths of both, at any depth:
  its prefixes are its real path plus each prefix of every link whose target
  holds it.
- A loop is followed, because a person can `cd` through it:
  `C:\ProgramData\Application Data\Application Data\Microsoft` is
  `C:\ProgramData\Microsoft`. Its paths never end, so the query decides how
  many passes through a loop are tested. Each pass adds the loop's text
  (`\Application Data`) to the path, so a test can span only as many passes
  as its own literal text is long. The engine generates the prefixes of a
  loop, once per query, up to the passes that the longest path test of the
  query can span, plus one. Every path that the query's text can name is
  then tested, and no more.
- A regular expression is the one case where the rule is not met in full.
  Its length does not bound the passes it needs: `(Application Data\\){5}`
  needs five. Windows stops a path at 63 reparse points, but testing up to
  63 passes would mean 63 path copies for each entry below
  `C:\ProgramData`, so a regular expression gets the passes that its
  literal text allows, and at least two. DESIGN says so.
- Two links that point into each other are the same case: a loop of two
  links.

## 6. Pipe payload

- `SearchRequest` and the rows do not change.
- With the recommended bold, `DaemonStatus` gains the reach table: for each
  followed link, its target's full path and its prefixes. So
  `PROTO_VERSION` goes from 16 to 17, and no old form is kept.
- Without that bold, the protocol does not change at all.

## Builds

- No build until the reviewer approves this design.
- During the work: one release exe for the owner to try, after I check the
  running `build.ps1` and `cargo` processes and tell the reviewer.
- The full `build.ps1` runs once, when the owner asks for the commit.
