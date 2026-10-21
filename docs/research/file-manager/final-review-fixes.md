# Final review: fix round 1

The final review (Opus, 2026-10-21) read a680253 and did not pass it. The
lead confirmed M1, M3 and H1 in the code. W2-W4 are filed as #110, the
low findings as #111; they are not in this round.

Fix in this round (find the code by name):

- **H1.** Two panels' keys (Up, Down, PgUp, PgDn, Home, End, Insert, Space,
  Tab) are taken in `offer_key` before the list or `IsDialogMessageW` sees
  them, and the only `show_focus_cues` caller is `move_tab_focus`. Since
  6bb243a the list draws the focus outline, which `UISF_HIDEFOCUS` hides,
  so the cursor may never show after a mouse start. Make the cues show
  when a panel key moves the focus (or when the panel takes the focus).
- **M1.** `Viewer::removal`, `Removal::Complete` drops the reader but
  leaves `Shown::File`: a pulled USB drive leaves the Viewer blank with no
  message and no Retry. Show what `Query` shows.
- **M2.** `refuse` opens `error_box` inside `on_panel_events`; the box's
  message loop delivers the next `WAKE`, so `on_panel_events` runs nested
  and the outer call applies older events after newer ones. Keep events in
  order (for example, show the box after the drain, or guard re-entry).
- **M3.** `Panels::accept_read`, `ReadEvent::Failed`: only `View::Folder |
  View::Search` give `Refused`; from Reading or NotReady a failed typed
  path or drive-menu pick shows "Drive X is not ready" for the wrong drive,
  loses the typed text, and Retry reads the old folder. The spec: a folder
  that cannot be opened gets a message box and is not a state.
- **M4.** `win/folder.rs` `follow`: the watch follows the folder object, so
  a rename of the shown folder by another program sends no `Lost`; rows of
  `out.bak` show under `C:\proj\out`, and F8 deletes in the new `out`.
  Detect it (for example, check the folder's path when the watch reports,
  or watch the parent for the folder's own name).
- **M5 (unconfirmed).** `Icons::up` appends the `..` glyph to the shared
  small system image list only (`ImageList_ReplaceIcon(-1)`), once more per
  colour change; shell32 keeps small and large lists index-aligned. Avoid
  appending to the system list unless you can show it is safe.
- **W1.** `Panel::filter` runs per change batch and, while a delete or move
  takes entries from the shown folder, folds and allocates the full path
  of every entry (`Taken::contains` → `folded`). Make it cost nothing per
  entry, for example fold the folder once and compare names.
- **Removals in files you touch:** `Message::EraseBackground` and
  `Message::Paint` (and their arm), `viewer.rs` `reopen` and its
  `remember_place()`, `in_flight.set(None)` in the `Query` arm, the
  `in_flight == Some(index)` guard in the `Block` arm, `.max(1)` in
  `viewer_window.rs` `set_area` and `wrapped`, `requests = None` in `Drop
  for FileReader`, `#[derive(Clone)]` on `WatchRelease`, the merge of
  `ReadChange::Opened`/`NotReady`, inlining `FileOperations::refresh`,
  `parent_of` spelled out in `transfer`, and `Saved::new`'s wrong doc
  comment (a rename is saved). Skip any that is not true when you look.
