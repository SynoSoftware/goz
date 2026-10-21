# Stage 7: fixes to do after the merge into main

Stage 7 was committed without these fixes, by the owner's choice (merge
everything now, review findings after). Line numbers are from the
pre-merge tree; find the code by name after the merge.

## From the adversarial review (Opus)

- **M1. A search panel keeps blank rows highlighted after an operation in
  another list takes them.** `show_taken` calls `show_panels_taken`
  before `model.set_taken(taken)`, so the panel's deselect pass reads the
  old `Model.taken` through `Model::row_taken`, while `Panels::set_taken`
  already dropped the file from the selection. The status line then
  counts fewer than the highlighted rows, and with one selected result
  F5/F6/F8 are greyed. Root: two copies of the taken set (`Model.taken`,
  `Panels.taken`). Fix: one owner of the taken set (or at least
  `model.set_taken` before `show_panels_taken`).
- **M2. A list already searching again after a failure restarts on every
  status reply.** `shows_recovering_failure` checks only
  `Shown::Message(Failure)`; add `&& !shown.results.searching()`, as
  `accept_status`'s `Outcome::Failed` branch does.
- **L1 = spec check: Ctrl+F is not a fixed key of Two panels.** Add it to
  `TWO_PANEL_KEYS` after Alt+F7; `Ctrl+E` stays Search-only; list Ctrl+F
  in the Keyboard note.
- **L2.** The Keyboard note leaves out Alt+, and Alt+. (all four
  catalogues).
- **L3.** Ctrl+A or a drag can select blank (taken) rows in a search
  panel and they stay highlighted (counts are right).
- **L4.** Comments no longer true: `Selection::Files` "never one folder
  alone" (only Search's list); `panels.rs` "A new search starts in this
  folder…" above the Look in sync.
- **L5.** Shift selection encodes each fetched row while an operation
  runs: add to issue #108.
- Removals: the second taken set (with M1); `MatchedParts::new` for panel
  searches; `Panel::select`'s marking loop for a search panel; the
  `ListMessage::MissingFolder` arm of `panel_list_message`.

## From the spec and prototype check (Sonnet), lead's decisions

- The left status line is empty when a search finds nothing; the
  prototype shows "0 results in C:\Users\user\Documents". Show it.
- The empty search box shows Search's cue "Find files and folders" while
  the list message already says what to type; the prototype has none.
  Remove the cue in a panel.
- No change: status bar totals alignment, the date format, the spec's
  "not indexed" text.

## Memory

Run 1's B1-B0 was 7.55 MB against 7.5 MB; run 2 was 7.04 MB; the rows
alone are 6.89 MB. Variation is plausible, not proven for run 1. Rerun the
memory script once after the fixes and record it.
