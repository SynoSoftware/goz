# Specs: es superset

gozler as a compatible superset of goz v0.2.0, with the Everything commands we
need on top.

The schedule and the order of work are in
[the es superset plan](es-superset-plan.md).

## Goal

Make gozler the dependable file finder for people and agents. It keeps every
command-line name of goz v0.2.0, fixes the behavior that is wrong, and adds
the Everything (`es.exe`) commands we need on top. Internals may be refactored
freely; this is not a smallest-change plan.

The work happens in our fork (`origin` = SynoSoftware/goz, `upstream` =
mustafaahci/goz).

## Compatibility rule

- Every switch, alias, search function and syntax goz v0.2.0 accepts keeps its
  name and purpose. Nothing is renamed or repurposed; behavior changes only
  where the bug table, the search syntax table or an owner decision below
  fixes it.
- Behavior that is wrong is fixed, not inherited: the bugs below, and the exit
  codes, which get their own complete design.
- **Owner decision: a default limit of 20 results.** v0.2.0 prints every
  match, which floods the context of an agent or the screen of a person who
  forgets `-n` on a volume of a million files. 20 rows plus the command, the
  warning and the next prompt fit one screen of a 25-line console. With no
  `-n`, gozler prints at most 20 rows to stdout; when more exist, stderr says
  `gozler: warning: showing 20 of 1,234,567 matches; narrow the query or pass -n all`
  and the exit code is 10. `-n all` prints every match. `-export-*` writes to
  a file, which floods nothing, so it exports every match unless `-n` is given.
  `-n 0` prints nothing and exits 10 when anything matches. Counts
  (`-get-result-count` and the size totals) cover every match, unlike es, where
  `-get-result-count` honors `-n`.
- New features follow gozler's style: an es switch keeps es's name, aliases and
  single-dash form; a gozler-only option goes behind `--`, as `--json` does.
  Messages use `gozler: <error>` and `gozler: warning: <text>`.
- **Owner decision: neither v0.2.0 nor es decides behavior.** They supply the
  names and the starting behavior; where either is wrong or worse for the
  user, gozler fixes it, and the help lists every difference from es (below).

### Differences from es

Scripts written for es need to know where gozler answers differently. The help's
NOTES print this list from one table in `gozler-core`, so it cannot fall behind:

| es | gozler | Why |
| --- | --- | --- |
| CSV is Filename, Size, Date, with locale dates | Size, Date, Filename, with ISO dates | v0.2.0's layout; scripts parse ISO dates without knowing the locale |
| An unknown function such as `dc:` is searched as text and matches nothing | exit 4 (bug 2) | a silent empty answer looks valid |
| `<a b>\|c` groups terms | an unquoted `<` or `>` outside a function value is exit 4 | grouping is not planned; Windows names cannot contain `<` or `>`, so the text never matched |
| One argument with a space is a phrase: `es "DESIGN md"` finds nothing | it splits into terms, as in v0.2.0; a phrase is written `'"DESIGN md"'` | scripts build a query in one string |
| `/` is not a separator: `es docs/DESIGN` finds nothing | `/` and `\` both work outside a regex (bug 9) | agents and Unix habits write `/` |
| Prints every match; `-n` cuts silently with exit 0 | 20 by default; `-n all`; exit 10 when the limit cuts the list, including `-n 0` | see the owner decision above |
| `-n all` is exit 4 | prints every match | the way to lift the default |
| `-get-result-count` honors `-n` | counts every match | a count is useless when it stops at the limit |
| Exit codes are undocumented | the exit-code table below | a script needs one code per outcome |
| Regex in es's dialect | Rust `regex` dialect | linear time, so a pattern cannot hang the daemon |
| Folder sizes can be indexed | `size:` and `-get-total-size` skip folders | the index holds no folder sizes |

`-timeout` on expiry joins the table if Everything's documentation (see "Not
verified") shows that es does not run the query anyway.

**Proof:** a golden corpus that records behavior, not v0.2.0's types: each row
is a command line, the entries it matches in a fixture `VolumeIndex`, and the
printed output. Capture it, from the code as it is when the plan starts,
through the seams that already run
without a pipe: `parse_argv`, `parse_query`, the daemon's `encode_query_binary`
over the fixture index (as `server.rs` tests do), and the CLI's `emit` and
`render_csv` over synthetic `QueryResults`, once `emit` writes to a `Write`
and `render_csv` takes the time zone as a parameter. Rows render in UTC, so
the corpus passes on any machine. The parser types change; the rows stay,
except rows that a bug fix changes on purpose, each marked with its bug number.

## Bugs to fix

Each of these returns a wrong answer that looks valid, or hides a limit:

| # | Command | v0.2.0 result | Fixed result |
| --- | --- | --- | --- |
| 1 | `gozler "C:\repo\docs\testing.md"` | nothing, exit 0 (paths are matched without the drive) | that file, as in es: a drive path is a full-path term |
| 2 | `gozler dm:today`, or any unknown `name:` function | nothing, exit 0 (searched as text, which can never match, because Windows names cannot contain `:`) | the implemented function, or exit 4 `gozler: unknown search function: dc:` |
| 3 | `gozler /a-d report` | nothing, exit 0 (`/a-d` searched as text) | files only (`/a` switch) |
| 4 | `gozler report -n 20` with 300 matches | 20 rows, no sign of more, exit 0 | the same 20 rows, the stderr warning, exit 10 |
| 5 | any query while a volume is still indexing | results, stderr warning, exit 0 | the same results and warning, exit 2 |
| 6 | `gozler report -size -dm` | paths only | paths with size and date columns |
| 7 | `gozler -help` | exit 6 | usage, exit 0 |
| 8 | pipe server fails the trust check | exit 9, the code es uses for "no results" | exit 3 |
| 9 | `gozler docs/agents/testing`, `gozler path:docs/agents`, `gozler C:/repo/docs/testing.md` | nothing, exit 0 (`-path C:/repo` already works) | the same matches as with `\` |
| 10 | `gozler 'path:"C:\Program Files"'` | searched as a name, because `parse_query` turns any token with quotes into a name term before it looks for a function | quotes delimit a function's value, as in es: `path:"C:\Program Files"` matches that path. Arguments are still joined and split into terms as in v0.2.0, so `gozler "ext:cs size:>1mb"`, the way scripts build a query in one string, keeps working. A quoted term that contains a separator is a path term, not a name phrase, so `gozler '"C:\Program Files (x86)\Microsoft"'` matches that path; unquoted, its piece `Files` would be a name term and match nothing. A wildcard in a quoted path term is still a wildcard (`'"C:\Program Files\*.cs"'`), because a literal `*` can never match a path. Examples never end a quoted path in `\`, because the Windows argument rules read `\"` as a literal quote |
| 11 | `gozler "!*\node_modules\*"`, `gozler "*\docs\*"` | wildcards match names only, so a wildcard with `\` matches nothing | as in es: a wildcard containing `\` matches the full path |
| 12 | `gozler size:0..` on `C:` with every volume live | 627 fewer files than `file:` (measured 2026-10-17), for example `C:\Program Files (x86)\Microsoft\EdgeWebView\Application\155.0.4283.62\155.0.4283.62.manifest`, which has a link count of 2 | find why those files never get a size or date, and fix it |

## Exit codes

One code per outcome a caller handles differently. Codes 4 to 8 keep their
v0.2.0 meanings; the rest are new or corrected.

| Code | Meaning |
| --- | --- |
| 0 | Complete answer, including no matches. |
| 1 | Internal failure: anything not listed below. |
| 2 | Incomplete answer: a volume in scope is indexing, rescanning or offline, or the query tests or totals size or date on a volume whose size and date pass failed. The results that exist are printed. |
| 3 | The pipe server is not the trusted daemon (`--insecure-no-server-check` skips the check). |
| 4 | Invalid input: a missing or bad switch value, an unknown search function, a malformed expression or regex, or conflicting options. |
| 5 | Output or export failed. |
| 6 | Unknown switch. |
| 7 | Communication failure with the daemon, including a reply that does not arrive in time, and a daemon older than the client. |
| 8 | The daemon is not running. |
| 9 | No matches, with `-no-result-error`. |
| 10 | The output is correct but the limit left out more matches; stderr gives the shown and total counts. |

When several apply, the more serious wins: 1 and 3 to 8 first, then 2, then 9
and 10, then 0. An empty answer from an indexing volume is 2, not 9.

Each failure writes one `gozler: ` line to stderr that says what failed and, where
it helps, what to do next. The codes live in one enum in `gozler-core`, with each
variant's meaning beside it, so the help lists them from the same source.

## New commands

All names come from Everything's command-line and search-syntax
documentation, and every one exists in `es.exe -help` 1.1.0.37 on this
machine. Before building one, check its behavior in Everything's
documentation; never run `es.exe` (owner ruling). Copy that behavior when it
is right, fix it when it is not, and write the test for the behavior gozler
chose.

### Switches

| Purpose | Add |
| --- | --- |
| Help | `-h`, `-help`, `-version` |
| Waiting | `-timeout MS`, with es's meaning: wait up to MS for the volumes in scope to become complete, then query; still incomplete runs anyway with exit 2 |
| Status | `--status --json`: machine-readable daemon and per-volume state; `--status` exits 0 when every volume is complete (`VolumePhase::is_complete`) and none has `metadata_pending` set, 2 otherwise |
| Matching | `-i` (alias of `-case`), `-whole-word` / `-ww`, `-match-path` / `-p`, `-regex` / `-r` |
| Scope | `-parent DIR` (direct children), `-parent-path PATH` |
| Kind and attributes | `/ad`, `/a-d`; other attribute letters after the attribute column below. The `/a` and `/o` parsers own every token that starts with `/a` or `/o`: a letter they do not support yet is exit 4 (`gozler: /aH needs the attribute column, which this release does not have`), never search text, because `parse_argv` today sends an unmatched `/` token into the query, where it matches nothing |
| Limit | `-count N`, es's alias of `-n`, and `-n all`. No paging: es's `-viewport-offset` and `-viewport-count` stay unsupported (exit 6). The pipe protocol keeps its `offset` field for applications. |
| Sorting | es's `/o` forms (`/on`, `/o-n`, `/os`, `/o-s`, `/od`, `/o-d`, `/oe`, `/o-e`) and sort key `extension`, beside the existing `-sort` family |
| Columns | `-name`, `-path-column`, `-full-path-and-name`, `-extension` / `-ext`; `-size` and `-dm` also in plain output |
| Counts | `-get-result-count`, `-get-total-size` (matching files only), `-get-folder-size DIR`; totals use checked arithmetic and fail rather than wrap |
| Output | `-csv`, `-tsv`, `-txt` to stdout, `-export-tsv`, `-export-txt` beside the existing `-export-csv`, all through the one writer that v0.2.0's CSV uses |
| Scripts | `-no-result-error` (exit 9 when nothing matches) |

### Search syntax

| Purpose | Add |
| --- | --- |
| Operators | `!` (not) and `\|` (or), with es's precedence: `\|` binds tighter than the space between terms (verified: `DESIGN\|argv rs` returns only `argv.rs`). `< >` grouping is not supported: an unquoted `<` or `>` outside a function value (`size:<1mb` stays valid) is invalid input, exit 4, until a real need for `<a b>\|c` appears. Windows names cannot contain `<` or `>`, so v0.2.0's literal `a<b`, which never matched, becomes that error; its corpus row changes on purpose. |
| Functions | `dm:`, `parent:`, `ww:`, `regex:`, `nopath:`; `attrib:` after the attribute column |
| Paths | `/` and `\` both work; `.\` and `..\` resolve against the working directory |

### Paths

A path term follows es: a term with a drive or a separator matches against the
full path, as a substring, and a wildcard in it matches the full path too.
`C:\repo\` therefore finds everything under `repo`, and `C:\repo\*.cs` finds
the `.cs` files under it. A file path also matches longer names that start
with it (`x.md.bak`), as in es. The client adds two things before it sends the
query: it resolves `.\` and `..\` against its working directory, and it turns
`/` into `\` in path terms, wildcards, the `path:`, `parent:` and `nopath:`
values, and the `-path`, `-parent` and `-parent-path` switches. It never
changes a regex: in `regex:` and `-r`, `/` and `\` keep their regex meaning,
and the help says a regex sees `\` separators, so write `\\` or `[\\/]`.
Output keeps Windows `\` paths.

A token that starts with `/` and matches a switch is still that switch, as in
es (`/ad`, `/on`), so the help says to write a relative path without a leading
`/`.

### Dates

`dm:` resolves once, from one query-start instant and a time zone, to UTC
bounds. One resolver in `gozler-core` takes (now, time zone): the client passes
its own, and the daemon's text path (below) passes the daemon's.

- `today`, `yesterday` and an ISO date are local calendar days, from midnight
  to the next midnight.
- `Ndays` is the rolling period of N times 24 hours ending at query start.
- Comparisons (`>`, `>=`, `<`, `<=`) and `lower..upper` ranges work on both
  forms; a reversed range is invalid input (exit 4).
- Test midnight, future timestamps and a daylight-saving change.

## Target help screen

`gozler -help` is generated from the tables (see Design), so the real text follows
the implementation. This sketch is the target it should read like.
Everything's documentation settles the details it guesses, such as the column
layout.

```text
gozler 0.3.0 - instant file search for Windows NTFS

Usage: gozler [switches] [search terms]

  Finds files and folders by name, path, size and date. Terms combine with
  AND. Paths accept \ or /. Shows 20 results unless -n says otherwise.
  Results: one full path per line on stdout; diagnostics on stderr.

SEARCH TERMS
  report              names containing "report"
  *.cs                whole names matching a wildcard (* and ?)
  '"two words"'       a phrase; the shell must pass the double quotes
  '"C:\Program Files (x86)"'    a quoted path with spaces
  docs\agents         paths containing docs\agents (or docs/agents)
  C:/repo/            everything under that folder
  C:/repo/*.cs        .cs files under that folder
  *\node_modules\*    a wildcard with \ matches the full path
  ./src/              a folder relative to the current directory
  !test               exclude names containing "test"
  a|b                 either term; | binds tighter than the space

SEARCH FUNCTIONS
  ext:cs;cpp          extension, one of a list
  size:>1mb           file size; also <, >=, <=, 1mb..10mb, tiny..gigantic
  dm:today            date modified; also yesterday, 7days, 2026-10-17, ranges
  path:text           match against the full path
  parent:dir          direct children of a folder
  file: / folder:     files only / folders only
  ww:word             whole word
  regex:pattern       regular expression (Rust dialect; paths use \, write \\)
  nopath:text         match the name only
  case:               match case

SWITCHES
  -n N, -count N      show at most N results (default 20); exit 10 if more exist
  -n all              show every result
  -path DIR           search under a folder
  -parent DIR         only direct children of a folder
  -parent-path PATH   search under PATH's parent folder
  /a[-]LETTERS        attributes: /ad folders only, /a-d files only
  -i, -case           match case
  -ww                 match whole words
  -p                  match terms against the full path
  -r PATTERN          search with a regular expression
  -sort KEY[-descending]   name, path, size, dm, extension
  /o[-]KEY            /on /os /od /oe; /o-n etc. reverse
  -size -dm           add size and date columns
  -name -extension    choose columns
  -get-result-count   print only the number of matches (ignores -n)
  -get-total-size     print the total size of matching files
  -get-folder-size DIR  print the size of everything in a folder
  -csv -tsv -txt      output format
  -export-csv FILE    also -export-tsv, -export-txt; exports every match
  -no-result-error    exit 9 when nothing matches
  -timeout MS         wait up to MS for indexing volumes before searching
  -h, -help, -version

GOZLER OPTIONS
  --json              results as JSON
  --status [--json]   daemon and volume state
  --insecure-no-server-check   skip the daemon trust check (testing only)

EXAMPLES
  gozler readme
  gozler C:/SynoSoftware/TinyTorrent2/*.cs
  gozler -path C:/repo ext:cs;cpp !test
  gozler *.log dm:7days /a-d
  gozler ext:cs -n all > files.txt
  gozler -get-result-count ext:cs
  gozler -r "^test[0-9]+\.cs$" -path C:/repo

NOTES
  A volume that is offline makes every search outside -path exit 2; search
  with -path on a live volume for a complete answer.
  Files whose size or date Windows did not report are left out of size: and
  dm: tests.
  Differences from es (one line each, from the table):
    CSV is Size, Date (ISO), Filename; es puts Filename first.
    Unknown functions are errors (exit 4); es searches them as text.
    < > is an error (exit 4); es groups terms with it.
    ...

EXIT CODES
  0   complete answer, including no matches
  1   internal failure
  2   incomplete: a volume is indexing or offline, or its sizes and dates
      could not be read
  3   the daemon failed the trust check
  4   invalid input: bad value, unknown function, malformed expression
  5   output or export failed
  6   unknown switch
  7   communication failure, no reply in time, or a daemon older than gozler
  8   daemon not running
  9   no matches, with -no-result-error
  10  output correct, but the limit left out more matches
```

## Design

- **The client parses; the daemon evaluates.** The client runs the gozler-core
  parser, resolves the working directory, time zone and query-start instant,
  and sends the parsed query with its scope, sort, limit and wanted columns,
  or asks for the count and size total only, which the daemon sums so no row
  crosses the pipe.
  Arguments are joined and split into terms as in v0.2.0 (bug 10). The
  daemon still accepts `QueryRequest.query` as text from other pipe clients and
  parses it with the same parser and its own clock; a relative path there is
  invalid input (`BadQuery`), never a literal that silently matches nothing. The
  parsed query is a wire type that only grows: new variants, never changed
  ones, the rule the JSON structs already follow.
- **Version gate.** Raise `PROTO_VERSION` to 2 and send `Request::Hello` first,
  as `proto/types.rs` already asks of a client built apart from the daemon.
  Each later change that grows the parsed query raises it by one. The daemon
  serves every version from 1 up to its own: it answers a `Hello` range with
  the highest version it covers, and serves a version-1 client the text
  queries version 1 promised. An older daemon answers the new client `Unsupported`;
  the client prints
  `gozler: the running gozlerd is older than this client; install the matching gozlerd`
  and exits 7. Without the gate, an old daemon searches new functions as text
  and finds nothing with exit 0, or rejects the new request with a serde error
  that the client would report as invalid input.
- **The query is AND-ed clauses**, each an OR of possibly negated terms. That
  covers `!` and `|` with es's precedence and keeps the evaluator flat. The
  folded-arena fast path needs one literal that every matching *name*
  contains: only a clause with a single positive name term supplies it. A
  wildcard containing a separator, a negated term, a `path:` term and a regex
  never do; today `CompiledQuery::new` takes every wildcard's longest literal,
  and for `*\docs\*` that literal is `\docs\`, which no name contains, so the
  scan would find nothing (bug 11 again). The scoped subtree walk does not
  depend on the query shape. Measure before and after with the criterion
  benches.
- **Sizes and dates.** A size test applies to files: a folder has no size by
  design (`set_stat` leaves it `SIZE_UNKNOWN`), so `size:` and the size totals
  skip folders, as `file:` would. "Unknown" means a file whose size or date
  was not read, because the size and date pass failed or stopped, or could not
  read that file. A size or date test, or its negation, never matches a file
  whose value is unknown: `!size:>1mb` does not report an unread file as
  small, and `report|size:>1mb` still finds `report.txt` through its other
  term. Because every clause is an OR, this gives the same rows as
  three-valued logic without tracking unknown results per entry.
  Bootstrap reads sizes and dates synchronously, so a live volume never has
  that pass still running; it can have a pass that failed or stopped, which
  `build_index` only logs today. The volume records that in
  `metadata_pending`, which nothing sets today (issue #5), and `--status`
  shows it. The daemon already ORs each volume's `metadata_pending` into
  `FLAG_METADATA_PENDING` in the results header; the client exits 2 when that
  flag is set and the query tests size or date, or asks for a size total
  (`-get-total-size`, `-get-folder-size`), which would otherwise come out
  short with exit 0. A single file that a
  successful pass could not read is left out without exit 2; after bug 12 is
  fixed such files should be rare, and the help says so.
- **Vocabulary tables.** Switches, sort keys, search functions and exit codes
  each live in one table in `gozler-core`, in the style of `SIMPLE_SWITCHES`. A
  row carries its aliases, its help group and its one-line description, so
  `-n N, -count N` prints on one line. `/a` and `/o` are prefix families with
  their own small parsers, because the dash-optional matcher makes `/ad` and
  `/a-d` (and `/on`, `/o-n`) collide as plain rows; each gets one help line,
  as es prints them.
- **Help is generated** from those tables, never written as one block. The
  guard is a test that parses every switch, function and example the help
  lists. The unknown-switch and unknown-function errors add
  `gozler: run gozler -help for the supported switches`.
- **Order is deterministic.** Ties break on folded name, then volume, then
  file ID, then parent file ID, then raw name. Hard links need the last two
  keys: each link name is its own entry, and all of them share one file ID
  (`notepad.exe` has three). The raw name separates two links in one
  case-sensitive folder whose names differ only in case. The same query over the same index then prints the same list, and
  no path is rebuilt under the index lock. Unknown sizes and dates sort last.
- **Dependencies.** `regex` (linear time, so a pattern cannot hang the daemon)
  and `jiff` (the date resolver) join gozler-core; add both to the allowlist in
  `ci.yml` on purpose. Document the regex dialect in the help; it is not es's.
- **An attribute column** in the index is needed for `/aH`, `/aS`, `attrib:`
  and an attributes output column: about 4 bytes per entry (7.4 MiB on a
  1.9 million-entry `C:`). Measure first; the `FILE_ATTRIBUTE_*` bits already
  arrive in the layout and USN records. Result items are positional, so the
  field travels only when the request asks for it through a free `iflags` bit;
  otherwise older clients misread every item.
- **Crate boundaries stay as `AGENTS.md` describes them:** parsing, tables,
  help text, date resolution, building the request from a working directory
  it is given, and evaluation in `gozler-core`; reading argv, the working
  directory and the clock, and writing output in `gozler-cli`.

## Not planned

Created and accessed dates (the index does not hold them), content search
(`rg` does that), `< >` grouping, EFU, playlists, bookmarks, macros,
highlighting, and the size and date format menus. Add one when a concrete
need appears.

From the filefinder plan, these do not fit gozler: its `/NAME:value` switch
grammar, `/F`, `/D`, `/EXT:` and `/NOT:` lists, the `AND`/`OR`/`NOT` keyword
grammar, exact-path lookups keyed on a trailing separator, results filtered by
the caller's file permissions (gozler's trust model shows metadata to every local
user), the scheduled-task install and tray icon (gozler has its service), and
staging all output before printing (exit codes 2 and 7 already mark a late
failure).

## Not verified

`-get-folder-size` returns "Error 8: Everything IPC not found" in the
installed es, so its test comes from Everything's documentation. es's
`-timeout` expiry, the case of `/a` letters and `-get-total-size` on folders
are settled from that documentation too, because `es.exe` is no longer run.

## Done when

- The golden corpus passes unchanged, apart from the default limit, the bug
  rows, the `a<b` row, and the `!` and `|` rows.
- The twelve bug commands give the fixed results in the table, and every exit
  path maps to one code in the exit-code table.
- A new `gozler` against the old daemon exits 7 with the version message.
- `gozler -help` lists every switch, function and exit code gozler accepts, and each
  listed switch, function and example parses.
- Every new command has a test for the behavior gozler chose, each difference
  from es is a row of the differences table and a line of the help, and the
  full CI set from `AGENTS.md` passes.
- `C:\Users\user\.claude\goz.md` drops its warnings and lists the new syntax.
