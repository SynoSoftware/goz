# Plan: the window's architecture

The shape `gozler-ui` should have for the code it holds now, and the
stages that get it there. First of all, no background operation may ever
stop the window: Gozler exists because Everything's window stops
responding while it works. Beyond that, the design stays: `core` decides,
`win` wraps Win32, `app` connects them, over native controls (DESIGN.md:
"Where the unsafe lives"). What changes: how the window proves that it
never waits, how it avoids flashing, how a change in the model reaches
the controls, where each part of the window keeps its state, and the
rules for writing window code.

## Schedule

**Not scheduled.** The owner decides when.

Stage 2 changes the same repaint code as the flicker work that another
session holds as defects 6 and 8, which landed its first part in
`362f5b2`. Stage 2 goes to that session, or starts after it ends.

## Why

Measured on `main` at `5146bc2`:

- **Nothing enforces that the window never waits.** Today the requests,
  the folder readings, the Viewer's reads, the service check and the
  update check run on threads of their own, and the window's thread joins
  a thread only once it has ended. But that holds because each author
  remembered it. One file read, one lock that a worker holds while it
  works, or one shell call that reaches a network drive, on the window's
  thread, brings back the "Not Responding" window, and nothing would
  catch it before the person does.
- `app/main_window.rs` and `app/main_window/panels.rs` hold 7,900 lines;
  `core/model.rs` holds 3,700 and `core/panel.rs` 4,050. Every feature of
  Search, Two panels, the inspector, the filter pane, the status bar,
  Settings, the tray and the file operations adds to `MainWindow` and
  `Model`.
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
  `IconList::set_icons`, `ListView::set_small_images`, `Button::set_icon`,
  which also clears the shield, and the progress bar. A window that
  refreshes often sets every text on each refresh, as the File operations
  window's `show_view` does on each poll.

## What DESIGN.md says that does not hold

1. **"A view diffed against the controls would keep a second copy of the
   selection and the scroll position."** A second copy is the wrong
   mechanism, but not for that reason: a copy of what was set drifts from
   the control. A check box and a toolbar check button toggle themselves
   on a click, an icon's bitmap depends on the DPI, the system colours
   and Show icons, and a Settings page is made from its template each
   time it first shows. The control itself is the record of what it
   shows. The code relies on that in the setters above; the rule is to
   make every setter do it, not to add a diffed view.
2. **"`WM_SETREDRAW` is turned off only around a change that would draw a
   control several times."** The rule is right, and a resize step is such
   a change: `ColumnSizing::resized` gives every column a new width on
   each step. What does not hold is how the step ends and what it covers:
   `draw_once_after` redraws with `RDW_ERASE`, so the double-buffered list
   is erased before it paints, and the list also moves inside it. A
   window whose drawing is off has no `WS_VISIBLE` bit, so its move
   invalidates nothing in the main window, the main window does not clip
   it, and a synchronous erase of the area around it can paint over it
   (suspected, not filmed).
3. **"The main window … erases only its own background between controls,
   once after each layout."** It is needed today only because the list
   moves while its drawing is off, so its move does not invalidate the
   strips it leaves. Once the list moves as a visible window, Windows
   invalidates what each move uncovers, the class has no `CS_HREDRAW` or
   `CS_VREDRAW`, and the whole-window erase on each step can go.

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

Each decision below was reached by checking the code, after two rounds of
review. When this plan is merged with another version, keep each one, or
replace it only with something that prevents the same failure.

| Decision | Why | What breaks without it |
| --- | --- | --- |
| Nothing stops the window is the first rule and stage 1 | Gozler exists because Everything's window stops responding while it works (owner ruling "the window never lags") | The one failure that makes Gozler pointless can come back through any single call |
| `OffThread` token on every function that can wait | Today the window waits on nothing only because each author remembered; the compiler can check it | A file read or shell call on the window's thread compiles and ships |
| A test that scans for waiting `std` calls | A token cannot guard `std::fs`, `sleep`, `join` or `recv` | The same, through `std` |
| The watchdog and `ui-slow.log`, with modal loops not counted | A wait the rules miss shows up the first time it happens, with its cause, even if it never ends | Waits are found only when a person meets them and reports them |
| Setters read the control; no saved copy of what was set, no `Screen` type | Controls change themselves: check boxes and toolbar check buttons toggle on a click, icons depend on DPI and colours, Settings pages are made anew | A saved copy drifts, and the window shows a value the model does not hold, or never sets one after a DPI or theme change |
| A refresh never sets the selection, focus, scroll position, typed text, column widths or order | The control or the person owns them | A refresh undoes a person's column drag or moves their selection |
| The list's item count is an effect, not a refreshed value | Setting it also redraws every row | A new search with the same total keeps showing the old rows |
| Check boxes, toolbar check buttons and the Match: list are set from the model | A toggle the model refuses must go back | The control shows checked while the model says unchecked |
| Menus stay out of refreshes | Their states are set when a menu opens (window rule 9) | Work on every update for menus nobody sees |
| Three steps: structure effects, refresh, other effects | A column must exist before its header is set; Windows drops the focus of a control it disables, so focus moves come after the sets | Lost header texts, or a focus that lands on a disabled control and vanishes |
| An effect that changes the model returns an `Accepted`, handled at once | Starting a panel's reading or reading memory changes the model after the refresh | The panel or the memory figures show the state from before the effect |
| A refresh repeats when the model changed during its sets; no guard | A message sent back by a set runs its own update | The outer refresh writes values from before the nested update over newer ones |
| A command's own dialog or menu runs at the end of step 3, not as a `ModalStep` | Modal steps run one at a time, and Settings starts fixes whose questions are steps | While Settings is open, a fix's question and a copy's conflict question never show |
| A dialog that a reply, disk event or operation starts is a `ModalStep` | Window rule 13 | A dialog's loop applies newer events before older ones |
| Effects stop once the window is destroyed | Window rule 5: request threads are joined at `WM_DESTROY` | A request starts after the joins and its reply reaches a reused handle |
| A notification that only records control-owned state is not an update | Shift+End over 100,000 rows sends a storm of `LVN_ITEMCHANGED` | One full update per notification stalls the window |
| The panels and operations keep their ordering rules | COM delivers messages while an operation's sources are read, so they come last | Reentrancy into a half-prepared operation |
| Refresh values come from the model plus measures that `app` takes | Layout needs measured caption widths, numbers need the regional format, paths are shortened by Windows | `core` cannot compute them; tests would need a window |
| The inspector's record of what it shows stays, in `core` | It decides when to restart the 60-second age timer and refit texts | Every status poll restarts the timer, so "modified … ago" freezes |
| Areas move before what an update returns changes | AGENTS.md: restructuring is its own stage, before features | Every area is edited twice |
| `Model` forwards to areas during the move | Lets the tests pass unchanged | The move cannot be proven by the existing tests |
| No `Request<T>` | The status and a panel's sizes keep their last answer while they ask again; `Model::accept` already drops stale replies | The status blanks on every poll |
| `Instance` moves into `core`; no `Role` enum | The queue and drop target are `app` objects made after the window; the drop target must be revoked at `WM_DESTROY`, which a `OnceCell` cannot do | Win32 types in `core`, or a registration never revoked and an `Rc` cycle |
| Settings' page in `core`, its handle in `app` | Two facts with two owners: `polls_status` reads the page | A copy, or Win32 types in `core` |
| `SearchArea` holds Search's `search_due` and `enter_on_answer` | A search panel's list has neither | Two always-false fields on every panel |
| The list moves as a visible window; only column widths stay inside `WM_SETREDRAW` | A window with drawing off has no `WS_VISIBLE`, so its move invalidates nothing | Stale strips once the whole-window erase is removed, or the erase stays forever |
| The redraw drops `RDW_ERASE`, keeps `RDW_FRAME` and `RDW_ALLCHILDREN` | The list is double-buffered; its border and scroll bars and its header need repainting at new widths | A flash on each resize step, or stale scroll bars or header |
| The whole-window erase goes only after the list moves visibly | It repaints the strips that a hidden move leaves | Stale strips |
| `TBSTYLE_EX_DOUBLEBUFFER` on every toolbar | It is off today, and toolbars blink on each redraw | Toolbar flicker on resize |
| No `WS_EX_COMPOSITED` | It buffers every child again and clashes with the list views' own buffering | Slower paints and list view glitches |
| One screen recording before and after, saved, no waiting | Flicker can only be seen; AGENTS.md forbids stages that wait for the owner and probes that rerun | No proof, or a stalled stage |
| Measured runs report numbers, with no threshold | AGENTS.md: checks must not depend on the machine's speed | A check that fails on a busy machine |
| No crate split for `core` | CI already builds and tests `core` on Ubuntu; the split loses dead-code warnings and the catalogue-key test's reach, for a saving nobody measured | Cost without a measured benefit |
| New rules are numbered 14 and up | Code and docs cite window rules 1 to 13 by number | Every citation points at the wrong rule |
| No thread per top-level window; the model stays on the window's thread | `LVN_GETDISPINFO` needs each cell's text before it returns; the File operations window is owned by the main window | A lock and a thread hop in every painted cell |
| Rule 21 covers `win`, split by control | `win/controls.rs` is already over 2,500 lines | The rule is broken the day it lands |

## The shape

### Nothing stops the window

The window's thread runs the message loop and nothing that can wait:

- **No call that waits on a disk, the network, the service, another
  process or another thread:** no file or registry read or write, no pipe,
  no service manager query, no shell call that can reach a drive, such as
  an icon or a display name for a path, no `WaitFor…`, no `recv` that
  blocks, no `sleep`, and no `join` of a thread that has not ended. That
  work runs on a thread of its own and reports through the replies
  channel and a wake (window rule 5).
- **No lock that another thread holds while it works.** A thread that
  shares a value with the window holds its lock only to copy the value in
  or out, never across a read, a write or a call; a channel is preferred.
- **The compiler enforces it.** Every function in `app` and `win` that can
  wait takes an `OffThread` token, a type that only `app::worker::spawn`
  creates and passes to the closure it runs on a new thread, so a call on
  the window's thread does not compile. A `core` test scans `app` and
  `win` for the `std` calls a token cannot guard (`std::fs`, `sleep`,
  `join`, a blocking `recv`) outside the worker modules, as the
  catalogue-key test scans the sources, and fails on one.
- **The watchdog catches the rest.** Before each message, the message loop
  records the message, its window and the time in an atomic, and clears it
  after; the message filter hook does the same for each message that a
  modal loop dispatches, so time a person spends in a dialog does not
  count. A watchdog thread checks every 100 ms, and a message that has run
  for over 100 ms writes a line to `ui-slow.log` in the settings folder:
  the message, its window or control, how long it has run, and again when
  it ends. So a wait shows up with its cause the first time it happens in
  daily use, also one that never ends, and each line becomes an issue
  under the owner ruling "the window never lags".

The window's own work, such as making the controls at start-up (#98) or
working out an operation's common folder, is not background work; it is
tracked in `window-responsiveness.md`, and the watchdog finds it too.

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
| Icon or image list | the handle, exact because `glyph_icon` and the image lists cache by glyph, size and colour |
| Enabled | `IsWindowEnabled` |
| Visible | the `WS_VISIBLE` bit, not `IsWindowVisible`, which walks the parents |
| Rectangle | `is_placed_at`, which compares only position and width for a combo box |
| Progress bar position and range | `PBM_GETPOS`, `PBM_GETRANGE` |
| Combo box selection set from the model | `CB_GETCURSEL` |

Two values cannot be read back: the inspector's file icon, because
`show_file_icon` sets a new copy of the icon each time, and whether a
progress bar's marquee runs, because `PBM_SETMARQUEE` has no getter and
setting it again restarts the animation. The wrappers are `Copy` handles,
so each window's `Controls` struct keeps the last such value beside the
control, made and dropped with it.

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
3. **The other effects, in order:** requests, timers, focus moves, which
   come after the sets because Windows drops the focus of a control it
   disables, cancels, and posts to Settings.

The rules around the steps:

- **An effect that changes the model** returns an `Accepted`, which is
  handled at once with its own three steps.
- **A message that a set or an effect sends back** runs its handler and
  its own steps; there is no guard. Each update counts the model's
  changes, and a refresh that sees the count change while it sets
  refreshes its areas once more, so it never leaves a value computed
  before the nested update.
- **A dialog or menu that a command opens** runs at the end of step 3,
  outside every borrow, as Settings and the context menu run now, so a
  dialog that a reply or an operation starts can still run inside it.
- **A dialog that a reply, a panel's disk event or a file operation
  starts** is queued as a `ModalStep` and runs from `Message::Deferred`
  (window rule 13), as now.
- **Effects stop once the window is destroyed:** a request started after
  `WM_DESTROY` has joined the request threads would break window rule 5.

### What the loop covers

- **The main window.** Its model goes through this loop. A notification
  that only records state a control owns, such as each `LVN_ITEMCHANGED`,
  which records the told rows and posts one `Message::Deferred`, is not an
  update; the loop runs at most once for each message it handles.
- **Settings, the File operations window and each Viewer** keep their own
  state and their own small loop of the same shape: Settings edits its
  draft (window rule 1), the File operations window words what the queue
  reports, a Viewer owns its file. They gain the setters that read the
  control, which ends the File operations window's sets on each poll.
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
by comparing the area's values with the last ones it computed, not by
reading a control. The inspector is the case: it restarts its 60-second
age timer and fits its texts again only when what it shows changed, so
its record of what it shows (`inspector_shown` today) stays, as a `core`
fact.

### Areas

The main window is made of areas: Search's list with its search box, the
filter pane, the inspector, the status bar, each panel, the key bar, the
file operations, the bridge to Settings, the tray and the instance. Each
area has a `core` type with its state, its update methods and the values
it shows, and an `app` module with its controls' messages and its
refresh. `Model` holds the areas and the facts several of them read: the
strings, the settings, the service's status and the instance.
`MainWindow` holds the window: its handle, the mode, the layout and one
table that routes each control ID and timer to its area.

### Painting

The window never flashes and never shows stale pixels, with Windows' own
means only:

- **Double buffering where the control offers it:** list views
  (`LVS_EX_DOUBLEBUFFER`) and the tree (`TVS_EX_DOUBLEBUFFER`), as now,
  and every toolbar (`TBSTYLE_EX_DOUBLEBUFFER`), which has it off today.
- **A control is set only when its value differs,** by the setters above,
  because a label, a button, a toolbar and a status bar part draw again on
  every set, also of an unchanged value.
- **A control moves as a visible window,** only when its rectangle
  changes, all of a layout's moves in one `DeferWindowPos` batch, every
  control with `WS_CLIPSIBLINGS` and every parent with `WS_CLIPCHILDREN`,
  so Windows invalidates what each move uncovers.
- **`WM_SETREDRAW` only around a change that would draw one control
  several times without moving it,** such as setting every column's width
  on a resize step, or a language switch, and the redraw after it has no
  `RDW_ERASE` for a double-buffered control.
- **No erase of the whole window after a layout.**
- **No `WS_EX_COMPOSITED`:** it buffers every child again, clashes with
  the list views' own buffering and slows every paint.

## Rules for writing window code

These join DESIGN.md's window rules as 14 and up, because code and docs
cite rules 1 to 13 by number:

14. **Nothing stops the window.** The window's thread makes no call that
    can wait on a disk, the network, the service, another process or
    another thread, and takes no lock another thread holds while it
    works; a function that can wait takes an `OffThread` token.
15. **A handler turns a message into an update and handles its
    `Accepted`.** It sets no control outside the three steps.
16. **Model the state by its rules.** A boolean for an independent fact;
    an enum that carries its data for states that exclude each other; an
    enum with one transition function for a state machine whose moves are
    constrained; a trait only where areas behave differently behind one
    call. Two fields that must agree, a combination that cannot happen,
    or the same condition tested in several places mean the model is
    wrong: change the representation instead of adding a branch.
17. **A fact has one owner,** the area that changes it; another area reads
    it through the model, never through a copy.
18. **Every `win` setter of a shown value reads the control first** and
    sets only a value that differs; a value the control cannot return is
    kept beside it in its window's `Controls`. `app` keeps no copy of a
    shown value only to skip a set.
19. **A timer is a variant of `Timer`,** never a bare number.
20. **A test asserts what the person would see,** the values an area
    shows for one representative state, given fixed measures, or the
    effects of an update in their order, against literals the scenario
    predicts. No test needs a window.
21. **A file over 2,500 lines is split:** `core` and `app` along their
    areas, `win` by control, test modules included in the count.

## Stages

One implementer keeps its context through the stages and fixes the
review's findings; one reviewer reads each stage and reports. Only stage
1 may change what a person sees, by moving a wait off the window's
thread; the others change no behavior other than removing flashes and
unchanged sets. Each is its own commits and lands on `main` with
`check.ps1` green. A defect found on the way is filed, or fixed in its own
commit when it is small.

### Stage 1: nothing stops the window

1. The watchdog and `ui-slow.log`, with the message filter hook's
   accounting for modal loops.
2. An inventory of every call on the window's thread that can wait:
   file and registry calls, service manager queries, shell calls on paths,
   clipboard opens, locks shared with a thread (the update check's, the
   service check's, the operations queue's), and joins. Each is moved to
   a thread, shown to end at once (such as a join after the thread's
   answer arrived), or filed as an issue that names the call and what it
   reads, with the inventory in the stage's report.
3. `OffThread` and `app::worker::spawn`, and every function that can wait
   takes the token.
4. The `core` test that scans `app` and `win` for unguarded waiting calls.

**Proof:**
- The scanning test, which fails on a waiting `std` call outside the
  worker modules. Its first run on the code before item 2 lists what item
  2 must handle.
- A `core` test of the watchdog's rule, given fixed times: a message over
  the limit is logged once while it runs and once when it ends, and time
  in a modal loop does not count.
- One day of the owner's normal use with the new build, after install;
  each `ui-slow.log` line becomes an issue. It waits for no stage.

### Stage 2: painting

1. `TBSTYLE_EX_DOUBLEBUFFER` on every toolbar (`win::toolbar`).
2. A resize step moves the list outside `draw_once_after`, with the other
   controls in the layout's batch, and keeps only the column widths
   inside it. Its redraw has no `RDW_ERASE`; it keeps `RDW_FRAME`, for
   the border and scroll bars at the new widths, and `RDW_ALLCHILDREN`,
   for the header, because whether the header follows the list's
   `WM_SETREDRAW` is not known.
3. The whole-window `InvalidateRect` after a layout goes, because every
   control now moves as a visible window. The implementer names any area
   the main window paints itself that no move invalidates, or says there
   is none.
4. One screen recording before the stage and one after, of the same
   steps: a border drag in both modes, a splitter drag, typing a search,
   a mode switch and a status update. Each runs once as part of the
   stage, and is saved for the owner to review later; nothing waits for
   the review. A frame that shows a cleared, half-drawn or doubled control
   after the stage is a defect of the stage.
5. Only if the recording after the stage still shows them:
   - the horizontal scroll bar flashing on a narrowing drag: the columns
     are fitted before the move when the list narrows;
   - a half-drawn page on a Viewer scroll: its area paints through
     `BeginBufferedPaint`. Today it erases nothing and paints each row
     opaque, so no fault is known.

### Stage 3: state types

1. `Instance` moves into `core`, `Model` gets it at creation and answers
   `offers_file_operations` from it, and `Model::file_operations` goes.
   `app` keeps the queue in its `OnceCell` and the drop target in its
   `RefCell`, which `WM_DESTROY` empties to revoke the registration while
   the window's handle is valid.
2. `Ending { Running, ForSetup, ForReset }` replaces `closing_for_setup`
   and `resetting`: a setup starts from a fix and Reset all from
   Settings, so a window never ends for both.
3. `core` keeps the shown Settings page as `Option<SettingsPage>` in
   place of `index_page_shown`; `app` keeps the dialog's handle. These are
   two facts with two owners.
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

### Stage 4: areas

A move, with no change of behavior or of what an update returns:

1. Each area gets its `core` type and its `app` module, in the order of
   how separate the areas already are: the inspector, the filter pane,
   the status bar, Search's list, the key bar, the bridge to Settings,
   the tray and the instance, then the panels, whose code already lives
   in `core::panel` and `app/main_window/panels.rs`.
2. `Model` keeps thin methods that forward to the areas while callers
   move, and drops each once no caller uses it.
3. `MainWindow` routes each control ID and timer through one table.
4. `core/captions.rs` splits along the areas whose texts it holds, and
   `win/controls.rs` by control.

**Proof:** the existing tests pass with only their paths and names
changed. Done when no area's field is in `Model` or `MainWindow` and no
file breaks rule 21.

### Stage 5: what an update returns

1. `Accepted` becomes `{ touched: Areas, effects: Vec<Effect> }`; `Then`
   becomes `Effect`, and `Change` goes. The stage starts by listing, from
   `MainWindow::apply` and `search`, what each arm does besides setting
   shown values; each such action becomes an effect. Known today:
   `ColumnShown` and `ColumnHidden` insert or delete a column;
   `NewSearch` selects, sets the count, shows and focuses a row, releases
   the operations, fetches the rows in view and may queue the step that
   opens the first result; `Message` clears the list's selection and
   count and cancels its `Selection` request; `Rows` and `SelectionRead`
   set the count or redraw rows and fetch the rows in view; `Status`
   reads the window's memory; `Fix`, `TotalCommander` and
   `IncludedCount` post to Settings; `Settings` replaces the menu bar and
   updates the tray icon; `ExportFailed` queues a `ModalStep`;
   `Searching` starts the searching timer; `Mode` starts or stops the
   panels; `Panes` moves the focus.
2. `app` handles an `Accepted` in the three steps, with the rules around
   them, and stops running effects once the window is destroyed.
3. `core` emits the modal steps it decides; `app` keeps its queue for the
   steps that its own threads, drops and checks raise.
4. The other windows' loops take the same three steps.

**Proof:** the 28 tests in `core/model.rs` that assert `change` or `then`
are rewritten to assert the touched areas and the effects in their order,
against what each scenario predicts.

### Stage 6: setters that read the control

1. The setters that set every time today read the control first, as the
   table says; the two values the table cannot read are kept in each
   window's `Controls`.
2. Area by area, the refresh calls the setters with the values `core`
   computes, `app`'s copies of shown values go (`status_right` and
   `status_memory` where the status bar's values can be computed again),
   and the File operations window stops setting every text on each poll.
   The inspector's record of what it shows moves into `core`.
3. One measured run on the window's thread of a status reply, a key in
   the search box, a resize step, a splitter drag and Shift+End over
   100,000 rows, reported with the stage. It sets no threshold, because a
   threshold depends on the machine.

**Proof:** the recording steps of stage 2 once more, compared with stage
2's, and a `core` test that the inspector restarts its age timer only
when what it shows changed.

### Stage 7: the docs

DESIGN.md "The window" takes rules 14 to 21, the watchdog, the three
steps and the areas; its Painting item takes the painting rules; the
paragraph on the rejected diffed view says that the controls hold what
they show and the setters read it. This plan is deleted.

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
  (#98) belongs to `window-responsiveness.md`.
- No feature and no change a person can see, other than waits moved off
  the window's thread and the flashes and unchanged sets that stages 2 and
  6 remove.
