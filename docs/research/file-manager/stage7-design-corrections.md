# Stage 7 design: corrections after the design recheck

These override `stage7-design.md` where they differ. Settled by the lead.

1. **B1, which failures search again.** On a status reply, a list
   searches again only when its shown failure `recovers()`
   (`DaemonFailure::recovers` in `core::fetch`). `Refused` and
   `NotGozler` fail the same way again, so they never search again by
   themselves. The status request that `accept_results` makes because
   another list shows a failure uses the same filter: it is made only
   for a failure that `recovers()`.
2. **B3, Insert and Space.** `PanelKey::Toggle` in `on_panel_key` uses
   `Panels::selectable` too, not `entry(row).is_some()`, so `Insert` on a
   focused row whose chunk has not arrived selects it.
3. **B4, placing a panel's message.** `layout()` places each panel's
   message again, next to `update_message()` for Search's, so a resize,
   a DPI change and a language switch move it. The layout test that
   asserts `panel.retry.bottom() <= panel.list.bottom()` is rewritten for
   the shared `place_message`; say what it pinned in the stage report.
4. **The size of a selection (2.10).** Skip the walk over parents when no
   folder is selected. Otherwise check only the depths at which a selected
   folder exists. The section 2.16 sentence "no list does per-row work
   that Search's list did not do at HEAD" is corrected to name this walk.

## Spec defects found by the stage 8 pass (do with stage 7)

5. **The selection after a submitted copy, move or delete.** The spec
   says submitting clears it; the code leaves the selection after a copy
   (a move or delete removes the entries with their selection). Clear the
   panel's selection when its Copy, Move or Delete is submitted.
6. **The Keyboard page in Two panels.** The spec says the page says which
   keys Two panels fixes and lists them, and that `Alt`+character
   shortcuts work only in Search. The page says none of this. The
   proposed text is in `.work/stage8-context-and-help.md`.
7. **Accepted as built, no code change:** a copy's source does not make a
   panel read its folder again (a copy does not change its source); the
   Exit question keeps stage 2's wording. The spec and DESIGN follow the
   code.
