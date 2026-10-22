# Plan: the window's architecture

The shape `gozler-ui` should have for the code it holds now, and the
stages that get it there. First of all, no background operation may ever
stop the window: Gozler exists because Everything's window stops
responding while it works, and in Total Commander, changing to a
`\\server\share` that does not answer freezes the whole window until it
answers. Beyond that, the design stays: `core` decides, `win` wraps Win32,
`app` connects them, over native controls (DESIGN.md: "Where the unsafe
lives"). What changes: one owner for all background work, how the window
proves that it never waits, how it avoids flashing, how a change in the
model reaches the controls, where each part of the window keeps its
state, and the rules for writing window code.

The bar is decency, not smoothness: the window never freezes, never makes
a person wait for what should be instant, and never visibly flashes or
shows garbage. 60 or 120 frames a second is no goal, and work whose only
gain is smoothness a regular user would not notice is cut or made
conditional on evidence.

## Schedule

**Not scheduled.** The owner decides when.

Stage 4 changes the same repaint code as the flicker work that another
session holds as defects 6 and 8, which landed its first part in
`362f5b2`. Stage 4 starts after that work ends, so the plan's one
implementer does it.

## Why

Measured on `main` at `5146bc2`:

- **The window still waits on disks and other programs.** Five waits on
  the window's thread are filed: Settings reads every drive's label, every
  2 seconds on the Drives and service page (#140); a search with a
  Location checks the folder on disk 150 ms after each key (#141); Open,
  Explore, Properties, Open file location and the open commands call the
  shell (#142); `ui.json` is saved (#143); opening the clipboard sleeps
  while another program holds it (#144). More are listed in stage 1. Each
  can freeze the window on a sleeping disk or a share that stopped
  answering.
- **Nothing enforces that the window never waits,** and background work
  has no owner. About 19 places in 12 files start a thread, each with its
  own rules for cancelling, waking the window and ending at exit: the
  requests, the panels' folder threads, the Viewer's reader, the Include
  save, Paste, the entry checks, the update check, the service check,
  Indexed folders, the clipboard, drops and the keyboard hook. What a slow
  task shows, whether a person can cancel it, and what happens at exit
  differ from window to window, and a new window would repeat them again.
- `app/main_window.rs` and `app/main_window/panels.rs` hold 7,900 lines;
  `core/model.rs` holds 3,700 and `core/panel.rs` 4,050. Every feature
  adds to `MainWindow` and `Model`.
- A model method answers `Accepted { change, then }`. `Change` has 23
  arms that are not exclusive: `Status` refreshes the status bar, the
  locations, the message and both panels, most arms end in
  `update_status`, and most arms also act, such as `Message`, which clears
  the list's selection and count. By the rule for modeling state, arms
  that overlap and repeat a condition are the wrong model.
- `then: Option<Then>` allows one follow-up, so compound follow-ups hide
  inside one variant (`ReadSelectionSoon` cancels a request and starts a
  timer) or inside `Change` arms.
- Many setters in `win` set a control only when its value differs, by
  reading the control: `Hwnd::set_enabled`, `Hwnd::set_visible`,
  `CheckBox::set_checked`, `Toolbar::set_checked` and `set_enabled`,
  `StatusBar::set_text`, `set_parts` and `set_icon`,
  `IconList::set_items`, `move_children`. Others set every time:
  `Hwnd::set_text` (labels, the title), `Toolbar::set_text`, which also
  sizes the buttons again, `Toolbar::show_images`, `StatusBar::set_tip`,
  `IconList::set_icons`, `ListView::set_small_images`,
  `ListView::set_header`, `Edit::set_cue`, `DatePicker::set_day`,
  `Button::set_icon`, which also sends `BCM_SETSHIELD`, and the progress
  bar. So the File operations window sets every text of its card on each
  500 ms poll during a copy, and each set draws the control again.

## What DESIGN.md says that does not hold

1. **"A view diffed against the controls would keep a second copy of the
   selection and the scroll position."** A second copy is the wrong
   mechanism, but not for that reason: a copy of what was set drifts from
   the control. A check box and a toolbar check button toggle themselves
   on a click, an icon's bitmap depends on the DPI, the system colours
   and Show icons, and a Settings page is made from its template each
   time it first shows. The control itself is the record of what it
   shows. The code relies on that in many setters; the rule is to make
   every setter do it, not to add a diffed view.
2. **"`WM_SETREDRAW` is turned off only around a change that would draw a
   control several times"**, and **"the main window erases only its own
   background between controls, once after each layout."** Both may cost a
   visible flash on a resize step: the list moves while its drawing is
   off, so it has no `WS_VISIBLE` bit and the main window does not clip
   it; `draw_once_after` erases the double-buffered list before it paints;
   and the whole window is erased after each layout. This is suspected,
   not filmed. The list moves inside `draw_once_after` on purpose, so a
   narrower window does not flash its horizontal scroll bar, and a resize
   step measures about 12 ms of layout and 22 ms of list paint
   (`docs/research/measurements/window-resize`). So stage 4's first
   recording decides: these change only if it shows a flash a regular
   user would see.

These hold and stay: native controls, no framework, no
`WS_EX_COMPOSITED`, no free functions over a global `RefCell`, captions as
functions of facts (window rule 12), dialogs queued as modal steps
(window rule 13), one thread and connection per request (window rule 3),
one crate. `core` builds and runs its tests on Ubuntu on each push to
`main` and each pull request (`ci.yml`), so the rule that it holds no
Win32 type is already enforced; a crate of its own would save compile time
only by an amount nobody has measured, and costs the dead-code warnings a
library's `pub` items lose, the catalogue-key test's reach into `app` and
`win`, and new entries in `deny.toml` and CI.

## Decisions to keep, and why

Each decision below was reached by checking the code, after three rounds
of review and the owner's answers. When this plan is merged with another
version, keep each one, or replace it only with something that prevents
the same failure.

| Decision | Why | What breaks without it |
| --- | --- | --- |
| Nothing stops the window is the first rule and the first stages | Gozler exists because Everything's window stops responding; in Total Commander a `\\server\share` that does not answer freezes the window (owner ruling "the window never lags") | The one failure that makes Gozler pointless comes back through any single call |
| The bar is decency, not smoothness (owner) | A regular office user, not a gamer | Effort spent on smoothness nobody notices, while freezes wait |
| One owner for all background work, used by every window (owner) | About 19 places start threads, each with its own rules for slowness, cancel and exit | Each new window repeats the rules, differently, and one of them freezes |
| `core::work` holds the rules, `win::work` starts every thread, `app::work` connects areas | The rules are tested on Ubuntu; `win`'s own threads use the owner without `win` calling `app` | Untested rules, or layering reversed |
| Cancel by doing something else in the same place (owner) | A new search or another folder replaces the work; the person never waits for work they left | Stale work runs on, or a click elsewhere cancels work the person wants |
| Work that changes files is cancelled only by its Cancel (owner) | A half-done copy lost to a typed search is worse than any wait | Lost or half-done copies, moves, deletes and exports |
| Cancel shows only when a task the person waits for is slow, where its result will appear (owner) | A fast task needs no button, and a slow one needs a way out | Flashing buttons, or no way out of a dead share |
| No automatic time limit for work the person started; background work gives up silently (owner) | A slow share may still answer and the person decides; nobody waits for background work | Results thrown away, or messages about work nobody asked for |
| Known waits move first, onto the owner (stage 1) | They are what a person feels; the tooling protects later work but gives nothing alone | Months of tooling while the freezes stay |
| `OffThread` token, `!Send`, on every function that can wait | The compiler rejects a marked waiting call on the window's thread | A marked call compiles there and ships |
| A test scans `app`, `win` and `core` for waiting `std` calls, a named list of waiting Win32 calls and of other crates' waiting functions; thread code only in `…_thread` modules or under `work` | The token guards only marked functions; the Location check lives in `gozler-client`, the settings save in `core` | Unmarked waits, such as #141 and #143, ship unseen |
| No join of a thread that may still run; at exit the window hides, tasks are cancelled and released through the `OpenWaker` flag | A join waits for whatever the thread waits for, such as a permission prompt or a dead share | Exit hangs |
| The watchdog is a heartbeat that every message loop dispatches | The filter hook fires only before a message and misses the size/move loop and drag and drop; a heartbeat stops only when the thread is blocked | Every open dialog or border drag logged as a hang, and real hangs mislabelled |
| The watchdog logs CPU cycles beside wall time, repeats while a stop lasts, records only message and control IDs, writes from its own thread, caps the log | Tells waiting from working; a long hang reads as long; window titles hold typed searches and file names; the log must not itself wait or grow | Noise, short readings of long hangs, private text in a log, a log that is a wait |
| The owner's day of use and the field checks go to `ui-after-install.md` | AGENTS.md: what only the owner can check waits for no stage | A stage that stalls on the owner |
| Painting starts with a recording, and its riskier changes happen only if the recording shows a flash | The list moves inside `draw_once_after` to stop a scroll bar flash; the suspected flashes are not filmed; a resize step is about 34 ms | A flash brought back to fix one nobody saw |
| `RDW_ERASE` stays in the main window's `draw_once_after` | A mode or language switch must erase the other mode's pixels | Garbage after a mode or language switch |
| Toolbar double buffering is checked in light, high contrast and 150 % DPI, and dropped if no blink is filmed | A double-buffered transparent toolbar can paint a wrong background | Black or wrong toolbar backgrounds |
| Unchanged sets are removed early, the File operations window first | Its card is set every 500 ms during a copy, the one blink on every update | The blink stays until the maintenance stages |
| Setters read the control; no saved copy of what was set, no `Screen` type | Controls change themselves, icons depend on DPI and colours, pages are made anew | The window shows a value the model does not hold, or never sets one after a DPI or theme change |
| The inspector's file icon, the shield and the marquee are kept beside their control in `Controls` | They cannot be read back: a new icon copy each time, no getter for `BCM_SETSHIELD` or `PBM_SETMARQUEE`; the wrappers are `Copy` handles | Sets on every refresh, or a restarted marquee |
| A refresh never sets the selection, focus, scroll position, typed text, column widths or order; the item count is an effect | The control or the person owns them; setting the count also redraws every row | A refresh undoes a column drag, or a new search with the same total shows old rows |
| Check boxes, toolbar check buttons and the Match: list are set from the model | A toggle the model refuses must go back | The control shows checked while the model says unchecked |
| Menus stay out of refreshes | Their states are set when a menu opens (window rule 9) | Work on every update for menus nobody sees |
| Three steps: structure effects, refresh, other effects | A column must exist before its header is set; Windows drops the focus of a control it disables | Lost header texts, or a lost focus |
| An effect that changes the model returns an `Accepted`, handled at once; a refresh repeats while the model's change count moves, a few times at most | Starting a reading or reading memory changes the model after the refresh; a message sent back by a set runs its own update | Stale panels and memory figures, or old values written over newer ones |
| A command's own dialog or menu runs at the end of step 3, not as a `ModalStep` | Modal steps run one at a time, and Settings starts fixes whose questions are steps | While Settings is open, a fix's question and a copy's conflict question never show |
| A dialog that a reply, disk event or operation starts is a `ModalStep` | Window rule 13 | A dialog's loop applies newer events before older ones |
| Effects stop once the window is destroyed | Window rule 5 | A reply reaches a reused handle |
| A notification that only records control-owned state is not an update | Shift+End over 100,000 rows sends a storm of `LVN_ITEMCHANGED` | One full update per notification stalls the window |
| The panels and operations keep their ordering rules | COM delivers messages while an operation's sources are read | Reentrancy into a half-prepared operation |
| Refresh values come from the model plus measures that `app` takes | Layout needs measured widths, numbers the regional format, paths `PathCompactPathW` | `core` cannot compute them; tests would need a window |
| The inspector keeps the facts it last showed, in `core`, not its captions | It decides when to restart the 60-second age timer; captions are display text (window rule 12) | Every poll restarts the timer, so "modified … ago" freezes |
| Areas make their own controls | So #98 can make them when an area first shows | The start-up wait gets harder to remove |
| The maintenance stages come after the user-visible ones, labelled as such, with a measured run against `8db8f37`'s figures | They give a person nothing and carry the regression risk | A regression in focus or selection speed ships unnoticed |
| Areas move before what an update returns changes; `Model` forwards meanwhile | AGENTS.md: restructuring is its own stage; the tests prove the move | Every area edited twice, and no proof |
| No `Request<T>` | The status and a panel's sizes keep their last answer while they ask again | The status blanks on every poll |
| `Instance` moves into `core`; no `Role` enum; the queue stays in its `OnceCell`, the drop target in its `RefCell` | The queue and the drop target are `app` objects made after the window; the drop target must be revoked at `WM_DESTROY` | Win32 types in `core`, or a registration never revoked and an `Rc` cycle |
| Settings' page in `core`, its handle in `app`; `SearchArea` for Search's flags | Two facts with two owners; a search panel's list has neither flag | Copies, or always-false fields |
| No `WS_EX_COMPOSITED` | It buffers every child again and clashes with the list views' buffering | Slower paints and list view glitches |
| Measured runs report numbers, with no threshold | AGENTS.md: checks must not depend on the machine's speed | A check that fails on a busy machine |
| No crate split for `core` | CI already tests `core` on Ubuntu; the split has costs and an unmeasured saving | Cost without a measured benefit |
| New rules are numbered 14 and up | Code and docs cite window rules 1 to 13 by number | Every citation points at the wrong rule |
| No thread per top-level window; the model stays on the window's thread | `LVN_GETDISPINFO` needs each cell's text before it returns | A lock and a thread hop in every painted cell |

## The shape

### Background work: one owner

All work that can wait runs through one owner, which every window uses:
Search, the file manager's panels, copying, the Viewer, Settings, and any
later window. A window says only "start this work, of this kind, shown in
this area".

- **`core::work` holds the rules,** with no Win32 type, tested on Ubuntu:
  the kind of each task, when a task counts as slow (one constant, a few
  seconds without an answer or progress), what then happens, and the one
  captions function that words "Waiting for …".
- **`win::work` is the only code that starts a thread** for a task. It
  gives the task an `OffThread` token, which is `!Send`, so it cannot
  reach the window's thread, a cancel token, and a registration that lets
  a cancel cut loose a stuck disk call (`CancelSynchronousIo`). It
  delivers the task's answers through the replies channel and an
  `OpenWaker`, which wakes the window only while it is open, so no task is
  joined. It lives in `win`, so `win`'s own threads (folder readings, the
  Viewer's reads, drops, the clipboard) use it without `win` calling
  `app`.
- **`app::work` connects the areas:** it starts a task for an area,
  cancels the area's task that a new one replaces, and routes Cancel and
  Esc to the task shown where the person is.

The kinds of task:

| Kind | Examples | When it is slow |
| --- | --- | --- |
| Replaced by the next action in the same place | a search as you type, a folder reading, a selection read, a Viewer's reads | A new task of the same purpose in the same area cancels it, silently |
| Waited for by the person | a search, a folder on a share, opening a file, an export, a copy | After the slow constant with no answer, its area's message place, where the result will appear, says "Waiting for \\server\share…" with Cancel. No automatic time limit |
| Background, nobody watching | the status, folder sizes, drive labels and free space, the update check | It gives up silently after its limit and runs again at the next natural moment |
| Changing files | a copy, move, delete or export | Never cancelled by another action; only its Cancel ends it. The engine (`gozler-fileops`) does the work; the owner shows its slowness in the card and the status bar when its progress stops |

Scrolling, opening Settings, switching to the other panel or opening the
Viewer cancel nothing. A call that Windows does not let go of is cut
loose and its thread released, never waited for. There is never a modal
"please wait".

### Nothing stops the window

The window's thread runs the message loop and nothing that can wait:

- **No call that waits on a disk, the network, the service, another
  process or another thread:** no file or registry read or write, no pipe,
  no service manager query, no shell call on a path, no `WaitFor…`, no
  `recv` that blocks, no `sleep`, and no `join` of a thread that may still
  run. That work is a task of the owner.
- **No lock that another thread holds while it works.** A thread that
  shares a value with the window holds its lock only to copy the value in
  or out, never across a read, a write or a call; a channel is preferred.
- **At exit** the window hides, every task is cancelled, and the threads
  are released through the `OpenWaker` flag, not joined. The waits after
  the window is hidden are the final writes of `ui.json`, which keeps the
  window's place and columns, and of `operations.json`, which lets the
  operations continue at the next start.

What enforces it, stated as what each part catches:

- **The token:** every function that can wait takes an `OffThread`, so a
  call to a marked function on the window's thread does not compile.
- **The scan:** a `core` test reads the sources of `app`, `win` and `core`
  (as the catalogue-key test reads them), skipping `#[cfg(test)]` code,
  and fails on a waiting call outside the thread modules. It matches the
  forms that wait, not bare names, because `Path::join`, a row's `is_dir`
  field and `std::process::id()` wait for nothing: `fs::` paths, `File::`
  calls, the `Path` methods `exists`, `is_dir`, `is_file`, `metadata`,
  `symlink_metadata`, `canonicalize` and `read_dir` called on a path, a
  `join()` with no arguments, `thread::sleep`, a `recv()` with no
  timeout, and `Command` or `Child::wait`; a named list of waiting Win32
  calls (`ShellExecuteW`, `SHOpenFolderAndSelectItems`,
  `GetVolumeInformationW`, the registry calls, `OpenClipboard`); and a
  named list of other crates' waiting functions
  (`gozler_client::folder_is_missing`, `gozler_fileops::Queue::new` and
  `close`). It also fails on `thread::spawn` or `thread::Builder` outside
  `win::work` (rule 15). The code a task runs lives only in modules named
  `…_thread` or under `work`, which hold nothing else, so a file that
  mixes the two splits. The few exceptions are listed in the test with
  their reasons: the panic hook's join, which keeps the window's thread
  from dispatching while its state is half changed (window rule 11), the
  keyboard hook's thread, which needs its own message loop, start-up
  before the window exists, and the final writes after the window
  hides. `core` holds no file call: `core` builds and parses the
  settings text, `app` reads the file at start-up and a task writes it.
- **The watchdog** catches the rest. A watchdog thread posts a heartbeat
  message to the window every 100 ms while a Gozler window is on screen.
  Every message loop on the thread dispatches it, modal ones included
  (dialogs, menus, the size and move loop, drag and drop), and a blocked
  thread does not. When a heartbeat goes unanswered past the limit, one
  constant in `core::work`, 200 ms to start, the watchdog writes a line,
  and again each second while the stop lasts: how long, the window
  thread's CPU cycles over the same time (`QueryThreadCycleTime`), so
  waiting is told from working, and the message in progress, from a
  stack that the window procedure wrappers in `win::window` and
  `win::dialog` push and pop around each message of a Gozler window or
  dialog, whichever loop dispatched it, so a nested loop never overwrites the outer record. A message to a
  system dialog leaves the record of the message that opened it on top,
  which is the one to name.
  It records message IDs and control IDs or classes, never window text,
  which holds typed searches and file names. Only the watchdog thread
  writes `ui-slow.log`, which keeps its newest 64 KB, in the settings
  folder or `--data-dir`. A line names the message, not the slow call;
  the issue names the call after reading the code.

The window's own work, such as making every control at start-up (#98), is
not background work; it is tracked in `window-responsiveness.md`, and the
watchdog finds it too.

### Setters that read the control

Every `win` setter of a shown value compares the new value with the
control's current value and sets only when they differ. Reading a control
is not a decision about the model, so `win` still decides nothing. A
value that the control changes in its own way when it is set, such as a
shortened path, is compared in that form.

| Value | How the setter reads it |
| --- | --- |
| Text of a label, an edit or the title | `WM_GETTEXT` |
| Check box | `BM_GETCHECK` |
| Toolbar button checked, enabled, text | `TB_ISBUTTONCHECKED`, `TB_ISBUTTONENABLED`, `TB_GETBUTTONINFOW` |
| Status bar part text, tip, edges, icon | `SB_GETTEXTW`, `SB_GETTIPTEXTW`, `SB_GETPARTS`, `SB_GETICON` |
| Tooltip text | `TTM_GETTEXTW` |
| List header text | `LVM_GETCOLUMNW` |
| Edit cue banner | `EM_GETCUEBANNER` |
| Date picker day | `DTM_GETSYSTEMTIME` |
| Icon or image list | the handle, exact because `glyph_icon` and the image lists cache by glyph, size and colour |
| Enabled | `IsWindowEnabled` |
| Visible | the `WS_VISIBLE` bit, not `IsWindowVisible`, which walks the parents |
| Rectangle | `is_placed_at`, which compares only position and width for a combo box |
| Progress bar position and range | `PBM_GETPOS`, `PBM_GETRANGE` |
| Combo box selection set from the model | `CB_GETCURSEL` |

Three values cannot be read back: the inspector's file icon, because
`show_file_icon` sets a new copy of the icon each time; the administrator
shield, because `BCM_SETSHIELD` has no getter and `Button::set_icon` sends
it on every call; and whether a progress bar's marquee runs, because
`PBM_SETMARQUEE` has no getter and setting it again restarts the
animation. The wrappers are `Copy` handles, so each window's `Controls`
struct keeps the last such value beside the control, made and dropped
with it.

A window inside its own `draw_once_after` has no `WS_VISIBLE` bit, so no
setter reads that window's visibility there.

### What a refresh never sets

Some values belong to the control or the person, and a refresh never sets
them: the selection, the focus, the scroll position, the text a person
types, and the column widths and order, which a person drags. They change
only through an effect: selecting rows, moving the focus, setting the
search box's text for Clear, filling the columns. The list's item count
is an effect too, because setting it also redraws every row, which a new
search with the same total needs.

A value that the person changes and the model then decides, such as a
check box, a toolbar check button or the Match: list, is set from the
model by a refresh: a toggle the model refuses goes back.

Menus stay out of refreshes: their item states are set when a menu opens
(`WM_INITMENUPOPUP`, window rule 9), and `replace_menu_bar` compares the
bar's titles.

### What an update returns

```rust
pub(crate) struct Accepted {
    /// The areas whose shown values may have changed: independent
    /// facts, so a set, not an enum.
    touched: Areas,
    /// What to do, in this order.
    effects: Vec<Effect>,
}
```

`app` handles an `Accepted` in three steps, after the model's borrow has
ended, with each area's values computed into owned data before its
setters run:

1. **Structure effects:** the effects that change what the controls are
   or hold, such as inserting a column with its header, setting a list's
   item count, or starting or stopping a panel's reading, because a value
   can be set only on a control part that exists, and some values, such as
   the rows in view, need the new count.
2. **Refresh** each touched area: compute its values in `core` from the
   model and the measured inputs, and call the setters.
3. **The other effects, in order:** tasks for the owner, timers, focus
   moves, which come after the sets because Windows drops the focus of a
   control it disables, cancels, and posts to Settings.

The rules around the steps:

- **An effect that changes the model** returns an `Accepted`, which is
  handled at once with its own three steps.
- **A message that a set or an effect sends back** runs its handler and
  its own steps; there is no guard. Each update counts the model's
  changes, and a refresh that sees the count move while it sets refreshes
  its areas again, a few times at most (`debug_assert` after that), so it
  never leaves a value computed before the nested update.
- **A dialog or menu that a command opens** runs at the end of step 3,
  outside every borrow, as Settings and the context menu run now, so a
  dialog that a reply or an operation starts can still run inside it.
- **A dialog that a reply, a panel's disk event or a file operation
  starts** is queued as a `ModalStep` and runs from `Message::Deferred`
  (window rule 13), as now.
- **Effects stop once the window is destroyed** (window rule 5).

### What the loop covers

- **The main window.** Its model goes through this loop. A notification
  that only records state a control owns, such as each `LVN_ITEMCHANGED`,
  which records the told rows and posts one `Message::Deferred`, is not an
  update; the loop runs at most once for each message it handles.
- **Settings, the File operations window and each Viewer** keep their own
  state and their own small loop of the same shape: Settings edits its
  draft (window rule 1), the File operations window words what the queue
  reports, a Viewer owns its file. They use the same owner for background
  work and the same setters.
- **The panels and the operations** keep their ordering rules: a
  listing's changes apply in the order `ReadChange` gives, and an
  operation's sources come last in `OperationSteps`, because COM delivers
  messages while the window waits.

### Values computed from the model and from measures

A refresh is not a function of the model alone. The layout takes the
measured width of each caption (`win::gdi::TextMeter`), the inspector fits
its texts, the numbers follow `WindowsNumbers`, and paths are shortened by
`PathCompactPathW`. `app` measures, `core` computes from the model and
those measures, as `core::captions` and `core::layout` do now, and the
Ubuntu tests give fixed measures.

Where a change of a shown value also starts an effect, `core` decides it
from facts, not by reading a control. The inspector is the case: it keeps
the facts it last showed, not its captions (window rule 12), and restarts
its 60-second age timer and fits its texts again only when they change.

### Areas

The main window is made of areas: Search's list with its search box, the
filter pane, the inspector, the status bar, each panel, the key bar, the
file operations, the bridge to Settings, the tray and the instance. Each
area has a `core` type with its state, its update methods and the values
it shows, and an `app` module with its controls' messages, its refresh,
and the making of its own controls, so #98 can make an area's controls
when it first shows. `Model` holds the areas and the facts several of
them read: the strings, the settings, the service's status and the
instance. `MainWindow` holds the window: its handle, the mode, the layout
and one table that routes each control ID and timer to its area.

### Painting

The window never visibly flashes and never shows stale pixels, with
Windows' own means only:

- **Double buffering where the control offers it:** list views
  (`LVS_EX_DOUBLEBUFFER`) and the tree (`TVS_EX_DOUBLEBUFFER`), as now,
  and the toolbars (`TBSTYLE_EX_DOUBLEBUFFER`) if stage 4's recording
  shows them blink and its screenshots show no wrong background.
- **A control is set only when its value differs,** by the setters above,
  because a label, a button, a toolbar and a status bar part draw again on
  every set, also of an unchanged value.
- **A control is moved only when its rectangle changes,** all of a
  layout's moves in one `DeferWindowPos` batch, every control with
  `WS_CLIPSIBLINGS` and every parent with `WS_CLIPCHILDREN`, as now.
- **`WM_SETREDRAW` only around a change that would draw one control
  several times,** such as the list's resize step or a language switch.
  The main window's redraw keeps `RDW_ERASE`, so a mode or language
  switch leaves none of the other mode's pixels.
- **No `WS_EX_COMPOSITED`:** it buffers every child again, clashes with
  the list views' own buffering and slows every paint.

## Rules for writing window code

These join DESIGN.md's window rules as 14 and up, because code and docs
cite rules 1 to 13 by number:

14. **Nothing stops the window.** The window's thread makes no call that
    can wait on a disk, the network, the service, another process or
    another thread, takes no lock another thread holds while it works,
    and joins no thread that may still run.
15. **All background work goes through the one owner.** Only `win::work`
    starts a thread for a task; a function that can wait takes an
    `OffThread` token; a window says what to run, of which kind, shown in
    which area, and the owner decides cancelling, slowness and exit.
16. **A handler turns a message into an update and handles its
    `Accepted`.** It sets no control outside the three steps.
17. **Model the state by its rules.** A boolean for an independent fact;
    an enum that carries its data for states that exclude each other; an
    enum with one transition function for a state machine whose moves are
    constrained; a trait only where areas behave differently behind one
    call. Two fields that must agree, a combination that cannot happen,
    or the same condition tested in several places mean the model is
    wrong: change the representation instead of adding a branch.
18. **A fact has one owner,** the area that changes it; another area reads
    it through the model, never through a copy.
19. **Every `win` setter of a shown value reads the control first** and
    sets only a value that differs; a value the control cannot return is
    kept beside it in its window's `Controls`. `app` keeps no copy of a
    shown value only to skip a set.
20. **A timer is a variant of `Timer`,** never a bare number.
21. **A test asserts what the person would see,** the values an area
    shows for one representative state, given fixed measures, or the
    effects of an update in their order, against literals the scenario
    predicts. No test needs a window.
22. **A file over 2,500 lines is split:** `core` and `app` along their
    areas, `win` by control, test modules included in the count.

## Stages

One implementer keeps its context through the stages and fixes the
review's findings; one reviewer reads each stage and reports. Each stage
is its own commits and lands on `main` with `check.ps1` green. A defect
found on the way is filed, or fixed in its own commit when it is small.
Stages 1 to 4 change what a person feels; stages 5 to 9 are maintenance,
change nothing a person should notice, and come after them. Their risk is
a regression in focus, selection or speed, which their proofs watch.

### Stage 1: the owner for background work, and the known waits

1. `core::work`: the kinds, the slow constant, the rules for replacing,
   cancelling and giving up, and the wording of "Waiting for …".
2. `win::work`: the one spawn, the `!Send` `OffThread` token, the cancel
   token, the registration for `CancelSynchronousIo`, and delivery
   through the replies channel and an `OpenWaker`. `app::work`: starting
   a task for an area and routing Cancel and Esc.
3. The known waits move onto the owner:
   - the Location check (#141): checked as a task when the Location
     changes and on F5; each search uses the latest answer, so typing
     never touches the disk;
   - the shell calls (#142): Open, Explore, Properties, Open file
     location and the open commands run as tasks on a thread with its
     own COM single-threaded apartment, with `SEE_MASK_NOASYNC`; a
     failure comes back as a `ModalStep`. A prompt the shell makes owned
     by a window of another thread attaches the two threads' input, so a
     hung shell call could still freeze the main window. One run on a
     share that stopped answering decides the owner: the main window if
     it keeps taking input, otherwise a window made on the shell's
     thread;
   - the `ui.json` save (#143): `core` builds and parses the settings
     text and holds no file call; the file is written as a task, one at
     a time, the newest text winning. At exit and at `WM_ENDSESSION` the
     last write is waited for after the window hides, as
     `operations.json` is, so the window's place and columns are kept,
     and a failure still shows its message;
   - the drive labels (#140), read as a background task, the last labels
     kept for each refresh;
   - the clipboard open (#144): retried on a short timer, not a sleep;
   - `is_paired`, which hashes gozlerd.exe; the registry read and write
     for start at sign-in; `Queue::new`'s read of `operations.json`; the
     drop's `GetData` from another process; and `EmptyClipboard`'s
     message to another process: each moved onto the owner, or shown to
     return at once, with the evidence in the stage's report;
   - the locks the window takes that a thread also holds: the
     operations queue's, which the File operations window's poll takes,
     and its write lock, which `Queue::close` waits on while a queue
     thread writes `operations.json`; and the service check's. Each is
     shown to be held only to copy a value, or replaced by a message,
     with the evidence in the stage's report; `Queue::close` runs only
     after the window hides.

**Proof:**
- `core` tests of the rules, with fixed times: a new task in the same
  area and purpose cancels the old one and one elsewhere does not; work
  that changes files is never cancelled by another action; a task the
  person waits for shows Cancel only after the slow constant; background
  work gives up silently; nothing is delivered after the window closes.
- The After install items in `ui-after-install.md`: a Location on a
  sleeping disk; Enter, F3 and Properties on a share that stopped
  answering; a pulled USB drive; a clipboard another program holds;
  Settings > Drives and service and Settings > Include with a drive
  asleep or offline.

### Stage 2: every thread onto the owner, exit without waiting, the scan

A move before the stage that changes what a person sees (AGENTS.md:
restructuring is its own stage). The one change of behavior is at exit,
because the owner joins no task:

1. The threads of today move onto the owner, each with the rules it has
   now: the requests, the panels' folder threads, the Viewer's reader,
   the Include save, Paste, the entry checks with `present()`, the update
   check, the service check, the fix thread, Indexed folders, the
   clipboard and drops.
2. Joins go with them: at exit the window hides, cancels every task and
   releases the threads through the `OpenWaker` flag; only the final
   writes of `ui.json` and `operations.json` are waited for, after the
   window is hidden.
3. The scan test, with its listed exceptions and the `…_thread` and
   `work` modules; files that mix window-thread code and thread code
   split.

**Proof:** the existing tests pass unchanged, and the scan test passes
with only its listed exceptions; its first run, before item 1 ends,
lists what is left. In `ui-after-install.md`: closing the window during
a search, a folder reading on a share that stopped answering and a copy,
and the window's place kept at the next start.

### Stage 3: slow tasks say so, and the watchdog

1. Each area shows its slow task in its message place, with Cancel where
   the rules say, and replaces or gives up its tasks by the kinds.
2. The watchdog and `ui-slow.log`.

**Proof:**
- `core` tests of the area rules, with fixed times, where stage 1's
  tests do not already cover them.
- A `core` test of the watchdog's rule, with fixed times: an unanswered
  heartbeat past the limit writes a line, a stop that lasts writes one
  each second, cycles near the wall time read as working and far below
  it as waiting, and popping a nested record restores the outer one.
- In `ui-after-install.md`: a day of the owner's normal use with the
  build; the implementer then files one issue for each distinct message
  and cause in `ui-slow.log`, adding repeats to the existing issue. No
  stage waits for it.

### Stage 4: painting and unchanged sets

1. One screen recording before any change: a border drag in both modes,
   a splitter drag, typing a search, a mode switch, a status update and a
   copy with the File operations window open. It runs once as part of the
   stage and is saved for the owner to review later.
2. The setters that set every time compare first: `Hwnd::set_text`,
   button captions, `Toolbar::set_text`, the status bar's and the
   tooltips' tips, `ListView::set_header`, `Edit::set_cue`,
   `DatePicker::set_day`, and `Button::set_icon` with the shield kept in
   `Controls`. The File operations window stops setting its card's texts
   on each poll.
3. Only if the first recording, at normal speed, shows a flash a regular
   user would see:
   - toolbar blink: `TBSTYLE_EX_DOUBLEBUFFER`, with screenshots in light,
     high contrast and at 150 % DPI;
   - the list flashing on a resize step: the list moves with the other
     controls, only its column widths stay inside its `draw_once_after`,
     whose redraw for the list drops `RDW_ERASE` but keeps `RDW_FRAME` and
     `RDW_ALLCHILDREN`; then the whole-window `InvalidateRect` after a
     layout goes, with any area the window paints itself named; and if
     the horizontal scroll bar then flashes on a narrowing drag, the
     columns are fitted before the move when the list narrows;
   - a half-drawn page on a Viewer scroll: its area paints through
     `BeginBufferedPaint`.

**Proof:** the same recording after the stage. A frame that shows a
cleared, half-drawn or doubled control is a defect of the stage.

### Stage 5: state types (maintenance)

1. `Instance` moves into `core`, `Model` gets it at creation and answers
   `offers_file_operations` from it, and `Model::file_operations` goes.
   `app` keeps the queue in its `OnceCell` and the drop target in its
   `RefCell`, which `WM_DESTROY` empties to revoke the registration while
   the window's handle is valid.
2. `Ending { Running, ForSetup, ForReset }` replaces `closing_for_setup`
   and `resetting`: a setup starts from a fix and Reset all from
   Settings, so a window never ends for both.
3. `core` keeps the shown Settings page as `Option<SettingsPage>` in
   place of `index_page_shown`; `app` keeps the dialog's handle.
4. Search's `search_due` and `enter_on_answer` move with Search's list
   into `SearchArea { list, due, enter_on_answer }`, because a search
   panel's list has neither.
5. `app` gets `IncludeSave { Idle, Saving(Saving), Saved(Result<(),
   NotSaved>) }` for `include_saving` and `include_saved`, and `RangeList
   { Idle, Closing(RangeFilter, ListClosing), Closed(RangeFilter) }` for
   `range_closing` and `range_closed`.
6. `enum Timer` with one function to and from Windows' timer ID.
7. `column_dragged` posts `Message::Deferred` once, as
   `selection_pending` and each panel's `ListChanged` already do.

The requests in flight stay as they are: the status and a panel's sizes
keep their last answer while they ask again, the Include count forgets it,
and `Model::accept` already drops a reply of another generation in one
place.

**Proof:** the existing tests pass, with only names changed. A new test
only where a rule moved into a new type and nothing watches it.

### Stage 6: areas (maintenance)

A move, with no change of behavior or of what an update returns:

1. Each area gets its `core` type and its `app` module, which makes the
   area's controls, in the order of how separate the areas already are:
   the inspector, the filter pane, the status bar, Search's list, the key
   bar, the bridge to Settings, the tray and the instance, then the
   panels, whose code already lives in `core::panel` and
   `app/main_window/panels.rs`.
2. `Model` keeps thin methods that forward to the areas while callers
   move, and drops each once no caller uses it.
3. `MainWindow` routes each control ID and timer through one table.
4. `core/captions.rs` splits along the areas whose texts it holds, and
   `win/controls.rs` by control.

**Proof:** the existing tests pass with only their paths and names
changed. Done when no area's field is in `Model` or `MainWindow` and no
file breaks rule 22.

### Stage 7: what an update returns (maintenance)

1. `Accepted` becomes `{ touched: Areas, effects: Vec<Effect> }`; `Then`
   becomes `Effect`, and `Change` goes. The stage starts by listing, from
   `MainWindow::apply` and `search`, what each arm does besides setting
   shown values; each such action becomes an effect. Known today:
   `ColumnShown` and `ColumnHidden` insert or delete a column and update
   `column_sizing`; `NewSearch` selects, sets the count, shows and focuses
   a row, cancels its `Selection` request, releases the operations,
   fetches the rows in view, fills around the first rows and may queue the
   step that opens the first result; `Message` clears the list's selection
   and count and cancels its `Selection` request; `Rows` and
   `SelectionRead` set the count or redraw rows and fetch the rows in
   view, and `Rows` for Search's list also fills around the first rows; `Status` reads the window's memory; `Fix`, `TotalCommander` and
   `IncludedCount` post to Settings; `Settings` replaces the menu bar and
   updates the tray icon; `ExportFailed` queues a `ModalStep`;
   `Searching` starts the searching timer; `Mode` starts or stops the
   panels, replaces the menu bar and moves the focus; `Panes` moves the
   focus; and the tail of `apply` sets the age, status and save timers and
   calls `set_viewer`, which changes the model.
2. `app` handles an `Accepted` in the three steps, with the rules around
   them, and stops running effects once the window is destroyed.
3. `core` emits the modal steps it decides; `app` keeps its queue for the
   steps that its own threads, drops and checks raise.
4. The other windows' loops take the same three steps.

**Proof:**
- The 28 tests in `core/model.rs` that assert `change` or `then` are
  rewritten to assert the touched areas and the effects in their order,
  against what each scenario predicts.
- One measured run on the window's thread of a status reply, a key in
  the search box, a resize step, a splitter drag, Ctrl+click and
  Shift+End over 100,000 rows, and the end of a move, reported beside
  today's figures from `8db8f37` (Ctrl+click 15.6 ms, Shift+End 5.3 ms,
  end of a move 39 ms, a status reply under 15 ms). It sets no threshold,
  because a threshold depends on the machine.
- In `ui-after-install.md`: the focus after Enter, Esc, Tab and a mode
  switch; header texts after a language switch; Settings' Apply.

### Stage 8: the remaining setters and the areas' refresh (maintenance)

1. The setters left setting every time, such as the progress bar's, read
   the control first, as the table says.
2. Area by area, the refresh calls the setters with the values `core`
   computes, and `app`'s copies of shown values go (`status_right` and
   `status_memory` where the status bar's values can be computed again).
   The inspector keeps the facts it last showed in `core`, and its file
   icon beside the control.

**Proof:** the recording steps of stage 4 once more, and a `core` test
that the inspector restarts its age timer only when the facts it shows
change.

### Stage 9: the docs

DESIGN.md "The window" takes rules 14 to 22, the owner for background
work, the watchdog, the three steps and the areas; window rule 5 is
rewritten for threads released through the `OpenWaker` flag instead of
joined; its Painting item takes the painting rules; the paragraph on the
rejected diffed view says that the controls hold what they show and the
setters read it. This plan is deleted.

## Not in this plan

- No framework, no data binding and no MVVM: Win32 has no binding, and
  writing one means notifications that call back into the window, the
  panic window rule 1 exists to prevent.
- No new drawn surface: owner draw still needs the owner's permission.
- No change to the virtual lists, to how rows are fetched, or to the
  selection contract; the model stays on the window's thread, because
  `LVN_GETDISPINFO` needs each cell's text before it returns.
- No thread for each top-level window: the File operations window is
  owned by the main window, and the waits are kept off the thread
  instead.
- No crate of its own for `core`, unless a measured rebuild of an edit to
  `app` shows a large saving.
- The start-up cost of making every control before the window shows
  (#98) belongs to `window-responsiveness.md`; stage 6 makes it easier.
- The 2-second status poll (#139) is a protocol change, outside this
  plan.
- No feature and no change a person can see, other than waits moved off
  the window's thread, slow tasks that say so with Cancel, and the
  flashes and unchanged sets that stages 4 and 8 remove.
