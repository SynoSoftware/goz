# Plan: the window's architecture

The shape `gozler-ui` should have for the code it holds now, and the
stages that get it there without changing what the window does. The
design stays: `core` decides, `win` wraps Win32, `app` connects them, over
native controls (DESIGN.md: "Where the unsafe lives"). What changes is how
a decision reaches the controls, how the window avoids flashing, where
each part of the window keeps its state, and the rules for writing window
code.

## Schedule

**Not scheduled.** The owner decides when.

Stage 1 changes the same repaint code as the flicker work that another
session holds as defects 6 and 8, which landed its first part in
`362f5b2`. Stage 1 goes to that session, or starts after it ends.

## What DESIGN.md says that does not hold

DESIGN.md gives reasons for the window's shape. Most hold. These do not,
or no longer do:

1. **"A view diffed against the controls would keep a second copy of the
   selection and the scroll position, which the controls own."** A diffed
   view does not need either. It needs only the values the window sets
   and the control never changes on its own: texts, tooltips, enabled and
   checked states, visibility, rectangles, icons. The selection, the
   focus, the scroll position, typed text and dragged column widths stay
   with the controls. The code shows the cost of turning it down: it now
   diffs by hand, one kind of control at a time. `StatusBar::set_text`
   reads each part's text back from the control to compare;
   `StatusBar::set_icon` and `set_parts` compare; `MainWindow` caches
   `inspector_shown` to skip unchanged texts; the Drives and service page
   sets only the texts that changed; `set_column_widths` sets only the
   widths that differ; `Hwnd::set_visible` skips an
   unchanged state; `replace_menu_bar` keeps an unchanged menu bar. Each
   was added after a flash was seen, and each control without one still
   flashes or redraws for nothing: the File operations window's
   `show_view` sets every text on each poll. One diff in `core` replaces
   all of them and covers every control.
2. **"A crate of its own for the window's Win32 layer … protects nothing
   more."** True for `win`, not for `core`. `core` is 29,000 of the
   crate's 57,000 lines. In one crate, an edit to `app` recompiles `core`
   and its tests; the rule that `core` holds no Win32 type is checked only
   because the Ubuntu job happens to build it; and `core` needs
   `#![cfg_attr(not(windows), allow(dead_code))]` and a `main` that exits
   on other systems. As its own crate, the compiler enforces the
   boundary, `core` builds and tests on any system on its own, and an
   `app` edit leaves it compiled.
3. **"`WM_SETREDRAW` is turned off only around a change that would draw a
   control several times."** The rule is right, but every resize step
   breaks it: `MainWindow::layout` moves the controls inside
   `draw_once_after` on the list, so the list's drawing is off while it
   moves, then the list and its header are erased and drawn again. A
   window whose drawing is off counts as hidden to Windows, so the main
   window can erase its background where the list is before the list
   draws over it (suspected, not filmed). The scroll bar flash that this
   prevents has a cause that can be removed instead (stage 1).
4. **"The main window … erases only its own background between controls,
   once after each layout."** Windows already invalidates the area a moved
   control leaves. The whole-window `InvalidateRect` after each layout
   adds an erase of every gap on each step, and is needed only where the
   window paints a background that no control covers and no move
   invalidates.
5. **Priority 3, "Little code to write and maintain",** is the reason
   given for keeping the current shape, but it argues for this plan: the
   screen diff deletes the hand-written diffs and the 23 hand-written
   arms of `Change`, and the area split deletes no code but makes each
   area's code findable.

These hold and stay: native controls, no framework, no
`WS_EX_COMPOSITED`, no free functions over a global `RefCell`, captions as
functions of facts (window rule 12), dialogs queued as modal steps
(window rule 13), one thread and connection per request (window rule 3).

## The shape

### One loop

Every event that changes the model goes through one loop, the Elm loop on
native controls:

```text
event → core: area.update(event) -> Effects        model borrowed mutably
        core: screen(model) -> Screen               model borrowed to read
        core: Screen::changes(&applied, &new) -> Vec<Set>
        app:  win carries out each Set              no borrow held
        app:  runs each Effect                      no borrow held
```

- **`Screen` holds only what the window sets and the control never
  changes on its own.** A control-owned value, such as the selection, the
  focus, the scroll position, a person's typing or a dragged column width,
  is never in it; the window reads it from the control when an event
  needs it, and changes it only through an effect. A virtual list's rows
  stay out of it too: `LVN_GETDISPINFO` still formats a cell with a short
  read borrow.
- **`Screen::changes` is in `core`,** so "set only what differs" is
  tested on Ubuntu, and `win` carries out a list of sets and decides
  nothing.
- **Captions stay functions of facts.** `screen` calls the `captions`
  functions; a language switch makes every caption differ, so every one
  is set, inside one `draw_once_after`.
- **Effects are a list,** run in order after every borrow has ended:
  requests, timers, modal steps, setting the search box's text, selecting
  or focusing rows, setting a list's item count. Window rule 1 then holds
  by construction.
- **A set or an effect that sends a message back** runs `update` for that
  message, which only changes the model and queues its effects; nothing
  is applied while a pass applies. The applier records each value as
  applied before it makes the call. When a pass ends and a nested message
  changed the model, the applier builds the screen again and applies its
  changes, until a pass changes nothing. A pass that never settles is a
  broken invariant.

### Areas

The window is made of areas: Search's list with its search box, the
filter pane, the inspector, the status bar, each panel, the key bar, the
file operations, the bridge to Settings, the tray and the instance. Each
area has a `core` type, which holds its state and its `update` and
`screen` part, and an `app` module, which handles its controls' messages.
`Model` holds the areas and the facts several of them read: the strings,
the settings and the service's status. `MainWindow` holds the window: its
handle, the mode, the layout and the routing of each message to its area,
through one table from a control's ID or a timer to the area.

### Painting

The window never flashes and never shows stale pixels, with Windows' own
means only:

- **Each control that offers double buffering has it on:** list views
  (`LVS_EX_DOUBLEBUFFER`) and the tree (`TVS_EX_DOUBLEBUFFER`), as now,
  and every toolbar (`TBSTYLE_EX_DOUBLEBUFFER`), which has it off today.
- **A control is set only when its value changes,** by `Screen::changes`,
  because a status bar part, a label, a button and a toolbar draw again
  on every set, also of an unchanged value.
- **A control is moved only when its rectangle changes,** all of a
  layout's moves in one `DeferWindowPos` batch, every control with
  `WS_CLIPSIBLINGS` and every parent with `WS_CLIPCHILDREN`, as now.
- **No erase of the whole window after a layout.** Only an area that the
  window paints itself and that no move invalidates is invalidated.
- **`WM_SETREDRAW` only around a change that would draw one control
  several times,** such as filling every column at once or a language
  switch; never around a move.
- **The Viewer's text area,** the one surface Gozler paints, draws each
  page into a buffer and shows it at once (`BeginBufferedPaint`), so a
  scroll never shows a half-drawn page.
- **No `WS_EX_COMPOSITED`:** it buffers every child again, clashes with
  the list views' own buffering and slows every paint.

## How window code is written

These become DESIGN.md's window rules as the stages land:

1. **A handler turns a message into an event and returns.** It calls one
   area's `update` and then the loop. It sets no control itself; only the
   applier and the effects call `win`.
2. **Model the state by its rules.** A boolean for an independent fact;
   an enum that carries its data for states that exclude each other; an
   enum with one transition function for a state machine whose moves are
   constrained, such as a request or a closing drop-down list; a trait
   only where areas behave differently behind one call. Two fields that
   must agree, a combination that cannot happen, or the same condition
   tested in several places mean the model is wrong: change the
   representation instead of adding a branch.
3. **A request in flight is a `Request<T>`,** and a timer is a variant of
   `Timer`, never a bare integer.
4. **State has one owner.** A fact lives in the area that changes it;
   another area reads it through the model, never through a copy.
5. **`win` decides nothing.** It wraps one Win32 call or one control per
   function and keeps no "only when changed" check; that check is
   `Screen::changes`.
6. **A test asserts what the person would see:** the screen of one
   representative state of an area, or the effects of an event, against
   the values the scenario predicts. No test needs a window.
7. **A file over 2,500 lines** is split along its areas.

## Stages

One implementer keeps its context through the stages and fixes the
review's findings; one reviewer reads each stage and reports. No stage
changes behavior other than removing flashes, each is its own commits,
and each lands on `main` with `check.ps1` green. A defect found on the
way is filed, or fixed in its own commit when it is small.

### Stage 1: painting

1. `TBSTYLE_EX_DOUBLEBUFFER` on every toolbar (`win::toolbar`).
2. A resize step moves the list without `draw_once_after`. The horizontal
   scroll bar flashed because the list became narrower before its columns
   did, so the columns are fitted first when the list narrows, and after
   the move when it widens. The fill then takes the width from the
   layout's rectangle for the list, less its borders and vertical scroll
   bar, because `fill_width` reads the control, which has not moved yet.
3. The whole-window `InvalidateRect` after a layout goes, except for an
   area the window paints and no move invalidates.
4. The Viewer's area paints through `BeginBufferedPaint`.

**Proof:** one screen recording before the stage and one after, of the
same steps: a border drag in both modes, a splitter drag, typing a
search, a mode switch and a status update. Each runs once, not as a
repeated probe, and is saved for the owner without waiting for them. A
frame that shows a cleared, half-drawn or doubled control after the stage
is a defect of the stage.

### Stage 2: state types

1. `Request<T> { Idle, Asked(Generation), Answered(T) }` for the status,
   the Total Commander search, the Include count, the settings-saved
   notice and a panel's sizes, each held today as an option of its
   generation beside an option or a flag for its answer. A reply of
   another generation is dropped in one place.
2. One enum for each pair that is one state: `IncludeSave { Idle,
   Saving(Saving), Saved(Result<(), NotSaved>) }`, `RangeList { Idle,
   Closing(RangeFilter, ListClosing), Closed(RangeFilter) }`, and
   Settings as `Closed` or `Open { page }`, held in one place instead of
   `Model::index_page_shown` beside `MainWindow::settings`.
3. `Role { First { queue, drop_target }, Other }` for what only the first
   window has, recorded today in three fields.
4. `enum Timer` with one function to and from Windows' timer ID.
5. Search's `search_due` and `open_on_answer` move into its list's state,
   as a search panel keeps its own.
6. `selection_pending`, `column_dragged` and `panel_changed` stay
   booleans, each an independent fact, and share one function that posts
   `Message::Deferred` once.
7. `closing_for_setup` and `resetting` become one enum only if they
   cannot both be true; check first.

**Proof:** the existing tests pass unchanged; a new test only where a
rule moved into a new type and nothing watches it, such as `Request<T>`
dropping a reply of an older generation.

### Stage 3: effects

1. `Accepted::then` becomes `effects: Vec<Effect>`, and `Then` is renamed
   `Effect`. `ModalStep` becomes a kind of effect, still run from
   `Message::Deferred` (window rule 13).
2. `app` runs effects in one function after the model's borrow ends, and
   the workarounds for a second follow-up go.
3. The `Change` variants that carry control-owned state, `Rows`,
   `NewSearch`, `ColumnShown`, `ColumnHidden`, `Selection` and
   `SelectionRead`, become effects.

**Proof:** `core` tests assert the effects of the events that now return
two or more, against the list the scenario predicts.

### Stage 4: the screen

1. `core::screen` builds `Screen`, `Screen::changes` lists the sets, `win`
   carries them out, and `app` keeps the screen applied.
2. Each area moves to it in its own commit, deleting the hand-written
   refresh code and the hand-written diff it replaces: the status bar in
   the main window and in Settings (`StatusBar`'s read-back compare); the
   File operations window, whose `View` already is its screen; the
   toolbars, menu states and command buttons; the filter pane and the
   inspector (`inspector_shown`); the Drives and service page; then the
   layout's rectangles and visibility (`Hwnd::set_visible`'s check).
3. The rest of `Change` goes.
4. DESIGN.md's paragraph on the rejected diffed view is replaced by what
   `Screen` holds and never holds.

**Proof:**
- A `core` test that two equal screens give no set, and that a changed
  status text gives that one set and nothing else; the failure it watches
  is a set of everything, which is a flash.
- A `core` test for each area moved, asserting the screen of one
  representative state, where no test already asserts those captions.
- Building the screen after each update must not slow the window. One
  run that counts the window thread's CPU cycles
  (`QueryThreadCycleTime`), as the window responsiveness plan's
  measurements did, over a status reply, a key in the search box and a
  resize. Over about 3 million cycles, about 1 ms, the screen is built
  only for the areas the update touched.
- The stage 1 recording steps once more, compared with stage 1's.

### Stage 5: areas

1. Each area gets its `core` type and its `app` module, in the order of
   how separate the areas already are: the inspector, the filter pane,
   the status bar, Search's list, then the key bar, the bridge to
   Settings, the tray and the instance. The panels already live in
   `core::panel` and `app/main_window/panels.rs` and move last into the
   same form.
2. `Model` keeps the areas and the shared facts; `MainWindow` keeps the
   window and the routing table.

**Proof:** the tests move with the code they test and pass unchanged.
Done when no area's field is in `Model` or `MainWindow` and no file of
`app` or `core` is over 2,500 lines.

### Stage 6: `core` as its own crate

1. `crates/gozler-ui-core` holds `core`, with `resources/ids.h`'s
   generated IDs, which `build.rs` writes for it. `gozler-ui` depends on
   it. The `cfg_attr(not(windows), allow(dead_code))` and the `main` for
   other systems go.
2. The same change adds the crate to CI's Ubuntu test step and to
   `deny.toml`.

**Proof:** `check.ps1` green, and one timed `check.ps1 -Test` after an
edit to one `app` file, before and after the stage.

### Stage 7: the docs

DESIGN.md "The window" describes the loop, the screen, the effects, the
areas, the painting rules and the rules for writing window code, and
this plan is deleted.

## Not in this plan

- No framework, no data binding and no MVVM: Win32 has no binding, and
  writing one means notifications that call back into the window, the
  panic window rule 1 exists to prevent.
- No new drawn surface: owner draw still needs the owner's permission.
- No change to the virtual lists, to how rows are fetched, or to the
  selection contract.
- No feature and no change a person can see, other than the flashes that
  stages 1 and 4 remove.
