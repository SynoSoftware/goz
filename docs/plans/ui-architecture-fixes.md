# Window architecture fixes

Two architecture reviews of `gozler-ui` on 2026-10-21 found places where a
rule has more than one owner, where a rule lives in `app` without a test, and
several defects found by reading the code. An independent reviewer and a
defender agreed each outcome below after reading the code at `7b94f06`. This
plan lists what is still to build.

## Schedule

**Not scheduled.** The owner decides when. Stages A, B, E, F and G and item C3 are built:

- A: a wake opens no dialog until every reply and panel event drained with it
  is applied (DESIGN.md: window rule 13); a new or failed search cancels its
  selection read; one method per panel kind ends that kind's work.
- B: the life of a file operation is in `core::operations`; the File
  operations window is made on first use; core decides the taskbar state.
- E: the "Use local" decision is in `core::programs`; a check that timed out
  runs once more.
- C3: an Include edit and closing Settings forget the Include count
  (DESIGN.md: The Include page).
- F: `run` reads the pending selection of both modes (DESIGN.md: The
  selection).
- G: a `WH_MSGFILTER` hook gives the File operations window and the Viewers
  their keys while a dialog of the main window is open.

Still open from B: run the ignored release-only timing test
`timing_progress_folder_per_operation` once. If 100,000 sources still take
longer than one frame (16.7 ms) with the allocation-free `common_folder`,
move the common folder to the engine's prepare thread and carry it in the
snapshot.

## C. One owner for saving

The window saves its state a second after it changes (DESIGN.md: Where
settings are saved). Today several changes start no save, so they are lost
when the window ends without Exit.

1. One `MainWindow` step decides saving. It compares the saved state with
   the state seen at its previous run and starts the save timer only when
   that state changed. The save timer's handler writes nothing when nothing
   differs from the last saved copy, which is taken when a save is
   attempted, whether or not it succeeds. The saved state is `Settings`,
   `Panels` (compared by reference with `SavedPanels`, so nothing is cloned
   unless it changed), the Viewers' state and the placement. The step runs
   at the end of `apply`, at the end of `on_panel_events`, after each panel
   command that calls `panels_mut()`, and after the Viewer's
   `remember_placement` and Wrap toggle. Panels stay outside `Accepted`.
   This replaces `MainWindow::read`'s own start of the save timer. That
   start comes too early: a folder that takes over a second to open is
   saved as the old folder, because `Panel.folder` changes only when the
   reading opens or completes.
2. The placement is recorded on `WM_WINDOWPOSCHANGED`, which covers maximize,
   restore, snap and Win+Arrow moves, and only once the window has shown.
   Today a start in the notification area in Two panels saves the hidden
   window's normal placement, so a maximized window comes back not
   maximized. The Viewer records its placement on the same message.
3. Column order is saved after a header drag (`HDN_ENDDRAG`, then
   `Message::Deferred` reads the order). Showing or hiding a column keeps
   `settings.window.columns` current.
4. Another window must not overwrite what this window saved. At save time,
   read `ui.json`, write only the fields whose value differs from this
   window's last-saved copy, and keep every other field from the disk: a
   key-by-key merge over `serde_json::Value`. This follows DESIGN.md's "A
   setting that two windows change is the one saved last" per setting;
   replacing whole groups is too coarse, because `settings` holds every
   choice.
5. Settings' Apply runs its refreshes inside one `draw_once_after`;
   `refresh_text` must not open its own. `show_index_page`, `set_language`
   and `set_destination_folder` return `Accepted`.

Defects this fixes: Language or Apply in another window; column reorder;
the main window's and the Viewer's placement; the active panel; a search
panel's sort; the Viewer's Wrap; the placement saved before the first show;
a slow folder saved as the old one.

## D. Column sizing

Commit 8da09ba fixed a window shown again from the notification area that
kept old column fits. Its fix added a sixth place that must reset Search's
sizing fields by hand.

1. A core type owns Search's five sizing facts. It takes events (width
   changed, drag started and ended, DPI changed, rows arrived, column hidden
   or shown, window shown) and answers with widths, so each reset becomes an
   event with a test. The panels stay outside it: their widths are fixed
   fits to sample values, and Name takes the rest.
2. One core function returns a Search cell's bold ranges, and none for a
   taken row. `draw_cell` and `fits_of` call it; `fill_cell` does not,
   because screen readers need only text and an icon. This fixes taken rows
   being measured with bold parts, which makes a column fill slightly too
   wide.
3. The panels' samples are too narrow. "xlsx" cuts "torrent", and
   99,999,999 KB is one digit short of a 100 GiB folder. Ext fits 7
   characters, and Size fits a 10-digit KB figure. The widths stay the same
   in both panels and every folder. Change `fill_panel_columns`' doc comment
   and DESIGN.md's "A panel" ("fit their content") to say so. A longer
   extension ends in "…". Check whether the label tip shows it whole for
   owner-drawn panel rows.

## C7. Apply's outcome, in core

- `gozler-fileops` exposes `Count::MIN` and `Count::MAX`. The per-processor
  clamp becomes `shown.min(Count::MAX)`. `ring_blocks` takes its powers from
  `RingBlock`'s range, not `16..=26`.
- `SettingsDraft` gets the per-window outcome: a refused shortcut applies
  nothing, and a failed registry write reverts that value. Use a name other
  than `include_saved`, and keep `applied()`'s signature. The save as
  administrator is `include_saved`, from the window-responsiveness work. The
  dialog's apply calls the new method.
- The page rules move to core with tests: the Total Commander link, Apply's
  enabling and its shield, the `read_speed` dependency, and the
  per-processor clamp.
- The draft keeps the number boxes that refused their text, by
  `SpeedNumber` with the text, so whether Apply is enabled is one core rule
  and not Win32 state.

Tests: a refused shortcut applies nothing; a failed write reverts and refills
the page; a declined Include save stays edited with the shield; turning
per-processor off clamps the count to the top.

## Documents

- DESIGN.md window rule 4 calls the generation check "the only staleness
  check", but the panels have three: `PanelThreads::is_reading`, the
  senders' `Stop`, and `ReadChange::Stale`.
- DESIGN.md "How normal changes land": a new column's arm is in
  `text::cell`, not the `LVN_GETDISPINFO` handler.

## Smaller findings

- `win::controls::ReportList::fit_columns` fits and fills a second way, for
  the File operations failure list. Route it through D's pure fits function,
  and keep its scroll-bar subtraction or settle it on purpose.
- `InspectorCaptions` uses stand-ins for states: an `Option` for "exactly
  one entry", an `Option` for "the row has an age", and an empty `String`
  for "no note". Give the inspector's state one enum: nothing, reading, one
  entry, several.
- The filter pane checks for an echo twice: `on_kind_selected`, and
  `set_filters` returning `Nothing`. The Match list repeats the pattern
  (`match_mode_index`, `MatchMode::ALL.get`).
- The drop-down height factors × 10 (Match, at most 5 items) and × 12 (Size,
  at most 10; Date at most 8) each fit their longest list. That reason is
  written nowhere.
- Not yet reviewed in the built code: the new `landing` and `stays_in_place`
  against the old `drag_to` "unchanged" test, and the File operations
  window's `show_hint`.

## Decided, nothing to build

- A controller type per list (pattern P1): the panels' lists differ from
  Search's in drawing, sizing, focus and activation, so one type would
  branch on the list kind.
- One channel for worker results (P5): the panels' disk threads already
  share one typed channel and one wake.
- The Inspector's layout (C5) and the filter pane's mapping (C6): no
  scheduled work touches them.
- Rectangles keyed by control id (C11): `Layout` already hides whole groups
  by mode.
- A panel keeps its scroll position by row number when entries are added
  above (P3), as built.
