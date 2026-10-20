# Plan: a sorted name index and rows read from the index

gozlerd keeps the order of every name in the index, so a search sorts its
matches by numbers instead of by strings. A reply then holds a 16-byte row
for each match, and gozlerd reads each page's names from the index when it
sends the page, instead of copying every name and folder before the first
page.

## Schedule

**Scheduled now.** The owner chose this design on 2026-10-20, over making
each copied row smaller.

## What it fixes

Measured over a copy that copied into itself, 5.6 million entries 193
folders deep ([deep folder rows](../research/deep-folder-rows.md)):

- **A full export holds 1.0 GB in gozlerd** above the index: a `QueryHit` of
  about 96 bytes for each row, its copied name, and the sort keys. The
  owner's laptop has about 2.5 GB free, and a failed allocation stops
  gozlerd, so every search then fails.
- **The export waits for copies.** After the scan, `order_hits` copies every
  hit, folds every name and sorts by folded names, and the pages follow:
  5 to 8 seconds in all. How that time splits is not measured.
- **The delete cannot take its list from gozlerd** (#84), because the first
  row waits for the copy and the sort of every match.

## How established programs do it

Everything keeps its index sorted by name at all times, and keeps the path,
size and date orders too when the person turns on their "fast sort". A
search holds one pointer for each result, and the list reads the entry
through that pointer when it shows the row, with its current name.

Gozler does the same in its own form:

| Everything | Gozler |
| --- | --- |
| Name order kept in the index | A rank for each distinct name, kept in the name store |
| Path order kept when "fast sort" is on | Folder ranks for each search, as `query::order::folder_ranks` does now; a folder's rank costs 4 bytes |
| Size and date orders kept when asked | Numbers the index already holds |
| One pointer for each result | One row for each match: the entry index and its file ID |

The index does not keep an order of every entry, for three reasons:

- A search with a literal, such as `kernel32`, finds its matches in the
  store of distinct names: it tests each name once, then follows the
  entries that share it (`scan`, case-insensitive). A walk of every entry in
  name order would test each entry instead. A search without a literal,
  such as the empty search, tests each entry in slot order, as many entries
  as a walk in name order would test.
- The journal adds thousands of entries a second. A sorted list of every
  entry moves its tail on each insert; a tree costs more memory than a rank
  for each name. This is the reason that decides: a kept order of every
  entry would spare the empty search its sort, but at that cost on every
  change.
- A search still needs its own list of matches, because a search picks a
  part of the index. Everything holds one too.

A database such as SQLite in memory does not fit either: its indexes cannot
answer a substring search, so every search would read every row.

## The design

### A rank for each name

- `NameStore` gains a rank for each name id: the place of its folded name
  among every folded name in the store. Names whose folded names are equal,
  such as `README` and `readme`, share a rank, because the result order
  compares folded names. It also gains the name ids in rank order. Together
  they cost 8 bytes for each distinct name: 45 MB for the 5.6 million
  distinct names of the deep tree, and less on a real volume, where names
  repeat.
- **A build ranks every name before the index is shared**, before
  `Building::finish`, so a search never meets an index without ranks. It
  needs no lock, because nothing else reads the index yet. It sorts every
  distinct folded name once, so it lengthens bootstrapping and rescanning.
- **A name added later has no rank.** Interning stays one hash insert on the
  journal's write lock. A search places each unranked name it matches by a
  binary search over the ids in rank order. That search steps over dead
  names, because `NameStore::release` zero-fills a dead name's folded
  bytes, which would break the sorted order.
- **A rename keeps the name id when the name stays the same.**
  `rewrite_entry` interns the new name before it releases the old one, so a
  move of a file with a unique name no longer kills its name and adds an
  unranked copy of it.
- **The tail refreshes the ranks by a merge**, where it runs
  `compact_index`, when unranked names or dead ranked names pass 0.1 % of
  the names. It sorts only the unranked names and merges them into the
  order, dropping the dead names: O(n + u log n) for n names of which u are
  unranked. Every index write already runs on the tail, so no compaction can
  run during the merge, and the index cannot change while the tail builds
  the new arrays under a read lock, which searches share; it then swaps
  them in under the write lock. At 0.1 %, a search of 5.6 million names places at
  most 5,600 unranked names, about 130,000 string compares, and a binary
  search steps over at most as many dead names.
- **A name compaction moves the ranks to the new ids.** `plan_compact` keeps
  the names in id order and gives them new ids, so `plan_compaction` also
  writes each rank at its new id and filters the ids in rank order through
  the remap, and `apply_compaction` installs them with the names, under the
  same upgradable read.

### Sorting packed keys under the lock

- **The scan writes each match as a 16-byte record**: a primary key (`u64`),
  the name key (`u32`) and the entry index (`u32`), compared as one number.
  The records replace `Candidate`, which already carries a folder rank
  computed before the sort. A scan that visits every entry, such as the
  empty search of a full export, reserves the live entry count once, so the
  list never grows; growing would hold its old and new buffers together,
  about 170 to 200 MB at 5.6 million matches. Other scans grow the list as
  `Vec` does, because they match fewer entries.
- **The name key** is 2 × the rank for a ranked name; ranks start at 1. For
  an unranked name, r is the rank of the nearest live ranked name whose
  folded name is not greater than its own, or 0 when there is none: its
  key is 2 × r when the folded names are equal, so `README.txt` shares the
  key of a ranked `readme.txt`, and 2 × r + 1 when its folded name is
  greater. Dead names never decide r, because their bytes are zero. So
  every unranked name between the same two live ranked names shares one
  key, and the tie runs below order them by their folded names. A ranked
  name costs one array read, an unranked name one binary search, and no
  table for the search is needed.
- **The keys:**

  | Sort | Primary key |
  | --- | --- |
  | Name | the name key |
  | Path | the folder rank, then the name key |
  | Size | the size; unknown sorts last in either direction |
  | Date modified | the date; unknown sorts last in either direction |
  | Extension | the extension rank, over the distinct extensions the search matched |

  A descending sort inverts the primary key, the whole of it for a path
  sort, and the tie-breaks stay ascending, as `compare` orders them.
  Unknown sizes and dates stay last after the inversion.
- **`query::order` owns the key encoding**, beside `compare`, because the
  key and the comparison are one rule. A proptest pins that they agree:
  when one record's key is smaller, `compare` orders it first.
- **The sort:** `par_sort_unstable` sorts every record in place, with no
  second buffer, as `order_top` sorts now; a limited page uses
  `select_nth_unstable`. Records whose primary and name keys are equal,
  such as hard links and the many `index.js` of a volume, are then put in
  order with `compare`, which reads the file IDs and raw names from the
  index. A limited page orders the run of equal keys at its last place the
  same way, over the whole match set, so a limited list stays the start of
  the full one.
- **The sort runs under the volume's read lock**, so the deferral to
  `order_hits`, its copy of the hits and its folded name keys go. The
  journal waits while a search holds the lock, so the target is that the
  sort and, for a path sort, the folder ranks add no more than 0.3 s to the
  scan's lock time for 5.6 million matches on the owner's laptop. Stage 1
  measures both sorts at that size and the tie runs on `C:\`.

### Rows instead of hits

- **After the sort, each record becomes its row**, in place: the file ID at
  the index's width (8 bytes on NTFS) and the entry index. A ReFS file ID is
  16 bytes, kept as two `u64`s so that a ReFS record is 24 bytes, not the
  32 that a `u128`'s alignment gives. A volume's answer is these rows with
  the index instance that answered. It holds no name, no folder and no
  `QueryHit`. These are the rows that the
  [result snapshot plan](result-snapshot.md) designs.
- **The index instance** is a number in `VolumeIndex`, from a counter shared
  by the process: every new index takes one, and `optimize_storage` takes a
  new one, because it gives the entries new indexes. It is not a build fact
  in `Indexed`, because `optimize_storage` also runs on a live index, in
  `apply_rules_in_place`.
- **The page builder** takes the read locks of the reply's volumes in the
  `VolumeSet` order, builds up to `PAGE_ROWS` items, releases the locks, and
  then sends the page, so the pipe and a slow client never hold a lock. The
  fixed order keeps two builders from waiting on each other behind a waiting
  writer, and becomes a rule in `volume_state.rs`'s lock rules.
- **A row is the file, read when its page is built.** A row is sent while
  its slot is live, holds the row's file ID and lies under the request's
  scope, with the file's current name and path, as Everything's list shows
  a renamed file. A row whose slot is dead or holds another file, or that
  left the scope, is left out, and so is a row whose folder is first copied
  at page time and has no full path (its folders end in lost+found or pass
  the hop limit). A folder already sent keeps its path. The scope folder itself is checked the same
  way once for each page. A file moved or renamed inside the scope may no
  longer match the search, and is sent anyway, as a row read later. Each
  page's total is the scan's total less the rows left out so far, because
  `gozler` reads the last page's total and exits 10 when it shows fewer
  rows. The window caches a chunk by its start, so a chunk that comes back
  short shows its missing rows empty until the next search; that needs a
  delete in the milliseconds between a chunk's sort and its pages.
- **A volume whose index instance changed** sends no more rows, and the
  pages after it set `volumes_incomplete`, so the client knows the answer is
  incomplete.
- **Folders.** `FolderCopy` stays the owner of which entries have a full
  path. The scan uses it for its orphan check and for the folder ranks of a
  path sort, as now, and frees both after the sort, before the first page.
  The page builder keeps one for each volume of the
  reply, keyed by folder slot and file ID, because a reused folder slot
  must not give a row another folder's path. It sends the folders a page
  needs that no earlier page sent, as now. A folder keeps the name and the
  parent the client got with the first page that sent it, so the later rows
  of a folder moved during the reply print its old path. `folder_ranks`
  keeps its input, the `ResultNames` table of the copied folders.
- **Visibility.** Once the [visibility plan](visibility-plan.md) lands, the
  page builder also leaves out a row whose current folder the view cannot
  list, as the result snapshot plan says, and its view request keeps the
  `VolumeSet` lock order.
- **Several volumes** merge one page at a time with `compare`. Its keys of
  folded names, sizes and dates compare across volumes. Folder ranks do
  not, so for a path sort `query::order` also owns the comparison of two
  folders on different volumes by their folded paths. A volume's root path
  is its folded mount prefix without the last separator, as `FolderCopy`
  names a root. When no volume's root path starts another's, such as `c:`
  and `d:`, every path of one volume sorts before every path of the other,
  so the volumes follow each other. Otherwise the merge compares paths:
  volumes mounted at `C:\Data\` and `C:\Data2\` interleave, because `2`
  sorts before `\`, and `c:\data` starts `c:\data2`.
- **Owners.** `query` in gozler-core reads the rows from each `&VolumeIndex`:
  the row checks, the folder table and the merge. The daemon only takes the
  locks in order and sends the pages. The demo query in `run_demo_query`,
  the benches and `tests/interned_names.rs` read their results through the
  same reader, because `QueryHit` goes.

### What it costs and gives (computed)

| Full export of 5.6 million rows, above the 794 MB index | Now (measured) | With this plan |
| --- | --- | --- |
| By name | 1,017 MB | about 135 MB: the records, reserved once (90 MB), and the page builder's folders (about 45 MB) |
| By path | 1,020 MB | about 225 MB, while it sorts: the records and the scan's folder table and folder ranks (about 135 MB, measured in the first page by path), freed before the first page |
| The index | 794 MB | plus 8 bytes for each distinct name, at most 45 MB here |
| First row | after the scan, the copy and the string sort | after the scan and the number sort |

## What it changes in the contracts

- **The wire does not change.** A page holds the same folders and items in
  the same layout, so `PROTO_VERSION` stays.
- **`docs/DESIGN.md`:** "Result order" says how the packed keys and the tie
  runs give the same order; "Rows on demand" says that every request sorts
  under the lock and that a page reads its rows when it is built; "Index
  coherence" says which rows a reply leaves out when the index changes.
- **The lock rules in `volume_state.rs`** gain the order of several volumes'
  read locks.
- **The result snapshot plan** gets these rows. When stage 2 lands, its
  "What the code does now" changes, its sentence that several read locks
  cannot deadlock takes the lock order instead, and a result snapshot then
  keeps a reply's rows instead of dropping them.
- **#84:** a delete can ask for its scope's rows sorted by path, descending,
  which lists every folder's contents before the folder, because a folder
  ranks below every folder inside it. The delete gets no full paths and no
  copy of the rows, and never a file that is not under its scope.
  Changing the delete belongs to the copy engine's own work.
- **The README** describes the deep-folder figures; each stage updates them.

## Stages

One implementer does both stages and fixes the review's findings. Each
stage ends with `check.ps1` green and a commit.

Stage 1 first adds the memory probe of the study as an example beside
`memsim.rs` (`crates/gozler-daemon/examples/deep_export.rs`): the deep tree,
a counting allocator, and gozlerd's steps for one search, through
gozler-core's public functions. It runs once before the change; for that
run, temporary timers in `run_query_impl`, which are not committed, split
today's 1.2 s for the first page by path into the scan, the folder copy and
`folder_ranks`. When the packed sort is built, the same temporary timers
time it too. If
the folder ranks and the sort together pass the 0.3 s target, they move
outside the lock, as "Risks" says, before stage 1 ends.

| Stage | Change | Proof |
| --- | --- | --- |
| 1 | Name ranks in `NameStore`: the ranks of a build, unranked names, the merge on the tail, and the ranks through a compaction; `rewrite_entry` interns before it releases. The packed records and their keys in `query::order`; the sort under the lock with its tie runs; the deferral and `order_hits`' sort go | a proptest over random names, with names added and released after the ranks, and merges and compactions between: the rank order equals the folded byte order; a proptest, with unranked names, new case spellings of ranked names, released names, and sizes and dates at 0, unknown and their extremes: the key order agrees with `compare`, in both directions; a limited list is the start of the full one when a tie run crosses its end; a move of a uniquely named file keeps its name id; the golden corpus unchanged; the memory probe of the study: lock time and memory of the sort of 5.6 million matches by name and by path, the rank build of a build, and the tie runs on `C:\` |
| 2 | Rows and the page builder: the records become rows; the page builder reads them from the index one page at a time, in the lock order; rows left out and the total; the index instance; the merge of several volumes. `QueryHit` and `order_hits` go | unit tests: a deleted row, a row that left the scope and a row whose folder, first copied at page time, has no full path are left out, and the last total counts them; a renamed row shows its new name; a reused folder slot gives no wrong path; a changed index instance sets `volumes_incomplete`; a proptest: volumes merged, with mount prefixes as the daemon stores them, ending in `\`, such as `C:\Data\` and `C:\Data2\`, give the order of `compare` over all of them; the golden corpus unchanged; the memory probe: a full export by name and by path |

## Risks

- **The sort holds the lock longer than its target** on a large volume.
  Then the journal falls behind while a broad search runs. The folder ranks
  and the sort can then move outside the lock: under the lock, the scan
  stores each match's folder in the copied folder table where its folder
  rank goes; after the lock, `folder_ranks` runs over the copied folders,
  which it reads without the index, the keys take the ranks, and the
  records are sorted. The folder copy stays under the lock, because it
  reads the index. The tie runs then take the lock again, and first check
  the index instance, as a page does: a changed instance sends no rows and
  sets `volumes_incomplete`, and a record whose slot died meanwhile is left
  out and counted out of the total before `compare` reads it, because a
  dead slot's parent and name id are `NIL`.

## Not measured

- The lock time of the sort by packed keys, by name and by path.
- How many tie runs `C:\` has, and what ordering them costs.
- The rank build inside a build, on `C:\` and on the deep tree.
- The time of the rank merge on the tail.

Stage 1 measures all four.

## After install

- Export `C:\` (`gozler -path C:\ -n all -export-txt`) while Task Manager
  shows gozlerd's memory, and check that it rises by about 25 bytes a row
  (16 for the row, about 9 for its folders), not by hundreds.
