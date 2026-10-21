# Window architecture fixes

Two architecture reviews of `gozler-ui` on 2026-10-21 found places where a
rule has more than one owner, where a rule lives in `app` without a test, and
several defects found by reading the code. An independent reviewer and a
defender agreed each outcome below after reading the code at `7b94f06`. This
plan lists what is still to build.

## Schedule

**Not scheduled.** The owner decides when. Stages A, B, C, D, E, F and G and items C3 and C7 are built:

- A: a wake opens no dialog until every reply and panel event drained with it
  is applied (DESIGN.md: window rule 13); a new or failed search cancels its
  selection read; one method per panel kind ends that kind's work.
- B: the life of a file operation is in `core::operations`; the File
  operations window is made on first use; core decides the taskbar state.
- E: the "Use local" decision is in `core::programs`; a check that timed out
  runs once more.
- C: `MainWindow::note_saved_state` starts the save, and a save writes
  only the settings this window changed (DESIGN.md: Where settings are
  saved).
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
