# Rows for deep folders

Studied on 2026-10-20 for issue #84 ("stream a folder's entries as a
tree"), which describes the problem: a search over 5.5 million entries in
folders 200 deep. The code was read at commit `acc5696`. The numbers for
`C:\` were measured on the owner's laptop from one export of every full path
(`gozler -path C:\ -n all -export-txt`, 2,151,043 rows, 5.6 s). The numbers
for the deep tree are computed, not run.

**Status.** Stages 1 and 2 are built: protocol 9, and the path sort by
folder rank (`query::order::folder_ranks`). The wire rules are in
`docs/DESIGN.md` ("Wire protocol"). "Where the cost is today" describes the
code before them. Since then, a sorted index has replaced the copied hits
that this study measured: a reply holds a 16-byte row for each match and
reads each page's names from the index ("Result order" and "Rows on demand"
in `docs/DESIGN.md`); "The deep tree, measured" gives its figures.

## Answer

The pipe is not the problem. The problem is that every row carries its full
path, and gozlerd builds the full path of every row of a reply before it
sends the first page. A full path grows with the folder depth, so the
memory, the pipe bytes and the time all grow as rows × depth. For the tree in
#84, that is about 12 GB that gozlerd holds at once, and a failed allocation
stops gozlerd.

I recommend two changes, in this order:

1. **A results page that sends each folder once, in protocol 9.** Inside
   gozlerd, each matched entry is kept as its name and its folder, never as
   its full path. On the pipe, a folder travels as its parent and its name,
   the first time a row needs it, and a row travels as its folder and its
   own name. Then gozlerd holds about the size of its index at any depth. On
   `C:\` the pipe carries 73 MB instead of 251 MB. For the deep tree it
   carries about 0.1 GB instead of 12 GB, because no byte on the wire depends
   on the depth. `gozler-client` builds the full paths, so its callers and the
   command line's output do not change. The page replaces today's page, and
   gozlerd stops serving the older protocols (see "What this changes in the
   contracts").
2. **Sort by path with one rank per folder** instead of comparing full
   folder paths. Today a path sort holds a copy of every distinct folder's
   full path, with no limit, under the index read lock.

What #84 asks for is then a scoped search that asks for this page with no
sort; it needs only a "no order" choice (stage 3 below).

## Where the cost is today

Read from the code. *L* is the length of a full path, *N* the matches and *F*
the distinct folders that hold them.

| Step | Code | Memory | Under the read lock | Grows with depth |
| --- | --- | --- | --- | --- |
| Find the matches | `scan` | 8 bytes per match (`Candidate`) | yes | no |
| Build the rows | `run_query_impl` builds a `QueryHit` with its own `path: Vec<u8>` for every row of the reply | N × (L + about 96) | yes | **yes** |
| Folder paths for those rows | `DirPaths` caches each folder's path once | bounded: 64 MB, and no path longer than 1,024 bytes is cached | yes | the cache does not, but a folder deeper than 1,024 bytes is rebuilt from its nearest cached ancestor for every row |
| Path sort of a slice | `FolderPaths` in `run_query_impl` | F × L, **no limit** | yes | **yes** |
| Path sort of a whole reply | `FolderPaths` in `order_hits`, the same store, keyed by volume and folder file ID | F × L, **no limit** | no | **yes** |
| Send | `stream_rows` sends pages only after all rows are built and sorted | one page in flight | no | **yes**: L bytes per row on the pipe |
| Receive | `gozler-client` decodes page by page into `ResultItem`s | one page | no | yes, per page only |

So the first row of a full export waits for every full path. The page
framing itself (the length prefix, the 16 MiB frame cap and 4,096 rows per
page) has no problem with depth: one row is at most about 98 KB.

## Measured on `C:\`

2,151,043 rows, 258,227 distinct folders, mean depth 9, deepest 28, longest
full path 451 bytes. Only the path part of a row is counted, because the
other fields cost the same in every format.

| Format | Bytes on the pipe | Needs full paths in gozlerd |
| --- | --- | --- |
| Today: length and full path | 251 MB | yes |
| Front coding (each path as its shared prefix with the row before), path order | 48 MB | yes |
| Front coding, name order | 142 MB | yes |
| Each folder once, each row as folder and name | 73 MB (66 MB rows, 7 MB folders) | **no** |

The mean name is 22.6 bytes and the mean full path 112.8 bytes. So even on a
shallow volume, 80 % of the bytes on the pipe are the folder part of each
path, which every row in the same folder sends again.

## The deep tree in #84, computed

5.5 million entries, each 200 folders deep, with names of 10 ASCII
characters, so a full path is about 200 × 11 = 2,200 bytes. #84 says 4 KB
and 22 GB. That counts 2 bytes per character, but paths travel and are held
as WTF-8, which uses 1 byte for an ASCII character. The figures below use
2,200 bytes. Names in a language that needs 2 or 3 bytes per character make
every figure that grows with depth 2 or 3 times larger.

| | Today | Recommended |
| --- | --- | --- |
| gozlerd memory for a full export | 12.6 GB (12.1 GB of paths, 0.5 GB of `QueryHit`s) | about 0.55 GB: about 100 bytes per row for the name, the folder and the fields |
| … sorted by path | about 13.8 GB: `FolderPaths` adds a folded path for each of 550,000 folders | about 0.55 GB, and about 70 bytes per folder, about 40 MB, while the ranks are computed |
| Bytes on the pipe | 12.1 GB | about 0.11 GB: 99 MB of rows and, at 10 entries per folder, 10 MB of folders |
| First row of a full export | after every full path is built and sorted | after the matches are copied and sorted |
| The window, 32,768 cached rows | 72 MB of paths, against a budget of 16 MB for the whole process | unchanged by stages 1 and 2; see stage 4 |

The next section measures these figures, and the full export takes about
three times the memory computed here.

## The deep tree, measured

Measured on 2026-10-20 on the owner's laptop, on `acc5696` (before) and
`fe10004` (after stages 1 and 2). A probe ran gozlerd's own steps for one
volume, as each version's `stream_rows` ran them: the search, the merge of
the volumes, and the page loop. It counted the peak heap above the built index with
a counting allocator. The tree is a copy that copied into itself: a chain of
190 folders, then branches of 200 folders with 9 files each, so a file's
path is about 2.1 KB. The empty search matched every entry. The old code
could run only at 1/20 of the size of #84, because the machine has less free
memory than it needs.

At 1/20: 280,330 entries, an index of 39 MB.

| Search | Before | After |
| --- | --- | --- |
| Full export by name | 1,189 MB (4.4 KB per row), 4.4 s, 571 MB on the pipe | 72 MB (271 bytes per row), 0.56 s, 7.3 MB on the pipe |
| Full export by path | 2,208 MB (8.3 KB per row), 10.4 s | 73 MB (271 bytes per row), 0.43 s |
| First 16,384 rows by name, as the window asks | 69 MB, 0.28 s | 6 MB, 0.04 s |
| First 16,384 rows by path | 147 MB, 0.74 s | 9 MB, 0.08 s |

At the full size of #84, after only: 5,602,990 entries, an index of 794 MB.

| Search | After stages 1 and 2 | After `order_hits` keeps one list of hits |
| --- | --- | --- |
| Full export by name | 1,506 MB (282 bytes per row), 10 s, 146 MB on the pipe | 1,017 MB (190 bytes per row), 7.8 s |
| Full export by path | 1,508 MB, 10.3 s | 1,020 MB, 5.5 s |
| First 16,384 rows by path | 182 MB, 1.2 s, all under the index lock | unchanged: `order_hits` only merges a sorted page |

The old code grows by row at the same rate, so at the full size it would
need about 25 GB for the export by name and 46 GB by path.

With the sorted index, measured with the same steps in
`crates/gozler-daemon/examples/deep_export.rs`, on an index of 667 MB: the
time is the daemon's part, from the scan to the last page.

| Search | Before the sorted index | With it |
| --- | --- | --- |
| Full export by name | 1,017 MB, 6.1 s | 183 MB (34 bytes per row), 1.2 s |
| Full export by path | 1,020 MB, 5.2 s | 183 MB, 1.5 s |
| First 16,384 rows by path | 182 MB, 0.75 s | 179 MB, 0.84 s, of which 0.53 s under the index lock |
| Full export at 1/20, by name and by path | 50 MB | 8 MB, 0.04 s and 0.06 s |

The first page by path still copies every match's folder to rank the
folders, so its peak stays near the full export's.

After stages 1 and 2, `order_hits` copied every `QueryHit`, about 96 bytes,
into a merged list, then again into the result order, so two lists of hits
were alive at each copy. It now takes the first volume's list as the merged
one and moves each hit into its place in that list. What remains is about
190 bytes per row: the `QueryHit`, its copied name, the sort keys and the
positions. A full export still holds every row, because each search sorts
its matches and keeps them until they are sent.

## Options

| Option | Pipe bytes | gozlerd memory | Cost | Verdict |
| --- | --- | --- | --- | --- |
| A. Build full paths per page inside gozlerd only | unchanged | about index size | gozlerd only | No: C does the same inside gozlerd and builds no full path at all |
| B. Front coding | depends on the order: 48 MB or 142 MB on `C:\` | still builds every full path to find the shared prefix | protocol | No: C is smaller in every order and builds no full path |
| C. Each folder once, rows as folder and name | 73 MB on `C:\`, no growth with depth | about index size | protocol 9 | **Do it** (stage 1) |
| D. Compress each page (zstd) | smaller | still builds every full path | a new dependency in `gozler-core`'s allowlist, CPU on both sides | No: it leaves the memory problem |
| E. The result snapshot (`docs/plans/result-snapshot.md`) | unchanged | adds 13 bytes per row | the plan's own | Not for this problem: it serves the window's 100,000-row limit. C makes its row reads cheaper too |

## The recommended design

### The page (protocol 9)

The results page (`RESP_TAG_RESULTS`) keeps its header and its trailing
sums, gains a folder section before the items, and each item carries its
folder and its name instead of its full path:

```text
total, generation, page_flags   as today
folder_count: u32               folders this page adds
folder_count folders, each:
  parent:    u32                the reply's id of its parent folder;
                                u32::MAX for a volume's root
  name_len:  u32
  name:      name_len bytes     WTF-8; for a volume's root, its mount prefix
                                without the last separator
count: u32                      items, as today
count items, each:
  folder:    u32                the reply's id of the item's folder
  name_len:  u32
  name:      name_len bytes     WTF-8
  iflags, size, mtime, attributes   as today
sums                            as today
```

- **A folder's id is its position in the reply.** The first folder that the
  reply sends is 0, the next 1, across every page of the same reply. So the
  id is not sent, and the client stores the folders in a `Vec`.
- **The decoder checks every id against the folders it has.** A parent must
  be lower than its folder's own id, and an item's folder must be a folder
  already sent. Bytes from the pipe can be anything; with these two checks a
  malformed page is an error and can never make a loop of folders.
- **A folder is sent before the first row that needs it,** and its parent
  before it. A page holds the folders that its rows need and that no earlier
  page sent. The frame cap still applies as `ResultsPage::push` applies it
  today: a row whose new folders would pass the cap starts the next page.
- **Ids are for one reply only.** The client drops its folders when the
  reply ends. Each chunk the window asks for is its own reply, so it has
  its own ids. No state lives in gozlerd after the reply.

### gozlerd

- Under each volume's read lock, after the scan, copy each match's name,
  its folder's `EntryIdx` and its fields into one buffer, and copy each
  folder that the matches need, with its ancestors, once: its parent and
  its name. The walk up stops at a folder already copied, so each folder is
  visited once, at any depth. The hop limit and lost+found keep their rule:
  an entry without a full path is left out, as today.
- After the lock, order as today, then encode page by page. A row whose
  folder has no id yet gives its folder, and its ancestors that have none,
  the next ids, and those folders go into the page first.
- gozlerd builds no full path for a row. It still builds full paths where
  a search term tests the full path, as today, and those stay inside the
  scan.

### gozler-client

The client decodes the page into the same `QueryResults` and
`ResultItem`s as today: it joins the names on the walk up from the row's
folder, with a bounded cache of folder paths. So `gozler-cli`, the window
and `gozler_core::output` do not change, and the command line writes the
same bytes. Joining raw WTF-8 at `\` is exact, because a separator always
sits between two names, so no surrogate pair is ever split.

### Path order with one rank per folder

The path sort is es's order: the folded full path of the parent folder, then
the name. Two folders compare as their folded path strings, and that order
is not a plain walk of the tree. `C:\a` comes before `C:\a b`, but `C:\a\x`
comes after it, because a space (0x20) sorts before `\` (0x5C).

The order can still be computed from the tree without building any path:

- Give each folder two keys among its siblings: its folded name followed by
  "end", for the folder itself, and its folded name followed by `\`, for
  everything below it. "End" sorts before every byte.
- Sort each folder's keys, and walk the tree in that order to give each
  folder its rank.

No key can be the start of another, because a name holds no `\`, so the keys
decide the order exactly as the strings do. A row then sorts by its folder's
rank and its name. Each folder's rank costs 4 bytes, and the tree that the ranks
are computed from holds about 70 bytes per folder while it lives, against
F × L before.

Two exceptions need care when this is built:

- Two sibling folders whose folded names are equal, which only a
  case-sensitive folder holds, compare equal today, so they must share a
  rank, and the folders below them must merge.
- A volume mounted in a folder of another volume puts its paths among that
  volume's paths. Its tree must be ranked as if it hung at its mount folder.

## Stages

Each stage is proven without the window.

| Stage | Change | Proof |
| --- | --- | --- |
| 1 | Protocol 9: gozlerd keeps names and folders and sends the folder page; the client builds full paths; the older protocols go | a round trip of mixed pages; a proptest that malformed pages return an error (bad ids, loops, truncation); the command line's golden output unchanged; a bench over a deep fixture (the `deep_index` builder in `query::paths` scaled up) shows memory and time against today |
| 2 | Path order by folder rank; remove `FolderPaths` | a proptest over random trees with names around the `\` and space case: the rank order equals today's string order |
| 3 | Only if #84 is scheduled: a "no order" choice that sends a scope's entries in the order of the subtree walk | the first page arrives without a sort; the delete takes its list from it |
| 4 | Only if the owner wants the window's 16 MB budget to hold for deep trees: the window caches chunks as folders and names, and builds a path when it draws a row | the memory measurement in `DESIGN.md`'s "Memory budget" over a deep tree |

Stages 1 and 2 do not depend on the result snapshot plan, and they do not
change it.

## What this changes in the contracts

The owner said on 2026-10-20 that gozler does not support older protocols,
of gozler or of goz. So stage 1 raises `OLDEST_PROTO_VERSION` to
`PROTO_VERSION`, and gozlerd refuses every older client at its `Hello` with
`Unsupported`, which the client already reports. The same change:

- removes protocol 1's `Request::Query`, `QueryRequest` and `text_search`,
  which serve clients that send no `Hello`;
- updates the rules that say otherwise: "The wire protocol" in `AGENTS.md`
  ("An installed daemon and a newer client must still talk"), and the
  version gate and the positional-binary bullets of "Wire protocol" in
  `docs/DESIGN.md`.

The command line does not change: scripts still call `gozler` with the
options of es and of goz v0.2.0, because that contract is about arguments
and output, not the pipe.

## Not measured

- **The time today to build paths for folders deeper than 1,024 bytes,**
  which `DirPaths` rebuilds for every row from the nearest cached ancestor.
  The code shows the cost grows with the depth past that point; it was not
  timed.
- **The client's time to join paths,** which moves from gozlerd to the
  client in stage 1.
