# Plan: a desktop window for gozler

**Scheduled:** not yet. The owner sets the date. The open decision in
"Decisions for the owner" comes first, because the window's code depends on
it.

## Goal

A person finds a file faster in a window than in a console: they type, see the
matches at once, narrow them with a few clicks, and open the file. The window
gives the search that `gozler` already does a Windows desktop face.

**It uses little memory.** The window stays open all day beside other work, so
it holds no index and no result set. The daemon already holds the index; the
window holds only a cache of at most 32,768 rows around the ones on screen.
The list reaches the first 100,000 rows of a search in the chosen order, so no
request for the list costs the daemon more than about 104,000 rows of work on
each volume, however many entries match (see "Rows on demand"). Export costs
what `gozler -export-csv` costs.

**It adds no search feature.** Every filter, column and sort is something
`gozler` already does today. Everything and Total Commander do more (see the
screenshots that started this plan); "Not planned" lists what is left out and
why. A new search feature goes to `gozler-core` first, then the window shows
it.

TinyTorrent2's window is the reference for where things go: the filter pane on
the left, the table in the middle, the inspector under it, the status bar at
the bottom, and settings by category. Its look and its code are not reused.

## Priorities

The owner set these, in this order. Each choice below names the one it serves.

1. **Low memory.** Essential. The window stays open all day beside other work.
2. **Usability and correct Windows behavior.** Essential.
3. **Little code to write and maintain.** Essential.
4. **A polished look.** Desirable. It never justifies substantial code.
5. **Dark mode.** Optional. It is dropped when it costs more than it gives.

## Toolkit evaluation

**Owner decision: native Win32 in Rust, light only in the first version.** It
uses the least memory, and Windows itself gives the list's selection, keyboard
navigation, column handling and accessibility, so gozler does not write them.
Slint looks better and gives dark mode for free, but that serves priorities 4
and 5. It costs more memory, a larger executable and longer builds, and its
list behavior would be code that gozler writes and keeps correct.

### How it was measured

Two throwaway prototypes of the same window were built and run on the owner's
machine (Windows 11, release builds, `lto = "fat"`). Neither is part of the
repository. Each has:

- the search box with four toggles;
- the filter pane: a list, three combo boxes, a check box and a button;
- a virtual list of 3 million synthetic rows, with shell icons and names in
  Latin, Chinese, Arabic, Devanagari and emoji;
- the inspector and the status strip;
- a menu bar, a context menu and sort arrows on header click;
- the Options window.

A script started each one and let it jump to row 1,500,000, to the last row
and back. It then selected a row, opened Options and read the process's
memory.

| | Win32 (`windows` 0.62.2) | Slint 1.18.1, software renderer | Slint 1.18.1, default renderer (FemtoVG, OpenGL) |
| --- | --- | --- | --- |
| Working set | 23.5 MB | 34 MB | 88 MB |
| Private memory | 4.6 MB | 12 MB | 120 MB |
| Threads | 4 | 10 | 14 |
| Executable | 182 KB | 13.9 MB | 13.9 MB |
| First release build | under 1 minute | 9.5 minutes | (same build) |
| Release rebuild after an edit | 9 seconds | 3 minutes 15 seconds | (same build) |
| Prototype code | 869 lines, 47 `unsafe` sites | 539 lines (353 of them Slint markup), 1 `unsafe` block | (same code) |

The Options window added under 1 MB in both toolkits. Most of the default
Slint renderer's memory comes from the OpenGL driver. To stay near the Win32
figures, Slint must be built with only its software renderer; the executable
size of that build was not measured.

### What each prototype needed

| Behavior | Win32 | Slint |
| --- | --- | --- |
| Virtual list of 3 million rows | built in (`LVS_OWNERDATA`) | built in (`ListView` with a Rust `Model`) |
| File icons in the list | the system image list, by index | each `HICON` copied to RGBA pixels (48 lines of `unsafe` GDI code); `StandardTableView` cells hold text only, so the list is a custom `ListView` |
| Selection with Shift, Ctrl and the marquee; keyboard navigation; type-ahead | built in, apart from type-ahead: a virtual list asks the window to find the row (`LVN_ODFINDITEM`), and most rows are not in the window. The marquee was not tried | written for Shift, the arrow keys, Page Up, Page Down, Home, End and Ctrl+A; Ctrl+click, the marquee and type-ahead still missing |
| Column resize and reorder, sort arrows | built in; an arrow is one header flag | resize and arrows written; reorder and double-click fit missing |
| Screen reader rows | built in (UI Automation) | `accessible-*` properties on each row; not checked with Narrator |
| Menu bar, context menu | native menus | `MenuBar` (native on Windows) and `ContextMenuArea` |
| Options as a dialog | a modal owned window, as Windows dialogs are | a second top-level window with a maximize button; modality written by hand |
| Non-Latin names | correct; emoji are monochrome, as in any GDI program | correct; emoji are monochrome |
| Look | plain Windows controls: clean, with dated buttons | Fluent controls: modern |
| Dark mode | about 90 lines of undocumented calls, and the menu bar stays white (see "Dark mode") | follows Windows by itself |
| Licence | MIT or Apache-2.0, which `deny.toml` allows | GPL-3.0, or Slint's royalty-free licence with an attribution notice; `deny.toml` allows neither |

Slint's smaller line count does not mean less code to own. Most of its lines
lay out the window. The list behavior that Win32 gives for free is code that
gozler would write, test and keep correct, and some of it is still missing
from the prototype.

## Toolkit: native Win32, in Rust

The window is a Rust binary that uses the Win32 common controls directly, the
way Everything is built. It is a new crate, `gozler-ui`, in this workspace,
built and installed with `gozlerd` and `gozler`.

It calls Win32 through Microsoft's `windows` crate, not `windows-sys`, which
the rest of the workspace uses. `windows-sys` has bindings for functions only.
The window also needs COM interfaces, such as the folder picker
(`IFileOpenDialog`), and `windows` has them, with `HRESULT` returned as a
`Result`. Both crates come from the same Microsoft repository under MIT or
Apache-2.0, which `deny.toml` already allows.

- **Least memory** (priority 1). The list view in virtual mode
  (`LVS_OWNERDATA`) holds no rows: it asks the window for the text of each
  visible cell
  ([List-View controls](https://learn.microsoft.com/en-us/windows/win32/controls/list-view-controls-overview)).
  The icons come from the system image list, which Windows already holds.
- **Established Windows behavior for free** (priority 2). Selection, keyboard
  navigation, column resizing and reordering, sort arrows,
  accessibility and per-monitor DPI are Windows' own.
- **One language and one build** (priority 3). The window calls the Rust code
  that the CLI uses: no bridge DLL, no second copy of the parser or the
  protocol. `gozler` and the window always speak the daemon's protocol
  version, because DESIGN.md requires one build for all of them.

The cost: Win32 code is verbose, and most of it is `unsafe`.

## Look

The window is made of plain Windows controls, with only the tweaks that cost
one call or a few lines. Every control stays a real Win32 control, so Windows
keeps its keyboard behavior, focus, accessibility and high contrast.

The TinyTorrent2 engine's popup (`engine/src/Desktop/Popup.cpp` and
`Acrylic.cpp`) was inspected for techniques to reuse. Its rendering system, a
window that draws every pixel itself, is not ported.

| Technique | Appearance gained | Code | Verdict |
| --- | --- | --- | --- |
| `SetWindowTheme(list, "Explorer")` on the list view | File Explorer's row hover, selection fill and header | one call | **take** |
| `LVS_EX_DOUBLEBUFFER`, `LVS_EX_FULLROWSELECT`, `LVS_EX_HEADERDRAGDROP` | no flicker while scrolling, whole-row selection, column reorder | one call | **take** |
| Layout in effective pixels, scaled with `GetDpiForWindow` and laid out again on `WM_DPICHANGED`, as the popup's `Scale` does | correct size and spacing on every monitor | one helper | **take** |
| The message font from `SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS)`, and one larger semibold font for the inspector's heading | Windows' own text and a clear heading | two fonts per DPI | **take** |
| `WM_CTLCOLORSTATIC` returning the window colour | labels and check boxes without the grey dialog background | a few lines | **take** |
| One-pixel dividers between the panes instead of 3D edges | flat Windows 11 edges | one paint routine | **take** |
| Custom-drawn Fluent buttons, as the popup draws them | modern toggles and buttons | one paint routine with hover, press, focus and high-contrast states | **skip**: polish only (priority 4) |
| Two surface colours for panes and content | panes read as chrome | colours in every `WM_CTLCOLOR*` and paint path | **skip**: polish only |
| Acrylic or Mica background, a custom title bar | a translucent, WinUI-style window | every control would need custom painting, because each paints an opaque GDI background | **skip** |
| Taller list rows | airier rows | small | **skip**: a search list shows more results at Windows' compact height, as Everything does |

### Dark mode: not in the first version

The prototype measured the cost. About 90 lines made the title bar, list,
header, edit and combo boxes, buttons, check boxes and the Options window dark.
They use the same calls as Notepad++: uxtheme's unnamed exports
`SetPreferredAppMode` (ordinal 135) and `AllowDarkModeForWindow` (ordinal
133), the theme classes `DarkMode_Explorer`, `DarkMode_ItemsView` and
`DarkMode_CFD`, `DWMWA_USE_IMMERSIVE_DARK_MODE`, and `WM_CTLCOLOR*`.

The menu bar stayed white. Windows has no dark theme for it; Notepad++ paints
it through the undocumented `WM_UAHDRAWMENU` and `WM_UAHDRAWMENUITEM`
messages. A white menu bar over a dark window looks broken, so dark mode
cannot ship without that painting, and the whole module would rest on
undocumented calls. Dark mode is optional (priority 5), so the first version is
light only (owner decision). It can come later as one module, `win::dark`, that falls back to
light when an export is missing.

## Decisions for the owner

Made:

1. **The toolkit:** native Win32 in Rust (see "Toolkit evaluation").
2. **Dark mode:** not in the first version (see "Dark mode").
3. **The top of the window:** the classic menu bar, then the search box with
   four push-button toggles (see "Main window"). A command bar, a toolbar of
   icon buttons with an overflow menu, was prototyped and not chosen.
4. **How far the list reaches:** the first 100,000 rows in the chosen order
   (see "Rows on demand"). A result snapshot kept by the daemon would remove
   the limit; it is new daemon work, and the window's cache would not change.

Open:

5. **Where the window's Win32 `unsafe` lives** (below).

**Where the window's Win32 `unsafe` lives.** Most calls in the `windows`
crate are `unsafe`. AGENTS.md keeps Win32 `unsafe` in `gozler-winfs`, so that
it stays in small, reviewed blocks behind safe functions. `gozler-winfs` is the
filesystem layer and depends on no other workspace crate; window code does not
belong there.

**Recommendation:** `gozler-ui` keeps its Win32 calls in one module, `win`
(see "Architecture"), under the same rule: the smallest `unsafe` block, behind
a safe function. The rest of the crate is safe Rust. AGENTS.md then names that
module beside `gozler-winfs`. The other answer is a separate crate,
`gozler-winui`, for the window's Win32 layer; it adds a crate boundary but no
extra safety. The choice changes little: `win` holds every `unsafe` block and
uses only the core's geometry types, so moving it into a crate later moves
files and few lines.

## Architecture

**A core with every decision, a thin Win32 layer, and no GUI framework.** The
core decides what to search, which rows to fetch, which replies are current,
where each control goes and what the status bar says. It is plain Rust with no
Win32 types, so its tests run on every OS. The `win` module wraps the Win32
calls the window uses in safe functions and holds every `unsafe` block. Between
them, the window's message handlers in `app` turn Windows messages into calls
on the core, and the core's answers into calls on `win`. The menus, the
accelerators and the dialogs are resources in a `.rc` file, so Windows runs
them.

```text
gozler-ui.exe
├─ UI thread
│   ├─ app:  message handlers for the main window and the dialogs (safe Rust)
│   ├─ win:  window class, controls, fonts, dialogs, shell calls (all unsafe)
│   └─ core: Model, Query, Results, Layout, Settings (no Win32 types)
└─ one thread per daemon request ── gozler-client ──\\.\pipe\gozler-v1──> gozlerd
```

### Alternatives considered

| Approach | Verdict | Why |
| --- | --- | --- |
| Free functions over one global `RefCell`, as the prototype did | rejected | a borrow held while Win32 calls back into the window procedure panics (rule 1 below); no decision can be tested without a window |
| `winsafe` 0.0.29 | rejected | one maintainer and no stability promise before 1.0; its DPI helpers read the system DPI, not the DPI of the window's monitor; the virtual list still needs raw `WM_NOTIFY` handling; the window would use a small part of a large API |
| `native-windows-gui` | rejected | no release since September 2022 |
| Messages and an update function (Elm style), with a view diffed against the controls | rejected | the list asks for each cell's text inside `LVN_GETDISPINFO` and needs the answer before it returns, so it cannot wait for an update cycle; a diffing layer is a GUI framework |
| A general command list that handlers return and one applier runs | rejected | the same `LVN_GETDISPINFO` constraint; the core's typed return values (`Accepted` and `Fetch`) already tell each handler what to do |
| A widget trait hierarchy with a layout engine | rejected | the window has one fixed layout and three small dialogs; the hierarchy would be more code than the window |
| **Core, thin `win` layer, `.rc` resources** | **chosen** | the least code that keeps the failures below out by construction |

### Rules that keep the first version from needing a rewrite

Each rule names the failure it prevents. These are the decisions that are
expensive to change after the window works, because every handler depends on
them.

1. **Win32 calls back into the window.** Many calls send messages to the same
   window procedure before they return: `SetWindowTextW` on the search box
   sends `EN_CHANGE`, and `SetWindowPos` sends `WM_SIZE`. Calls with their own
   message loop dispatch every message of the thread, replies from the daemon
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
   5 ms); it checks the token on each poll and returns `Cancelled`. The daemon
   notices a cancelled request only at its next page, so cancelling frees the
   window at once and the daemon soon after. For a list request, the limit in
   "Rows on demand" bounds that work; a cancelled export costs the daemon what
   a whole CLI export costs before it stops.
4. **One cancel for different purposes.** A request has a purpose: `Results`
   (pages of the list), `Inspector` (a folder's size or the selection's size),
   `Status`, or `Transfer` (Copy or Export). If selecting another row cancelled
   "the current request", it could cancel a results page. **Rule:** at most one
   request of each purpose is live. A new request cancels the live request of
   its own purpose and no other. Requests of different purposes run at the
   same time on separate connections, which the daemon serves in parallel, so
   a slow folder size never delays the list. A `Transfer` is never replaced:
   Copy and Export are disabled while one runs. Each request carries the
   generation the model gave it, and the model drops a reply whose generation
   is not the newest of its purpose; that is the only staleness check. Closing
   the window cancels every request.
5. **Replies reaching a destroyed window.** A message posted to a destroyed
   window's handle can reach another window that reuses the handle.
   **Rule:** replies travel on one unbounded channel; `PostMessageW` only wakes
   the window and carries no data. The window joins every request thread in
   `WM_DESTROY`, while its handle is still valid, so no wake-up can reach a
   reused handle. The channel is unbounded because a thread blocked on a full
   channel, while the UI thread waits to join it, would never return.
6. **Pixels mixed with device-independent pixels.** Per-monitor DPI touches
   every size: layout, fonts, column widths and icons. Added later, it means
   finding every number. **Rule:** sizes are designed in `Dip` and converted
   to `Px` only in `Layout::compute` and in the font and icon code. They are
   distinct types, so the compiler rejects a mix.
7. **Layout spread over handlers.** **Rule:** one pure function computes every
   control's rectangle from the client size, the DPI, which panes are shown
   and the inspector's height. `WM_SIZE` and `WM_DPICHANGED` only apply its
   result. A new filter or a hidden pane changes that function and its tests.
8. **Keys that two owners want.** An accelerator for `Ctrl+A` or `Ctrl+C`
   takes those keys from the search box, and `IsDialogMessageW` turns `Enter`
   into `IDOK` and `Esc` into `IDCANCEL` before any control sees them.
   **Rule:**
   - The accelerator table holds only keys that no focused control uses:
     `Ctrl+I`, `Ctrl+B`, `Ctrl+U`, `Ctrl+R`, `Ctrl+F`, `Ctrl+E`, `Ctrl+,`,
     `F5`, and `Ctrl+Enter` for Open folder location.
   - The main window handles `IDOK` and `IDCANCEL` by the focused control:
     `Enter` in the list opens the entry, `Enter` in the search box runs the
     search, and `Esc` clears the search box.
   - The message loop takes a few keys from `WM_KEYDOWN` before
     `IsDialogMessageW`, by the focused control, so no `WM_CHAR` follows
     them. In the search box, `Down` moves focus to the list and `Ctrl+A`
     selects the box's text. In the list, `Ctrl+A` selects every row,
     `Ctrl+C` copies and `Ctrl+Shift+C` copies as path.
9. **The selection held in memory.** Fetching every selected row before an
   action holds them all at once. **Rule:** a selection holds at most 100,000
   rows, because the list reaches no more (see "Rows on demand"). The window
   reads the selected ranges from the list. When every selected row is in the
   cache, the window uses the cache. Otherwise it sends one request for the
   rows from the first selected row to the last, because each request costs
   the daemon its offset plus its limit, and the request thread skips the
   rows that are not selected. It uses each page as it arrives and keeps no
   rows:
   - Copy and Copy as path build the clipboard data, at most 100,000 paths or
     about 20 MB of UTF-16 text. The reply carries that data to the UI
     thread, which sets the clipboard, because `SetClipboardData` needs a
     clipboard opened by a window.
   - The selection's size is an `Inspector` request that sums the files, the
     folders and the sizes, and counts the files of unknown size. It starts
     150 ms after the selection last changed.

   The daemon keeps no result set, so a row
   index names the row at that position now: when the index changes between
   the display and the action, the copied rows can differ from the shown rows
   by the entries added or removed. `F5` shows the current state.
10. **A panic with half-changed state.** A panic cannot unwind out of a window
    procedure (`extern "system"`), and the release profile unwinds. **Rule:**
    a panic hook writes the message to `%LOCALAPPDATA%\gozler\ui-error.log`,
    shows a message box with no owner window from a new thread and waits for
    it, then aborts the process. The message box runs on its own thread so
    that the UI thread dispatches no message while its state is half changed,
    and it has no owner because disabling an owner would wait for the UI
    thread, which is waiting for the message box. The rule is the same on
    every request thread, so a dead request never leaves a window that waits
    forever.

Two smaller rules. Every user-visible string is in one `text` module or the
`.rc` file, so a later translation changes two places. `gozler-ui` calls Win32
only through the `windows` crate; values from `gozler-winfs` and the client
cross into it as Rust types, never as raw handles.

### Shared code

| Rule | Owner, used as is |
| --- | --- |
| Query syntax and its errors | `gozler_core::query::parse`, for the typed text and for each filter's text |
| Request building, `-r`, `-parent`, the scope | `gozler_core::escompat::build_search` |
| `dm:` resolution with the local clock and time zone | `gozler_core::query::dates` |
| Whether a volume's results are complete | `VolumePhase::is_complete` |
| Pipe server owner check, version handshake | the pipe client, with `gozler-winfs` |
| Binary results pages | `gozler_core::proto` |
| CSV, TSV and TXT export, and its date format | `gozler_core::output` |

Changes these need:

- The CLI's date formatter for exports, `format_date_local` in `gozler-cli`,
  moves into `gozler_core::output`, so the window's exports and the CLI's
  write the same dates.
- The blocking pipe client in `gozler-cli/src/client.rs` moves into a library
  crate, `gozler-client`, that `gozler-cli` and `gozler-ui` both use. It
  returns its own error type, with one variant for each failure the window
  shows differently: the service is not running, the owner check failed, the
  daemon is older than the client, the daemon refused the request (its
  `Response::Error` text), the connection was lost, and `Cancelled`. Its read
  loop takes a cancel check (rule 3); `gozler` passes one that never cancels.
  `gozler-cli`'s `failure.rs` keeps mapping the errors to exit codes.
- `build_search` takes a working folder. The window has none, so it must
  accept none. A relative path in the query is then the parser's
  `RelativePath` error, which asks for the full path.
- The attribute names that the inspector shows (Hidden, System, …) become
  public in `gozler_core::attributes`.
- `deny.toml` lists the crates that may depend on `gozler-core`; `gozler-ui`
  and `gozler-client` join that list.
- On Linux, CI compiles `gozler-ui` and `gozler-client` as stubs, as
  `gozler-cli` does with its `cfg(not(windows))` `execute`. The core has no
  Win32 calls, so its tests run on Ubuntu.

### Requests

The UI thread never waits for the pipe. Each request runs on its own thread
(rules 3 to 5).

- Search as you type starts a search 150 ms after the last keystroke.
- `F5` runs the current search again. Results do not update by themselves when
  files change, because the protocol has no change notification.
- When the service is not running or the connection was lost, the window asks
  for status every 2 seconds. When the daemon answers, it runs the search
  again, so a service that starts or restarts later needs no action in the
  window. The other failures stay on screen until the user changes the
  search, because the same request would fail the same way: the daemon
  refuses it again, the owner check fails again, or the daemon is still older.

### Rows on demand

1. A search asks for the first 4,096 rows (one `PAGE_ROWS` frame) in the chosen
   sort. At offset 0 with a limit, the daemon selects the top rows instead of
   sorting every match.
2. The reply carries the total. The window sets the list view's item count
   (`LVM_SETITEMCOUNT`) to the total, or to 100,000 when the total is larger.
   The status bar then says "Showing the first 100,000 of 3,204,000 results".
   The order still covers every match: the first 100,000 by name are the first
   100,000 of all matches by name.
3. When the list view needs rows that are not cached (`LVN_ODCACHEHINT`, or a
   jump with the scroll bar or `End`), the window asks for that chunk with
   `offset`. Until it arrives, those rows show empty cells.
4. At most one chunk request is in flight. When it arrives, the window asks
   again for whatever the visible rows still lack, so fast scrolling never
   queues a request for every chunk it passed.
5. Every reply carries the total. When it differs from the count the list
   shows, the window sets the count again.
6. The cache keeps at most 8 chunks (32,768 rows) and drops the chunk used least
   recently. Memory therefore stays the same for any number of matches.

**Why the list stops at 100,000 rows.** A chunk at offset N costs the daemon
N rows of work. Each volume sorts its first `offset + limit` matches and builds
a full path for each of them, under the volume's index lock, and the server
then skips the first `offset` rows (`per_vol_limit` in `gozler-daemon`'s
server, `order_top` in `gozler_core::query::engine`). Pressing `End` in a list
of 3 million entries would make the daemon sort and build about 3 million
paths on each volume to send 4,096 of them, while the volume's journal updates
wait for the lock. With the limit, the worst chunk is about 104,000 rows. A
result snapshot kept by the daemon for each search would remove the limit; it
needs a new request kind and state for each connection, and the window's cache
would not change.

**Export** writes every match, not only the rows the list reaches: one
`Transfer` request with no limit, written page by page through
`gozler_core::output`, as `gozler -export-csv` does, so it costs the daemon
what the CLI's export costs.

A new search, sort or refresh clears the selection.

### Modules

```text
gozler-ui/src
├─ main.rs           cfg(windows): start the window; otherwise a stub
├─ core/             no Win32 types; the tests run on Ubuntu
│  ├─ model.rs       Model: query, results, panes, selection, latest status
│  ├─ query.rs       Query, Filters, MatchOptions -> SearchRequest
│  ├─ results.rs     Results: the chunk cache and the chunk in flight
│  ├─ fetch.rs       Purpose, Generation, Fetch, Fetched, DaemonFailure
│  ├─ layout.rs      Dip, Px, Dpi, Rect, Layout::compute
│  ├─ settings.rs    Settings: load and save ui.json
│  └─ text.rs        every user-visible string that is not in the .rc file
├─ win/              cfg(windows); every unsafe block
│  ├─ window.rs      Hwnd, window class, the window procedure trampoline
│  ├─ dialog.rs      modal dialogs from .rc templates
│  ├─ controls.rs    Edit, Button, ComboBox, ListBox, ListView, StatusBar
│  ├─ notify.rs      WM_NOTIFY and WM_COMMAND decoded into enums
│  ├─ gdi.rs         Font, Fonts, the system image list
│  ├─ shell.rs       open, open folder location, properties, clipboard, folder picker
│  └─ wake.rs        Waker
└─ app/              cfg(windows); safe Rust
   ├─ main_window.rs MainWindow and its message handlers
   ├─ dialogs.rs     Options, Custom date, Custom size
   ├─ icons.rs       the system image index for each extension
   └─ requests.rs    Requests: one thread per Fetch, the cancel tokens, the reply channel
```

`gozler-ui` depends on `gozler-core`, `gozler-client`, `windows` (only the
features it uses) and `serde_json`, and on `embed-resource` as a build
dependency.

### The .rc file

Windows declares menus, accelerators, dialogs, the icon, the version and the
manifest in a resource script, and the window needs an icon and version
information in any case. `build.rs` compiles `gozler-ui.rc` with
`embed-resource`, which runs the Windows SDK's `rc.exe`, and skips it on other
targets. The script holds:

- the menu bar and the accelerator table (rule 8);
- the Options, Custom date and Custom size dialogs, as `DIALOGEX` templates.
  `DialogBoxParamW` then gives the modal loop, the Tab order, `Enter` and
  `Esc`, and per-monitor DPI scaling, which Windows applies to template
  dialogs. The Options categories are child dialogs, one template each;
- the manifest: common controls 6, per-monitor DPI awareness v2 and the
  supported OS list;
- the icon and the version information.

The main window is created in code, because its layout follows the window size
and the panes (rule 7).

### API sketches

These show the shape of the types, not final code.

The core:

```rust
/// Device-independent pixels, as the layout is designed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Dip(pub(crate) i32);

/// Physical pixels on the window's monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Px(pub(crate) i32);

/// The DPI of the monitor that shows the window; 96 is 100 %.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Dpi(NonZeroU32);

impl Dip {
    pub(crate) fn to_px(self, dpi: Dpi) -> Px;
}

impl Layout {
    /// Every control's rectangle above the status bar, which sizes itself.
    pub(crate) fn compute(client: Size, status_bar: Px, dpi: Dpi, panes: &Panes) -> Layout;
}

/// What a request is for; at most one request of each purpose is live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose { Results, Inspector, Status, Transfer }

/// Which request of its purpose a reply answers; a newer request has a
/// larger number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Generation(u64);

/// A request the model wants sent; `app::requests` runs it.
pub(crate) struct Fetch {
    pub(crate) purpose: Purpose,
    pub(crate) generation: Generation,
    pub(crate) ask: Ask,
}

pub(crate) enum Ask {
    /// One chunk of the list, gathered from all its pages.
    Rows { request: SearchRequest },
    Totals { request: SearchRequest },
    /// The rows of `span`, used page by page and not kept (rule 9); only the
    /// rows in `selected` count.
    Stream { request: SearchRequest, span: Range<u64>, selected: Vec<Range<u64>>, to: StreamTarget },
    Status,
}

pub(crate) enum StreamTarget { Sums, Clipboard(ClipboardFormat), File(PathBuf, ExportFormat) }

/// A reply, in the core's terms, so the model's handling of replies and
/// failures is tested without a daemon.
pub(crate) struct Fetched {
    pub(crate) purpose: Purpose,
    pub(crate) generation: Generation,
    pub(crate) outcome: Outcome,
}

pub(crate) enum Outcome {
    Rows { start: u64, rows: Vec<ResultRow>, total: u64 },
    Totals(QueryTotals),
    Status(DaemonStatus),
    Sums(SelectionSums),
    /// Built on the request thread; the UI thread sets the clipboard (rule 9).
    Clipboard(ClipboardData),
    Exported { rows: u64 },
    Failed(DaemonFailure),
}

pub(crate) struct SelectionSums { files: u64, folders: u64, bytes: u64, unknown_size: u64 }

/// The failures the window shows differently; `app` maps the client's error.
pub(crate) enum DaemonFailure { NotRunning, NotGozler, TooOld, Refused(String), Lost(String) }

/// What a reply changed, and the request it makes necessary, if any: the
/// next chunk, or the search again after the daemon answers.
pub(crate) struct Accepted {
    pub(crate) change: Change,
    pub(crate) then: Option<Fetch>,
}

pub(crate) enum Change {
    Stale,
    NewSearch { count: u64 },
    Rows { rows: Range<u64>, count: u64 },
    /// The list is empty and shows a message: a parse error or a failure.
    Message,
    Status,
    Inspector,
    Clipboard(ClipboardData),
    Exported { rows: u64 },
}

impl Results {
    /// The row at `index`, or `None` while its chunk has not arrived.
    pub(crate) fn row(&self, index: u64) -> Option<&ResultRow>;
    /// The chunk the visible rows need next, unless one is already in flight.
    pub(crate) fn next_chunk(&mut self, visible: Range<u64>) -> Option<Range<u64>>;
}

impl Model {
    pub(crate) fn set_text(&mut self, text: String);
    pub(crate) fn start_search(&mut self) -> Fetch;
    pub(crate) fn rows_needed(&mut self, visible: Range<u64>) -> Option<Fetch>;
    /// Applies a reply; a reply of an older generation changes nothing.
    pub(crate) fn accept(&mut self, fetched: Fetched) -> Accepted;
}
```

The `win` layer:

```rust
/// A window handle that does not own its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hwnd(HWND);

/// One window class's message handling. The trampoline holds an `Rc` clone
/// for the length of each call (rule 2), so a handler may destroy its window.
pub(crate) trait WindowProc: 'static {
    fn message(&self, hwnd: Hwnd, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT>;
}

/// Creates a top-level window owned by `state`. Messages that arrive before
/// `WM_NCCREATE`, such as `WM_GETMINMAXINFO`, go to `DefWindowProcW`.
pub(crate) fn create_window<W: WindowProc>(class: &WindowClass, state: Rc<W>, params: CreateParams) -> windows::core::Result<Hwnd>;

/// Runs a modal dialog from a .rc template. `state` is borrowed for the
/// dialog's whole life, because the call returns only after it closes.
pub(crate) fn run_modal<D: DialogProc>(owner: Hwnd, template: DialogId, state: &D) -> isize;

/// An owned GDI font, deleted on drop.
pub(crate) struct Font(HFONT);

/// The fonts the controls use. `replace` gives the controls the new fonts
/// before it deletes the old ones, because `WM_SETFONT` does not take
/// ownership and a control must never hold a deleted font.
pub(crate) struct Fonts { ui: Font, heading: Font }

/// A list notification decoded from `WM_NOTIFY`. The borrow ties it to the
/// message that carried it, so no handler can keep it.
pub(crate) enum ListNotify<'a> {
    GetDispInfo(DispInfo<'a>),
    CacheHint(Range<u64>),
    /// `LVN_ITEMCHANGED`, item -1 (every row) included, and
    /// `LVN_ODSTATECHANGED`, which a virtual list sends for a range such as a
    /// Shift+click.
    SelectionChanged,
    ColumnClick(Column),
}

impl DispInfo<'_> {
    pub(crate) fn index(&self) -> u64;
    pub(crate) fn column(&self) -> Column;
    /// Copies `text` into the list's buffer, cut to the buffer's length.
    pub(crate) fn set_text(&mut self, text: &[u16]);
    pub(crate) fn set_image(&mut self, image: SystemImage);
}

/// Wakes the main window from a request thread; carries no data (rule 5).
pub(crate) struct Waker(isize);
// SAFETY: PostMessageW may be called from any thread. The window joins every
// request thread in WM_DESTROY, while its handle is valid, so the handle
// never names a destroyed or reused window.
unsafe impl Send for Waker {}
```

The `app` layer:

```rust
struct MainWindow {
    controls: OnceCell<Controls>, // filled in WM_CREATE; Windows destroys the controls
    fonts: RefCell<Fonts>,        // dropped after WM_NCDESTROY, when no control is left
    model: RefCell<Model>,        // never borrowed across a Win32 call (rule 1)
    icons: RefCell<Icons>,        // extension -> system image index; filled with no borrow held
    requests: Requests,
}

impl Requests {
    /// Cancels the live request of `fetch.purpose`, then runs `fetch` on a new
    /// thread that connects, sends, maps the client's error to
    /// `DaemonFailure`, sends a `Fetched` on the channel and wakes the window.
    fn start(&self, fetch: Fetch);
    /// Every reply that has arrived, without waiting.
    fn drain(&self) -> impl Iterator<Item = Fetched>;
    /// Cancels every request and joins every thread.
    fn shut_down(&self);
}
```

### Execution paths

**Start.** `main` installs the panic hook and calls `CoInitializeEx` with an
apartment-threaded model, because the folder picker and `SHGetFileInfoW` need
COM. It loads `Settings` and creates the window with an `Rc<MainWindow>`. The
trampoline stores the `Rc` in `GWLP_USERDATA` on `WM_NCCREATE`. `WM_CREATE`
creates the controls with the fonts for the window's DPI and fills
`controls`. Every other handler starts by reading `controls` and does nothing
while it is empty, because `EN_CHANGE` and other notifications can arrive
while `WM_CREATE` runs. The first `WM_SIZE` applies `Layout::compute`. After
`create_window` returns, `main` starts the first search. The message loop
runs `TranslateAcceleratorW`, then the keys it takes for the focused control
(rule 8), then
`IsDialogMessageW` for Tab, `Enter` and `Esc`, then `DispatchMessageW`.

**Typing a search.** `EN_CHANGE` arrives. The handler calls
`model.borrow_mut().set_text(text)`, the borrow ends, and the handler restarts
a 150 ms timer. On `WM_TIMER`, `model.borrow_mut().start_search()` raises the
`Results` generation and returns a `Fetch` for rows 0 to 4,095. The handler
calls `requests.start(fetch)`, which cancels the live `Results` request. That
request's thread sees its token at its next poll, returns `Cancelled` and
ends; the new thread connects and sends the new request.

**A chunk arrives.** The request thread gathers the chunk's pages, sends one
`Fetched` on the channel and calls `Waker::wake`, which posts `WM_APP`. The
handler drains the channel. For each reply, `model.borrow_mut().accept(reply)`
returns `Accepted`, and the borrow ends. The handler starts the `then` fetch,
if there is one, and updates the controls for the `change`. For
`NewSearch { count }`, the handler
calls `ListView::set_count` and updates the status bar. Windows then sends
`LVN_GETDISPINFO` for the visible rows. The handler formats each cell from
`model.borrow()` and copies it into the list's buffer. For the icon, it copies
the extension out, ends the borrow, and looks the image index up in `icons`; a
missing extension calls `SHGetFileInfoW` with no borrow held. A `Stale` reply
changes nothing.

**Scrolling to row 90,000.** The list sends `LVN_ODCACHEHINT` for the visible
range. `model.borrow_mut().rows_needed(range)` returns one `Fetch`, and the
handler starts it. Until the chunk arrives, `LVN_GETDISPINFO` finds no row and
leaves the cells empty. When the chunk is accepted with `Change::Rows`, the
handler sets the count if it changed and redraws those rows; `then` carries
the next chunk that the visible rows still lack.

**Selecting a folder.** `ListNotify::SelectionChanged` arrives. The model returns an
`Inspector` `Fetch` for a totals search scoped to the folder; `requests.start`
cancels the previous folder's request, if one is live. The `Results` request
for the list keeps running on its own connection.

**Copying 60,000 selected rows.** `Ctrl+Shift+C` in the list. The window
reads the selected ranges from the list; the model finds rows outside the
cache and returns a `Transfer` `Fetch` with `Ask::Stream` from the first
selected row to the last. The request thread builds the quoted paths page by
page and sends `Outcome::Clipboard`. The handler opens the clipboard with the
main window and sets the text. Copy and Export are disabled until the reply
arrives.

**Moving to a monitor with another DPI.** `WM_DPICHANGED` carries the new DPI
and the suggested rectangle. The handler builds new `Fonts`, calls
`Fonts::replace`, scales the column widths and moves the window to the
suggested rectangle. The `WM_SIZE` that follows applies `Layout::compute` at
the new DPI. Template dialogs scale themselves.

**The daemon restarts while the user scrolls.** The chunk's thread gets a
connection error and sends `Outcome::Failed(Lost)`. The model empties the list
and shows the message, and the window starts asking for status every 2
seconds. When a `Status` reply arrives, `accept` returns the search again as
`then`. A `Refused` failure starts no polling (see "Requests").

**Exit during a search.** File > Exit calls `DestroyWindow`. In `WM_DESTROY`,
the handler calls `requests.shut_down()`: every request thread sees its token
at its next poll and ends, and the join returns. An export that was stopped
deletes its partial file. The handler saves `Settings` and posts quit. Windows
destroys the controls, then sends `WM_NCDESTROY`, where the trampoline drops
the window's `Rc`. The trampoline call for the Exit command still holds its own
clone, so `MainWindow` and its fonts are freed only when that call returns,
after every control is gone.

**Options.** File > Options copies the settings out of the model and calls
`run_modal` with a dialog state that holds the copy and a `&MainWindow`. While
the dialog runs, Windows disables the main window, and the accelerators do not
run, but replies still reach the main window, which handles them as usual
(rule 1). The Index category runs its own 2-second timer: each tick reads the
model's latest status through the `&MainWindow` and asks the main window for a
new one. Apply and OK call a `MainWindow` method that gives the copy to the
model and saves it.

**A panic.** The hook writes the log, shows the message box from a new thread,
waits for it, and aborts (rule 10). No handler runs with half-changed state.

### How normal changes land

| Change | Where it lands |
| --- | --- |
| A new column | a `Column` entry in the core, and its arm in the `GetDispInfo` handler |
| A new filter | `Filters` and its clause text, one control in `Controls`, its rectangle in `Layout::compute` |
| A new Options category | one child-dialog template and its `DialogProc` |
| Dark mode | a `win::dark` module; every control is created in `win::controls`, so theming has one place to hook in |
| Translations | `text.rs` and the `.rc` file |
| A second top-level window | another `WindowProc` with the same trampoline |
| Scrolling past 100,000 rows | a result snapshot in the daemon and a new `Ask`; the cache and the list do not change |
| Results that update when files change | a new `Outcome`, after the protocol gets change notification |

### Tests

The core carries the tests, because it holds the decisions and runs on every
OS:

- `Query` to `SearchRequest` through the real parser: a typed `ext:rs` with
  Kind set to Documents gives two clauses that both must match; an unclosed
  `"` in the typed text leaves the filter's clause intact;
- `Model::accept`: a reply of an older generation changes nothing, for each
  purpose; a `Lost` failure empties the list and a later `Status` reply
  returns the search again as `then`; a `Refused` failure starts no polling;
- `Results`: at most one chunk is in flight; a changed total changes the
  count; the count stops at 100,000; the cache drops the chunk used least
  recently;
- `Layout::compute` at 96, 144 and 192 DPI: no control overlaps another or
  leaves the client area, and the list keeps a minimum size;
- `Settings`: a damaged value falls back to its default.

The `win` layer and the handlers are checked by hand at each step of the work
order.

## Main window

| Area | Control |
| --- | --- |
| Top | Menu bar, then a strip with the search box and four match-option toggles |
| Left | Filter pane |
| Center | Results list: a list view in report and virtual mode |
| Bottom | Inspector pane, under a splitter that the user drags |
| Bottom edge | Status bar control (`STATUSCLASSNAME`) with three parts |

The window is per-monitor DPI aware (manifest `PerMonitorV2`) and uses the
Windows message font. It opens at its saved placement.

### Search box

- An edit control with a cue banner ("Search"). It fills the strip's width.
- The window opens with it focused. `Ctrl+F` and `Ctrl+E` focus it.
- `Escape` clears it. `Down` moves focus to the first row.
- Four toggle buttons beside it, push-like check boxes
  (`BS_AUTOCHECKBOX | BS_PUSHLIKE`), map to the CLI switches:

  | Toggle | Switch | Shortcut |
  | --- | --- | --- |
  | Match case | `-i` | `Ctrl+I` |
  | Match whole words | `-ww` | `Ctrl+B` |
  | Match path | `-p` | `Ctrl+U` |
  | Regular expression | `-r` (the whole text is the pattern) | `Ctrl+R` |

- A parse error empties the list, and the list's empty area shows the parser's
  message, such as `unknown search function: dc:`.

### Results list

| Column | Default | Header click sorts by | Content |
| --- | --- | --- | --- |
| Name | shown | `name` | the name, with the icon of its extension |
| Path | shown | `path` | the folder of the full path |
| Size | shown | `size` | the size; empty for a folder; "Unknown" for an unknown size |
| Date modified | shown | `dm` | local date and time, in the Windows short date and time format |
| Extension | hidden | `extension` | the extension; empty for a folder |
| Attributes | hidden | (not sortable) | the attribute letters, as `gozler` prints them |

- A header click runs the search again in that order; a second click reverses
  it. The first click on Size or Date modified sorts largest or newest first,
  as gozler's sort keys do. The header shows the arrow (`HDF_SORTUP`,
  `HDF_SORTDOWN`).
- A right-click on the header shows and hides columns. Column order, widths and
  visibility are saved.
- Icons come from `SHGetFileInfoW` with `SHGFI_USEFILEATTRIBUTES` and
  `SHGFI_SYSICONINDEX`, so Windows gives the icon for the extension without
  reading the file. The window asks once for each extension. A per-file icon,
  such as an `.exe`'s own, would read every file from disk.
- Double-click or `Enter` opens the entry. `Ctrl+Enter` opens the folder that
  holds it, with the entry selected.
- `Ctrl+C` copies the selected files to the clipboard as files (`CF_HDROP`), as
  File Explorer does. `Ctrl+Shift+C` copies their full paths as text, each in
  double quotes on its own line, as File Explorer's Copy as path does (rule 9
  in "Architecture").
- The context menu: Open, Open folder location, Copy, Copy as path, Copy name,
  Search in this folder (it sets the Location filter to the entry's folder, or
  to the folder itself), and Properties (the shell's properties dialog).

### Status bar

1. **Results:** "1,204 results", or "Showing the first 100,000 of 3,204,000
   results". While a search runs: "Searching…".
2. **Selection:** "3 selected · 12.4 MB". The size counts files only. When the
   selection reaches outside the cache, the count shows at once and the size
   when its request returns (rule 9).
3. **Index state**, shown only when it affects the answer:
   - A volume in scope is not complete (`VolumePhase::is_complete`): "C:\ is
     indexing — results may be incomplete.", with the phase's own word for an
     offline or failed volume. While a volume in scope is bootstrapping or
     rescanning, the window asks for status every 2 seconds and runs the
     search again once it is complete. An offline or failed volume is not
     polled, because waiting does not complete it.
   - `metadata_pending` and the query tests a size or date: "Some sizes and
     dates on D:\ are unknown."

### Daemon failures

A failure empties the list and its empty area shows the message. The window
then recovers by itself (see "Requests").

| Failure | Message |
| --- | --- |
| No pipe | The gozler service is not running. |
| Owner check fails | The search service is not the gozler service, so gozler did not connect. |
| Daemon older than the window | The installed gozler service is older than this window. Install the matching version. |
| The daemon refuses the request | The daemon's message, such as a scope on no indexed volume or a regular expression that does not compile. |
| Connection lost | The connection to the gozler service was lost. |

### Menu bar

- **File**: Export results… (CSV, TSV or TXT; every match), Options… (`Ctrl+,`),
  Exit.
- **Edit**: Copy, Copy as path, Copy name, Select all.
- **View**: Filters, Inspector, Refresh (`F5`).
- **Search**: the four match options, and Sort by (Name, Path, Size, Date
  modified, Extension; ascending or descending).
- **Help**: Search syntax (the CLI's full reference, `HelpScreen::Full`, in a
  dialog), About.

### Window lifetime

- One window per user session. A second start brings the first window
  forward; a named mutex (`Local\gozler.ui`) decides which process is first.
  The `Local\` namespace is already one per session.
- Closing the window ends the process. There is no tray icon (DESIGN.md: "Not
  planned").

## Filter pane

A fixed-width pane on the left, shown and hidden with View > Filters. A "Clear
filters" button is at the top, then one group per filter. Groups combine with
AND, and with the typed text. A group left at its first choice adds nothing.

Each group except Location produces one clause of query text, such as
`ext:mp3;wav`. The window parses that text on its own, with gozler's parser,
and adds its clauses to those of the typed text, as `build_search` adds the
tests of `-r` and `-parent`. The parser therefore stays the only owner of the
syntax, and nothing the user types changes what a filter means: an unclosed
`"`, a trailing `|` or a typed `ext:` stays inside the typed text. Location
sets the request's scope, as `-path` does, or with the check box its parent
folder, as `-parent` does.

| Group | Control | Choices | Adds |
| --- | --- | --- | --- |
| Kind | list box | All, Folders, Files, Audio, Archives, Documents, Executables, Pictures, Videos | `folder:`, `file:`, or `ext:` with the group's list |
| Location | list box | Everywhere, each indexed volume by mount prefix, Choose folder… | the scope, as `-path` |
| | check box | Only directly in this folder | the parent folder, as `-parent`: a `parent:` test that is also the scope |
| Date modified | combo box | Any time, Today, Yesterday, Last 7 days, Last 30 days, Last 365 days, Custom… | `dm:today`, `dm:yesterday`, `dm:7days`, …, `dm:2026-01-01..2026-03-31` |
| Size | combo box | Any size, Empty, Tiny, Small, Medium, Large, Huge, Gigantic, Custom… | `size:empty`, …, `size:1mb..10mb` |
| Attributes | one combo box each: Any, Only, Exclude | Hidden, System, Read-only, Link (reparse point), Compressed, Encrypted | `attrib:h` or `!attrib:h` |

- **Extension groups** live in one table in `gozler-ui`. Starting lists:
  - Audio: `mp3;wav;flac;aac;ogg;opus;wma;m4a`
  - Archives: `zip;7z;rar;tar;gz;bz2;xz;zst;cab;iso`
  - Documents: `pdf;doc;docx;xls;xlsx;ppt;pptx;odt;ods;odp;rtf;txt;md;csv`
  - Executables: `exe;msi;bat;cmd;ps1;com;scr`
  - Pictures: `jpg;jpeg;png;gif;bmp;webp;tif;tiff;heic;svg;ico`
  - Videos: `mp4;mkv;avi;mov;wmv;webm;m4v;mpg;mpeg`
- **Location** lists the volumes from Status. A volume that is not complete
  shows its phase after its name ("D:\ (indexing)"). Choose folder… opens the
  Windows folder picker (`IFileOpenDialog` with `FOS_PICKFOLDERS`).
- **Custom date** opens a small dialog with two date pickers, From and To, each
  with a check box. From alone adds `dm:>=FROM`, To alone adds `dm:<=TO`.
- **Custom size** opens a small dialog with Minimum and Maximum and a unit
  (KB, MB, GB). Either may be empty.
- Each size word shows its range in the combo box ("Tiny (up to 10 KB)"). The
  ranges are a `match` inside the parser today; `gozler-core` moves them into a
  table that the parser and the window both read, so they cannot drift.
- **No counts beside the choices.** A count is one totals search per choice,
  which multiplies the cost of every keystroke.
- **Other attribute letters** are left to `attrib:` in the search box. Sixteen
  combo boxes would bury the six that people use.

## Inspector pane

A pane under the list, behind a splitter, shown and hidden with View >
Inspector. Its height is saved. Values are read-only edit controls, so the
user can select and copy any part of them, as in File Explorer's Properties.

### One entry selected

The icon, the name, and Open and Open folder location buttons, then:

| Field | Value |
| --- | --- |
| Folder | the folder |
| Full path | the full path |
| Type | File or Folder, and the extension |
| Size | the formatted size and the exact bytes; "Unknown" for an unknown size |
| Date modified | local date and time, to the second |
| Attributes | each attribute as a word: Hidden, System, Read-only, … |
| Volume | its mount prefix and phase, such as "C:\ (live)" |

For a **folder**, Size shows its contents: the number of entries below it and
the summed size of its files. The window gets them with a totals search scoped
to the folder, as `gozler -get-folder-size` does, as an `Inspector` request. A
new selection cancels it. While it runs, the field shows "Calculating…".

### Several entries selected

- How many files and how many folders.
- The summed size of the selected files (rule 9). Files with an unknown size
  are counted apart: "12.4 MB, 2 files unknown". Folder contents are not
  added, because a selected folder and a selected file below it would be
  counted twice.

### Nothing selected

The number of matches.

## Options dialog

File > Options… opens a dialog with a category list on the left and the
category's controls on the right, as Everything's Options dialog does. It has
OK, Cancel and Apply, the Windows convention for an options dialog.

| Category | Setting | Control | Default |
| --- | --- | --- | --- |
| Search | Search as you type | check box | On. Off: `Enter` runs the search. |
| | Show every file and folder when the search is empty | check box | On, as in Everything. Off: an empty search shows nothing. |
| Results | Open folders with | File Explorer, or a command | File Explorer |
| | Command | edit box; `{path}` is the folder | empty |
| Index | (read-only) | see below | — |

"Open folders with" applies to Open on a folder and to Open folder location.
A user who browses with another file manager, such as Total Commander, needs
gozler to open folders there.

### The Index category

It shows the daemon's status, not settings, because gozler's index has no
options: the daemon indexes every fixed NTFS volume by itself, as a service.
The category says this in one line at the top, so a person who looks for
Everything's index options learns why they are absent.

- A list with one row per volume: mount prefixes, phase, entries, and whether
  sizes and dates are unknown (`metadata_pending`).
- The daemon's memory: its working set and private bytes.
- One line that names `gozler --status` for the full report.
- The category refreshes every 2 seconds while it is shown.

### Where settings are saved

`%LOCALAPPDATA%\gozler\ui.json`, written by the window alone: to a temporary
file, then renamed over the old one. A value that is missing or damaged falls
back to its default. It holds two groups:

- `settings`: the options above.
- `window`: placement, column order, widths and visibility, sort, filter pane
  shown and its choices, inspector shown and its height, and the match options.

The keys are the file format, so renaming a key needs a migration.

## Language

The window is in English, as the CLI is. Messages from the parser and the help
come from `gozler-core` unchanged.

## Not planned

| Feature | Why not |
| --- | --- |
| Exclude lists, include-only lists, removable, ReFS and network volumes | gozler indexes every fixed NTFS volume; the index has no options |
| Database location, compression, force rebuild | the index lives in the daemon's memory and rebuilds by itself |
| Date created and date accessed | the index does not hold them (DESIGN.md) |
| Find duplicate files, content search, plugins | gozler searches names, paths, sizes, dates and attributes |
| File lists, ETP/FTP and HTTP servers | not planned in DESIGN.md |
| Tray icon, run in background, global hotkey | the daemon is the background part; DESIGN.md rules out a tray icon |
| Search and run history | not a gozler feature; it waits for a concrete need |
| Editable context menu and keyboard shortcuts, fonts and colors, size and date format options | Windows settings decide the font and the date format |
| Acrylic or Mica background, and a custom title bar | the common controls paint opaque backgrounds; see "Look" |
| Translations | the CLI is English; a catalogue waits for a concrete need |
| Highlighted search terms | not planned in DESIGN.md |
| Live update of results while files change | the protocol has no change notification; `F5` runs the search again |
| The other hard links of a file | results carry no file ID |
| Rename, delete and drag out of the list | file operations are not search; Open folder location gives File Explorer for them |
| Type-ahead in the list | a virtual list must find the row itself (`LVN_ODFINDITEM`), and most rows are not in the window; typing in the list does nothing, and `Ctrl+F` or `Ctrl+E` goes to the search box |
| Scrolling past the first 100,000 rows | each deep chunk costs the daemon work in proportion to its offset; it needs a result snapshot in the daemon (see "Rows on demand") |

## Work order

Each step ends with something a person can run.

1. **Shared client.** Move the pipe client into `gozler-client`, with its own
   error type and the cancel check; let `build_search` take no working folder;
   make the attribute names public; move the export date formatter into
   `gozler_core::output`. `gozler` behaves as before.
2. **Window with search and results.** The crate with its `core`, `win` and
   `app` modules and the `.rc` file; the panic hook and the request threads;
   search box, match options, virtual list with chunk fetching and header
   sort, status bar, daemon failures, menu bar, context menu, export. Before
   the `.rc` file, check that `embed-resource` has an allowed licence and
   builds with the workspace's minimum Rust version, and that `rc.exe` is on
   the owner's machine and the Windows CI runner.
3. **Filter pane.**
4. **Inspector pane.**
5. **Options dialog.** Check that the category child dialogs scale when the
   window moves to a monitor with another DPI. If they do not, the categories
   become groups of controls in the one dialog, shown and hidden.
6. **Installer.** `Setup.iss` installs `gozler-ui.exe` and adds a Start menu
   shortcut named "gozler"; the release workflow packages `gozler-ui.exe` with
   the other two executables. `installer/Build.ps1` still looks for the
   package `goz-daemon`, which is now `gozler-daemon`, and fails before it
   builds anything.

When the window ships, its rules move into DESIGN.md and this plan is deleted.
