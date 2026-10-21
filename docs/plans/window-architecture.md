# Plan: tidy the window's architecture

How to make `gozler-ui` cheaper to change without changing what it does.
The design stays: `core` decides, `win` wraps Win32, `app` connects them,
over native controls (DESIGN.md: "Where the unsafe lives"). What changes is
how a decision reaches the controls, and where each part of the window
keeps its state.

## Schedule

**Not scheduled.** The owner decides when.

The flicker work that another session holds as defects 6 and 8 changes the
same repaint code as stage 3. Stage 3 starts after that work lands, and
keeps what it fixed.

## Why

Measured on `main` at `91a69ec`:

- `app/main_window.rs` and `app/main_window/panels.rs` hold 7,700 lines,
  about 216 methods and about 40 `Cell` or `RefCell` fields. `core/model.rs`
  holds 3,700 lines and 198 functions. Every feature of Search, Two panels,
  the inspector, the filter pane, the status bar, Settings, the tray and
  the file operations adds to these two types.
- A model method answers `Accepted { change, then }`. `Change` has 23
  variants, and for each one `app` chooses by hand which controls to set
  again. When in doubt it sets too much: a layout repaints the whole main
  window, the status bar sets every part's text and tip every 2 seconds,
  and the File operations window's `show_view` sets every text on each
  poll whether or not it changed. Each of these is a flash, and each was
  found one at a time.
- `then: Option<Then>` allows one follow-up per event, so an event that
  needs two works around it in `app`.
- Window rule 1, no model borrow across a Win32 call, holds only because
  every handler remembers it; a mistake panics at runtime, and only when
  Windows happens to send the message.
- Several facts are spread over fields that can disagree. Examples:
  `include_count_asked` and `included_count`; `include_saving` and
  `include_saved`; `range_closing` and `range_closed`; `index_page_shown`
  in `Model` with `settings` in `MainWindow`; whether this is the first
  window, which `Model::file_operations`, `MainWindow::file_operations`
  and `MainWindow::drop_target` each record. The timers are bare integers
  (`PANEL_SETTLE: [8, 9]`).

## The design

One loop for every event that changes the model, the Elm loop on native
controls:

```text
event → core: update(model, event) -> Effects      model borrowed mutably
        core: screen(model) -> Screen               model borrowed to read
        core: Screen::changes(&last, &new) -> Vec<Set>
        app:  win applies each Set                  no borrow held
        app:  runs each Effect                      no borrow held
```

- **`Screen` holds only what the window sets and the control never
  changes on its own:** each control's text, tooltip, enabled and checked
  state, visibility, rectangle and icon, the menu items' states, the
  status bar's parts, the title. DESIGN.md rejects a view diffed against
  the controls because it would keep a second copy of the selection and
  the scroll position. `Screen` keeps no copy of anything a control owns:
  the selection, the focus, the scroll position, the text a person types,
  column widths and order a person drags, and a virtual list's rows,
  which `LVN_GETDISPINFO` still reads with a short borrow. Those reach the
  controls as effects, as they do today.
- **`Screen::changes` lives in `core`,** so the rule "set only what
  differs" is tested on Ubuntu, and `win` only carries out a list of sets.
  A control is set only when its value differs from the last one applied,
  so an unchanged status text is never written, and a control that did not
  move is never moved or repainted.
- **Captions stay functions of facts** (window rule 12). `screen` calls the
  same `captions` functions that `WindowCaptions::new` calls now; a
  language switch makes every caption differ, so every caption is set.
- **Effects are a list,** run in order after every borrow has ended. A
  call that can send a message back, such as setting the search box's text
  for Clear, or selecting rows, is an effect. So window rule 1 holds by
  construction, not by memory.
- **A set or an effect that sends a message back** runs `update` for that
  message, which only changes the model and queues its effects; it applies
  nothing while the outer pass applies. The applier records each value as
  applied before it makes the call. When the pass ends and a nested
  message changed the model, the applier builds the screen again and
  applies its changes, until a pass changes nothing. A pass that never
  settles is a broken invariant.
- **Each area of the window owns its state:** the search list with its
  box, the filter pane, the inspector, the status bar, each panel, the
  file operations, Settings' bridge, the tray and the instance. Each area
  has a `core` type with its own `update` and `screen` part, and an `app`
  module with its messages. `Model` and `MainWindow` hold the areas and
  route to them; they hold no area's fields.

This is not MVVM. Win32 has no data binding, and writing one would mean
property notifications that call back into the window, the panic that
window rule 1 exists to prevent.

## Stages

One implementer keeps its context through the stages and fixes the
review's findings; one reviewer reads each stage and reports. Each stage
changes no behavior, is its own commits, and lands on `main` with
`check.ps1` green. No stage mixes in a feature or a fix of a defect it
did not find; a defect found on the way is filed, or fixed in its own
commit when it is small.

### Stage 1: state types

Local changes the compiler checks at every use:

1. `Request<T> { Idle, Asked(Generation), Answered(T) }` for the requests
   in flight that are held as an option of their generation and an option
   of their answer: the status, the Total Commander search, the Include
   count, the settings-saved notice and a panel's sizes. A reply whose
   generation is not the one asked is dropped in one place.
2. One enum for each pair that is one state: `IncludeSave { Idle,
   Saving(Saving), Saved(Result<(), NotSaved>) }`, `RangeList { Idle,
   Closing(RangeFilter, ListClosing), Closed(RangeFilter) }`, and
   Settings as `Closed` or `Open { page }`, held in one place.
3. `Role { First { queue, drop_target }, Other }` for what only the first
   window has.
4. `enum Timer { Search(List), Status, Selection(List), PanelSettle(Side),
   … }` with one function to and from Windows' timer id.
5. Search's own flags, `search_due` and `open_on_answer`, move into its
   list's state, as a search panel keeps its own.
6. The flags that say "a deferred read is posted", `selection_pending`,
   `column_dragged` and `panel_changed`, stay booleans, because each is an
   independent fact, but share one function that posts `Message::Deferred`
   once.

Before merging `closing_for_setup` and `resetting`, check whether both can
be true at once; merge them only if not.

**Proof:** the existing tests pass unchanged. A new test only where a rule
moved into a new type and no test watches it, such as `Request<T>`
dropping a reply of an older generation.

### Stage 2: effects

1. `Accepted::then` becomes `effects: Vec<Effect>`; `Then` is renamed
   `Effect`.
2. `app` runs effects in one function, after the model's borrow ends,
   and the workarounds for a second follow-up go.
3. The calls that send a message back and are made inside handlers today
   move into effects where the handler holds a borrow nearby.

**Proof:** `core` tests assert the effects of the events that now return
two, against the list the scenario predicts.

### Stage 3: the screen and its changes

1. `core::screen` builds `Screen`, and `Screen::changes` lists the sets.
   `win` carries them out; `app` keeps the last screen applied.
2. Areas move to it in this order, each in its own commit, deleting the
   hand-written refresh code it replaces:
   1. the status bar, in the main window and in Settings;
   2. the File operations window, whose `View` already is its screen;
   3. the toolbars, the menu states and the command buttons;
   4. the filter pane and the inspector, whose `inspector_shown` cache
      becomes part of the screen;
   5. the layout's rectangles: a control is moved only when its
      rectangle changed, and the whole-window repaint after a layout
      goes, except where the window paints its own background.
3. The `Change` variants that only said which controls to set again go.
   Those that carry control-owned state, such as `Rows`, `NewSearch`,
   `ColumnShown`, `ColumnHidden` and `Selection`, become effects.
4. DESIGN.md's paragraph on the rejected diffed view is rewritten to say
   what `Screen` holds and what it never holds.

**Proof:**
- A `core` test that `Screen::changes` of two equal screens is empty, and
  that a changed status text gives one set and nothing else; the failure
  it watches is a set of everything, which is the flicker.
- One `core` test for each area moved, asserting the screen of one
  representative state, where no test already asserts its captions.
- One screen recording of the window before stage 3 and one after, of the
  same steps: a border drag, a splitter drag, typing a search, a mode
  switch and a status update. Each run happens once, not as a repeated
  probe. A frame that shows a cleared, half-drawn or doubled control
  after the stage is a defect of the stage. The recordings are saved for
  the owner and wait for no one.
- Building the screen after every update must not slow the window. One
  run that counts the window thread's CPU cycles
  (`QueryThreadCycleTime`), as the window responsiveness plan's
  measurements did, over a status reply, a key in the search box and a
  resize gives the cost. If it is over about 3 million cycles, about 1 ms, on the
  window's thread, the screen is built only for the areas the update
  touched.

### Stage 4: split by area

1. Each area gets its `core` type, with the fields, `update` and `screen`
   part that are its own, and its `app` module, with the messages of its
   controls. The order follows how separate the areas are already: the
   inspector, the filter pane, the status bar, the search list, then
   Settings' bridge, the tray and the instance. The panels already live
   in `core::panel` and `app/main_window/panels.rs`.
2. `Model` keeps the areas and the facts several areas read, such as the
   strings, the settings and the service's status. `MainWindow` keeps the
   window: its handle, the mode, the layout and the routing of each
   message to its area.
3. A message reaches an area by its control's ID or its timer, through
   one routing table, not a chain of `if let`.

**Proof:** the tests move with the code they test and pass unchanged. The
stage is done when no area's field is in `Model` or `MainWindow`, and no
file of `app` is over 2,500 lines.

### Stage 5: the docs

1. DESIGN.md "The window" describes the loop, the screen, the effects and
   the areas, and window rule 1 says that the loop enforces it.
2. This plan is deleted.

## Not in this plan

- No framework, no data binding, no MVVM.
- No drawn surface: every control stays native, and owner draw still needs
  the owner's permission.
- No change to the virtual lists, to how rows are fetched, or to the
  selection contract.
- No new feature and no change a person can see, other than the flashes
  that stage 3 removes.
