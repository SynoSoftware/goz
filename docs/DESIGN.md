# Design

## Workspace

gozler is seven crates:

- **`gozler-core`**: pure, OS-agnostic. USN, MFT and folder-listing parsing, the index and the steps that bring it up to date after each journal batch, the folder walk, the query engine, the wire protocol, CSV output, and `es`-compatible argv parsing. Tests run on any OS.
- **`gozler-winfs`**: the thin `unsafe` Win32 layer (`windows-sys` only). Buffers, typed structs, and opaque handles; it never parses an ioctl record payload. It depends on nothing else in the workspace, enforced by cargo-deny.
- **`gozler-daemon`** (`gozlerd`): bootstrap, USN journal tail, the tokio named-pipe server, and Windows service integration.
- **`gozler-client`**: the blocking named-pipe client, with the state of the gozler service and the scope-folder check, which `gozler` and the window share. Its error types build on every OS; the pipe is Windows only.
- **`gozler-cli`** (`gozler`): the `es`-compatible CLI.
- **`gozler-ui`** (`gozler-ui`): the gozler window, a native Win32 window that searches, copies, moves and deletes files and, in Commander mode, browses folders. Its `core` module decides what the window shows and holds no Win32 type, so its tests run on any OS; its `win` module holds every Win32 call; `app` joins them. "The window" describes it.
- **`gozler-fileops`**: the engine of the window's file operations: copy, move, delete, rename and new folder, with a queue of operations that wait for each other. It runs in the window's process with the person's own permissions (see "File operations"). It holds no `unsafe`; its Win32 calls go through `gozler-winfs`. "The copy, move and delete engine" describes it.

The command line splits by one rule: what asks the operating system is in `gozler-cli` and `gozler-client`, and everything else is in `gozler-core`, where CI tests it on Ubuntu. `gozler-core` owns the parsing, the vocabulary tables, the help text, the date resolver, the request built from a working directory and a query start it is given, the evaluation, and the output rows. `gozler-cli` reads the arguments, the working directory, the clock and the time zone, resolves drive-relative folders, talks to the pipe through `gozler-client`, and writes to stdout or the export file.

## Index coherence

gozler indexes fixed NTFS and ReFS volumes that have a drive letter or a mount folder; removable drives, other filesystems (exFAT, FAT32) and volumes with no mount, such as Windows' hidden recovery partition, are out of scope and are not tracked, because no program opens a path on a volume without a mount. The set of indexed volumes follows the machine: within a minute, a fixed NTFS or ReFS volume that appears (an attached VHD, an unlocked BitLocker drive, a Dev Drive) is indexed, and a volume that is removed leaves the index. A `-path` scope on no indexed volume is an error (exit 4, with a message that names the folder), never an empty answer, because an empty answer reads as "no such file". The Include settings can leave out a volume, and folders and names on it (see "Include settings").

The daemon answers queries from the moment it starts. A volume is `bootstrapping` until its first build finishes, and like `rescanning` it marks results incomplete; a volume that cannot be opened is retried on the same budget as a tail that exits, then reported `failed`.

Every start builds each index from the volume, because nothing is saved between runs. Each build starts from the root folder's file ID, read from the volume: record 5 on NTFS, and ReFS's own. On NTFS the build reads the MFT once, with `FSCTL_QUERY_FILE_LAYOUT`, which gives each file every name, its attributes, size and date; it is also the size and date pass of "Sizes and dates". One pass instead of two halves the MFT read after a boot (measured on 2 million entries: 10 s instead of 22 s), and that read is most of the time a volume stays `bootstrapping`. When that pass stops part way, the MFT enumeration (`FSCTL_ENUM_USN_DATA`) runs into the same index and names the files the pass did not reach, so a failed read never drops a file from the index; their sizes and dates stay unknown. Every name on the volume is indexed, the NTFS metadata files such as `$Boot` and those under `$Extend` included. A saved journal cursor on its own must never skip that build: it would tell a fresh, empty index that it is current, and the daemon would serve an empty volume as `live`. The index itself is never saved (see "Not planned").

ReFS has no MFT and answers neither request, so a ReFS build walks the folders from the root. It opens each folder by its 128-bit file ID, with backup semantics so that permissions hide no name, and lists it with `FileIdExtdDirectoryInfo`, which gives each name its file ID, attributes, size and date; the walk is also the size and date pass. It reads each folder's security descriptor from the handle it lists the folder with (see "Visibility"). A folder reparse point, such as a junction, is indexed but not entered. The walk is slow next to an MFT read: 30 s for the 250,000 entries of a Dev Drive with a cold cache, about 24 times longer per entry. ReFS file IDs are 128 bits, so a ReFS volume reads its journal as V3 records (`READ_USN_JOURNAL_DATA_V1`), and its index keys entries on the whole file ID. NTFS keeps V2 records, because asking it for V3 records switches every record to V3, and its index keeps 8 bytes per file ID, keyed by record number. The daemon decides the width from the volume's filesystem (`Filesystem::ids`, `VolumeIndex::with_ids`), never from the root's ID, so a ReFS index keys whole IDs from before its first build.

A walk lists one folder at a time, so, unlike an MFT read, it can meet an entry twice or miss one that moves. The journal cursor is taken before every build, so each change made during the walk replays. A name the walk meets again never renames its entry: a file met in a second folder is a hard link and keeps both names, and a folder met again moved during the walk, so its replayed rename record moves it. A hard link renamed during the walk can be listed twice or never, so until the reads pass the journal's next USN at the walk's end, or first find nothing new, every renamed file also has its links reconciled (see "Hard links"). The tail's listing of missed folders, below, is a walk too, so its changes replay the same way. A folder moved during the walk from a part not yet listed into one already listed is never listed, and the journal records no change to what it holds. So a rename record for a folder the index lacks makes the tail list that folder and everything under it, and one for a file the index lacks makes the tail read the file's size and date. A folder that is being deleted, which Windows allows only when it is empty, is skipped; a listing that fails for any other reason starts a rescan.

The index is a cache of the filesystem, and a cache that cannot prove it is coherent must rebuild itself. Every sync-loss path (journal wrap, a parse anomaly, a journal that cannot be read at startup, reads that stop answering, even a sick disk that leaves a journal read pending forever, which a supervisor cancels, or a tail thread that exits outright, which the same supervisor revives on a bounded budget) ends in one of three states:

- **in sync**, provably (phase `live`);
- **rescanning**, with a full rescan scheduled (phase `rescanning`); or
- **offline** (phase `offline`, or `failed` once retrying has stopped), reported as no longer tracking. A volume that can never have a journal is `snapshot`: complete as of its last build, and said so once in `--status` instead of on every result page.

There is never a silent fourth state.

Some irregularities do not lose sync and are not rescanned. A delete for a file ID the index does not hold has nothing to remove. A record number that NTFS has given to a new file has its stale entries removed in place. Both are normal while the journal replays changes made during the build, so a rescan would rebuild the same index; they are counted, and `--status` shows the counts. An entry whose walk to the root is longer than the hop limit has no full path and is left out of results. That happens briefly when the replay applies an old move on top of the newer scan, which can close a loop in the parent links until the later moves arrive, and permanently for a real folder tree that deep, which a rescan would rebuild the same way. Neither case starts a rescan. `--status`, the daemon handshake, and every result page decide a volume's state with a single shared rule, so a client can always tell whether the results it just got are complete. A request reads each volume's phase while it holds that volume's index, so the state a page reports is the state of the index that answered it.

A reply reads its rows from the index again when it builds each page (see "Rows on demand"), so a page shows each file as it is then, and leaves out a row that no longer answers the search as it ran: a row whose file was deleted or whose slot holds another file, a row that left the search's folder scope, and a row whose folder has no full path now. Each page's total is the match count less the rows left out so far, because `gozler` reads the last page's total and exits 10 when it shows fewer rows. A file renamed or moved inside the scope may no longer match the search, and is sent anyway, with its new name. A volume whose index a rebuild replaced, or whose entries `optimize_storage` gave new indexes, sends no more rows, and the pages after it set `volumes_incomplete`, so the client knows the answer is incomplete.

## Hard links

A hard-linked file is one record with several names, and a USN record names only one of them.

- **Everything 1.4** never sees a hard link created after its index was built. Per the [Everything documentation](https://www.voidtools.com/support/everything/), *"changes made to a hard link will only update the first hard link entry."* Only a manual **Tools -> Options -> Indexes -> Force rebuild** picks it up. This is a long-standing, documented limitation.
- **gozler** reconciles hard links live. A hard-link-change record names only one link (and may name a dead one), so gozler walks the file's real link set over Win32 (`FindFirstFileNameW` / `FindNextFileNameW`) and reconciles the index to exactly that set: names added or removed are applied, not just size and dates. It finds each link's folder by the folder's file ID, not by its path, because during the replay of a build the index may not yet know a folder's current name. A walk that cannot complete (it failed part way, or a link's folder is not indexed yet) is skipped rather than applied as a partial set, counted, and surfaced by `--status`, and a rescan trues it up. A file that is gone by the time of the walk is left to its delete record. While the tail replays the changes made during a folder walk (see "Index coherence"), every renamed file has its links reconciled this way too, because the walk can list a renamed link twice or never. The gap is visible, never silent.
- **Everything 1.5** (beta) does track new and renamed hard links.

## Include settings

The Include settings decide which drives, folders and names the index holds, for every user of the PC. Settings > Include edits them (see "The Include page"), and `gozler_core::include` owns their rules, so the service, its count and the window read them alike.

- **Only an administrator saves them.** They are the value `Include` of `HKLM\SOFTWARE\Gozler`, as JSON (`IndexSettings`). Only administrators and the system can write that key, so Windows proves who saved the settings, and the pipe needs no trust check of its own (`docs/SECURITY.md`). Every user can read it. A value that is missing or does not parse gives the defaults, which index everything, and gozlerd logs why.
- **`SettingsSaved` makes gozlerd read the value again** and apply it when it differs from what gozlerd applies. Any client may send it, because it can only apply what an administrator saved. A gozlerd that is not running reads the value when it starts.
- **A rebuild is asked for the same way.** Rebuild on the Drives and service page writes a new token, the time, to the value `Rebuild` of the same key, then sends `SettingsSaved`, so the pipe needs no request and no trust check of its own. gozlerd rebuilds every indexed volume when it reads a token it has not seen; the token saved when it starts asks for nothing, because a start builds every volume. A volume's tail rebuilds it between journal batches (`RescanReason::Asked`), which does not count against the rescan budget, and a volume with no tail gets one, as for changed rules below. One value of `VolumeState` says what a volume's index owes (`Owed`), and a build clears it, so a rebuild and a rules change that arrive together cause one build, and a request during a build causes one more. When the supervisor gives up on the tail started for a rebuild, the volume fails with "the rebuild asked for did not run".
- **The nearest check box at or above an entry decides.** Each drive has a check box, and a folder has one only where it differs from the folder above, so a new folder follows the folder above it, and a folder that is renamed or deleted loses its own choice. To index only `C:\Users\ana`, a person unchecks `C:\` and checks `C:\Users\ana`. Each drive is keyed by its volume GUID path, so a new drive letter keeps its choices, and keeps its name as last seen for the row of a drive that is not connected. A drive without an entry follows "Include new fixed drives automatically"; the window saves an entry for every drive it shows, so a drive is new only until the next save.
- **Name patterns, Exclude hidden and Exclude system** leave out a matching entry, and a matching folder with everything in it, also below a checked folder. A pattern matches the name with `*` and `?`, ignoring case.
- **A folder above a checked folder stays as the path to it,** so the checked folder has a full path. The search finds that folder, but none of its other entries.
- **A drive with nothing checked is not indexed.** The supervisor removes it at once, as it removes a drive that is gone, and status lists it as left out, so the window shows it as "Left out" and leaves it out of Location. A drive that becomes checked is added from the supervisor's last list of the machine's volumes, without waiting for the next scan.
- **A build reads the whole volume, then removes what the rules leave out** (`VolumeIndex::apply_rules`), because a folder's path is known only once every folder is read. It then compacts the entry table, so the memory of the removed entries returns.
- **A change acts in each volume's tail thread,** so it never races a journal batch. A change that only leaves more out removes the entries in place. A change that keeps anything the index does not hold (`VolumeRules::adds_to`) rebuilds the volume, which reads as `rescanning` meanwhile; that rebuild does not count against the rescan budget. A volume with no tail, a `snapshot` or one whose tail the supervisor gave up on, gets a new tail, as after a restart, whose first build reads the volume with the new rules, even when they only leave more out. Until that build begins, a `snapshot` volume reads as `offline`, so its results are flagged incomplete while its index follows the old rules, also while the new tail fails to start; when the supervisor gives up on it, the volume fails with "the changed Include settings were not applied".
- **The journal follows the rules.** The index keeps the file ID of each folder it left out, so a record inside one is dropped, and a change below an unchecked folder costs nothing. Every other record is checked before it is applied: an entry that moves or is renamed into a left-out place is removed with what is below it; a folder that moves out of a left-out folder is listed, as the missed-folder walk lists a folder after a rename; and an entry whose attributes change so that it is no longer left out is read as an entry the build missed. A hard link keeps only the names the rules keep.
- **Dates created keeps each entry's date created,** for `dc:`, `-dc` and the window's Date created column. It is a field of the `Include` value (`IndexSettings::dates_created`), not a value of its own, so `SettingsSaved` applies it by the route above; a value saved before it existed reads as off. It is off by default, because it adds 8 bytes per entry to the service's memory. Turning it on keeps what the index does not hold (`VolumeIndex::lacks_for`), so it rebuilds each volume, because only a read of the volume brings the dates; turning it off drops them in place. While it is off, gozlerd refuses a query with `dc:` as invalid input, with a message that names the setting (`IndexSettings::allows`), because an empty answer would read as "no file was created then". A `-dc` column or a sort by date created is not refused: its dates are unknown, as a file's date is when the size and date pass could not read it.
- **`CountIncluded` counts without changing anything:** for each indexed volume, the entries that would stay under the edited settings (`VolumeIndex::count_kept`). The index does not hold what the applied settings leave out, so what edited settings add back is counted only once it is indexed. gozlerd answers only a user who sees every file (see "Visibility").

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

An answer is incomplete (exit 2) for two causes: a volume in scope is not ready (indexing, rescanning, offline or failed), or the query tests or totals sizes or dates on a volume whose size and date pass failed (see "Sizes and dates"). Rows that only show sizes from such a volume warn and exit 0, because every row is there. `--status` exits 2 when any volume is not complete or the size and date pass of any volume failed. `-timeout MS` polls the daemon's status until the volumes in scope are complete or MS runs out, then searches anyway; a volume still incomplete then makes the answer exit 2. The client finds the volumes in scope with `Scope::of`, the rule the daemon routes by, so the two cannot disagree.

## Search

### The client parses, the daemon evaluates

The client parses the query with the `gozler-core` parser and sends the parsed query, its scope and what it wants back (`SearchRequest`). The client resolves everything that depends on where and when it runs: the working directory, the time zone and the query start. The daemon only evaluates.

The arguments are joined with spaces and split into terms again, as goz v0.2.0 does, because scripts build a query in one string: `gozler "ext:cs size:>1mb"` is two terms. Quotes make spaces and operators plain text, also in a function's value: `path:"C:\Program Files"`.

A query is AND-ed clauses, each an OR of terms that may be negated. That covers `!` and `|` with es's precedence, where `|` binds tighter than the space, and keeps the evaluator flat. The parser refuses with exit 4 what would otherwise search text that no name contains. An unknown function such as `dc:` is one: Windows names cannot contain `:`, so as text it gives an empty answer that looks valid. An unquoted `<` or `>` outside a function value is another, because Windows names cannot contain them either. es groups terms with `< >`; gozler does not.

Regular expressions use Rust's `regex` crate, which matches in linear time, so no pattern can hang gozlerd. The price is that a pattern has no look-around and no backreferences.

**Word start and name start.** `wordstart:` matches a term where a word of the name starts: at the start of the name or after a character that is not a letter or digit, so `wordstart:1234` finds `IMG_1234.jpg` and `wordstart:para` does not find `comparator.txt`. A term that starts with such a character starts its own word, so `wordstart:.dll` finds `7z.dll`, as a person typing `.dll` expects. `startwith:` matches at the start of the name. With `-p`, or with `\` or `/` in the value, each tests the full path instead, as `ww:` does. Accents count, as in every comparison (`gozler_core::fold`). The window's match mode is a parse default (`ParseOptions::term_match`). Start of words and Start of name reach name terms only; a term with a separator and the values of `path:`, `nopath:`, `file:` and `folder:` stay plain substrings, so a typed folder still finds what is below it. Whole words is `-ww` and keeps its rule, which reaches those terms and values too. With `-p`, a mode tests the full path.

The folded-arena scan needs one literal that every matching name contains. Only a clause of one positive term supplies it: a name term, a whole-word, word-start or name-start name term, a name wildcard or a single extension (`CompiledClause::name_literal`). A negated term, a path term, a path wildcard and a regular expression never do. For example, the literal of `*\docs\*` is `\docs\`, which no name contains, so a scan for it would find nothing.

**Match path.** `-p` makes every typed term a path term, so the window searches as fast with Match path as without it only because most path terms need no full path. A path term without a separator lies inside one component of the full path: its text cannot span a `\`, and a `\`, like the start or end of a name, is no letter or digit, so a whole word or a word start reads the same in the component as in the path. So the full path matches the term exactly when the root's name (`C:`), the name of a folder above the entry or the entry's own name does. The scan decides that once per folder and query and passes each folder's result down to what the folder holds (`query::paths::ComponentBits`), so no full path is built. It also visits only the candidates: the folded-arena scan finds the names that contain the longest such term of a clause of one positive term, and the scan visits those entries and everything below the ones whose name passes the term, each once. When the candidates pass a quarter of the volume, or the root's name holds the term, or the query matches case, it visits every entry instead. On the bench's 2.1-million-entry volume, `-p kernel32` costs 19 million CPU cycles instead of about 380 million for a path copy of every entry, and `-p report`, which matches every file, costs what `report` does without `-p`. A path wildcard that is a `*` and then text without a `\`, `*` or `?` needs only the name, because the full path ends with `.dll` exactly when the name does, so `-p *.dll` tests the name and scans for `.dll` as `*.dll` does without `-p`; the same text between two `*`s, as in `*report*`, is a path term that the components decide. A `startwith:` without a separator, which Start of name makes of each typed word, is decided by the root's name alone, because every full path starts with it and a `\`: on `C:`, `-p startwith:report` matches nothing and `-p startwith:c` matches everything, so a volume whose root's name fails such a clause is not scanned at all, and on one whose name passes it, the components decide it with no full path. A term with a separator, any other path wildcard and a regular expression match where the whole path ends, or across a `\`, so they still copy each candidate's full path.

### Paths

- **A path term matches the full path**, as in es. A term with a drive or a separator is a path term, and it matches as a substring: `C:\repo\` finds everything below `repo`, and `C:\repo\x.md` also finds `x.md.bak`. A path wildcard matches the whole full path, so `C:\repo\*.cs` finds the `.cs` files below `repo`. A quoted term with a separator is still a path term, and a `*` in it is still a wildcard, because a path cannot hold a literal `*`.
- **`/` and `\` both separate folders**, because agents and Unix habits write `/`. The parser turns `/` into `\` in every path that a term, a function value or an option names. It never changes a regular expression, where `/` and `\` keep their regex meaning; a regular expression sees `\` separators. Output keeps `\`.
- **A token that starts with `/` and names an option is that option**, as in es (`/ad`, `/on`, `/p`). So a relative path is written without a leading `/`.
- **The client resolves relative paths.** A path term that starts with `.\` or `..\`, a relative `parent:` folder, and a relative `-path`, `-parent` or `-parent-path` folder are joined to the working directory. `gozler_core::winpath` applies the Windows rules itself, without asking the operating system, so a command line builds the same request on any machine; `std::path::absolute` would turn `C:\repo` into a Unix path on the Ubuntu CI. After the join it removes `.` and `..`, because the daemon looks up each component of a scope as a child folder. A rooted `\foo` takes the drive of the working directory. Only a drive-relative folder such as `-path D:x` is resolved by `gozler-cli`, with `std::path::absolute`, before the request is built, because only Windows knows the current folder of each drive.
- **A scope folder that does not exist is invalid input.** When the folder that `-path`, `-parent-path`, `-parent` or `-get-folder-size` selects does not exist, gozler exits 4 with `gozler: <folder> does not exist` before it connects. es gives an empty answer with exit 0, and an agent reads that as "no such file" when the folder is wrong. Only a folder that the operating system reports as missing is refused: a folder the user cannot read still searches, because gozlerd answers from its index, and the search shows what the user may see there (see "Visibility"). When `-parent` and `-path` are both given, only the `-path` folder is checked, because it is the scope.

### Dates

A `dm:` or `dc:` value resolves once, to UTC bounds, from the query start and a time zone. The date resolver in `gozler-core` (`query::dates`, with `jiff`) does it: the client passes its own clock and time zone, and the daemon's text path passes its own. The bounds travel as UTC, so the daemon never needs the client's time zone.

- `today`, `yesterday` and a `YYYY-MM-DD` date are calendar days in the time zone, from midnight to the next midnight, so a day around a daylight-saving change lasts 23 or 25 hours.
- `Ndays` is the N times 24 hours that end at the query start.
- `dm:` and `dc:` take only these forms, because they are the dates scripts and agents write. A slash date such as `03/04/2026` is a different day in another locale.
- Output prints dates as ISO 8601 in local time, and plain output groups digits with commas, whatever the locale, so a script reads the output without knowing the locale.
- A comparison compares times at the value's granularity, as es does: `>today` is after today, and `<7days` is older than the last 7 days. A range `lower..upper` runs from the start of `lower` to the end of `upper`, and a reversed range is invalid input.

### Sizes and dates

A folder's size is the sum of the known sizes of every file below it, each name of a hard-linked file counted: the number `-get-folder-size` gives for that folder, as in Everything with "Index folder size" on. `-size`, `--json` and the CSV and TSV exports show it, `size:` tests it, and the size sort places a folder by it. `-get-total-size` and `-get-folder-size` still add files only, because a folder's size is already the sum of its files. The index always keeps the sums, in each folder's own size slot, so they cost no memory and need no setting. A folder's size is unknown until the volume's size and date pass ends. Then `VolumeIndex::compute_folder_sizes` sums them once, and from then on every change adds its difference to each folder above it, so a journal change costs a walk up its folders, never a walk of their contents. A move that opens or closes a loop in the parent links (see "Index coherence") changes each folder of the loop by more than what moved, so it, and a file that becomes a folder, make the index sum every folder again; both are rare. The sums wrap at 2^64 instead of failing, so each change reverses exactly; only a corrupt size can come near that bound. **Owner ruling:** give the partial answer, not the excuse. After a pass that failed or stopped, a folder's size sums the files the pass read, because the space found tells the person more than no size, and the volume's `metadata_pending` says that some sizes are missing. `VolumeIndex::record_size_pass_stopped` then marks every folder's size as a lower bound. A size test matches such a folder only when it holds for every size from that sum up, fails it when it holds for none, and otherwise treats the size as unknown, because a folder whose unread files could take it out of the range must never be listed: `size:>1gb` still finds a folder whose read files already pass 1 GB, while `size:<1mb` and `!size:<1mb` list no folder whose read files sum to 100 KB.

A folder has a size only for a user who sees every file (see "Visibility"). For every other user a folder's size is unknown, as it is while a size pass runs: `-size` and `--json` show none, `size:` never matches a folder, and the size sort places folders as it places other unknown sizes. Files keep their sizes. `-get-folder-size` and `-get-total-size` still work, because they add the sizes of the visible files at the time of the search: 10 visible files of 500 MB in total give 500 MB, whatever is hidden beside them. **Owner decision:** this is the rule, not a missing pass, so it sets no `metadata_pending` and never causes exit 2. The reason: a size of only the visible files for each folder would need a sum for every folder above a hidden file for each such user, which is megabytes per user, an update of every such sum under the tail's write lock on every size change, and a pass over every entry when such a user first searches, while searches on that volume wait. Fast searching matters more than folder sizes for these users.

A file's size or date is unknown when the size and date pass failed or stopped, or could not read that file. A size or date test, negated or not, never matches an unknown value: `!size:>1mb` does not report an unread file as small, and `report|size:>1mb` still finds `report.txt` through its other term. Because every clause is an OR, this gives the same rows as three-valued logic without tracking unknown results for each entry.

The size and date pass runs in each build, before the volume serves as live, so a live volume never has the pass still running. A pass that failed or stopped sets the volume's `metadata_pending`, and `--status` shows it. Each results page and totals reply carries the flag for the volumes in scope. The client warns and exits 2 when the flag is set and the query tests a size or a date or asks for a size total, because that answer may be short. When the flag is set and the rows show sizes, in a Size column or JSON, or are sorted by size, the client warns that some sizes are unknown and a folder's size counts only the files read, and exits 0, because every row is there. A file that a successful pass could not read is left out without exit 2; such files are rare, and the help says so.

### Visibility

gozlerd shows each user the entries of the folders that user may list, and every entry to the users on a stored list. The index holds every name on the volume, because a build reads the volume with backup semantics, so without this filter a user could search for `C:\Users\ana\Documents\*` and read the names, sizes and dates of ana's files, and a folder's size would show how much she keeps there. Windows' permissions decide what each user sees; gozler invents no folder permissions of its own, and the stored list is the only exception.

- **A user sees an entry when they may list the folder that holds it.** "May list" means that the folder's security descriptor grants the user's token `FILE_LIST_DIRECTORY`, as `AccessCheck` decides it from the token's user, groups, restricted SIDs and integrity level, and from the security attributes that conditional ACEs test. A user who may list a folder sees every name in it, also of files they cannot open, as in File Explorer. Each hard link is its own entry, so a file with names in two folders shows under the folder the user may list and not under the other. The volume's root has no folder above it, so every user sees it (`VolumeIndex::is_seen`).
- **A full path can name folders the user cannot list.** When a folder deep in a tree is shared with a user, its entries show with their full path, also when the user may not list a folder above it. Windows shows those names to a user who types the path, because every user has "Bypass traverse checking" by default.
- **Who sees every entry.** A request sees every entry when its token's user, or one of its enabled groups, is on the stored list, or when it runs as SYSTEM, which can already read every folder and whose callers are tasks and services that work for the whole PC. **Owner decision:** no administrator exception. An administrator, elevated or not, and a service account such as Local Service or Network Service see what their token may list, unless the list names them. A user or group that the token holds only for deny does not count, so Administrators on the list lets administrators see every entry only when they run elevated. A restricted token, one that a program made to reach less than its user, never sees every entry, through the list or as SYSTEM, because the program that restricted it meant it to reach less; it is filtered by its own `AccessCheck`.
- **The stored list**, "Users and groups who see every file", is the `REG_MULTI_SZ` value `SeeEveryFile` of `HKLM\SOFTWARE\Gozler`, beside `Include`, one SID string per line (`S-1-1-0` for Everyone). There is no separate on-off setting: adding Everyone turns the filter off. One list covers every case: a person who works as a standard user for safety on their own PC puts their own account on it and sees every file without running as administrator, and an administrator adds a group or Everyone. Every user may read the list, and only administrators and SYSTEM may write it, because a user who could change it could turn the filter off for themselves. gozlerd reads it for each request, so a change applies from the next search, with no restart and no `SettingsSaved`. A missing value is an empty list. A value that does not read is logged once and counts as empty, so only SYSTEM sees every entry and every other caller is filtered by their own token, the safe answer. A line that is not a SID is skipped without a log line, because only a hand edit writes one. The uninstaller keeps the list, as it keeps `Include`, so a new install uses it again.
- **Who is asking.** `gozler` and the window open the pipe at the identification level, and gozlerd reads the token of the client that sent the last message, once for each request, not each connection, because the pipe gives the token of the last message read and the read costs a few microseconds (`access::of_pipe_client`). It impersonates the client only long enough to open the thread token, reverts at once, and keeps an identification-level copy, so gozlerd can read the token's user and groups and run `AccessCheck`, but can never act as the client; if the revert fails, the process stops, because the thread would serve the next client as this user. The client still checks that the pipe belongs to SYSTEM or Administrators before it sends anything, so a program that took the pipe name never gets a token for the user (`docs/SECURITY.md`).
- **An unknown caller is refused.** A search, an Include count, or a read or sort of a result snapshot from a client that connects anonymously gets `ProtoError::Unsupported` with the message `gozlerd could not tell which user is searching; open the pipe at the identification level`; when Windows gives no token for another reason, the message is `gozlerd could not tell which user is searching`. `gozler` exits 7, its code for `Unsupported`. `Hello`, `Status` and `SettingsSaved` still answer, because they show no name. `Status` also tells each client what it sees (`DaemonStatus::caller_sees`): every file, the files in folders it may list, or nothing, because gozlerd could not tell who it is. The Visibility page shows it, because nothing else tells a person whether the list covers them. Serving an anonymous client as nobody would give it empty answers that look like "no such file", and serving it everything would let any program bypass the filter. This breaks a program that opens the pipe anonymously, until it opens the pipe at the identification level.
- **Status and the Include count.** `--status`, `Hello` and the window's Drives and service page give whole-volume counts: a count of entries shows no name, size or date, and counting for each user would need a full scan for each status request. The Include page's preview count is the one count of the whole index, and gozlerd answers `CountIncluded` only to a user who sees every file, because its folder and name rules would otherwise let any other user count hidden entries one pattern at a time until the counts spell their names. Any other user gets `Unsupported` with `only a user who sees every file can count what the Include settings keep`. The window does not ask for a filtered user, whom the status names as one, and the preview says so (see "The Include page"). **Include decides the index; visibility decides each user's view of it:** an excluded folder is in no user's results, and an included one shows to the users the rules above allow.
- **One descriptor, kept once.** A folder usually inherits its parent's permissions, so a volume has few distinct descriptors. The index keeps each distinct descriptor's bytes once and a column with a 4-byte `DescriptorId` for each entry, set for folders only (`index::descriptors`); the column takes 8.1 MB, 4.1 % more, on the owner's `C:` of 2,015,690 entries (194.6 MB without it). The index compares descriptors only as bytes and never parses one, so `gozler-core` stays free of Windows. A freed or reused entry slot, a compaction and a folder that turns into a file all reset or move the folder's ID with the entry, because a folder in a reused slot with another folder's descriptor would show a user every name in it.
- **Reading a folder's permissions.** `read_folder_descriptor` opens the folder by its file ID with `READ_CONTROL` and calls `GetKernelObjectSecurity` for the owner, the group, the DACL and the integrity label: `AccessCheck` needs the owner and the group, the label carries the integrity level, and the SACL does not decide listing. The read is capped at 64 KiB. A descriptor larger than that, or without an owner or a group, fails the read, because `AccessCheck` refuses such a descriptor and its folders would otherwise be hidden from every filtered user with no sign in `--status`. ReFS gives no `SecurityId`, so its build reads every folder. The ReFS build and the walk of missed folders read each folder's descriptor from the handle they list it with, which costs almost nothing, so the ReFS build also reads the folders that the Include rules then remove; the tail reads the folders of a journal batch on their own. A failed read does not fail a walk. gozlerd's backup privilege grants `READ_CONTROL` whatever the DACL says, so a refused read means that something else stands in the way, such as an antivirus filter driver.
- **The NTFS build reads one descriptor for each `SecurityId`** (`read_build_descriptors`), after the Include rules, so it never reads a folder they removed. NTFS gives every file with the same descriptor the same `SecurityId`, and the layout pass gives it for each folder. Reading all 274,208 folders of the owner's `C:` took 27 s (2026-10-26); the 1,499 reads that sharing needs took 0.11 s, for 1,444 distinct descriptors. The build reads the first kept folder of an ID and gives every folder of that ID its bytes only when that folder's USN still equals the one the layout pass read, because a permission change writes a journal record and so changes the USN; reading the descriptor before the USN makes this safe. Otherwise it reads each folder of the ID on its own, as it does for `SecurityId` 0, which a folder with its own descriptor has. It also reads on its own every folder the layout gave no `SecurityId`, such as each folder that a layout pass that stopped did not reach. A change after a read is in the journal, whose cursor was taken before the build, so the replay reads that folder again. Until the replay, a folder can keep its old descriptor: one read before its change, and one that changed after the layout pass while the first folder of its `SecurityId` did not, so it got that folder's bytes. A right taken away during the build then applies to such a folder only once the build ends. Without the USN check, one folder changed during the build would give its new permissions to every folder that shared its old `SecurityId` until the next rescan. A volume without an active journal reads every folder on its own instead, because without the journal NTFS does not update a folder's USN, so the check could not tell a changed folder from an unchanged one. Every read first checks whether the build was cancelled.
- **NTFS's own folders are never read or counted.** `$Extend` (the root's child in MFT record 11) and every folder below it hold only NTFS's own files, and Windows refuses them even to the backup privilege (`VolumeIndex::wants_descriptor`). Without this rule their failed reads would show in `--status` on every PC. They have no descriptor, so their entries stay hidden from every user who does not see every file. On ReFS a reparse folder, such as a junction, is not read either: the walk lists it but never enters it, so it holds no entry, and it would count as unread for good. On NTFS it is read, because the MFT gives the entries of a reparse folder, such as a OneDrive folder, as of any other.
- **Until gozlerd has read a folder's permissions, the folder's entries are hidden** from every user who does not see every file. `apply_batch` takes the descriptor away from a folder at once, under the batch's write lock, when a record carries `USN_REASON_SECURITY_CHANGE` (`UsnOp::SecurityDirty`) and for a folder that has none, such as a new folder, and `settle_batch` reads those folders right after the apply, before the missed-folder walk and the size, date and link reads, which can take seconds. So a folder that loses a user's permission never shows that user a name added in the same batch, and a new folder's entries wait only for the reads themselves. A permission change that folders inherit writes a record for each changed folder (`SetNamedSecurityInfo`), so each is read. ReFS writes `USN_REASON_SECURITY_CHANGE` as NTFS does, for the changed folder and for a child that inherits the change (checked on a ReFS volume), so a permission change shows as fast as a rename on both. A file's security record changes nothing, because only folders have descriptors.
- **A read that fails hides the folder.** The folder stays without a descriptor, its entries stay hidden, and `VolumeStatus::unread_folders` counts it while the index holds the folder, as it counts a folder whose read is still to come; `--status` shows `Permissions   N folders whose permissions are not read yet or could not be read; such a folder's contents stay hidden from users who do not see every file until gozlerd reads its permissions`, and a volume never indexed shows no such line. A folder that is gone counts nothing, because its delete record removes it. The tail reads the folder again when the journal next records a change to the folder itself. A file added inside it is a change to that file, so such a folder can stay hidden until the next rescan, which reads every folder again. Hiding is the safe default, because showing the entries could show a name to a user who may not see it. A filtered user's answer does not say that it leaves such a folder's entries out; `--status` counts the folders instead. On the owner's `C:` no read failed, a read still to come takes milliseconds, and a mark on each answer would need a count of unread folders kept exact through every journal batch, which is the kind of count that drifted before it was computed from the index. A volume without a journal (`snapshot`) and a volume whose tail has stopped (offline or failed) show no permission change until the next rescan, as for any other change.
- **A request asks once for each distinct descriptor,** under its first hold of each volume and before the scan (`VolumeIndex::listable`). A search for a filtered user gets a `may_list` closure from gozlerd, which runs `ClientToken::may_list` and turns an error into "no", so an error hides and never shows. The answers (`Listable`) last to the end of the request: IDs are only added while an index instance lives, an ID past the answers' end is hidden, and every later hold of a page checks that the instance is the same. Nothing outlives the request, so no answer goes stale and no cache needs a key, a cap or a sweep. Asking before the scan also keeps the cost of the access checks from depending on which names match; the scan still tests hidden names, so its own time does (see "Not covered"). The cost is one `AccessCheck` for each descriptor of each volume the request reads, 1.88 µs each, measured over the 114 distinct descriptors of 20,000 folders under Program Files, ProgramData, Windows and Users; a request that sees everything pays nothing, so the owner pays nothing. The owner's `C:` has 1,444 distinct descriptors, so a filtered request pays about 2.7 ms there. The window sends a request about 150 ms after the last keystroke, for each scrolled chunk, for each selection change and for the selected folder's size, so a filtered user's busiest moment is about 5 requests a second. A cache of answers for each token was left out: its key would have to hold every token input `AccessCheck` reads (the user, the groups, the restricting SIDs, the integrity level, the access flags of the token and the security attributes from AuthZ except `TSA://ProcUnique`), because a packaged process and the user's other processes share the user and logon SIDs, and so do an administrator's elevated and filtered tokens, yet each pair lists different folders. If the checks of one request take over about 10 ms, that design returns, with an LRU cap and an expiry, and first a check that AuthZ gives `WIN://SYSAPPID` for a packaged process.
- **The table is collected when it has grown.** It only gains descriptors, such as a package update's folders, until `DescriptorTable::collect` drops the ones no folder uses, renumbers the rest and gives the index a new instance. That runs when the table holds more than twice what the last collection kept plus 1 MiB, so a user who changes a folder's permissions in a loop cannot grow the daemon's memory without bound. The accepted cost: such a loop makes other users' running searches and result snapshots on that volume start again, and a separate descriptor generation would add a check on every path that now checks the instance. Collection cannot help a volume that really has many distinct descriptors in use, such as explicit permissions on each of millions of folders: its table is large and every filtered request is slow. Only the owner's volumes were measured; `--status` shows the table's memory.
- **One test filters every answer.** `Scan::evaluate` rejects an entry that the request does not see, right after the scope test and before the orphan check and the path clauses, so no hidden entry pays for a path, and the cheap name tests have already rejected most candidates before a parent's descriptor is read. All four scan paths (the scope walk, the folded arena, the component walk and the full scan) pass through it, so a search's rows and `total`, `-get-result-count`, `-get-total-size`, `-get-folder-size`, the sums of a page, every query form and a `-path` scope follow it, and filtering comes before the sort and the limit. A page tests each row again in `find_head`, because a file can move into a folder the user cannot list after the scan and an export's pages can be read minutes later; it takes such a row off the total, as it does a row that is gone. A read of a result snapshot tests its rows the same way and reports a row the user cannot see as gone, and a sort of one builds the sorting user's answers first. `EntryView::size` gives no size for a folder when the reader is filtered, so every reader of a folder's size follows with no change of its own, and the size sort's tie runs and the merge of several volumes order folders of equal key by name, never by a real size the user may not learn.
- **A filtered answer is complete.** Leaving out entries the user may not see never causes exit 2 or a warning. Exit codes 9 and 10 count only the entries the user sees, and totals count what the user sees too. So when a folder the user may list lies below one they may not, the totals count its entries, although a copy or a delete that starts above it cannot reach them: the user's own walk cannot list the folder between, and reports that folder as an error.
- **The protocol.** No request or results page gains or loses a field for filtering. `VolumeStatus` gains `unread_folders`, the count of folders whose permissions are not read, `VolumeMemory` the memory of the descriptor table, and `DaemonStatus` `caller_sees`, so `PROTO_VERSION` is 16 and keeps no old form (see "Wire protocol").
- **The service log names no folder.** Every user may read `%SystemRoot%\Logs\Gozler\gozlerd.log`, so a build logs only how many permission reads it made and how many folders it could not read.
- **`gozlerd see-every-file` shows and changes the list.** Without an argument it prints each entry by its account name, or by its SID string when Windows has no name, one for each line; a line of the value that is not a SID is marked as ignored; an empty list says `The list is empty: each user sees the files in folders they can open.` Unless Everyone is on the list, it ends with the commands that show every file to every user, `pushd` with the folder of the gozlerd.exe that runs and `.\gozlerd.exe see-every-file add Everyone`, after a line that says to run them in an administrator terminal, so the way to turn the filter off is found where the list is. It needs no elevation, because every user may read the list. `add <account>` and `remove <account>` change the list. Neither works without elevation, and the check comes before the name is looked up, so a standard user learns at once that the change is not theirs: `gozlerd: changing who sees every file needs an elevated terminal`, then the same command to run in an administrator terminal, after `pushd` with the folder, as docs/WRITING.md asks, with the account in quotes, because a name may hold a space, an `&` or a `'`. An account is a user or group name such as `PC\ana`, `Everyone` in any case, which works on Windows in any language, or a SID such as `S-1-5-32-545`; an unknown name fails with `gozlerd: no user or group is named <account>`. Adding an entry already on the list, or removing one that is not, changes nothing and says so.
- **The setup offers the signed-in user a place on the list.** On a first install, when no `SeeEveryFile` value exists and the setup is not silent, the setup shows one page after the cleanup page, or after the license page when there is none. It is a page of its own, because removing earlier programs and granting a user are unrelated choices, and it is the last choice before the install starts. It shows the grant before anything is stored, so whoever runs the install can clear it. The page has one check box, checked, "Let PC\ana see the names of every file on this PC", with a text that says that this lets that user also see the names of files in folders they cannot open, and folder sizes, that it does not let them open those files, that a cleared box leaves that user with no folder sizes, and that Settings > Visibility changes the list later. A person who clears the box to keep other users' files private would otherwise lose folder sizes without knowing why. The text does not mention running as administrator, because an administrator who is not on the list sees most folders through their own rights but not all. The user is the user of the Windows session that runs the setup (`WTSQuerySessionInformationW`), not the account that approved the elevation, so an install that another administrator approved names the person who started it. The page shows for an administrator too, because an administrator sees every entry only through the list. The setup shows no page and adds nobody when a list is already stored, as on an upgrade or a new install after an uninstall, so that a user an administrator removed is never added back; when it runs silently, because nobody saw the choice; and when no user is signed in to its session. The setup stores the choice with `gozlerd install --see-every-file <account>`, or `--see-every-file none` for a cleared box, before the service starts, so the first search already follows it, and only when no value exists; otherwise it prints `Kept the existing list of users who see every file.` An account that does not resolve, such as an unknown name, prints a warning, and the install goes on without storing a list, so the next setup asks again. `gozlerd install` without the option, as a hand install and the window's Install fix run it, leaves the list as it is.
- **Not covered.** How long a query takes: a query that matches many hidden entries takes longer than one that matches none, and a `-path` search into a hidden folder takes longer the more entries that folder holds, so durations show roughly how many entries a hidden folder holds. Padding the response times would slow every search, and Windows already tells a user which paths exist. Central access policies: on a domain that uses Dynamic Access Control, Windows also applies central access policies and claims when a user opens a folder, but gozlerd checks the folder's descriptor against the token only, so its answer can differ from Explorer's there. Settings > Visibility edits the list (see "The Visibility page").

### Attributes

The index holds each entry's attributes in two bytes (`PackedAttributes`): one bit for each attribute letter but D, because the entry's folder flag already says D. The record's `u32` would take four bytes, and the index holds millions of entries. An attribute without a letter is dropped, so `attrib:` takes letters only.

### Result order

The same query over the same index prints the same list, and a limited list is the start of the full one. `query::order` owns the one comparison that the engine and the daemon both sort by. The sort key decides first, and an unknown size or date sorts last in either direction. Ties then break, always ascending, on the folded name, the volume, the file ID, the parent folder's file ID and the raw name. Hard links need the last two keys: each link name is its own entry, and all of them share one file ID. The raw name separates two links in one case-sensitive folder whose names differ only in case. Each key comes from the row's entry in the index, so no path is rebuilt under the index lock to break a tie. A path sort descending lists every folder's contents before the folder itself, because a folder ranks below every folder inside it.

A search sorts numbers, not names. Each name in the index has a rank, its place among the volume's folded names, and equal folded names, such as `README` and `readme`, share one. A build ranks every name before the index answers a search: 0.12 s for the 2.2 million entries of `C:\`, 0.6 to 0.8 s for 5.6 million distinct names, which cost 8 bytes each. A name the journal adds later has no rank, so a search places it among the ranked names by a binary search. When the unranked names or the removed ranked names pass 0.1 % of the names, the tail merges the new names into the ranks: 0.11 s beside searches and 3 ms under the write lock for 5.6 million names. Each match becomes one 16-byte record, compared as one number: a primary key from the sort (the name's key; the folder's rank and the name's key; the size; the date; or the extension's rank among the matched extensions), the name's key, and the entry. A descending sort inverts the primary key, and unknown sizes and dates stay last. `query::order` owns this encoding beside the comparison, and a test pins that the two agree. Matches whose keys are equal, such as hard links and the many `index.js` of a volume, form a run that the comparison orders. A limited page also orders the matches past its end whose keys equal its last one's, so a limited list stays the start of the full one. On `C:\`, a sort by name has about 271,000 such runs, which hold 1.6 million of the 2.2 million matches and order in 0.03 s.

The index keeps a rank for each distinct name, not an order of every entry, as Everything keeps its name order. A search with a literal, such as `kernel32`, tests each distinct name once and then follows the entries that share it, so a walk of every entry in name order would test more. The journal adds thousands of entries a second, and a sorted list of every entry would move on each insert. A search still needs its own list of matches, because it picks a part of the index.

Several volumes merge one page at a time, by the same comparison, reading each row's keys from its own index. Folder ranks order only one volume's folders, so for a path sort `query::order` compares two folders on different volumes by their folded full paths, which order folders as the ranks do. A volume's root path is its folded mount prefix without the last separator. When no volume's root path starts another's, such as `c:` and `d:`, every path of one volume sorts before every path of the other, so the volumes follow each other with no comparison. Otherwise the merge compares paths: volumes mounted at `C:\Data\` and `C:\Data2\` interleave, because `2` sorts before `\`, and `c:\data` starts `c:\data2`.

## Wire protocol

- **The version gate.** The client sends `Request::Hello` with `PROTO_VERSION` as both its lowest and highest version. **Owner ruling:** the daemon serves only its own version (`OLDEST_PROTO_VERSION` is `PROTO_VERSION`), of gozler or of goz, and refuses any other with `Unsupported`, because `gozler` and `gozlerd` install together from one build, and an old layout kept for an old client would bind every later change. The client prints `gozler: the running gozlerd is older than this client; install the matching gozlerd`, then the link to the gozler setup, and exits 7. Without the gate, a daemon of another build would misread the request or the client the pages.
- **Each change to a request or a reply raises `PROTO_VERSION` by one**, such as a new kind of term in the parsed query or a new page layout.
- **No result carries its full path.** gozlerd keeps each match as a row, its entry and its file ID, and a page reads each row's name and folder from the index into a `ResultNames` table, which holds each folder once with its parent, so a reply's memory grows with the folders and not with the folder depth. A results page (`proto::binary`) sends the folders its items need that no earlier page of the reply sent, then each item as its folder and its name; the client keeps the reply's folders and joins each item's full path. On `C:\` with 2.15 million entries this is 73 MB on the pipe instead of 251 MB of full paths. A tree 200 folders deep, such as a copy that copied into itself, costs gozlerd about 100 bytes a row instead of 2 KB or more ([deep folder rows](research/deep-folder-rows.md)).
- **Item fields travel only when the request asks for them** (`Rows::want_size`, `want_mtime`, `want_created`, `want_attributes`), and each item's flags say which it carries. gozler asks for the date created only for a `-dc` column, because most indexes hold none.
- **A page's sums follow its items.** `Rows::want_sums` asks for the matches' files and folders and their sizes (`MatchSums`), which the daemon computes in the scan that counts `total`. They travel after the last item, flagged in the page flags.
- **Totals cross the pipe without rows.** `-get-result-count`, `-get-total-size` and `-get-folder-size` ask for `Returns::Totals`, and the daemon sums the sizes it already holds for each hit. Summing on the client would stream every match, and `-get-folder-size C:\` is the whole volume. The sum uses checked arithmetic; an overflow, which only a corrupt size in the index can cause, is `ProtoError::Overflow` and exit 1.

### Result snapshots

A result snapshot is the list of one search's matches, in result order, that gozlerd keeps after it answers, so a client reads any rows of it later without searching again. The name is always written in full, in the docs and in code (`ResultSnapshot`, `ResultSnapshotId`), because `VolumePhase::Snapshot` already names a volume without a journal.

- **Making one.** A search with `Returns::ResultSnapshot { rows, replaces }` searches for every match, in `rows`' sort. The reply is one `Response::ResultSnapshot { id, total }`, then the results pages of `rows`' offset and limit, as a read sends them. It costs what a search without a limit costs: the scan under each volume's read lock, the sort without it, and the rows under it again.
- **What it holds** (`query::ResultSnapshot`): each volume's rows, the entry and the file ID that a reply drops after its last page, 16 bytes a row on NTFS and 24 on ReFS, which never change after the search, so readers share them through an `Arc`. A search over several volumes adds the merged order: one byte for each row naming the volume of the next row, and every 4,096 rows each volume's position. The merge compares the values the indexes hold, so it is walked once, a page at a time under the read locks of the volumes the page reads, and a row that could not be sent then is dropped, because a dead slot has no value to compare. The total and the two coverage flags are the search's.
- **Reading rows.** `Request::ResultSnapshotRows { id, offset, limit, want_size, want_mtime, want_attributes }` answers results pages. A read starts at the checkpoint below its offset and replays at most 4,095 bytes without reading an index, so a read at a far offset costs only its own rows. Each row is checked under its volume's read lock as a page checks it (`row_folder`): a row whose file was deleted, moved out of the search's folder, has no full path, or is in a folder the reader may not list (see "Visibility") is gone, and keeps its place and the total, because a client holds rows by their place. A gone row is item flag bit 4 with no folder, no name and no other field (`ResultItem::is_gone`). A renamed or moved file keeps its place in the order until a new search. A volume whose index was replaced, or a volume removed, makes the whole result snapshot gone: `ProtoError::ResultSnapshotGone`, which a client answers with a new search. A read upgrades each volume it holds weakly and checks it is the request's own (`Arc::ptr_eq`), because a tail that the supervisor abandoned still owns its removed volume, and a volume attached again is a new one.
- **Sorting one again.** `Request::SortResultSnapshot { id, rows }` sorts the result snapshot `id` by `rows`' sort with no search (`ResultSnapshot::sort_again`), and is answered as a search that makes one is: a new result snapshot, which replaces `id`, then the pages of `rows`' offset and limit. Each volume's rows are keyed under one hold of its read lock, where the sorting user's answers are made too (see "Visibility"), sorted without it, and become rows under a second hold, as a search's matches do; only the scan is left out. The rows stay the search's: a file created since is missing, and a file deleted since is left out, because its slot has no value to sort by. The sums and the two coverage flags are the search's. **Owner decision:** a sort change may show the rows of the search's moment until `F5`, as scrolling does, so the window sends this when only the sort changed and the list reads a result snapshot, and keeps the sums the search counted. Before the list has one, a sort change searches again.
- **Lifetime** (`result_snapshots.rs`). One table for the daemon, behind a mutex that is never held while a volume lock is taken, because a connection serves one request. The id is 128 random bits (`BCryptGenRandom`), which only the client that made it learns, over its own pipe. Each user, by the client process's user SID, keeps at most 2; all of them hold at most twice the entries of every index in rows; one nobody read for 10 minutes is dropped whenever the table is used. Each rule drops the one read least recently, and the request that makes one drops the one it `replaces`. A dropped result snapshot costs only a new search.
- **Memory.** The empty search on the owner's `C:` (2,015,690 rows) takes 32 MB, beside an index of 194.6 MB; the merged order of several volumes adds about 6 % of the NTFS rows.

## The window

`gozler-ui` gives the search that `gozler` does a Windows desktop face: a person types, sees the matches at once, narrows them with a few clicks and opens the file. It adds no search feature. Every filter, column and sort is something `gozler` does, and a new search feature goes into `gozler-core` first. For the same query it shows the same count, the same rows in the same order, and exports the same bytes as `gozler`, because both use `gozler-core`'s parser, request rules and output code. When something fails, it says what to do next and, where it can, offers a button that does it. The window has a second mode, Commander mode, a file manager that browses folders itself and takes only the folder sizes and its searches from gozlerd (see "Commander mode"). This chapter describes Search, and the rules that both modes share.

**Owner decision: the priorities, in this order.** Each choice in this chapter serves one of them.

1. Low memory, because the window stays open all day beside other work. It holds no index and no result set (see "Memory budget").
2. Usability and correct Windows behavior.
3. Little code to write and maintain.
4. A polished look. It never justifies substantial code.
5. Dark mode. It is dropped when it costs more than it gives.

**Owner decisions.** These are settled:

- **No dark mode.** It needs undocumented uxtheme exports, and the menu bar stays white without undocumented `WM_UAHDRAWMENU` painting. It can come later as one module, `win::dark`, that falls back to light when an export is missing.
- **Native controls, not drawn ones.** Where Windows has a control for what Gozler would draw, Gozler uses it, because a drawn imitation never looks or behaves quite like the real control on every Windows version, theme and high contrast setting: toolbars for the main window's icon buttons, push buttons with an image list for the dialogs', list views for lists with icons or paths, static icons and etched lines, status bar parts and the list view's own insertion mark. A list view shortens a path itself (`TextMeter::compact_path`, which calls `PathCompactPathW`), so its drive and its name stay, and its info tip shows the path whole.
- **The list reaches every match** of a search, in the chosen order: the rows past the first chunk come from a result snapshot (see "Rows on demand" and "Result snapshots").
- **Only the window is translated.** `gozler` and `gozlerd` stay in English, because their users are scripts, agents and developers. The catalogues are JSON files in the style of TinyTorrent2's localisation contract, so the owner's projects share one catalogue style; Fluent and other translation crates are not used.
- **Start of words is the default match mode**, the rule of the owner's SearchHelper: `para 50` finds `Paracetamol 500mg.pdf`.
- **The window uses TinyTorrent2's words** where both programs mean the same thing, such as Settings, not Options. `CONTEXT.md` holds them.

### Where the unsafe lives

The window is native Win32, through Microsoft's `windows` crate. Two throwaway prototypes of the same window, with a virtual list of 3 million rows, measured it against Slint 1.18.1: 4.6 MB of private memory against 12 MB with Slint's software renderer and 120 MB with its OpenGL renderer, a 182 KB executable against 13.9 MB, and a first release build in under a minute against 9.5 minutes. Windows itself gives the list's selection, keyboard navigation, column resizing and reordering, sort arrows and accessibility, which in a framework are code that gozler would write. The window uses `windows`, not the workspace's `windows-sys`, because it needs COM interfaces such as the folder picker (`IFileOpenDialog`).

- **`core` holds every decision**: what to search, which rows to fetch, which replies are current, what the selection holds, which commands are enabled, where each control goes and what every caption says. It has no Win32 type, so its tests run on Ubuntu, as `gozler-core`'s do beside `gozler-winfs`. It starts with `#![cfg_attr(not(windows), allow(dead_code))]`, because on Ubuntu nothing outside its tests calls it and CI denies warnings.
- **`win` holds every `unsafe` block of the window**, each the smallest block that needs it, behind a safe function, and decides nothing. `core` and `app` start with `#![forbid(unsafe_code)]`, so the compiler rejects `unsafe` there. The test-only `timing` module, which times shell calls on synthetic paths, makes its own calls in `unsafe` blocks and is never built into the window.
- **`app` holds the message handlers**, which turn Windows messages into calls on `core`, and `core`'s answers into calls on `win`.
- **The dialogs are templates** in `resources/gozler-ui.rc`, so Windows runs their modal loop, Tab order, `Enter`, `Esc` and per-monitor DPI scaling. The main window, the menus and the accelerator table are built in code, because the layout follows the window's size and the menus follow the command table and the person's shortcuts.

Window code stays out of `gozler-winfs`, because that crate uses `windows-sys` only and depends on no other workspace crate, and the window needs COM and `core`'s types. A crate of its own for the window's Win32 layer would add a public API and `deny.toml` and CI entries, and protect nothing more, because the layer has one user; if a second program needs it, `src/win/` moves into a crate as it is. Values from `gozler-winfs` and `gozler-client` cross into the window as Rust types, never as raw handles.

The alternatives fail on one Win32 fact or another. A view diffed against the controls would keep a second copy of the selection and the scroll position, which the controls own. The window does run its effects through one applier (`Accepted`, which `apply` runs), and answers `LVN_GETDISPINFO`, which needs each cell's text before it returns, with a short read borrow of the model. Free functions over one global `RefCell` panic when Win32 calls back into the window (window rule 1). `winsafe` makes no stability promise before 1.0 and reads the system DPI, not the monitor's, and `native-windows-gui` has had no release since September 2022.

Controls remain the authority for what they show. Refreshes use cheap native
getters to skip unchanged text, fonts, images, checks, counts and geometry;
they do not keep a second caption or interaction model. Only properties Windows
cannot return, such as marquee mode and the administrator shield, have a small
flag beside their actual control owner. Glyphs still follow native image-list
readback, current DPI, colours and Show icons even when a shield flag is unchanged.

### Window rules

Each rule prevents a failure that is expensive to fix once every handler depends on the code it governs.

1. **No model borrow across a Win32 call.** Many calls send messages to the same window procedure before they return: `SetWindowTextW` on the search box sends `EN_CHANGE`, `SetWindowPos` sends `WM_SIZE` and `LVM_SETITEMSTATE` sends `LVN_ITEMCHANGED`. Calls with their own message loop, such as `TrackPopupMenuEx`, `DialogBoxParamW`, `IFileOpenDialog::Show` and `MessageBoxW`, dispatch every message of the thread, the daemon's replies included. A `RefCell` borrowed across such a call panics, and only when Windows happens to send that message. A Win32 call here is any call that can send or dispatch a message: one that takes a window handle, a COM or shell call, or a modal loop. So a handler changes the model in a statement of its own, through a `Model` method that returns what changed, and makes its Win32 calls after that borrow ends. The model's fields are private to `core`, so `app` changes a fact only through a method. In `app`, a borrow is bound with `let` only in a block that makes no Win32 call, such as the `LVN_GETDISPINFO` handler, which formats a row into the list's buffer. A dialog edits its own copy of the values, and OK and Apply give the copy to the main window by a method call.
2. **The window's state outlives each message.** File > Exit calls `DestroyWindow`, which delivers `WM_NCDESTROY` before it returns, so state freed there would leave the Exit handler running on freed memory. The window owns its state through an `Rc`, the window procedure holds its own clone for the length of each message, and `WM_NCDESTROY` drops only the window's reference.
3. **Each request has its own connection and cancel token.** Closing a connection from the UI thread races the thread that reads it, and the daemon closes a connection after 30 seconds with no request (`IDLE_TIMEOUT`), so a connection kept for later fails after a pause. Connecting is one pipe open, the owner check and one `Hello`, as each `gozler` run does. The client polls the pipe every 5 ms and checks the token on each poll and between its connect retries, so a cancel frees the window at once; sending needs no check, because a request of a few hundred bytes fits the pipe's 64 KiB buffer without the daemon reading it. The daemon notices a cancel at its next page.
4. **At most one request of each purpose is current.** The purposes are `Results` and `Selection` for each list, `Status`, `Export`, `Program`, `IncludeCount`, `SettingsSaved` and `FolderSizes` for each panel. Replacing a request cancels interest in that purpose alone, so selection never cancels results and one panel never cancels another's sizes. Different purposes use independent connections. A cancelled executing slot remains occupied until native cleanup finishes (rule 5). Model checks reply generations; collection passes also validate their input snapshots before publication. The folder work owner rejects events from a replaced reading, and Panel rejects a read it no longer awaits. Model owns which requests are current, including whether an export runs. Exit cancels request interest; hiding the window cancels none.
5. **No reply reaches a reused handle.** UI work delivers owned replies through the live-window endpoint in `win::work`; a native wake only tells the UI to drain them. The endpoint closes before native registrations and the HWND are destroyed, so a late worker cannot wake a reused handle. Purpose and generation identify the current result. Cancellation removes interest but retains an executing slot until the call and resource cleanup finish, because an uninterruptible native call must not admit unlimited replacements. Replies and cancelled large payloads are retired through the same size policy. No worker joins on the window thread. Normal Exit closes the UI immediately; essential persistence and accepted-operation cleanup can retain the invisible process. Session-end acknowledgment pumps for at most two seconds, checks explicit finalizer completion and preserves an earlier `WM_QUIT` for the outer loop. That bound does not promise that Windows will let an unfinished writer survive sign-out.
6. **Sizes stay device-independent until the last step.** Sizes are designed in `Dip` and converted to `Px` only in `core::layout` and in the font and icon code. The two are distinct types, so the compiler rejects a mix, and per-monitor DPI never means finding every number.
7. **One function places every control.** `Layout::compute` gives every control's rectangle from the client size, the DPI, the shown mode (Search's controls, or Commander mode's two panels and key bar), the panes shown, the inspector's height, the font height and the measured width of each caption that the window places in code. `WM_SIZE`, `WM_DPICHANGED` and a language switch only apply its result, and its margins and gaps are named constants (see "Look"). It also gives the window's minimum size, which leaves out the filter pane's height: when the window is shorter than the pane, the pane gets a scroll range.
8. **Each key has one owner.** An accelerator for `Ctrl+A` or `Ctrl+C` would take those keys from the search box, and `IsDialogMessageW` turns `Enter` and `Esc` into `IDOK` and `IDCANCEL` before any control sees them. So the message loop runs `TranslateAcceleratorW` first, and a shortcut reaches its command whatever control has the focus. Then it takes, from `WM_KEYDOWN`, the keys that `core::commands::control_key` gives the focused control: in the search box `Down` moves to the list, on the row the person left there, or on the first row when none is selected, `Enter` acts on the result (see "Searching"), or runs the search with search as you type off, and `Esc` clears the box; in the list `Enter` acts on the focused result (see "Searching") and `Esc` runs File > Cancel. Then it gives a `WM_SYSCHAR` to the visible toolbar whose button has that access key (`TB_MAPACCELERATOR`, see "Words"), because `IsDialogMessageW` matches access keys only against windows' texts. Then come `IsDialogMessageW` and `DispatchMessageW`. Copy and Select all act on the focused control, so with the search box focused they send it `WM_COPY` and `EM_SETSEL`. The Keyboard page refuses every key that a focused control uses, so no shortcut can take one. Commander mode's accelerator table is the exception: it leaves out a person's shortcut on `Alt` with a character, which quick search needs (see "Keys"). Shift+F10 and the Menu key reach the list as `WM_CONTEXTMENU` with no point, and the context menu opens at the focused row, or at the first selected row when Select all left no row focused. The message loop gives each message to the keys of its own top-level window (`GetAncestor(msg.hwnd, GA_ROOT)`): the main window's accelerators and control keys, a Viewer's own keys (`win::window::own_keys`), so `F3` or `F5` in a Viewer never acts on a panel. The main window has an accelerator table for each mode, built when the mode shows. In Commander mode, a panel's list, its path box and its search box also take the keys of "Keys", and the list takes the characters that start a quick search before it sees them. While a dialog or a message box runs its own loop, a `WH_MSGFILTER` hook (`win::window::MessageFilter`) gives its messages to the same routing as the outer loop (`route_other`), so the Viewers, which stay usable, keep `Tab`, their access keys, `Enter` and `Esc`.
9. **One row per command.** `core::commands` holds one row for each command of "Commands" and "Commands in Commander mode": its ID, its catalogue key, its default keys, its menu and group, and functions of the model that say whether it is enabled and checked. Each row also names the modes it belongs to, and the menu bar and the accelerator table of a mode hold only the commands that belong to it. The menus are built from it, `WM_INITMENUPOPUP` sets each item's state from it, and the match controls and buttons take their state from it, so no surface grows its own idea of a command. Every surface calls the same `MainWindow` method. A command chosen from a menu with its own loop runs after the menu returns, and the handler asks again whether it is enabled (`T11`).
10. **The selection is the files of one moment.** The daemon keeps no result set, so a position names whichever file is there now, and a selection read again later can name other files. So the selection is a set of files built at one moment, and every action reads it (see "The selection"). It is never built from rows of two moments, such as cached rows and rows fetched later, because an insert between them would list one file twice and miss another.
11. **A panic ends the process after it is reported.** A panic cannot unwind out of a window procedure (`extern "system"`), and the release profile unwinds. The panic hook writes the message to `ui-error.log` in the settings folder, shows a message box with no owner window from a new thread, waits for it, and aborts. The new thread keeps the UI thread from dispatching a message while its state is half changed, and the box has no owner because disabling an owner would wait for the UI thread, which waits for the box. Every request thread has the same hook, so a dead request never leaves a window that waits forever.
12. **The model holds facts, never display text.** A text that was built and stored earlier stays in the language it was built in. So the model and the controls' state hold facts, such as counts, phases, typed failures and parse errors, and every caption is a function of those facts and the current `Strings`. `core::captions::WindowCaptions::new` calls every such function, and `MainWindow::apply_text` sets its result: at creation, after a language switch and after a change to the Windows regional format. `layout::CaptionWidths` measures the same value, so the layout sizes what is shown, and a handler that updates one caption after a reply calls the same function. A catalogue key is written whole, as a literal, so a test finds every key the code uses in `en.json`. The list's cells are formatted when Windows asks for them, so a redraw updates them.
13. **A wake opens no dialog until everything it drained has applied.** A dialog or a message box runs its own message loop, which dispatches the next wakes, so a dialog opened in the middle of a drain lets newer replies and disk events apply before the older ones that the drain still holds. A panel could then show a file again after a newer event said it is gone. So a dialog that a reply, a panel's disk event or a file operation starts is queued as a `ModalStep` and runs from `Message::Deferred`, after the current message returns. The steps run one at a time, so no dialog opens over another. A step that acts checks first that it still applies: the setup question that the fix still waits for, the first result while the list still shows the search that `Enter` waited for, and a file operation's question or report as `core::operations` decides. A message box that only reports, such as a failed export, shows as it is. Exit clears the steps that wait, so none runs against the destroyed window.
14. **Move waits and large passes off the window thread.** Disk, network,
    service and process work, and passes above the inline size, use the work
    owner. Native control messages remain on their owning thread.
15. **UI work shares one policy owner.** `core::work` decides admission and
    lifetime, `win::work` executes, and `app::work` routes typed purposes and
    results. Queue retains engine workers, device scheduling and recovery.
16. **Model changes use one update path.** A handler applies its `Accepted`
    through the three phases in "Shared code". Native interaction belongs to
    its control owner; user dialogs keep their existing scoped lifetimes.
17. **Model state by its rules.** Independent facts use booleans; mutually
    exclusive states use enums carrying their data. Constrained transitions
    belong in one transition function. A trait earns its place where behavior
    actually varies. Change a representation that permits impossible states
    rather than spreading agreement checks among callers.
18. **Each fact has one owner.** Cross-area readers consult that owner.
    Core area types exist for private facts or rules; shared facts stay in Model.
19. **Skip unchanged native sets.** Use cheap native readback or an established
    change. Keep only unreadable control properties beside the control; a
    second caption or interaction model is not a setter optimization.
20. **Main-window timer IDs use `Timer`.** One mapping joins its variants to
    native IDs; handlers do not invent another numeric identity.
21. **Tests pin lasting caller-visible behavior.** Use representative cases
    and the boundaries that fail differently, with literal predicted outcomes.
    Pure transitions and invariants need no window. Resolve uncertain native
    behavior with a bounded check; use the After install list for environment
    and visual checks rather than adding a display-only regression suite.

Every text a person sees is a key in `en.json`: no English is written in the code or in the `.rc` file, which holds the templates with control IDs and empty captions. `resources/ids.h` is the one source of the resource and control IDs, the main window's included; the `.rc` file includes it, and `build.rs` turns it into `core::ids`. `build.rs` refuses two controls with one ID, a control with ID 1 or 2, which a dialog sends as `IDOK` and `IDCANCEL`, and a dialog that the script does not define, which `rc.exe` and rustc would both accept.

### Shared code

The window builds its own `SearchRequest` in `core::query`, not through `build_search`, because `build_search` reads the command line's `EsPlan`, parses one query string and asks for rows from offset 0, while the window combines several parses and asks for rows at any offset. Every rule that both need has one owner, which both call:

| Rule | Owner |
| --- | --- |
| Query syntax and its errors | `gozler_core::query::parse`, for the typed text and for each filter's text. The window parses with no working folder, so a relative path is the parser's `RelativePath` error, which asks for the full path. `QueryError::Malformed` carries a typed reason, so the window says in every language what is wrong; its `Display` writes `gozler`'s English. |
| The `-r` and `-parent` tests and the scope `-parent` sets | `gozler_core::escompat::request` |
| How a name term matches | `ParseOptions::term_match`, which the window sets from its match mode, as `-ww` sets whole words |
| The matched part of a name or path | `gozler-core`'s matching, which returns the ranges; the window asks only for the visible rows and draws them bold with `NM_CUSTOMDRAW` |
| `dm:` and `dc:` resolution | `gozler_core::query::dates`, with the local clock and time zone |
| The size words' ranges | one table in `gozler-core` that the parser and the filter pane both read |
| A folder's size | `gozlerd`'s index (see "Sizes and dates") |
| Help > Search syntax | `gozler_core::escompat::search_syntax`, from the tables that `gozler --help` reads |
| Whether a volume's results are complete | `VolumePhase::is_complete` |
| The owner check, the version handshake, the service's state, and the check that a scope folder exists | `gozler-client`. The UI work owner checks Location in the background when it changes and on F5. Searches use its last observation, so typing never reads the folder. `core` parses settings and builds changed-key intent; `app::work` reads the file before the window exists and writes it on its persistence worker. |
| Results pages, and CSV, TSV and TXT output with its date format | `gozler_core::proto` and `gozler_core::output` |

`gozler-client`'s error keeps every distinction that `gozler-cli`'s exit codes need. The window collapses them into the failures of "Daemon failures", and shows `Communication` as a lost connection, because both mean that the pipe failed after the service was found.

Each window feature owns its controls, native interaction state and event
handling. MainWindow coordinates routing, layout, focus, lifetime and commands
that cross features. Shared facts have one Model owner; a feature gets private
core state only for facts or rules it owns. Inspector retains semantic displayed
facts in core and its native icon beside the control; captions remain derived.
Panel's row owner hides storage and ordering behind row operations, while Panel
owns navigation and acceptance. Work's typed groups own task execution and input
coalescing, not accepted presentation facts: Model accepts Location observations
for its current path, and Settings retains labels for its current volume prefixes.

Model transitions return `Accepted { touched, effects }`. MainWindow first
applies structural effects, refreshes only touched areas, then delivers the
remaining ordered actions: requests, focus, timers and dialogs. A nested native
update supersedes older presentation; one Deferred wake repairs interrupted
areas and revalidates pending work against its existing intent identity.
Accepted file commands are consumed once. Repair replies carry the same
generation as their accepted work, so a cancelled repair cannot advance its
replacement before that replacement's worker starts. Viewer transfers saved
state explicitly; refresh does not copy it into Model. Settings keeps its draft,
Viewer its document, and synchronization its preview and explicit commit within
their existing scopes; transfers remain shared inspector content.

The UI work owner (`core::work`, `win::work`, `app::work`) admits one executing
task and one pending replacement per purpose. Cancellation retains the executing
slot until the native call actually returns. Location checks and the fresh Use
local binary check use this owner. Settings opens after its background registry
read; reads and sign-in writes are serialized. Apply acknowledges choices after
shortcut preflight, and a registry answer updates only its submitted field,
preserving newer edits. Include starts after that answer and any failure dialog.
An accepted registry write can finish after the UI closes. Settings reads volume
labels in the background by mount prefix, including before first show, and uses
cached answers on refresh. Its timer does not cancel an unchanged active label
read. About's repository link, Open Services and each list/action retain their
own shell STA and message pump between invocations; returning from URI activation or Properties
is not its lifetime boundary. Panel Enter follows the same route. Accepted open
commands stay with their invocation. Setup has a protected slot and unwinds
modal loops only after activation succeeds; it never waits for the installer.
These calls pass no main HWND to their worker. Queue retains execution and
recovery. Results carry task identities, and a deferred action rechecks its
identity before it runs. Persistence finishes its executing write and combines
pending changed keys, including removals and a value changed back to its prior
value. These paths close delivery before HWND destruction and never join their
workers on the window thread. The file-operation engine retains its own
workers, device scheduling, operation cancellation and recovery; the UI owner
serializes commands through that engine's interface and observes its answers.

Native dialog, Viewer and transfer-pane creation failures submit diagnostics to
Work instead of opening ui-error.log on UI. One worker serializes the append
requests; pending batches concatenate in constant time. Closing transfers their
completion off UI independently of essential persistence. These diagnostics are
best effort and never keep the window or essential writes waiting.

Work also owns the window watchdog and its dedicated UI-thread heartbeat HWND.
Its worker samples native top-level visibility every 100 ms, including system
dialogs and tray menus while the main window is hidden or minimized. With no
visible, non-minimized window it resets the pending interval. An unanswered
SendNotifyMessage heartbeat reports after 200 ms and then once per second.
Window and dialog trampolines retain a nested numeric message/control-ID context;
the worker copies it under a short lock before querying cycles or writing the
newest 64 KB to ui-slow.log beside ui.json. Each append locks the file across
processes and reads its current tail before trimming it, so separate windows
preserve each other's records. Logs contain no window text, paths or
queries; elapsed wall time and raw CPU cycles remain separate. Closing Work
stops the monitor and closes endpoint delivery before destroying its HWND,
without joining on UI. Sent-message acknowledgment is a diagnostic observation,
not proof that every posted input can run during a cross-thread native wait.

CPU passes use the same work owner with a callback that receives no blocking
capability. `core::work::pass` runs at most 1,000 source items inline and defers a
larger input; its answer follows the same task identity and delivery rules.
Superseded shell/clipboard payloads and replaced taken-path snapshots use this
boundary for disposal, coalescing ownership without walking their contents on
UI. Cached result chunks are immutable shared rows built by their producer;
selection replies partition their selected and cached rows before delivery.
Replacing or rejecting a large cache transfers its disposal to that owner,
including a search panel's nested caches when it changes views. Selected files and
positions also share backing. The common pass materializes cached selections,
filters gone/taken entries and calculates a search panel's selected bytes.
Commands remain unavailable during filtering; native highlights stay until an
accepted answer determines which rows to clear. Selection identity and exact
Taken/kept snapshots guard acceptance. Each list calculates its kept operations
from the fetched-row evidence captured at each completion, even if scrolling
evicts those rows meanwhile. It accepts the previous kept set before dispatching
the next batch, and Queue release waits for all pending lists. An operation
receipt leaves any required selection filtering pending; ordinary filter
acceptance dispatches its effects, including folder-size requests. Receipt
delivery, Queue release and native tile updates continue in bounded slices
through one coalesced native timer per owning window, so a chain of posted
continuations cannot outrank input and painting.
Native deselection also waits for that bookkeeping; conservative hidden rows
used for painting never clear highlights while their status is unresolved.
Inspector/status totals are calculated with the accepted selection and read
directly on refresh, with no second pass over its files.

Task timing follows the last answer or progress, not only its start. An awaited
read becomes visibly slow and offers Cancel where cancellation is safe;
background status, labels and hover observations expire silently and clear only
their matching interest. Expiry does not free an executing native slot or throw
away an accepted write. Search and folder cancellation preserve accepted rows;
a late reply cannot replace a newer navigation. A waiting Settings request and
its Cancel share the same current-page choice, and auxiliary work cannot replace
the status of a running repair or accepted operation.

The model owns whether this is the first window; it alone is eligible for file
operations. Command availability also reads whether its actual queue has been
adopted after recovery, so eligibility never admits commands before the engine
is ready. Search owns its own list, debounce and Enter-on-answer facts; the
panels' search lists do not inherit those Search-only rules. Timer IDs have one
typed mapping, and repeated column-drag notices post only one deferred read.

Search Copy captures shared selected rows. Its existing native task serializes
and allocates the clipboard data before publishing it, so both preparation and
publication leave UI. Busy retries keep that exact prepared block. The command
captures the clipboard sequence before encoding, so another program's newer
copy is protected even while preparation runs.

### Requests

The UI thread never waits for the pipe (window rules 3 to 5).

- **At start** the window asks for status beside the first search, because only a status reply names the volumes, which the Location list, the status bar's right group and the decision to poll need.
- **The status poll.** The window asks for status every 2 seconds while `Model::polls_status` says so: the service is not running, runs but does not answer yet, or the connection was lost; a volume in scope is bootstrapping or rescanning; Settings shows the Drives and service page; or the status bar's memory group is on screen (see "Status bar"), whatever the failure; a refused request and a failed owner check still keep their message, because a status reply never runs their search again. One timer serves every reason. An offline or failed volume is not polled, because waiting does not complete it. When a status reply shows that the daemon answers again, or that every volume in scope is complete, the search runs again (`T4`).
- **A results page** carries only the combined flags `volumes_incomplete` and `metadata_pending`. When a page sets a flag that the latest status does not explain, the model asks for status once.
- **A refused request and a failed owner check** start no poll and stay on screen until the person changes the search, because the same request would fail the same way. A daemon older than the window is polled, because the setup that replaces it ends the failure without a change to the search.
- **A failure means something different for each purpose.** A `Results` failure shows its message in the list. A `Status` failure updates whether the daemon answers and replaces a failure that the list shows for another reason, so the list never offers the fix for a state the service has left; while a search is in flight, its own reply shows instead, and a `Status` failure that leaves the rows shows in the status bar's right group (`T10`). A `Selection` failure is the service failing while rows are shown, so it shows as a `Results` failure does and clears the selection (`T3`); a selection left unread would keep its commands disabled with nothing to say why. An `Export` failure is the export's message box and leaves the list alone.
- **Exit** closes delivery and cancels request interest without joining on the UI thread (window rule 5). A stopped export's partial-file cleanup finishes off UI, independently of essential settings and recovery writes, so a slow export file cannot delay those writes.

### Rows on demand

A search asks for the first chunk of `CHUNK_ROWS` rows (16,384) in the chosen sort; at offset 0 with a limit, the daemon selects the top rows instead of sorting every match. The reply carries the total, and the list's item count is the total. When the list needs rows that are not cached (`LVN_ODCACHEHINT`, or a jump with the scroll bar or `End`), the window asks for their chunk, and those rows show empty cells until it arrives. A chunk past the first comes from the list's result snapshot (see "Result snapshots"): the first request for one makes it, naming the list's earlier one as the one it replaces, and from then on every chunk and every selection read reads it by its id, the first chunk too. A search makes none on its own, because search as you type searches 150 ms after each keystroke and a result snapshot sorts every match; most searches never scroll. When the result snapshot arrives, the cached chunks are dropped, because they are of an earlier moment than its rows (window rule 10), and the list draws its rows again. A row whose file is gone shows "No longer exists" in Name, with no icon and no other value, and the selection leaves it out. A sort change sorts the list's result snapshot again, with no search. When the service no longer keeps the result snapshot, the list searches again, as `F5` does. At most one chunk request is in flight; when it arrives, the window asks again for what the rows visible now still lack, so fast scrolling never queues a request for every chunk it passed. A request whose chunk has left the view, as after a jump with the scroll bar, is replaced by one for the rows in view, so the jump never waits for the rows it left. The request that makes the result snapshot stays in flight instead, because a replacement would sort every match again: a fast drag of the thumb would start one sort for each step, and the rows where it stopped would wait for all of them, since gozlerd finishes a search whose client has gone. When its first page names the result snapshot, the rows in view are read from it. A chunk's first page shows as soon as it arrives, and the whole chunk replaces it, so a `Selection` reply does not replace a first page's rows (see "The selection"). Every reply carries the total, and a changed total sets the count again. The cache keeps at most 32,768 rows in whole chunks and drops the chunk used least recently, so memory stays the same for any number of matches.

Every search scans every match before it sorts, about 0.3 seconds for the empty search over 2 million entries on the owner's machine. The scan dominates, so a large chunk costs the daemon little more and halves the waits on a long scroll. Each volume holds its index lock twice to answer a request, so the journal waits for neither the folder ranks nor the sort. The first hold scans the matches and keys them, and for a sort by path copies each match's folder. Without the lock, a sort by path ranks the copied folders, and the records are sorted, or, for a limited page, the first `offset + limit` are selected and only they are sorted. The second hold orders the runs of equal keys, and the first `offset + limit` records become the volume's rows: each row is the entry index and the file ID at the index's width. On NTFS a row takes 16 bytes, the size of a record, so the rows are made in place; on ReFS it takes 24, so the rows take a second list. The second hold also leaves out each match whose entry was removed since the first, or whose slot a new entry took over since, which the index's note of the latest 65,536 slots it reused tells. The note covers the changes the journal applies between the two holds, which last 0.6 s at most for 5.6 million matches, so it keeps a burst of deletes and creates, such as `npm install` over an old `node_modules`, of up to about 100,000 reuses a second, for 256 KB on a volume that reuses that many slots. Past the note, or when a rebuild or `optimize_storage` replaced the index's entries, the volume sends no rows, and the reply says its results are incomplete.

Each page then reads its rows from the indexes while the request holds the read locks of the volumes it reads: each volume with rows left, or, for volumes whose paths follow each other, only the one whose turn it is, so a page never waits behind a writer of a volume it does not read. It takes them in `VolumeSet` order and releases them before it sends the page, so the pipe and a slow client never hold a lock. `query::ResultRows` in `gozler-core` runs every step of the search: the holds above, the row checks, the folder copies and the merge. It takes each volume's lock through `query::IndexedVolume`, in the order the volumes are given; the daemon gives them in `VolumeSet` order, decodes the request and sends the pages. It checks each row as "Index coherence" says, merges the volumes as "Result order" says, and skips the first `offset` rows. A skipped row counts against the page's 4,096 rows like a sent one, so a far offset is skipped a page at a time, and such a page goes out with no rows. The page builder copies each folder a page needs once for the reply, keyed by its slot and checked by its file ID on every use, because a folder whose slot another folder took over must never give a row its path. A folder keeps the name and the parent of the first page that sent it, so the later rows of a folder moved during the reply print its old path. A folder found to have no full path is looked up again on the next page, because the journal can give it one, as when it names the parent of a folder under lost+found.

A chunk comes back short when rows are left out. On one volume, that takes a change, such as a delete, in the moment between the chunk's sort and its pages. Over several volumes, the chunk also ends where a volume runs out of rows while its limit cut off later matches, because that volume's next match is unknown, and sending another volume's later row first would skip it and show that row again in the next chunk. The window caches a chunk by its start, so a short chunk shows its missing rows empty until the next search.

For 5.6 million entries in a tree that copied into itself 190 folders deep, a full export by name holds the lock for 0.13 s, then 0.06 s, then at most 0.04 s for each page, and peaks at 183 MB above the index; by path, 0.41 s, then 0.04 s, then at most 0.04 s a page, also 183 MB. The first page by path holds it for 0.50 s and then 0.03 s, and peaks at 179 MB, most of it the records reserved for every match and the copied folders of the path sort. `End` in a list of 3 million entries makes the result snapshot, which sorts about 3 million records on each volume once; every later chunk then costs only its own rows.

### Main window

| Area | Control |
| --- | --- |
| Top | the menu bar, then a strip with the search box, a toolbar with its Clear button and the Match case and Match path toggles, and the Match: list |
| Left | the filter pane |
| Center | the results list: a list view in report and virtual mode |
| Bottom | the inspector pane, under a splitter that the person drags |
| Bottom edge | the status bar, with three groups, and the memory group while Settings > General turns it on |

The window is per-monitor DPI aware (manifest `PerMonitorV2`). It opens at its saved placement, and a placement that no monitor shows any more opens on the nearest monitor. Its minimum size is the smallest at which every control fits, the filter pane aside, which scrolls. Its title is the typed search and the program's name, "report – Gozler", as Windows titles a document window, or "Gozler" alone while the search is empty.

### Look

The window is made of plain Windows controls, with only the tweaks that cost one call or a few lines, so Windows keeps each control's keyboard behavior, focus, accessibility and high contrast.

- The list keeps Windows' default list style, and its selected rows are light blue as in File Explorer, without a border (see "Selecting entries"). The "Explorer" visual style is not set, because it draws a border around each selected row, which doubles between adjacent rows (**Owner decision**). `LVS_EX_DOUBLEBUFFER`, `LVS_EX_FULLROWSELECT` and `LVS_EX_HEADERDRAGDROP` give no flicker, whole-row selection and column reorder.
- The font is the message font from `SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS)`, with one larger semibold font for the inspector's heading.
- `WM_CTLCOLORSTATIC` returns the window color, so labels and check boxes have no gray dialog background. Every color comes from `GetSysColor`, so high contrast works.
- One-pixel dividers separate the panes instead of 3D edges. Each is an empty static control that `WM_CTLCOLORSTATIC` fills with the 3D light color, so the window paints no divider itself. The search row's toolbar parts it into three groups with its own separators: the search box with Clear, Match case and Match path, and Match: with its list.
- **Icon buttons are toolbars** (`win::toolbar`): the search row's Clear, Match case and Match path, Show filters, the filter pane's Clear and Hide filters, the inspector's copy icons and Hide details, and the status bar's Show details. Windows draws their hover, pressed, checked and disabled looks and their separators in every theme, and a flat toolbar lets its parent's background show through. A toolbar button is not a window, so it takes no focus and is never a Tab stop, each button's text is its accessible name, and its tooltip comes from `TBN_GETINFOTIP`. `Layout::compute` takes each toolbar's width from `TB_GETIDEALSIZE`, which the window measures after it sets the toolbar's font, texts and images.
- **Icons come from Windows' own icon font:** Segoe Fluent Icons on Windows 11, or Segoe MDL2 Assets on Windows 10, which lacks it, so Gozler ships no images and every icon follows the text colour, the DPI and high contrast. Code names each icon (`core::glyph::Glyph`), by Microsoft's name for it in `docs/segoe-icons.tsv`, such as `Glyph::Copy` or `Glyph::Settings`, and its code point appears only there; only icons that Windows 10's font also has are used; a test checks that both fonts draw every one, and an icon the font on the machine lacks draws nothing, never a box. Which icon a command, a Kind, a Settings page or group has is a fact beside its caption in `core`. `win::gdi` draws them all, 16 px at 100 %, in the system text colour, gray when disabled: before the label of the common commands in the menus and the row's context menu (Open, Open file location, Explore, Properties, Export results…, Settings…, Exit, the copy commands, Select all, Find, Search in this folder, Refresh, Search syntax, About, Show Search and Show Commander), in the column of the check marks, because no item has both; before the caption of every push button, in the main window and in the dialogs, except each Clear, which then shows only its icon, by the catalogue key of its caption (`core::captions::button_glyph`), such as a check mark on OK and an arrow turning back on Reset, through the button's own image list, which makes the shared button widths an icon wider, and a button that asks for administrator permission shows Windows' shield there instead; before the caption of each key bar button, the icon of the command it runs, the same as in the menus, so one action has one icon; on each toolbar button that has one, through the toolbar's normal and disabled image lists; on transfer pane actions, icon and text when Show icons is on and text alone when off, with 32 px operation icons spanning two lines; before each item of the Kind list and the Settings page list, through each list view's image list; before each Settings section's title, in a static icon (`SS_ICON`); before the files and the folders of the status bar's totals; and in the memory group, a processor chip (`Component`) before the service's figure and a window (`Favicon`) before the app's, the nearest that both fonts have to TinyTorrent2's CPU and window icons. Each icon is drawn once at four times its size and averaged down, because GDI's own smoothing draws diagonal strokes much lighter than upright ones, and the image is kept, so a repaint only copies it. Field labels, check boxes and option buttons have none, because an icon there adds nothing the word does not say. Settings > General's Show icons turns every icon beside a text off (see "The General page"); `win::gdi` alone decides from it whether an icon draws, and the layout reads it for the button widths.
- **Spacing** follows Microsoft's Windows desktop layout guidelines: 11 px margins at the window's edges, 7 px between related controls and 11 px between groups, at 96 DPI. They are named constants in `core::layout`.
- **Nothing moves by itself.** Only the window's size, the DPI, the panes shown, the splitter and the language move a control; a count, a path, a message or a state never does. The status bar's left and middle groups have fixed widths, each sized for its longest message with the largest numbers in the current language and DPI, and a longer value ends in an ellipsis. The middle group fits the longest of a running job's texts beside its progress bar and Cancel, and of a job's results; Windows' words for a fix that Windows could not run have no longest form, so a long one ends in an ellipsis. The memory group has a fixed width too, and each of its figures a fixed place, sized for every form of a size in MB and GB, so a figure that changes moves neither the next figure nor the right group.
- **Painting.** Windows draws every control of the main window; layout and refresh use native clipping, and one owner, `win::present`, decides when the pixels appear. Each message to a window procedure of the window's thread is an update, which includes the messages that its handler sends (`core::present`). A change only marks the windows it changes, and when the outermost update ends, the presenter paints the marked windows of the main window with one `RedrawWindow(RDW_UPDATENOW | RDW_ALLCHILDREN)` when two or more are marked, and leaves a single one to its next `WM_PAINT`. A modal loop ends the update that is open when it starts, each message inside the loop is an update of its own, and what the handler changes after the loop is a new one. An update that posts `DEFERRED` to finish an interrupted update ends with that message. A native setter that paints its control inside the message runs inside `win::present::mark_after`, which turns drawing off on that one control and marks it, and the test `win::painting` keeps such setters in their `win` wrappers. Each update that marked a window is logged to `ui-frames.log` beside `ui-slow.log`, by the watchdog's thread. The layout owner combines geometry and visibility for direct children in one `DeferWindowPos` batch (`MainWindow::layout`, `win::place_children`), skipping unchanged rectangles and visibility while retaining visibility-only changes; a shrinking list fits its columns to the planned smaller width before the move, while a growing list moves before its columns grow. Changed children repaint their new rectangles instead of copying old client pixels (`SWP_NOCOPYBITS`), including when the batch falls back to individual moves. Feature owners prepare the list messages, panel rows and inspector fields for that same batch; their standalone refreshes reuse those rules. Children of another window, such as the filter and transfer panes, use their own parent's batch. The transfer pane applies an available current model view before reveal, shows Preparing while its first view is pending, and lays out its children through the existing size notification, without holding a model or view borrow across native callbacks. Every control the windows create in code has `WS_CLIPSIBLINGS` (`win::controls::child`, `win::pane::create`, `win::window::create_child_window`), because without it a control moved first draws into a sibling's old area, and the sibling's own move copies those pixels as its own: a clipped copy of a neighbour's text that stays until something repaints the control. Where such controls overlap on purpose, the one created first lies above, as the list's message lies above the list. The status bar erases and draws a part for every `SB_SETTEXT`, also an unchanged one, and the whole bar for every `WM_SIZE`, so a part's text, icon and edges are set only when they change (`win::controls::StatusBar`), and the main window sends the bar `WM_SIZE` only when its client size changed or after measuring. The main window has `WS_CLIPCHILDREN`; the batch places with `SWP_NOREDRAW` and marks each moved, resized or shown child with its frame and its children, and the place that a child leaves in its parent, without a whole-window invalidation after layout. Besides `mark_after`, `WM_SETREDRAW` is turned off only around a change that would draw a control several times (`Hwnd::draw_once_after`), such as the column fills of Search's list and of each panel's list, followed by one `RedrawWindow`; a list takes each column width by drawing every row it shows, twice when its horizontal scroll bar comes and goes, so a panel's list draws once after all its new widths, and not at all when none changed. Column changes use the list's native buffer without background erasure, retaining frame and child redraw (`RDW_FRAME`, `RDW_ALLCHILDREN`); sibling positioning stays outside that scope. The dialogs' redraw batches retain `RDW_ERASE`. The transfer pane shows its idle caption when neither a card nor a queue occupies the content. A hidden window is not touched, because `WM_SETREDRAW` TRUE would make it visible. A menu bar whose titles stay the same at the same DPI, such as on a switch between Search and Commander mode, gets only new menus and is not drawn again (`win::menu::replace_menu_bar`). `WS_EX_COMPOSITED` is not used. Gozler paints only what the test `win::painting` allows: the Viewer's text area, Search's cells with matches in bold, the fill and focus outline of the lists' rows, and the presenter's cover.
- **Text fits.** A control that the window places in code takes its width from its measured caption, so a longer translation widens it instead of clipping it. Dialog templates leave room for twice the English length on captions of up to 12 characters and 50 % more on longer ones.
- **Push buttons share one width,** in each dialog, and in each group of the main window's: the inspector's Explore, Open location, Open and Copy path; the list message's buttons; and the status bar's Cancel alone. The width is 50 DLU in a dialog and 75 px at 96 DPI in the window, as Windows sizes OK and Cancel, or the widest caption among them in the current language when one does not fit. A group in the window shares its width only within itself, because one width for the whole window let the longest caption, such as Search everywhere, widen every button. Choose folder… spans the filter pane instead. Every Clear shares no width. The search row's and the filter pane's are toolbar buttons, which show their icon while icons show and the word Clear while they are hidden. Settings' search and the Keyboard page's are push buttons (`layout::clear_width`): while icons show, a square button with only its icon, as tall as the box beside it, and while they are hidden, as wide as its caption, Clear. Each Clear's caption stays its name for screen readers and its access key, and its tooltip says "Clear", or "Clear all filters." in the filter pane. The widest counts the captions a button shows later, such as every fix's. A dialog's buttons are sized when it opens, its pages' included (`layout::sized_buttons`), and a status bar's Cancel takes the same width, and the window's by its layout (`layout::ButtonWidths`). A row of buttons keeps its right edge and its gaps, and the control to its left, such as an edit box, shrinks to keep its gap; a row at the left margin, such as the Include page's Modify…, keeps its left edge. Short captions keep the width small: a button's caption is one word, or two (`AGENTS.md`), and a tooltip says what it does when the caption cannot, as on the inspector's Open location and Explore and each Settings page's Reset.

Acquired-frame painting acceptance remains open in
[Native window painting](plans/window-painting.md), separately from the
implemented ownership and work contracts. The current native mechanisms do
not certify that every intermediate frame is complete.

### Words

- Labels use sentence-style capitalization, as current Windows does: "Open file location", "Copy path". Where File Explorer has a word for the same act, the window uses it.
- A label is a verb phrase for a command and a noun phrase for a setting, and it names what happens, not how: "Copy name", not "Copy file name to clipboard". A command that opens a dialog ends in "…".
- Every menu item, dialog label and button has an access key, except OK and Cancel, which `Enter` and `Esc` press, as in Windows' own dialogs, and the status bar's Cancel (see "Status bar"), and the icon buttons, the inspector's copy icons, Hide details and Show details and the filter pane's hide and show icons, which show an icon, not a caption; the Edit menu's Copy name and Copy folder have keys. `IsDialogMessageW` matches an access key only against a window's text, and a toolbar button is not a window, so the message loop first asks each visible toolbar of the main window for the key (`TB_MAPACCELERATOR`) and presses the button that has it, such as Match &case. Each catalogue chooses its own letters, unique within each menu and each dialog. The main window's letters also differ from the menu bar's, so an access key never opens a menu by accident. The six attribute combo boxes share one letter, on the Attributes label, which focuses the first.
- A message says what happened in one sentence, then what to do (see "Feedback"). It never blames the person and never shows a code alone.

### Search box

While the filter pane is hidden, Show filters stands at the row's left margin and the search box starts a gap after it (see "Filter pane"); while the pane shows, the search box starts at the margin. The search box shows the cue banner "Find files and folders" while it is focused and empty (`EM_SETCUEBANNER` with `wParam` TRUE), because the window opens with the box focused. Clear, the first button of the toolbar right of the box, empties it as `Esc` does and is disabled while the box is empty; it has no access key, because `Esc` is its key. Beside it are the match controls, in this order, so the drop-down list ends at the row's right margin:

| Control | Choices | Command line |
| --- | --- | --- |
| Match &case (toolbar toggle) | off by default | `-case` |
| Match &path (toolbar toggle) | on in a new install | `-p` |
| M&atch: (drop-down list) | Start of words (default), Start of name, Anywhere, Whole words, Regular expression | `wordstart:`, `startwith:`, a plain term, `-ww`, `-r` |

- **The match mode is one drop-down list,** because its modes exclude each other: a regular expression reads the whole text as one pattern, so no other mode applies to it, and separate check boxes would allow combinations that mean nothing.
- **Match case and Match path are toggle buttons with their words** (`BTNS_CHECK`), because each is one independent choice, and the toolbar shows a checked one pressed in, as File Explorer's command bar shows a toggle.
- **The modes apply to name terms.** A path term, a wildcard and a search function keep their own meaning, so `ext:pdf para` finds PDF files with a word that starts with `para` (see "Word start and name start"). Start of words differs from SearchHelper in two ways that keep gozler's own rules: accents count, because ignoring them would need a second folded copy of every name in the daemon's index, and a typed term is matched whole, so `report.txt` finds `my report.txt`.

### Searching

- **Owner ruling: where Open goes.** Search behaves as Everything and Commander mode as Total Commander, so each mode has its own rule:
  - **Search.** Enter and a double-click act on the focused result alone, as in Total Commander, and Open on the selected results (see "Selecting entries"). Each runs the command set in Settings > Context menu, Open folders for a folder and Open files for a file, so a person who prefers Total Commander to Commander mode goes there. Without one, the result shows in Commander, as Show in Commander does: a folder opens there, and a file shows in its containing folder with the file focused. Explorer opening from Gozler's own search is the failure this rule prevents. With several results selected, each result with a command runs it, and Commander shows the first result without one, because Commander shows one place. `core::open::Commands::open_route` decides this.
  - **Commander mode.** Enter and a double-click act on the focused row alone, as in Total Commander: `..` shows the parent folder, a folder, also a search panel's, shows in the panel, and a file opens as Windows opens it, so an `.exe` runs, in its own folder as File Explorer starts it. `core::panel::Panels::enter` decides this. The row menu's Open, its bold default, does what a double-click does: one folder shows in the panel, and a file or several entries open as Windows opens them. Settings > Context menu's commands never run in Commander mode (`core::open::Commands::for_list`), because a person in Commander mode has chosen it as their file manager.

  Settings > Search has no separate Enter choice. Show in Commander stays a command of its own, so a person with an Open folders command can still go to a result in Commander.

  Show in Commander, also as Open, changes the window only after the folder opens, as File Explorer and Total Commander stay where they are when a folder refuses them. The shell worker first opens the folder for listing, as a panel does; when the result is gone the window reports it as for Open, and when Windows refuses the folder, such as `C:\Windows\Prefetch` for a user who is not an administrator, the window stays where it was and says "Windows could not open" the folder, with Windows' reason. A slow folder shows in the status bar as any slow open does, with Cancel. From Search, the active panel then reads the folder while Search still shows, and Commander mode shows when the listing arrives, so the panel appears with that folder instead of the one it showed before; the panel reads it once.
- Search as you type starts a search 150 ms after the last keystroke. `Enter` in the search box then acts on the selected row, or the first row when none is selected, as `Enter` in the list does, so a person types a name and presses `Enter` to reach it. When the search for the current text has not answered yet, because `Enter` came within the 150 ms or while the search runs, the search runs at once and its first row takes `Enter` when it answers; a search that finds nothing does nothing, and typing again before the answer cancels it (`Model::enter_in_search_box`). With search as you type off, `Enter` runs the search.
- While a search runs, the list keeps the previous results, and the status bar keeps their counts and totals until the new ones arrive. Only a search that has run 300 ms without an answer makes the status bar say "Searching…" and drop the totals, so a search that answers at once, as most do while the person types, changes the status bar once instead of blinking on every keystroke (**Owner ruling**). The search itself starts at once. The new results replace the previous ones at once, scrolled to the first row (`T1`, `T2`).
- When the Location folder does not exist, the window does not search, as `gozler` refuses the same search (`S8`).
- `F5` runs the search again. Results do not update by themselves when files change, because the protocol has no change notification.

### Results list

| Column | Default | Content |
| --- | --- | --- |
| Name | shown; cannot be hidden | the name, with its extension's icon |
| Path | shown | the folder of the full path |
| Size | shown | the size in one unit for every row, so sizes line up for comparison: KB, rounded up, as File Explorer shows it, unless Settings > General's Show sizes in chooses MB or Bytes (see "The General page"); for a folder, its folder size, and an empty cell when that is unknown; "Unknown" for a file whose size is unknown |
| Date modified | shown | the local timestamp in Settings > General's Short date format |
| Date created | hidden | as Date modified; empty while Dates created is off (see "Include settings") |
| Extension | hidden | the extension; empty for a folder |
| Attributes | hidden; not sortable | the attribute letters, as `gozler` prints them |

- **The matched part of a name or path is bold,** as in Everything, so the person sees why a row matched: `para 50` makes "Para" and "50" bold in `Paracetamol 500mg.pdf`. `gozler-core` gives the ranges, so the bold part is always what the daemon matched. A regular expression shows its match; filter clauses show none.
- **A folder's Size cell is empty whenever its size is unknown,** as File Explorer leaves every folder's Size cell empty: while a size pass runs, and always for a user who does not see every file, who gets no folder sizes (see "Visibility"). The volume's state already says when a pass runs. "Unknown" in every folder row of such a user would read as "still loading". A file's unknown size still reads "Unknown".
- A cell whose text is cut off shows the whole text in a tooltip.
- **A short header never widens a column for its full name.** Date modified, Date created and Extension have the optional short headings Modified, Created and Ext in Search and Commander mode. Fits measure the short heading; the native header shows the full name whenever it fits the allocated width, and switches back as the width changes. Column menus keep the full names. Headers and cells supply `core::text::Text`, a full text and an optional short form, and `ListView::fitting_text` chooses from the list's measured text width and cell padding. Date cells fit their full timestamps; only a header uses its short form when measuring a fit.
- A header click shows the rows in that order, and a second click reverses it. While the list reads a result snapshot, gozlerd sorts it again with no search (see "Result snapshots"); otherwise the search runs again in that order. The first click on Size, Date modified or Date created sorts largest or newest first, as gozler's sort keys do. The header shows the arrow, and View > Sort by shows the same choice and gives the keyboard a way to sort.
- View > Columns and a right-click on the header show and hide columns. Column order and visibility are saved; widths are not, because the columns fill the list's width, as TableView's do (`lib/TableView/docs/tableview-contract.md`, section 10), and a file from an earlier version keeps its columns and ignores their widths.
- **The columns fill the list's width** when the window shows and whenever the list's width changes: a resize, the filter pane shown or hidden, or the window moved to a monitor with another DPI. A fill measures each column on the rows the list shows and one row above and below them, never on rows off screen: its widest cell and the average of its cells, each with the list's padding and, in Name, the icon, and neither narrower than its header; a column that none of those rows shows anything in has its header's width for both. When every widest cell fits, each column gets its widest cell and all grow by one factor to the list's right edge. When they do not, the columns share the width by their averages, scaled by one factor, each held between its narrowest and its widest width (`column_sizing::fill_widths`). Sharing by the average keeps one long path from taking the room of the other columns, which a share by the widest cell gives it, cutting Size and Date modified instead. A column's narrowest width is 40 px at 100 %, and Size's is its widest cell, because a cut size is a wrong size (`ColumnRow::whole`). While the person drags the window's border, the first fill measures and the next ones scale the same fits, so the widths follow the pointer, and when the drag ends (`WM_EXITSIZEMOVE`) the columns are fitted again at the final width; a size change outside a drag, such as maximize, restore or a snap, fits at once. A DPI change discards the drag's fits, because they are in the old DPI's pixels. A fill sets only the widths that change, and the list draws once after it, not between the resize and the fill. Minimizing and restoring do not fill, so they keep the widths the person dragged. Before any row arrives, each column's first width stands for its fit, because fitting the headers alone would give Size as much room as Name; the window shows before its first rows, so the first rows that appear on screen fill the columns once more. Results after those, sorting, scrolling and showing or hiding a column do not fill, so the columns do not move under the person's eyes; a column shown again gets the width it had when it was hidden, which the window keeps while it runs, as TableView does. The fill leaves room for the vertical scroll bar whether it shows or not, so rows that bring the bar never bring a horizontal one; a horizontal scroll bar shows only when the person drags a column wider than the list, or the columns at their narrowest do not fit. `core::layout::column_sizing::ColumnSizing` owns when a fill happens and what each of these events resets, and the window only measures the texts and sets the widths it answers.
- **A double-click on a header divider fits that column,** and the header's right-click menu starts with Fit column "Name" for the column under the pointer and Fit columns for every shown column, then the columns to show and hide. A fit changes only the column it names. A width the person drags or fits holds until the next fill.
- **Icons** come from `SHGetFileInfoW` with `SHGFI_USEFILEATTRIBUTES` and `SHGFI_SYSICONINDEX`, once for each extension, so Windows gives the extension's icon without reading the file. A per-file icon, such as an `.exe`'s own, would read every file from disk.
- A right-click on a row that is not selected selects it first, as in File Explorer, and a right-click below the last row shows no menu. Typing in the list does nothing, because a virtual list must find the row itself (`LVN_ODFINDITEM`) and most rows are not in the window; `Ctrl+F` goes to the search box.

### The selection

The selection is a set of files, fixed when the person makes it. Every action on it acts on those files, however long the person waits and whatever changes in the index meanwhile, and no action refuses (window rule 10).

- **The list owns which positions are selected.** The window never reimplements comctl32's anchors, Ctrl+Shift or marquee; the cursor and selection keys are Total Commander's (see "Selecting entries"). Native notifications are coalesced into one deferred capture; their ranges are confirmed against the control's count and selected rows before use. If that evidence is insufficient, a native cursor reads at most the work owner's inline size of selected rows per UI turn. One native-selection timer continues reads and writes after input and painting, rather than a chain of posted messages that outranks both. Whole-range bookkeeping runs through the work owner. A shorter list drops positions past its count without a notification, so capture leaves them out. Earlier synchronous reads of 100,000 rows held the window for about half a second on Ctrl+click. Every command surface uses the same capture path before deciding availability; a command that must wait retains its originating list and temporarily disables that list until capture and filtering finish. At most 100,000 rows are read (`SELECTION_LIMIT`): one `LVM_GETSELECTEDCOUNT` checks the limit before reading positions. Past it, the selection holds no files and the status bar says "3,000,000 selected · Select up to 100,000 to use a command" (`S4`).
- **A remapped selection retains its unchanged intersection.** A bounded cursor writes only changed rows, counting range traversal against the same turn budget. Whole-list changes use native bulk messages; clearing also wins when its cost is below the smallest possible difference. An interrupted native update has no reliable prior selection and resets before writing its replacement. These writes and their eventual disposal retain shared inputs through the work owner.
- **Every selected row cached:** the selection is those rows, the files the person sees.
- **Filtering is selection state.** Model's `Selection::Filtering` retains the input snapshot while the common work owner checks taken entries and kept operations and computes the sums. File commands wait for an accepted answer; stale answers cannot publish files or totals for another input. Taken rows lose their native highlight only after the matching filtering result is accepted, so highlights and command availability follow the same selection. A whole selection can be cleared with one native message; scattered changes use bounded turns.
- **Otherwise** the selection is being read: 150 ms after the last change, one `Selection` request asks for the rows from the first selected position to the last. Until it arrives, the status bar says "Reading the selection…", the commands that act on the selection are disabled, and their keys do nothing (`S12`, `T6`). The request thread keeps the selected rows and the rows of the cached chunks inside the span, and drops every other row, so a Ctrl+click on row 0 and on row 99,999 carries two files and at most the cache. Its chunks replace the cached ones, so the highlighted rows show the files that the selection holds, from the same moment.
- **A selected row the person never saw** is the file at that position in the list's result snapshot, of the moment the result snapshot was made; before the list has one, a selection that reaches past the first chunk makes it.
- **The files** are kept in selection order, each with its full path, whether it is a folder, its size and its date, and at most once. Open, Open file location, Properties, Search in this folder, the three copies and the inspector's sums read them, and no action fetches. Open, Open file location, Explore and Properties first check through the work owner which of Search's files still exist, because the index can be behind the disk and a share that stopped answering would hold the window, then act on those and say that the first missing one no longer exists. Each List/action has a bounded slot: unrelated purposes stay independent, while only the newest replacement waits for an executing check of the same purpose. A panel's entries come from a listing, and Windows reports a missing one itself, so Commander mode checks none.
- **A new search, sort or refresh** clears the selection, in the same step that accepts the new rows.
- **A row whose entry a delete or a move took** shows struck through, in grey, with the text and icon it had and no bold part, from the moment the operation takes it, and the selection leaves it out; the list then unhighlights the taken rows, every row with one message when the selection lost every file, so a delete of 100,000 selected rows reads and changes no row one by one. It stays until the next search, because gozlerd numbers its results again when it applies the delete, so a list that removed the row itself would map later positions to the wrong files (see "The person never waits for the disk").

### Copy

Copy, Copy name, Copy folder and Copy path read the selection's files.
`ClipboardData` in `core` knows each format's exact size before it writes a
byte: for `CF_HDROP`, the `DROPFILES` header with `fWide` set and the double-NUL
path list, and for the other three, the text. `win::clipboard` allocates one
`GMEM_MOVEABLE` block of that size, which `SetClipboardData` requires, and `core`
writes into it once, so a copy holds no second buffer. After a successful
`SetClipboardData` Windows owns the block; after a failure its owner frees it.
One bounded task publishes on its own native message-only window. Both opening
and clearing the previous owner happen off UI. A busy clipboard keeps the same
prepared allocation for up to five attempts, with a 20 ms timer between them.
Supersession and the sequence are checked while the clipboard is held, before
clearing it; an unrelated newer copy wins. Once clearing begins, publication
finishes its formats and closes even if cancellation arrives. File copies keep
their preferred Copy or Move effect. The formats are eager and survive the
worker's owner window; no delayed rendering is installed. Native failures and
busy exhaustion return through a current-identity checked error on UI.

- **Copy puts files on the clipboard as File Explorer's Copy does,** `CF_HDROP` with `Preferred DropEffect` set to copy, and Cut as File Explorer's Cut does, with it set to move. Cut rows are not dimmed, because any program can change the clipboard at any time.
- **Paste moves files with Gozler's engine.** On one selected folder row, Paste moves the files that were cut, in Gozler or in File Explorer, and copies the files that were copied, with the engine and its saved policy (see "File operations"). The window listens for clipboard changes (`AddClipboardFormatListener`), so Paste is enabled while the clipboard holds files. Paste reads the clipboard on a thread of its own, which keeps the data until the paste is submitted or dropped, because the program that holds the clipboard answers each call as slowly as it does.
- **After a move is submitted,** and only if the clipboard still holds the data that Paste read (`GetClipboardSequenceNumber`), Paste sets `Paste Succeeded` to move on that data and empties the clipboard, as File Explorer does after its own paste. Checking the sequence number keeps anything the person copied since. Paste never sets `Performed DropEffect` to move, because that value tells the program that cut the files to delete them itself ("Handling Shell Data Transfer Scenarios", delete-on-paste). Gozler's files pasted in File Explorer move too, but with File Explorer's engine.

### Dragging

Dragging selected rows out of Search's list or a Commander panel behaves as a drag from File Explorer: File Explorer, Total Commander, an editor, an email or the Recycle Bin take the drop, the drag shows Explorer's drag image, Ctrl, Shift and Alt choose copy, move or link at the target, and a drag with the right button gives the target's menu.

- **Start:** `LVN_BEGINDRAG` or `LVN_BEGINRDRAG` reads the selection, as a command does, and drags the entries that the list's file commands act on: Search's selected files; a panel's selected entries, else its focused entry, never `..`. A selection that is still being read drags nothing (see "The selection").
- **Data:** `SHParseDisplayName` names each path through a bind context that carries the row's find data under `STR_FILE_SYS_BIND_DATA`: the folder attribute, the row's attributes, its size and its date modified where the row has them; a panel's row has them all, as its folder listing read them. So the shell does not read the entry from the disk: 1,000 files took 685–1,130 ms without the find data and 9.6 ms with it, measured in a release build on the owner's laptop. `SHCreateDataObject` makes the shell's own data object, with the common folder and each entry's last ID when all share one folder, or each entry's ID from the desktop when they do not. So the target gets `CF_HDROP` and the Shell IDList Array, as from Explorer. `SHDoDragDrop` runs the drag with the shell's drop source and drag image, allowing copy, move and link. After a move the window does nothing: the index follows the journal.
- **OLE:** the window's thread calls `OleInitialize` after `CoInitializeEx`, which the drag needs.
- **No shell extension loads:** only the drop target's code runs, as with any drag, so the rule against loading shell context menus holds.
- **At most 1,000 entries:** the window names every entry and builds the data object on its thread before the drag starts. A larger selection does not start a drag; a message gives the limit and the number selected and points to Edit > Copy, which has no limit, or in a panel, where Edit > Copy is not available, to `F5`.
- **A missing entry:** a deleted entry is dragged too, because its path is named from the row and not from the disk; the drop target reports it, as File Explorer's own drop targets report any stale item. A path that the shell cannot name at all, such as one on a drive that is gone, is left out, the others are dragged, and after the drop the window reports the first one, as Open does.
- **Dropping into Gozler:** files dropped on a folder row, from another program or from the list itself, are copied or moved into it by the engine, as Paste does. `core::drop_target` chooses the effect, and `win::drop_target` follows the shell's rules for an optimized move and for a source that works asynchronously, below. Only the window that owns the file operations accepts a drop. File Explorer's rules choose the effect: a move when the files are on the folder's volume, a copy otherwise; `Ctrl` forces a copy and `Shift` a move, each only where the source allows that effect (`DragEnter`'s effect mask). The row under the pointer is highlighted, and the pointer shows "not allowed" over a file row, over empty space, and for data without `CF_HDROP`, such as an e-mail attachment, because the engine copies paths. The saved policy resolves conflicts without another question (see "File operations").
  - **The drop reads the paths again.** `DragEnter`'s `CF_HDROP` only chooses the effect. A source can make its files only when the button is released: 7-Zip names just its empty `7zE…` temporary folder until then, and extracts the files, with its own Copying progress, when the target asks for `CF_HDROP` after the drop. So after `Drop` a thread of its own reads `CF_HDROP` through an agile reference to the data object, and the window starts the operation with those paths when they arrive; data that holds none drops nothing. The thread waits for the source's process, and the window does not.
  - **A move is reported as optimized.** Gozler moves the files itself, so `Drop` returns copy, not move, and, unless the source works asynchronously, sets `CFSTR_PERFORMEDDROPEFFECT` to none. Reporting move would make the source delete the originals as well ("Handling Shell Data Transfer Scenarios", optimized move).
  - **A source that works synchronously keeps its files until the operation ends.** A source without `IDataObjectAsyncCapability` in asynchronous mode, such as 7-Zip, deletes the files it made for the drag as soon as `Drop` returns. So `Drop` returns only when the operation has ended, as File Explorer's does ("Dragging and Dropping Shell Objects Asynchronously"), and runs the window's message loop until then, so the window does not wait. The source stays frozen for the operation's whole life: while it waits in the queue, while it is paused, until a dialog opened meanwhile closes, and, when another such drop came meanwhile, until that drop's operation ends too. Exit ends the wait. The list's own drag is not held: its files are the person's, which no source deletes, and a wait would hold the drag that started it, with every later drag inside it. The shell's data object that it carries ignores `SetAsyncMode`, so `Drop` compares the dropped object with it by COM identity (`IUnknown`) instead.
  - **A modal dialog or a modal step blocks drops.** While a modal dialog runs on the window's thread, which Windows shows by disabling its owner, or a modal step runs (window rule 13), the window offers no effect and `Drop` does nothing, as for the person's other input. A drop under the dialog would leave it without its keys, and the code that waits for its answer, until the drop's operation ended. A step can wait with no window disabled, such as Open's call into another program, and a drop held under it would never let that call return.
  - **A source that works asynchronously keeps its files until it is told.** When the data supports `IDataObjectAsyncCapability` with asynchronous mode on, `Drop` calls `StartOperation` and returns at once, sets no `CFSTR_PERFORMEDDROPEFFECT`, and the window keeps the data object under the operation's id until the operation ends; the engine holds no COM object. When the operation ends, Gozler sets `CFSTR_PERFORMEDDROPEFFECT` and calls `EndOperation` with the same effect: none for a move, so the source deletes nothing, also not the files the move skipped or could not move; copy for a copy. The result code says whether the operation succeeded. `EndOperation`'s effect has the meaning of `CFSTR_PERFORMEDDROPEFFECT`, so move there would tell the source to delete its originals. These calls run through the work owner on a worker with agile references to the data object. Closing the window queues abort feedback for remaining sources and starts a one-second completion opportunity, independently of settings and recovery writes. The off-UI finalizer waits only for the remainder of that same second; other cleanup consumes the budget rather than starting another one. A frozen native COM call can outlive the budget, and feedback queued behind it may be lost when the process ends.
- **A drop with the right button** shows File Explorer's menu, from another program and from the list's own right-drag alike: Copy here and Move here, with the effect a drop without the menu would have in bold, then Cancel. An item that the source does not allow, or that would do nothing, such as Move here into the files' own folder, is disabled. Gozler makes no shortcuts, so the menu has no shortcut item. Cancel, or a click outside the menu, drops nothing. `DragEnter` records the right button, because the keys that `Drop` gets no longer hold the released button.
- **The list scrolls during a drag** while the pointer rests on its header or within a row's height of its top or bottom edge, a row each tenth of a second, as File Explorer's list does. Each row it scrolls moves the highlight to the folder row now under the pointer.

### Status bar

The status bar has three groups, as TinyTorrent2's has: what the view holds on the left, work in the middle and the service on the right. While Settings > General's Show memory use in the status bar is on, the main window's bar also has the memory group at its right end, before Show details and the size grip. `app::status_bar` owns the Win32 status bar and the controls in its middle group, `core::captions` gives it the texts and `core::layout::StatusLayout` the places.

| Group | Shows |
| --- | --- |
| Left | the results, then the selection: "1,204 results · 3 selected · 12.4 MB". The results are "1,204 results" or "Searching…", with "· Filtered" added while a filter is set and the filter pane is hidden (`S10`). The selection is "3 selected · 12.4 MB", with the files' sizes only, "3 selected" when only folders are selected, or "Reading the selection…" |
| Middle | left-aligned activity: the current job, transfer progress and per-operation actions, then indexing progress (see "File operations"). Export, service fixes and ordinary Delete retain their own native status object. Delete shows its completed item count and items per second using the same rolling meter as copying; the rate waits for a one-second sample and disappears while paused. Each object's last result stays until its next job; a long text ends in an ellipsis and its tooltip shows it whole |
| Right | the service or index state when it affects the answer, then the shown search’s totals: "1,192 files (3.4 GB) and 12 folders". The file and folder totals each follow their icon in adjacent native status bar parts, without borders between them. The files part sums known file sizes and omits the unknown-size count. The folders part shows only its count. A kind with no matches is left out, and "and" appears only while both totals are shown. Each total after the first right-hand part shows only when it fits whole. Where the file total does not fit, it drops the size. Its tooltip shows both totals whole when either is shortened or left out. The totals cover the whole result set, not the selection, and come from the service’s search scan. A later deleted or moved row can lower the result count without changing these totals until the next search. No totals show while a search is running or when the service sent no sums. The Settings dialog’s status bar belongs to Settings |
| Memory | the memory that the service and the app use, each after its icon, as TinyTorrent2 shows its engine's and its UI's, but with no "Memory" caption before them, because the words Service and UI with their icons already say what each figure is: "Service 380 MB  UI 45 MB". Each figure is the process's working set, the figure of Task Manager's Working set column; the Drives and service page shows the same figure for the service. The service reports its own in each status reply, and the app reads its own with one call when a reply brings the service's, so both come from the same moment. A figure that is unknown shows "—": the service's while it does not answer, because the latest status can be from before it stopped. Each figure has a tooltip that says what it measures. Each figure is a part of the status bar of its own, with its icon (`SB_SETICON`) and its text, and only the main window's bar has the group |

- **Cancel** stops the job that the group shows: an export, a running file operation, or a fix while it waits for the service, finds the latest release or downloads its setup (see "Service fixes"). File > Cancel does the same, and so does `Esc` in the results list, as `Esc` in the search box empties the box, except for a file operation (see "Commands"); the Keyboard page can give File > Cancel other keys. `Tab` reaches the button in the Settings dialog, as it reaches every button of a dialog, but not in the main window (see "Dialogs"). It has no access key, because every letter of the English "Cancel" belongs to the menu bar or another control of the window; File > Cancel has one. When the job ends while Cancel has the focus after a click, the focus moves to the search box, because a hidden control keeps the focus but takes no key.
- **The progress bar moves without measuring, except for a download,** because an export does not know its total, and a fix does not know how long Windows and the service take. A download measures its bytes against the setup's size, which GitHub gives.
- **The Settings dialog's status bar belongs to Settings.** It sits below OK, Cancel and Apply, without a size grip. Its fixed left part says "Unsaved changes" only while the edited settings differ from those last applied; its remaining width belongs to Settings' work. It shows "Saving settings…" while an Include save awaits Windows and "Requesting rebuild…" while a rebuild request awaits Windows. Those bars stay out of the main window. A service fix running when Settings opens, or started while it is open, shows its progress and result here, and the main window leaves out that same fix's text, progress and Cancel until Settings closes. Settings' Cancel stops that fix directly, even while an export or file operation runs elsewhere. A newly opened dialog does not show an earlier fix's result. Search counts, file operations, exports, their completion messages, memory figures and update notices stay in the main window; the service and drive states stay on Drives and service. Settings places progress and Cancel against its own right edge and shortens a long running message in the space left, with the full message in its tooltip.
- **Show details** stands at the right end of the main window's status bar, before the size grip, while the inspector is hidden: a toolbar button with only its icon, as high as Cancel (see "Inspector pane"). `Tab` does not reach it; View > Inspector does the same.
- **The memory figures are fresh every 2 seconds while the window is on screen,** neither minimized nor hidden in the notification area, through the status poll (see "Requests"), and when the window comes back on screen it asks at once. A hidden window asks for no figure, because nobody reads it.
- **The progress bar, Cancel and Show details are children of the status bar,** which passes their notifications to the window, so a themed button draws its corners, and the flat toolbar its background, on the status bar's background. Cancel is a compact button 6 px taller than its font at 96 DPI, and the status bar's drawing area is Cancel's height plus 2 px above and below it.

The right group shows the service's failure when the list keeps its rows or another message, so the window still says that the service stopped (`T10`), and reports an offline or failed volume in scope. Active indexing appears only in the center group's progress bars. Unknown sizes or dates that affect a search on a completed volume still show their warning. A successful service start adds no completion message. When Check for updates finds a newer release, its notice runs Update when clicked.

Index builds publish their progress in the same status replies as the volume
phase, separately from the entry count of the published index. Each active
drive has a label, a native progress bar and a value in the center slot, alongside other
work. NTFS shows an estimated scan percentage from the MFT record position
and the MFT's size at the start of the scan. Sequence bits are excluded from
the position; unused records are part of the scan, not files still to find.
The estimate stays below 100% until the scan ends. If the MFT size cannot be
read or records arrive out of order, the entry shows names found instead,
as the ReFS folder walk does. The row reads "Indexing C: [bar] 55% · D: [bar] 230,000 entries".
Final processing keeps the moving bar and entry count. The entry disappears
when the volume is ready or fails. These updates
use the existing two-second status poll while the window is on screen,
including drives outside the search's scope. Drives and service shows the
same progress and the build's current entry count. Each volume keeps its
existing worker; reporting adds no scan or thread, and no work on the window's
thread. At narrow widths the per-drive details collapse to a complete
"Indexing" label. Generic-job text gives up width before that label or the
job's progress and Cancel controls. The tooltip retains all drive details.

### Window lifetime

- **One window per user session, unless the person allows more.** `main` creates the named mutex `Local\gozler.ui`, and the `Local\` namespace is already one per session. A second start finds the first window by its class name, calls `AllowSetForegroundWindow` for its process, posts it a registered message and exits; the first window comes forward, restored when it was minimized and still maximized when it was maximized, with the search box focused and its text selected (`MainWindow::show_window`, `C24`); a second start explicitly shows Search, as Show Search does. A second start with `--location <folder>` also sets Location, through `WM_COPYDATA`. A start with `--location`, the first or a second, shows Search, because Location is Search's filter.
- **A second start of another program replaces the first window,** because a person who starts a build expects to see that build. Another program is another file, such as another build's folder or a running copy that a developer renamed so the build could replace it, or the same file written after the first process started (`app::runs_another_program`); names compare without case, as NTFS does, and a process whose file Windows does not name counts as the same program, so a start never closes a window it cannot name. The second start sends the first window the registered message `gozler.ui.exit`, which runs the same Exit as File > Exit: with file operations running or waiting, the first window comes forward and asks the Exit question, so they are saved for Continue. When the first window ends, the second start waits, without a limit, for that process to end, which can outlive its window while essential persistence and accepted cleanup finish, and then becomes the first window; when the person keeps the operations, or the first window has a dialog open, the first window stays in front and the second start ends. A first window from a build before this message does not answer it, and comes forward instead. A start at sign-in never closes the running window. The setup sends the same message when the person selects Install, not when it starts, and the uninstaller once the person confirms the uninstall; each says that Gozler is running only when it stays.
- **A first process without a window** is starting, and shows its window, or ending, while its threads finish, and still holds the mutex. A second start waits up to 10 seconds for either: it hands over to the window that appears, or becomes the first window when the mutex is free, so a start just after Exit still shows a window. After 10 seconds it opens another window, as when "Allow only one Gozler window at a time" is off, because a start never ends without showing a window; a start at sign-in ends instead, because nobody waits for its window.
- With `--data-dir`, the mutex and the class name end in an FNV-1a hash of the folder (`app::instance_suffix`), because a mutex name allows no backslash after `Local\`, so a test's window and the person's window never hand over to each other.
- **"Allow only one Gozler window at a time"** (Settings > General, on by default, as Total Commander's "Allow only 1 copy of Total Commander at a time") decides what a second start does. Off, it opens another window in its own process, and `--location` sets that window's Location. Another window has no tray icon, no shortcut to show Gozler and no update check, because the first window does those; Close ends it; and it saves only what its Settings applies, so closing it never undoes a setting that another window saved. A setting that two windows change is the one saved last. A start at sign-in never opens another window. `main` reads `ui.json` before the mutex, so the second start knows the choice.
- **"Keep Gozler running in the notification area when I close its window"** (Settings > General, on by default, as Everything's "Run in background"):

  | | Off | On |
  | --- | --- | --- |
  | Close (`Alt+F4`, the title bar's X) | exits | hides the window |
  | Tray icon | none | shown while Gozler runs |
  | The shortcut from any program | works while the window is open | always works |
  | At sign-in, with "Start Gozler when I sign in" on | the window starts minimized | Gozler starts in the notification area, with no window |
  | Exit (File > Exit, `Ctrl+Q`, the tray menu) | exits | exits |

- **The tray icon** uses `Shell_NotifyIconW` with `NOTIFYICON_VERSION_4`. A click or `Enter` on it shows Search (`NIN_SELECT`, `NIN_KEYSELECT`); its menu holds Show Search, bold and the default, Show Commander, and Exit, and its tooltip is "Gozler". Before its menu opens, the window calls `SetForegroundWindow`, as the `Shell_NotifyIconW` documentation requires so that the menu closes on a click elsewhere. Explorer drops every tray icon when it restarts, so the registered `TaskbarCreated` message adds the icon again.
- **The first time** the window hides, a Windows notification (`NIF_INFO`) says "Gozler is still running. Press {shortcut} or click its icon in the notification area to show it. Choose Exit from its menu to stop it." `ui.json` records it, so it shows once, and again after Settings' Reset all.
- Hiding the window changes nothing else: an export goes on, and the status bar's middle group shows its result when the window shows again. Exit cancels request interest and closes the UI; required finalization may retain the invisible process (window rule 5, `T14`).

### Commands

`core::commands` holds each command's menu, group and default keys (window rule 9), and Settings > Keyboard changes the keys. A disabled command stays in its menu, gray, so the menus never change shape. "The selection is read" means that it holds its files.

| ID | Command | Enabled when | Acts on |
| --- | --- | --- | --- |
| C1 | Open | 1 to 15 rows selected, and the selection is read | in Search, the configured folder or file command, or without one the result in Commander; in Commander mode, as Windows opens it (see Searching) |
| C36 | Show in Commander | one Search row selected and read | once the folder opens, shows it in Commander mode, or the file's containing folder with the file focused; a folder Windows refuses leaves the window where it was (see Searching) |
| C2 | Open file location | as C1 | the folder of each entry, with the entry selected, or the "Open file location" command |
| C26 | Explore | as C1 | each entry in File Explorer, whatever the open commands say, as Everything's Explore: a folder opens, and a file's folder opens with the file selected; it has no default shortcut |
| C3 | Properties | exactly 1 row selected, and the selection is read | the shell's Properties dialog for the entry |
| C4 | Export results… | the list shows results and no export runs | every match of the shown search, not only the rows the list reaches |
| C5 | Cancel | the status bar's middle group shows a job that Cancel can stop: an export, a running file operation, or a fix that waits for the service, finds the latest release or downloads its setup | that job, as the status bar's Cancel does; a stopped export's partial file is deleted; `Esc` in the results list runs it, except for a file operation |
| C6 | Settings… | always | — |
| C7 | Exit | always | ends Gozler |
| C8 | Copy | the list is focused with rows selected and read, or the search box has selected text | the files, as `CF_HDROP`, as File Explorer copies them; or the selected text |
| C9 | Copy path | rows selected, and the selection is read | each full path in double quotes on its own line, as File Explorer's Copy as path |
| C10 | Copy name | as C9 | each name on its own line |
| C25 | Copy folder | as C9 | the path of the folder that holds each entry, on its own line, a drive's root with its backslash, such as `C:\`; it has no default shortcut |
| C11 | Select all | always | the focused control: the search box's text, or every row |
| C12 | Search in this folder | exactly 1 row selected, and the selection is read | sets Location to the entry's folder, or to the folder itself |
| C13 | Filters | always | shows or hides the filter pane; the pane's hide icon and Show filters run it too |
| C14 | Inspector | always | shows or hides the inspector pane |
| C15 | Columns ▸ one item per column | always; Name cannot be hidden | shows or hides the column |
| C16 | Language ▸ Windows default, each shipped language | always | the window's language (see "Language") |
| C17 | Refresh | a search has run | runs the shown search again |
| C18 | Find | always | focuses the search box and selects its text |
| C19 | the match controls | always | the match mode or option; runs the search again |
| C20 | Sort by ▸ each sortable column; Ascending, Descending | always | the sort; sorts the result snapshot again, or runs the search again |
| C21 | Clear filters | a filter is set | sets every filter group to its first choice; the pane's Clear runs it |
| C22 | Search syntax | always | a dialog with examples, the search terms, the search functions, the attribute letters and notes; no options, sort keys, shell quoting or exit codes, because none applies in the search box |
| C23 | About Gozler | always | a dialog with the window's version and the version of the service it is connected to |
| C24 | Show Search | the window is running | shows Search and selects the search box's text |
| C35 | Show Commander | the window is running | shows Commander mode and focuses the active panel |
| C27 | Copy to folder… | rows selected and read, in the window that owns the file operations | the files, copied to the destination with the accepted policy (see "File operations"); it has no default shortcut |
| C28 | Move to folder… | as C27 | the files, moved as C27 copies them |
| C29 | Delete | as C27 | the files, deleted permanently after the Delete question; `Delete` and `Shift+Delete` in the list run it |
| C30 | Cut | the list is focused, and as C27 | the files, on the clipboard to be moved (see "Copy"); `Ctrl+X` in the list runs it |
| C31 | Paste | the list is focused, the window owns the file operations, the clipboard holds files, and exactly one folder row is selected and read | the clipboard's files, moved into the folder when they were cut and copied when they were copied (see "Copy"); `Ctrl+V` in the list runs it |
| C32 | File operations | the window owns the file operations | opens the shared File transfers pane |
| C33 | Commander | always, checked while Commander mode shows | switches between Search and Commander mode (see "Commander mode") |
| C34 | Synchronize folders… | the window owns the file operations | compares explicit source and destination folders, then offers Synchronize; no default shortcut |

Copy to folder…, Move to folder… and Commander have no default shortcut, because File Explorer has none for the first two and `F5` is Refresh in Search. `Delete`, `Shift+Delete`, `Ctrl+X` and `Ctrl+V` are keys of the list, not accelerators (window rule 8), so in the search box they keep deleting, cutting and pasting text. `Esc` never stops a file operation, because one stray key would leave a copy half done; File > Cancel and the status bar's Cancel do. Select all reaches rows the person never saw, up to 100,000 (`S4`), so the Delete question gives the count.

Open, Open file location and Explore stop at 15 entries, because File Explorer asks before it opens more than 15 items, and the window would rather not open many windows by accident. An empty open command does what File Explorer does (see "The Context menu page").

### Menus

Groups are separated by a separator, in this order:

- **File:** Open, Open file location, Properties | Copy to folder…, Move to folder…, Delete | Synchronize folders… | Export results…, Cancel | Settings… | Exit
- **Edit:** Cut, Copy, Paste, Copy path, Copy name, Copy folder | Select all | Find. Cut, Copy and Paste are in File Explorer's order.
- **View:** Commander | Filters, Inspector, Columns | Sort by | Refresh | File operations
- **Help:** Search syntax | About Gozler
- **Language,** aligned right on the menu bar (`MFT_RIGHTJUSTIFY`): Windows default | each shipped language by its own name
- **The context menu on a row:** Open, Open file location, Explore | Cut, Copy, Paste, Copy path, Copy name, Copy folder, Copy to folder…, Move to folder…, Delete | Search in this folder | Properties. Paste shows only on a folder row, which it pastes into. Each item is shown, shown only with Shift down, or hidden as Settings > Context menu says (see "The Context menu page"). Gozler builds this menu itself and never asks Windows for the shell's context menu (`IContextMenu`), because that loads every installed shell extension, other programs' code, which can take seconds and crash the window.
- **The tray menu:** Show Search | Show Commander | Exit

The menu bar has four menus besides Language, as TinyTorrent2's has, and each holds what its name says: File acts on the selected files and the window, Edit on the clipboard and the selection, View on what the window shows, and Help on how to use it. There is no Search menu: a menu closes after each click, so setting two match controls took two trips, while the strip beside the search box shows and changes them in one place. Find is in Edit, where Notepad has it, and Sort by in View, where File Explorer has it. A context menu shows no shortcuts, as File Explorer's does. The shortcut that a menu item shows comes from the same row as its accelerator, so they cannot differ, and the modifier names come from the catalogue, because German Windows writes `Strg` for `Ctrl`.

### States

What the list area and the results in the status bar's left group show. Each failure's message is in "Daemon failures".

| ID | State | List area | Results in the status bar |
| --- | --- | --- | --- |
| S1 | Starting, before the first reply | empty, no message | empty, then Searching… after 300 ms |
| S2 | A new search runs | the previous rows | the previous counts, then Searching… after 300 ms |
| S3 | Results | the rows | "1,204 results" |
| S4 | More than 100,000 rows are selected | unchanged | unchanged; the selection after them says "3,000,000 selected · Select up to 100,000 to use a command" |
| S5 | No results | "No files match.", with Clear filters when a filter is set and Search everywhere when Location is set | "0 results" |
| S6 | The empty search, with "Show every file and folder when the search is empty" off | "Type to search." | empty |
| S7 | The search does not parse | the parser's message and how to fix it, such as "dm: needs a date, such as dm:today or dm:2026-01-31" | empty |
| S8 | The Location folder does not exist | "The folder D:\Old does not exist.", with Choose folder… and Search everywhere | empty |
| S9 | A daemon failure | the failure's message and its button | empty |
| S10 | A filter is set and the filter pane is hidden | the rows | "· Filtered" added |
| S11 | An export runs | unchanged | unchanged; the middle group shows its progress |
| S12 | The selection is being read | unchanged | unchanged; the selection after them says "Reading the selection…" |
| S13 | A volume in scope is not complete | the rows | unchanged; the right group says so |
| S14 | A row's file is gone since the list's result snapshot was made | the row says "No longer exists" in Name | unchanged |

### Transitions

What changes, and what does not, when an event arrives. The `Model` test for a row has the row's ID in its name, such as `t3_a_failure_while_rows_are_shown_removes_them`.

| ID | Event | What the person sees | Work in progress |
| --- | --- | --- | --- |
| T1 | The search text, a match control, a filter or the sort changes | the previous rows and selection stay; the status bar says Searching… once the search has run 300 ms | the running search is cancelled; an export and a selection read go on |
| T2 | A new search's first rows arrive | the new rows replace the old ones, scrolled to the top; the selection is cleared, and its commands are disabled | a selection read for the old search is cancelled |
| T3 | A search fails while rows are shown | the rows are removed and the failure's message shows | a selection read is cancelled; an export goes on and ends by itself |
| T4 | Status shows that the daemon answers again, or that every volume in scope is complete | the search runs again | the status poll stops when nothing needs it |
| T5 | A volume in scope starts or ends indexing | its progress entry appears or disappears; the rows stay | the status poll starts or stops |
| T6 | The selection changes while it is read | the status bar says Reading the selection…; its commands stay disabled | the old read is cancelled; one new read starts 150 ms after the last change |
| T7 | The selection changes while a folder's size is counted | the inspector shows the new selection | the old size request is cancelled |
| T8 | An export ends while a search runs | the middle group shows the export's result; the left group still shows the search's state (S2) | none |
| T9 | An export fails | a message box names the file and Windows' reason; the middle group is cleared | the partial file is deleted |
| T10 | The service stops while the Settings dialog is open | the dialog stays usable; its Drives and service page shows the failure; the main window shows it behind | the status poll runs |
| T11 | New results arrive while the context menu is open | the menu stays open; a command chosen from it acts on the selection when it is chosen, and does nothing when the selection was cleared meanwhile | none |
| T12 | The language changes | every caption changes; nothing else does | none; no search runs |
| T13 | The window moves to a monitor with another DPI | the layout and fonts scale; nothing else changes | none |
| T14 | Gozler exits while work runs | the window closes at once | every request is cancelled; a partial export is deleted; the settings are saved |

### Feedback

Each kind of message has one place, so the person always knows where to look:

- **A lasting fact** about the results or the index is in the status bar.
- **The result of a search** that has no rows, or failed, is in the list's empty area, with a button for the fix when there is one.
- **The progress and result of an export and of a file operation** share the status bar's middle group as separate objects (see "File operations").
- **A failed action that the person started** is a message box owned by the window. It names the file and Windows' reason and says what to do, as Notepad does for a failed save.
- Nothing shows success for an action whose result the person can see, such as Open or Copy.

The message boxes cover an export that cannot write its file, a clipboard that another program holds, a file to open that no longer exists, an open command that cannot start, settings that cannot be saved, and a shortcut that another program holds at start. A failed save shows once per session, because the changes still apply until Gozler exits. The shortcut's message shows the first time the window shows in that session, because at sign-in Gozler starts without a window.

### Daemon failures

A failure empties the list, and its empty area shows the message and its button. The message says what is wrong and that Gozler cannot search, such as "The Gozler service is stopped, so Gozler cannot search."; the button says what it does in one common Windows word, two at most, because the sentence beside it gives the context. The window then recovers by itself: after a daemon restart or a late service start, it searches again with no action from the person (`T4`). The service states are the ones `gozler` tells apart for its fix-it messages (see "Exit codes"):

| Failure | Button |
| --- | --- |
| The service is stopped | Start |
| The service is disabled | Turn on: the service starts automatically from then on |
| The service is not installed, and `gozlerd.exe` is beside the window | Install: it installs that `gozlerd.exe` |
| The service is not installed, and `gozlerd.exe` is missing | Install: it downloads and runs the latest setup |
| The daemon is older than the window | Update: it downloads and runs the latest setup; Downgrade when that release is older than the window. Before it, Use local, when the `gozlerd.exe` beside the window serves the window's protocol: it points the service at that `gozlerd.exe` |
| The service's state cannot be read | Open Services |
| The service runs but does not answer yet; or the connection is lost, or the pipe cannot be opened or read | none: the window polls |
| The owner check fails | none: the message says to restart Windows, so that the Gozler service starts first |
| The daemon refuses the request | none: the message says to change the search, followed by the daemon's English detail |

The buttons run the commands that `gozler` prints for the same failure, as a service fix (see "Service fixes"). A button that runs them as administrator shows the administrator shield, as Windows' own buttons that ask for permission do. The status poll that the failure started then notices the service and searches again. Update replaces a daemon older than the window with the latest release's setup. When the service is first found older than the window and Check for updates is on, the window asks GitHub for the latest release on a thread of its own, and the button reads Downgrade once the release is known to be older than the window; until then, when GitHub does not answer, and with Check for updates off, it reads Update, and the question after the click says whether it updates or downgrades (see "Service fixes"). Use local is a button of its own, because the `gozlerd.exe` beside the window may be a development build, so the person chooses it. It shows only when that file serves the window's protocol, which the window reads from the file's hidden `gozlerd protocol` command, without starting a service, once for each build of the file, and only when its bytes differ from the program that the service's configuration names (`core::programs` decides the order of these checks, and `app::work::local_service` reads and runs the files). The setup's build pairs the window with the gozlerd.exe built with it: `installer\Pair.ps1` writes that file's SHA-256 into gozler-ui.exe after the build, because cargo cannot make the window's link wait for gozlerd.exe. A paired window offers Use local only for a gozlerd.exe with that SHA-256, compares it before it runs the file's `protocol` command, and again when the person confirms Use local, because the file may have been rebuilt or replaced since the check. Otherwise Use local stays off, and when the second comparison fails, a message says "Use local isn't available because the gozlerd.exe beside this app wasn't built with it." and nothing runs. The pairing catches a wrong, stale or half-built gozlerd.exe and one replaced on its own, but not both programs replaced together, because they sit in one folder; there are no signed releases or published hashes to compare with. A window that was never paired, such as one from `cargo build`, offers any gozlerd.exe that passes the other checks. The work owner checks metadata and runs the file checks in separate bounded tasks, and ends a program that has not answered after 10 seconds, so a status request never waits for it and the button shows from the next status after the check. A check that ended a program at that limit, such as during an antivirus scan, runs once more at the next status, and its result is then kept until the next build of the file; while it runs again, Settings > Index keeps showing the first check's result. A blocked check retains its execution slot but cannot hold the window open at Exit. The bytes decide, because a development build reports the same version as the release it follows, and an installed Gozler, whose window runs beside the service's own `gozlerd.exe`, then never offers it. Its tooltip says "Switch the Gozler service to version 0.5.0, which matches this app." and gives that file's path on a second line. Update and the download's Install run the latest setup, which asks for permission itself, and Open Services opens `services.msc` without elevation.

### Service fixes

A fix runs a failure's commands, or downloads and starts the latest setup, and shows its current step in the owning status surface. `core::fixer` owns the distinct command and setup states, so a step of one cannot pair with the other. `app::work::fixes` runs them through the common work owner and reports steps through its bounded channel and live-window wake endpoint. Model owns the one fix, which outlives the Settings dialog; the list and Settings buttons both call `MainWindow::run_fix`.

| Step | Middle group | Progress bar | Cancel |
| --- | --- | --- | --- |
| Asking Windows | "Waiting for permission…" | none | no: the prompt is Windows' |
| Running the commands | "Starting the service…", "Installing the service…" or "Updating the service…" | moving | no: a half-run `sc.exe` command is worse than a finished one |
| Waiting for the service | "Waiting for the service to answer…" | moving | yes |
| Finding the release | "Checking for updates…" | moving | yes |
| Asking the person | "Checking for updates…", under the question | none | no: the question is modal |
| Downloading | "Downloading Gozler 0.4.0… 3.1 MB of 8.4 MB" | bytes of the setup's size | yes |
| Checking the file | "Checking the download…" | moving | no: it takes under a second |
| Starting the setup | "Gozler closes to install 0.4.0." | none | no |

A fix ends as done ("The service runs again."), cancelled ("Stopped waiting for the service." or "Download cancelled."; a declined question says nothing), refused ("Windows did not give permission."), or failed: "The service could not be started." (or installed, or updated) with Windows' reason or the exit code, "Windows could not run the commands." or "Windows could not start the setup." with Windows' words, "The service did not answer after its commands ran.", "The download failed. Check the internet connection, then try again.", "Gozler could not save the setup in C:\Users\…\Temp\Gozler." with Windows' reason or "The download was damaged, so Gozler deleted it. Try again."

- **The commands are watched.** `app::work::fixes` joins them with `&&` into one `cmd /c`, which `win::shell::Session::start_as_administrator` starts with the `runas` verb and `SEE_MASK_NOCLOSEPROCESS`, so Windows asks for permission once, a failed command stops the ones after it, and the window never sees or passes credentials. `ERROR_CANCELLED` from the prompt is the refused result. The fix's thread waits for the process and reads its exit code.
- **A failed command says Windows' reason.** The commands run hidden as administrator, and `net start` ends with 2 for any failure, so the exit code says nothing. Each command therefore appends what it writes, and its errors, to a file in the person's `%TEMP%`. When the commands fail, the window reads the last line that says something and shows it after a translated sentence, as it shows the daemon's English detail: "The service could not be started. The service cannot be started, either because it is disabled or because it has no enabled devices associated with it." `net`'s closing pointer to `NET HELPMSG` is skipped, because it says nothing itself. `net` writes in the console's code page and `gozlerd` in UTF-8, so the line is read as UTF-8 when it is valid UTF-8, and in the console's code page otherwise. Without such a line, the sentence gives the exit code. The file is deleted when the commands end, and every earlier fix's file when a fix starts, because commands that still run when their window closes write after it has deleted their file. A `%TEMP%` that administrators cannot write makes the commands fail before they run, which is rare enough to need no other path.
- **After the commands, the fix waits up to 30 seconds for the service** to answer a status request. The window asks once when the commands end and then through the status poll, which runs while the fix waits, because a service that answered while the commands ran has already stopped the poll. An answer before the commands end does not end the fix, because the commands may still fail.
- **Use local asks, then points the service at the local program.** A task dialog captioned "Use local service" asks "Switch to the local Gozler service?", says "Installed version: 0.3.0" and "Compatible version: 0.3.0 (2026-10-19 21:10)" on two lines, offers Switch as the default, and Cancel, and says where the new version comes from in its footer, beside an information icon: "Found in: C:\Source\goz\target\debug". The service refuses the window before it says its version, so the installed version is what the service's program prints for `--version`; the line is left out when that fails. Then its commands run that `gozlerd.exe install`, the Install fix's commands, which change the service's configuration to that file, and then stop and start the service. `install` comes first, because it changes only the configuration of a service that runs, so a failed `install` leaves the service running its old program. Nothing is copied: the running service locks its program, and the setup that installed it still owns that file. A later setup points the service back at its own `gozlerd.exe`. The window stays open and searches again once the service answers.
- **Update, Downgrade and the download's Install ask before they download.** They ask GitHub for the latest release, as Check for updates does, and take its asset `gozler-<version>-x64-Setup.exe` (`core::update::setup_to_install`). Then a Windows task dialog, owned by the Settings dialog while it is open and else by the window, asks. Its title names the command, as Windows' own dialogs do, and its body gives the versions and its footer, beside an information icon, says where the new version comes from, because nobody reads long text:

  | | Update | Downgrade |
  | --- | --- | --- |
  | Title | "Update Gozler" | "Downgrade Gozler" |
  | Question | "Update Gozler to version 0.4.0?" | "Downgrade Gozler to version 0.3.0?" |
  | Body | "Installed version: 0.3.0 (2026-10-19 21:10)" | the same |
  | Footer | "You'll download 3.74 MB from GitHub." | the same |
  | Icon | none | warning, as Windows marks a choice the person may regret |
  | Buttons | Update, the default, and Cancel | Downgrade and Cancel, the default |

  The installed version is the window's, with a local build's time. No line of these dialogs wraps: the window sets the dialog's width from its longest line, measured in the dialog's fonts, footer included. The values in a body line up: the window measures each label, such as "Installed version:", in the message font, which is proportional, and puts spaces after the shorter labels until each value starts within half a space of the others (`win::dialog::aligned`), at most the width of the monitor's work area, because Windows' own width wraps a long folder. The download's Install asks "Install Gozler 0.4.0?" and says "Gozler closes while the setup installs it. Before that, Gozler downloads the setup from GitHub (3.9 MB) and checks it.". The buttons carry the window's words, because Windows' common Cancel follows the Windows display language. Cancel ends the fix with nothing in the status bar. When the release gives no setup, a message box says "The latest Gozler release has no setup that Gozler can check.", and nothing downloads.
- **The download** goes over HTTPS to a folder of its own in `%TEMP%\Gozler\`, named by the process and the fix, so a stopped download that ends late never deletes the next one's file. It reads the file back and compares its SHA-256 with the asset's `digest`. A file that differs is deleted and the fix fails as damaged; a file that cannot be written or read back, such as one that antivirus software took away, fails with Windows' reason; a release without a digest gives no setup, because a setup that cannot be checked is not run. The window then starts the setup and closes, because the setup replaces `gozler-ui.exe`, which a running window locks; the setup's own window shows the install and offers to start Gozler again.
- **The window closes for the setup only outside every modal loop.** Settings, the service report, a message box or the folder picker may be open when the setup starts, and destroying the window inside one leaves that loop running on a destroyed owner. So the window posts `WM_QUIT` instead of closing: the loops of a dialog, a message box and the file dialog each end on it and post it again, and the window closes once its own message loop has returned (`MainWindow::close_for_setup`). A menu's loop neither ends on `WM_QUIT` nor posts it again, so an open menu is ended first (`win::window::quit_modal_loops`). A dialog that `WM_QUIT` ends returns its default button's ID, so `win::dialog::run_modal` reports it as Cancel, and a Custom date or size is not applied on the way out.
- **The setup asks for permission itself, after Gozler has closed.** The window starts it as a double-click does, not with `runas`, because the setup restarts Gozler as the person who started it, and with `runas` that would be the administrator. The window therefore cannot see a refused prompt; the person sees the prompt and answers it.
- **An update from another window closes the first window too.** At Install, the setup asks it to exit, as a start of another build does (see "Window lifetime"), and stops with "Gozler is still running" only when the person keeps its file operations or it has a dialog open.
- **A release older than this window is a downgrade,** never an update: its version is lower than the window's, or it is the same and GitHub published it before this local build was built, so its setup would put the build's changes back. A release of the window's own version is not older for a release build, so a window from a release can repair its own service. The status bar names only a newer release.
- **No fix runs by itself;** each starts from a click on its button, because each changes the machine or downloads a program.
- **One fix runs at a time.** While one runs, every repair button is disabled, in the list and on the Drives and service page. Open Services stays enabled, because it starts nothing that a fix could meet.
- **Cancel ends interest in a cancellable fix step.** The worker checks cancellation while waiting for the service and between download reads, and cleans up a stopped download's partial file. A later fix can be accepted without waiting on the UI thread, but its execution waits for the old native call and cleanup to release the bounded slot. Old events cannot update the newer fix.
- **Closing the window closes the fix's delivery** without waiting on the UI thread. Accepted repair commands finish off UI; a download loses interest and finishes its cancellation and partial-file cleanup there. This cleanup is independent of settings and recovery writes (window rule 5).

### Filter pane

A pane on the left, shown and hidden with View > Filters (`C13`). Its width follows its widest measured caption. A header heads the pane: the title "Filters" at the left, in the semibold of the window's font, as Settings' page headings are, and at the right a toolbar with Clear and Hide filters. Clear runs Clear filters (`C21`), is enabled only while a filter is set, and has the tooltip "Clear all filters.". Hide filters shows only an arrow that points left into a bar, the side the pane goes to (`Glyph::OpenPane`, as Microsoft names it), with the tooltip "Hide filters", and runs View > Filters. While the pane is hidden, a toolbar button with the arrow that points right, out of the bar (`Glyph::ClosePane`), and the tooltip "Show filters" stands at the left margin of the search row, and runs View > Filters too. Hide filters and Show filters have no access key, as the copy icons have none. When the pane hides with the focus in it, the focus moves to the search box, because a toolbar button takes no focus. There is no close button and no pin: hiding keeps the pane one click away, and a pin needs a pane that hides by itself, which Gozler does not have. Under the header comes one group per filter. When the window is shorter than the pane, the controls under the header scroll and the header stays, because the lists need about 630 px at 100 %, and at 150 % a 1080p screen cannot hold them beside the rest of the window. Choose folder… spans the pane, with the same edges as its lists and boxes, so it is not part of the window's one button width. Groups combine with AND, and with the typed text; a group left at its first choice adds nothing.

Each group except Location produces one clause of query text, such as `ext:mp3;wav`. The window parses that text on its own, with gozler's parser, and adds its clauses to those of the typed text, as `gozler` does for `-r` and `-parent`. So the parser stays the only owner of the syntax, and nothing the person types changes what a filter means: an unclosed `"`, a trailing `|` or a typed `ext:` stays inside the typed text.

| Group | Control | Choices | Adds |
| --- | --- | --- | --- |
| Kind | list view, with an icon before each kind and one selection | All, Folders, Files, Audio, Archives, Documents, Executables, Pictures, Videos | `folder:`, `file:`, or `ext:` with the group's extensions, from one table in `core` |
| Location | list view, with Choose folder… below it | Everywhere, each indexed volume by its mount prefix, and the chosen folders | the scope, as `-path` |
| | check box | Only directly in this folder | the parent folder, as `-parent` |
| Date modified | combo box | Any time, Today, Yesterday, Last 7 days, Last 30 days, Last 365 days, Custom… | `dm:today`, …, `dm:2026-01-01..2026-03-31` |
| Size | combo box | Any size, the size words with their ranges, such as "Tiny (up to 10 KB)", Custom… | `size:empty`, …, `size:1mb..10mb` |
| Attributes | one combo box each for Hidden, System, Read-only, Link, Compressed and Encrypted | Any, Only, Exclude | `attrib:h` or `!attrib:h` |

- **Location** lists the volumes from status, and a volume that is not complete shows its phase after its name ("D:\ (indexing)"). Choose folder… is a button, not a list item, so moving through the list with the arrow keys never opens a dialog. It opens the Windows folder picker (`IFileOpenDialog` with `FOS_PICKFOLDERS`), and the chosen folder becomes a selected list item; Cancel keeps the previous choice. Each chosen folder, also from Search in this folder (`C12`), stays an item of its own, so the person can go back to an earlier one, and choosing a listed folder again selects it. The list keeps the 5 latest, as many as it shows rows, and the oldest leaves first. They are saved with the other filter choices, and Clear filters keeps them. A path wider than the list loses its middle, as Windows shortens a path, so its drive and last folder stay visible, and the list's info tip shows it whole.
- **Custom…** opens its dialog when the drop-down list closes on it, or on `Enter`. Moving onto it with the arrow keys while the list is closed opens nothing and keeps the previous clause, and Cancel in the dialog returns the combo box to the previous choice. Custom date has From and To, each with a check box: From alone adds `dm:>=FROM`, and To alone `dm:<=TO`. A new range opens with From on, at 7 days ago, and To off, because a range from today alone is what Today already gives. Custom size has Minimum and Maximum, either of which may be empty, and a unit. A custom range shows as its own item above Custom…, such as "At least 1 MB", so the combo box always shows the filter that is set.
- **No counts beside the choices,** because a count is one totals search per choice, which multiplies the cost of every keystroke.
- **Other attribute letters** are left to `attrib:` in the search box, because sixteen combo boxes would bury the six that people use.

### Inspector pane

A pane under the list, behind a splitter, shown and hidden with View > Inspector. Its height is saved, and the first height fits every field of one entry. Values are read-only edit controls, so the person can select and copy any part of them, as in File Explorer's Properties. Its sums come from the selection's files, so they need no request of their own.

- **One entry:** the icon, the name, then Folder, Size (the formatted size and the exact bytes, or "Unknown"; once when the two read the same, as "16 bytes"), Date modified (to the second, then its age in the muted text colour: "· 2 hours ago", or "· in 2 days" for a date to come) and Attributes (each as a word, such as "Hidden, System, Archive"). The icon is the shell's large icon of the entry, in a static icon control. There is no line for the full path, which is the folder and the name, for the type, which the name's extension and its icon show, or for the drive, which starts the folder. For a folder, Size shows the number of entries below it and the summed size of its files, from a totals search scoped to the folder, as `gozler -get-folder-size` does, so for a person who does not see every file they count the visible files only (see "Visibility"); while it runs, the field shows "Calculating…", and a new selection cancels it (`T7`).
- **Copy icons:** right after the name and right after the folder stands a toolbar of one button with the copy icon (`Glyph::Copy`, see "Look"), 16 px at 100 % in a square as high as a field row, 8 px after the text and centred on its line. It shows no border or background until the mouse is on it or it is pressed; it is always visible, so a first-time user finds it. It runs Copy name or Copy folder, so the status bar says what it copied, and its tooltip is that command's name. Tab does not reach it; Edit > Copy name, Copy path and Copy folder do the same. A text too long for its line ends in an ellipsis before the icon, so the icon stays visible.
- **Hide details** is a toolbar button at the right end of the heading's row, with only the arrow that points down, the way the pane goes (`Glyph::ChevronDown`), and the tooltip "Hide details". It runs View > Inspector. While the pane is hidden, Show details, with the arrow that points up (`Glyph::ChevronUp`) and the tooltip "Show details", stands at the right end of the status bar (see "Status bar") and runs View > Inspector too, so the pane is one click away without the menu. Neither has an access key, as the copy icons have none. When the pane hides with the focus in it, the focus moves to the search box, as when the filter pane hides.
- **Copy path** is a text button at the right end of the Date modified row, the row above the buttons, as wide as they are and with its right edge on theirs, because the full path has no line of its own. It runs Copy path. The date and its age end before it.
- **Explore, Open location and Open** stand at the right end of one entry's last row, Attributes, centred on it, in that order from the left, so Open, the one used most, is at the right edge and the buttons end the content, however tall the pane is. They share one width with Copy path and have the gap between related controls. The row's value ends before them, so no control overlaps them. The lowest pane shows every row.
- **Long values:** a value too long for its line ends in "…", and a path loses its middle, as Windows shortens one, while the value does not have the focus; with the focus, it holds its whole text, so the person selects what is there. The values work with the mouse only: Tab does not reach them and their labels have no access keys (owner ruling), and the Copy commands copy the name, the path and the folder.
- **Age and muted text:** the age is in the largest whole unit, rounded down, the same way for the past and the future: "just now" under a minute, then minutes, hours, days under 30 days, months of 30 days under a year, then years. It is computed from the current time when it shows, and again every minute while one entry shows. The muted text colour is the system gray text colour, and in high contrast the normal text colour, because there gray means unavailable. The age is a text of its own after the date, which keeps the normal colour, and ends in "…" when the line is too short.
- **Several entries:** how many files and how many folders, and the summed size of the selected files, with unknown sizes counted apart: "12.4 MB, 2 files unknown". Folder contents are not added, because a selected folder and a selected file below it would be counted twice. While the selection is read, both say "Reading the selection…".
- **Nothing selected:** the number of matches when available, and the hint "Select an item to see its details.", including before the first search, so the empty pane says what it is for.

### Export

File > Export results… opens the Windows save dialog (`IFileSaveDialog`) with the formats CSV, TSV and TXT. The export is one `Export` request with no limit, so it writes every match of the shown search, not only the rows the list reaches. The request thread writes each page through `gozler_core::output` as it arrives, so a CSV or TSV file holds the same bytes as `gozler -export-csv FILE` with `-size`, `-dm`, `-dc`, `-ext` and `-attributes` for the visible columns, and a TXT file holds the full paths, as `-export-txt` writes them. Name and Path are parts of the full path, so the full path stands for both. The thread sends a progress reply about twice a second, which the status bar's middle group shows. A failure to create or write the file is the export's own outcome, apart from the daemon failures, and a cancelled or failed export deletes its partial file.

### Settings dialog

File > Settings… opens a dialog with a page list on the left and the page on the right, as Everything's Options dialog does: General, Search, Context menu, Keyboard, File operations, Performance, Drives and service, Include and Visibility. Each page is a child-dialog template, made when it first shows, so the dialog opens without making every page's controls; the dialog's copy of the settings holds the values of a page not made yet. Its push buttons get the one width of "Look" from every page's captions, also those of a page not made yet. Each page has its name as a heading, its controls in sections and, on the pages with settings, Reset at its bottom right, which sets that page's controls to their defaults without applying them. **Reset all,** at the bottom left on the row of OK, Cancel and Apply, starts Gozler again as a new installation, as TinyTorrent2's Reset all does, because each page's Reset already restores that page. It asks first, with Cancel as the default, because it cannot be undone. Then Gozler exits as File > Exit does, `run` deletes `ui.json` and the sign-in entry, and the window opens again, so every setting, the match controls, the sort, the columns, the panes, the filters and the one-time notices are a new installation's. The Include settings and the list on the Visibility page stay, because the service keeps them and changing them asks for administrator rights. A section has a heading, its icon and its title in the page heading's semibold font, with its controls indented below it, and an etched line (`SS_ETCHEDHORZ`) separates two sections of a page. Windows draws every part, so no group box is drawn with an icon in its caption. Each section holds more than one control, as Windows' guidelines ask of a group.

- **A search box across the top of the dialog**, with Clear at its right and the page list and the page in a row below it, keeps the pages whose name or captions hold every word typed, each at the start of a word, ignoring case, so a few letters find a setting without matching inside words: "upd" finds General, by "Check for updates", and "dates" finds nothing. The shown page stays while it is kept; otherwise the first kept page shows, and when none is kept, the page area says "No setting matches the search." A setting that Apply refuses shows its page, and empties the search when the search left that page out. On every page, a caption that holds a word of the search shows in bold, because the page list says only which page holds the setting; a page's heading and its sections' titles keep their own font.

- **The settings change what Gozler does, never how it looks.** The look follows File Explorer and Windows ("Look"), because the owner does not want appearance settings. Show sizes in is the owner's exception: File Explorer shows KB and Total Commander bytes, and people compare sizes with both. Show memory use in the status bar is another, which TinyTorrent2 has too: the figures cost a status request every 2 seconds, so the person turns them on. An option of Everything comes in only when it changes what Gozler finds or does, and Gozler does not already decide it (see "Not planned").
- **General, Search, Context menu, Keyboard, File operations and Performance are the person's settings,** saved in their `ui.json`. Drives and service shows the service's state, which one service keeps for every user of the PC, so anyone can read it without administrator permission. Include and Visibility are the service's settings: Include decides what the index holds, and Visibility who sees every file, for every user, so anyone can read them and only an administrator saves them (see "The Include page" and "The Visibility page"). Dates created and Rebuild index, on the Drives and service page, also need an administrator.
- The dialog edits its own copy of the settings (window rule 1). While it runs, Windows disables the main window and the accelerators do not run, but replies still reach the main window, which handles them as usual. Apply and OK check the input (see "Dialogs"), then give the copy to the model and save it. `core::settings::SettingsDraft` decides what Apply's outcome leaves on the pages: a shortcut that Windows refuses applies nothing, a registry setting whose write failed shows its earlier value again, and the service's settings (the Include settings and the list on Visibility) stay edited, with the administrator shield on OK and Apply, until they are saved. It also holds the text of a number box that its range refuses, so whether Apply is enabled is one rule of the copy. The Drives and service page tells the model when it is shown or hidden, so the main window's status poll serves it, and the page keeps its own 2-second timer only to show the model's latest status.

| Page | Group | Setting | Default |
| --- | --- | --- | --- |
| General | Language and display | Language | Windows default |
| | | Show icons in menus, buttons and lists | on |
| | | Show memory use in the status bar | off, as in TinyTorrent2 |
| | Formats | Show sizes in: Bytes, KB or MB | KB, as File Explorer |
| | | Short date format: Date/Time, Date Only or Relative Date | Date/Time |
| | Starting and closing | Start Gozler when I sign in to Windows | off |
| | | Check for updates when Gozler starts | off |
| | | Keep Gozler running in the notification area when I close its window | on, as in Everything |
| | | Allow only one Gozler window at a time | on |
| Search | Search box | Search as you type | on, `Enter` acts on the result; off, `Enter` runs the search |
| | | Show every file and folder when the search is empty | on, as in Everything; off, the empty search shows nothing (`S6`) |
| | | Select the text when I click the box | on |
| | | Select the text when the window is activated | off |
| | Results | Enter, double-click and Open | run Context menu's open command; without one, show the result in Commander |
| Context menu | — | Items: the row menu's items, with Open as Open (folders) and Open (files) | — |
| | the selected item's name | Show, Show only when Shift is down, Don't show | Show |
| | Open with | Folders, Files, File location: each an edit box with Browse…; the Use Total Commander link | empty |
| Keyboard | From any program | Separate Show Search and Show Commander boxes capture the keys pressed, the Windows key included; each Clear removes its shortcut, as `Backspace` in that box does | none |
| | Commands | Containing; the list of commands; Shortcuts for the selected command, with Add…, Edit… and Remove | the defaults of "Commands" |
| File operations | Copying | Existing files: Update changed files, Update newer files, Keep both, Skip existing files; Transfers per device: 1–3 | Update changed files; 1 |
| Performance | — | Speed settings: Automatic, Parallel copy, Ring copy, Ring for large files, many small files at once, Windows copy function, unbuffered from 256 MiB, or Custom | Automatic |
| | Copying, Ring copy, Deleting | the chosen preset's values | Automatic's |
| Drives and service | Drives | each fixed NTFS or ReFS drive, with its state and its entry count | — |
| | What the index holds | names, sizes, folder sizes, dates modified and attributes: always held, because the search, the filters and the Size column need them | — |
| | | Dates created: the Date created column and `dc:` | off |
| | Service | the service's state and memory, its working set, the figure the status bar's memory group shows, the service's program and the gozlerd.exe beside the window, Use local and the failure's fix, and Report | — |
| Include | — | the preview line: entries indexed now and after OK | — |
| | Folders | Folders Gozler indexes: each indexed folder with the folders left out inside it, and Modify… | every drive, nothing left out |
| | | Include new fixed drives automatically | on |
| | Names | Names that are not indexed: name patterns separated by `;` | empty |
| | | Exclude hidden files and folders | off |
| | | Exclude system files and folders | off |
| Visibility | Users and groups who see every file | the list, with Add… and Remove | the list the service keeps; empty until the setup or an administrator stores one |
| | | Every user sees every file | on only when the list holds Everyone |

"Select the text when the window is activated" acts only when the person switches to Gozler from another program; closing one of Gozler's own dialogs leaves the focus where it was.

### The General page

The page reads in three groups: Language and display holds Language, Show icons and Show memory use; Formats holds Show sizes in and Short date format; Starting and closing holds the four check boxes that decide when Gozler starts and stops.

Compact sizes throughout the window use whole bytes and KB, then up to three
significant digits from MB upward. Extra digits are dropped rather than rounded;
precision is automatic, with no separate setting. The Size column keeps the
chosen unit and its rounding rules below.

- **Show sizes in** sets how the Size column shows sizes, folder sizes included: KB, rounded up, as File Explorer shows them; MB with two decimals, rounded up to the hundredth, so a file that is not empty never shows 0.00 MB; or Bytes, the exact count, as Total Commander shows it. Each uses the regional number format. The status bar, the inspector and Export keep their own formats. Applying a change fits Search's columns again and sizes both panels' Size columns for the new unit. Size's minimum includes the singular unit and the largest supported size, even before any rows arrive, so larger values in later rows cannot be clipped by a fill.
- **Short date format** applies to Search's Date modified and Date created columns and both panels' Date modified columns. Every cell shows the Windows short date and time when it fits its column, measured in the list's font with its cell padding. When it does not fit, Date/Time keeps it, Date Only falls back to the Windows short date, and Relative Date uses one short label: Now for less than a minute in either direction; Yesterday or Tomorrow for an adjacent local calendar day; otherwise, on the same local day, a whole number of minutes or hours, such as "5 min ago", "In 5 min", "3 hr ago" or "In 3 hr". Other days fall back to the Windows short date. Column sizing measures the full date and time, so a fallback never holds a column at its shorter width. Relative labels refresh once a minute while the window is on screen, without reading disk or changing column widths. The inspector and Export keep their exact timestamps, and sorting always compares timestamps. The saved `date_format` defaults to Date/Time when missing or damaged.
- **Keep Gozler running in the notification area** decides what Close does and whether the shortcut works after the window closes (see "Window lifetime").
- **Start at sign-in** writes the `gozler` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, `"<this gozler-ui.exe>" --sign-in`, which starts Gozler as "Window lifetime" says. The check box shows on only when that value exists and Windows has not turned it off: Task Manager's Startup apps page and Windows Settings turn an entry off without removing it, by a `gozler` value under `...\Explorer\StartupApproved\Run` whose first byte is odd. That format is not documented; Task Manager writes 2 for on and 3 for off. Turning the check box on deletes that value, and off deletes both. The registry is the only copy, so the check box and Task Manager never disagree. Uninstalling Gozler removes both values.
- **Allow only one Gozler window at a time** decides what a second start does (see "Window lifetime").
- **Show icons in menus, buttons and lists** shows or hides every icon beside a text (see "Look"), at once, Settings included; off, the window looks as it did before it had icons: no icon column in the menus, buttons and the Kind list as narrow as their text, and Settings sections' titles and status bar totals without their icon. The icon-only buttons keep their icons, because they have no text. It is a setting because the icons are new and might not render well on every machine.
- **Show memory use in the status bar** shows the status bar's memory group (see "Status bar"). Off, the default, as in TinyTorrent2, the group has no part, the right group takes its width, and the app neither asks for status nor reads its own memory for it. Apply shows the group at once, with the app's figure read then; the service's arrives with the status reply that Apply asks for.
- **Check for updates** asks `https://api.github.com/repos/SynoSoftware/goz/releases/latest` with WinHTTP, from its own thread, at most once a day when Gozler starts. A newer tag shows in the status bar's right group, and a click runs Update; Gozler downloads and installs a release only when the person clicks Update or Install and then accepts the question (see "Service fixes"). A failed request is ignored, because no update is worth a message. On, Gozler also asks once when it finds the service older than the window, so the service's fix button can read Downgrade before the click (see "Daemon failures"). Off, Gozler makes no network request but the one after a click on Update, Downgrade or Install.

### The Context menu page

The page sets a row's context menu, as Everything's Context Menu page does: a list of the menu's items in the menu's order, under it whether the menu shows the selected item, and under that the programs that open folders and files. Open is two items, Open (folders) and Open (files), because a person may want one in the menu and not the other.

- **Show this item, Show this item when Shift is down, Do not show this item** decide whether the row menu shows the item. Shift is read as the menu opens, by a right-click or by Shift+F10 and the Menu key, as File Explorer's extended menu does. Open follows the first selected row's kind: a folder takes Open (folders)'s setting, a file Open (files)'s. A group whose items are all hidden leaves no separator, and with every item hidden the menu does not open. A hidden item still runs from its shortcut, the menu bar and the inspector, because the setting is about the menu only. The settings are saved in `ui.json` under `context_menu`, and a missing or damaged value shows the item.
- **Open with** holds the three open commands, Open folders, Open files and Open file location, each in an edit box with Browse…; they do not change with the selected item. They serve Search only (see Searching): Open folders runs for Open (`C1`) on a folder, Open files for Open on a file, and Open file location for `C2`. A person who browses with another file manager, such as Total Commander, sets them, so a result opens there. An empty Open folders or Open files command shows the result in Commander. An empty Open file location command opens the folder in File Explorer with the entry selected. Commander mode never runs them. The empty boxes and the note below them explain these defaults. In a command, `%1` is the entry's full path, as in Everything and Windows' own commands, so a command copied from inside Everything's `$exec(...)` works as it is. With several entries selected, the command runs once for each. Explore (`C26`) ignores the commands and always uses File Explorer. The commands keep the keys `open_folders`, `open_files` and `open_file_location` in `ui.json`. A line under the boxes says what `%1` is and that only Search runs the commands, and each empty box shows what it does, and the Use Total Commander link ends that line at the Browse… buttons' right edge; the line wraps in the width to the link's left.
- **Browse…** opens the Windows file dialog for programs and writes the chosen program in quotes, followed by ` "%1"`. The three buttons have no access key, because one letter cannot name three buttons; Tab reaches them.
- **Use Total Commander** fills Open folders and Open files with `"<path>" /O /A /P=L /L="%1"`, because nobody remembers Total Commander's options, and leaves Open file location alone. It finds Total Commander in the index with one search, `file: totalcmd*.exe` sorted newest first, and takes `TOTALCMD64.EXE`, or `TOTALCMD.EXE` when there is no 64-bit one; of several, it takes the newest by date modified, because an older copy is usually a backup. The boxes show the path, so the person sees which one it took. When both boxes already hold that command, a balloon on the link says "Total Commander is already set.", because the click would otherwise seem to do nothing. When it finds none, a balloon on the link says so and the commands keep their text; while the service cannot search, the balloon gives the service's message. The dialog stays responsive while the request runs, and closing it cancels the request. Like every change on the page, the filled commands apply with OK or Apply.
- **Gozler does not change File Explorer.** Earlier versions could add "Search with Gozler" to File Explorer's menu on folders; Gozler now removes that entry when it starts, so no item it no longer serves stays there, and uninstalling removes it too.

### The Keyboard page

The page has two parts: the separate shortcuts that show Search and Commander from any program, and the shortcuts of the window's commands. The existing `show_shortcut` setting now means Show Search, preserving the person's key; `commander_shortcut` is separate and starts empty. `app::main_window::shortcuts` owns registration, replacement and dispatch. Each registration has an ID; both native hotkeys and the shared Windows-key hook deliver that ID through `WM_HOTKEY`. The shortcut owner maps it to a mode, and every show command calls `MainWindow::show_mode` before bringing the window forward. The mode left open never decides a shortcut's destination.

- **Each shortcut to show Search or Commander needs the Windows key, or two of `Ctrl`, `Alt` and `Shift`,** because a shortcut with one of them, such as `Ctrl+F`, would take that key from every program. The box turns such a combination into `Ctrl+Alt` and the key, as the Shortcut key field of a Windows shortcut does. On keyboards with AltGr, `Ctrl+Alt` with some keys types a character, such as `@` with `Ctrl+Alt+Q` on a German keyboard; a person whose character stops working chooses another shortcut.
- **The box takes the Windows key as it is pressed,** such as `Win+S`, which the Windows hotkey control cannot enter, so the page needs no Windows key check box. While the box has the focus, a low-level keyboard hook on its own thread (`win::shortcut::Capture`) gives each key pressed to Settings instead of to the box or the shell, so `Win+E` reaches the box and does not open File Explorer. The modifiers pass, and so do `Tab` and `Esc`, because Windows switches programs and opens Task Manager with them, and `Enter` alone, which presses OK. The box shows only what Settings writes in it. Each empty box says, in gray, to press the keys together. Neither says Backspace, which also removes the shortcut, because the Clear button beside each box does that in sight. Print Screen passes through with any modifier, so a screenshot works while the box has the focus, and no shortcut uses it.
- **A shortcut with `Ctrl`, `Alt` and `Shift` only** is a `RegisterHotKey` on the main window. When another program holds it, the dialog says so, and Gozler never takes such a shortcut from another program.
- **Windows-key shortcuts share one low-level keyboard hook** (`WH_KEYBOARD_LL`), because Explorer registers most Windows-key combinations, so `RegisterHotKey` fails for them. `win::shortcut::Registry` shares the hook between registrations; its last registration dropping stops the hook. It runs on its own thread with its own message loop, because Windows removes a low-level hook whose callback does not return within `LowLevelHooksTimeout`, and the window's thread can be inside a modal loop. The callback matches the active bindings and posts `WM_HOTKEY` with the chosen registration's ID. It owns each swallowed physical key through repeats and release, so changing modifiers while holding a key cannot switch bindings or lose its release. Before the Windows key comes up, it sends one unassigned key (`0xE8`), so the shell sees a combination and does not open Start. `Win+L` always locks the computer, so the box ignores it. While a program that runs as administrator is in front, its keys reach Windows, not Gozler.
- **Windows' own shortcut is disabled in Explorer's registry setting** for each registered shortcut that uses a letter or digit with the Windows key. The Settings worker adds those characters to `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced\DisabledHotkeys`, preserving the other characters and the string's type. This setting disables the key across modifiers: `S` can disable both Windows Search's `Win+S` and screen capture's `Win+Shift+S`. Some Windows shortcuts ignore it; keys other than letters and digits retain only the live hook. The line under the boxes says which Windows shortcut it replaces when known, that other shortcuts with the same key may stop working, and that signing out applies it. Gozler never restarts Explorer. The setting does not launch Gozler when it is not running.
- **Gozler restores only its own registry change** when either saved shortcut changes, is cleared, or Gozler is uninstalled. `WindowsShortcutBackup` under `HKCU\Software\Gozler` saves the added keys and prior string and type, including an absent value, before changing Explorer. The earlier single-key backup also loads. A character already present is left alone; later edits to other characters survive restoration. The backup remains until restoration succeeds, so the next first window can recover after a crash. The existing Settings writer serializes changes off the window thread; failures appear in the window and its error log. The registry override follows the saved shortcut across Exit and sign-out, so Explorer can read it at sign-in. Uninstall calls `gozler-ui --unregister-shortcut`, which uses the same restoration. Explorer may retain its old setting until sign-out.
- **After the window closes,** the shortcut works only while Gozler keeps running in the notification area. The boxes' tooltip says so, because it concerns a setting on the General page and is help, not a warning.
- **The line under the boxes** shows the Windows key warning, which a person must see before they apply, and is empty otherwise. Keys that the other box holds move to the box being edited, because one shortcut cannot show both modes, and a balloon on the other box says where they went, as Total Commander's key editor clears the command that had the key. It keeps a fixed two lines, so the Commands section never moves while the person types, as Windows' guidelines ask of a stable layout; each catalogue's longest warning fits them.
- **The command list** shows every command that is one item of the menu bar or the context menu, as its menu and its name, such as "Edit > Find", with its shortcuts. A submenu of choices, such as Columns, Sort by or Language, the match controls and Clear filters take no command shortcut. The first rows are "Notification area > Show Search" and "Notification area > Show Commander", each with its registered shortcut, so the list shows what works in the system: it changes when Apply registers the box's shortcut, and it is empty while Windows refuses the saved one or another Gozler window holds it. The two boxes above set those shortcuts, so Add…, Edit… and Remove are disabled on those rows. "Containing" keeps the commands whose name or shortcut contains the text, so `ctrl+e` finds the command that has it. Add… and Edit… open a small dialog, titled "Add shortcut" or "Edit shortcut", with one hotkey control labelled with the command, such as "Shortcut for Edit > Find:", and a command can have several shortcuts, as Find has `Ctrl+F` and `Ctrl+E`.
- **A command shortcut needs `Ctrl`, `Alt` or a function key.** `Alt` with a letter or a digit is refused, because those keys open the menus and reach the window's controls ("Words"), and a key with neither `Ctrl` nor `Alt` is refused unless it is a function key, because the search box needs it for typing. A shortcut that another command has moves to the command being edited, and a balloon on its shortcut list names the command it came from, so two commands never share one and the person never has to remove it there first. Keys that belong to a control keep their meaning and are refused: `Tab`, `Enter`, `Esc`, `Shift+F10`, `Alt+F4`, `Ctrl+V`, `Ctrl+X`, `Ctrl+Z`, and `Ctrl` with an arrow, `Home`, `End`, `Page Up`, `Page Down`, `Backspace` or `Delete`.
- **Commander mode keeps some keys** (see "Keys"): `F3` to `F9`, `Alt+F1`, `Alt+F2`, `Alt+F7`, `Ctrl+F`, `Ctrl+L` and `Ctrl+R`, and `Alt` with a letter, a digit, a comma or a period. Its accelerator table leaves out a person's shortcut on one of these keys, so such a shortcut works in Search only. The shortcut dialog says so in a balloon when the person picks one of these keys, and OK still accepts it; the page shows no standing note, because the limit matters only to a person who picks such a key.
- **The menus show the person's shortcuts,** so what a menu says is always what the key does. Reset gives every command its default keys again.

### The File operations page

This page sets the saved copy policy and the 1–3 transfers-per-device limit.
The Copy/Move dialogs share that policy. Committing remembers it; Cancel
leaves it unchanged. Paste and Drop capture it on acceptance and display it
with their progress. Later Settings changes affect future requests.

### The Performance page

The page sets the speed settings that every copy, move and delete starts with. "Speed settings", under "The copy, move and delete engine", says what each value does to the engine.

- **Speed settings** is a drop-down list of presets, in this order: Automatic, the default; Parallel copy; Ring copy; Ring for large files, many small files at once; Windows copy function, unbuffered from 256 MiB; and Custom. One sentence under the list says what the chosen preset does, such as "Copies many files at once with Windows' copy function, unbuffered for large files." for Parallel copy. No preset is File Explorer's copy, which copies one file at a time.
- **The chosen preset's values** are below the list, in three sections. Copying has the size where large files start, the copy method for large files between SSDs and when a hard disk is at either end, and how many files are copied at once from an SSD, a USB drive, a hard disk and a share. Ring copy has the block size, the blocks at once and the buffer, between SSDs and with a hard disk. Deleting has how many files are deleted at once, or one per logical processor. A choice between methods is a drop-down list, and a number is a number box with its unit.
- **Changing a value selects Custom, and choosing a preset sets every value.** Custom keeps the person's values while another preset is chosen, so choosing Custom again brings them back. A number outside its range is refused with a message that gives the range, as the other pages refuse a bad value; the delete count is not checked while one per logical processor is on, because its box is off. Turning one per logical processor off keeps the count the box showed, at most the top of its range (`Count::MAX`) on a computer with more logical processors. The engine's setting types own every range, so a damaged value in `ui.json` falls back to the default and never reaches an operation (`core::speed`).

### The Drives and service page

- The drive list shows each fixed NTFS or ReFS drive by its mount prefix and label, such as "C:\ Windows", with its state and its entry count. A live drive whose size and date pass failed or stopped reads "Live, some sizes and dates unknown". Removable drives, network drives and other filesystems are not in the list, because the daemon does not index them (see "Index coherence").
- The page refreshes its states and counts every 2 seconds while it is shown. A refresh sets only the texts, cells and buttons that changed, because setting a control redraws it, and the memory and the entry counts change at nearly every refresh, so a page that set everything would flash. The drive list keeps its selection while its rows stay the same.
- **Report** opens the service's report, the text that `gozler --status` prints, in English as `gozler` prints it, in a read-only box in a monospaced font, because the report aligns its values in a column. Copy selects the whole report and copies it, so a person can paste it into a bug report, and the selection stays visible while Copy has the focus. The button is enabled while the service answers, because only then is there a report. The report shows the service's state at the moment it opened, from the latest status, which the page refreshes every 2 seconds; it does not change while it is open, so the copied text is the text the person read. Its place is the bottom right of the Service group, with the fix's button to its left and Use local to the left of that. Both always show, so nothing moves in the row: a button with nothing to offer is disabled, as Windows' Services dialog disables Start while a service runs. Without a fix, the fix's button reads Start, with no shield and no tooltip, and Use local keeps its caption without a tooltip.
- **Rebuild** has the service read every indexed drive again and build a new index, for an index the person no longer trusts; the service keeps itself current without it. It acts at once, so it shows the administrator shield, and Windows asks for permission as the Include page's save does: the window starts `gozler-ui.exe --rebuild-index` with the `runas` verb, which writes the rebuild token (see "Include settings") and exits with 0 or the Windows error code, and the window then sends `SettingsSaved`. The dialog waits for the answer, with OK, Cancel, Apply, Reset all and Rebuild disabled, and a failure says "Couldn't rebuild the index." with Windows' words. While a drive rebuilds it reads "Indexing", and searches give incomplete results. The button is enabled while the service answers, at the bottom left of the Service group, apart from the fixes and Report, and its tooltip says what it does, because its caption cannot.
- While the service does not answer, the drive list's empty area says "Gozler cannot read the drives while the service does not answer.", as File Explorer says that a folder is empty (`LVN_GETEMPTYMARKUP`), and the Service group's buttons offer the failure's fixes, the same as the list's.
- **The Service group says which programs run.** Under the service's state, two label and value lines show "Program:", the gozlerd.exe that the service's configuration names, and "Beside this app:", the gozlerd.exe beside the window. Each has its path on one line and its build on the next, such as "Version 0.3.0, modified 20/10/2026 09:14:05". The build gives the date modified with seconds, because `--version` prints the Cargo version alone, so two builds print the same version. A gozlerd.exe beside the window that holds the service program's bytes reads "Same build as the service's program", and one that is not this window's pair shows only its date modified, because it is never run. A path ends in a path ellipsis, with the whole path in its tooltip, so a long path never widens the page or wraps. The lines are empty while the window has not read the files yet, and after a failure other than a service older than the window, because then the window reads no files. A developer who runs a new window against an older debug service sees the two builds differ and switches with Use local, one click and Windows' permission prompt.
- When a paired window finds a gozlerd.exe that is not its pair (see "Daemon failures"), a line under the programs says "Use local isn't available because the gozlerd.exe beside this app wasn't built with it."
- **Dates created**, under What the index holds, has the service keep each entry's date created, for the Date created column and `dc:` (see "Include settings"). It is off by default, because it adds 8 bytes an entry to the service's memory, which the Service group shows. It is a field of the Include settings, so OK and Apply save it with the administrator shield, as they save the Include page, and that page's Reset keeps it. It is disabled while the service gives no settings. It takes over Everything's Index date created, as Rebuild takes over Force Rebuild.
- Folder sizes cost about 8 bytes a folder, 2.2 MB for the 270,137 folders on the owner's machine against 196 MB for the whole index, so they are always held and need no setting.
- A drive that the Include settings leave out reads "Left out", with no entry count. It is not "Not indexed", because that names a drive the service failed to index.

### The Include page

The page follows Windows' Indexing Options, which every Windows user can meet: a list of the indexed folders with what is left out inside each, a Modify… button that opens a folder tree with check boxes, and a count of what is indexed. "Include settings" has the rules it edits.

Settings rebuilds the folder summary only when its draft, connected mount
prefixes, accepted labels or number format changes. Preview invalidation also
follows indexed entry totals and its current count reply. Unrelated replies and
waiting timers neither clone the Include rules nor rebuild native rows. The
list keeps its actual previous rows through replacement, including an empty
replacement, so native row removal remains correct.

- **The top of the page says** "These settings apply to everyone on this PC. You'll need administrator permission to change them." While they differ from what the service applies, OK and Apply show the administrator shield.
- **The preview line** says what OK would change before it is pressed: "Indexed now: 2,273,581 entries. After OK: about 1,871,204." The service counts it with `CountIncluded`, 300 ms after the last change, through the window's worker, and a newer change cancels the older request (window rule 4). Each change, and closing Settings, also forgets the count that arrived (`Model::forget_included_count`), so the preview shows "…" until the count of the settings it shows arrives, and a late count never shows in the next Settings session. The preview reads the connected drives from the status, without asking Windows for their labels, so an edit costs no Win32 call per drive; the page reads the labels when it fills and after Modify. The dialog formats the preview and the folder summary in its own language, as its captions, until it closes. Folders checked again add ", and the folders you checked again, counted once they are indexed", because the index does not hold them yet. It counts the whole index, not the entries the person may see, because the change reaches every user, so gozlerd gives it only to a person who sees every file (see "Visibility"). For a filtered user, whom the status names as one, the window asks for no count, and the line says "Indexed now: 2,273,581 entries. Only users who see every file see the count after OK.", because the service's refusal reads as a failed search and the line has room for two lines only.
- **Folders Gozler indexes** is Windows' "Index these locations" list: one row for each indexed folder, a checked drive or a checked folder below an unchecked one, with the folders left out inside it. Its columns are Folder (`C:\`), Left out, the count ("2 folders", or "None"), and Folders, those folders below the drive's root (`ProgramData; Windows`). The count has its own column, because the Folders column cuts a long list off. A row's tooltip lists those folders by their full paths; a list view asks for a tooltip per row, not per column, so it shows over the whole row. The list is read-only, as in Windows; Modify… changes it, and so does a double-click on a row, because Indexed folders already shows every folder in full. The index holds nothing for a folder left out, so the list shows no sizes.
- **Modify… opens Indexed folders,** Windows' "Indexed Locations" dialog: a tree of each drive with its label, opened one level deep so the folders to uncheck are in view, the hint "Uncheck a folder to leave it out of the index. To index only some folders, uncheck the drive and check those folders.", and the same summary, where selecting a row opens the tree to that folder. Its OK puts the choices on the page for the Settings dialog's OK or Apply.
- **The tree lists the folders on the drive, not in the index,** so a folder that is left out can be checked again. It lists a folder's subfolders, the drives' roots included, on a thread of its own when the folder first opens, because a folder such as `C:\Windows\WinSxS` takes most of a second to list and a sleeping disk takes seconds to wake, and adds them when they arrive. Each item that the tree adds, removes or checks costs Windows an accessibility event, up to a few tenths of a millisecond, so the 24,408 folders of WinSxS take seconds. The dialog therefore adds them, and sets the check boxes after a click, 15 ms at a time between the person's input; a folder opens with its first subfolders, and the rest follow. The newest work goes first, so a folder the person opens, or a click, does not wait behind a large folder, and while a click's check boxes are set, the rows in view get theirs first in each slice, so the person never sees or clicks an old one. A summary row whose folder is not listed yet opens the tree once the folders on the way are in it. OK and Cancel close the dialog at once, and the window removes the tree's items afterwards the same way. As in File Explorer's folder tree, the tree has no horizontal scroll bar (`TVS_NOHSCROLL`, with `TVS_EX_AUTOHSCROLL`), because a tree with the bar measures every name that opening a folder shows: 1.4 s for WinSxS. It does not scroll sideways under the pointer, so a folder nested deeper than the tree is wide shows cut off at the right; Space still sets the selected folder's check box. It does not open a junction or a symbolic link, because they lead elsewhere. A partial check box means that some folders below it differ.
- **Clicking a check box sets the folder and everything below it,** and drops the choices below it, as Windows' folder trees do: a checked folder becomes unchecked, and an unchecked or partial one checked. The dialog sets every check box from the settings after each click, so the control's own partial step never shows.
- **A drive that is not connected keeps its row** in the tree and the summary, so a choice for a docking drive is not lost.
- **Saving asks Windows for permission.** OK or Apply writes the settings to a file in the temporary folder and starts `gozler-ui.exe --save-include <file>` with the `runas` verb, so Windows shows its permission prompt. That copy writes the registry value, and the Visibility list when it changed too, and exits with 0, or with the Windows error code, which the window shows in a message. A thread of its own waits for the person's answer and for that copy, so the window keeps working; the dialog waits for the answer, with OK, Cancel, Apply and Reset all disabled. Once the settings are saved, the window sends `SettingsSaved` itself, so the copy never waits for the service, and after OK the dialog closes. A person who says no to the prompt leaves the dialog open with the settings unsaved; the person's other settings apply as usual.
- **While the service gives no status,** the page shows the service's message in the preview line and its controls are disabled, because the window knows neither the settings nor the drives.

### The Visibility page

The page shows and changes the list of users and groups who see every file; "Visibility" has the rules it edits. It reads the list from the registry when Settings opens, and anyone may read it.

- **The top of the page says** that the settings apply to everyone on this PC and need administrator permission, in the Include page's words. While the list differs from the stored one, OK and Apply show the administrator shield.
- **A line below it says what the person sees,** from the status, which the page asks for when it shows: "You see every file, with folder sizes.", "You see only the files in folders you can open, and no folder sizes.", or, when gozlerd could not tell who they are, "The service can't tell who you are, so it doesn't run your searches." While the service gives no status, or its last status is out of date because a later request failed, the line shows the service's message, as the Include page's preview line does. It says what the service decides now, not what the edited list would decide, so it changes after OK or Apply.
- **Users and groups who see every file** is a section with the people icon, the check box and the list. The list has one column and no header, and one row for each entry, by its account name as Windows gives it (`DOMAIN\name`), by its SID string while Windows has not answered, by the line itself when a hand edit stored a line that is not a SID, or, for a SID that Windows knows no account for, such as a deleted account's, "Account unknown (S-1-5-21-…)", as Windows' permission editor shows it. The names come from a thread of its own, because asking can wait on a domain controller. Add… and Remove stand to its right.
- **Every user sees every file** is checked when the list holds Everyone. Checking it adds Everyone and clearing it removes Everyone, so it stores nothing of its own, and Everyone shows as a row of the list too.
- **Add… opens Windows' Select Users or Groups dialog,** the one Windows' own permission editors open, for the users and groups of this PC and of its domain. It adds those the person chose that the list does not hold. If the dialog cannot open, a message box says "Couldn't open Select Users or Groups." with Windows' words. **Remove** takes the selected row off the list and selects the row that takes its place; it is disabled while no row is selected. When Remove takes the last row, the focus moves to Add…, because a disabled button keeps the focus but takes no key.
- **An empty list says** "Each user sees only the files in folders they can open." in the list's area.
- **A note below the list says what the list means:** users who are not on it see only the files in folders they can open, and no folder sizes; and being in the Administrators group counts only while Gozler runs as administrator, so a person adds their own account to see every file. The window runs without elevation, and Windows then gives an administrator's token the Administrators group for deny only (see "Visibility"), so an administrator who adds that group would otherwise see no change.
- **A list that cannot be read changes nothing.** The check box, the list and Add… are disabled and the list's area says "Couldn't read the list." with Windows' words, because saving a list that was never read would replace the stored one.
- **Only a real change is saved.** An account picked again, or Everyone turned on and off, leaves the list as it was, so OK and Apply show no shield and ask Windows for nothing. The window compares two lists as sets, ignoring case, because the service reads the list as a set: a list that is only reordered, or whose SID strings differ only in letter case, counts as unchanged.
- **Saving asks Windows for permission once for all the service's settings.** OK or Apply starts the same `gozler-ui.exe --save-include <file>` copy, with the `runas` verb, as the Include page does, and that copy writes each service setting that changed, so a change to both pages asks once. A failure shows "Couldn't save the settings for everyone on this PC." with Windows' words. The service reads the list for each search, so a change to the list alone sends no `SettingsSaved`. Once the list is saved, the main window's Search list searches again, as F5 does, also while Commander mode shows, because it keeps its results until it shows again; in Commander mode each panel reads its folder again, and in Search the panels read theirs when they next show. The window also asks for the status, so the line at the top of the page shows what the person sees now.
- **The page has no Reset, and Reset all leaves the list alone,** because the service keeps it for every user and changing it asks for administrator rights.

### Dialogs

Every dialog follows the same rules, so the person never has to look for a button:

- The buttons are at the bottom right, in the Windows order: OK, Cancel, then Apply where there is one. About and Search syntax have one button, Close; Service report has Copy and Close.
- OK is the default button and `Enter` presses it; `Esc` is Cancel.
- Apply is disabled until a value changes. OK applies and closes. Cancel closes and changes nothing that Apply has not already applied.
- Focus starts on the first field, and Tab moves through the fields in reading order.
- A dialog checks its input when OK or Apply is pressed. An invalid field shows a balloon tip that says what to enter, focus moves to it, and the dialog stays open.

Custom date needs at least one check box on, and From on or before To; with neither on, OK stays disabled. Custom size takes numbers with the Windows decimal separator, at least one of them, and Minimum no larger than Maximum. The open commands take any text, so a command that cannot start shows its message when it is used ("Feedback"). The shortcuts follow "The Keyboard page", and each refusal names what to choose instead.

The window also does what a Windows user expects without being told: Shift+F10 and the Menu key open the context menu at the focused row, `Alt+Enter` opens Properties, as in File Explorer. Every task works with the mouse alone, and every task but selecting part of an inspector value works with the keyboard alone.

**Owner ruling: in the main window, Tab reaches only the search box and the results.** Tab after typing a search therefore reaches the results at once, as in Everything. The owner prefers shortcuts to Tab stops: the status bar's Cancel is not one, because File > Cancel and `Esc` in the results stop the job it shows, and a toolbar button never is one. Every other control is for the mouse, its access key, a menu command or a shortcut, and the inspector's values are for the mouse only (see "Inspector pane"). Those controls keep their tab stop only so a label's access key reaches the control after it; from such a control, Tab goes to the results and Shift+Tab to the search box (`commands::tab_stop`).

### Where settings are saved

`%LOCALAPPDATA%\Gozler\ui.json`, found through the Windows known-folder API, is written by the window alone: to a temporary file, then renamed over the old one. It is written a second after the person last changed the window's state, when the window hides, at Exit, and when Settings applies, so a change survives a window that ends without Exit, such as one ended by a rebuild, and a splitter drag writes it once. `MainWindow::note_saved_state` decides when: it runs after each change that can touch the saved state, the panels' folders, sorts and active panel and the Viewer's size, place and Wrap long lines included, and starts the timer only when that state changed. A save writes over the file only the settings that changed since this window's last save, a setting being a key of `settings` or `window`, and keeps the others, so a setting that another window saved meanwhile stays; nothing is written when nothing changed, and a save that failed is not tried again until the next change. The placement is recorded on every `WM_WINDOWPOSCHANGED` once the window shows, so maximize, a snap or `Win+Arrow` is saved, and a window that starts in the notification area keeps the saved placement. The column order is saved after a header drag. A value that is missing or damaged falls back to its default, and a failed save shows its message ("Feedback"). `gozler-ui --data-dir <folder>` keeps `ui.json` and `ui-error.log` in another folder instead, so a test never touches the person's settings. It holds two groups:

- `settings`: the settings above, with the language as a language tag, such as `de`, or absent for Windows default.
- `window`: the placement, the mode (a file without it opens in Search), column order and visibility, the sort, whether the filter pane is shown and its choices, whether the inspector is shown and its height, the match controls, whether the tray notification has been shown, each panel's folder, sort and last folders and which panel is active, and the Viewer's size, place and Wrap long lines.

The keys are the file format, so renaming a key needs a migration.

The read/merge/write runs on one persistence worker. It reads the latest file
at each attempt and keeps unrelated keys saved by another window. Queuing a
save advances this window's attempted baseline even on failure, preserving the
retry-on-next-change rule. Failures are logged on the worker before delivery,
so closing the UI cannot discard a deferred error without a record.

Settings/recovery finalization starts after the window is destroyed and never
joins its writer on the UI thread. Essential writes can retain the invisible
process until they finish. Completion has its own signal, since
setup may already have posted WM_QUIT. At Windows session end, the destroyed
window's thread dispatches messages for at most two seconds, preserves an
earlier quit for outer loops, then returns. Windows may terminate the writer
afterward, so the newest state cannot be guaranteed at sign-out; atomic file
replacement keeps the last valid file. Nonessential readers lose delivery and
large discarded payloads retire off UI; accepted-mutation cleanup has its own
background lifetime.

### Language

The window's text is translated, and the person changes the language while the window runs.

- **Two places choose it.** The Language menu, the last item of the menu bar, aligned right, with a radio mark on the current choice, is one click from the window and stays in the same place in every language. The Language list at the top of Settings > General is where Everything and TinyTorrent2 keep it. Both list "Windows default", then each shipped language by its name in the current language and, when that differs, its own name in brackets, as Chrome does: "Dutch (Nederlands)" in English, "Nederlands" in Dutch. The translated name reads as a word of the current language, which "Nederlands" alone did not for an English reader, and the own name lets a person who picked a language they cannot read still find their own. Both show even when only English ships, so the window has one layout.
- **At once.** A click in the Language menu changes the language immediately, with no restart, and saves the choice; in Settings, OK or Apply does the same. `MainWindow::set_language` loads the new `Strings`, records the choice and the strings in a statement of its own, and runs `apply_text`, whose changes the window paints once, when the update ends (see "Painting"). A combo box, list box or list view gets its items again with the same selection index. A selection set by a message sends no `CBN_SELCHANGE` or `LBN_SELCHANGE`, and the Kind list's `LVN_ITEMCHANGED` selects the kind the filter already holds, so no filter changes and no search starts. The typed search, the results, the selection, the scroll position and the focus are never touched, and no search runs again, because the results do not depend on the language (`T12`). The switch takes about 100 to 150 ms on the owner's laptop, which is accepted for a rare, deliberate choice. After Apply, the open Settings dialog keeps its text and shows the new language the next time it opens, because changing the text of four open pages would need a second caption pass for one moment.
- **Windows default** is the choice until the person makes another. It takes the first language in the Windows display language list that the window ships, or its parent language (`pt` for `pt-BR`), and English when none matches. Numbers, dates and times follow the Windows regional format in every language, because the region is a separate Windows choice: Windows formats them (`GetNumberFormatEx`, `GetDateFormatEx`, `GetTimeFormatEx`), through a `Numbers` value that `win` implements and the tests implement in English, and a `WM_SETTINGCHANGE` with `intl` runs `apply_text`.
- **What stays in English:** the search syntax, such as `ext:` and the size words, because a query must mean the same in every language, so Help > Search syntax shows it from the same tables as `gozler --help`: the examples that use no option, in their search-box form, and the notes that hold for a search box, plus two notes on the Match list, because the term descriptions describe Anywhere and a regular expression there reads the whole box; the detail that the daemon or a library gives, such as why a regular expression does not compile, which follows a translated sentence, because the daemon sends English text and no code; the service report that Report opens, because it is `gozler --status`'s text, which a bug report quotes as `gozler` prints it; paths, names, exported files and copied text, which are data; and Windows' own dialogs, which follow the Windows display language. The parser's messages are translated in full, with what to do, because a mistyped search is the failure a person meets most.
- **Catalogues.** `resources/en.json` holds every key and its English text, and another language is `<tag>.json` with the same keys, grouped by surface in one level of objects with snake_case names, such as `menu.export`. Placeholders are numbered, `{0}`, so a translation may reorder them. A counted message has an `_one` and an `_other` form; the first shipped language that needs another plural rule takes it from the ICU that Windows includes (`uplrules_select`), and no ICU is bundled. Text is plain data, and a menu item, dialog label or button holds its access key as `&`, so each language chooses its own letters. Size units, such as "MB", are catalogue messages.
- **Loading.** `include_str!` embeds each catalogue, and a table in `core::text` lists each language's tag, its own name, the key of its translated name and its catalogue. Every catalogue names every shipped language in its `language` group, such as `language.dutch`, so listing the languages loads no other catalogue. The loader rejects a key it has already seen, because `serde_json` keeps the last of two equal keys without a word. A message comes from the language, then its parent language, always whole with all its plural forms; a translated message whose key English lacks, or whose placeholders differ from English, is ignored. A missing translation does not fall back to English: a message that the language and its parent both lack shows as its dotted key, such as `settings_index.live_pending`, so a person sees the gap and nothing panics. A catalogue that does not parse is a build defect that the catalogue test catches; at run time each of its messages shows as its dotted name. Another language ships when the owner adds its catalogue, its row in the table and its name in every catalogue.
- **The core reports codes, not sentences.** The window maps each `QueryError` variant with its typed reason and each `VolumePhase` to a key with a `match`, which fails to compile when a variant is added. Attribute letters are `char`s, so the catalogue test checks that every letter has its key.
- **The pseudo-language** `qps-ploc`, the tag Windows uses for its own long-text pseudo-locale, is generated from `en.json` by `build.rs`, only with the crate's `pseudo-language` feature. A message of up to 12 characters becomes twice as long and a longer one 50 % longer, with accented letters, keeping placeholders and access keys, so a screenshot shows where a control still clips. A shipped executable never contains it.

### Memory budget

The release `gozler-ui.exe` stays under 16 MB of private bytes in steady state: after the empty search has been scrolled to its last reachable row, with no selection held. The full cache of 32,768 rows takes about 6.5 MB, at about 200 bytes a row for the average full path of about 110 characters. When a commit raises the measured figure by more than 1 MB, its message says why. Two actions that the person starts use more, only while they last: a large selection holds its files, about 20 MB for 100,000 rows, until it changes, and a large copy builds its clipboard data once, about 22 MB for 100,000 paths, which Windows owns after the copy. The window stays responsive during both: no message waits more than 100 ms.

Commander mode holds more, in proportion to what its panels show, so the 16 MB describes Search. A folder panel holds about 100 bytes for each entry of its folder, which makes about 100 MB for a folder of a million entries while the panel shows it: an entry is at most 48 bytes (a test pins it), plus its row's index and its name on the heap. An entry keeps a hash of its folded name, not the folded name, to stay within that size. A search panel caches its rows as Search's list does, about 7 MB for its two chunks, and drops them when it leaves the search. A Viewer keeps at most 16 blocks of 64 KB, 1 MB, whatever the file's size; only while Copy runs does it hold the whole file and its text (see "The Viewer").

The last measurement, at f5573d9, before the panels' rows were drawn by the list view (6bb243a), with `docs/research/file-manager/scripts/stage7-memory.ps1`: both panels listed hold 8.80 MB; one search panel with `*` on all drives, 2.26 million results, adds 6.96 MB against a budget of 7.5 MB; two such search panels add 14.43 MB against 15 MB; leaving both searches and filling them again adds nothing (-0.46 MB); and Search, scrolled to its last row, holds 10.44 MB against its 16 MB. It is to be run again on the current code. Commander mode's minimum client width was 833 DIP in English, 859 in Spanish and 853 in Dutch and Turkish (`stage7-minwidth.ps1`), set by the panels' search row. The key bar's icons add 20 DIP to each of its six buttons, so it now sets Spanish's, whose widest caption is `F7 Nueva carpeta`, at about 879 DIP, and the others stay at 833, 853 and 853. These figures are worked out from the layout's rule and Segoe UI's character widths, not measured: run `stage7-minwidth.ps1` again and record what it gives.

### How normal changes land

| Change | Where it lands |
| --- | --- |
| A new command | a row in "Commands", a row in `core::commands`, and its `MainWindow` method |
| A new column | a `Column` entry in `core`, and its arm in `text::cell`, which the `LVN_GETDISPINFO` handler calls |
| A new filter | `Filters` and its clause text, one control, and its rectangle in `Layout::compute` |
| A new Settings page | one child-dialog template and its dialog procedure |
| Dark mode | a `win::dark` module, which `win::controls` calls for each main-window control and `win::dialog`'s procedure for each dialog, whose procedure then answers `WM_CTLCOLOR*` |
| A new text | a key in `en.json` and in every other catalogue, which otherwise shows the dotted key, and the function that formats it (window rule 12) |
| A new language | its `<tag>.json` in `resources/`, a row in the language table, and its name in the `language` group of every catalogue |
| A right-to-left language | the main window and the dialogs created with `WS_EX_LAYOUTRTL`, which mirrors their coordinates, so `Layout::compute` does not change; whether a switch between directions must create the controls again is not verified |
| A second top-level window | another window procedure with the same trampoline, and `win::window::own_keys` when it takes its own keys, as the Viewer does |
| Results that update when files change | a new outcome, after the protocol gets change notification |

## Commander mode

Commander mode is the window's second mode: a file manager that works like Norton Commander. A person who knows Norton Commander or Total Commander presses the same keys and gets the same result. Search is the other mode. A panel lists the disk itself and takes only the folder sizes from gozlerd, because gozlerd does not index USB, network and FAT drives, and a file manager must browse them. The disk work of a file operation is done by the engine, `gozler-fileops` (see "File operations"); this chapter says what the person sees and does.

**Owner decisions.** These are settled:

- **A Norton Commander style mode,** with Norton Commander's keys:

  | Key | Result |
  | --- | --- |
  | `F3` | view the file, as text or as hex |
  | `F4` | edit: shown in the key bar, not available |
  | `F5` | copy to the other panel |
  | `F6` | move to the other panel, or rename |
  | `F7` | new folder |
  | `F8` | delete |
  | `Alt+F7` | show Search, the window's search with gozlerd; `Ctrl+F` searches in the panel |
  | `Alt`+a letter | quick search: go to the first name that starts with the letters |
  | `Alt+F1`, `Alt+F2` | change the left or the right panel's drive or folder |

- **gozlerd finds the files and gives the folder sizes.** Each folder in a panel shows its size, which gozlerd already knows, without a key press.
- **Copy, move and delete are as fast as possible,** with the engine's methods: parallel deletion and the copy engine that measures faster.
- **Only the asked features:** the keys above, with what makes them usable, such as selecting several entries for `F5`. "Not planned" lists what this left out.
- **No Recycle Bin.** Every delete is permanent and fast. A person who wants the Recycle Bin uses File Explorer.
- **Operations run in the background,** with a queue (see "Waits").
- **The window never waits for disk work:** a command replies when the person's choice is recorded, the disk work runs on a worker, and its progress shows on the operation.
- **No custom drawing for little gain.** The list view draws the panels' rows and Windows draws the key bar's buttons, because a drawn imitation needs care in every theme and still does not look or work quite like the real control.

### Commander mode's window

| Area | Control |
| --- | --- |
| Top | the menu bar |
| Left half, right half | a panel: the drive button and the path box, the list, and the status line |
| Under the panels | the key bar |
| Bottom edge | the status bar |

- **Commander mode and Search share one window.** View > Commander switches between them, and the window opens in the mode it closed in. When Search shows again, its list scrolls its focused row, or else its first selected row, into view. Each panel takes half the window, with no splitter between them. The filter pane and the inspector belong to Search, and Commander mode does not show them.
- **Switching modes keeps each mode's state:** the search text, its results and the panels' folders. While Search shows, the panels stop watching their folders and ask gozlerd nothing, and they read their folders again when Commander mode shows, because a watched folder on a USB drive blocks its removal and nobody sees the changes.
- **The title is "Gozler",** because a title that names one panel's folder would be wrong for the other.
- **Show Commander** (the tray or its shortcut from any program) explicitly selects Commander mode and focuses the active panel's list. Show Search and a second start explicitly select Search (see "Window lifetime").

### A panel

- **The drive button** shows the drive letter, such as `C:`, and opens the drive menu (see "The drive menu"), so the mouse reaches it too. A folder on a share shows `\\`.
- **The path box** is an edit box with the full path of the panel's folder. `Ctrl+L` or a click focuses it, with its text selected. The person types a path and presses `Enter` to go there. `%USERPROFILE%` and the other environment variables are expanded, as File Explorer does. A relative path, such as `..\Other`, is joined to the panel's folder. A file's path shows its folder with the file focused. `Esc` puts the folder's path back and returns the focus to the list. When the path box loses the focus without `Enter`, such as from a click in the list, it also puts the folder's path back. The path box decides nothing from the disk: the panel's reading thread opens the typed path, and the list keeps the old folder's rows until it has. A typed path that cannot be opened then leaves the panel in its folder, and a message box says why, such as "D:\Old does not exist." or "Windows could not open D:\Private: Access is denied." When it closes, the path box has the focus with the typed text selected, so the person corrects it.
- **The list** is a virtual list view in report mode, as Search's is, so a folder of a million entries shows at once. It has four columns:

  | Column | Content |
  | --- | --- |
  | Name | the full name, with the extension's icon, as in Search |
  | Ext | the extension without its dot; empty for a folder |
  | Size | a file's size, in the unit of Settings > General's "Show sizes in", as in Search (see "Results list"); a folder's size from gozlerd, or "Folder" while gozlerd does not know it |
  | Date modified | as in Search |

  - The first row is `..` in every folder except a drive's or a share's root. Its icon is Windows' open folder (`SIID_FOLDEROPEN`), which is already in the system image list, so the list draws it as it draws every other icon (**Owner decision**: no arrow, because the native control wins over a detail of the prototype; on Windows 11 the small open folder looks almost like a closed one, and that is accepted). It has no size and no date, and no command acts on it.
  - Folders come first, then files. Each group is sorted by the chosen column. Names compare by their folded form, as Search's name sort does, so the two modes order names the same.
  - A click on a column header sorts by it, and a second click reverses the order; View > Sort by does the same for the active panel, without Path, which a panel has no column for. The first sort by Size or Date modified puts the largest or the newest first, as in Search. The header shows the arrow.
  - Hidden and system entries are shown, in the gray text color, as File Explorer shows them when "Hidden items" is on. A copy of a folder copies them too, so the person must see them.
  - Ext, Size and Date modified have the same widths in both panels and every folder, so nothing moves by itself: Ext fits an extension of 7 letters, such as "torrent", Size a size of 10 digits in KB, and Date modified any date. A longer extension ends in "…", and the label tip shows it whole. Name takes the width that is left. The person can drag a column's divider; widths are not saved.
- **The status line** under the list counts the panel's entries, without `..`, and shows the free space of the panel's drive at its right end, so before `F5` the person compares the selection's size with the other panel's free space without leaving the panel:

  | Case | Left | Right |
  | --- | --- | --- |
  | Nothing selected | "14 items · 5.2 GB" | "120 GB free" |
  | Entries selected | "3 of 14 selected · 4.2 GB" | "120 GB free" |
  | A search panel | "1,204 results in C:\Users\me", or "3 of 1,204 selected · 4.2 GB" | the searched drive's free space; for All drives, the focused result's full folder, because the Folder column may cut it off. A folder that does not fit loses its middle, so the drive and the last folders stay |

  The size adds the sizes of the counted files and folders, in the short form of Search's status bar (see "Status bar"), such as "4.2 GB", because the Size column's one unit does not compare at a glance with the free space. A panel's files always have their size, so the size sums the known sizes and counts the folders without one, as "14 items · (3.4 GB, 12 unknown)". A search panel's selection counts the same way: an entry with no known size, such as every folder for a person who does not see every file (see "Visibility"), is counted as unknown, and never hides the sum of the others. It never says only "size unknown": when no counted size is known, it shows the count alone, as "(12 unknown)" (**Owner ruling**: show what is known and how many are not). Until gozlerd first answers the sizes asked for a folder just listed, a sum that lacks some shows no size, "14 items", because the folders are not yet unknown, only not yet asked, and the line would flash "(…, 12 unknown)" on each move into a folder. The panel keeps the sum as entries come and go, so a large folder is never added up again. The free space is read on a thread, and again after each change in the panel.
- **The active panel** has the focus. Its path box shows the system highlight colors, as Total Commander shows its active path, except while the path box has the focus: then it is a plain edit box, so selected text and the caret show. The other panel shows its selected entries in the gray that Windows gives a list without the focus, and no focused row. Commands act on the active panel. The other panel's folder is the destination of `F5` and `F6`. `Tab` makes the other panel active, and so does a click anywhere in it.
- **Reading a folder** runs on the panel's own thread, so a slow network share never holds the window. The path box shows the new path at once. The list keeps the old folder's rows until the thread has opened the new folder, then says "Reading…" until the listing is complete, so a folder that cannot be opened leaves the panel where it was. Going to another folder cancels the reading. The thread sorts the listing by the panel's sort, so a large folder is never sorted on the window's thread. **Reading the shown folder again** (after lost changes, after an operation ends, on `Ctrl+R`) keeps its rows, the focus, the selection and the scroll position until the new listing is complete, then applies it by name, so the person never loses their place.
- **The list follows the disk.** The panel watches its folder, not the subfolders, with `ReadDirectoryChangesW`, and applies each change by name, so a file that another program creates appears at once. The reading opens the watch before it lists the folder, so a change between the two is not lost: the thread reports the changes after the listing, which they then apply to. When Windows reports that it lost changes, the panel reads the folder again. The focus and the selection stay on the same names. When the focused entry goes, the focus moves to the next row, and when a change brings the name back, as an editor does that saves through a temporary name, the focus returns to it. A change costs work in proportion to the change: the reading thread folds each name once, and the entry keeps a hash of it, so the window finds a name by the hash and folds a name only when the hash matches. A name that the watch reports as added, or as the new name of a rename, is the name as stored on the disk, so it matches by exact name only, and `makefile` added beside `Makefile` in a case-sensitive folder removes nothing. A removed name and the old name of a rename match without regard to case, as NTFS compares names.
- **Safe removal requires actual handle closure.** The watch's handle and a Viewer's file handle are registered with `RegisterDeviceNotificationW`. A removal query requests release from their workers and consents only after the workers acknowledge closure. If closure is still pending, Gozler denies that attempt; the person can retry once the handles close. The ensuing failure notification does not reopen a handle when Gozler itself denied the attempt. If Gozler consented and another program stops removal, it resumes the watch or Viewer instead. Explicit navigation or Retry can also reopen the drive. Removal-pending and removal-complete notifications retire the registration; the panel shows its "not ready" state with Retry. The window neither waits for a worker nor closes a handle another thread may be using.
- **A folder that Windows cannot watch,** such as one on some network shares, is read again when an operation in it ends and on `Ctrl+R`. The error of the watch's first wait tells it, `ERROR_INVALID_FUNCTION`, `ERROR_NOT_SUPPORTED` or `ERROR_INVALID_PARAMETER`, not how soon it came, because a share can refuse late and a folder can be deleted at once; any other error reads the folder again.
- **No disk call holds the window or Exit.** The threads that read a folder, watch it, read the drive labels and the free space, and read for a Viewer can block for a minute on a network share that stopped answering. They are ended with `CancelSynchronousIo`, as the engine's are, are not joined at Exit, and wake the window only while it is open (window rule 5).
- **A panel remembers** its folder, its sort and its last folders in `ui.json`, and the window remembers which panel was active. The first time, the left panel shows Documents and the right panel Downloads, the two folders a person copies between most, or `C:\` when Windows names neither. At start, a folder that no longer exists is replaced by the nearest folder above it that exists; the reading thread finds it, so a saved folder on a network share never delays the start.

Structural panel calculations use the common work owner. The panel keeps an
immutable accepted row snapshot while a large listing, watcher batch, taken-file
filter or size/order calculation runs. Its private candidate owns the changes;
current selection, focus and visibility can be remapped before publication
without replaying the disk change. Rows and their place publish together. File
commands wait for a current answer, while selection and focus remain usable.
Pending structural events are bounded; overflow requests a fresh listing, and
only a complete listing started after every omitted end can release retained
operations. The awaited command's end acknowledgment survives a discarded row
candidate. Known folder sizes share the previous snapshot until the reading
thread has applied them, then release it before watching. Counts and command
input preparation still have separate migration work in the architecture plan.

What the list area shows:

| State | List area | Status line |
| --- | --- | --- |
| Reading | "Reading…" | empty |
| The folder | its rows | the counts |
| An empty folder | the `..` row only | "0 items" |
| The shown drive is removed, or stops answering | "Drive E:\ is not ready. Insert it again, then select Retry.", with Retry | empty |
| The shown folder is deleted | the nearest folder above it that exists, as File Explorer does | its counts |

A folder that cannot be opened, from the path box, a navigation key or the drive menu, is not a state: the panel stays in its folder, and a message box says why, as File Explorer and Total Commander do (see "Feedback", a failed action that the person started). After a navigation key or the drive menu, the path box shows the panel's folder again; after a typed path, it keeps the typed text, as above.

### Moving around

| Key | Does |
| --- | --- |
| `Up`, `Down`, `PgUp`, `PgDn`, `Home`, `End` | move the focus and keep the selection, as Total Commander's cursor does (see "Selecting entries") |
| `Enter`, a double-click | On the focused row alone, as in Total Commander: on `..`, show the parent folder; on a folder, show it; on a file, open it as Windows does (see Searching) |
| `Backspace` | show the parent folder, with the folder the person came from focused |
| `Ctrl+Left`, `Ctrl+Right` | show in the left or the right panel, as in Total Commander: on a folder, that folder; on `..`, the parent folder, with the folder the panel came from focused; on a file, the folder that holds it, with the file focused. The active panel stays active. A panel that already shows that folder reads nothing, so `Ctrl+Left` on a file of the left panel keeps its selection |
| `Tab`, `Shift+Tab` | make the other panel active, from the list, the path box, the search box and Look in; from any other control, such as a drive button, return to the active panel. Search's controls are hidden, so `Tab` never reaches them |
| `Alt+F1`, `Alt+F2` | open the left or the right panel's drive menu |
| `Ctrl+L` | focus the path box |
| `Ctrl+R` | read the folder again, and ask gozlerd for the folder sizes again |
| `Esc` | close the quick search box; when it is closed, clear the selection; when nothing is selected, in a search panel, show the folder again. Each press does the first that applies, so the first `Esc` never drops a search whose results are selected |
| `Alt` pressed and released alone, `F9`, `F10` | open the menu bar, because `Alt`+a letter is quick search |

Going to a parent folder focuses the folder that the panel came from, as Norton Commander and File Explorer do, so `Backspace` then `Enter` returns.

### Selecting entries

Search's list and a panel's keep the focused row, the cursor, apart from the selected entries, as Total Commander does (**Owner decision**, 2026-10-28: Total Commander's behavior, not File Explorer's, and the same in both lists). The cursor keys move only the focus and keep the selection; `Insert`, `Space` and `Shift` with a cursor key select. The mouse keeps the native list's behavior, as Total Commander's left mouse button mode does.

- **The list view already keeps both:** the focused row is its `LVIS_FOCUSED` row, and the selected entries are its `LVIS_SELECTED` rows. So Narrator reads the selection, and `Ctrl`+click, `Shift`+click and `Ctrl+A` keep Windows' own behavior. The list owns which rows are selected, as in Search (see "The selection"). The window reads the selected rows once for each action of the person. When a change moves rows, the panel captures their stored names and selects the same exact names among the new visible rows; selection does not mutate the entries.
- **The window takes the cursor keys** (`core::selection::moved` and `passed`, which `MainWindow::select_key` applies to either list). `Up`, `Down`, `PgUp`, `PgDn`, `Home` and `End` move the focus as the list would: a page key first goes to the last or the first row in view, then a page further. `Ctrl` with them is the list's own and also moves only the focus.
- **The list view draws the rows** in Windows' default list style, as in Search's list. Gozler does not ask for the "Explorer" visual style, which draws a border around each selected row, so adjacent selected rows show a double line. In the list that has the focus, selected rows are File Explorer's light blue as one block, behind the text and the icon, without a border (**Owner decision**). Before the list draws such a row (`NM_CUSTOMDRAW`, item prepaint), the window fills the row and has the list draw it as unselected, with its text on that fill and no text background of its own. A list draws a row's icon on white, so after the list (item postpaint) the window fills the icon's box again and draws the icon on it. A list draws a focus outline only over the cells it drew, and Search draws its Name and Path cells itself, so in the focused list the window draws the focused row's outline over the whole row after the list. A list without the focus and high contrast keep Windows' own selection colors. The focused row has an outline while its list has the focus, so a focused selected row is an outline on the fill. A focused row that is not selected gets a light gray under its outline, so the row that `Enter` opens shows (**Owner decision**, 2026-10-28: the outline alone was too faint to find, and a blue fill looked like a selection). The gray is lighter than the selection of a list without the focus. While the search box has the focus, Search's list draws its selected rows and that row as the focused list does, without the outline, because `Enter` in the box opens that row, and Windows' gray for a list without the focus would look like the cursor row's gray. The list's own drawing follows the theme and high contrast. Before the list draws a row, the window also sets its font and color (`NM_CUSTOMDRAW`, item prepaint): hidden and system entries in gray, and a taken result of a search panel struck through, in gray.
- **Commands act on the selected entries,** or on the focused row when nothing is selected, as in Norton Commander. No command acts on `..`, and `..` cannot be selected. In a list of results, the selection is then the focused row (`Model::set_selection`), so `Enter`, the commands and the inspector use it, and the status bar counts no selected entry.

| Key or mouse | Does |
| --- | --- |
| `Insert` | select the focused row, or clear it when it is selected, and move the focus down |
| `Space` | select the focused row, or clear it when it is selected, and keep the focus there, as Total Commander does by default; while quick search is open, it types into it |
| `Ctrl+Space` | the list's own: select the focused row, or clear it |
| `Shift+Up`, `Shift+Down`, `Shift+PgUp`, `Shift+PgDn`, `Shift+Home`, `Shift+End` | select the rows the focus leaves as it moves, and the row it ends on when the list's edge stops it, which `Shift+Home` and `Shift+End` always reach. A run of these keys gives its rows the state opposite to its first row's, so a run that starts on a selected row clears; a new run starts when Shift is pressed again or the focus has moved another way, so a second run adds another range. `..` is left out |
| `Ctrl+A` | select every entry |
| a click | select only that row and focus it, as in File Explorer |
| `Ctrl`+click | select the row, or clear it when it is selected |
| `Shift`+click | select the rows from the focused row to the clicked one |
| a right-click | on a row that is not selected: clear the selection and focus the row, as File Explorer does; on a selected row: focus it and keep the selection; then open the context menu |
| `Esc`, Edit > Clear selection | clear the whole selection (**Owner decision**), because every delete is permanent, so a forgotten selection must take one key to drop |

- **The selection is cleared** when the panel shows another folder, and when the person submits a copy, a move or a delete of it, as in Total Commander, because the rows of a move or a delete leave the panel.
- **Search's list and a search panel use the same keys and the same list selection.** A search panel's selection is read from gozlerd, as Search's is.

### Quick search

`Alt` and a letter, a digit or a punctuation key opens the quick search box, as in Norton Commander. The same key without `Alt` opens it too, because the panels have no command line to type into, and File Explorer's lists jump the same way. These are keys of a panel's list (window rule 8): in the path box a letter is typed, and `Alt`+a letter opens the menus. The message loop takes the list's `WM_CHAR` and `WM_SYSCHAR` before the list sees them, and lets the `WM_SYSKEYDOWN` through, so releasing `Alt` after a quick search does not open the menu bar, while `Alt` pressed and released alone still does. So in Commander mode the menu bar opens with `Alt` pressed and released alone and then the letter, or with `F9` or `F10`, as in Far Manager; `Alt+F` does not open the File menu while a panel's list has the focus.

- **The box** takes the place of the active panel's status line while it is open, and shows the typed text, such as "Find: rep". It opens with the first character.
- **The first row whose name starts with the typed text is focused,** as a cursor key would, and the selection stays, from the top of the list, without regard to case. `..` never matches. The text is the start of a name, not a pattern, so `*` and `?` match themselves.
- **Each next character extends the text,** with or without `Alt`. `Space` is a character while the box is open, so a name with spaces is found. A character after which no name would match is not added, so the focus never leaves a matching row; a first character that matches nothing does not open the box.
- **A folder update preserves pending characters.** A background match retries against changed rows; newer selection, focus, navigation or `Esc` invalidates its old intent instead.
- **`Backspace`** removes the last character, and the first row that matches the shorter text is focused.
- **`Up` and `Down`** go to the previous and the next row that matches.
- **`Enter`** closes the box and does what `Enter` does on the focused row.
- **`Esc`** closes the box. The row stays focused.
- **Any other key, or a click,** closes the box and then does what it does without the box. So `F5` after a quick search copies the focused row.
- **In a search panel** it looks only at the rows already fetched, because the rest are not in the window.

### The drive menu

`Alt+F1` opens the drive menu of the left panel and `Alt+F2` that of the right panel, as in Norton Commander, whichever panel is active. The drive button opens it too. The panel becomes active.

- **The menu** opens under the panel's drive button. It lists every drive, "&C:  Windows", with the drive letter as its access key, so `Alt+F1` then `D` goes to drive D, as in Norton Commander. A separator follows, then the last 10 folders that this panel showed, newest first, with the access keys `1` to `9` and `0`, without the folder the panel shows, because choosing it would do nothing. The folder list is saved in `ui.json`. The panel's own drive has the radio mark, and the menu opens with it highlighted, as Norton Commander's drive menu opens on the current drive.
- **A drive** shows the folder that the panel last showed on that drive in this session, as Norton Commander remembers a folder for each drive, or the drive's root. The drive that the panel already shows goes to its root, because otherwise the choice would do nothing. In a search panel, a choice also leaves the search.
- **The drives** are read when the menu opens (`GetLogicalDrives`, which touches no drive). **The labels** are read on a thread and kept, so a sleeping network or optical drive never delays the menu. A label not read yet is left out.
- **A drive that is not ready,** such as a card reader with no card or a disconnected network drive, is listed. Choosing it leaves the panel in its folder, and a message box says "Drive E:\ is not ready." with Windows' reason.

### Folder sizes

A folder's size is the size of everything below it, as gozlerd keeps it (see "Sizes and dates"). The panels ask for it; they never count.

- **One request gives every size.** When a panel has read a folder that holds folders, it asks gozlerd for the folder's direct subfolders: a search for `folder:` with the folder as its `-parent` scope. The reply gives each subfolder's name and size, and the panel matches them to its rows by name, exactly first, then without regard to case, as NTFS compares names. The request reads at most 100,000 subfolders; a folder of more shows "Folder" for the rest. A panel sorted by size has its folders sorted by the new sizes on a thread before it shows them, because sorting a folder of many subfolders would hold the window.
- **Each panel has its own request purpose** (window rule 4), so one panel's request never cancels the other's, and a slow answer never delays the list. The rows show first, and the sizes fill in.
- **The cost is accepted.** gozlerd answers `-parent` by walking the folder's subtree, or the whole volume for a large tree or a drive's root. Measured with `gozler -parent <folder> folder: -size` against the owner's gozlerd (2.4 million entries): 142 ms for `C:\`, 122 to 149 ms for large folders and 38 ms for `Documents`, each with about 35 ms of process start. The request runs on its own thread, so the sizes appear about a tenth of a second after the rows.
- **A size that gozlerd does not know shows "Folder".** That happens when the drive is not indexed (USB, FAT, network), Include leaves the folder out, the volume's size pass has not completed, the person does not see every file (see "Visibility"), or the service does not run. The panel never counts a size by walking the folder, because the person never waits for a count (owner ruling). The status bar's right group says why for a drive that is indexing or is not indexed (see "The status bar in Commander mode"); that case has no note in the status bar, because the Visibility page and the setup say that a user who does not see every file gets no folder sizes, and the Visibility page says whether the person is one.
- **An accepted size answer replaces the previous sizes,** including an empty answer or one with only unknown sizes. Missing sizes become unknown in totals and Size sorting. An expired request keeps the previously accepted sizes until another answer arrives.
- **The panel asks again** after each reading of the folder, 500 ms after the last change in the folder, when a file operation whose source or destination is in or below the folder ends, and on `Ctrl+R`. The service applies a change from the volume's journal after the disk has it, so after an operation ends the panel asks once more 2 seconds later. A program that writes deeper in the tree changes a size without a change in the shown folder; that size is out of date until one of these events.

### Search in a panel

`Ctrl+F` turns the active panel into a search panel, as Norton Commander's Find File searches from the current folder.

- **The search box** takes the place of the path box, with a drop-down list beside it, "Look in": "This folder and subfolders" (the default), as Far Manager's search does, or "All drives". The drive button stays, so its menu leaves the search with the mouse too. The focus goes to the box. While the box is empty, the list area says "Type a name to search C:\Users\me and its subfolders.", or "Type a name to search all indexed drives." On a drive that gozlerd does not index, it shows the "not indexed" message below instead, so the person learns it before typing.
- **The search** runs as the person types, 150 ms after the last key, through gozlerd, as in Search. It uses Search's syntax with the default match: Start of words, without Match case and Match path. Search's match controls and filters do not apply, because the panel does not show them; the typed syntax, such as `case:` or `regex:`, gives the rest.
- **One state for each list.** Search's list and each panel keep their own search text, results, selection and taken entries, and each list has its own `Results` and `Selection` requests (window rule 4), so a search in one list never cancels or replaces another list's.
- **The list** shows Name, Ext, Folder, Size and Date modified. With "This folder and subfolders", Folder is the path below the searched folder, such as `Projects\Accounts`, or empty for the folder itself; with All drives, it is the full path. A path that does not fit ends in "…", and the list's label tip shows it whole, as any cut-off cell's (**Owner decision**: no custom drawing for little gain). Its cells are Search's, so a folder of unknown size has an empty Size cell and a file of unknown size reads "Unknown" (see "Results list"). Its rows come on demand, every match, as in Search (see "Rows on demand"). It does not follow the disk; `Ctrl+R` runs the search again. A search that finds nothing says "No items match your search.", and with "This folder and subfolders" a second line says "Choose All drives in Look in to search every indexed drive." The panel drops its rows when it leaves the search, so a search panel costs memory only while it shows. It leaves the search by `Esc` with nothing selected, `Backspace`, or a choice in the drive menu. A searched folder that was deleted meanwhile opens the nearest folder above it that exists.
- **The sort** is the panel's own, the one its folder uses, so the header works as in a folder panel; gozlerd sorts the results. It never changes Search's sort. The Folder header does not sort, because the panel's sort also orders its folder, which has no Folder column.
- **A folder on a drive that gozlerd does not index** cannot be searched with "This folder and subfolders". The list area says "E:\ is not indexed, so Gozler cannot search it. Choose All drives in Look in to search the indexed drives.", instead of an empty list that reads as "no such file".
- **Keys in the search box:** `Down` and `Enter` go to the list. `Esc` shows the folder again, with the row that was focused before the search. `Tab` goes to the other panel's list, as from the path box. `Alt+L`, the access key of "Look in", goes to the drop-down list, whose `Enter` returns to the search box.
- **Keys in the list:**

  | Key | Does |
  | --- | --- |
  | `Enter` | as in a folder panel: a folder result shows that folder, and a file result opens as Windows opens it |
  | `Esc` | clear the selection; with nothing selected, show the folder again, as before the search |
  | `Backspace` | show the folder again, as before the search |
  | `Ctrl+F` | go to the search box with its text selected |

- **Commands:** `F3` views the focused result, as in a folder panel. `F5`, `F6` and `F8` act on the selected results, or on the focused one (see "Selecting entries"). New folder is disabled, because the list shows gozlerd's results, not a folder. When the other panel is a search panel too, `F5` and `F6` are disabled, because there is no destination folder.
- **A result that a delete or a move takes shows struck through, in gray** (see "The person never waits for the disk").
- **States:** the list area shows Search's messages for no results, a search that does not parse and a failure of the service (see "States", `S5`, `S7` and `S9`), and the "not indexed" state above, without Search's buttons, because "Look in" is the panel's only filter. A list whose service failure can recover searches again when the service answers; one that cannot does not repeat its request.

### The key bar

A row of push buttons under the panels: `F3 View`, `F4 Edit`, `F5 Copy`, `F6 Move`, `F7 New folder`, `F8 Delete`.

- **Each caption has the key and the word,** as Total Commander's bar does, because the bar is there to teach the keys. The word follows the owner ruling on button captions (`AGENTS.md`). Windows draws the buttons, in every theme and in high contrast.
- **The buttons share the window's width equally.**
- **Each button shows its command's icon** before its caption while Show icons is on, the same icon as that command has in the menus (see "Look"): View, Edit, Copy to folder, Move to folder, New folder and Delete. The caption still starts with its key. The icon and its gap widen each button, so the key bar's minimum width counts them while icons show.
- **A click runs the command** on the active panel. The buttons take no focus, so the focus stays in the panel, and `Tab` does not reach them, because `Tab` switches panels.
- **A button is disabled when its command is,** never hidden. `F4 Edit` is always disabled, so the bar has Norton Commander's order and the person sees that Edit is planned; its tooltip says "Editing is not available yet." `F6 Move`'s tooltip says "Move to the other panel, or rename.", because no other place tells a person who wants to rename that `F6` does it.

### Commands in Commander mode

| Command | Keys | Enabled when | Acts on |
| --- | --- | --- | --- |
| View | `F3` | the focused row is a file | the focused file, in a Viewer |
| Edit | `F4` | never | — |
| Copy to folder… | `F5` | an entry is selected or focused | the selected entries, or the focused row |
| Move to folder… | `F6` | as Copy to folder… | as Copy to folder… |
| New folder… | `F7` | the panel shows a folder | the panel's folder |
| Delete | `F8`, `Delete`, `Shift+Delete` | an entry is selected or focused | the selected entries, or the focused row |
| Find | `Ctrl+F` | always | the active panel (see "Search in a panel") |
| Open, Properties | `Enter` on a file in a folder panel; none | as in Search | the focused row, or the selected entries |
| Select all | `Ctrl+A` | always | the panel's entries, or the focused box's text |
| Clear selection | `Esc` | an entry is selected | the panel's selection |
| Refresh | `Ctrl+R` | always | the panel's folder and its sizes, or its search |

Copy to folder… and Move to folder… are commands of both modes (window rule 9). Both use the destination dialog of "Copy and Move". The key bar still reads `F5 Copy` and `F6 Move`. The key bar, the menus and the context menus show each command disabled by these rules, on the active panel's state, so the key bar never offers a key that does nothing. A second Gozler window, which owns no file operations, disables Copy to folder…, Move to folder…, Synchronize folders…, New folder… and Delete (see "Interrupted operations").

Commander mode has no clipboard commands. `Ctrl+C`, `Ctrl+X` and `Ctrl+V` in the path box and the search box edit their text, as in any edit box.

### Keys

**Keys belong to a mode.** `F5` is Refresh in Search and Copy to folder… in Commander mode. Each mode has its own accelerator table, built when the mode shows. A key has one of three owners:

- **Commander mode's fixed keys** are accelerators of Commander mode. They work wherever the focus is in Commander mode, the path box included, as Norton Commander's function keys do. They are fixed because they are the keys people choose Norton Commander for.
- **Keys of the focused control** (window rule 8) act only in that control. A panel's list takes the keys marked "list" below; the path box takes `Enter`, `Esc` and `Tab`; a panel's search box takes `Down`, `Enter`, `Esc` and `Tab`. The path box and the search box keep every other key for their text.
- **The person's shortcuts** (Settings > Keyboard) for a command that both modes have, such as Settings…, work in both modes. In Commander mode a fixed key wins over a shortcut on the same key, and the menus show the key that works in the shown mode, so a menu never shows a key that does something else. Commander mode's accelerator table also leaves out a person's shortcut on `Alt` with a letter, a digit, `,` or `.`, such as `Alt+.`, because the message loop translates accelerators first and quick search needs those keys; such a shortcut works only in Search, and the Keyboard page says so (see "The Keyboard page").

| Key | Commander mode | Owner in Commander mode | Search |
| --- | --- | --- | --- |
| `F3` | View | fixed | — |
| `F4` | Edit (disabled) | fixed | — |
| `F5` | Copy to folder… | fixed | Refresh |
| `F6` | Move to folder… | fixed | — |
| `F7` | New folder… | fixed | — |
| `F8` | Delete | fixed | — |
| `F9`, `F10` | open the menu bar | fixed; `F10` is Windows' own | `F10` opens the menu bar |
| `Alt+F1`, `Alt+F2` | the left or the right drive menu | fixed | — |
| `Alt+F7` | show Search, with its search box focused and its text selected, as Total Commander's `Alt+F7` opens its search | fixed | Find |
| `Ctrl+F` | Find | fixed | `Ctrl+F`, `Ctrl+E` and `Alt+F7` are Find |
| `Ctrl+R` | Refresh | fixed | — |
| `Ctrl+L` | focus the path box | fixed | — |
| `Delete`, `Shift+Delete` | Delete | list | the same, a key of the list |
| `Ctrl+C`, `Ctrl+X`, `Ctrl+V` | the path box's and the search box's own | the box | Copy, Cut and Paste |
| `Ctrl+A` | Select all | acts on the focused control, as in Search | the same |
| `Enter`, `Backspace`, `Esc`, arrows, `PgUp`, `PgDn`, `Home`, `End` | as "Moving around" says | list | the list's own |
| `Ctrl+Left`, `Ctrl+Right` | as "Moving around" says | list | — |
| `Insert`, `Space`, `Shift`+arrows | as "Selecting entries" says | list | — |
| a character, `Alt`+a character | quick search | list | — |
| `Tab` | the other panel's list | list, path box and search box | the next control |

View > Commander has no default shortcut, as Copy to folder… has none; the person can give it one.

### Copy and Move

1. **The question** is a dialog titled Copy to folder or Move to folder, in both modes. It says "Copy 3 items to:" or "Copy report.pdf to:", then an edit box with the other panel's folder, ending in `\`, all selected. `F6` on one entry adds the entry's name, with the name selected without its extension, as Total Commander's does, so the person sees that typing a name renames (**Owner decision**). Left as it started, that box moves the entry into the other panel's folder, as the folder alone does, so a folder of the same name there is merged and not entered. The other panel's folder counts from the moment it opens, also while it still says "Reading…". When the other panel shows no folder, such as a search or a drive that is not ready, the box starts empty, so the person types the destination. Its buttons are Copy or Move, the default, and Cancel; Copy or Move is disabled while the box is empty. `Enter` starts the operation at once, as in Norton Commander.
2. **The dialog closes at once,** because it decides nothing from the disk. It decides only what its text alone decides: a path that ends in `\` or `/`, or any path while several entries are selected, names a folder, and so does a path that ends in the one entry's own name, case included, as `F6` fills the box. It stays open only for a path that is not valid as text, such as one with `*` or `?`, and then a message box gives the reason. Each name in the path follows "Names the person types". The engine's prepare, on its own thread, decides the rest from the disk, so a slow network share never holds the dialog:

   | The edit box holds | Result |
   | --- | --- |
   | an existing folder | the entries go into it |
   | a path ending in `\`, or any path while several entries are selected | a folder; it is created, with its missing parents, when it does not exist, as Total Commander does |
   | any other path while one entry is selected | the entry's new full path: it is copied or moved under that name. `F6` with a new name in the same folder renames, as Norton Commander's RenMov does, and the focus and the selection go with the renamed entry, also to a name that Replace takes from an entry already there. A name that differs only in case renames the entry in place |
   | a relative path | joined to the active panel's folder, so `report-old.pdf` copies the file beside itself |
   | a path that Windows refuses when the operation runs | the operation's failure dialog gives Windows' reason (see "File operations") |

3. **Then the operation goes on as every operation does,** with its accepted policy and the shared transfer pane (see "File operations").
4. **The engine** takes a missing destination folder: prepare opens the nearest folder above it that exists, and takes from it the identity, the volume, the disks and the free space that it checks. The missing folders below it are new: they have no conflicts, and the run creates them before it writes into them. A second operation into the same new folder runs, and is not refused. For one entry copied to a full path, prepare checks conflicts under the new name, and the run writes the entry under that name; the saved operation records the new name, so Continue after a crash writes the same file, and a saved file without it, from an earlier version, loads as before. The missing-folder rule applies to every copy and move, so Search's Copy to folder… also creates a chosen folder that was deleted after it was chosen, as Total Commander does.

### Names the person types

The engine opens paths with `\\?\`, which turns off Windows' own trimming and its device names, so New folder and the Copy and Move box apply them to each name they are given, as File Explorer does. `core::typed` owns the rules:

- Trailing dots and spaces are trimmed: `Photos. ` creates `Photos`.
- A device name, `CON`, `PRN`, `AUX`, `NUL`, `COM1` to `COM9` or `LPT1` to `LPT9`, also with an extension such as `nul.txt`, is refused with a message box: "Windows uses the name NUL for a device. Choose another name."
- A character that no name holds, `< > : " | ? *` or a control character, is refused with a message box that says what the balloon of New folder says, also when it was pasted. In the Copy and Move box, `\` and `/` separate the names, and `:` may follow a drive letter.
- The Copy and Move box reads a trailing `\` or `/` after it expands the environment variables, and before it removes the separators that end a path, so `%BACKUP%\2026\` names a folder, not a file `2026`.
- The other panel's folder that the Copy and Move box opens with is a real path, so a text that starts with it expands only what follows, and a folder named `%USERNAME%` stays so. The entry's own name is compared before it is trimmed, so `F6` left as it opened on `notes.` moves `notes.` and does not rename it to `notes`.

### New folder

`F7` opens a dialog titled New folder, with "Name:" and an empty edit box, and Create and Cancel. Create is disabled while the box holds no name, also while it holds only dots and spaces. The name follows "Names the person types".

The accepted folder-row snapshot carries compact exact and folded name orders,
prepared with its filtering and ordering through the existing Calculation work.
Name validation searches these orders instead of scanning all rows on UI. Exact
spelling wins; a folded match keeps the first shown spelling. The indexes and
rows publish and retire together, including after watch and taken-entry changes.

- A character that Windows does not allow in a name is refused as it is typed, with File Explorer's balloon: "A file name can't contain any of the following characters: \ / : * ? " < > |". So `\` is refused too, because the box names one folder.
- A name that a row of the panel shows, without regard to case: a message box says "Photos is already in this folder. Choose another name.", and the dialog stays open with the name selected. It names the row's entry as the row spells it, without its folder, because a long path breaks across the message box's lines. The check reads the panel's rows in memory, never the disk, and leaves out the rows that a delete or a move takes, because the person does not see them. A name that another program takes meanwhile is refused by the engine, which never replaces it, and the failure dialog says so.
- The engine creates the folder (`Request::NewFolder`). When it appears in the list, the focus goes to it. The same holds for the new name of a rename that Gozler starts in the panel's folder. The panel waits for the name only while it shows that folder, and stops when a listing read after the operation ended lacks the name, because then the operation did not create it.

### Delete

`F8`, `Delete` and `Shift+Delete` ask the Delete question of "File operations". After the delete is submitted, the selection is cleared and the first row after the deleted ones is focused.

### Menus in Commander mode

The menus follow Search's (see "Menus"): File acts on the selected entries and the window, Edit on the selection, View on what the window shows. The items of the menu bar show their keys, so the menus teach the keys as the key bar does.

- **File:** Open, View, Edit, Properties | Copy to folder…, Move to folder…, New folder…, Delete | Synchronize folders… | Cancel | Settings… | Exit. Edit is disabled, as in the key bar. Cancel stops the job that the status bar shows, as its Cancel does.
- **Edit:** Select all, Clear selection | Find
- **View:** Commander | Sort by ▸ | Refresh | File operations
- **Help** and **Language** as in Search.
- **The context menu on a row:** Open, View | Copy to folder…, Move to folder…, Delete | Properties. The context menu on the empty area of a list: New folder… | Refresh.

### The status bar in Commander mode

It has Search's three groups (see "Status bar"):

| Group | Shows |
| --- | --- |
| Left | nothing, and it takes no width, because each panel's status line shows its counts and its drive's free space |
| Middle | the running job, as in Search, or what the last operation did (see "File operations") |
| Right | in a folder panel, the index state when it leaves the panel's sizes unknown: "C:\ is indexing — sizes may be missing." while the volume is not complete or its size and date pass is not, or "E:\ is not indexed, so folder sizes are not shown." when no indexed volume holds the folder; in a search panel, the search's totals, as in Search |

The memory group follows the right group while Settings > General shows it, as in Search. Search's Show details does not show; File transfers uses the shared inspector in both modes.

### The Viewer

`F3` on a file opens the Viewer, a window of its own, as Total Commander's Lister is. Each `F3` opens a new Viewer, so the person can view two files side by side. A Viewer is a top-level window that no window owns, so it has its own taskbar button and stays reachable while the main window is hidden. Its keys are its own (window rule 8), so `F3` or `F5` in a Viewer never act on a panel, also while a dialog of the main window is open. Exit in the main window closes every Viewer; closing the main window to the notification area leaves them open. A second Viewer opens below and to the right of the last one by the height of a caption, so the two never cover each other exactly.

- **The title** is the file's name and the command: "settings.ini – View".
- **It opens at once, for a file of any size.** It reads only the part it shows, in blocks of 64 KB, on its own thread, and keeps the last 16 blocks. Until the first block arrives, it says "Reading…". So a 50 GB file or a file on a slow network share never holds a window. The file's length is the length when it opened: a file that shrinks ends where its bytes end, and a file that grows shows no more than it did.
- **Removing a USB drive closes the file:** the Viewer follows the panel's acknowledged-release rule (see "A panel"). A removal query shows "Preparing to remove the drive."; removal-pending or removal-complete shows "The drive was removed." Retry opens the file at the same place. After Gozler consents, a failed removal restores the file; after its own veto it stays closed for the next attempt.
- **It opens the file so that other programs can still write, rename and delete it** (`FILE_SHARE_READ`, `FILE_SHARE_WRITE` and `FILE_SHARE_DELETE`), so a log file that a program writes opens, and the Viewer never blocks a delete.
- **Text or Hex** is chosen from the first 64 KB: Hex when it holds a zero byte and no UTF-16 byte order mark, otherwise Text. UTF-16 without a byte order mark holds zero bytes, so it shows as Hex. `F4` switches, as in Norton Commander and Far Manager. View > Text and View > Hex show the choice.
- **Text:**
  - The encoding comes from the byte order mark: UTF-8, UTF-16 LE or UTF-16 BE. Without one, the text is UTF-8 when the first 64 KB is valid UTF-8, and Windows' ANSI code page otherwise, which the status bar names "ANSI".
  - A line ends at CR LF, LF or CR. A tab moves to the next multiple of 8 columns. Bytes that the encoding cannot read show as �, as Notepad shows them.
  - Long lines wrap at the window's width. `F2` and View > Wrap long lines switch wrapping off, as in Norton Commander; then `Left` and `Right` scroll sideways. Wrapping is on by default. In Hex both are disabled, because a hex row is always 16 bytes. A line longer than 10,000 characters is shown in pieces, because the Viewer finds a line's start by reading back from the shown position.
- **Hex:** each row shows its offset, 8 hex digits, or 16 for a file of 4 GB or more; then 16 bytes in hex, with a gap after the eighth; then the same 16 bytes as characters, with `.` for each byte outside the printable ASCII range.
- **Moving:** `Up`, `Down`, `PgUp`, `PgDn`, the mouse wheel and the scroll bar. `Home` or `Ctrl+Home` goes to the start, and `End` or `Ctrl+End` to the end. The scroll bar maps to the byte position in the file, also past 4 GB, so dragging it to the middle of a 50 GB file shows the middle at once. In Text, a jump shows the first line that starts at or after that byte. When the whole file shows, the scroll bar is disabled, as Notepad's is.
- **Closing:** `Esc` or `F3`, as in Norton Commander, or File > Close. Closing a Viewer that a panel opened activates the main window, with the same focused row, so `F3`, `Esc`, `Down`, `F3` walks through the files as in Norton Commander. Windows would otherwise activate whichever window was behind the Viewer. A main window that the person minimized or hid stays so. A held `F3` does not open and close Viewers.
- **The status bar:** the file's name, the encoding (Text) and the file's size, and at the right the position: "Top", "37 %" or "End" in Text, and the first row's offset in Hex.
- **Select all and Copy:** `Ctrl+A` or Edit > Select all selects the whole file, which shows in the system's selection colours, and stays selected while the person scrolls or switches between Text and Hex. A click in the text clears it, as in Notepad. `Ctrl+C` or Edit > Copy puts the selected file on the clipboard as text, and is disabled while nothing is selected. In Text it copies the decoded text after the byte order mark, with the file's own line ends and tabs; in Hex it copies the rows as the Viewer shows them, with CR LF between them. The Viewer's reading thread reads the whole file and puts the text on the clipboard, so the window never waits, and a copy that takes a while shows "Waiting for…" with Cancel, as a slow read does. While another program holds the clipboard, it tries again as the main window's Copy does, and then says "Another program is using the clipboard. Try again in a moment." Copy takes at most 32 million characters, 64 MB on the clipboard, because it holds the whole file and its text in memory at once: a text file up to 32 MB, or about 6.5 MB as Hex. A larger file shows "huge.log is too large to copy. Copy takes files up to 32.0 MB as text." and copies nothing.
- **Menus:** File: Close. Edit: Copy | Select all. View: Text, Hex | Wrap long lines.
- **A file that cannot be opened:** the Viewer's area says "Windows could not open settings.ini: Access is denied.", with Retry. A file that fails while the Viewer reads it says "Windows could not read settings.ini: The network name is no longer available.", with Retry.
- **The font** is Consolas, at the height of the window's message font, because columns of text and hex must line up.
- **The Viewer remembers** its size and position and Wrap long lines in `ui.json`.
- **Bytes from the disk are untrusted:** no function of `core::viewer` panics on any input, and each allocates in proportion to its input.

## File operations

These rules apply to both modes. Search reaches them through Copy to folder…, Move to folder…, Delete, Cut, Paste and a drop on a folder row (see "Commands", "Copy" and "Dragging"); Commander mode through `F5`, `F6`, `F7` and `F8`. The engine, `gozler-fileops`, does the disk work, in the window's process with the person's own permissions, never in gozlerd, because a pipe request to a service that runs as SYSTEM would let any local user delete any file. "The copy, move and delete engine" holds the engine's design.

The UI work owner constructs the engine Queue in the background. Commands
become available only after its saved operations have been read and adopted,
so no new operation can replace recovery state before restoration finishes.
Closing during restoration adds no command or persistence wait. Queue keeps
its own scheduler and process-exit lifetime.

- **Copy and Move ask first.** Both modes use the dialog of "Copy and Move", titled "Copy to folder" or "Move to folder". Browse opens the standard Windows folder picker. Search opens at the last folder chosen there.
- **Delete asks:** "Permanently delete 12 items?" and below it "They won't go to the Recycle Bin."; for one entry it names it, "Permanently delete "report.pdf"?", and below it "It won't go to the Recycle Bin." Delete and Cancel, with Cancel as the default. Gozler never uses the Recycle Bin (owner ruling).
- **Copy and Move capture one policy before preparing.** Update changed files, the default, replaces every same-name file whose size or last-write time differs, including a newer destination. Update newer files replaces only a strictly older destination, even when equal dates have different sizes, and lists the files it keeps as skipped. Under both, a file of equal size and last-write time counts as done, not skipped, so a copy onto a current backup completes without errors; a move keeps that source and lists it as kept. Keep both uses the first free numbered name. Skip existing files keeps the destination. Missing names are written, same-name folders merge, and file/folder mismatches fail. The dialog explains that the default also replaces newer destination files. Both modes put the same combo below the destination edit, with a single-line explanation. The dialog widens to fit the localized explanation. There are no later conflict questions, including for a file that appears after preparation.
- **The operation then runs in the background.** The person goes on browsing, searching and starting other operations.
- **File transfers shares the inspector in both modes.** View > File transfers or the flat operation icon/name button in the status bar opens or collapses it. Starting, running, completing, failing and restoring operations never opens it or takes browsing focus. Its native queue and selected progress sit beside each other, with Flip to swap them; a single operation uses the whole pane. The shared splitter remembers its normal height. The transfer layout measures its minimum from its actual fonts; opening a pane that needs more room enlarges the window, and the splitter always has valid bounds. Maximize fills the workspace below File/Edit/View, hiding both browsing panels and the status and key bars; Restore returns to the saved height. Maximize and Flip are remembered. A saved size alone never opens the pane. There is no separate transfer window.
- **The selected transfer keeps its own actions.** From and To follow the operation and its state, then Overall and the item/byte totals, then the current file and its independent byte meter. These positions remain fixed when a progress field is unavailable. Overall measures bytes for Copy/Move or entries for Synchronize, or says Total unknown; performance sits at the top right. Clear's tooltip explains that it removes the transfer and report while leaving files in place. The queue uses native tiles and supports dragging, its context menu and Alt+Up/Down to reorder. A waiting transfer can go before a running one: the engine first quiesces the displaced transfer, preserves its progress and later readmits it through the existing device scheduler. A manual pause survives that return. Cancel all cancels accepted Copy, Move and Synchronize work, including pending clipboard/drop intake. Ordinary Delete is independent: it has no transfer row and keeps its own progress, Cancel and report.
- **The status bar shows one transfer summary.** Its progress covers all accepted work in the current queue, retaining finished contributions until a new queue starts. Unknown or mixed totals remain indeterminate. Only the operation icon and name toggle the pane, using a flat native button; activity, progress, percentage and rates sit outside it. Pause/Resume and Cancel are separate actions on the displayed operation id, independent of pane selection. Completed work has no disabled Pause or Cancel placeholders; retained errors offer Report. A completed error summary names only the failed kinds: Copy, Move, Synchronize or their combination. Export, service fixes and Delete keep their separate status object beside it. Only the status engine separates those shared objects. File > Cancel targets its displayed operation. The taskbar combines running progress.
- **When an operation ends,** panels refresh and taken entries are released independently of opening Report. Successful and cancelled transfers disappear from the status bar; retained errors remain until their reports are cleared. Successful item counts exclude skipped or failed source entries; recovery and unfinished-output cleanup errors do not subtract successfully processed items. The selected completed transfer remains selected. Reports hold shared immutable outcomes and may outlive their engine entries; Clear releases that report and its status contribution.
- **A failure does not stop other possible work.** Failed or skipped entries are marked Report available in the queue. Report replaces selected progress with a native virtual Item/Reason list, a summary at the bottom and actions above it on the right. Close returns to progress and retains the report; Clear releases it. Refused operations and a full disk remain reportable without an automatic dialog. Long paths have info tips with the full path and reason. Delete keeps its independent failure dialog.
- **Exit while an operation runs** asks first; the question says how many operations run and offers to stop them and exit. Close, when Gozler keeps running in the notification area, only hides the window, and the operations go on.

### Copy policy and recovery

The metadata comparison selects files; it does not verify their contents.
A skipped move retains its source. Equal metadata never permits deleting a
source the operation did not transfer. Self-copy numbering and no-follow
link checks precede policy handling. Two selected entries never overwrite
one another; replacement policies number the second, including when pairs
run concurrently. Ordinary copies keep destination-only files.

Recovery checks completed and unfinished files before new conflicts.
Continue retains an explicit saved policy. Legacy Replace stays
unconditional, Skip and Keep both retain their meaning, and a missing
legacy choice uses Update changed files afterward. Recovery retains its
existing timestamp tolerances; first-run selection compares the reported
last-write timestamps directly.

### Synchronize folders

The Copy to folder dialog also offers Mirror and Sync at the left of its
button row, with Copy and Cancel at the right. Copy remains the default;
F5 still opens Copy. These extra actions require one selected folder and
apply immediately, without opening the comparison dialog. They follow Copy's
destination rules: a folder destination receives the selected folder under
its own name; a new typed name renames that destination root. Sync keeps
extra files. Mirror permanently deletes extras only inside that root, never
its siblings. The engine still compares and validates before applying;
file/folder conflicts refuse the operation. The Copy policy does not change
the synchronization rule. Move offers neither extra action.

File > Synchronize folders opens a native dialog in either mode. Source and
Destination are explicit folder roots. Commander mode fills them from its actual
folder panels, from active to other; Search selections never stand in for a
folder tree. Changes go one way, from Source to Destination. Delete extra
files (Mirror) starts unchecked. Browse uses the standard Windows folder picker.

Compare reads both complete trees on the engine's workers under device
admission. Its virtual Action/Path list shows additions, replacements,
unchanged entries, conflicts, and destination-only entries to keep or delete.
Comparison releases its devices while the person reviews it. Synchronize
stays disabled until comparison is complete and has no file/folder conflicts.
Editing either root or Mirror invalidates the preview; Cancel closes it and
cancels that comparison. Synchronize commits the retained intent on the same
operation id, reacquires its devices, and uses the shared transfer pane.

Synchronization uses Update changed files and includes empty folders. It
keeps destination-only entries by default. Mirror permanently deletes only
approved extras, after all required additions and updates succeed. Before
deleting anything it verifies every preview source fingerprint and its output
through relative no-follow opens; a missing, changed, unreadable or redirected
required path prevents the entire deletion phase. Each candidate is then
checked again for source absence and unchanged destination identity, kind,
size and last-write time on the handle being deleted. Extra folders go only
when their approved descendants are gone and they are empty, so newly arrived
children survive. Both chosen root bindings are checked before verification
and every deletion retry. These checks are point-in-time observations, not a
filesystem snapshot. Copy output uses the existing path-based executors and
does not lock intermediate names against replacement.

Overall synchronization progress counts processed entries across updates and
Mirror cleanup; copying every byte cannot complete the meter while deletions
remain. The current copy retains its independent byte meter. Mirror shows the
current deletion path without a byte meter. The operation's Cancel, Pause and
Run now use the same admission rules as Copy and Move; Cancel all includes it.
Recovery saves roots and Mirror intent, never deletion candidates. Explicit
Continue opens a fresh comparison and requires another Synchronize.

### The person never waits for the disk

These rules come from TinyTorrent2's removal redesign (SynoSoftware/TinyTorrent2#540, its `docs/plans/14-removed-torrents-plan.md`), where a removed item kept in the list with a flag made 44 readers remember to skip it, and one forgot.

- **A command replies when the person's choice is recorded.** The disk work runs on worker threads, and transfer progress shows in the shared pane. Listing a slow folder, such as a network share, also runs off the window's thread; the panel says "Reading…" until it arrives.
- **Prepare runs off the window's thread too.** The transfer pane shows "Preparing…", and submission follows when prepare is done. Prepare's thread first works out the folder that holds the operation's sources, which the window names as From and in the operation's row (`SourcesFolder`), because reading every source on the window's thread took 56 to 96 ms for 100,000 sources, three to six frames; the queue does the same on a thread of its own for each interrupted operation when it opens. Until then the row names how many sources there are, such as "Copy 4,210 items → D:\Backup".
- **An entry that a delete or a move takes leaves the folder panels and shows struck through in the search lists at once,** before the disk work ends. A row never carries a "being deleted" flag.
- **A folder panel's rows** are the folder listing minus the sources of the deletes and moves that prepare, wait or run, and of those that ended until the panel has read its folder again (the engine's taken entries). When an operation ends whose destination is the panel's folder or a path in it, such as a new name or a created folder, or a delete or a move whose source is in the panel's folder, is that folder or holds it, the panel reads its folder once more, because the change notifications and the operation's end arrive in no fixed order, and a folder that cannot be watched gets none. The first complete listing that the panel started after the end, of this folder or of another one the person went to, releases the operation: it takes nothing more from the panel. So an entry that a delete could not remove, a cancelled move's leftovers and a file copied back to a path it was moved from all show, and a deleted row never shows again for a moment. A moment counter orders the ends and the starts of the readings. An entry that a delete or a move hides loses its selection, so a delete that fails does not select the entry again later.
- **Each search list keeps the ended operations it shows struck through,** and releases them on its own next search, or when a search panel leaves its search and drops its rows. So one list's new search never makes another list's struck-through rows show deleted files as present again. Search's list and each search panel are separate lists. The queue lets go of an ended operation when no list keeps it.
- **A search list shows a taken row struck through, in gray,** with the text and icon it had, so the person sees what the delete or move took, and the selection leaves taken rows out. The list keeps gozlerd's count and positions, because gozlerd renumbers its results when its journal tail applies the delete, and a list that removed rows itself would then map positions to the wrong files. The struck-through rows go when the person searches again, or when their chunk is fetched again after gozlerd has applied the delete.
- **A rename uses Move's transfer presentation.** New folder remains outside the transfer list; the panel shows the change when the disk has it.

### Device admission

The engine's Queue admits each copy, move or synchronization transfer between one source
volume and one destination volume. It grants all of that transfer's distinct
physical devices together or none. Partitions on one disk share a device;
spanned volumes may need several. Network shares use their server identity,
and unreadable device information falls back to a separate volume identity.
The copy plan and workers are chosen for each volume pair. All pairs in one
operation share destination name ownership, including nested merged folders.

Settings > File operations > Copying sets Transfers per device to 1, 2 or 3;
the default is 1, independently of the speed preset. Increasing the limit
admits fitting work immediately. Lowering it lets existing transfers drain.
Ready work is scanned in queue order, and blocked work reserves no device,
so a later independent transfer can start. The Queue reserves no paths and
infers no copy/delete dependencies. Ordinary Delete and New folder start
independently of transfer slots. A same-volume move or rename uses admission.

Pause stops dispatch and releases each transfer's grants only after its
backend is quiet. Resume requests fresh admission. Run now yields only
conflicting transfers and returns them automatically after the priority work
ends; a manual Pause remains paused. Nested priority changes retain their
return order. Cancelled waiting workers reacquire their devices before
unfinished-output cleanup, while never-started work is discarded without a
worker. Cancellation and completion remain scoped to the whole operation.

### Interrupted operations

A crash, a power loss or a forced exit does not mean the person changed their mind, but the person must know what happened.

- **The engine saves each running operation** before it changes a file. The window gives it the file `%LOCALAPPDATA%\Gozler\operations.json`, or the same name in the `--data-dir` folder.
- **At the next start,** saved transfers are available as Interrupted in the shared pane, with Continue and Dismiss; startup never opens the pane automatically.
- **A copy or move to a new name is saved without `destination`,** under `named`, so v0.4.0, which knows no new name and needs `destination`, leaves it out instead of continuing it under the entry's old name, which Replace would overwrite.
- **Waiting operations are not saved:** they have changed no file, and the Exit question says they will not run.
- **Another window offers no file operations.** When "Allow only one Gozler window at a time" is off, a second window shows Delete, Cut, Paste, Copy to folder…, Move to folder… and New folder… disabled and accepts no drop, as it shows no tray icon, because the first window's queue owns the saved file (one queue for each user session).

### What a stopped operation leaves

The person must always know where each file is. "What a stopped operation leaves on disk", under "The copy, move and delete engine", says what each stopped operation leaves on disk. The window adds two things: a crash also leaves the operation saved as Interrupted, and transfer reports and Delete's failure dialog say how many items were done, so the person knows that a result is partial.

## The copy, move and delete engine

`gozler-fileops` copies, moves and deletes files for the window's two modes,
Commander mode and Search. This section says what the engine does and why each
method was chosen; "File operations" and "Commander mode" say what the person
sees. Every claim about another tool or a Windows API, with its source and
its numbers, is in [the research record](research/fast-copy-delete.md), and
each decision names the record's section that holds its evidence.

### Owner decisions

| Date | Decision |
| --- | --- |
| 2026-10-19 | Copy, move and delete as fast as the disks allow. |
| 2026-10-19 | A Norton Commander style mode with two panels; operations run in the background, with a queue. |
| 2026-10-19 | The UI never waits for disk work: a command replies when the person's choice is recorded, and the progress shows on the operation. |
| 2026-10-19 | Where the research does not show which method is faster, Gozler implements both and the person chooses. |
| 2026-10-19 | The person chooses the speed settings: Automatic, which stage D's measurements fill, or a preset taken from the copiers that were read, shown without the copier's name, and can change each value. |
| 2026-10-19 | **Owner ruling: resume inside a file.** A copy writes straight to the final name, with no temporary name and no rename. A file in progress when the operation is interrupted stays, and Continue resumes it at its last confirmed byte, never discarding bytes it can confirm. No file-size limit decides this. Recovery adds no work to a copy that is not interrupted, because maximum copy speed comes first. A reliable resume comes before a fast one: where a case is in doubt, the file is copied from the start or goes to the conflict choice. Which unfinished file is the operation's own is decided by the date, as FastCopy and AngelCOPY decide by dates, not by a record that would slow the copy. A move deletes a source only after its copy is complete. |
| 2026-10-19 | **Owner ruling: no Recycle Bin.** Every delete is permanent, with the fast method of D1. Gozler is the tool a person picks for speed; a person who wants the Recycle Bin uses File Explorer. |

### Decisions

Each row is settled unless it says otherwise. "Research" names the section of
the research record that holds the evidence.

| # | Question | Chosen | Not chosen | Why | Research |
| --- | --- | --- | --- | --- | --- |
| D1 | How to delete a file | Open the entry relative to its parent folder's handle, with `DELETE` and the open-reparse-point flag, and also `FILE_WRITE_ATTRIBUTES` only when the entry is read-only, because an open fails when it asks a right the file's permissions deny, while `DELETE` alone can come from the parent folder's `FILE_DELETE_CHILD`; set `FileDispositionInfoEx` with POSIX semantics and "ignore read-only". Where the file system refuses the flags (`ERROR_INVALID_PARAMETER`, `ERROR_NOT_SUPPORTED`, `ERROR_INVALID_FUNCTION`), clear the read-only attribute through `FileBasicInfo` on the same handle and set classic `FileDispositionInfo`. The first refusal on a volume is remembered for the rest of the operation, so every later entry on that volume goes straight to the classic delete | `DeleteFileW`; detecting support from the file system's name; `DeleteFileW` after `SetFileAttributesW`; `FILE_FLAG_DELETE_ON_CLOSE`; opening each entry by its full path, as rmbrr does | One call per file, the name leaves at once so the folder can go at once, and a read-only file needs no second call on NTFS, which made FastDelete 2× slower on read-only files. The open relative to the parent and the reparse-point flag are Rust's fix for CVE-2022-21658: a folder swapped for a link during the walk cannot redirect the delete, and a file symbolic link is deleted, not its target. The classic delete refuses a read-only file, which TinyTorrent2's `Payload.cpp` learned on FAT. Support depends on the Windows version and the driver, not only on the file system's name (ReFS refused it in 2020), so the refusal itself is the test; remembering it saves a failed call per file, 10,000 calls for a FAT drive with 10,000 files | Deleters (rmbrr, Rust's standard library, fs_at); Delete APIs |
| D2 | How many delete workers | A speed setting: one per logical processor in most presets; a work item is one folder, at most 256 files of it. Stage D confirmed it: on 30,000 small files with 8 processors, one per processor and 4 workers cost the same CPU cycles within 3 %, one per processor ran 13 % faster, and 16 workers cost 15 % more cycles (`docs/BENCHMARKS.md`) | One thread (`rd /s`); a fixed 8 (AngelCOPY) | rmbrr and FastDelete use one per processor; AngelCOPY found 16 no better than 8 on 10,000 files, which leaves the count open. The 256-file split lets one large folder use several workers, which FastDelete cannot | Deleters |
| D3 | When a folder is deleted | As soon as its last file and subfolder are gone, by the worker that removed the last one | One pass at the end, as AngelCOPY deletes folders deepest first after its files | A combination of two sources. The counter that deletes a folder when its last child goes is rmbrr's (`broker.rs`), but rmbrr first walks the whole tree on one thread and holds every path. The streaming walk in items of up to 256 files of one folder is AngelCOPY's (`Delete.cpp`). Together, folders go while the walk runs elsewhere, and no list of the whole tree is held; stage A's "partly deleted" test proves the combination | Deleters |
| D4 | How to list a folder | `GetFileInformationByHandleEx` with `FileFullDirectoryInfo` on a folder handle, parsed in `gozler_core::layout` | `FileIdExtdDirectoryInfo`, which gozlerd's ReFS walk parses; `FindFirstFileExW` (rmbrr, FastCopy); `NtQueryDirectoryFile` | The engine needs the name, attributes, size, date and reparse tag, not file IDs. `FileFullDirectoryInfo` gives all of them, the tag in `EaSize` for a reparse point (MS-FSCC), and every Windows file system answers it, FAT included. `FileIdExtdDirectoryInfo` is not handled by the FAT driver sample, so it would need this class as a fallback anyway | Listing APIs |
| D5 | Where an operation's totals come from | **Owner ruling: the person never waits for a count, and the work starts at once.** Prepare of a copy or a move takes its item and byte totals from gozlerd. A delete asks gozlerd for nothing at prepare: when it starts, a thread of its own asks gozlerd for its total, and the window shows it when it arrives. When gozlerd's index may hold less than the disk below a selected folder (its volume is not indexed or not live, a size is unknown, or the Include settings leave entries out on that volume), the operation has no total and shows the count of items done, and a copy has no free-space refusal at prepare. Prepare compares only the selected entries' own names with the destination folder: a selected folder that merges into a folder of its name counts as a merge, and the run applies the accepted policy to each file it meets inside, because gozlerd's results lack the creation time and reparse tag that the conflict rules read (Conflicts). The delete and the copy's run list each folder from disk as they reach it (D3, Copy step 1), so no pass counts before the work | Walking the disk to count, as File Explorer does before it starts; deleting from gozlerd's list of the entries below the selection | A count by walking makes the person wait before anything happens, and the index already holds the totals: a count of 2,156,101 entries took 0.1 s. A list from gozlerd makes the delete wait instead: gozlerd builds and sorts every result's full path before it sends the first row, which took 5.6 s for those entries, and in a deep tree each path grows with the depth, to gigabytes in gozlerd. The walk deletes from the first folder it lists, and it opens each folder relative to the one above it, so a deep folder costs the same as a shallow one. SynoSoftware/goz#84 holds a gozlerd request that would make a list fit. The visibility plan's filter would hide only the entries of folders the person may not list, which a walk with the person's token could not list either | — |
| D6 | The Recycle Bin | Not used (owner ruling) | `IFileOperation` with `FOFX_RECYCLEONDELETE`, as TinyTorrent2's `RecycleBin.cs` does | Recycling runs at File Explorer's speed and only through File Explorer's engine, which would also need its own thread, its own wait rules and its own questions | Delete APIs |
| D7 | Which copy engine | **Both, and the person chooses** (owner decisions 4 and 5): the Windows copy engine (`CopyFile2`) and Gozler's ring, combined for each case by the speed settings. The default preset, Automatic, holds Parallel copy's values, which stage D confirmed on an SSD: for one 8 GiB file the Windows engine with D9's `ioDesiredSize` cost 9 % fewer CPU cycles than the ring's SSD profile, and 8 MiB ring blocks 40 % fewer than 2 MiB blocks; stage E's 1,600 MiB file gave the Windows engine the shorter wall time in 3 of 4 runs (`docs/BENCHMARKS.md`). The After install checks replace the hard-disk, USB and share values they measure; the other presets are the configurations of the copiers that were read ("Speed settings") | One engine; a choice between whole engines only | The research does not settle it: no source compares a ring with `CopyFile2` and `COPY_FILE_NO_BUFFERING`, and no source measured a hard disk. The ring wins in theory on one hard disk with two partitions and measured +32 % on one large NVMe file; it also writes every byte itself, so a defect damages files with nothing to detect it | Copiers; What the research leaves open |
| D8 | The Windows engine's rules | Rename on one volume; `CopyFile2` across volumes; `COPY_FILE_NO_BUFFERING` from 32 MiB between local volumes; files under 32 MiB on 16 workers between volumes without a seek penalty, 2 on a USB volume or a share; one file at a time otherwise | Explorer's defaults; robocopy `/MT` | Copied from AngelCOPY's `NativeCopy.cpp`, through TinyTorrent2's `Payload::Move`: the rename, the 16 workers with 256-file items, and `CopyFile2` for shares, each measured by AngelCOPY. Not copied: for files of 32 MiB and more AngelCOPY uses its ring, and TinyTorrent2 put unbuffered `CopyFile2` there instead, with AngelCOPY's size limit, when it decided not to build a ring. Stage D measured it: for files of 32, 64 and 128 MiB the unbuffered copy one at a time cost 32 % to 45 % fewer CPU cycles than the buffered copy on 16 workers, so the limit stays at AngelCOPY's 32 MiB rather than BetterCopy's 256 MiB (`docs/BENCHMARKS.md`). BetterCopy's 2 workers for a USB bulk-only device and for a share | Copiers |
| D9 | `COPYFILE2_EXTENDED_PARAMETERS_V2::ioDesiredSize` | 32 MiB for an unbuffered copy, from Windows build 22621, where the sparse flag also starts; not set for a buffered copy | Leave it to `CopyFile2`; 8 MiB | Microsoft says it "may be reduced" and "may be ignored". Stage D measured one 8 GiB file: 32 MiB cost 25 % fewer CPU cycles than leaving it unset and ran 22 % faster; 8 MiB gave nothing. Unset, the engine's large-file copy cost more cycles than `robocopy /j` (`docs/BENCHMARKS.md`) | Copy APIs |
| D10 | Where a copy is written, and where an interrupted file resumes | At its final name (owner ruling), created only when the name is free. An interrupted file stays (What a stopped operation leaves on disk), and Continue resumes it on the ring: it compares the file with the source block by block from the start, and writes from the first block that differs (Saved operations). Replace on the ring writes over the existing file the same way. A normal copy does no extra work for this: no checkpoint, no flush, no record | A temporary name renamed when complete (rsync's default, browsers); `COPY_FILE_RESTARTABLE`, as robocopy `/z`; a checkpoint flushed as the copy runs; a check of the last block only; rsync `--append-verify`, which resumes at the destination's size, useless for a file preallocated to its full size, and verifies the whole file afterwards | No copier that the research read gives both full speed and crash recovery, so this is Gozler's own design, proven by stage E's tests rather than by another tool's use; SynoSoftware/goz#80 tracks what to revisit. FastCopy opens every file with `CREATE_ALWAYS` and AngelCOPY deletes a partial file; robocopy `/z` slows every copy, Microsoft says "significantly", and recovery must not (owner ruling); Ultracopier resumes only within one running process. The compare is correct whatever happened before it: a block is kept only when it equals the source, and the source stays open without write sharing from the compare to the end of the copy. So a gap from out-of-order writes, the zero tail of a preallocated file, a power loss, an interrupted Replace and a source changed since the crash all end at the first block that differs. A last block that matches would prove nothing about the blocks before it. The compare read 1.11 GB/s against 0.41 GB/s for a copy, so a 50 GB file stopped at 49 GB resumes in about 47 s instead of 120 s. Stage D measured the temporary name at 43 % more CPU cycles on 30,000 small files, which stage E removed (`docs/BENCHMARKS.md`) | Resuming inside a file |
| D11 | When the file list is made | When the work starts, by walking the disk as the work goes (D5) | When the person asks | An operation can wait in the queue for minutes. TinyTorrent2 rejected a design that took the names at the request, because renames still running changed them | Lessons from TinyTorrent2 |
| D12 | How paths are compared | By identity: the final path (`GetFinalPathNameByHandleW`), component by component, with `VOLUME_NAME_GUID` on a local volume and `VOLUME_NAME_DOS` on a share, which has no volume GUID path | The path text | Two strings name one folder through case, short names, mount points and junctions; a text compare would miss an overlap or move a folder into itself | Lessons from TinyTorrent2 |
| D13 | What may be deleted | The entries of a delete; a moved source after its copy is complete; an emptied source folder; a file this operation was writing when the person cancelled it; the file that Replace replaces; a link at a name that Replace replaces, never its target; the operation's own unfinished file when its streams cannot be kept; unchanged destination-only entries approved by a complete Mirror comparison, after required output is revalidated | Anything else, such as a folder a copy merged into | A path that an operation protects is never also a path it deletes; Mirror separately revalidates its approved extras | Lessons from TinyTorrent2; Synchronize folders |
| D14 | Device admission | Queue admits all distinct devices of each source/destination volume pair together or none, default 1 transfer per device and configurable from 1 to 3. Waiting work reserves nothing. Delete and New folder bypass admission; no paths are reserved | Path dependencies; HDD-only serialization; a scheduler in the window | One scheduler limits contention independently of drive class. Pause releases grants after quiescence and Resume requests admission. Run now yields only conflicting transfers | Copy behaviour |
| D15 | Where the engine runs | In the window's process, with the person's token | In gozlerd | A pipe request to an elevated service would let any local user delete files as SYSTEM | — |
| D16 | Rename on one volume | Yes, for a move | Copy and delete, as FastCopy does | A rename changes only the folder entry, so 100 GB moves in milliseconds; AngelCOPY, BetterCopy and Ultracopier rename too | Copiers |
| D17 | Our own copy loop for the Windows engine | No: `CopyFile2` | A loop of `ReadFile`/`WriteFile` | BetterCopy deleted its own loop in favour of `CopyFileExW`; the ring (D7) is the one own loop, chosen by the person | Copiers |
| D18 | Verify a copy by reading it back | Not now | FastCopy's and TeraCopy's Verify | It reads every byte twice. The ring's own tests compare hashes; a Verify setting returns when the ring shows a defect in use | Copiers |
| D19 | How an operation survives a crash | The engine saves each operation before it changes a file, and offers it as Interrupted at the next start; the person chooses Continue or Dismiss | No record, as File Explorer; continue by itself, as TinyTorrent2 does once | Without the record, the person does not know which files moved. TinyTorrent2 continues a move once by itself because a torrent must be whole; a file manager that starts a large move at sign-in surprises the person. TinyTorrent2 never repeats an interrupted delete; Gozler offers it, because the person chooses Continue and the run walks the disk again, so it deletes only what is still there | Lessons from TinyTorrent2 |
| D20 | A copy within one volume that can clone blocks | A volume can clone when its flags (`GetVolumeInformationByHandleW`) include `FILE_SUPPORTS_BLOCK_REFCOUNTING`; a volume whose flags cannot be read, such as a share, cannot. There the ring clones the blocks itself (`FSCTL_DUPLICATE_EXTENTS_TO_FILE`), on every Windows version, then copies the metadata as for any file. The Windows engine leaves the clone to `CopyFile2` | The ring's full copy; an own clone in the Windows engine too; detecting ReFS by the file system's name, which says nothing about the volume's version or about another file system that clones | A clone writes no data, so it takes milliseconds for any size, and the ring would otherwise write every byte. `CopyFile2` clones by itself from Windows 11 23H2 with the February 2024 update (KB5034848); an own clone in the Windows engine would help only Windows before that, which has no Dev Drive. The owner's Dev Drive is ReFS | Copy APIs |
| D21 | A link inside a copied tree | Copied as a link and not entered: a junction or symbolic link becomes the same link at the destination | Follow it, as robocopy does without `/XJ` or `/SJ` | Following a link copies data from outside the selection and can loop forever; the copy then also differs from its source | Copy APIs |

### What an operation receives

Full paths: the active panel's selection, or Search's selection, which is
the files of one moment (window rule 10).

- **The engine reads each entry's kind itself.** It opens every selected
  entry with the open-reparse-point flag and reads its attributes and
  reparse tag (`FileAttributeTagInfo`), because Search's selection holds no
  reparse tag, and a junction taken for a folder would be entered.
- An entry below another selected folder is dropped; it goes with that
  folder. Two entries with one identity, such as one folder reached through
  two junctions, are one entry.
- For a copy or move, a destination inside a selected folder is refused, as
  File Explorer refuses it.
- An entry moved to the folder it is already in is left alone.
- **A selected entry that no longer exists** is not a refusal, because the
  selection is the files of one moment. A delete counts it as deleted. A
  move lists it as a failure, except at Continue, which counts it as moved
  when something exists at its final name, or for Keep both at one of its
  numbered names: after a crash Continue meets exactly these entries,
  because they already moved, while on a first run the engine knows it
  moved nothing, so a source gone meanwhile is never reported as moved. A
  copy lists it as a failure.
- A selected entry whose identity cannot be read, for example a file whose
  permissions deny the person every right, is listed as a failure with
  Windows' error and left out; the operation does not touch it. An unreadable destination identity
  refuses the operation, as does an unreadable standalone rename source.

Each test uses the identity comparison of D12.

**Which opens follow links.** A selected entry and every entry the walk
meets are opened with the open-reparse-point flag, so the engine never acts
on a link's target by accident. The destination is a folder the person
chose, so it is opened following links, and its identity is its target's:
a destination `C:\out` that is a junction to `C:\proj\build` is inside
`C:\proj`, and a copy of `C:\proj` there is refused.

**A folder with a reparse tag that is not a name surrogate,** such as a
OneDrive folder, is listed through a second handle opened without the
flag, after a check that both handles have the same volume serial number
and file ID (`FileIdInfo`), because a file ID is unique only on its own
volume. A folder of
Files On-Demand gets its children from the cloud filter, and a listing that
bypassed the filter could see an empty folder, which a move would then
delete. The check keeps a folder swapped for a junction between the two
opens from redirecting the walk.

**One volume or two** is decided at prepare, from the identities of the
sources and the resolved destination, because it decides whether the
transfer moves bytes, its device admission and its free-space check. A
rename that fails at run time with `ERROR_NOT_SAME_DEVICE` is a listed
failure, never a copy that nothing waited for or counted.

### Delete

1. A work item is one folder. A worker lists it (D4), deletes up to 256 of
   its files (D1), and queues its subfolders as new work items (D2). A work
   item carries its folder's open handle, not a path, so every open below
   it is relative to that handle (D1) and a swap of a parent for a link
   cannot redirect it.
2. Each folder counts what is still open below it: its own listing until
   the listing ends, each item of its files until the item is done, and
   each subfolder until it is deleted. A folder whose count reaches zero is
   deleted, and its parent's count goes down (D3); a folder that keeps a
   failed entry is not tried, so it and its parents stay. rmbrr's counter was
   complete before its first delete, because rmbrr walks first; with the
   streaming walk, a count of the subfolders alone would reach zero while
   other workers still delete the folder's files.
3. **A link is deleted, never entered.** A folder whose reparse tag is a name
   surrogate (`IsReparseTagNameSurrogate`: a junction, a symbolic link or a
   mount point) is deleted as a link, because entering it would delete files
   outside the selection. A cloud placeholder's tag is not a name surrogate,
   so a OneDrive folder is entered.
4. **Short retries.** A delete refused with a sharing violation or a lock
   violation, and a folder that is still not empty, are tried again, the
   first wait 1 ms and each wait 1.2 times longer than the one before,
   until the waits add up to 200 ms, as rimraf does; scanners and indexers hold files open for a
   moment. Access denied is a failure at once: D1 already deletes read-only
   files, so what remains is a permission, which does not change in 200 ms,
   and retrying it would cost 125 s on 10,000 files the person may not
   delete. rimraf and Rust's standard library do not retry it either.
5. **One failure does not stop the rest.** A file that cannot be deleted is
   recorded with Windows' error, and its folders stay. A file the walk
   listed that is already gone counts as deleted.

### Copy

1. **The walk streams,** as the delete does and as AngelCOPY's `WalkStream`
   does. It runs when the copy starts (D11), on the same workers as the
   files, so the worker count of the volume pair bounds both. A work item
   is one source folder. The worker that lists it (D4) creates its
   destination folder, reads that folder's existing names once (step 2),
   queues its subfolders as new work items, and hands its files on: up to
   256 files under the large-file limit as one item for the workers, and
   each large file to the one-at-a-time lane. Between volumes without a
   seek penalty the lane runs beside the workers. The
   worker hands direct large files to that lane before it opens and creates
   the subfolders, so folder setup does not leave the large lane idle
   ([SSD measurements](BENCHMARKS.md#ssd-early-dispatch-before-and-after)). When
   either volume has a seek penalty, large files join the workers' queue,
   so one file moves at a time, because a second stream makes a hard disk seek.
   Copying starts while the walk still runs, and memory holds only the
   folders in flight. A name-surrogate entry is copied as a link and not
   entered (D21).
2. **Conflicts.** One listing of each destination folder that already
   exists gives the set of its names, folded as Windows compares them: each
   UTF-16 unit upper-cased one to one (`CharUpperW` on the unit, or a table
   in `gozler-fileops`), with no normalization. Neither
   `gozler_core::fold`, which normalizes, nor Rust's `to_uppercase`, which
   turns `ß` into `SS`, matches NTFS, and a false match with Skip would
   leave a file uncopied. Several workers hand files into one destination
   folder, for example the destination of a Search selection from many
   folders, so each set has its own lock. The worker that hands a file on
   decides its final name, its number included, under that lock and adds
   the name to the set, so two workers never give one number. A conflict,
   and Keep both's first free number, are lookups in that set, not one
   probe per name: a folder that holds `index (2).html` to
   `index (500).html` costs no 500 probes. A name taken after the listing is
   caught when the file is created (step 5). Two selected entries with one name
   are also a conflict. Folders with one name merge, as in File Explorer,
   but never into a junction or folder link at the destination: that is a
   listed failure, because writing through it would write outside the
   destination. Prepare applies the same rules to the selected entries'
   own names without walking a folder that merges
   (D5).
3. **Free space.** A copy that needs more bytes than the destination volume
   has free is refused at prepare when gozlerd gave its total (D5), as File
   Explorer refuses it. The run
   compares prepare's byte count with the free space again before it
   starts, and does not count again. Within one volume that can clone
   blocks the check is skipped for both engines, because both clone there
   (D20). A disk that fills during the copy, also on a Windows that does
   not clone, stops it as step 9 says.
4. **Folders** are created with `CreateDirectoryExW` and the source folder
   as the template, which keeps its attributes, each by the one worker that
   lists its source folder, so no two workers create one folder. Creating a
   folder and opening a file use the delete's short retries (Delete, step
   4), and a folder create also retries `ERROR_PATH_NOT_FOUND` when this
   operation made its parent: AngelCOPY added that after about 30 folders
   on a file server returned it and their subtrees were lost.
5. **Files** are copied by the engine that the speed settings choose for
   their case (D7), straight to their final name (D10). The file is created
   only when the name is free: the ring creates it with `CREATE_NEW`, and the
   Windows engine passes `COPY_FILE_FAIL_IF_EXISTS`. When another file took
   the name after the walk, the create fails with `ERROR_FILE_EXISTS` or
   `ERROR_ALREADY_EXISTS`. It fails with `ERROR_ACCESS_DENIED` both for a
   hidden or read-only file and for a folder whose permissions refuse the
   create, so on that code the engine asks whether the name exists
   (`GetFileAttributesExW`). A name that exists is a conflict with someone
   else's file, never deleted (D13): the operation's choice applies (Keep
   both takes the next free number, Replace writes over it), and with Skip
   the file is listed as skipped. The accepted default updates changed files. Otherwise the error is a
   failure. A file of one of two selected entries with one name never
   replaces the other's copy (Conflicts).
6. **Replace** clears the read-only, hidden and system attributes of the
   file it replaces and copies with the engine the speed settings choose
   for the file. The ring opens it in place without following a reparse
   point and writes over it as Continue resumes a file (D10). The Windows
   engine deletes the entry at the name without following it (D13) and
   copies with `COPY_FILE_FAIL_IF_EXISTS`, which is always set; an entry
   that takes the name between the delete and the copy makes the copy fail
   and be listed. At Continue the ring also takes a Replace whose file was
   last written at or after the operation's start, which is the Replace
   this operation was writing, so it resumes; the person chose Replace for
   that file, so a wrong guess costs time, never a file. Giving every
   Replace to the ring would copy many small files one at a time. The old
   content is gone from the moment Replace starts, as with File Explorer,
   robocopy and FastCopy.
7. **A link at a destination name is never written through.** Every open
   of an existing destination file uses `FILE_FLAG_OPEN_REPARSE_POINT`. A
   reparse point found there is never the operation's unfinished file
   (Saved operations); Replace deletes the link itself through its handle
   and creates the file with `CREATE_NEW`. Writing through a link would
   write outside the destination, wherever the link points.
8. **A file and a folder with one name** are a listed failure whatever the
   choice, never replaced, as File Explorer refuses them.
9. **Failures.** A file that fails is recorded with Windows' error, and the
   copy goes on. A full destination disk stops the copy, because every later
   file would fail too.

Hard links in a copied tree become separate files, as with File Explorer and
robocopy.

#### The Windows engine

| Case | Method |
| --- | --- |
| Both volumes local, file of 32 MiB or more | `CopyFile2` with `COPY_FILE_NO_BUFFERING`, one file at a time |
| Both volumes local without a seek penalty, file under 32 MiB | `CopyFile2` on 16 workers, 2 when either volume is on USB, each taking up to 256 files of one folder |
| A network share at either end | `CopyFile2`, buffered, on the share's workers (2 in Parallel copy, as BetterCopy uses), so the round trips of one file overlap another's. Shares use their server identity for device admission, with the same transfer limit as local disks |
| Every other case | `CopyFile2`, buffered, one file at a time. A volume whose seek penalty is unknown counts as a hard disk |

The 32 MiB limit and the worker counts are speed settings; the table shows
the values of the Parallel copy preset. A selection from several volumes,
such as a Search selection, copies with the method of the slowest volume
class involved.

A link, an entry whose reparse tag is a name surrogate, is copied as a link
(D21), with `COPY_FILE_DIRECTORY` added for a folder. A symbolic link is
copied with `COPY_FILE_COPY_SYMLINK`, which needs the right to create
symbolic links and otherwise fails with `ERROR_PRIVILEGE_NOT_HELD`, a listed
failure. Every other link, a junction or a mount point, is copied with
`COPY_FILE_OPEN_AND_COPY_REPARSE_POINT` (Windows 10 build 19041 and later),
which does not need that right. A symbolic link never gets
`COPY_FILE_OPEN_AND_COPY_REPARSE_POINT`: without the right, `CopyFile2`
then succeeds without the link, copying a file link's target's data and
making a folder link an empty folder ("Not verified" has the run). Every
other file is copied without either flag, because a OneDrive
placeholder, a compressed (WOF) or a deduplicated file also has a reparse
point, and copying that point instead of reading through its filter gives
a file without its data. A sparse file stays sparse with
`COPY_FILE_ENABLE_SPARSE_COPY` on Windows build 22621 and later when the
destination supports sparse files. `CopyFile2` keeps alternate data streams
such as `Zone.Identifier`, attributes and dates, and tries ODX and SMB
offload by itself: its parameter page says that only its low free space
mode does not. Between shares on one server AngelCOPY measured
`CopyFileExW` 30 times faster than its ring.

Rules that TinyTorrent2's `Payload.cpp` and the copiers learned:

- **The 32 MiB limit is AngelCOPY's; BetterCopy uses 256 MiB** for
  `COPY_FILE_NO_BUFFERING`. Stage D measured both, and 32 MiB stays (D8).
- **The seek penalty and the bus type** come from
  `IOCTL_STORAGE_QUERY_PROPERTY` on a volume handle opened with no access
  rights, as BetterCopy reads them, so no administrator right is needed.
- **A volume's class** is one of four, and the worker counts, the ring's
  profile read it: a share, by its path; a
  hard disk, with a seek penalty or an unknown one; USB, without a seek
  penalty and a USB bulk-only device by BetterCopy's test in
  `profiler.rs`; otherwise an SSD.
- **`COPY_FILE_REQUEST_COMPRESSED_TRAFFIC` is not set:** compression helps
  only on a slow link and costs CPU on a fast one. Robocopy leaves it to its
  `/compress` switch.
- **The Windows build** comes from `RtlGetVersion`, because
  `GetVersionEx` reports the build that the executable's manifest declares,
  not the running one. The sparse flag is passed only where it exists:
  Microsoft does not document what an older Windows does with it.
- **The free space a copy needs** counts a sparse file that stays sparse at
  its allocated size (`GetCompressedFileSizeW`), and every other file at its
  full size, because a sparse file copied where it cannot stay sparse takes
  its full size on the destination.
- **Progress is limited to the file's size:** `CopyFile2`'s
  `uliTotalBytesTransferred` also counts the alternate data streams.
- **Long paths:** every path goes to Windows with the `\\?\` prefix, or
  `\\?\UNC\` for a share, because Windows takes a path longer than
  `MAX_PATH` only with it. A volume root goes without the prefix, because not
  every volume query accepts it.

#### The ring engine

Gozler's own copy loop, from FastCopy's and AngelCOPY's designs (research,
"Copiers"). Their code is not used: FastCopy v3 is GPLv3 and AngelCOPY has no
licence.

| Setting | Hard-disk profile (FastCopy) | SSD profile (AngelCOPY) |
| --- | --- | --- |
| Applies to | every file, when either volume has a seek penalty | files of 32 MiB and more, when neither volume has one; smaller files use the Windows engine's workers |
| Block | 2 MB, FastCopy v5's default | 8 MiB |
| Blocks in flight | 8 reads and 8 writes, as FastCopy's help recommends | 8 in total, each reading or writing, as AngelCOPY's `RingCopyFile` |
| Buffer | 256 MB, shared by all files | the 8 blocks, 64 MiB |
| One physical disk | read until the buffer is full, then write all of it | not used: an SSD has no seeks |
| Two disks | reader and writer run at the same time | each block is written when it is read |
| Files opened ahead | 16 threads, up to 256 open files | one file at a time |

The 32 MiB limit, the blocks, the blocks in flight and the buffer are speed
settings; the table shows the values of the Ring copy preset.

Both profiles:

- **Unbuffered:** sources open with `FILE_FLAG_NO_BUFFERING`; destinations of
  64 KB and more too. Smaller destinations in the hard-disk profile go
  through the cache, because a small unbuffered write waits for the disk.
  Each block starts at an address aligned to the larger sector size of the
  two volumes.
- **Overlapped** I/O on a completion port for files of 512 KB and more;
  smaller files use synchronous I/O.
- **A ring error falls back.** Any error of the ring other than a cancel, a
  sharing violation or a full disk copies that file again with the Windows
  engine and counts it in the result, as AngelCOPY's `CopyOneBig` does, so
  a file the ring cannot copy, such as one on a disk with an unusual sector
  size, is still copied. A sharing violation stays a listed failure,
  because it is how the ring refuses a file another program is writing; a
  full disk stops the copy (Copy, step 9).
- **FAT destinations** are written unbuffered from 128 KB, not 64 KB,
  FastCopy's limit for FAT.
- **The end of file is set first,** to the size rounded up to a whole
  sector, as FastCopy and AngelCOPY do, and then set to the true size after
  the last block. A write that extends a file runs synchronously (Microsoft
  KB 156932), so growing the file with each write would serialize the
  writes in flight. Ultracopier reserves space with `FileAllocationInfo`
  instead, so that a partial file stays short and a check of the size alone
  cannot take it for complete. Gozler's check compares the last write time
  too, which both engines set last, and Continue finds where a file stopped
  by comparing it, not from its size.
- **Resume and Replace (D10)** open the existing destination in place,
  without following a reparse point, and the source. Two reader threads
  compare them with the profile's blocks from the start, up to the smaller
  of the two sizes. A block is kept only when it is whole and equal; the
  source's last, shorter block is kept when the destination holds all of
  it. The ring then sets the end of file to the kept length, sets or
  clears the sparse mark to the source's, and copies from there as for a
  new file, so the reads and writes stay aligned. A destination with an
  alternate data stream or extended attributes, or whose streams cannot be
  read, is deleted and copied from the start, because writing over it in
  place would keep a stream the source may lack, such as an old
  `Zone.Identifier`. A compare that fails gives the file to the Windows
  engine. Every unfinished file resumes on the ring, also one the Windows
  engine wrote, except the files the ring hands to the Windows engine
  (below), which `CopyFile2` copies again from the start, because it
  cannot start inside a file; for a network end that costs no time, as the
  compare would read the written part across the network. A clone (D20)
  starts again, because it writes no data and takes milliseconds.
- **Sources share read and delete, not write.** A file that another program
  has open for writing then fails with a sharing violation and is listed,
  instead of being copied as a mix of old and new bytes under its final
  name.
- **Sparse files:** the reader reads only the allocated ranges
  (`FSCTL_QUERY_ALLOCATED_RANGES`). On a destination that supports sparse
  files, the writer marks it sparse (`FSCTL_SET_SPARSE`), sets its size, and
  writes only those ranges; elsewhere the holes are written as zeros.
- **Metadata:** every alternate data stream and the extended attributes go
  through `BackupRead`/`BackupWrite`, skipping the main stream; attributes and
  the creation, access and write times through `FileBasicInfo`. Security
  descriptors are not copied, as `CopyFile2` does not copy them by default.
  This is FastCopy's method. AngelCOPY's ring copies only times and
  attributes and so loses `Zone.Identifier`, the mark that makes Windows
  warn before it runs a downloaded file.
- **The clone (D20):** within one volume that can clone blocks the ring marks
  the destination sparse, sets its end of file to the source's size, and
  clones the data in ranges under 4 GB, the largest range Windows accepts.
  It then clears the sparse mark when the source is not sparse. This is
  what `CopyFileExW` does: ReFS reserves the whole size of a file that is
  not sparse, so a clone into such a file needs free space for all of it.
  When the volume cannot hold the whole size, ReFS refuses to clear the
  mark and the copy stays sparse, as `CopyFileExW` leaves it. The data
  stream is then complete, and the metadata follows as for a copied file. A
  refused clone falls back to the ring's copy and is counted in the result,
  so a clone that never runs shows.
- **Handed to the Windows engine:** a network end, an unbuffered open that a
  file system filter refuses, any entry with a reparse tag, and an EFS file.
  The walk opens entries without following reparse points, so a ring read
  of a OneDrive placeholder or a compressed file would bypass its filter. AngelCOPY measured its
  ring 30 times slower than `CopyFileExW` between two shares on one server,
  and about 2 times slower from a share to a local disk, because buffered
  reads from a share read ahead.
- **Progress** counts each byte once, when it is written. On one hard disk
  the progress moves in steps of up to 256 MB.
- The buffer exists only while a copy runs.

#### Speed settings

The two engines differ only for large files between SSDs and when a hard
disk is at either end; everywhere else both do the same thing. The speed
settings decide those cases and the numbers both engines use. Every copy,
move and delete receives them, and a saved operation keeps them, so
Continue runs with the same ones. They are one typed value whose
constructor refuses a value out of range, so a bad value from the Settings
page or from `ui.json` never reaches a copy.

| Setting | Range | What it decides |
| --- | --- | --- |
| Large-file limit | 1 to 1,024 MiB | from this size, a file between two local volumes is large: the Windows copy function copies it unbuffered and one at a time, and between SSDs the next row may give it to the ring |
| Large files between SSDs | Windows copy function, unbuffered; or the ring's SSD profile | the first case where the engines differ |
| A hard disk at either end | Windows copy function; or the ring's hard-disk profile, for every file | the second case |
| Workers for files that are not large | 1 to 64, set separately for SSD, USB, hard disk and share | how many files Windows' copy function copies at once |
| Ring, SSD profile | blocks of 64 KiB to 64 MiB, a power of two; 1 to 64 in flight | a power of two keeps every block aligned to any sector size |
| Ring, hard-disk profile | blocks as for the SSD profile; 1 to 64 reads in flight and as many writes; a buffer of 16 to 1,024 MiB | |
| Delete workers | 1 to 64, or one per logical processor | D2 |

**Presets.** The Settings page offers these. A preset sets every value;
changing one value makes the choice Custom. A preset is shown by what it
does, not by the copier it comes from, because Gozler uses none of their
code and a copier's name would promise that copier's exact behavior.

| Preset, as shown | Large-file limit | Large files between SSDs | Hard disk | Workers SSD / USB / hard disk / share | Ring SSD | Ring hard disk | Delete workers | From |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Automatic (default) | 32 MiB | Windows | Windows | 16 / 2 / 1 / 2 | 8 MiB × 8 | 2 MB × 8, 256 MB | one per processor | Parallel copy's values, which stages D and E confirmed on an SSD (D2, D7, D8); the After install checks replace the hard-disk, USB and share values |
| Parallel copy | 32 MiB | Windows | Windows | 16 / 2 / 1 / 2 | — | — | one per processor | TinyTorrent2's `Payload::Move`; share workers from BetterCopy; delete from rmbrr |
| Ring copy | 32 MiB | ring | ring | 16 / 2 / 1 / 2 | 8 MiB × 8 | 2 MB × 8, 256 MB | one per processor | AngelCOPY's ring and FastCopy v5's hard-disk mode |
| Ring for large files, many small files at once | 32 MiB | ring | Windows | 16 / 16 / 16 / 16 | 8 MiB × 8 | — | 8 | AngelCOPY 1.6.0 |
| Windows copy function, unbuffered from 256 MiB | 256 MiB | Windows | Windows | 16 / 2 / 1 / 2 | — | — | one per processor | BetterCopy |

A dash is a value the preset does not use; it keeps the Automatic value.
Three copiers have no preset, because the engine cannot reproduce them:
FastCopy, which on an SSD runs its reader and writer at once over its
large buffer, a mode the ring has only for hard disks; Ultracopier's
buffered ring; and TeraCopy, whose method is not published. FastCopy's
hard-disk mode is in Ring copy.

### Move

- **On one volume** each selected entry is renamed (`MoveFileExW` without
  `MOVEFILE_COPY_ALLOWED`), a whole folder in one call. No walk runs, except
  into folders that merge: a folder of that name at the destination makes
  the move enter both, rename each child into the existing folder, merge
  child folders the same way, and delete each emptied source folder.
  Prepare counts each merge as it does for a copy (D5).
- **Across volumes** it is a copy as the speed settings choose, and each source
  file is deleted when its copy is complete, and only when it is
  still the file that was copied: the same file ID, size and last write
  time as before the copy. Otherwise it stays and is listed, because the
  Windows engine copies a file another program has open for writing, and
  a program can replace a file by renaming a new one over it. A source folder is
  deleted when it is empty (D3). A cancel takes effect before a file's
  copy is complete or after its source's delete, never between them, so a
  cancel or a failure leaves each file in one place.

### Conflicts

Copy/Move captures its policy before preparation and runs without conflict questions.
Settings and the command dialogs share the saved policy; Paste and Drop snapshot
it on acceptance. Late intake cannot change it.

| Policy | Existing same-name file | Second selected entry of one name |
| --- | --- | --- |
| Update changed files (default) | Skip equal size/time; replace otherwise, including a newer destination | Number it |
| Update newer files | Replace only if the source last-write time is later | Number it |
| Keep both | First free numbered name | Number it |
| Skip existing files | Skip and retain source | Skip |

Equal metadata does not verify contents. A skipped move retains its source.
A file/folder mismatch remains a failure. The policy also governs entries
appearing after the destination listing; no-follow placement checks still
protect link targets. Interrupted legacy requests retain an explicitly saved
Replace, Skip or Keep both. A missing legacy choice uses Update changed files
after recovery checks for completed and unfinished outputs.

- Two selected entries never replace each other: on a move that would delete
  one of the person's files, and on a copy the result would depend on the
  order. This holds inside two selected folders with one name, which merge
  when the destination already has a folder of that name: a file there
  created at or after the operation's start is the other entry's copy and
  is treated as the second entry of one name; an older file is the
  person's and gets the choice.
- A copy into the entry's own folder gets a number without a question, as
  in File Explorer, because Replace would replace the source with its own
  copy.

### What a stopped operation leaves on disk

| Operation | Stopped by | Leaves |
| --- | --- | --- |
| Copy | Cancel or a failure | the files copied so far and the folders made; the files in progress are deleted |
| Copy | Exit, power loss or a crash | the files copied so far and the folders made, and the files in progress, unfinished at their final names, until Continue resumes them; Dismiss leaves them, and the Interrupted row says so. At Exit the queue's `close()` cancels the run without stopping `CopyFile2`, which deletes its file when stopped, so a Windows-engine file keeps growing until the process ends |
| Move on one volume | anything | each entry in one place: a rename is complete or did not happen |
| Move across volumes | Cancel or a failure | each file in one place: moved, or still at its source |
| Move across volumes | Exit, power loss or a crash | each file in one place, except the file in progress, which is whole at its source and unfinished at the destination, and a file whose source delete had not run, which is at both |
| Delete | anything | the entries not yet deleted |

An unfinished file has the time it was last written, not its source's,
because both engines set the times last, so a copy that compares size and
date, Gozler's or robocopy's, never takes it for complete. A Replace that
stops has already overwritten the file it replaces (Copy, step 6).

### The interface

The window talks to one object, the engine's queue. It creates the queue at
start with two things: the file where operations are saved, and a wake
function that the queue calls after each change of an operation's state.
The window's wake function only posts a message (window rule 5) and then
reads the states. It posts only while a flag that the window clears in
`WM_DESTROY` is set, because at Exit engine threads can outlive the window
by up to 3 seconds, and a message to a destroyed window handle can reach a
new window that reuses it.

1. **Prepare** takes the kind (copy, move, synchronize, delete or new folder), the
   speed settings, the sources and the destination, and returns an id at once.
   On a queue thread it applies "What an operation receives" and ends
   prepared, or ended with a typed refusal: a destination inside a source,
   a destination that does not exist, a destination whose identity cannot
   be read, or too little free space. A selected entry that no longer exists counts as
   "What an operation receives" says. A
   prepared operation holds identities (D12), volume-pair transfers with
   their copy plans and distinct devices, and shared operation-wide
   destination name ownership. A copy or a move takes its items
   and bytes from gozlerd, and a delete takes its items from gozlerd while
   it runs; neither walks to count them (D5), so prepare ends at once; with
   no total, the progress is the count of items done. It keeps no conflict
   counts or list of files (D11). Synchronization instead opens its two roots
   and prepares a comparison; enumeration starts only after device admission.
2. **Submit** takes the id of a prepared operation whose request already
   holds its policy. The queue starts the operation as soon as D14
   lets it.
3. **The operation runs** on its own thread: it checks the free space again
   (Copy, step 3), walks as it copies, applies "Conflicts", and ends complete, cancelled, or stopped
   by a full disk, with the counts of what was done and each failure or
   skipped file with its path and Windows error, in the order they happened.

**Each operation's state** is one of: preparing; prepared, waiting for the
submission; waiting for device capacity, with its blocker and totals; running;
compared, waiting for explicit synchronization commitment;
ended, with its result; or interrupted. A running operation carries its
counts of items and bytes, its totals, whether it only renames, whether it
is paused, and its current file. An ended operation stays until the window
removes it. The queue is the one owner of which operations exist.
`Queue::operations()` lists each with its request: the running ones in the
order they started, then the waiting ones in run order, then the rest in
the order they were prepared. Snapshots hold a weak comparison reference;
the dialog releases its strong preview through Queue's worker so destroying
a large preview never holds the window.

**Synchronize** compares complete source and destination trees under the same
device grant. It rejects identical/nested roots and unknown or name-surrogate
subtrees. The retained comparison holds source fingerprints and approved
destination-only candidates. It releases its grant for review; explicit
commitment reacquires the devices and uses the existing Update changed files
copy executor, with totals from comparison rather than another daemon read.
Mirror verifies required output before removing extras. Relative no-follow
opens, matched cloud-filter views and chosen-root identity checks prevent
unknown or redirected output from authorizing deletion. Candidate removal
checks source absence and unchanged identity/metadata on the same handle it
deletes; extra folders are removed only if empty. It never recursively deletes
new children. Failed required copies or cancellation prevent starting the
deletion phase. These are
point-in-time checks; ordinary copy remains path-based and does not lock
intermediate names. Overall synchronization progress uses processed entries,
including approved deletions; current-copy progress still uses bytes.

**Pause** closes an operation's transfer gates. Each transfer keeps its
grants until its workers and native I/O are quiet, then releases them.
Resume requests fresh admission. Cancelled previously admitted work must
reacquire its devices before cleanup. Never-started work needs no worker.
The ring pauses between blocks; CopyFile2 pauses through its native return;
other workers pause between entries. Shared destination initialization
publishes before releasing its initializer's grant; waiting consumers are
idle and reacquire their gate before returning.

**The current file** is the file the operation most recently started, with
its destination, its bytes copied and its size; for a delete, the entry
being deleted, with no destination and 0 for both. With several workers it
is the one that started last.

**Taken entries.** The queue gives the sources of the deletes and moves
that wait, run or ended, and which of them ended. A path is taken when it
equals or is below a source of a waiting or running one, or of an ended one
that the list keeps, compared as Windows compares names, ignoring case. The
window asks this, and nothing else, to leave those entries out of the panels
and show them struck through, in grey, in the search results.
Each list keeps the ended operations it still needs
("The person never waits for the disk"): a
folder panel keeps an operation whose end makes it read its folder
again until its first complete listing started after the end, because the
change notifications and the end arrive in no fixed order; a search list
keeps an ended operation only when, at its end, the operation takes a row
of the list's fetched results, and then until its next search, because
gozlerd applies a large delete's journal records for some time after the
delete.
The window removes an ended operation from the queue when no list keeps it.
This display rule on the original path text creates no scheduler dependency.

The test runs for every row Windows paints, about 50 rows per paint, so it
must not compare a row with every taken path: 100,000 deleted rows would
cost 5 million compares per paint. The queue keeps the taken sources in a
hash map from each folded full path to the operations that take it, and a
row is taken when its folded path or one of its parent paths is in the map
for an operation that takes it from that list. A parent path is looked up only
when a taken path has its length, so most rows need one or two lookups.
At each wake the window takes a snapshot of the map (`Arc`), so a paint
never waits for the engine's lock while a worker holds it, and decides from
that snapshot which lists keep each operation that ended.

**Identity comparisons** use a trie for selection deduplication,
entries below another selected folder and destinations inside a source.
Device admission creates no path dependency.

**Cancel** takes an id. **Cancel all** atomically cancels every accepted
copy, move and synchronization, including pending source intake, before scheduling other
work; ordinary Delete and dormant Interrupted work are independent.
An operation shows Cancelling until its workers and required recovery
removal finish. A running one stops within one `CopyFile2` chunk (its callback returns
`COPYFILE2_PROGRESS_CANCEL`), one ring block, or one delete and its 200 ms of
retries. A `CreateFileW` blocked on a share that stopped answering is ended
with `CancelSynchronousIo` on its thread, as `gozler_winfs`'s
`cancel_synchronous_io` does for gozler's other blocked calls. A read in
flight inside `CopyFile2` is overlapped (its page says only the low free
space mode uses no overlapped I/O), so it waits for the share's timeout.

**One queue per user session.** Only the window that owns the
single-instance mutex creates a queue. Another window, which Gozler opens
when "Allow only one Gozler window at a time" is off (DESIGN.md, "Window
lifetime"), offers no file operations, as it shows no tray icon: two queues
would replace each other's saved file, and an interrupted operation would
then be missing at the next start.

**Saved operations (D19).** Before a running operation changes its first
file, the queue writes its kind, its speed settings, its sources, its
destination, its conflict choice and the time it first started to the saved
file: to a temporary file, then renamed over the old one, as `ui.json` is
written. The start time comes from `GetSystemTimeAsFileTime`, which never
runs ahead of the clock that stamps files, and Continue keeps the first
run's. It removes the operation when it ends, cancelled included.
A waiting operation is not saved, because it has changed no file. The
operation's own thread, and for a removal the window asked for, a thread
of the queue, writes the file, because the file lists every source of
every saved operation and the window's calls wait for no disk write. A
failed initial save refuses the operation before mutation; failed removal
is reported without reducing the count of successful file operations.
Explicit cancellation makes the id ineligible for a delayed first save.
Exit keeps the process alive for the background persistence result, also
when setup ended the message loop first. When the queue is created, each
saved operation becomes an interrupted one.

Synchronization saves only roots and Mirror intent, never the approved
deletion list. Continue makes a fresh comparison for explicit review and
commitment before further changes. Copy/Move retain the recovery rules below.

- **Continue** prepares and submits it again with two more rules, which its
  prepare applies while it counts, from the destination listing it reads
  for conflicts, so that a file found done counts no bytes and an unfinished
  file counts only what it lacks: otherwise a 100 GB copy that crashed at
  90 GB on a drive with 120 GB free could not continue.
  - **Done:** a file at the destination's final name, or for Keep both at
    one of its numbered names, with the source's size and last write time.
    The times compare within 2 seconds, or within one hour and 2 seconds
    after a daylight saving change, because FAT keeps them to 2 seconds in
    local time, as TinyTorrent2's `Payload::IsCopy` does.
  - **Unfinished:** a file there that this operation made and did not
    finish: it is not done, it is not a reparse point (Copy, step 7), no
    other selected entry has its name, and the listing's creation time is
    at or after the time the operation first started, less FAT's 2
    seconds. It is resumed on the ring (D10), never put to the conflict
    choice. The creation time decides, not the last write time, because
    the person's own file that the first run skipped and that they edited
    during the interruption has a new last write time but its old creation
    time. Where another selected entry has the name, either copy could be
    the other's, so the file goes to the conflict choice, and Replace and
    Keep both number the copy. Windows can give a new file the creation
    time of a file of that name deleted a few seconds before (file name
    tunnelling); such a file goes to the conflict choice too, which costs a
    question, never a file, and an interrupted Replace still resumes
    (Copy, step 6).
  - A file that another program creates at the name after the operation
    started is taken for the operation's own and resumed over; it ends
    equal to the source. The date cannot tell them apart without a record
    of each file, which would slow every copy (SynoSoftware/goz#80).
  - An operation saved before the start time was recorded finds no file
    unfinished; every file that is not done goes to the conflict choice.
- **A move that continues keeps the source of a file found done** and lists
  it in the result. Equal size and time are a guess, and TinyTorrent2 trusts
  that guess only because a hash check of the torrent follows; Gozler checks
  no hash (D18), so it never deletes a source on it.
- **Dismiss** removes it.
- The file's keys are a saved format: an older entry that lacks a key keeps
  its old meaning, as `AGENTS.md` asks of the wire.

**Rename and new folder** retain their existing engine calls. Same-volume
moves and standalone rename require admission; standalone rename refuses an
unreadable source identity. Ordinary Delete and New folder bypass admission.
Rename uses MoveFileExW without replacement and New folder uses
CreateDirectoryW, so an existing name is refused.

**Progress** is atomic counters of bytes or entries and the current file,
which the window reads about twice a second. A file the run counts without
copying it, such as one that Continue finds done or one it skips, adds its
bytes to a counter of their own, so the progress reaches the total and the
speed counts only the bytes copied. The window never shows more done than
the total, because gozlerd's total can miss entries, such as those a
delete removed before gozlerd answered.

**Threads.** The queue owns every engine thread: one for each preparing or
admitted transfer, and the workers of each transfer, which are scoped threads
that the run creates and joins. Dropping the queue cancels every operation
and joins every thread, so no engine thread outlives it. At Exit the window
signals the operations to stop and waits off the window thread for recovery
state to be written; the process does not join all engine workers. An
operation that had not ended stays in the saved file
and shows as Interrupted at the next start, which is the truth. The process
keeps its single-instance mutex until it ends, because a second Gozler
started earlier would read and rewrite the same saved file.

**Handles.** Every handle the engine opens shares delete, so its own handles
never make a delete fail. TinyTorrent2's mark writer opens without
`FILE_SHARE_DELETE`, and a delete of the same file then fails.

### Where the code goes

- **`gozler-winfs`:** every Win32 call, as safe functions: open an entry
  relative to a folder handle, list a folder, delete one entry, rename,
  `CopyFile2` with a progress closure that can cancel, clone a file's blocks,
  create a folder from a template, the final path, a volume's class and
  disks and free space, and the ring's unbuffered, overlapped reads and
  writes, sparse ranges and backup streams. An overlapped read or write owns
  its buffer until the I/O completes, and waits for that completion when it
  is dropped, because Windows writes into the buffer until then.
- **`gozler-core`:** the `FileFullDirectoryInfo` parser, and the parser of
  `BackupRead`'s stream headers (`WIN32_STREAM_ID`), each with a
  test that malformed bytes return an error, because `gozler-winfs` parses
  nothing. `deny.toml`'s list of `gozler-core`'s dependents gains
  `gozler-fileops`.
- **`gozler-fileops`:** everything in this section. No `unsafe`, no window code.
  Its decisions (the numbers for Keep both, which file is unfinished, nested
  selections, a destination inside a source, device admission, taken paths,
  the method for a pair of volumes, the ring's block plan) build and test on
  every OS; the disk work is Windows only.
- **`gozler-ui`:** the window that this document describes. It
  shows the queue's states and decides nothing about which
  operation waits.

The engine is a crate because `gozler-ui` is a binary only: the measurement
program and the engine's tests on real folders need a library.

### Windows behavior checked once

Each Win32 case below ran once, with its result here; a case not run is
in the engine's checks of [After install](plans/ui-after-install.md).

- The ring can set the true end of file on its unbuffered handle (stage C,
  run once on 2026-10-19 on the owner's Dev Drive, to 12288 and to 10000
  bytes). The ring still sets it on the handle that copies the streams, as
  AngelCOPY sets it on a reopened handle, because that handle is opened
  for the streams anyway.
- What one listing per destination folder costs on a share. After install
  times a small-file copy to a share.
- `CopyFile2` and an interrupted copy (D10), run once in stage E on
  2026-10-20, 4 GiB, unbuffered, `ioDesiredSize` 32 MiB:
  - A process that ends during the copy leaves the destination at its full
    size, because `CopyFile2` sets the size first, with a creation time
    when the copy started and a last write time when the process ended. So
    the done rule never matches it, and Continue finds it unfinished.
  - The progress callback's STOP and CANCEL both return
    `ERROR_REQUEST_ABORTED` and delete the file; PAUSE returns `S_OK` and
    copies the whole file. So Exit does not stop `CopyFile2` (What a
    stopped operation leaves).
- The compare's speed against a copy, stage E on the owner's SSD with a
  1,600 MiB file: 1.11 GB/s against 0.41 GB/s (`docs/BENCHMARKS.md`). Not
  measured on a hard disk, where the compare reads two files on one disk.
- No test cuts the power. Stage E's tests build each interrupted state
  directly: a prefix, a zero tail, a zero block inside, a changed, grown
  and shrunk source, a link at the name, and two entries with one name.

- ReFS accepts `FILE_DISPOSITION_FLAG_POSIX_SEMANTICS` (stage A, run once on
  2026-10-19, Windows 11 25H2 build 26200, the owner's Dev Drive): with
  "ignore read-only" it deleted a file, a read-only file and an empty
  folder, as NTFS did, so D1's fallback is not taken there. An older ReFS
  that refuses it takes the fallback.
- A share refuses `FILE_DISPOSITION_FLAG_POSIX_SEMANTICS` with
  `ERROR_INVALID_PARAMETER`, so a delete there takes D1's classic delete
  (stage A, run once on 2026-10-19 through `\\localhost\C$`). The delete
  opens ask for no `SYNCHRONIZE` and no synchronous I/O, so that a file
  whose own permissions grant nothing is still deleted through its
  folder's `FILE_DELETE_CHILD`. Over the share, on those handles, the
  refused POSIX call, the `FileBasicInfo` query and set, and the classic
  delete all completed: stage A's delete tests passed with their folders on
  the share, a read-only file, a held file and a file that grants nothing
  included.
- That `GetFinalPathNameByHandleW` with `VOLUME_NAME_DOS` gives one path for
  one share folder reached two ways (D12). After install.
- `CopyFile2` copies a file that another program has open for writing,
  when that program shares reading, as a log writer does; it fails with
  `ERROR_SHARING_VIOLATION` only when the writer shares nothing (stage B,
  run once on 2026-10-19). The Windows engine does nothing about it, as
  File Explorer and robocopy do not: refusing such a file needs one more
  open of every file, without the write sharing, held through the copy.
  The ring refuses it, as "The ring engine" says.
- How `CopyFile2` copies links, run once on 2026-10-19 without elevation,
  with Developer Mode on (the owner's machine has it; turning it off is not
  the engine's to do):
  - A folder junction with `COPY_FILE_OPEN_AND_COPY_REPARSE_POINT`: refused
    with `ERROR_ACCESS_DENIED`; with `COPY_FILE_DIRECTORY` added, copied as
    a junction to the same target.
  - A folder symbolic link: with `COPY_FILE_OPEN_AND_COPY_REPARSE_POINT` and
    `COPY_FILE_DIRECTORY`, an empty folder, without the link; with
    `COPY_FILE_COPY_SYMLINK` and `COPY_FILE_DIRECTORY`,
    `ERROR_PRIVILEGE_NOT_HELD`.
  - A file symbolic link: with `COPY_FILE_OPEN_AND_COPY_REPARSE_POINT`, a
    plain file holding its target's data; with `COPY_FILE_COPY_SYMLINK`,
    `ERROR_PRIVILEGE_NOT_HELD`.

  Developer Mode does not let `CopyFile2` create a symbolic link without the
  right. Not run: a symbolic link copied by an elevated Gozler, which holds
  the right, and Developer Mode off. After install.
- `GetDiskFreeSpaceExW` takes a folder path with the `\\?\` prefix and a
  trailing backslash, locally and as `\\?\UNC\localhost\C$\...` (stage B,
  run once on 2026-10-19), so the free-space check reads the destination
  as the copy writes it.
- `COPY_FILE_NO_BUFFERING` does not stop `CopyFile2`'s own clone within
  one volume that can clone blocks (stage D, 2026-10-19, the owner's Dev
  Drive): 20 files of 64 MiB with the flag copied in 0.14 s, so the Windows
  engine keeps the flag there. That clone cost 0.22 G CPU cycles against
  the ring's 0.36 G, both under 0.4 s; the ring keeps its own clone (D20),
  which works on every Windows version.
- `FSCTL_DUPLICATE_EXTENTS_TO_FILE` (D20), run once in stage C on
  2026-10-19 on the owner's Dev Drive (ReFS, 4 KB clusters), through the
  ring:
  - It takes a range rounded up to a whole cluster past the true end of
    file: a file of 33 MiB + 333 bytes and one of 6 GiB + 12345 bytes, the
    second in two ranges, cloned in 20 ms to 1 s and matched byte for byte.
  - A sparse 64 MiB file stays sparse, with 139264 bytes allocated.
  - The end of file set before the clone on a destination that is not
    sparse takes the whole size from the free space; `CopyFileExW` and a
    clone into a destination marked sparse whose mark is then cleared take
    the same 6.2 GB for the 6 GB file. A destination marked sparse takes
    4 KB.
  - With 5.2 GB free, the end of file of a 6 GB destination that is not
    sparse fails with `ERROR_DISK_FULL`. `CopyFileExW` then still clones
    and leaves its copy sparse; clearing the ring's sparse mark fails with
    `ERROR_DISK_FULL`, and the ring keeps the mark, as "The clone" says.
  - ReFS reports space freed by a delete up to about a minute late.
- Whether the parallel walk is slower than one walker on a hard disk.
- Every speed claim of D2 and D7 to D9 between two disks, on a hard disk,
  a USB drive or a share. Stage D measured them on one SSD
  (`docs/BENCHMARKS.md`); the rest is in the engine's checks of
  [After install](plans/ui-after-install.md).

## Not planned

Each of these waits for a concrete need:

- Content search: `rg` does that.
- `< >` grouping of terms.
- Paging: es's `-viewport-offset` and `-viewport-count` are unknown options (exit 6). The pipe protocol keeps its `offset` field for other applications.
- EFU file lists, playlists, bookmarks, macros, and the size and date format menus.
- In the window: search and run history; results that update while files change, because the protocol has no change notification (`F5` searches again); the other hard links of a file, because results carry no file ID; and right-to-left languages, because no such catalogue is planned (see "How normal changes land").
- In Commander mode: Edit (`F4`), a later plan, which the key bar shows disabled; a command line under the panels, as Norton Commander had, because Windows Terminal does that; counting a folder's size by walking it on a drive that gozlerd does not index, because the person never waits for a count; searching inside files' contents, because gozlerd indexes names; archives as folders and FTP; drag and drop between the panels, a later plan, so Commander mode accepts no drop, though its panels drag out (see "Dragging"); thumbnails and other views than the report list, and choosing the panels' columns; a splitter between the panels; and, beyond the owner's list, rename in place (`F6` with a new name renames), the clipboard commands, selecting by pattern and inverting the selection (`Num +`, `Num -`, `Num *`), swapping the panels (`Ctrl+U`) and the sort keys (`Ctrl+F3` to `Ctrl+F6`).
- In the Viewer: searching, selecting part of the text, images, printing, choosing the encoding, and Reload.
- A speed limit for a file operation.

These do not fit gozler:

- A second command-line grammar beside es's: `/NAME:value` options, `/F`, `/D`, `/EXT:` and `/NOT:` lists, `AND`/`OR`/`NOT` keywords, and exact-path lookups keyed on a trailing separator.
- Accessed dates: Windows can turn their updates off (`NtfsDisableLastAccessUpdate`), so they can mislead.
- A scheduled-task install: gozlerd is a service.
- Appearance settings in the window, such as fonts, colors, size and date formats and editable menus, and polish such as Mica, a custom title bar, custom-drawn buttons or taller rows: the owner wants Windows and File Explorer to decide the look, polish that needs a paint routine for every state costs more code than it gives, and a list at Windows' compact height shows more results.
- Rename in Search's list: Open file location gives File Explorer for it.
- A window run as administrator: only the service needs administrator rights. One main window serves a session unless the person allows more (see "One window per user session, unless the person allows more"); the Viewers have separate top-level windows; transfers stay in the main window.
- Staging all output before printing it: exit codes 2 and 7 already mark a late failure.
- Saving the index between runs: the file would keep every name on the volume on disk after gozlerd stops, where backups and disk images copy it, and a damaged or planted file could be served as `live`. One MFT pass already builds a volume of 2 million entries in about 10 s.

## Golden corpus

The golden corpus records what gozler prints for a set of command lines over fixed data, so a change that alters behavior fails a row. The search corpus (`gozler-core`, `escompat::corpus`) runs each command line through `parse_argv` and `build_search` into the search the daemon runs, `query::ResultRows` and its pages or `query::search_totals`, over a fixture index. The output corpus (`gozler-core`, `output::results::corpus`) prints a fixed result set through `ResultWriter`, which `gozler` and the window's Export both write through, in each output format. Dates render in UTC, so every row passes in any time zone.

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
