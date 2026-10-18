# Plan: es superset

The order in which to build [the es superset specs](es-superset-specs.md). The
specs decide what gozler does; this plan decides the order and how each stage
proves it is done.

## Schedule

**Owner decision:** this plan starts after the open issues labelled `bug` are
fixed, or closed after confirmation. An issue that also carries the `in plan`
label is fixed by the stage that names it.

Issues #1 (tie order), #5 (`metadata_pending` never set), #6 (a hard-link
name gets no size or date), #14 (a wildcard with a separator never matches)
and #23 (a path term starting with `\` misses folders at the volume root) are
fixed before the plan with the other bugs, because their fixes do not depend
on it. They also fix spec bugs 1 and 11. The golden corpus records all of
them as its baseline. A stage entry whose issue is already closed when the
stage starts is done.

## How every stage works

- A stage is done when the full CI set from `AGENTS.md` passes and every item
  of its **Done when** is proven. Each new switch, function and exit code has
  a test for the behavior gozler chose, and its table row, so the generated help
  and its parse-everything test cover it. Each new difference from es gets
  its row in the differences table.
- Stages 1 to 3 change only what the client does; stage 2 also moves one
  daemon rule into `gozler-core` without changing it. The installed daemon already sends
  what they need: every results page carries the full match count (`total`)
  and the `volumes_incomplete` flag. Their live checks run the built
  `target\release\gozler.exe` against the running daemon, which only reads.
- Stages 4 to 9 change the daemon. Their **Done when** is proven by tests over
  fixture indexes, as the `server.rs` tests do: a second daemon cannot run
  beside the owner's, because `PIPE_NAME` is fixed and the running daemon owns
  it.
- Each daemon stage that grows the parsed query on the wire raises
  `PROTO_VERSION` by one. The daemon serves every version from 1 up to its
  own, and the client asks for its own version as the minimum, so a client
  newer than the daemon exits 7 with the version message.
- Installing is the owner's step. A client from stages 1 to 3 installs alone.
  From stage 4 on, the daemon and client install together, from one build,
  alone or with the next stage, because every search on the machine depends
  on the running daemon. A stage's **After install** checks run then.
- Check es behavior in Everything's documentation. `es.exe` is never run
  (owner ruling).

## Stage 0: golden corpus

- Make `emit` write to a `Write`, and make `render_csv` take the time zone as
  a parameter.
- Capture the golden corpus ([specs: Proof](es-superset-specs.md#differences-from-es))
  from the code as it is when this plan starts.

**Done when:** `cargo test --workspace` runs the corpus, and it passes in UTC
on any machine.

## Stage 1: exit codes, tables and help

- The exit-code enum in `gozler-core` ([specs: Exit codes](es-superset-specs.md#exit-codes)),
  with one mapping from every client failure to one code. This fixes issue
  #40 (three error paths in `main.rs`) and issue #30 (`--json` and `--status`
  ignore write errors; they become exit 5).
- Bugs 5 (exit 2 while a volume indexes), 7 (`-help`) and 8 (exit 3 for a
  failed trust check).
- The vocabulary tables for every existing switch, function and sort key; the
  help generated from them, with `-h`, `-help` and `-version`; the differences
  table that the help's NOTES print.

**Done when:**
- A test maps each failure to its code, and the precedence rule holds.
- The help test parses every switch, function and example the help lists.
- Against the running daemon: `gozler -help` exits 0, and a query on a live
  volume exits 0.

## Stage 2: the default limit and the status commands

- The default limit of 20, `-n all`, `-count`, `-n 0` and exit 10 with its
  warning (bug 4), from the owner decision in
  [specs: Compatibility rule](es-superset-specs.md#compatibility-rule).
- `-no-result-error` (exit 9).
- `--status --json`, which prints the wire's `DaemonStatus` as `--json`
  prints `QueryResults`, and the `--status` exit code, which counts
  `metadata_pending` (set since #5).
- `-timeout MS`: the client polls the status until the volumes in scope are
  complete or the time runs out. The rule that maps a scope to its volumes
  moves from the daemon's `targets()` into `gozler-core`, and both call it.

**Done when:** against the running daemon, with `-path C:\` on each command
so a rescan elsewhere cannot turn the answer into exit 2,
- `gozler ext:cs` prints 20 rows and the warning, and exits 10;
- `gozler ext:cs -n all > $null` exits 0 (redirected, because it prints every
  match);
- `gozler -n 0 ext:cs` prints nothing and exits 10;
- `gozler zzzzqqq -no-result-error` exits 9.

**After install:** `C:\Users\user\.claude\goz.md` drops the warnings this
stage fixes, and the owner decides whether CLAUDE.md and the TinyTorrent2
filename-discovery ruling still need `-n 20`.

## Stage 3: switches, columns and output

- The `/a` and `/o` prefix families, which own every `/a` and `/o` token
  ([specs: Switches](es-superset-specs.md#switches)). `/ad` and `/a-d` (bug 3)
  become `folder:` and `file:` in the query text; other letters, and `/oe`
  until stage 8, exit 4 with a message.
- The `/o` forms that map to the existing sort keys.
- Columns (bug 6), `-csv`, `-tsv`, `-txt` and the exports through the one
  writer.
- `-get-result-count`, which reads `total`.

**Done when:** against the running daemon, `gozler /a-d report` lists only
files; `gozler /aH report` exits 4; `gozler report -size -dm` prints the two
columns; and `-get-result-count` equals the `total` in `--json` output.

## Stage 4: protocol 2, with today's meaning

- The parsed query as a wire type, `PROTO_VERSION` 2, the `Hello` gate, and
  the daemon's text path through the same parser
  ([specs: Design](es-superset-specs.md#design)).
- The query as AND-ed clauses of OR-ed, possibly negated terms, with the name
  prefilter rule from the specs. The client parses and sends the query; every
  query keeps the meaning it had before this stage.
- The request builder (`build_query`, from `EsPlan` to `QueryRequest`) moves
  from `gozler-cli`'s `main.rs` into `gozler-core`, taking the working directory as
  a parameter, because a test can reach it there and stage 6 needs that
  parameter for relative paths. The builder joins paths against that
  directory with Windows separator rules of its own and never calls
  `std::path::absolute`, which on Ubuntu, where `gozler-core` is tested, turns
  `C:\repo` into a Unix path. After the join it removes `.` and `..`
  components, as `GetFullPathNameW` does today, because the daemon's
  `resolve_scope` looks each component up as a child folder; the corpus gets
  a `-path .` row and a `-path ..\x` row. A rooted `\foo` takes the drive of
  the given directory. Only a drive-relative value such as `-path D:foo` is resolved by
  `gozler-cli` with `std::path::absolute` before it calls the builder, because
  only Windows knows each drive's current directory. The corpus then runs
  each row, in
  `server.rs`'s tests, from its command line through that builder into
  `encode_query_binary`, so it proves the path the client now uses; the text
  path keeps one row of its own.
- `-get-total-size` and `-get-folder-size`: the request asks for the count
  and the size total without rows, and the daemon sums the sizes it already
  holds per hit, with checked arithmetic. Summing on the client would stream
  every match: `-get-folder-size C:\` is the whole volume.

**Done when:**
- The corpus passes with no changed row.
- The criterion benches show no slowdown that the change does not explain.
- A fixture test proves the totals request returns the same count and sum as
  adding up every row of the same query.
- The new client against the installed daemon exits 7 with the version
  message.
- A `server.rs` test sends a version-1 text `QueryRequest` without `Hello`
  and decodes the reply as the stage 3 client does.

## Stage 5: operators and functions that refuse

- `!`, `|`, the refusal of `< >`, unknown functions as exit 4 (bug 2), and
  quoted function values (bug 10).

**Done when:** the corpus passes, apart from the rows marked for bugs 2 and
10, the `a<b` row, and the `!` and `|` rows, which change from refused to
supported.

## Stage 6: paths

- Bug 9 (`/` as a separator), relative paths resolved in the client, `/`
  turned into `\` except in a regex, and quoted path terms
  ([specs: Paths](es-superset-specs.md#paths)).
- `path:`, `parent:`, `nopath:`, `-p`, `-parent` and `-parent-path`; a
  relative path on the daemon's text path is `BadQuery`.

**Done when:** over a fixture index, bug 9's commands and the quoted-path
examples in bug 10's row give their fixed results, and the corpus rows for
bugs 1 and 11 still pass.

## Stage 7: matching

- `ww:` and `-ww`, `regex:` and `-r` with the `regex` crate added to the
  gozler-core allowlist in `ci.yml`, and `-i` as the alias of `-case`.

**Done when:** a regex test covers `\\` in a path, and the help states the
regex dialect.

## Stage 8: sizes, dates and order

- `dm:` and the date resolver, with `jiff` added to the gozler-core allowlist
  ([specs: Dates](es-superset-specs.md#dates)).
- The size and date rule, with exit 2 for queries that test or total size or
  date on a volume with `metadata_pending` set.
- Sort key `extension` and `/oe`, in the tie-break order that #1 set.

**Done when:**
- The date tests cover midnight, a future timestamp and a daylight-saving
  change.
- A fixture test proves the exit-2 rule for a size test, a date test and a
  size total on a volume with `metadata_pending` set.

**After install:** `file: size:0..` on `C:` counts the same files as `file:`,
or each remaining difference has a named cause (bug 12).

## Stage 9: the attribute column

- Measure the memory cost on the owner's `C:` first, with
  `crates/gozler-daemon/examples/memsim.rs`; the owner decides from the
  measurement whether the column goes in.
- The column, the `iflags` request bit, the remaining `/a` letters, `attrib:`
  and the attributes output column.

**Done when:** a fixture test lists hidden files for `/aH`, and a result
encoded without the bit decodes correctly in the stage 8 client's decoder.

## When the plan is done

The specs' [Done when](es-superset-specs.md#done-when) list holds. The rules
of this plan and the specs move into `docs/DESIGN.md` and the help, and both
files are deleted.
