# Plan: the window's architecture

The shape `gozler-ui` should have for the code it holds now, and the
stages that get it there without changing what the window does. The
design stays: `core` decides, `win` wraps Win32, `app` connects them,
over native controls (DESIGN.md: "Where the unsafe lives"). What changes:
how the window avoids flashing, how a change in the model reaches the
controls, where each part of the window keeps its state, and the rules
for writing window code.

## Schedule

**Not scheduled.** The owner decides when.

Stage 1 changes the same repaint code as the flicker work that another
session holds as defects 6 and 8, which landed its first part in
`362f5b2`. Stage 1 goes to that session, or starts after it ends.

## Why

Measured on `main` at `5146bc2`:

- `app/main_window.rs` and `app/main_window/panels.rs` hold 7,900 lines;
  `core/model.rs` holds 3,700 and `core/panel.rs` 4,050. Every feature of
  Search, Two panels, the inspector, the filter pane, the status bar,
  Settings, the tray and the file operations adds to `MainWindow` and
  `Model`.
- A model method answers `Accepted { change, then }`. `Change` has 23
  arms that are not exclusive: `Status` refreshes the status bar, the
  locations, the message and both panels, most arms end in
  `update_status`, and five arms also act, such as `Message`, which
  clears the list's selection and count. By the rule for modeling state,
  arms that overlap and repeat a condition are the wrong model.
- `then: Option<Then>` allows one follow-up, so compound follow-ups hide
  inside one variant (`ReadSelectionSoon` cancels a request and starts a
  timer) or inside `Change` arms.
- Most setters in `win` already set a control only when its value
  differs, by reading the control: `Hwnd::set_enabled`,
  `Hwnd::set_visible`, `CheckBox::set_checked`, `Toolbar::set_checked`
  and `set_enabled`, `StatusBar::set_text`, `set_parts` and `set_icon`,
  `IconList::set_items`, `move_children`. The rest set every time:
  `Hwnd::set_text` (labels, the title), `Toolbar::set_text`, which also
  sizes the buttons again, `StatusBar::set_tip`, and the progress bar.
  So `app` grew its own caches to avoid them (`inspector_shown`,
  `status_right`, `status_memory`), and a window without a cache sets
  every text on each poll, as the File operations window's `show_view`
  does.

## What DESIGN.md says that does not hold

1. **"A view diffed against the controls would keep a second copy of the
   selection and the scroll position."** A second copy is the wrong
   mechanism, but not for that reason: a copy of what was set drifts from
   the control. A check box and a toolbar check button toggle themselves
   on a click, an icon's bitmap depends on the DPI, the system colours
   and Show icons, and a Settings page is made from its template each
   time it first shows. The control itself is the record of what it
   shows. The code already relies on that in the setters above; the rule
   is to make every setter do it, not to add a diffed view.
2. **"`WM_SETREDRAW` is turned off only around a change that would draw a
   control several times."** The rule is right, and a resize step is such
   a change: `ColumnSizing::resized` gives every column a new width on
   each step. What does not hold is how the step ends: `draw_once_after`
   redraws with `RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN`, so the list is
   erased before its double-buffered paint and its scroll bars are drawn
   again on every step. Also, a window whose drawing is off has no
   `WS_VISIBLE` bit, so the main window does not clip the list while it
   moves, and a synchronous erase of the area it uncovers can paint over
   it (suspected, not filmed).
3. **"The main window … erases only its own background between controls,
   once after each layout."** Windows already invalidates the area a moved
   control leaves, and the class has no `CS_HREDRAW` or `CS_VREDRAW`. The
   whole-window `InvalidateRect` with erase after each layout erases every
   gap again on each step, and is needed only for an area the window
   paints itself that no move invalidates; the dividers are static
   controls, so there may be none.

These hold and stay: native controls, no framework, no
`WS_EX_COMPOSITED`, no free functions over a global `RefCell`, captions as
functions of facts (window rule 12), dialogs queued as modal steps
(window rule 13), one thread and connection per request (window rule 3),
one crate. `core` builds and runs its tests on Ubuntu on every push and
pull request (`ci.yml`), so the rule that it holds no Win32 type is
already enforced; a crate of its own would save compile time only by an
amount nobody has measured, and costs the dead-code warnings a library's
`pub` items lose, the catalogue-key test's reach into `app` and `win`, and
new entries in `deny.toml` and CI.

## The shape

### Setters that read the control

Every `win` setter compares the new value with the control's current
value and sets only when they differ. Reading a control is not a decision
about the model, so `win` still decides nothing. Then a refresh may call
every setter of an area, a nested message may refresh again, and
reentrancy costs nothing, because a set that changes nothing does
nothing.

| Value | How the setter reads it |
| --- | --- |
| Text of a label, an edit or the title | `WM_GETTEXT`; a shortened path compares with the shortened candidate |
| Check box | `BM_GETCHECK` |
| Toolbar button checked, enabled, text | `TB_ISBUTTONCHECKED`, `TB_ISBUTTONENABLED`, `TB_GETBUTTONINFOW` |
| Status bar part text, edges, icon | `SB_GETTEXTW`, `SB_GETPARTS`, `SB_GETICON` |
| Icon or image list | the handle, exact because `glyph_icon` and the image lists cache by glyph, size and colour |
| Enabled | `IsWindowEnabled` |
| Visible | the `WS_VISIBLE` bit, not `IsWindowVisible`, which walks the parents |
| Rectangle | `is_placed_at`, which compares only position and width for a combo box |
| Progress bar position and range | `PBM_GETPOS`, `PBM_GETRANGE` |
| List item count, column width, combo selection | `LVM_GETITEMCOUNT`, `LVM_GETCOLUMNWIDTH`, `CB_GETCURSEL` |

A few values cannot be read back. The wrapper keeps the last value it
set, made and dropped with the control:

- the inspector's file icon, because `show_file_icon` sets a new copy of
  the icon each time, so a handle never compares equal;
- the administrator shield, because `BCM_SETSHIELD` has no getter;
- whether a progress bar's marquee runs, because `PBM_SETMARQUEE` has no
  getter and setting it again restarts the animation;
- a tooltip's text, kept by `Tooltip`, which owns its tools.

A window inside its own `draw_once_after` has no `WS_VISIBLE` bit, so no
setter reads that window's visibility there.

Menus stay out of this: their item states are set when a menu opens
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
ended:

1. **Structure effects:** the effects that change what the controls are,
   such as inserting a column with its header or setting a list's item
   count, because a value can be set only on a control part that exists,
   and some reads, such as the rows in view, need the new count.
2. **Refresh** each touched area: compute its values in `core` from the
   model and the measured inputs, and call the setters.
3. **The other effects, in order:** requests, timers, focus moves, which
   come after the sets because Windows drops the focus of a control it
   disables, and cancels. An effect that runs its own message loop, such
   as Settings, a menu, a drag or a message box, is queued as a
   `ModalStep` and runs from `Message::Deferred` (window rule 13), so no
   loop runs inside these steps. Effects stop once the window is
   destroyed: a request started after `WM_DESTROY` has joined the request
   threads would break window rule 5.

A message that arrives during a step, sent back by a set or an effect,
runs its own handler and its own three steps; there is no guard.

### What the loop covers

- **The main window.** Its model goes through this loop. A notification
  that only records state a control owns, such as each
  `LVN_ITEMCHANGED`, which records the told rows and posts one
  `Message::Deferred`, is not an update; the loop runs at most once for
  each message it handles.
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
- **A control is moved only when its rectangle changes,** all of a
  layout's moves in one `DeferWindowPos` batch, every control with
  `WS_CLIPSIBLINGS` and every parent with `WS_CLIPCHILDREN`, as now.
- **`WM_SETREDRAW` only around a change that would draw one control
  several times,** such as the list's move and its column widths on a
  resize step, or a language switch, and the redraw after it has no
  `RDW_ERASE` and no `RDW_FRAME` for a double-buffered control.
- **No erase of the whole window after a layout;** only an area that the
  window paints itself and that no move invalidates.
- **No `WS_EX_COMPOSITED`:** it buffers every child again, clashes with
  the list views' own buffering and slows every paint.

## Rules for writing window code

These join DESIGN.md's window rules as 14 and up, because code and docs
cite rules 1 to 13 by number:

14. **A handler turns a message into an update and handles its
    `Accepted`.** It sets no control outside the three steps.
15. **Model the state by its rules.** A boolean for an independent fact;
    an enum that carries its data for states that exclude each other; an
    enum with one transition function for a state machine whose moves are
    constrained; a trait only where areas behave differently behind one
    call. Two fields that must agree, a combination that cannot happen,
    or the same condition tested in several places mean the model is
    wrong: change the representation instead of adding a branch.
16. **A fact has one owner,** the area that changes it; another area reads
    it through the model, never through a copy.
17. **Every `win` setter reads the control first** and sets only a value
    that differs; a value the control cannot return is kept by its
    wrapper. `app` keeps no cache of what a control shows.
18. **A timer is a variant of `Timer`,** never a bare number.
19. **A test asserts what the person would see,** the values an area
    shows for one representative state, given fixed measures, or the
    effects of an update in their order, against literals the scenario
    predicts. No test needs a window.
20. **A file over 2,500 lines is split:** `core` and `app` along their
    areas, `win` by control, test modules included in the count.

## Stages

One implementer keeps its context through the stages and fixes the
review's findings; one reviewer reads each stage and reports. No stage
changes behavior other than removing flashes and unchanged sets. Each is
its own commits and lands on `main` with `check.ps1` green. A defect found
on the way is filed, or fixed in its own commit when it is small.

### Stage 1: painting

1. `TBSTYLE_EX_DOUBLEBUFFER` on every toolbar (`win::toolbar`).
2. The redraw at the end of the list's `draw_once_after` on a resize
   step has no `RDW_ERASE` and no `RDW_FRAME`. It keeps `RDW_ALLCHILDREN`
   for the header, because whether the header follows the list's
   `WM_SETREDRAW` is not known.
3. The whole-window `InvalidateRect` after a layout goes. The implementer
   names the area the main window paints itself that no move
   invalidates, or says there is none.
4. One screen recording before the stage and one after, of the same
   steps: a border drag in both modes, a splitter drag, typing a search,
   a mode switch and a status update. Each runs once, when the machine is
   free, and is saved for the owner without waiting for them. A frame that
   shows a cleared, half-drawn or doubled control after the stage is a
   defect of the stage.
5. Only if the recording after the stage still shows them:
   - the list flashing while it moves: the list moves outside its
     `draw_once_after`, and only the column widths stay inside;
   - the horizontal scroll bar flashing on a narrowing drag: the columns
     are fitted before the move when the list narrows;
   - a half-drawn page on a Viewer scroll: its area paints through
     `BeginBufferedPaint`. Today it erases nothing and paints each row
     opaque, so no fault is known.

### Stage 2: state types

1. `Instance` moves into `core`, `Model` gets it at creation and answers
   `offers_file_operations` from it, and `Model::file_operations` goes.
   In `app`, the queue and the drop target become one
   `OnceCell<FirstWindow>`, filled when the window is created.
2. `Ending { Running, ForSetup, ForReset }` replaces `closing_for_setup`
   and `resetting`: a setup starts from a fix and Reset all from
   Settings, so a window never ends for both.
3. `core` keeps the shown Settings page as `Option<SettingsPage>` in
   place of `index_page_shown`; `app` keeps the dialog's handle. These are
   two facts with two owners.
4. Search's `search_due` and `open_on_answer` move with Search's list
   into `SearchArea { list, due, open_on_answer }`, because a search
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

### Stage 3: areas

A move, with no change of behavior or of what an update returns:

1. Each area gets its `core` type and its `app` module, in the order of
   how separate the areas already are: the inspector, the filter pane,
   the status bar, Search's list, the key bar, the bridge to Settings,
   the tray and the instance, then the panels, whose code already lives
   in `core::panel` and `app/main_window/panels.rs`.
2. `Model` keeps thin methods that forward to the areas while callers
   move, and drops each once no caller uses it.
3. `MainWindow` routes each control ID and timer through one table.

**Proof:** the existing tests pass with only their paths and names
changed. Done when no area's field is in `Model` or `MainWindow` and no
file breaks rule 20.

### Stage 4: what an update returns

1. `Accepted` becomes `{ touched: Areas, effects: Vec<Effect> }`; `Then`
   becomes `Effect`, and `Change` goes. Its acting arms become effects:
   `Message` clears the list's selection and count and cancels its
   `Selection` request, `ExportFailed` queues a `ModalStep`,
   `TotalCommander` and `IncludedCount` post to Settings, `NewSearch`
   cancels a request, `Mode` starts or stops the panels, and `Panes`
   moves the focus.
2. `app` handles an `Accepted` in the three steps, and stops running
   effects once the window is destroyed.
3. `core` emits the modal steps it decides; `app` keeps its queue for the
   steps that its own threads, drops and checks raise.
4. The other windows' loops take the same three steps.

**Proof:** the tests that assert `change` or `then` (38 in
`core/model.rs`) are rewritten to assert the touched areas and the
effects in their order, against what each scenario predicts.

### Stage 5: setters that read the control

1. `Hwnd::set_text`, `Toolbar::set_text`, `StatusBar::set_tip` and the
   progress bar's setters read the control first; the four values the
   table above cannot read are kept by their wrappers.
2. Area by area, the refresh calls the setters with the values `core`
   computes, and `app`'s caches go: `inspector_shown` (its file icon
   index moves into the wrapper), `status_right` and `status_memory`
   where the status bar's values can be computed again, and the File
   operations window's sets on each poll.
3. One measured run on the window's thread of a status reply, a key in
   the search box, a resize step, a splitter drag and Shift+End over
   100,000 rows, reported with the stage. It sets no threshold, because a
   threshold depends on the machine.

**Proof:** the recording steps of stage 1 once more, compared with stage
1's.

### Stage 6: the docs

DESIGN.md "The window" takes rules 14 to 20, the three steps and the
areas; its Painting item takes the painting rules; the paragraph on the
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
- No crate of its own for `core`, unless a measured rebuild of an edit to
  `app` shows a large saving.
- The start-up cost of making every control before the window shows
  (#98) and a log of slow messages belong to `window-responsiveness.md`.
- No feature and no change a person can see, other than the flashes and
  unchanged sets that stages 1 and 5 remove.
