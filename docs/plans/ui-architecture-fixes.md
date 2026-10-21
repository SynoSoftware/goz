# Window architecture fixes

Two architecture reviews of `gozler-ui` on 2026-10-21 found places where a
rule has more than one owner, where a rule lives in `app` without a test, and
several defects found by reading the code. An independent reviewer and a
defender agreed each outcome below after reading the code at `7b94f06`. This
plan lists what is still to build.

## Schedule

**Not scheduled.** The owner decides when. Stages A, B, D, E, F and G and items C3 and C7 are built:

- A: a wake opens no dialog until every reply and panel event drained with it
  is applied (DESIGN.md: window rule 13); a new or failed search cancels its
  selection read; one method per panel kind ends that kind's work.
- B: the life of a file operation is in `core::operations`; the File
  operations window is made on first use; core decides the taskbar state.
- E: the "Use local" decision is in `core::programs`; a check that timed out
  runs once more.
- C3: an Include edit and closing Settings forget the Include count
  (DESIGN.md: The Include page).
- D: `core::layout::column_sizing::ColumnSizing` owns Search's column
  sizing (DESIGN.md: Results list); a taken row has no bold part; the
  panels' Ext and Size fit 7 letters and 10 digits in KB.
- F: `run` reads the pending selection of both modes (DESIGN.md: The
  selection).
- G: a `WH_MSGFILTER` hook gives the File operations window and the Viewers
  their keys while a dialog of the main window is open.
- C7: `SettingsDraft` holds Apply's outcome, its enabling and shield, the
  refused number boxes and the Use Total Commander link (DESIGN.md: Settings
  dialog, The File operations page); `Count::MAX` and `RingBlock`'s range
  are the engine's.

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
