# Design

## Workspace

gozler is six crates:

- **`gozler-core`**: pure, OS-agnostic. USN, MFT and folder-listing parsing, the index, the query engine, the wire protocol, CSV output, and `es`-compatible argv parsing. Tests run on any OS.
- **`gozler-winfs`**: the thin `unsafe` Win32 layer (`windows-sys` only). Buffers, typed structs, and opaque handles; it never parses an ioctl record payload. It depends on nothing else in the workspace, enforced by cargo-deny.
- **`gozler-daemon`** (`gozlerd`): bootstrap, USN journal tail, the tokio named-pipe server, and Windows service integration.
- **`gozler-client`**: the blocking named-pipe client, with the state of the gozler service and the scope-folder check, which `gozler` and the window share. Its error types build on every OS; the pipe is Windows only.
- **`gozler-cli`** (`gozler`): the `es`-compatible CLI.
- **`gozler-ui`** (`gozler-ui`): the gozler window, a native Win32 search window. Its `core` module decides what the window shows and holds no Win32 type, so its tests run on any OS; its `win` module holds every Win32 call; `app` joins them. `docs/plans/ui-design.md` describes it.

The command line splits by one rule: what asks the operating system is in `gozler-cli` and `gozler-client`, and everything else is in `gozler-core`, where CI tests it on Ubuntu. `gozler-core` owns the parsing, the vocabulary tables, the help text, the date resolver, the request built from a working directory and a query start it is given, the evaluation, and the output rows. `gozler-cli` reads the arguments, the working directory, the clock and the time zone, resolves drive-relative folders, talks to the pipe through `gozler-client`, and writes to stdout or the export file.

## Index coherence

gozler indexes fixed NTFS and ReFS volumes; removable drives and other filesystems (exFAT, FAT32) are out of scope and are not tracked. The set of indexed volumes follows the machine: within a minute, a fixed NTFS or ReFS volume that appears (an attached VHD, an unlocked BitLocker drive, a Dev Drive) is indexed, and a volume that is removed leaves the index. A `-path` scope on no indexed volume is an error (exit 4, with a message that names the folder), never an empty answer, because an empty answer reads as "no such file".

The daemon answers queries from the moment it starts. A volume is `bootstrapping` until its first build finishes, and like `rescanning` it marks results incomplete; a volume that cannot be opened is retried on the same budget as a tail that exits, then reported `failed`.

Every start builds each index from the volume, because nothing is saved between runs. Each build starts from the root folder's file ID, read from the volume: record 5 on NTFS, and ReFS's own. On NTFS the build reads the MFT once, with `FSCTL_QUERY_FILE_LAYOUT`, which gives each file every name, its attributes, size and date; it is also the size and date pass of "Sizes and dates". One pass instead of two halves the MFT read after a boot (measured on 2 million entries: 10 s instead of 22 s), and that read is most of the time a volume stays `bootstrapping`. When that pass stops part way, the MFT enumeration (`FSCTL_ENUM_USN_DATA`) runs into the same index and names the files the pass did not reach, so a failed read never drops a file from the index; their sizes and dates stay unknown. Every name on the volume is indexed, the NTFS metadata files such as `$Boot` and those under `$Extend` included. A saved journal cursor on its own must never skip that build: it would tell a fresh, empty index that it is current, and the daemon would serve an empty volume as `live`. The index itself is never saved (see "Not planned").

ReFS has no MFT and answers neither request, so a ReFS build walks the folders from the root. It opens each folder by its 128-bit file ID, with backup semantics so that permissions hide no name, and lists it with `FileIdExtdDirectoryInfo`, which gives each name its file ID, attributes, size and date; the walk is also the size and date pass. A folder reparse point, such as a junction, is indexed but not entered. The walk is slow next to an MFT read: 30 s for the 250,000 entries of a Dev Drive with a cold cache, about 24 times longer per entry. ReFS file IDs are 128 bits, so a ReFS volume reads its journal as V3 records (`READ_USN_JOURNAL_DATA_V1`), and its index keys entries on the whole file ID. NTFS keeps V2 records, because asking it for V3 records switches every record to V3, and its index keeps 8 bytes per file ID.

A walk lists one folder at a time, so, unlike an MFT read, it can meet an entry twice or miss one that moves. The journal cursor is taken before every build, so each change made during the walk replays. A name the walk meets again never renames its entry: a file met in a second folder is a hard link and keeps both names, and a folder met again moved during the walk, so its replayed rename record moves it. A hard link renamed during the walk can be listed twice or never, so until the replay first finds nothing new, every renamed file also has its links reconciled (see "Hard links"). A folder moved during the walk from a part not yet listed into one already listed is never listed, and the journal records no change to what it holds. So a rename record for a folder the index lacks makes the tail list that folder and everything under it, and one for a file the index lacks makes the tail read the file's size and date. A folder that is being deleted, which Windows allows only when it is empty, is skipped; a listing that fails for any other reason starts a rescan.

The index is a cache of the filesystem, and a cache that cannot prove it is coherent must rebuild itself. Every sync-loss path (journal wrap, a parse anomaly, a journal that cannot be read at startup, reads that stop answering, even a sick disk that leaves a journal read pending forever, which a supervisor cancels, or a tail thread that exits outright, which the same supervisor revives on a bounded budget) ends in one of three states:

- **in sync**, provably (phase `live`);
- **rescanning**, with a full rescan scheduled (phase `rescanning`); or
- **offline** (phase `offline`, or `failed` once retrying has stopped), reported as no longer tracking. A volume that can never have a journal is `snapshot`: complete as of bootstrap, and said so once in `--status` instead of on every result page.

There is never a silent fourth state.

Some irregularities do not lose sync and are not rescanned. A delete for a file ID the index does not hold has nothing to remove. A record number that NTFS has given to a new file has its stale entries removed in place. Both are normal while the journal replays changes made during the build, so a rescan would rebuild the same index; they are counted, and `--status` shows the counts. An entry whose walk to the root is longer than the hop limit has no full path and is left out of results. That happens briefly when the replay applies an old move on top of the newer scan, which can close a loop in the parent links until the later moves arrive, and permanently for a real folder tree that deep, which a rescan would rebuild the same way. Neither case starts a rescan. `--status`, the daemon handshake, and every result page decide a volume's state with a single shared rule, so a client can always tell whether the results it just got are complete.

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

- **The version gate.** The client sends `Request::Hello` with `PROTO_VERSION` as both its lowest and highest version. The daemon serves every version from `OLDEST_PROTO_VERSION` up to its own and answers with the highest version both sides speak, or refuses with `Unsupported`. A daemon older than the client therefore refuses, and the client prints `gozler: the running gozlerd is older than this client; install the matching gozlerd` and exits 7. Without the gate, an old daemon would search a new function as text and find nothing with exit 0, or reject the request with a serde error that the client would report as invalid input. So `gozler` and `gozlerd` install together, from one build.
- **Each change an older daemon cannot serve raises `PROTO_VERSION` by one**, such as a new kind of term in the parsed query. The parsed query only grows: new variants, never changed ones. A protocol 1 client sends its text query without a `Hello`, and the daemon still serves it.
- **Results pages are positional binary** (`proto::binary`), so a field an older client does not know would make it misread every item. A new item field therefore travels only when the request asks for it: the attributes need `Rows::want_attributes`, which no client before protocol 6 sends.
- **Totals cross the pipe without rows.** `-get-result-count`, `-get-total-size` and `-get-folder-size` ask for `Returns::Totals`, and the daemon sums the sizes it already holds for each hit. Summing on the client would stream every match, and `-get-folder-size C:\` is the whole volume. The sum uses checked arithmetic; an overflow, which only a corrupt size in the index can cause, is `ProtoError::Overflow` and exit 1.

## Not planned

Each of these waits for a concrete need:

- Created and accessed dates: the index does not hold them.
- Content search: `rg` does that.
- `< >` grouping of terms.
- Paging: es's `-viewport-offset` and `-viewport-count` are unknown options (exit 6). The pipe protocol keeps its `offset` field for other applications.
- EFU file lists, playlists, bookmarks, macros, and the size and date format menus.

These do not fit gozler:

- A second command-line grammar beside es's: `/NAME:value` options, `/F`, `/D`, `/EXT:` and `/NOT:` lists, `AND`/`OR`/`NOT` keywords, and exact-path lookups keyed on a trailing separator.
- Results filtered by the caller's file permissions: gozler's trust model shows metadata to every local user (see `SECURITY.md`).
- A scheduled-task install and a tray icon: gozler has its service.
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
