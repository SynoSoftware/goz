# Design: gozler-ui

How the window that [the spec](ui-spec.md) describes is built: the toolkit,
who owns what, the threads, the rules that keep the first version from
needing a rewrite, and the trade-offs. It changes when a design decision
changes. [The plan](ui-plan.md) holds the order of work, the tests and the
provisional API sketches.

## Decisions

1. **Native Win32 in Rust,** through Microsoft's `windows` crate (MIT or
   Apache-2.0). Two throwaway prototypes of the same window, with a virtual
   list of 3 million rows, measured it against Slint 1.18.1:

   | | Win32 | Slint, software renderer | Slint, OpenGL renderer |
   | --- | --- | --- | --- |
   | Private memory | 4.6 MB | 12 MB | 120 MB |
   | Executable | 182 KB | 13.9 MB | 13.9 MB |
   | First release build | under 1 minute | 9.5 minutes | (same build) |

   Windows itself gives the list's selection, keyboard navigation, column
   resizing and reordering, sort arrows and accessibility; in Slint they are
   code that gozler would write. The window uses `windows`, not the
   workspace's `windows-sys`, because it needs COM interfaces such as the
   folder picker (`IFileOpenDialog`). Dark mode, which the spec leaves out of
   the first version, can come later as one module, `win::dark`, that falls
   back to light when an export is missing.
2. **The window's Win32 `unsafe` lives in one module,** `win`, inside
   `gozler-ui` (below).

## Where the `unsafe` lives

`AGENTS.md` keeps Win32 `unsafe` in `gozler-winfs`, so that it stays in small,
reviewed blocks behind safe functions, and every decision stays in code that
is tested on any OS. Window code cannot go into `gozler-winfs` without breaking
that crate's contract: it uses `windows-sys` only and depends on no other
workspace crate, and the window needs the `windows` crate for COM and the
core's geometry types.

`gozler-ui` keeps the same two protections inside one crate:

- Every `unsafe` block is in `src/win/`, the smallest block that needs it,
  behind a safe function. `core` and `app` start with
  `#![forbid(unsafe_code)]`, so the compiler rejects `unsafe` anywhere else.
- `gozler-ui` sets `[lints] workspace = true`, as every gozler crate does, so
  each `unsafe` operation states its own scope (`unsafe_op_in_unsafe_fn`) and
  each `unsafe` block carries a `SAFETY` comment
  (`clippy::undocumented_unsafe_blocks`).
- Every decision is in `core`, which has no Win32 types and is tested on
  Ubuntu, as `gozler-core` is beside `gozler-winfs`. `gozler-ui` is a binary
  and `app` is `cfg(windows)`, so on Ubuntu nothing calls `core` outside its
  tests; `core` starts with `#![cfg_attr(not(windows), allow(dead_code))]`,
  the attribute that `gozler-cli`'s `failure.rs` puts on its items, so CI's
  `-D warnings` passes there.

A separate crate for the window's Win32 layer would add a `Cargo.toml`, a public
API and its rules, and `deny.toml` and CI entries, and protect nothing more.
`gozler-winfs` is a crate because two programs share it and `deny.toml` must
stop it from depending on `gozler-core`; the window's layer has one user. If a
second program ever needs it, `src/win/` moves into a crate as it is, because
only the core's geometry types cross into it.

`AGENTS.md` changes its rule to name `gozler-winfs` and `gozler-ui`'s `win`
module, so that a later reader does not move window code into the filesystem
crate.

## Architecture

**A core with every decision, a thin Win32 layer, and no GUI framework.** The
core decides what to search, which rows to fetch, which replies are current,
what the selection holds, which commands are enabled, where each control goes
and what every caption says. It is plain Rust with no Win32 types, so its
tests run on every OS. The `win` module wraps the Win32 calls the window uses
in safe functions and holds every `unsafe` block. Between them, the window's
message handlers in `app` turn Windows messages into calls on the core, and
the core's answers into calls on `win`. The dialogs are templates in a `.rc`
file, so Windows runs them. Every caption comes from a JSON catalogue, so the
language can change while the window runs.

```text
gozler-ui.exe
├─ UI thread
│   ├─ app:  message handlers for the main window and the dialogs (safe Rust)
│   ├─ win:  window class, controls, fonts, dialogs, shell calls (all unsafe)
│   └─ core: Model, Query, Results, Selection, Commands, Layout, Strings (no Win32 types)
└─ one thread per daemon request ── gozler-client ──\\.\pipe\gozler-v1──> gozlerd
```

The type names below, such as `Fetch`, `Accepted` and `Requests`, are sketched
in [the plan's API sketches](ui-plan.md#api-sketches). The sketches may change
during the work; the rules here change only by a design decision.

### Alternatives considered

| Approach | Verdict | Why |
| --- | --- | --- |
| Free functions over one global `RefCell`, as the prototype did | rejected | a borrow held while Win32 calls back into the window procedure panics (rule 1 below); no decision can be tested without a window |
| `winsafe` 0.0.29 | rejected | one maintainer and no stability promise before 1.0; its DPI helpers read the system DPI, not the DPI of the window's monitor; the virtual list still needs raw `WM_NOTIFY` handling; the window would use a small part of a large API |
| `native-windows-gui` | rejected | no release since September 2022 |
| Messages and an update function (Elm style), with a view diffed against the controls | rejected | the list asks for each cell's text inside `LVN_GETDISPINFO` and needs the answer before it returns, so it cannot wait for an update cycle; a diffing layer is a GUI framework |
| A general command list that handlers return and one applier runs | rejected | the same `LVN_GETDISPINFO` constraint; the core's typed return values (`Accepted` and `Fetch`) already tell each handler what to do |
| A widget trait hierarchy with a layout engine | rejected | the window has one fixed layout and six small dialogs; the hierarchy would be more code than the window |
| **Core, thin `win` layer, `.rc` resources** | **chosen** | the least code that keeps the failures below out by construction |

### Rules that keep the first version from needing a rewrite

Each rule names the failure it prevents. These are the decisions that are
expensive to change after the window works, because every handler depends on
them.

1. **Win32 calls back into the window.** Many calls send messages to the same
   window procedure before they return: `SetWindowTextW` on the search box
   sends `EN_CHANGE`, `SetWindowPos` sends `WM_SIZE`, and
   `LVM_SETITEMSTATE` sends `LVN_ITEMCHANGED`. Calls with their own message
   loop dispatch every message of the thread, replies from the daemon
   included: `TrackPopupMenuEx`, `DialogBoxParamW`, `IFileOpenDialog::Show`
   and `MessageBoxW`. A `RefCell` borrowed across such a call panics, and only
   when Windows happens to send that message. Here a Win32 call means any call
   that can send or dispatch a message: a call that takes a window handle, a
   COM or shell call, and a modal loop. Formatting a date or copying text into
   a buffer is not one. **Rule:** a handler changes the model in a statement
   of its own and makes Win32 calls only after that borrow ends. The model
   returns what changed, and the handler then updates the controls. In `app`,
   a borrow is bound with `let` only in a block that makes no Win32 call; the
   `LVN_GETDISPINFO` handler is that block, because it formats a row and
   copies it into the list's buffer. The core cannot break the rule, because
   it has no Win32 types and builds on Ubuntu. A dialog edits its own copy of
   the values; OK and Apply give the copy to the main window by a method call,
   which borrows the model in a statement of its own.
2. **The window's state freed while a handler runs.** File > Exit calls
   `DestroyWindow`, which delivers `WM_NCDESTROY` before it returns. If
   `WM_NCDESTROY` frees the state, the Exit handler then runs on freed memory.
   **Rule:** the window owns its state through an `Rc`, and the window
   procedure holds its own clone for the length of each message, so
   `WM_NCDESTROY` only drops the window's reference.
3. **Stopping a request.** Closing a connection from the UI thread races the
   thread that reads it. The daemon also closes a connection after 30 seconds
   with no request (`IDLE_TIMEOUT` in `gozler-daemon`'s server), so a
   connection kept for later fails after a pause. **Rule:** each request runs
   on its own thread, with its own connection and its own cancel token.
   Connecting is one pipe open, the owner check and one `Hello` round trip, as
   each `gozler` run does now. The client already polls (`PeekNamedPipe` every
   5 ms); it checks the token on each poll, and between its connect retries
   while the pipe is busy, and returns `Cancelled`. Sending needs no check: a
   request is a few hundred bytes, and gozlerd's pipe accepts 64 KiB without
   reading, so the write never waits for the daemon. The daemon notices a
   cancelled request only at its next page, so cancelling frees the window at
   once and the daemon soon after. For a list request, the limit in "Rows on
   demand" bounds that work; a cancelled export costs the daemon what a whole
   CLI export costs before it stops.
4. **One cancel for different purposes.** A request has a purpose: `Results`
   (the list's rows), `Selection` (the selection's rows, or a selected
   folder's size), `Status`, or `Export`. If selecting another row cancelled
   "the current request", it could cancel the list's rows. **Rule:** at most
   one request of each purpose is live. `Requests::start` cancels the live
   request of the new request's purpose and no other; that is its only rule.
   The model records which purposes are in flight, so it is the one owner of
   "an export runs": it never asks for a second export, and Export results…
   is disabled from that fact. Requests of different purposes run at the same
   time on separate connections, which the daemon serves in parallel, so a
   slow folder size never delays the list. Each request carries the
   generation the model gave it, and the model drops a reply whose generation
   is not the newest of its purpose; that is the only staleness check.
   Exit cancels every request; hiding the window cancels none.
5. **Replies reaching a destroyed window.** A message posted to a destroyed
   window's handle can reach another window that reuses the handle.
   **Rule:** replies travel on one unbounded channel; `PostMessageW` only wakes
   the window and carries no data. The window joins every request thread in
   `WM_DESTROY`, while its handle is still valid, so no wake-up can reach a
   reused handle. "Every" includes the cancelled threads: a cancelled thread
   runs until its next poll and still holds a `Waker`, so `Requests` keeps its
   handle until it has been joined. The channel is unbounded because a thread
   blocked on a full channel, while the UI thread waits to join it, would
   never return.
6. **Pixels mixed with device-independent pixels.** Per-monitor DPI touches
   every size: layout, fonts, column widths and icons. Added later, it means
   finding every number. **Rule:** sizes are designed in `Dip` and converted
   to `Px` only in `Layout::compute` and in the font and icon code. They are
   distinct types, so the compiler rejects a mix.
7. **Layout spread over handlers, or decided by hand.** **Rule:** one pure
   function computes every control's rectangle from the client size, the DPI,
   which panes are shown, the inspector's height, the font height and the
   measured width of each caption that the window places in code (the
   match controls, the filter labels and buttons, the status bar's longest
   messages). `WM_SIZE`, `WM_DPICHANGED` and a language switch only apply its
   result. The margins and gaps are named constants in `core::layout`, taken
   from Microsoft's Windows desktop layout guidelines (spec: Look), so no
   control's position is a number typed in a handler. The function also gives
   the window's minimum size, which leaves out the filter pane's height: when
   the window is shorter than the pane, the function gives the pane a scroll
   range instead. A new filter or a hidden pane changes that function and its
   tests.
8. **Keys that two owners want.** An accelerator for `Ctrl+A` or `Ctrl+C`
   takes those keys from the search box, and `IsDialogMessageW` turns `Enter`
   into `IDOK` and `Esc` into `IDCANCEL` before any control sees them.
   **Rule:**
   - The accelerator table holds the command shortcuts in use: the person's
     from Settings > Keyboard, else the command table's defaults (`Ctrl+F`,
     `Ctrl+E`, `Ctrl+,`, `Ctrl+C`, `Ctrl+Shift+C`, `Ctrl+A`, `F1`, `F5`,
     `Alt+Enter`, and `Ctrl+Enter` for Open file location). Copy and Select
     all act on the focused control, so with the search box focused they
     send it `WM_COPY` and `EM_SETSEL`. The Keyboard page refuses every key
     that a focused control uses (spec: The Keyboard page), so no shortcut
     can take one.
   - The main window handles `IDOK` and `IDCANCEL` by the focused control:
     `Enter` in the list opens the selection, `Enter` in the search box runs
     the search, and `Esc` clears the search box.
   - The message loop takes a few keys from `WM_KEYDOWN` before
     `IsDialogMessageW`, by the focused control, so no `WM_CHAR` follows
     them. In the search box, `Down` moves focus to the list and `Ctrl+A`
     selects the box's text. In the list, `Ctrl+A` selects every row,
     `Ctrl+C` copies and `Ctrl+Shift+C` copies as path.
   - Shift+F10 and the Menu key reach the list as `WM_CONTEXTMENU` with no
     point, and the handler opens the context menu at the focused row.
9. **Commands that disagree between surfaces.** The menu bar, the context
   menu, the keys, the match controls, the header and the inspector's buttons can
   each grow their own idea of a command. **Rule:** `core::commands` holds one
   row for each command of the spec's Commands table: its ID, its catalogue
   key, its key, its menu and group, and a function of the model that says
   whether it is enabled and checked. Every surface reads that row: the menus
   are built from it, `WM_INITMENUPOPUP` sets each item's state from it, and
   the match controls and buttons take their state from it after each
   `Accepted`.
   Every surface calls the same `MainWindow` method for a command. A command
   chosen from a menu with its own loop (`TrackPopupMenuEx` with
   `TPM_RETURNCMD`) runs after the menu returns, and the handler asks the
   model again whether it is enabled; when the selection was cleared while the
   menu was open, the command does nothing (spec `T11`).
10. **The selection's meaning drifting from what the user selected.** The
    daemon keeps no result set, so a position names whichever file is there
    now, and a selection that is read again later can name other files.
    **Rule:** the selection is a set of files, built at one moment, and every
    action reads it ("The selection" below). It is never built from rows of
    two moments, such as cached rows and rows fetched later, because an insert
    between them would list one file twice and miss another.
11. **A panic with half-changed state.** A panic cannot unwind out of a window
    procedure (`extern "system"`), and the release profile unwinds. **Rule:**
    a panic hook writes the message to `ui-error.log` in the settings folder,
    shows a message box with no owner window from a new thread and waits for
    it, then aborts the process. The message box runs on its own thread so
    that the UI thread dispatches no message while its state is half changed,
    and it has no owner because disabling an owner would wait for the UI
    thread, which is waiting for the message box. The rule is the same on
    every request thread, so a dead request never leaves a window that waits
    forever.
12. **Text left in the old language.** A language switch must change every
    caption on screen, but a text that was built and stored earlier stays in
    the language it was built in. **Rule:** the model and the controls' state
    hold facts, such as counts, phases, typed failures and parse errors, and
    never display text. Every caption comes from a function of those facts
    and the current `Strings`. `MainWindow::apply_text` calls every such
    function: at creation, after a language switch, and after a change to the
    Windows regional format. A handler that updates one caption after a reply
    calls the same function, so the two paths cannot show different text. The
    list's cells are formatted when Windows asks for them, so a redraw
    updates them.

Two smaller rules. Every user-visible string is a key in `en.json` (see "Text
and languages"), and no English is written in the code or in the `.rc` file.
`gozler-ui` calls Win32 only through the `windows` crate; values from
`gozler-winfs` and the client cross into it as Rust types, never as raw
handles.

### Shared code

| Rule | Owner, used as is |
| --- | --- |
| Query syntax and its errors | `gozler_core::query::parse`, for the typed text and for each filter's text; the window parses with no working folder, so a relative path is the parser's `RelativePath` error, which asks for the full path |
| The `-r` test, the `-parent` test and the scope it sets | `gozler-core`'s request rules, which `build_search` and the window both call |
| How a name term matches: anywhere, whole words, from a word start or from the name's start | `gozler_core::query::parse` and the engine; the window passes its match mode as the parser's default for every name term, as `-ww` passes whole words |
| Which part of a name or path a search matched, for the bold part of a cell | `gozler-core`'s matching, which returns the ranges of each match; the window asks only for the visible rows and draws them with `NM_CUSTOMDRAW` |
| `dm:` resolution with the local clock and time zone | `gozler_core::query::dates` |
| The size words' ranges | one table in `gozler-core` that the parser and the filter pane both read |
| A folder's size: the sum of the known sizes of the files below it | `gozlerd`'s index, which keeps it for each folder and sends it as the folder's size; `size:`, the size sort and the output read it as they read a file's size |
| The search syntax that Help > Search syntax shows | `gozler_core::escompat::search_syntax`, beside `help`, which renders the search terms, `SEARCH_FUNCTIONS` and the attribute `LETTERS` with the sections of the full reference, without color; the dialog shows its text |
| Whether a volume's results are complete | `VolumePhase::is_complete` |
| Pipe server owner check, version handshake, the service's state | `gozler-client`, with `gozler-winfs` |
| A scope folder that does not exist is invalid input (DESIGN.md) | `gozler-client`, which both programs call before they send a scoped search; in the window, `app` calls it and passes the result to `Model::start_search`, because the core makes no filesystem call |
| Binary results pages | `gozler_core::proto` |
| CSV, TSV and TXT export, and its date format | `gozler_core::output` |

The window builds its own `SearchRequest` in `core::query`, not through
`build_search`, because `build_search` reads the command line's `EsPlan`,
parses one query string and always asks for rows from offset 0, while the
window combines several parses and asks for rows at any offset. So the rules
that both need move out of `build_search` into public functions that it
calls, and the window calls the same functions. Each rule keeps one owner.

Changes these need:

- `regex_test` in `gozler_core::query::parse` becomes public, and the
  `-parent` rule (a `parent:` test that is also the scope when no other scope
  is set) becomes a public function in `gozler_core::escompat::request` that
  `build_search` calls.
- `ParseOptions::whole_word` becomes `term_match`, an enum of the four ways a
  name term matches: anywhere, whole words, word start and name start. The
  new search functions `wordstart:` and `startwith:` set it for one term, as
  `ww:` does, and the engine gains the two tests. A daemon that does not know
  them cannot read the request, so the protocol version rises and the window
  needs a daemon of its own release, as it already does for the version
  handshake. The window's Regular expression mode is the existing `-r`.
- The size ranges move out of the `match` in `parse_size` into a table that
  the parser reads, with no change to what `size:` means.
- **Folder sizes** (spec: Results list). Each volume's index keeps one sum
  for each folder: the known sizes of every file below it, each name of a
  hard-linked file counted, as `-get-folder-size` counts today. The size and
  date pass computes the sums when it ends; until then a folder's size is
  unknown, as its files' sizes are. Every later journal change keeps them
  exact: a file created, deleted or resized changes each folder above it by
  the difference, and a moved file or folder moves its size from the old
  folders above it to the new ones. A results row already carries a size
  for any item (`proto::binary`'s `has_size`), so the layout does not
  change. The engine's size test and the size sort read a folder's sum as
  they read a file's size; the totals still add files only, because a
  folder's size is already the sum of its files.
- `QueryError::Malformed` carries a typed reason in place of its `detail`
  string, so the window can say in every language what is wrong and how to
  fix it. Its `Display` writes the same English as today, so `gozler`'s
  messages do not change.
- The CLI's date formatter for exports, `format_date_local` in `gozler-cli`,
  moves into `gozler_core::output`, so the window's exports and the CLI's
  write the same dates.
- The blocking pipe client in `gozler-cli/src/client.rs` moves into a library
  crate, `gozler-client`, that `gozler-cli` and `gozler-ui` both use, with the
  service-state query and the scope-folder check. Its error type keeps every
  distinction that `gozler-cli`'s `failure.rs` maps today, so the CLI's exit
  codes and fix-it messages do not change:
  - the service is not running, with its state: stopped, disabled, not
    installed (with the folder and whether `gozlerd.exe` is beside it), or
    unknown;
  - the service runs but the pipe is not ready;
  - the pipe cannot be opened or read (`Communication`);
  - the owner check failed;
  - the daemon is older than the client;
  - the daemon refused the request, with its `ProtoError` code and its text,
    because the code decides the CLI's exit code;
  - the connection was lost;
  - `Cancelled`.

  The window collapses them into the failures the spec's table shows;
  `Communication` shows as a lost connection, because both mean the pipe
  failed after the service was found. Its
  read loop takes a cancel check (rule 3); `gozler` passes one that never
  cancels. Its connect takes the pipe name and whether to check the server's
  owner, so its tests serve their own pipe instead of the running daemon's.
- `deny.toml`'s `wrappers` for `gozler-core` gain `gozler-client` and
  `gozler-ui`.
- On Linux, CI compiles `gozler-ui` and `gozler-client` as stubs, as
  `gozler-cli` does with its `cfg(not(windows))` `execute`. The core has no
  Win32 calls, so its tests run on Ubuntu.

### Requests

The UI thread never waits for the pipe. Each request runs on its own thread
(rules 3 to 5).

- At start, the window asks for status beside the first search, because only
  a status reply names the volumes: the Location list, the index state in
  status part 4 and the decision to poll all need it.
- The window asks for status every 2 seconds while `Model::polls_status` says
  so: the service is not running, runs but does not answer yet, or the
  connection was lost, a volume in scope
  is bootstrapping or rescanning, or the Settings Index page is shown. One
  timer serves every reason.
- A results page carries only the combined flags `volumes_incomplete` and
  `metadata_pending`. When a page sets a flag that the latest status does not
  explain, the model asks for status once.
- When a status reply shows that the daemon answers again, or that every
  volume in scope has become complete, the model runs the search again.
- A refused request, a failed owner check and a daemon older than the window
  start no polling, because the same request would fail the same way.
- A failure means something different for each purpose. A `Results` failure
  shows its message in the list. A `Status` failure only updates whether the
  daemon answers. A `Selection` failure leaves the selection unread, with its
  commands disabled, and shows the failure in the list when the results also
  failed. An `Export` failure is the spec's export message box and leaves the
  list alone.

### Rows on demand

1. A search asks for the first chunk of `CHUNK_ROWS` rows in the chosen sort.
   At offset 0 with a limit, the daemon selects the top rows instead of
   sorting every match.
2. The reply carries the total. The window sets the list view's item count
   (`LVM_SETITEMCOUNT`) to the total, or to 100,000 when the total is larger.
3. When the list view needs rows that are not cached (`LVN_ODCACHEHINT`, or a
   jump with the scroll bar or `End`), the window asks for that chunk with
   `offset`. Until it arrives, those rows show empty cells.
4. At most one chunk request is in flight. When it arrives, the window asks
   again for whatever the visible rows still lack, so fast scrolling never
   queues a request for every chunk it passed.
5. Every reply carries the total. When it differs from the count the list
   shows, the window sets the count again.
6. The cache keeps at most 32,768 rows in whole chunks and drops the chunk
   used least recently. Memory therefore stays the same for any number of
   matches.

**The chunk size.** Every request scans every match before it sorts (the
candidate collection in `run_query_impl` and `order_top` in
`gozler_core::query::engine`), about 0.3 seconds for the empty search on the
owner's machine. That scan dominates, so a larger chunk costs the daemon
little more and halves the waits on a long scroll. `CHUNK_ROWS` is a named
constant between 4,096 and 16,384 rows, chosen in stage 3 by measuring the
wait per chunk at both ends.

**Why the list stops at 100,000 rows.** After the scan, each volume sorts its
first `offset + limit` matches and builds a full path for each of them, under
the volume's index lock, and the server then skips the first `offset` rows
(`per_vol_limit` in `gozler-daemon`'s server). Pressing `End` in a list of 3
million entries would make the daemon sort and build about 3 million paths on
each volume to send one chunk, while the volume's journal updates wait for the
lock. With the limit, the worst chunk builds about 100,000 paths plus one
chunk. A result snapshot kept by the daemon for each search would remove the
limit and the repeated scan; it needs a daemon-wide table of snapshots with a
lifetime beyond one connection, checks for entries deleted since, and a new
protocol version. The window's cache and selection would not change.

### The selection

The spec's rule: the selection is a set of files, fixed when the user makes
it (rule 10).

- **The list owns which positions are selected.** The model never rebuilds
  the selection from the list's notifications, because comctl32's rules for
  anchors, Ctrl+Shift, the marquee and the keyboard would then have a second,
  incomplete copy. One user action sends several notifications
  (`LVN_ITEMCHANGED` for one item or with item -1, `LVN_ODSTATECHANGED` for a
  range), so the handler posts itself one message and, when it arrives, reads
  the selected positions once with `LVM_GETNEXTITEM` and `LVNI_SELECTED`. A
  selection reaches at most 100,000 rows, so the read is at most 100,000
  in-process messages, tens of milliseconds.
- **Every selected row cached:** `Model::set_selection` builds the files from
  the cache. They are the rows the user sees.
- **Otherwise:** the selection is `Reading`, and 150 ms after the last change
  the model asks for one `Selection` request for the rows from the first
  selected position to the last. The request thread keeps the selected
  positions' rows and the rows of the chunks that the cache holds inside the
  span, and drops every other row, so a Ctrl+click on row 0 and on row
  99,999 carries two files and at most the cache, not 100,000 rows. The
  reply builds the files, and its chunks replace the cached ones, so the list
  shows the files the selection holds, from the same moment. A new selection
  raises the `Selection` generation, so an older reply is dropped.
- **The files** are kept in selection order, each with its full path, whether
  it is a folder, its size and its date, and at most once: the list never
  selects one position twice, and one moment's rows never hold one path
  twice.
- **Every action reads the files:** Open, Open file location, Properties,
  Search in this folder, the three copies and the inspector's sums. No action
  fetches.
- **A new search** sets the selection to none in the same `accept` that
  accepts its first rows, raises the `Selection` generation, and the handler
  clears the list's selection; the notification that follows reads an empty
  selection, which agrees.

### Copy

Copy, Copy as path and Copy name read the selection's files on the UI thread.
`ClipboardData` in the core knows the exact size of each format from the files
before it writes a byte: the `DROPFILES` header with its `pFiles` offset and
`fWide` set, then the double-NUL path list for `CF_HDROP`, or the text for the
other two. `win::shell` allocates one `GMEM_MOVEABLE` block of that size,
which `SetClipboardData` requires, and the core writes into the block's
memory as a slice. The UI thread opens the clipboard with the main window,
empties it and sets the block. After a successful `SetClipboardData` Windows
owns the block; after a failure the window frees it. So a copy holds one
block and no second buffer. When `OpenClipboard` fails because another
program holds the clipboard, the window tries again five times over 100 ms,
then shows the spec's message.

### Export

Export is one `Export` request with no limit. The request thread writes each
page through `gozler_core::output` as it arrives, as `gozler -export-csv`
does, so it costs the daemon what the CLI's export costs. It sends a progress
reply about twice a second, which status part 3 shows. A failure to create or
write the file is an `ExportFailed` outcome with the path and Windows' error,
apart from the daemon failures. A cancelled or failed export deletes its
partial file. The save dialog is `IFileSaveDialog`.

### Feedback and fixes

The spec's buttons for daemon failures run the commands that `gozler`
already prints for them. `win::shell` runs them with `ShellExecuteExW` and
the `runas` verb, so Windows asks for administrator permission; the window
never stores or passes credentials. After the command, the status poll that
the failure already started notices the service and runs the search again.
Download gozler and Open Services open the releases page and `services.msc`
the same way, without elevation.

### Transitions

The spec's transitions table lists, in the user's terms, what each event
changes. In the model they are calls of `Model::accept`, `start_search` and
`set_selection` from a given `Shown`, set of purposes in flight and
`Selection`. Each row whose "Checked by" names a core test is one test case
of those functions with the row's ID in its name, so the table and the tests
cannot drift. The other rows, such as a language switch or a DPI move, are
not model calls, and their stage checks prove them.

### Text and languages

The window follows TinyTorrent2's localisation contract (spec: owner decision
5), and uses only the parts of it that the window needs.

- **Catalogues.** `crates/gozler-ui/resources/en.json` holds every key and
  its English text. Another language is `<tag>.json` with the same keys.
  Keys are grouped by surface in one level of objects, with snake_case names,
  such as `menu.export` and `status.results_other`. Placeholders are
  numbered, `{0}`, and a translation may reorder them. A counted message has
  an `_one` and an `_other` form. Text is plain data, never markup. A menu
  item, a dialog label or a button holds its access key as `&` in its text,
  so each language chooses its own letters.
- **Duplicate keys.** `serde_json` keeps the last of two equal keys without a
  word, so the catalogue loader and its test read a catalogue with a map
  visitor that rejects a key it has already seen.
- **Embedded.** `include_str!` puts each catalogue in the executable. A table
  in `core::text` lists each language's tag, its own name and its catalogue.
  The Language menu and the Language list in Settings read that table, so
  listing the languages loads no
  catalogue.
- **`Strings`** is one loaded language, and it does not change.
  `Strings::load` parses the language's catalogue and English. A message
  comes from the language, then from its parent language, then from English,
  always as a whole message with all its plural forms. A translated message
  whose key English lacks, or whose placeholders differ from English, is
  ignored, so the English shows. A key that English lacks shows as its
  dotted name, so a person sees the gap and nothing panics.
- **Plural forms** use the rule that English has: `_one` for 1, `_other`
  otherwise. The first shipped language that needs another rule takes it
  from the ICU that Windows includes (`uplrules_select`); no ICU is bundled.
- **Numbers, dates and times** follow the Windows regional format, and
  Windows formats them: `GetNumberFormatEx`, `GetDateFormatEx` and
  `GetTimeFormatEx`. The core's text functions take a `Numbers` value that
  `win` implements with those calls and the tests implement with English
  formatting, so the regional rules have one owner, Windows. A
  `WM_SETTINGCHANGE` with `intl` calls `apply_text` (rule 12). Choosing a
  language never changes the regional format. Size units, such as "MB", are
  catalogue messages.
- **The switch runs on the UI thread.** Loading parses an embedded
  catalogue of a few tens of kilobytes, which should take well under a
  millisecond, so the switch ends before the window handles the next
  message, and no choice can overtake another. Stage 8 checks that no pause
  shows. The old `Strings` is dropped after the switch. An embedded catalogue
  that does not parse is a build defect that the catalogue test catches; at
  run time the window then keeps its current language.
- **The core reports codes, not sentences,** because the window translates
  what the core reports. The window maps each `QueryError` variant with its
  typed reason and each `VolumePhase` to a key with a `match`, which fails to
  compile when a variant is added. Attribute letters are `char`s, so the
  catalogue test checks that every letter of `Attributes::all_letters()` has
  its key. Text that comes from a library or from the daemon, such as a
  regular expression error or a refusal, is shown as English detail after a
  translated sentence.
- **Sizes.** Controls that the window places take their width from measured
  text (rule 7). Dialog templates leave the room the spec names. The
  pseudo-language shows where a control still clips.
- **The pseudo-language** `qps-ploc`, the tag that Windows uses for its own
  long-text pseudo-locale, is generated from `en.json` by `build.rs`, only
  with the crate's `pseudo-language` feature. A message of up to 12
  characters becomes twice as long, a longer one 50 % longer, with accented
  letters; placeholders and access keys are kept. A shipped executable never
  contains it.

### Modules

```text
gozler-ui/src
├─ main.rs           cfg(windows): start the window; otherwise a stub
├─ core/             no Win32 types; the tests run on Ubuntu
│  ├─ model.rs       Model: query, shown results, purposes in flight, panes, latest status
│  ├─ query.rs       Query, Filters, MatchMode, Match -> SearchRequest
│  ├─ results.rs     Results: the chunk cache and the chunk in flight
│  ├─ selection.rs   Selection and its files; SelectionSums; ClipboardData
│  ├─ fetch.rs       Purpose, Generation, Fetch, Fetched, Outcome, DaemonFailure
│  ├─ commands.rs    the command table: IDs, keys, menus, enabled and checked
│  ├─ layout.rs      Dip, Px, Dpi, Rect, the spacing constants, Layout::compute
│  ├─ settings.rs    Settings: load and save ui.json
│  └─ text.rs        Strings, the language table, Numbers, every caption function
├─ win/              cfg(windows); every unsafe block
│  ├─ window.rs      Hwnd, window class, the window procedure trampoline
│  ├─ dialog.rs      modal dialogs from .rc templates
│  ├─ controls.rs    Edit, Button, ComboBox, ListBox, ListView, StatusBar, Tooltip
│  ├─ notify.rs      WM_NOTIFY and WM_COMMAND decoded into enums
│  ├─ gdi.rs         Font, Fonts, text measurement, the system image list
│  ├─ shell.rs       open, open file location, properties, clipboard block, folder picker, save and program dialogs, run as administrator, the `Run` value
│  ├─ shortcut.rs    RegisterHotKey, and the keyboard hook thread for Windows-key shortcuts
│  ├─ tray.rs        the tray icon, its menu and its notification
│  ├─ locale.rs      the Numbers implementation, date and time formatting, the display languages
│  └─ wake.rs        Waker
└─ app/              cfg(windows); safe Rust
   ├─ main_window.rs MainWindow and its message handlers
   ├─ dialogs.rs     Settings, the command shortcut dialog, Custom date, Custom size, Search syntax, About
   ├─ icons.rs       the system image index for each extension
   └─ requests.rs    Requests: one thread per Fetch, the cancel tokens, the reply channel
```

Beside `src`, `resources/` holds the catalogues, `gozler-ui.rc` and the icon.

`gozler-ui` depends on `gozler-core`, `gozler-client`, `windows` (only the
features it uses), `serde` with `derive` for `Settings`, `serde_json`, and
`jiff`, because `QueryStart` and the export date formatter take `jiff`
values. Its build dependencies are `embed-resource` and, for the
pseudo-language, `serde_json`.

### The .rc file

Windows declares dialogs, the icon, the version and the manifest in a resource
script, and the window needs an icon and version information in any case.
`build.rs` compiles `gozler-ui.rc` with `embed-resource`, which runs the
Windows SDK's `rc.exe`, and skips it on other targets. The script holds no
text, because English has one source, `en.json` (rule 12). It holds:

- the Settings dialog and its pages, the command shortcut dialog (Settings >
  Keyboard, Add… and Edit…), and the Custom date, Custom size, Search syntax
  and About dialogs, as
  `DIALOGEX` templates with control IDs and empty captions. `DialogBoxParamW`
  then gives the modal loop, the Tab order, `Enter` and `Esc`, and
  per-monitor DPI scaling, which Windows applies to template dialogs. The
  Settings categories are child dialogs, one template each. Each dialog's
  table of control IDs and keys gives `apply_text` its captions in
  `WM_INITDIALOG`;
- the manifest: common controls 6, per-monitor DPI awareness v2 and the
  supported OS list;
- the icon and the version information.

The main window is created in code, because its layout follows the window size
and the panes (rule 7). So are the menu bar, the context menu and the
accelerator table, from the command table and the person's shortcuts
(rule 9). The shortcut that a menu
item shows comes from the same row as its accelerator, so they cannot differ.
The modifier names come from the catalogue, because German Windows writes
`Strg` for `Ctrl`.

### Execution paths

**Start.** `main` installs the panic hook and calls `CoInitializeEx` with an
apartment-threaded model, because the folder picker and `SHGetFileInfoW` need
COM. It finds the settings folder (`--data-dir`, or the known folder
`FOLDERID_LocalAppData` and `gozler`), loads `Settings`, loads the `Strings`
of the saved language (or of Windows default, from
`GetUserPreferredUILanguages`), builds the menu bar and the accelerator table
from the command table, and creates the window with an `Rc<MainWindow>`. The
trampoline stores the `Rc` in `GWLP_USERDATA` on `WM_NCCREATE`. `WM_CREATE`
creates the controls with the fonts for the window's DPI, fills `controls`,
and calls `apply_text` (rule 12). Every other handler starts by reading
`controls` and does nothing while it is empty, because `EN_CHANGE` and other
notifications can arrive while `WM_CREATE` runs. The first `WM_SIZE` applies
`Layout::compute`. After `create_window` returns, `main` starts the first
search and the first status request. `--sign-in`, which the `Run` value
passes, starts gozler as "Window lifetime" in the spec says: in the
notification area when it keeps running there, otherwise minimized.

**One window per session.** Before it creates the window, `main` creates the
named mutex `Local\gozler.ui`. When the mutex already exists, `main` finds the
first window by its class name, calls `AllowSetForegroundWindow` for its
process, posts it a registered message and exits; the first window shows
itself and focuses the search box (`MainWindow::show_window`, command C24).
With `--data-dir`, the mutex and the class name end in an FNV-1a hash of the
canonical folder (`app::instance_suffix`), because a mutex name allows no
backslash after `Local\`.

**The shortcut.** `win::shortcut` owns it, and `MainWindow` sees only "show
the window". Two ways reach the same handler:

- A shortcut with `Ctrl`, `Alt` and `Shift` only is a `RegisterHotKey` on the
  main window, and `WM_HOTKEY` calls `show_window`. A failed registration is
  the spec's "another program holds it", in the dialog and at start.
- A shortcut with the Windows key is a low-level keyboard hook
  (`WH_KEYBOARD_LL`), because Explorer registers most Windows-key
  combinations, such as `Win+S`, so `RegisterHotKey` fails for them. The
  hook runs on its own thread with its own message loop: Windows removes a
  low-level hook whose callback does not return within
  `LowLevelHooksTimeout`, and the window's thread can be inside a modal loop
  or a long handler. The callback swallows the key's down and up events while
  a Windows key is held and posts the registered show message to the main
  window. Before the Windows key's release it sends one unassigned key
  (`0xE8`) with `SendInput`, so the shell sees a combination and does not
  open Start. The hook exists only while a Windows-key shortcut is set:
  choosing another shortcut or none unhooks it, and so does exit, so Windows'
  own use of the key returns at once and nothing needs restoring.

**The tray.** `win::tray` adds the icon with `Shell_NotifyIconW` and
`NOTIFYICON_VERSION_4` while gozler keeps running in the notification area
(Settings > General), and removes it when that is turned off and at exit. `NIN_SELECT` and `NIN_KEYSELECT` call
`show_window`; `WM_CONTEXTMENU` calls `SetForegroundWindow` on the main
window and then `TrackPopupMenuEx`, as the `Shell_NotifyIconW` documentation
requires so that the menu closes on a click elsewhere. The registered
`TaskbarCreated` message adds the icon again after Explorer restarts. The
first hide shows the spec's notification through `NIF_INFO` and records it
in `ui.json`. `WM_CLOSE` hides the window while gozler keeps running in the
notification area and exits otherwise; File > Exit and the tray's Exit always exit.

**Search with gozler.** The setting writes or deletes the `gozler` verb
under `HKCU\Software\Classes\Directory\shell` and
`...\Directory\Background\shell`, with the command
`"<this gozler-ui.exe>" --location "%V"`. A second start passes the folder
to the first window with the registered show message's `WM_COPYDATA`.

**Use Total Commander.** The button sends one search through the window's
`gozler-client` worker, as a search from the search box goes:
`file: totalcmd*.exe`, sorted by date modified, newest first. The window
keeps the rows whose name is `TOTALCMD64.EXE`, else `TOTALCMD.EXE`, takes
the first, and writes the spec's command with its full path into Open
folders and Open files. The dialog stays responsive while the request runs,
and closing it cancels the request (rule 4).

**Check for updates.** At start, at most once a day, a thread asks
`https://api.github.com/repos/SynoSoftware/goz/releases/latest` with WinHTTP
and compares the tag with the window's version. A newer one posts its tag to
the window; a failed request is ignored, because no update is worth a
message. Off, the thread never starts.

**Service settings.** The Index and Include pages read and change the
service's settings: the folder choices, the name exclusions and the optional
index contents. The folder tree in Indexed folders (Include > Modify…) is a
tree view with `TVS_CHECKBOXES` and `TVS_EX_PARTIALCHECKBOXES`; it lists a
folder's subfolders with `FindFirstFileExW` when the folder is first
expanded, and derives each check box from the nearest saved choice above it.
The preview line asks the service for a dry-run count of the page's draft,
300 ms after the last change, through the window's worker; a newer change
cancels the older request (rule 4). How the service stores, checks and
applies the settings, how it counts the dry run without changing anything,
and how a request proves an administrator sent it, belong to the service
settings plan, which is not written yet; these pages wait for it.

**Start at sign-in.** The check box reads two values: the `gozler` value
under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, and the
`gozler` value under `...\Explorer\StartupApproved\Run`, where Task Manager
records that it turned the entry off. Saving the setting on writes the `Run`
value (`"<this gozler-ui.exe>" --sign-in`) and deletes the `StartupApproved`
value; saving it off deletes both. The registry is the only copy, so the
check box and Task Manager cannot disagree. The message loop runs
`TranslateAcceleratorW`, then the keys it takes for the focused control
(rule 8), then `IsDialogMessageW` for Tab, `Enter` and `Esc`, then
`DispatchMessageW`.

**Typing a search.** `EN_CHANGE` arrives. The handler calls
`model.borrow_mut().set_text(text)`, the borrow ends, and the handler restarts
a 150 ms timer. On `WM_TIMER`, the handler runs `gozler-client`'s
scope-folder check for the Location, if one is set, then
`model.borrow_mut().start_search(scope)` raises the `Results` generation and
returns `Accepted`: a `Change` and, unless the text does not parse, the
Location folder does not exist, or the search is empty and the settings say
to show nothing, a `Fetch` for the first chunk. The
handler calls `requests.start(fetch)`, which cancels the live `Results`
request and keeps its handle until it is joined. That request's thread sees
its token at its next poll, returns `Cancelled` and ends; the new thread
connects and sends the new request. Until the new search's first chunk
arrives, the list and the selection keep the previous results, the status bar
says "Searching…", and `rows_needed` asks for nothing.

**A chunk arrives.** The request thread gathers the chunk's pages, sends one
`Fetched` on the channel and calls `Waker::wake`, which posts `WM_APP`. The
handler drains the channel. For each reply, `model.borrow_mut().accept(reply)`
returns `Accepted`, and the borrow ends. The handler starts the `then` fetch,
if there is one, updates the controls for the `change`, and sets or kills the
one 2-second status timer from `model.borrow().polls_status()`. For
`NewSearch { count }`, the handler clears the list's selection, calls
`ListView::set_count`, scrolls the list to row 0 (`LVM_ENSUREVISIBLE`), as a
new search shows its results from the top, and updates the status bar. The
visible rows are then inside the first chunk. Windows then sends
`LVN_GETDISPINFO` for the visible rows. The handler formats each cell from
`model.borrow()` and copies it into the list's buffer. For the icon, it copies
the extension out, ends the borrow, and looks the image index up in `icons`; a
missing extension calls `SHGetFileInfoW` with no borrow held. A `Stale` reply
changes nothing.

**Scrolling to row 90,000.** The list sends `LVN_ODCACHEHINT` for the visible
range. `model.borrow_mut().rows_needed(range)` returns one `Fetch`, and the
handler starts it. Until the chunk arrives, `LVN_GETDISPINFO` finds no row and
leaves the cells empty. When the chunk is accepted with `Change::Rows`, the
handler sets the count if it changed and redraws those rows. It then reads
the visible range from the list (`LVM_GETTOPINDEX`, `LVM_GETCOUNTPERPAGE`) and
calls `rows_needed` again, the same path the hint takes, so every chunk
request comes from the rows visible now.

**Selecting rows.** The user Shift+clicks from row 4,000 to row 60,000. The
list sends a deselect-all, a range and a focus change; the first of them makes
the handler post itself one message. When it arrives, the handler reads the
selected positions and calls `model.borrow_mut().set_selection(positions)`.
Rows outside the cache make the selection `Reading`; status part 2 says
"Reading the selection…", and the commands that act on the selection are
disabled. 150 ms later, with no further change, the model returns a
`Selection` `Fetch` for rows 4,000 to 60,000. Its reply builds the files,
replaces the cached chunks inside the span, and enables the commands; the
handler redraws the visible rows and the inspector. A selected folder alone
is cached, so its files are ready at once, and the model returns a
`Selection` `Fetch` for the folder's totals instead.

**Copying 60,000 selected rows.** `Ctrl+Shift+C` in the list. The handler asks
the model for the copy; the model builds `ClipboardData` from the selection's
files with its exact size; `win::shell` allocates the block and the core
writes into it; the handler opens the clipboard with the main window and sets
the block (see "Copy"). No request runs.

**A context menu while results arrive.** A right-click opens the context menu
with `TrackPopupMenuEx` and `TPM_RETURNCMD`. While it is open, its loop
dispatches a reply that brings a new search's first rows, and the selection
is cleared. When the menu returns the chosen command, the handler asks the
command table whether it is enabled now; it is not, so nothing happens.

**Moving to a monitor with another DPI.** `WM_DPICHANGED` carries the new DPI
and the suggested rectangle. The handler builds new `Fonts`, calls
`Fonts::replace`, measures the captions again, scales the column widths and
moves the window to the suggested rectangle. The `WM_SIZE` that follows
applies `Layout::compute` at the new DPI. Template dialogs scale themselves.

**The daemon restarts while the user scrolls.** The chunk's thread gets a
connection error and sends `Outcome::Failed(Lost)`. The model empties the list
and shows the message; `polls_status` is now true, so the handler starts the
status timer. When a `Status` reply arrives, `accept` returns the search again
as `then`. After a `Refused` failure, `polls_status` stays false (see
"Requests").

**Exit during a search.** File > Exit calls `DestroyWindow`. In `WM_DESTROY`,
the handler calls `requests.shut_down()`: every request thread sees its token
at its next poll and ends, and the join returns. The join is bounded because
no request thread waits on anything without a token check: the client checks
it every 5 ms while it waits for the reply and between connect retries, so a
daemon that stops answering cannot hold the window open, and sending never
waits (rule 3). A request thread writing an export checks it after each page.
The plan's stage 1 tests prove that bound with a pipe server that never
answers. An export that was stopped deletes its partial file. The handler
saves `Settings` and posts quit. Windows destroys the controls, then sends
`WM_NCDESTROY`, where the trampoline drops the window's `Rc`. The trampoline
call for the Exit command still holds its own clone, so `MainWindow` and its
fonts are freed only when that call returns, after every control is gone.

**Settings.** File > Settings copies the settings out of the model and calls
`run_modal` with a dialog state that holds the copy and a `&MainWindow`. While
the dialog runs, Windows disables the main window, and the accelerators do not
run, but replies still reach the main window, which handles them as usual
(rule 1). When the Index page is shown or hidden, it tells the model
through the `&MainWindow`, and the main window sets or kills its status timer
from `polls_status`. The page keeps its own 2-second timer only to show
the model's latest status. Apply and OK check the input (spec: Dialogs), then
call a `MainWindow` method that gives the copy to the model and saves it.

**Switching the language.** A click on Language > Deutsch sends
`WM_COMMAND`, and the handler calls `MainWindow::set_language(choice)`. OK
or Apply in Settings, with Deutsch chosen in General > Language, calls the
same method after it gives the other settings to the model.
`Strings::load` builds the new strings, `model.borrow_mut().set_strings(…)`
replaces the old ones in a statement of its own, and the handler measures the
captions again and calls `apply_text`, which sets every caption:

- the menu bar's items (`SetMenuItemInfoW`, then `DrawMenuBar`), with the
  radio mark on the new choice (`CheckMenuRadioItem`);
- the column headers (`LVM_SETCOLUMNW`, which keeps their widths and order);
- the search box's cue banner (`EM_SETCUEBANNER`); the typed text is the
  user's and is not touched;
- the filter pane's labels and choices. A combo box or a list box gets its
  items again with the same selection index. A selection set by a message
  sends no `CBN_SELCHANGE` or `LBN_SELCHANGE`, so no filter changes and no
  search starts;
- the match controls' captions and the Match list's items, the inspector, the status bar's parts
  and the list's message.

`Layout::compute` runs again with the new widths. `InvalidateRect` on the list
then redraws the visible cells, which `LVN_GETDISPINFO` formats with the new
strings. Focus, selection and scroll position are never touched, so they
stay. The choice is saved in `settings.language`, which both places read.

**A panic.** The hook writes the log, shows the message box from a new thread,
waits for it, and aborts (rule 11). No handler runs with half-changed state.

### How normal changes land

| Change | Where it lands |
| --- | --- |
| A new command | a row in the spec's Commands table with its check, a row in `core::commands`, and its `MainWindow` method |
| A new column | a `Column` entry in the core, and its arm in the `GetDispInfo` handler |
| A new filter | `Filters` and its clause text, one control in `Controls`, its rectangle in `Layout::compute` |
| A new Settings category | one child-dialog template and its `DialogProc` |
| Dark mode | a `win::dark` module; every control is created in `win::controls`, so theming has one place to hook in |
| A new text | a key in `en.json`, and the function that formats it (rule 12) |
| A new language | its `<tag>.json` in `resources/` and a row in the language table |
| A right-to-left language | the main window and the dialogs created with `WS_EX_LAYOUTRTL`, which mirrors their coordinates, so `Layout::compute` does not change; whether a switch between directions must create the controls again is not verified |
| A second top-level window | another `WindowProc` with the same trampoline |
| A daemon result snapshot | a new `Ask` for rows of a snapshot; the cache, the list and the selection's rule do not change, and a selected row the user never saw becomes exact |
| Results that update when files change | a new `Outcome`, after the protocol gets change notification |

## Not verified

Each item is checked by the stage named in the plan before code depends on it.

- That `rc.exe` is on the Windows CI runner.
- That UI Automation exposes the virtual list's visible rows. If it does not
  expose the rows' cells, the probe reads them with `LVM_GETITEMTEXTW`
  through memory it allocates in the window's process. Stage 3 decides
  this, with the first rows on screen, before its live checks.
- That the keyboard hook takes `Win+S` before Windows Search opens, that the
  unassigned key keeps Start closed, and that `SetForegroundWindow` from the
  show message brings the window to the front after a swallowed key. If the
  last one fails, the hook thread calls `AllowSetForegroundWindow` for the
  window's own process before it posts. Stage 7 checks all three on the
  owner's Windows 11.
- That `StartupApproved\Run` still holds Task Manager's off mark in the
  first byte (2 on, 3 off). The format is not documented; if it changes, the
  check box reads only the `Run` value.
- That per-monitor DPI v2 rescales the Settings category child dialogs when the
  window moves to a monitor with another DPI. If it does not, the categories
  become groups of controls in the one dialog, shown and hidden.
- That the shell's system image list gives icons at the window's monitor's
  DPI. If it gives the system DPI only, `app::icons` keeps one image list per
  DPI, filled with `SHGetFileInfoW` and `SHDefExtractIconW` at that size.
- That the marquee selects rows in a virtual report list.
- That with `TVS_EX_PARTIALCHECKBOXES` the window can show the partial state
  while a click moves only between checked and unchecked. If a click also
  reaches the partial state, the window handles the click itself and sets
  the state. Stage 7 checks this.
- File Explorer's wording for the commands the window shares with it, such as
  "Open file location".
- Whether clipboard memory that Windows owns after a copy still counts in the
  window's private bytes.
