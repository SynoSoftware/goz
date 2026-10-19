# Design

## Workspace

gozler is six crates:

- **`gozler-core`**: pure, OS-agnostic. USN, MFT and folder-listing parsing, the index and the steps that bring it up to date after each journal batch, the folder walk, the query engine, the wire protocol, CSV output, and `es`-compatible argv parsing. Tests run on any OS.
- **`gozler-winfs`**: the thin `unsafe` Win32 layer (`windows-sys` only). Buffers, typed structs, and opaque handles; it never parses an ioctl record payload. It depends on nothing else in the workspace, enforced by cargo-deny.
- **`gozler-daemon`** (`gozlerd`): bootstrap, USN journal tail, the tokio named-pipe server, and Windows service integration.
- **`gozler-client`**: the blocking named-pipe client, with the state of the gozler service and the scope-folder check, which `gozler` and the window share. Its error types build on every OS; the pipe is Windows only.
- **`gozler-cli`** (`gozler`): the `es`-compatible CLI.
- **`gozler-ui`** (`gozler-ui`): the gozler window, a native Win32 search window. Its `core` module decides what the window shows and holds no Win32 type, so its tests run on any OS; its `win` module holds every Win32 call; `app` joins them. "The window" describes it.

The command line splits by one rule: what asks the operating system is in `gozler-cli` and `gozler-client`, and everything else is in `gozler-core`, where CI tests it on Ubuntu. `gozler-core` owns the parsing, the vocabulary tables, the help text, the date resolver, the request built from a working directory and a query start it is given, the evaluation, and the output rows. `gozler-cli` reads the arguments, the working directory, the clock and the time zone, resolves drive-relative folders, talks to the pipe through `gozler-client`, and writes to stdout or the export file.

## Index coherence

gozler indexes fixed NTFS and ReFS volumes that have a drive letter or a mount folder; removable drives, other filesystems (exFAT, FAT32) and volumes with no mount, such as Windows' hidden recovery partition, are out of scope and are not tracked, because no program opens a path on a volume without a mount. The set of indexed volumes follows the machine: within a minute, a fixed NTFS or ReFS volume that appears (an attached VHD, an unlocked BitLocker drive, a Dev Drive) is indexed, and a volume that is removed leaves the index. A `-path` scope on no indexed volume is an error (exit 4, with a message that names the folder), never an empty answer, because an empty answer reads as "no such file".

The daemon answers queries from the moment it starts. A volume is `bootstrapping` until its first build finishes, and like `rescanning` it marks results incomplete; a volume that cannot be opened is retried on the same budget as a tail that exits, then reported `failed`.

Every start builds each index from the volume, because nothing is saved between runs. Each build starts from the root folder's file ID, read from the volume: record 5 on NTFS, and ReFS's own. On NTFS the build reads the MFT once, with `FSCTL_QUERY_FILE_LAYOUT`, which gives each file every name, its attributes, size and date; it is also the size and date pass of "Sizes and dates". One pass instead of two halves the MFT read after a boot (measured on 2 million entries: 10 s instead of 22 s), and that read is most of the time a volume stays `bootstrapping`. When that pass stops part way, the MFT enumeration (`FSCTL_ENUM_USN_DATA`) runs into the same index and names the files the pass did not reach, so a failed read never drops a file from the index; their sizes and dates stay unknown. Every name on the volume is indexed, the NTFS metadata files such as `$Boot` and those under `$Extend` included. A saved journal cursor on its own must never skip that build: it would tell a fresh, empty index that it is current, and the daemon would serve an empty volume as `live`. The index itself is never saved (see "Not planned").

ReFS has no MFT and answers neither request, so a ReFS build walks the folders from the root. It opens each folder by its 128-bit file ID, with backup semantics so that permissions hide no name, and lists it with `FileIdExtdDirectoryInfo`, which gives each name its file ID, attributes, size and date; the walk is also the size and date pass. A folder reparse point, such as a junction, is indexed but not entered. The walk is slow next to an MFT read: 30 s for the 250,000 entries of a Dev Drive with a cold cache, about 24 times longer per entry. ReFS file IDs are 128 bits, so a ReFS volume reads its journal as V3 records (`READ_USN_JOURNAL_DATA_V1`), and its index keys entries on the whole file ID. NTFS keeps V2 records, because asking it for V3 records switches every record to V3, and its index keeps 8 bytes per file ID.

A walk lists one folder at a time, so, unlike an MFT read, it can meet an entry twice or miss one that moves. The journal cursor is taken before every build, so each change made during the walk replays. A name the walk meets again never renames its entry: a file met in a second folder is a hard link and keeps both names, and a folder met again moved during the walk, so its replayed rename record moves it. A hard link renamed during the walk can be listed twice or never, so until the replay first finds nothing new, every renamed file also has its links reconciled (see "Hard links"). A folder moved during the walk from a part not yet listed into one already listed is never listed, and the journal records no change to what it holds. So a rename record for a folder the index lacks makes the tail list that folder and everything under it, and one for a file the index lacks makes the tail read the file's size and date. A folder that is being deleted, which Windows allows only when it is empty, is skipped; a listing that fails for any other reason starts a rescan.

The index is a cache of the filesystem, and a cache that cannot prove it is coherent must rebuild itself. Every sync-loss path (journal wrap, a parse anomaly, a journal that cannot be read at startup, reads that stop answering, even a sick disk that leaves a journal read pending forever, which a supervisor cancels, or a tail thread that exits outright, which the same supervisor revives on a bounded budget) ends in one of three states:

- **in sync**, provably (phase `live`);
- **rescanning**, with a full rescan scheduled (phase `rescanning`); or
- **offline** (phase `offline`, or `failed` once retrying has stopped), reported as no longer tracking. A volume that can never have a journal is `snapshot`: complete as of bootstrap, and said so once in `--status` instead of on every result page.

There is never a silent fourth state.

Some irregularities do not lose sync and are not rescanned. A delete for a file ID the index does not hold has nothing to remove. A record number that NTFS has given to a new file has its stale entries removed in place. Both are normal while the journal replays changes made during the build, so a rescan would rebuild the same index; they are counted, and `--status` shows the counts. An entry whose walk to the root is longer than the hop limit has no full path and is left out of results. That happens briefly when the replay applies an old move on top of the newer scan, which can close a loop in the parent links until the later moves arrive, and permanently for a real folder tree that deep, which a rescan would rebuild the same way. Neither case starts a rescan. `--status`, the daemon handshake, and every result page decide a volume's state with a single shared rule, so a client can always tell whether the results it just got are complete. A request reads each volume's phase while it holds that volume's index, so the state a page reports is the state of the index that answered it.

## Hard links

A hard-linked file is one record with several names, and a USN record names only one of them.

- **Everything 1.4** never sees a hard link created after its index was built. Per the [Everything documentation](https://www.voidtools.com/support/everything/), *"changes made to a hard link will only update the first hard link entry."* Only a manual **Tools -> Options -> Indexes -> Force rebuild** picks it up. This is a long-standing, documented limitation.
- **gozler** reconciles hard links live. A hard-link-change record names only one link (and may name a dead one), so gozler walks the file's real link set over Win32 (`FindFirstFileNameW` / `FindNextFileNameW`) and reconciles the index to exactly that set: names added or removed are applied, not just size and dates. It finds each link's folder by the folder's file ID, not by its path, because during the replay of a build the index may not yet know a folder's current name. A walk that cannot complete (it failed part way, or a link's folder is not indexed yet) is skipped rather than applied as a partial set, counted, and surfaced by `--status`, and a rescan trues it up. A file that is gone by the time of the walk is left to its delete record. The gap is visible, never silent.
- **Everything 1.5** (beta) does track new and renamed hard links.

## Command line

A script written for Everything's `es.exe` or for goz v0.2.0 changes only the program name to `gozler`.

- **Names never change.** Every option, alias, search function and syntax that goz v0.2.0 accepts keeps its name and purpose. An es name keeps es's aliases and single-dash form, so a script written for es finds it; that is why `-whole-words`, `-filename-column`, `datemodified:`, `date-modified:`, `attributes:` and `attr:` exist beside the shorter names. A gozler-only option starts with `--`, as `--json` does, so it can never collide with an es option. `-v`, beside `-version` and `--version`, is the one exception, because most command-line programs answer `-v` with their version and es has no `-v`. Messages read `gozler: <error>` and `gozler: warning: <text>`.
- **Owner decision: neither v0.2.0 nor es decides behavior.** They supply the names and the starting behavior. Where either answers wrong, or worse for the user, gozler answers differently.
- **Owner decision: the help describes only what gozler does.** It does not compare gozler with es or say why gozler's behavior is better, because a user needs to know how to run gozler, and a claim about another program stays true only while someone checks it against that program's documentation. This document records the reasons for gozler's behavior.
- **Owner decision: 20 results by default.** A query that forgets `-n` on a volume of a million files would flood an agent's context or a person's console. 20 rows, the command, the warning and the next prompt fit one 25-line console. Without `-n`, gozler prints at most `DEFAULT_LIMIT` rows. When the limit leaves matches out, stderr gives the shown and total counts and gozler exits 10; `-n 0` also exits 10 when anything matches. `-n all` prints every match. `-export-*` writes a file, which floods nothing, so it exports every match unless `-n` is given. `-get-result-count` and the size totals cover every match whatever `-n` says, because a count that stops at the limit tells nothing.
- **es's behavior comes from its documentation and its source.** `es.exe` is never run (owner ruling). gozler copies es where es is right, and each test pins the behavior gozler chose.
- **The tables generate the help.** The options and sort keys (`SWITCHES` and `SORT_KEYS` in `escompat::argv`), the search functions (`SEARCH_FUNCTIONS` in `query::parse`), the attribute letters (`attributes::LETTERS`) and the exit codes (`ExitCode`) each live in one table that the parser and the help both read. A new option, function or sort key is one row, and the help lists it. A test parses every option, function, term and example the help lists, so the help cannot offer something gozler refuses.
- **The help has two screens.** `-h`, `-?`, `/?` and `--h` print the quick help: the examples marked quick, each a command with what it finds, then exit codes 0, 2 and 10 in short and the command for the full reference. It fits an 80 by 25 console with the command and the next prompt, so it never scrolls. `--help` and `-help` print the full reference: every name of every option, grouped by task, with the search functions, attribute letters, notes and every exit code. An example marked `quick` is also a line of the quick help. The split exists because a new user, or an agent, needs the common tasks first, and the full reference fills several console screens. Each line of help text is one short sentence-case phrase; detail that does not fit one line is a note in the full reference. Every example is a command that works as typed in both cmd and PowerShell.
- **Bare `gozler` prints the overview**, not a search for every file: the version, what gozler does, the state of the gozler service with the commands that start it, and the commands to search and to read the help, in fewer than 25 lines. A first-time user learns there what gozler is and whether it can answer, and a list of 20 random files tells nothing. A command line of options alone, such as `gozler -n all`, still searches. The overview exits 0 when gozlerd answers, and otherwise with the failure's code: 7 when the service is not ready, 8 when it is not running. The overview and both help screens are in color only when stdout is a console that draws ANSI colors and `NO_COLOR` is not set, so a file or a pipe receives plain text. It uses the 16 standard console colors, so it follows the terminal's scheme, and words carry every state, so plain text loses nothing.
- **`/a` and `/o` are prefix families** with their own small parsers, because es's dash-optional name matching would make `/ad` and `/a-d`, or `/on` and `/o-n`, collide as plain rows. They own every token that starts with `/a` or `/o`, and refuse a letter they do not know with exit 4. Another unknown `/` token is search text, as in es, but as text an `/a` or `/o` token would match nothing.

## Exit codes

Each outcome that a caller handles differently has its own code. `ExitCode` in `gozler-core` holds every code with its meaning, and the help prints that list. Codes 4 to 8 keep their goz v0.2.0 meanings, so scripts written for it still read them. `Failure::exit_code` in `gozler-cli` is the one place that maps a failure to its code. Each failure writes a `gozler: ` line to stderr that says what failed and, where it helps, what to do next. An unknown option adds `gozler: run gozler --help for every option`, and an unknown function adds `gozler: run gozler --help for every search function`.

When the pipe does not exist, gozler asks Windows for the state of the gozler service and prints the commands that fix that state, each alone on its line so that it can be copied into cmd, Windows PowerShell or PowerShell 7: `net start gozler` for a stopped service, `sc.exe config gozler start= auto` and then `net start gozler` for a disabled one, `pushd` to the folder of the running `gozler.exe` and then `.\gozlerd.exe install` when no service is installed, and the releases page when `gozlerd.exe` is not in that folder. These exit 8. A service that is running, starting, stopping or paused without its pipe exits 7 with `try again shortly`, because the pipe may open soon: a script retries on 7 and repairs on 8.

When several codes apply, the more serious wins: a failure (1, and 3 to 8) before an incomplete answer (2), that before no matches (9) or a cut list (10), and those before a complete answer (0). A failure ends the run before it has an answer; `Answer::exit_code` orders the rest. So an empty answer while a volume in scope is indexing exits 2, not 9, because the matches may exist, and a cut list from such a volume exits 2, not 10, because 10 promises that the rows shown are correct. `-no-result-error` exits 9 only when nothing matched, not when `-n 0` hid the matches. A count covers every match, so it never exits 10.

An answer is incomplete (exit 2) for two causes: a volume in scope is not ready (indexing, rescanning, offline or failed), or the query tests or totals sizes or dates on a volume whose size and date pass failed (see "Sizes and dates"). `--status` exits 2 when any volume is not complete or the size and date pass of any volume failed. `-timeout MS` polls the daemon's status until the volumes in scope are complete or MS runs out, then searches anyway; a volume still incomplete then makes the answer exit 2. The client finds the volumes in scope with `Scope::of`, the rule the daemon routes by, so the two cannot disagree.

## Search

### The client parses, the daemon evaluates

The client parses the query with the `gozler-core` parser and sends the parsed query, its scope and what it wants back (`SearchRequest`). The client resolves everything that depends on where and when it runs: the working directory, the time zone and the query start. The daemon only evaluates. It also accepts protocol 1's text query (`QueryRequest`) from other pipe clients and parses it with the same parser, its own clock and no working directory. A relative path there is invalid input (`BadQuery`), never a literal that silently matches nothing.

The arguments are joined with spaces and split into terms again, as goz v0.2.0 does, because scripts build a query in one string: `gozler "ext:cs size:>1mb"` is two terms. Quotes make spaces and operators plain text, also in a function's value: `path:"C:\Program Files"`.

A query is AND-ed clauses, each an OR of terms that may be negated. That covers `!` and `|` with es's precedence, where `|` binds tighter than the space, and keeps the evaluator flat. The parser refuses with exit 4 what would otherwise search text that no name contains. An unknown function such as `dc:` is one: Windows names cannot contain `:`, so as text it gives an empty answer that looks valid. An unquoted `<` or `>` outside a function value is another, because Windows names cannot contain them either. es groups terms with `< >`; gozler does not.

Regular expressions use Rust's `regex` crate, which matches in linear time, so no pattern can hang gozlerd. The price is that a pattern has no look-around and no backreferences.

**Word start and name start.** `wordstart:` matches a term where a word of the name starts: at the start of the name or after a character that is not a letter or digit, so `wordstart:1234` finds `IMG_1234.jpg` and `wordstart:para` does not find `comparator.txt`. `startwith:` matches at the start of the name. With `-p`, or with `\` or `/` in the value, each tests the full path instead, as `ww:` does. Accents count, as in every comparison (`gozler_core::fold`). The window's match mode is a parse default (`ParseOptions::term_match`). Start of words and Start of name reach name terms only; a term with a separator and the values of `path:`, `nopath:`, `file:` and `folder:` stay plain substrings, so a typed folder still finds what is below it. Whole words is `-ww` and keeps its rule, which reaches those terms and values too. With `-p`, a mode tests the full path.

The folded-arena scan needs one literal that every matching name contains. Only a clause of one positive term supplies it: a name term, a whole-word, word-start or name-start name term, a name wildcard or a single extension (`CompiledClause::name_literal`). A negated term, a path term, a path wildcard and a regular expression never do. For example, the literal of `*\docs\*` is `\docs\`, which no name contains, so a scan for it would find nothing.

### Paths

- **A path term matches the full path**, as in es. A term with a drive or a separator is a path term, and it matches as a substring: `C:\repo\` finds everything below `repo`, and `C:\repo\x.md` also finds `x.md.bak`. A path wildcard matches the whole full path, so `C:\repo\*.cs` finds the `.cs` files below `repo`. A quoted term with a separator is still a path term, and a `*` in it is still a wildcard, because a path cannot hold a literal `*`.
- **`/` and `\` both separate folders**, because agents and Unix habits write `/`. The parser turns `/` into `\` in every path that a term, a function value or an option names. It never changes a regular expression, where `/` and `\` keep their regex meaning; a regular expression sees `\` separators. Output keeps `\`.
- **A token that starts with `/` and names an option is that option**, as in es (`/ad`, `/on`, `/p`). So a relative path is written without a leading `/`.
- **The client resolves relative paths.** A path term that starts with `.\` or `..\`, a relative `parent:` folder, and a relative `-path`, `-parent` or `-parent-path` folder are joined to the working directory. `gozler_core::winpath` applies the Windows rules itself, without asking the operating system, so a command line builds the same request on any machine; `std::path::absolute` would turn `C:\repo` into a Unix path on the Ubuntu CI. After the join it removes `.` and `..`, because the daemon looks up each component of a scope as a child folder. A rooted `\foo` takes the drive of the working directory. Only a drive-relative folder such as `-path D:x` is resolved by `gozler-cli`, with `std::path::absolute`, before the request is built, because only Windows knows the current folder of each drive.
- **A scope folder that does not exist is invalid input.** When the folder that `-path`, `-parent-path`, `-parent` or `-get-folder-size` selects does not exist, gozler exits 4 with `gozler: <folder> does not exist` before it connects. es gives an empty answer with exit 0, and an agent reads that as "no such file" when the folder is wrong. Only a folder that the operating system reports as missing is refused: a folder the user cannot read still searches, because gozlerd answers from its index. When `-parent` and `-path` are both given, only the `-path` folder is checked, because it is the scope.

### Dates

A `dm:` value resolves once, to UTC bounds, from the query start and a time zone. The date resolver in `gozler-core` (`query::dates`, with `jiff`) does it: the client passes its own clock and time zone, and the daemon's text path passes its own. The bounds travel as UTC, so the daemon never needs the client's time zone.

- `today`, `yesterday` and a `YYYY-MM-DD` date are calendar days in the time zone, from midnight to the next midnight, so a day around a daylight-saving change lasts 23 or 25 hours.
- `Ndays` is the N times 24 hours that end at the query start.
- `dm:` takes only these forms, because they are the dates scripts and agents write. A slash date such as `03/04/2026` is a different day in another locale.
- Output prints dates as ISO 8601 in local time, and plain output groups digits with commas, whatever the locale, so a script reads the output without knowing the locale.
- A comparison compares times at the value's granularity, as es does: `>today` is after today, and `<7days` is older than the last 7 days. A range `lower..upper` runs from the start of `lower` to the end of `upper`, and a reversed range is invalid input.

### Sizes and dates

A folder's size is the sum of the known sizes of every file below it, each name of a hard-linked file counted: the number `-get-folder-size` gives for that folder, as in Everything with "Index folder size" on. `-size`, `--json` and the CSV and TSV exports show it, `size:` tests it, and the size sort places a folder by it. `-get-total-size` and `-get-folder-size` still add files only, because a folder's size is already the sum of its files. The index always keeps the sums, in each folder's own size slot, so they cost no memory and need no setting. A folder's size is unknown until the volume's size and date pass completes, and stays unknown until a rescan when the pass failed or stopped, because a sum that misses unread files would look exact; after a complete pass `VolumeIndex::compute_folder_sizes` sums them once, and from then on every change adds its difference to each folder above it, so a journal change costs a walk up its folders, never a walk of their contents. A move that opens or closes a loop in the parent links (see "Index coherence") changes each folder of the loop by more than what moved, so it, and a file that becomes a folder, make the index sum every folder again; both are rare. The sums wrap at 2^64 instead of failing, so each change reverses exactly; only a corrupt size can come near that bound.

A file's size or date is unknown when the size and date pass failed or stopped, or could not read that file. A size or date test, negated or not, never matches an unknown value: `!size:>1mb` does not report an unread file as small, and `report|size:>1mb` still finds `report.txt` through its other term. Because every clause is an OR, this gives the same rows as three-valued logic without tracking unknown results for each entry.

The size and date pass runs in each build, before the volume serves as live, so a live volume never has the pass still running. A pass that failed or stopped sets the volume's `metadata_pending`, and `--status` shows it. Each results page and totals reply carries the flag for the volumes in scope. The client warns and exits 2 when the flag is set and the query tests a size or a date or asks for a size total, because that answer may be short. A file that a successful pass could not read is left out without exit 2; such files are rare, and the help says so.

### Attributes

The index holds each entry's attributes in two bytes (`PackedAttributes`): one bit for each attribute letter but D, because the entry's folder flag already says D. The record's `u32` would take four bytes, and the index holds millions of entries. An attribute without a letter is dropped, so `attrib:` takes letters only.

### Result order

The same query over the same index prints the same list, and a limited list is the start of the full one. `query::order` owns the one comparison that the engine and the daemon both sort by. The sort key decides first, and an unknown size or date sorts last in either direction. Ties then break, always ascending, on the folded name, the volume, the file ID, the parent folder's file ID and the raw name. Hard links need the last two keys: each link name is its own entry, and all of them share one file ID. The raw name separates two links in one case-sensitive folder whose names differ only in case. Each key comes from the hit itself, so no path is rebuilt under the index lock to break a tie.

## Wire protocol

- **The version gate.** The client sends `Request::Hello` with `PROTO_VERSION` as both its lowest and highest version. The daemon serves every version from `OLDEST_PROTO_VERSION` up to its own and answers with the highest version both sides speak, or refuses with `Unsupported`. A daemon older than the client therefore refuses, and the client prints `gozler: the running gozlerd is older than this client; install the matching gozlerd`, then the link to the gozler setup, and exits 7. Without the gate, an old daemon would search a new function as text and find nothing with exit 0, or reject the request with a serde error that the client would report as invalid input. So `gozler` and `gozlerd` install together, from one build.
- **Each change an older daemon cannot serve raises `PROTO_VERSION` by one**, such as a new kind of term in the parsed query. The parsed query only grows: new variants, never changed ones. A protocol 1 client sends its text query without a `Hello`, and the daemon still serves it.
- **Results pages are positional binary** (`proto::binary`), so a field an older client does not know would make it misread every item. A new item field therefore travels only when the request asks for it: the attributes need `Rows::want_attributes`, which no client before protocol 6 sends.
- **Totals cross the pipe without rows.** `-get-result-count`, `-get-total-size` and `-get-folder-size` ask for `Returns::Totals`, and the daemon sums the sizes it already holds for each hit. Summing on the client would stream every match, and `-get-folder-size C:\` is the whole volume. The sum uses checked arithmetic; an overflow, which only a corrupt size in the index can cause, is `ProtoError::Overflow` and exit 1.

## The window

`gozler-ui` gives the search that `gozler` does a Windows desktop face: a person types, sees the matches at once, narrows them with a few clicks and opens the file. It adds no search feature. Every filter, column and sort is something `gozler` does, and a new search feature goes into `gozler-core` first. For the same query it shows the same count, the same rows in the same order, and exports the same bytes as `gozler`, because both use `gozler-core`'s parser, request rules and output code. When something fails, it says what to do next and, where it can, offers a button that does it.

**Owner decision: the priorities, in this order.** Each choice in this chapter serves one of them.

1. Low memory, because the window stays open all day beside other work. It holds no index and no result set (see "Memory budget").
2. Usability and correct Windows behavior.
3. Little code to write and maintain.
4. A polished look. It never justifies substantial code.
5. Dark mode. It is dropped when it costs more than it gives.

**Owner decisions.** These are settled:

- **No dark mode.** It needs undocumented uxtheme exports, and the menu bar stays white without undocumented `WM_UAHDRAWMENU` painting. It can come later as one module, `win::dark`, that falls back to light when an export is missing.
- **The top of the window** is the classic menu bar, then the search box with its match controls. A command bar of icon buttons was prototyped and not chosen.
- **The list reaches the first 100,000 rows** of a search, in the chosen order (see "Rows on demand"). A result snapshot kept by the daemon would remove the limit; it is new daemon work.
- **Only the window is translated.** `gozler` and `gozlerd` stay in English, because their users are scripts, agents and developers. The catalogues are JSON files in the style of TinyTorrent2's localisation contract, so the owner's projects share one catalogue style; Fluent and other translation crates are not used.
- **The filter pane uses list boxes** for Kind and Location.
- **Start of words is the default match mode**, the rule of the owner's SearchHelper: `para 50` finds `Paracetamol 500mg.pdf`.
- **The window uses TinyTorrent2's words** where both programs mean the same thing, such as Settings, not Options. `CONTEXT.md` holds them.

### Where the unsafe lives

The window is native Win32, through Microsoft's `windows` crate. Two throwaway prototypes of the same window, with a virtual list of 3 million rows, measured it against Slint 1.18.1: 4.6 MB of private memory against 12 MB with Slint's software renderer and 120 MB with its OpenGL renderer, a 182 KB executable against 13.9 MB, and a first release build in under a minute against 9.5 minutes. Windows itself gives the list's selection, keyboard navigation, column resizing and reordering, sort arrows and accessibility, which in a framework are code that gozler would write. The window uses `windows`, not the workspace's `windows-sys`, because it needs COM interfaces such as the folder picker (`IFileOpenDialog`).

- **`core` holds every decision**: what to search, which rows to fetch, which replies are current, what the selection holds, which commands are enabled, where each control goes and what every caption says. It has no Win32 type, so its tests run on Ubuntu, as `gozler-core`'s do beside `gozler-winfs`. It starts with `#![cfg_attr(not(windows), allow(dead_code))]`, because on Ubuntu nothing outside its tests calls it and CI denies warnings.
- **`win` holds every `unsafe` block**, each the smallest block that needs it, behind a safe function, and decides nothing. `core` and `app` start with `#![forbid(unsafe_code)]`, so the compiler rejects `unsafe` anywhere else.
- **`app` holds the message handlers**, which turn Windows messages into calls on `core`, and `core`'s answers into calls on `win`.
- **The dialogs are templates** in `resources/gozler-ui.rc`, so Windows runs their modal loop, Tab order, `Enter`, `Esc` and per-monitor DPI scaling. The main window, the menus and the accelerator table are built in code, because the layout follows the window's size and the menus follow the command table and the person's shortcuts.

Window code stays out of `gozler-winfs`, because that crate uses `windows-sys` only and depends on no other workspace crate, and the window needs COM and `core`'s types. A crate of its own for the window's Win32 layer would add a public API and `deny.toml` and CI entries, and protect nothing more, because the layer has one user; if a second program needs it, `src/win/` moves into a crate as it is. Values from `gozler-winfs` and `gozler-client` cross into the window as Rust types, never as raw handles.

The alternatives fail on one Win32 fact or another. A view diffed against the controls, or a list of commands that one applier runs, cannot answer `LVN_GETDISPINFO`, which needs each cell's text before it returns. Free functions over one global `RefCell` panic when Win32 calls back into the window (window rule 1). `winsafe` makes no stability promise before 1.0 and reads the system DPI, not the monitor's, and `native-windows-gui` has had no release since September 2022.

### Window rules

Each rule prevents a failure that is expensive to fix once every handler depends on the code it governs.

1. **No model borrow across a Win32 call.** Many calls send messages to the same window procedure before they return: `SetWindowTextW` on the search box sends `EN_CHANGE`, `SetWindowPos` sends `WM_SIZE` and `LVM_SETITEMSTATE` sends `LVN_ITEMCHANGED`. Calls with their own message loop, such as `TrackPopupMenuEx`, `DialogBoxParamW`, `IFileOpenDialog::Show` and `MessageBoxW`, dispatch every message of the thread, the daemon's replies included. A `RefCell` borrowed across such a call panics, and only when Windows happens to send that message. A Win32 call here is any call that can send or dispatch a message: one that takes a window handle, a COM or shell call, or a modal loop. So a handler changes the model in a statement of its own, through a `Model` method that returns what changed, and makes its Win32 calls after that borrow ends. The model's fields are private to `core`, so `app` changes a fact only through a method. In `app`, a borrow is bound with `let` only in a block that makes no Win32 call, such as the `LVN_GETDISPINFO` handler, which formats a row into the list's buffer. A dialog edits its own copy of the values, and OK and Apply give the copy to the main window by a method call.
2. **The window's state outlives each message.** File > Exit calls `DestroyWindow`, which delivers `WM_NCDESTROY` before it returns, so state freed there would leave the Exit handler running on freed memory. The window owns its state through an `Rc`, the window procedure holds its own clone for the length of each message, and `WM_NCDESTROY` drops only the window's reference.
3. **Each request has its own thread, connection and cancel token.** Closing a connection from the UI thread races the thread that reads it, and the daemon closes a connection after 30 seconds with no request (`IDLE_TIMEOUT`), so a connection kept for later fails after a pause. Connecting is one pipe open, the owner check and one `Hello`, as each `gozler` run does. The client polls the pipe every 5 ms and checks the token on each poll and between its connect retries, so a cancel frees the window at once; sending needs no check, because a request of a few hundred bytes fits the pipe's 64 KiB buffer without the daemon reading it. The daemon notices a cancel at its next page.
4. **At most one request of each purpose is live.** A request's purpose is `Results` (the list's rows), `Selection` (the selection's rows, or a selected folder's size), `Status` or `Export`. `Requests::start` cancels the live request of the new request's purpose and no other, so selecting a row never cancels the list's rows. Requests of different purposes run at once on separate connections, so a slow folder size never delays the list. Each carries the generation that the model gave it, and the model drops a reply whose generation is not the newest of its purpose; that is the only staleness check. The model records which purposes are in flight, so it is the one owner of "an export runs". Exit cancels every request; hiding the window cancels none.
5. **No reply reaches a reused handle.** A message posted to a destroyed window's handle can reach another window that reuses the handle. So the replies travel on one unbounded channel, `PostMessageW` only wakes the window, and the window joins every request thread, the cancelled ones included, in `WM_DESTROY`, while its handle is still valid. The channel is unbounded because a thread blocked on a full channel, while the UI thread waits to join it, would never return.
6. **Sizes stay device-independent until the last step.** Sizes are designed in `Dip` and converted to `Px` only in `core::layout` and in the font and icon code. The two are distinct types, so the compiler rejects a mix, and per-monitor DPI never means finding every number.
7. **One function places every control.** `Layout::compute` gives every control's rectangle from the client size, the DPI, the panes shown, the inspector's height, the font height and the measured width of each caption that the window places in code. `WM_SIZE`, `WM_DPICHANGED` and a language switch only apply its result, and its margins and gaps are named constants (see "Look"). It also gives the window's minimum size, which leaves out the filter pane's height: when the window is shorter than the pane, the pane gets a scroll range.
8. **Each key has one owner.** An accelerator for `Ctrl+A` or `Ctrl+C` would take those keys from the search box, and `IsDialogMessageW` turns `Enter` and `Esc` into `IDOK` and `IDCANCEL` before any control sees them. So the message loop runs `TranslateAcceleratorW` first, and a shortcut reaches its command whatever control has the focus. Then it takes, from `WM_KEYDOWN`, the keys that `core::commands::control_key` gives the focused control: in the search box `Down` moves to the list, `Enter` runs the search and `Esc` clears the box; in the list `Enter` opens the selection. Then come `IsDialogMessageW` and `DispatchMessageW`. Copy and Select all act on the focused control, so with the search box focused they send it `WM_COPY` and `EM_SETSEL`. The Keyboard page refuses every key that a focused control uses, so no shortcut can take one. Shift+F10 and the Menu key reach the list as `WM_CONTEXTMENU` with no point, and the context menu opens at the focused row, or at the first selected row when Select all left no row focused.
9. **One row per command.** `core::commands` holds one row for each command of "Commands": its ID, its catalogue key, its default keys, its menu and group, and functions of the model that say whether it is enabled and checked. The menus are built from it, `WM_INITMENUPOPUP` sets each item's state from it, and the match controls and buttons take their state from it, so no surface grows its own idea of a command. Every surface calls the same `MainWindow` method. A command chosen from a menu with its own loop runs after the menu returns, and the handler asks again whether it is enabled (`T11`).
10. **The selection is the files of one moment.** The daemon keeps no result set, so a position names whichever file is there now, and a selection read again later can name other files. So the selection is a set of files built at one moment, and every action reads it (see "The selection"). It is never built from rows of two moments, such as cached rows and rows fetched later, because an insert between them would list one file twice and miss another.
11. **A panic ends the process after it is reported.** A panic cannot unwind out of a window procedure (`extern "system"`), and the release profile unwinds. The panic hook writes the message to `ui-error.log` in the settings folder, shows a message box with no owner window from a new thread, waits for it, and aborts. The new thread keeps the UI thread from dispatching a message while its state is half changed, and the box has no owner because disabling an owner would wait for the UI thread, which waits for the box. Every request thread has the same hook, so a dead request never leaves a window that waits forever.
12. **The model holds facts, never display text.** A text that was built and stored earlier stays in the language it was built in. So the model and the controls' state hold facts, such as counts, phases, typed failures and parse errors, and every caption is a function of those facts and the current `Strings`. `core::captions::WindowCaptions::new` calls every such function, and `MainWindow::apply_text` sets its result: at creation, after a language switch and after a change to the Windows regional format. `layout::CaptionWidths` measures the same value, so the layout sizes what is shown, and a handler that updates one caption after a reply calls the same function. A catalogue key is written whole, as a literal, so a test finds every key the code uses in `en.json`. The list's cells are formatted when Windows asks for them, so a redraw updates them.

Every text a person sees is a key in `en.json`: no English is written in the code or in the `.rc` file, which holds the templates with control IDs and empty captions. `resources/ids.h` is the one source of the resource and control IDs, the main window's included; the `.rc` file includes it, and `build.rs` turns it into `core::ids`. `build.rs` refuses two controls with one ID, a control with ID 1 or 2, which a dialog sends as `IDOK` and `IDCANCEL`, and a dialog that the script does not define, which `rc.exe` and rustc would both accept.

### Shared code

The window builds its own `SearchRequest` in `core::query`, not through `build_search`, because `build_search` reads the command line's `EsPlan`, parses one query string and asks for rows from offset 0, while the window combines several parses and asks for rows at any offset. Every rule that both need has one owner, which both call:

| Rule | Owner |
| --- | --- |
| Query syntax and its errors | `gozler_core::query::parse`, for the typed text and for each filter's text. The window parses with no working folder, so a relative path is the parser's `RelativePath` error, which asks for the full path. `QueryError::Malformed` carries a typed reason, so the window says in every language what is wrong; its `Display` writes `gozler`'s English. |
| The `-r` and `-parent` tests and the scope `-parent` sets | `gozler_core::escompat::request` |
| How a name term matches | `ParseOptions::term_match`, which the window sets from its match mode, as `-ww` sets whole words |
| The matched part of a name or path | `gozler-core`'s matching, which returns the ranges; the window asks only for the visible rows and draws them bold with `NM_CUSTOMDRAW` |
| `dm:` resolution | `gozler_core::query::dates`, with the local clock and time zone |
| The size words' ranges | one table in `gozler-core` that the parser and the filter pane both read |
| A folder's size | `gozlerd`'s index (see "Sizes and dates") |
| Help > Search syntax | `gozler_core::escompat::search_syntax`, from the tables that `gozler --help` reads |
| Whether a volume's results are complete | `VolumePhase::is_complete` |
| The owner check, the version handshake, the service's state, and the check that a scope folder exists | `gozler-client`. `app` runs the folder check and passes the result to `Model::start_search`, because `core` makes no filesystem call. |
| Results pages, and CSV, TSV and TXT output with its date format | `gozler_core::proto` and `gozler_core::output` |

`gozler-client`'s error keeps every distinction that `gozler-cli`'s exit codes need. The window collapses them into the failures of "Daemon failures", and shows `Communication` as a lost connection, because both mean that the pipe failed after the service was found.

### Requests

The UI thread never waits for the pipe (window rules 3 to 5).

- **At start** the window asks for status beside the first search, because only a status reply names the volumes, which the Location list, status part 4 and the decision to poll need.
- **The status poll.** The window asks for status every 2 seconds while `Model::polls_status` says so: the service is not running, runs but does not answer yet, or the connection was lost; a volume in scope is bootstrapping or rescanning; or Settings shows the Drives and service page. One timer serves every reason. An offline or failed volume is not polled, because waiting does not complete it. When a status reply shows that the daemon answers again, or that every volume in scope is complete, the search runs again (`T4`).
- **A results page** carries only the combined flags `volumes_incomplete` and `metadata_pending`. When a page sets a flag that the latest status does not explain, the model asks for status once.
- **A refused request and a failed owner check** start no poll and stay on screen until the person changes the search, because the same request would fail the same way. A daemon older than the window is polled, because updating it ends the failure without a change to the search.
- **A failure means something different for each purpose.** A `Results` failure shows its message in the list. A `Status` failure updates whether the daemon answers and replaces a failure that the list shows for another reason, so the list never offers the fix for a state the service has left; while a search is in flight, its own reply shows instead, and a `Status` failure that leaves the rows shows in status part 4 (`T10`). A `Selection` failure is the service failing while rows are shown, so it shows as a `Results` failure does and clears the selection (`T3`); a selection left unread would keep its commands disabled with nothing to say why. An `Export` failure is the export's message box and leaves the list alone.
- **Exit** joins every request thread, and the join is bounded, because no request thread waits on anything without a token check: the client checks it while it waits for a reply and between connect retries, and an export checks it after each page. A stopped export deletes its partial file, and the settings are saved.

### Rows on demand

A search asks for the first chunk of `CHUNK_ROWS` rows (16,384) in the chosen sort; at offset 0 with a limit, the daemon selects the top rows instead of sorting every match. The reply carries the total, and the list's item count is the total, or 100,000 when the total is larger. When the list needs rows that are not cached (`LVN_ODCACHEHINT`, or a jump with the scroll bar or `End`), the window asks for their chunk with `offset`, and those rows show empty cells until it arrives. At most one chunk request is in flight; when it arrives, the window asks again for what the rows visible now still lack, so fast scrolling never queues a request for every chunk it passed. Every reply carries the total, and a changed total sets the count again. The cache keeps at most 32,768 rows in whole chunks and drops the chunk used least recently, so memory stays the same for any number of matches.

Every request scans every match before it sorts, about 0.3 seconds for the empty search over 2 million entries on the owner's machine. The scan dominates, so a large chunk costs the daemon little more and halves the waits on a long scroll. After the scan, each volume sorts its first `offset + limit` matches and builds a full path for each, under the volume's index lock, and the server then skips the first `offset` rows. `End` in a list of 3 million entries would make the daemon sort and build about 3 million paths on each volume to send one chunk, while the volume's journal updates wait for the lock. The limit of 100,000 rows bounds the worst chunk at about 100,000 paths plus one chunk: rows near 100,000 cost about 0.4 seconds.

### Main window

| Area | Control |
| --- | --- |
| Top | the menu bar, then a strip with the search box, its Clear button and its match controls |
| Left | the filter pane |
| Center | the results list: a list view in report and virtual mode |
| Bottom | the inspector pane, under a splitter that the person drags |
| Bottom edge | the status bar, with four parts |

The window is per-monitor DPI aware (manifest `PerMonitorV2`). It opens at its saved placement, and a placement that no monitor shows any more opens on the nearest monitor. Its minimum size is the smallest at which every control fits, the filter pane aside, which scrolls. Its title is the typed search and the program's name, "report – Gozler", as Windows titles a document window, or "Gozler" alone while the search is empty.

### Look

The window is made of plain Windows controls, with only the tweaks that cost one call or a few lines, so Windows keeps each control's keyboard behavior, focus, accessibility and high contrast.

- `SetWindowTheme(list, "Explorer")` gives the list File Explorer's row hover, selection fill and header, and `LVS_EX_DOUBLEBUFFER`, `LVS_EX_FULLROWSELECT` and `LVS_EX_HEADERDRAGDROP` give no flicker, whole-row selection and column reorder.
- The font is the message font from `SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS)`, with one larger semibold font for the inspector's heading.
- `WM_CTLCOLORSTATIC` returns the window color, so labels and check boxes have no gray dialog background. Every color the window paints comes from `GetSysColor`, so high contrast works.
- One-pixel dividers separate the panes instead of 3D edges.
- **Spacing** follows Microsoft's Windows desktop layout guidelines: 11 px margins at the window's edges, 7 px between related controls and 11 px between groups, at 96 DPI. They are named constants in `core::layout`.
- **Nothing moves by itself.** Only the window's size, the DPI, the panes shown, the splitter and the language move a control; a count, a path, a message or a state never does. The status bar's first three parts have fixed widths, each sized for its longest message with the largest numbers in the current language and DPI, and a longer value ends in an ellipsis.
- **Text fits.** A control that the window places in code takes its width from its measured caption, so a longer translation widens it instead of clipping it. Dialog templates leave room for twice the English length on captions of up to 12 characters and 50 % more on longer ones.

### Words

- Labels use sentence-style capitalization, as current Windows does: "Open file location", "Copy as path". Where File Explorer has a word for the same act, the window uses it.
- A label is a verb phrase for a command and a noun phrase for a setting, and it names what happens, not how: "Copy name", not "Copy file name to clipboard". A command that opens a dialog ends in "…".
- Every menu item, dialog label and button has an access key, except OK and Cancel, which `Enter` and `Esc` press, as in Windows' own dialogs. Each catalogue chooses its own letters, unique within each menu and each dialog. The main window's letters also differ from the menu bar's, so an access key never opens a menu by accident; in English, `P` belongs to Match path, so the inspector's button reads "Copy a&s path". The six attribute combo boxes share one letter, on the Attributes label, which focuses the first.
- A message says what happened in one sentence, then what to do (see "Feedback"). It never blames the person and never shows a code alone.

### Search box

The search box shows the cue banner "Find files and folders" while it is focused and empty (`EM_SETCUEBANNER` with `wParam` TRUE), because the window opens with the box focused. Clear, right of the box, empties it as `Esc` does and is disabled while the box is empty; it has no access key, because `Esc` is its key. Beside it are the match controls:

| Control | Choices | Command line |
| --- | --- | --- |
| M&atch: (drop-down list) | Start of words (default), Start of name, Anywhere, Whole words, Regular expression | `wordstart:`, `startwith:`, a plain term, `-ww`, `-r` |
| Match &case (check box) | off by default | `-case` |
| Match &path (check box) | off by default | `-p` |

- **The match mode is one drop-down list,** because its modes exclude each other: a regular expression reads the whole text as one pattern, so no other mode applies to it, and separate check boxes would allow combinations that mean nothing.
- **Match case and Match path are check boxes,** because each is one independent choice that a check box shows at a glance, as the Find dialogs of Windows do. A push-like toggle shows its state only as a slightly darker button.
- **The modes apply to name terms.** A path term, a wildcard and a search function keep their own meaning, so `ext:pdf para` finds PDF files with a word that starts with `para` (see "Word start and name start"). Start of words differs from SearchHelper in two ways that keep gozler's own rules: accents count, because ignoring them would need a second folded copy of every name in the daemon's index, and a typed term is matched whole, so `report.txt` finds `my report.txt`.

### Searching

- Search as you type starts a search 150 ms after the last keystroke. With it off, `Enter` runs the search.
- While a search runs, the list keeps the previous results and status part 1 says "Searching…". The new results replace them at once, scrolled to the first row (`T1`, `T2`).
- When more than 100,000 rows match, status part 1 says "Showing the first 100,000 of 3,204,000 results". The order still covers every match: the first 100,000 by name are the first 100,000 of all matches by name.
- When the Location folder does not exist, the window does not search, as `gozler` refuses the same search (`S8`).
- `F5` runs the search again. Results do not update by themselves when files change, because the protocol has no change notification.

### Results list

| Column | Default | Content |
| --- | --- | --- |
| Name | shown; cannot be hidden | the name, with its extension's icon |
| Path | shown | the folder of the full path |
| Size | shown | the size in KB, rounded up, as File Explorer shows it, so sizes line up for comparison; for a folder, its folder size; "Unknown" for an unknown size |
| Date modified | shown | the local date and time in the Windows short formats |
| Extension | hidden | the extension; empty for a folder |
| Attributes | hidden; not sortable | the attribute letters, as `gozler` prints them |

- **The matched part of a name or path is bold,** as in Everything, so the person sees why a row matched: `para 50` makes "Para" and "50" bold in `Paracetamol 500mg.pdf`. `gozler-core` gives the ranges, so the bold part is always what the daemon matched. A regular expression shows its match; filter clauses show none.
- A cell whose text is cut off shows the whole text in a tooltip.
- A header click runs the search again in that order, and a second click reverses it. The first click on Size or Date modified sorts largest or newest first, as gozler's sort keys do. The header shows the arrow, and View > Sort by shows the same choice and gives the keyboard a way to sort.
- View > Columns and a right-click on the header show and hide columns. Column order, widths and visibility are saved.
- **Icons** come from `SHGetFileInfoW` with `SHGFI_USEFILEATTRIBUTES` and `SHGFI_SYSICONINDEX`, once for each extension, so Windows gives the extension's icon without reading the file. A per-file icon, such as an `.exe`'s own, would read every file from disk.
- A right-click on a row that is not selected selects it first, as in File Explorer, and a right-click below the last row shows no menu. Typing in the list does nothing, because a virtual list must find the row itself (`LVN_ODFINDITEM`) and most rows are not in the window; `Ctrl+F` goes to the search box.

### The selection

The selection is a set of files, fixed when the person makes it. Every action on it acts on those files, however long the person waits and whatever changes in the index meanwhile, and no action refuses (window rule 10).

- **The list owns which positions are selected.** The model never rebuilds the selection from the list's notifications, because comctl32's rules for anchors, Ctrl+Shift, the marquee and the keyboard would then have a second, incomplete copy. One action sends several notifications (`LVN_ITEMCHANGED` for one item or with item -1, `LVN_ODSTATECHANGED` for a range), so the handler posts itself one message and then reads the selected positions once with `LVM_GETNEXTITEM`. At most 100,000 rows are selected, so the read is at most 100,000 in-process messages, tens of milliseconds.
- **Every selected row cached:** the selection is those rows, the files the person sees.
- **Otherwise** the selection is being read: 150 ms after the last change, one `Selection` request asks for the rows from the first selected position to the last. Until it arrives, status part 2 says "Reading the selection…", the commands that act on the selection are disabled, and their keys do nothing (`S12`, `T6`). The request thread keeps the selected rows and the rows of the cached chunks inside the span, and drops every other row, so a Ctrl+click on row 0 and on row 99,999 carries two files and at most the cache. Its chunks replace the cached ones, so the highlighted rows show the files that the selection holds, from the same moment.
- **The protocol's one limit.** A selected row that the person never saw is the file that was at that position when the selection was read, because the daemon keeps no result set and results carry no file ID. A result snapshot in the daemon would make it exact.
- **The files** are kept in selection order, each with its full path, whether it is a folder, its size and its date, and at most once. Open, Open file location, Properties, Search in this folder, the three copies and the inspector's sums read them, and no action fetches.
- **A new search, sort or refresh** clears the selection, in the same step that accepts the new rows.

### Copy

Copy, Copy as path and Copy name read the selection's files. `ClipboardData` in `core` knows each format's exact size before it writes a byte: for `CF_HDROP`, the `DROPFILES` header with `fWide` set and the double-NUL path list, and for the other two, the text. `win::clipboard` allocates one `GMEM_MOVEABLE` block of that size, which `SetClipboardData` requires, and `core` writes into it once, so a copy holds no second buffer. After a successful `SetClipboardData` Windows owns the block; after a failure the window frees it. When another program holds the clipboard, the window tries again five times over 100 ms, then shows its message.

### Status bar

| Part | Shows |
| --- | --- |
| 1. Results | "1,204 results", "Showing the first 100,000 of 3,204,000 results" or "Searching…"; "· Filtered" is added while a filter is set and the filter pane is hidden (`S10`) |
| 2. Selection | "3 selected · 12.4 MB", with the files' sizes only; "3 selected" when only folders are selected; or "Reading the selection…" |
| 3. Activity | an export's progress and result, such as "Exporting… 120,000 rows" and "Exported 2,011,814 rows to results.csv". Cancel export clears it; the result stays until the next export starts |
| 4. Index | the index state, only when it changes the answer, in the width left |

Part 4 shows the service's failure when the list keeps its rows or another message, so the window still says that the service stopped (`T10`); "C:\ is indexing — results may be incomplete." or the word for an offline or failed volume while a volume in scope is not complete; and "Some sizes and dates on D:\ are unknown." when `metadata_pending` is set and the search tests a size or a date. When "Check for updates" finds a newer release, it says "Gozler 0.4.0 is available. Click here to open its page."

### Window lifetime

- **One window per user session, unless the person allows more.** `main` creates the named mutex `Local\gozler.ui`, and the `Local\` namespace is already one per session. A second start finds the first window by its class name, calls `AllowSetForegroundWindow` for its process, posts it a registered message and exits; the first window comes forward, restored when it was minimized and still maximized when it was maximized, with the search box focused and its text selected (`MainWindow::show_window`, `C24`). A second start with `--location <folder>` also sets Location, through `WM_COPYDATA`. With `--data-dir`, the mutex and the class name end in an FNV-1a hash of the folder (`app::instance_suffix`), because a mutex name allows no backslash after `Local\`, so a test's window and the person's window never hand over to each other.
- **"Allow only one Gozler window at a time"** (Settings > General, on by default, as Total Commander's "Allow only 1 copy of Total Commander at a time") decides what a second start does. Off, it opens another window in its own process, and `--location` sets that window's Location. Another window has no tray icon, no shortcut to show Gozler and no update check, because the first window does those; Close ends it; and it saves only what its Settings applies, so closing it never undoes a setting that another window saved. A setting that two windows change is the one saved last. A start at sign-in never opens another window. `main` reads `ui.json` before the mutex, so the second start knows the choice.
- **"Keep Gozler running in the notification area when I close its window"** (Settings > General, on by default, as Everything's "Run in background"):

  | | Off | On |
  | --- | --- | --- |
  | Close (`Alt+F4`, the title bar's X) | exits | hides the window |
  | Tray icon | none | shown while Gozler runs |
  | The shortcut from any program | works while the window is open | always works |
  | At sign-in, with "Start Gozler when I sign in" on | the window starts minimized | Gozler starts in the notification area, with no window |
  | Exit (File > Exit, the tray menu) | exits | exits |

- **The tray icon** uses `Shell_NotifyIconW` with `NOTIFYICON_VERSION_4`. A click or `Enter` on it shows the window (`NIN_SELECT`, `NIN_KEYSELECT`); its menu holds Show window, bold and the default, and Exit, the words of TinyTorrent2's tray, and its tooltip is "Gozler". Before its menu opens, the window calls `SetForegroundWindow`, as the `Shell_NotifyIconW` documentation requires so that the menu closes on a click elsewhere. Explorer drops every tray icon when it restarts, so the registered `TaskbarCreated` message adds the icon again.
- **The first time** the window hides, a Windows notification (`NIF_INFO`) says "Gozler is still running. Press {shortcut} or click its icon in the notification area to show it. Choose Exit from its menu to stop it." `ui.json` records it, so it shows once.
- Hiding the window changes nothing else: an export goes on, and status part 3 shows its result when the window shows again. Exit ends every request and the process (`T14`).

### Commands

`core::commands` holds each command's menu, group and default keys (window rule 9), and Settings > Keyboard changes the keys. A disabled command stays in its menu, gray, so the menus never change shape. "The selection is read" means that it holds its files.

| ID | Command | Enabled when | Acts on |
| --- | --- | --- | --- |
| C1 | Open | 1 to 15 rows selected, and the selection is read | each entry: a folder with the "Open folders" command, a file with the "Open files" command |
| C2 | Open file location | as C1 | the folder of each entry, with the entry selected, or the "Open file location" command |
| C3 | Properties | exactly 1 row selected, and the selection is read | the shell's Properties dialog for the entry |
| C4 | Export results… | the list shows results and no export runs | every match of the shown search, not only the rows the list reaches |
| C5 | Cancel export (in place of C4) | an export runs | the export; its partial file is deleted |
| C6 | Settings… | always | — |
| C7 | Exit | always | ends Gozler |
| C8 | Copy | the list is focused with rows selected and read, or the search box has selected text | the files, as `CF_HDROP`, as File Explorer copies them; or the selected text |
| C9 | Copy as path | rows selected, and the selection is read | each full path in double quotes on its own line, as File Explorer's Copy as path |
| C10 | Copy name | as C9 | each name on its own line |
| C11 | Select all | always | the focused control: the search box's text, or every row |
| C12 | Search in this folder | exactly 1 row selected, and the selection is read | sets Location to the entry's folder, or to the folder itself |
| C13 | Filters | always | shows or hides the filter pane |
| C14 | Inspector | always | shows or hides the inspector pane |
| C15 | Columns ▸ one item per column | always; Name cannot be hidden | shows or hides the column |
| C16 | Language ▸ Windows default, each shipped language | always | the window's language (see "Language") |
| C17 | Refresh | a search has run | runs the shown search again |
| C18 | Find | always | focuses the search box and selects its text |
| C19 | the match controls | always | the match mode or option; runs the search again |
| C20 | Sort by ▸ each sortable column; Ascending, Descending | always | the sort; runs the search again |
| C21 | Clear filters | a filter is set | sets every filter group to its first choice |
| C22 | Search syntax | always | a dialog with examples, the search terms, the search functions, the attribute letters and notes; no options, sort keys, shell quoting or exit codes, because none applies in the search box |
| C23 | About Gozler | always | a dialog with the version |
| C24 | Show window | Gozler runs in the notification area | shows the window and selects the search box's text |

Open and Open file location stop at 15 entries, because File Explorer asks before it opens more than 15 items, and the window would rather not open many windows by accident. An empty open command does what File Explorer does (see "The General page").

### Menus

Groups are separated by a separator, in this order:

- **File:** Open, Open file location, Properties | Export results… or Cancel export | Settings… | Exit
- **Edit:** Copy, Copy as path, Copy name | Select all | Find
- **View:** Filters, Inspector, Columns | Sort by | Refresh
- **Help:** Search syntax | About Gozler
- **Language,** aligned right on the menu bar (`MFT_RIGHTJUSTIFY`): Windows default | each shipped language by its own name
- **The context menu on a row:** Open, Open file location | Copy, Copy as path, Copy name | Search in this folder | Properties
- **The tray menu:** Show window | Exit

The menu bar has four menus besides Language, as TinyTorrent2's has, and each holds what its name says: File acts on the selected files and the window, Edit on the clipboard and the selection, View on what the window shows, and Help on how to use it. There is no Search menu: a menu closes after each click, so setting two match controls took two trips, while the strip beside the search box shows and changes them in one place. Find is in Edit, where Notepad has it, and Sort by in View, where File Explorer has it. A context menu shows no shortcuts, as File Explorer's does. The shortcut that a menu item shows comes from the same row as its accelerator, so they cannot differ, and the modifier names come from the catalogue, because German Windows writes `Strg` for `Ctrl`.

### States

What the list area and status part 1 show. Each failure's message is in "Daemon failures".

| ID | State | List area | Status part 1 |
| --- | --- | --- | --- |
| S1 | Starting, before the first reply | empty, no message | Searching… |
| S2 | A new search runs | the previous rows | Searching… |
| S3 | Results | the rows | "1,204 results" |
| S4 | More than 100,000 results | the first 100,000 rows | "Showing the first 100,000 of N results" |
| S5 | No results | "No files match.", with Clear filters when a filter is set and Search everywhere when Location is set | "0 results" |
| S6 | The empty search, with "Show every file and folder when the search is empty" off | "Type to search." | empty |
| S7 | The search does not parse | the parser's message and how to fix it, such as "dm: needs a date, such as dm:today or dm:2026-01-31" | empty |
| S8 | The Location folder does not exist | "The folder D:\Old does not exist.", with Choose folder… and Search everywhere | empty |
| S9 | A daemon failure | the failure's message and its button | empty |
| S10 | A filter is set and the filter pane is hidden | the rows | "· Filtered" added |
| S11 | An export runs | unchanged | unchanged; part 3 shows its progress |
| S12 | The selection is being read | unchanged | unchanged; part 2 says "Reading the selection…" |
| S13 | A volume in scope is not complete | the rows | unchanged; part 4 says so |

### Transitions

What changes, and what does not, when an event arrives. The `Model` test for a row has the row's ID in its name, such as `t3_a_failure_while_rows_are_shown_removes_them`.

| ID | Event | What the person sees | Work in progress |
| --- | --- | --- | --- |
| T1 | The search text, a match control, a filter or the sort changes | the previous rows and selection stay; part 1 says Searching… | the running search is cancelled; an export and a selection read go on |
| T2 | A new search's first rows arrive | the new rows replace the old ones, scrolled to the top; the selection is cleared, and its commands are disabled | a selection read for the old search is cancelled |
| T3 | A search fails while rows are shown | the rows are removed and the failure's message shows | a selection read is cancelled; an export goes on and ends by itself |
| T4 | Status shows that the daemon answers again, or that every volume in scope is complete | the search runs again | the status poll stops when nothing needs it |
| T5 | A volume in scope starts or ends indexing | part 4 changes; the rows stay | the status poll starts or stops |
| T6 | The selection changes while it is read | part 2 says Reading the selection…; its commands stay disabled | the old read is cancelled; one new read starts 150 ms after the last change |
| T7 | The selection changes while a folder's size is counted | the inspector shows the new selection | the old size request is cancelled |
| T8 | An export ends while a search runs | part 3 shows the export's result; part 1 still says Searching… | none |
| T9 | An export fails | a message box names the file and Windows' reason; part 3 is cleared | the partial file is deleted |
| T10 | The service stops while the Settings dialog is open | the dialog stays usable; its Drives and service page shows the failure; the main window shows it behind | the status poll runs |
| T11 | New results arrive while the context menu is open | the menu stays open; a command chosen from it acts on the selection when it is chosen, and does nothing when the selection was cleared meanwhile | none |
| T12 | The language changes | every caption changes; nothing else does | none; no search runs |
| T13 | The window moves to a monitor with another DPI | the layout and fonts scale; nothing else changes | none |
| T14 | Gozler exits while work runs | the window closes at once | every request is cancelled; a partial export is deleted; the settings are saved |

### Feedback

Each kind of message has one place, so the person always knows where to look:

- **A lasting fact** about the results or the index is in the status bar.
- **The result of a search** that has no rows, or failed, is in the list's empty area, with a button for the fix when there is one.
- **The progress and result of an export** are in status part 3.
- **A failed action that the person started** is a message box owned by the window. It names the file and Windows' reason and says what to do, as Notepad does for a failed save.
- Nothing shows success for an action whose result the person can see, such as Open or Copy. No command destroys anything, so there are no confirmations.

The message boxes cover an export that cannot write its file, a clipboard that another program holds, a file to open that no longer exists, an open command that cannot start, settings that cannot be saved, and a shortcut that another program holds at start. A failed save shows once per session, because the changes still apply until Gozler exits. The shortcut's message shows the first time the window shows in that session, because at sign-in Gozler starts without a window.

### Daemon failures

A failure empties the list, and its empty area shows the message and its button. The window then recovers by itself: after a daemon restart or a late service start, it searches again with no action from the person (`T4`). The service states are the ones `gozler` tells apart for its fix-it messages (see "Exit codes"):

| Failure | Button |
| --- | --- |
| The service is stopped | Start the service |
| The service is disabled | Turn on the service: it starts automatically from then on |
| The service is not installed, and `gozlerd.exe` is beside the window | Install the service |
| The daemon is older than the window, and `gozlerd.exe` is beside the window | Update the service: it stops the service and runs the window's `gozlerd install`, which points the service at that `gozlerd.exe` and starts it |
| The service is not installed, and `gozlerd.exe` is missing; or the daemon is older than the window and `gozlerd.exe` is missing | Download Gozler |
| The service's state cannot be read | Open Services |
| The service runs but does not answer yet; or the connection is lost, or the pipe cannot be opened or read | none: the window polls |
| The owner check fails | none: the message says to restart Windows, so that the Gozler service starts first |
| The daemon refuses the request | none: the message says to change the search, followed by the daemon's English detail |

The buttons run the commands that `gozler` prints for the same failure. A button that runs them as administrator shows the administrator shield, as Windows' own buttons that ask for permission do. `run_fix` joins them with `&&` into one `cmd /c`, which `win::shell::run_as_administrator` runs with the `runas` verb, so Windows asks for permission once, a failed command stops the ones after it, and the window never sees or passes credentials. The status poll that the failure started then notices the service and searches again. Download Gozler and Open Services open the releases page and `services.msc` without elevation.

### Filter pane

A pane on the left, shown and hidden with View > Filters. Its width follows its widest measured caption. When the window is shorter than the pane, the pane scrolls, because its list boxes need about 630 px at 100 %, and at 150 % a 1080p screen cannot hold them beside the rest of the window. Clear filters is at the top, then one group per filter. Groups combine with AND, and with the typed text; a group left at its first choice adds nothing.

Each group except Location produces one clause of query text, such as `ext:mp3;wav`. The window parses that text on its own, with gozler's parser, and adds its clauses to those of the typed text, as `gozler` does for `-r` and `-parent`. So the parser stays the only owner of the syntax, and nothing the person types changes what a filter means: an unclosed `"`, a trailing `|` or a typed `ext:` stays inside the typed text.

| Group | Control | Choices | Adds |
| --- | --- | --- | --- |
| Kind | list box | All, Folders, Files, Audio, Archives, Documents, Executables, Pictures, Videos | `folder:`, `file:`, or `ext:` with the group's extensions, from one table in `core` |
| Location | list box, with Choose folder… below it | Everywhere, each indexed volume by its mount prefix, and the chosen folders | the scope, as `-path` |
| | check box | Only directly in this folder | the parent folder, as `-parent` |
| Date modified | combo box | Any time, Today, Yesterday, Last 7 days, Last 30 days, Last 365 days, Custom… | `dm:today`, …, `dm:2026-01-01..2026-03-31` |
| Size | combo box | Any size, the size words with their ranges, such as "Tiny (up to 10 KB)", Custom… | `size:empty`, …, `size:1mb..10mb` |
| Attributes | one combo box each for Hidden, System, Read-only, Link, Compressed and Encrypted | Any, Only, Exclude | `attrib:h` or `!attrib:h` |

- **Location** lists the volumes from status, and a volume that is not complete shows its phase after its name ("D:\ (indexing)"). Choose folder… is a button, not a list item, so moving through the list with the arrow keys never opens a dialog. It opens the Windows folder picker (`IFileOpenDialog` with `FOS_PICKFOLDERS`), and the chosen folder becomes a selected list item; Cancel keeps the previous choice. Each chosen folder, also from Search in this folder (`C12`), stays an item of its own, so the person can go back to an earlier one, and choosing a listed folder again selects it. The list keeps the 5 latest, as many as it shows rows, and the oldest leaves first. They are saved with the other filter choices, and Clear filters keeps them. A path wider than the list loses its middle, as Windows shortens a path, so its drive and last folder stay visible, and its tooltip shows it whole.
- **Custom…** opens its dialog when the drop-down list closes on it, or on `Enter`. Moving onto it with the arrow keys while the list is closed opens nothing and keeps the previous clause, and Cancel in the dialog returns the combo box to the previous choice. Custom date has From and To, each with a check box: From alone adds `dm:>=FROM`, and To alone `dm:<=TO`. A new range opens with From on, at 7 days ago, and To off, because a range from today alone is what Today already gives. Custom size has Minimum and Maximum, either of which may be empty, and a unit. A custom range shows as its own item above Custom…, such as "At least 1 MB", so the combo box always shows the filter that is set.
- **No counts beside the choices,** because a count is one totals search per choice, which multiplies the cost of every keystroke.
- **Other attribute letters** are left to `attrib:` in the search box, because sixteen combo boxes would bury the six that people use.

### Inspector pane

A pane under the list, behind a splitter, shown and hidden with View > Inspector. Its height is saved, and the first height fits all seven fields of one entry. Values are read-only edit controls, so the person can select and copy any part of them, as in File Explorer's Properties. Its sums come from the selection's files, so they need no request of their own.

- **One entry:** the icon, the name, the buttons Open, Open file location and Copy as path, then Folder, Full path, Type (Folder, or File and the extension in capitals without its dot, such as "File (PDF)", as File Explorer writes "PDF File"), Size (the formatted size and the exact bytes, or "Unknown"), Date modified (to the second), Attributes (each as a word) and Volume (its mount prefix and phase, such as "C:\ (live)"). For a folder, Size shows the number of entries below it and the summed size of its files, from a totals search scoped to the folder, as `gozler -get-folder-size` does; while it runs, the field shows "Calculating…", and a new selection cancels it (`T7`).
- **Several entries:** how many files and how many folders, and the summed size of the selected files, with unknown sizes counted apart: "12.4 MB, 2 files unknown". Folder contents are not added, because a selected folder and a selected file below it would be counted twice. While the selection is read, both say "Reading the selection…".
- **Nothing selected:** the number of matches, and under it the gray hint "Select a file to see its details here.", so the empty pane says what it is for.

### Export

File > Export results… opens the Windows save dialog (`IFileSaveDialog`) with the formats CSV, TSV and TXT. The export is one `Export` request with no limit, so it writes every match of the shown search, not only the rows the list reaches. The request thread writes each page through `gozler_core::output` as it arrives, so a CSV or TSV file holds the same bytes as `gozler -export-csv FILE` with `-size`, `-dm`, `-ext` and `-attributes` for the visible columns, and a TXT file holds the full paths, as `-export-txt` writes them. Name and Path are parts of the full path, so the full path stands for both. The thread sends a progress reply about twice a second, which status part 3 shows. A failure to create or write the file is the export's own outcome, apart from the daemon failures, and a cancelled or failed export deletes its partial file.

### Settings dialog

File > Settings… opens a dialog with a page list on the left and the page on the right, as Everything's Options dialog does: General, Search, File Explorer, Keyboard and Drives and service. Each page is a child-dialog template with its name as a heading, its controls in group boxes and, on the pages with settings, Restore defaults at its bottom right, which sets that page's controls to their defaults without applying them. Each group box holds more than one control, as Windows' guidelines ask.

- **A search box across the top of the dialog**, with Clear at its right and the page list and the page in a row below it, keeps the pages whose name or captions hold every word typed, each at the start of a word, ignoring case, so a few letters find a setting without matching inside words: "upd" finds General, by "Check for updates", and "dates" finds nothing. The shown page stays while it is kept; otherwise the first kept page shows, and when none is kept, the page area says "No setting matches the search." A setting that Apply refuses shows its page, and empties the search when the search left that page out. On every page, a caption that holds a word of the search shows in bold, because the page list says only which page holds the setting; a page's heading keeps its own font.

- **The settings change what Gozler does, never how it looks.** The look follows File Explorer and Windows ("Look"), because the owner does not want appearance settings. An option of Everything comes in only when it changes what Gozler finds or does, and Gozler does not already decide it (see "Not planned").
- **General, Search, File Explorer and Keyboard are the person's settings,** saved in their `ui.json`. Drives and service shows the service's state, which one service keeps for every user of the PC, so anyone can read it without administrator permission. The service settings that change what the index holds wait for their own plan (`docs/plans/service-settings.md`).
- The dialog edits its own copy of the settings (window rule 1). While it runs, Windows disables the main window and the accelerators do not run, but replies still reach the main window, which handles them as usual. Apply and OK check the input (see "Dialogs"), then give the copy to the model and save it. The Drives and service page tells the model when it is shown or hidden, so the main window's status poll serves it, and the page keeps its own 2-second timer only to show the model's latest status.

| Page | Group | Setting | Default |
| --- | --- | --- | --- |
| General | — | Language | Windows default |
| | Starting and closing | Start Gozler when I sign in to Windows | off |
| | | Check for updates when Gozler starts | off |
| | | Keep Gozler running in the notification area when I close its window | on, as in Everything |
| | | Allow only one Gozler window at a time | on |
| Search | Search box | Search as you type | on; off, `Enter` runs the search |
| | | Show every file and folder when the search is empty | on, as in Everything; off, the empty search shows nothing (`S6`) |
| | | Select the search text when I click the search box | on |
| | | Select the search text when Gozler is activated | off |
| File Explorer | — | Show "Search with Gozler" on folders | off |
| | Open with | Open folders, Open files, Open file location: each an edit box with Browse… | empty |
| | | Use Total &Commander: a button | — |
| Keyboard | Show Gozler | Shortcut to show Gozler from any program: a hotkey control, a "Windows key" check box and Clear, which removes the shortcut as `Backspace` in the box does | none |
| | Commands | Show commands containing; the list of commands; Shortcuts for the selected command, with Add…, Edit… and Remove | the defaults of "Commands" |
| Drives and service | Drives | each fixed NTFS or ReFS drive, with its state and its entry count | — |
| | What the index holds | names, sizes, folder sizes, dates modified and attributes: always held, because the search, the filters and the Size column need them | — |
| | Service | the service's state and memory, and `gozler --status` for the full report | — |

"Select the search text when Gozler is activated" acts only when the person switches to Gozler from another program; closing one of Gozler's own dialogs leaves the focus where it was.

### The General page

- **Keep Gozler running in the notification area** decides what Close does and whether the shortcut works after the window closes (see "Window lifetime").
- **Start at sign-in** writes the `gozler` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, `"<this gozler-ui.exe>" --sign-in`, which starts Gozler as "Window lifetime" says. The check box shows on only when that value exists and Windows has not turned it off: Task Manager's Startup apps page and Windows Settings turn an entry off without removing it, by a `gozler` value under `...\Explorer\StartupApproved\Run` whose first byte is odd. That format is not documented; Task Manager writes 2 for on and 3 for off. Turning the check box on deletes that value, and off deletes both. The registry is the only copy, so the check box and Task Manager never disagree. Uninstalling Gozler removes both values.
- **Allow only one Gozler window at a time** decides what a second start does (see "Window lifetime").
- **Check for updates** asks `https://api.github.com/repos/SynoSoftware/goz/releases/latest` with WinHTTP, from its own thread, at most once a day when Gozler starts. A newer tag shows in status part 4, and a click opens the release page; Gozler never downloads or installs anything. A failed request is ignored, because no update is worth a message. Off, Gozler makes no network request.

### The File Explorer page

The page holds how Gozler works with File Explorer and the programs that open what it finds, as Everything's Context Menu page does, apart from General, so each page has room for its settings.

- **"Search with Gozler" on folders** writes the `gozler` verb under `HKCU\Software\Classes\Directory\shell` and `...\Directory\Background\shell`, with the command `"<this gozler-ui.exe>" --location "%V"`, which opens Gozler with Location set to that folder; a running Gozler takes the folder over. Off, and uninstalling Gozler, remove it.
- **The three open commands,** as on Everything's Context Menu page: Open folders runs for Open (`C1`) on a folder, Open files for Open on a file, and Open file location for `C2`. A person who browses with another file manager, such as Total Commander, sets them, so Gozler opens everything there. An empty command does what File Explorer does: a folder opens in File Explorer, a file starts its default program, and Open file location opens the folder with the entry selected. Each empty box shows that as its cue, so the person sees what happens without a separate choice. In a command, `%1` is the entry's full path, as in Everything and Windows' own commands, so a command copied from inside Everything's `$exec(...)` works as it is. With several entries selected, the command runs once for each. A line under the three boxes says what `%1` is and what an empty box does.
- **Browse…** opens the Windows file dialog for programs and writes the chosen program in quotes, followed by ` "%1"`. The three buttons have no access key, because one letter cannot name three buttons; Tab reaches them.
- **Use Total Commander** fills Open folders and Open files with `"<path>" /O /A /P=L /L="%1"`, because nobody remembers Total Commander's options, and leaves Open file location alone. It finds Total Commander in the index with one search, `file: totalcmd*.exe` sorted newest first, and takes `TOTALCMD64.EXE`, or `TOTALCMD.EXE` when there is no 64-bit one; of several, it takes the newest by date modified, because an older copy is usually a backup. The box shows the path, so the person sees which one it took. When it finds none, a balloon on the button says so and the boxes keep their text; while the service cannot search, the balloon gives the service's message. The dialog stays responsive while the request runs, and closing it cancels the request. Like every change on the page, the filled text applies with OK or Apply.

### The Keyboard page

The page has two parts, as Everything's Keyboard page has: the one shortcut that shows Gozler from any program, and the shortcuts of the window's commands. `win::shortcut` owns the first, and `MainWindow` sees only "show the window".

- **The shortcut to show Gozler needs the Windows key, or two of `Ctrl`, `Alt` and `Shift`,** because a shortcut with one of them, such as `Ctrl+F`, would take that key from every program. The hotkey control turns such a combination into `Ctrl+Alt` and the key, as the Shortcut key field of a Windows shortcut does. On keyboards with AltGr, `Ctrl+Alt` with some keys types a character, such as `@` with `Ctrl+Alt+Q` on a German keyboard; a person whose character stops working chooses another shortcut.
- **The Windows key** is a check box beside the hotkey control, because the Windows hotkey control cannot enter it. With it on, the key alone is enough: `Win+S`. A line under the control says to press the keys together and that Backspace removes the shortcut, because the control shows no caret and does not say how to clear it.
- **A shortcut with `Ctrl`, `Alt` and `Shift` only** is a `RegisterHotKey` on the main window. When another program holds it, the dialog says so, and Gozler never takes such a shortcut from another program.
- **A shortcut with the Windows key** is a low-level keyboard hook (`WH_KEYBOARD_LL`), because Explorer registers most Windows-key combinations, so `RegisterHotKey` fails for them. Gozler thus replaces Windows' own use of the shortcut, such as `Win+S` for Windows Search, while it runs with that shortcut, and it changes no Windows setting to do so. The hook runs on its own thread with its own message loop, because Windows removes a low-level hook whose callback does not return within `LowLevelHooksTimeout`, and the window's thread can be inside a modal loop. The callback swallows the key's down and up events while a Windows key is held and posts the show message to the window; before the Windows key comes up, it sends one unassigned key (`0xE8`), so the shell sees a combination and does not open Start. The hook exists only while a Windows-key shortcut is set, so choosing another shortcut, none, or Exit gives Windows its key back at once, with nothing to restore and no restart or sign-out. Windows keeps two exceptions: `Win+L` always locks the computer, so the dialog refuses it, and while a program that runs as administrator is in front, its keys reach Windows, not Gozler.
- **After the window closes,** the shortcut works only while Gozler keeps running in the notification area, and the page says under the shortcut whether it does, so the person knows before they close the window.
- **The command list** shows every command that is one item of the menu bar or the context menu, as its menu and its name, such as "Edit > Find", with its shortcuts. A submenu of choices, such as Columns, Sort by or Language, the match controls, Clear filters and Show window take no command shortcut. "Show commands containing" keeps the commands whose name or shortcut contains the text, so `ctrl+e` finds the command that has it. Add… and Edit… open a small dialog, titled "Add shortcut" or "Edit shortcut", with one hotkey control labelled with the command, such as "Shortcut for Edit > Find:", and a command can have several shortcuts, as Find has `Ctrl+F` and `Ctrl+E`.
- **A command shortcut needs `Ctrl`, `Alt` or a function key.** `Alt` with a letter or a digit is refused, because those keys open the menus and reach the window's controls ("Words"), and a key with neither `Ctrl` nor `Alt` is refused unless it is a function key, because the search box needs it for typing. A shortcut that another command has is refused with that command's name, so two commands never share one. Keys that belong to a control keep their meaning and are refused: `Tab`, `Enter`, `Esc`, `Shift+F10`, `Alt+F4`, `Ctrl+V`, `Ctrl+X`, `Ctrl+Z`, and `Ctrl` with an arrow, `Home`, `End`, `Page Up`, `Page Down`, `Backspace` or `Delete`.
- **The menus show the person's shortcuts,** so what a menu says is always what the key does. Restore defaults gives every command its default keys again.

### The Drives and service page

- The drive list shows each fixed NTFS or ReFS drive by its mount prefix and label, such as "C:\ Windows", with its state and its entry count. A live drive whose size and date pass failed or stopped reads "Live, some sizes and dates unknown". Removable drives, network drives and other filesystems are not in the list, because the daemon does not index them (see "Index coherence").
- The page refreshes its states and counts every 2 seconds while it is shown.
- Folder sizes cost about 8 bytes a folder, 2.2 MB for the 270,137 folders on the owner's machine against 196 MB for the whole index, so they are always held and need no setting.

### Dialogs

Every dialog follows the same rules, so the person never has to look for a button:

- The buttons are at the bottom right, in the Windows order: OK, Cancel, then Apply where there is one. About and Search syntax have one button, Close.
- OK is the default button and `Enter` presses it; `Esc` is Cancel.
- Apply is disabled until a value changes. OK applies and closes. Cancel closes and changes nothing that Apply has not already applied.
- Focus starts on the first field, and Tab moves through the fields in reading order.
- A dialog checks its input when OK or Apply is pressed. An invalid field shows a balloon tip that says what to enter, focus moves to it, and the dialog stays open.

Custom date needs at least one check box on, and From on or before To; with neither on, OK stays disabled. Custom size takes numbers with the Windows decimal separator, at least one of them, and Minimum no larger than Maximum. The open commands take any text, so a command that cannot start shows its message when it is used ("Feedback"). The shortcuts follow "The Keyboard page", and each refusal names what to choose instead.

The window also does what a Windows user expects without being told: Shift+F10 and the Menu key open the context menu at the focused row, `Alt+Enter` opens Properties, as in File Explorer, and Tab and Shift+Tab move between the search box, the match controls, the filter pane, the list and the inspector, in that order. Every task works with the keyboard alone and with the mouse alone.

### Where settings are saved

`%LOCALAPPDATA%\Gozler\ui.json`, found through the Windows known-folder API, is written by the window alone: to a temporary file, then renamed over the old one. A value that is missing or damaged falls back to its default, and a failed save shows its message ("Feedback"). `gozler-ui --data-dir <folder>` keeps `ui.json` and `ui-error.log` in another folder instead, so a test never touches the person's settings. It holds two groups:

- `settings`: the settings above, with the language as a language tag, such as `de`, or absent for Windows default.
- `window`: the placement, column order, widths and visibility, the sort, whether the filter pane is shown and its choices, whether the inspector is shown and its height, the match controls, and whether the tray notification has been shown.

The keys are the file format, so renaming a key needs a migration.

### Language

The window's text is translated, and the person changes the language while the window runs.

- **Two places choose it.** The Language menu, the last item of the menu bar, aligned right, with a radio mark on the current choice, is one click from the window and stays in the same place in every language. The Language list at the top of Settings > General is where Everything and TinyTorrent2 keep it. Both list "Windows default", then each shipped language by its own name, such as "Deutsch", so a person who picked a language they cannot read can still find their own, and both show even when only English ships, so the window has one layout.
- **At once.** A click in the Language menu changes the language immediately, with no restart, and saves the choice; in Settings, OK or Apply does the same. `MainWindow::set_language` loads the new `Strings`, records the choice and the strings in a statement of its own, and runs `apply_text` inside `Hwnd::draw_once_after`, which turns drawing off with `WM_SETREDRAW` so the window draws once. A combo box or list box gets its items again with the same selection index; a selection set by a message sends no `CBN_SELCHANGE` or `LBN_SELCHANGE`, so no filter changes and no search starts. The typed search, the results, the selection, the scroll position and the focus are never touched, and no search runs again, because the results do not depend on the language (`T12`). The switch takes about 100 to 150 ms on the owner's laptop, which is accepted for a rare, deliberate choice. After Apply, the open Settings dialog keeps its text and shows the new language the next time it opens, because changing the text of four open pages would need a second caption pass for one moment.
- **Windows default** is the choice until the person makes another. It takes the first language in the Windows display language list that the window ships, or its parent language (`pt` for `pt-BR`), and English when none matches. Numbers, dates and times follow the Windows regional format in every language, because the region is a separate Windows choice: Windows formats them (`GetNumberFormatEx`, `GetDateFormatEx`, `GetTimeFormatEx`), through a `Numbers` value that `win` implements and the tests implement in English, and a `WM_SETTINGCHANGE` with `intl` runs `apply_text`.
- **What stays in English:** the search syntax, such as `ext:` and the size words, because a query must mean the same in every language, so Help > Search syntax shows it from the same tables as `gozler --help`: the examples that use no option, in their search-box form, and the notes that hold for a search box, plus two notes on the Match list, because the term descriptions describe Anywhere and a regular expression there reads the whole box; the detail that the daemon or a library gives, such as why a regular expression does not compile, which follows a translated sentence, because the daemon sends English text and no code; paths, names, exported files and copied text, which are data; and Windows' own dialogs, which follow the Windows display language. The parser's messages are translated in full, with what to do, because a mistyped search is the failure a person meets most.
- **Catalogues.** `resources/en.json` holds every key and its English text, and another language is `<tag>.json` with the same keys, grouped by surface in one level of objects with snake_case names, such as `menu.export`. Placeholders are numbered, `{0}`, so a translation may reorder them. A counted message has an `_one` and an `_other` form; the first shipped language that needs another plural rule takes it from the ICU that Windows includes (`uplrules_select`), and no ICU is bundled. Text is plain data, and a menu item, dialog label or button holds its access key as `&`, so each language chooses its own letters. Size units, such as "MB", are catalogue messages.
- **Loading.** `include_str!` embeds each catalogue, and a table in `core::text` lists each language's tag, its own name and its catalogue, so listing the languages loads none. The loader rejects a key it has already seen, because `serde_json` keeps the last of two equal keys without a word. A message comes from the language, then its parent language, then English, always whole with all its plural forms; a translated message whose key English lacks, or whose placeholders differ from English, is ignored, so the English shows. A key that English lacks shows as its dotted name, so a person sees the gap and nothing panics. A catalogue that does not parse is a build defect that the catalogue test catches; at run time the window keeps its current language. Another language ships when the owner adds its catalogue, which changes data only.
- **The core reports codes, not sentences.** The window maps each `QueryError` variant with its typed reason and each `VolumePhase` to a key with a `match`, which fails to compile when a variant is added. Attribute letters are `char`s, so the catalogue test checks that every letter has its key.
- **The pseudo-language** `qps-ploc`, the tag Windows uses for its own long-text pseudo-locale, is generated from `en.json` by `build.rs`, only with the crate's `pseudo-language` feature. A message of up to 12 characters becomes twice as long and a longer one 50 % longer, with accented letters, keeping placeholders and access keys, so a screenshot shows where a control still clips. A shipped executable never contains it.

### Memory budget

The release `gozler-ui.exe` stays under 16 MB of private bytes in steady state: after the empty search has been scrolled to its last reachable row, with no selection held. The full cache of 32,768 rows takes about 6.5 MB, at about 200 bytes a row for the average full path of about 110 characters. When a commit raises the measured figure by more than 1 MB, its message says why. Two actions that the person starts use more, only while they last: a large selection holds its files, about 20 MB for 100,000 rows, until it changes, and a large copy builds its clipboard data once, about 22 MB for 100,000 paths, which Windows owns after the copy. The window stays responsive during both: no message waits more than 100 ms.

### How normal changes land

| Change | Where it lands |
| --- | --- |
| A new command | a row in "Commands", a row in `core::commands`, and its `MainWindow` method |
| A new column | a `Column` entry in `core`, and its arm in the `LVN_GETDISPINFO` handler |
| A new filter | `Filters` and its clause text, one control, and its rectangle in `Layout::compute` |
| A new Settings page | one child-dialog template and its dialog procedure |
| Dark mode | a `win::dark` module, which `win::controls` calls for each main-window control and `win::dialog`'s procedure for each dialog, whose procedure then answers `WM_CTLCOLOR*` |
| A new text | a key in `en.json`, and the function that formats it (window rule 12) |
| A new language | its `<tag>.json` in `resources/` and a row in the language table |
| A right-to-left language | the main window and the dialogs created with `WS_EX_LAYOUTRTL`, which mirrors their coordinates, so `Layout::compute` does not change; whether a switch between directions must create the controls again is not verified |
| A second top-level window | another window procedure with the same trampoline |
| A daemon result snapshot | a new request for rows of a result snapshot, and a selected row the person never saw becomes exact; the list reaches every match, a row whose file is gone and an expired result snapshot get states of their own, and the selection gets its own limit of 100,000 rows (`docs/plans/result-snapshot.md`) |
| Results that update when files change | a new outcome, after the protocol gets change notification |

## Not planned

Each of these waits for a concrete need:

- Created dates: the index does not hold them, and they cost daemon memory, so the service settings plan makes them a choice (`docs/plans/service-settings.md`).
- Content search: `rg` does that.
- `< >` grouping of terms.
- Paging: es's `-viewport-offset` and `-viewport-count` are unknown options (exit 6). The pipe protocol keeps its `offset` field for other applications.
- EFU file lists, playlists, bookmarks, macros, and the size and date format menus.
- In the window: search and run history; results that update while files change, because the protocol has no change notification (`F5` searches again); rows past the first 100,000, which need a result snapshot in the daemon; the other hard links of a file, because results carry no file ID; and right-to-left languages, because no such catalogue is planned (see "How normal changes land").

These do not fit gozler:

- A second command-line grammar beside es's: `/NAME:value` options, `/F`, `/D`, `/EXT:` and `/NOT:` lists, `AND`/`OR`/`NOT` keywords, and exact-path lookups keyed on a trailing separator.
- Results filtered by the caller's file permissions: gozler's trust model shows metadata to every local user (see `SECURITY.md`).
- Accessed dates: Windows can turn their updates off (`NtfsDisableLastAccessUpdate`), so they can mislead.
- A scheduled-task install: gozlerd is a service.
- Appearance settings in the window, such as fonts, colors, size and date formats and editable menus, and polish such as Mica, a custom title bar, custom-drawn buttons or taller rows: the owner wants Windows and File Explorer to decide the look, polish that needs a paint routine for every state costs more code than it gives, and a list at Windows' compact height shows more results.
- Rename, delete and drag out of the window's list: file operations are not search, and Open file location gives File Explorer for them.
- A second window, or a window run as administrator: one window serves a session, and only the service needs administrator rights.
- Staging all output before printing it: exit codes 2 and 7 already mark a late failure.
- Saving the index between runs: the file would keep every name on the volume on disk after gozlerd stops, where backups and disk images copy it, and a damaged or planted file could be served as `live`. One MFT pass already builds a volume of 2 million entries in about 10 s.

## Golden corpus

The golden corpus records what gozler prints for a set of command lines over fixed data, so a change that alters behavior fails a row. The daemon's corpus (`gozler-daemon`, `server::tests::corpus`) runs each command line through `parse_argv` and `build_search` into the daemon's request path over a fixture index, and one row as protocol 1's text query. The output corpus (`gozler-core`, `output::results::corpus`) prints a fixed result set through `ResultWriter`, which `gozler` and the window's Export both write through, in each output format. Dates render in UTC, so every row passes in any time zone.

goz v0.2.0 returned a wrong answer that looked valid, or hid a limit, for each command line below. A corpus row or test marked `Bug N` keeps bug N fixed.

| # | Command line | goz v0.2.0 | gozler |
| --- | --- | --- | --- |
| 1 | `gozler C:\repo\docs\testing.md` | nothing, because paths were matched without the drive | that file: a drive path is a path term |
| 2 | `gozler dm:today`, or any unknown function | nothing: searched as text | the function, or exit 4 for an unknown one |
| 3 | `gozler /a-d report` | nothing: `/a-d` searched as text | files only |
| 4 | `gozler report -n 20` with 300 matches | 20 rows, exit 0, no sign of more | the same rows, a warning, exit 10 |
| 5 | any query while a volume indexes | results and a warning, exit 0 | the same, exit 2 |
| 6 | `gozler report -size -dm` | paths only | size and date columns |
| 7 | `gozler -help` | exit 6 | the help, exit 0 |
| 8 | a pipe server that fails the trust check | exit 9, es's code for no results | exit 3 |
| 9 | `gozler docs/agents/testing` | nothing | the same matches as with `\` |
| 10 | `gozler 'path:"C:\Program Files"'` | searched as a name | quotes delimit a function's value, and a quoted term with a separator is a path term |
| 11 | `gozler "*\docs\*"` | nothing: wildcards matched names only | a wildcard with `\` matches the whole full path |
| 12 | `gozler size:0..` on `C:` | 627 fewer files than `gozler file:`: names of hard-linked files had no size or date | each name of a hard-linked file gets its size and date (issue #6; issue #47 holds the check on a live `C:`); the corpus pins that a file with an unknown size never matches `size:` |
