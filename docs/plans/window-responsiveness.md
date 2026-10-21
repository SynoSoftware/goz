# Window responsiveness

The window still does some avoidable work on its message thread. This plan
lists what is left to remove in the existing Win32 window. It does not
change the renderer. Under the owner ruling "the window never lags"
(`AGENTS.md`), each wait here is a defect with an open issue.

Stages 1 and 2 of this plan landed in `89891df`. The timings that decided
what was built, and what was measured as not worth building, are in the
"Measured" section of `git show f6a8e7e:docs/plans/window-responsiveness.md`,
and the timing tests are `crates/gozler-ui/src/timing.rs`.

## Schedule

**Not scheduled.** The owner decides when.

## Keep the renderer

The window stays Win32 with the native results list. A Direct2D surface is
not part of this plan, for three reasons:

- [DESIGN.md](../DESIGN.md) puts low memory first and asks for plain Windows
  controls, so that Windows keeps keyboard behavior, focus, accessibility
  and high contrast. A custom results surface must rebuild all of these,
  and the selection contract gives selection to the native list today.
- A hardware Direct2D device loads the graphics driver into the process.
  DESIGN's own measurement was 4.6 MB for the native window, 12 MB for
  Slint's software renderer and 120 MB for its OpenGL renderer. Direct2D
  itself was not measured.
- The waits below are not drawing. A faster renderer does not shorten a
  filesystem call, a shell call, a prompt or a construction of controls.

Reconsider a GPU surface only if warm scrolling of cached rows is slow and a
profile shows that drawing dominates. A production replacement of the list
is about 3,000–6,000 lines; a rendering-only experiment is about 500–1,000.

## Remaining work

Main holds the code for #98's Settings part, #89, #90, #92, #95 and #96,
merged from `synosoft/responsiveness` without a build. Its rules are in
DESIGN.md ("Rows on demand", "Settings dialog", "The Include page",
"Copy") and `fast-copy-delete-spec.md` (D19). It is done after one green
`check.ps1` and a check in the running window:
Settings opens and shows every page; Include's Apply asks for permission
while the window keeps working; Indexed folders opens `C:\Windows`; Paste
copies and moves; a scroll bar jump in a long list shows its rows.

| Rank | Slow step | Issue | Measured | Fix | Needs |
| --- | --- | --- | --- | --- | --- |
| 1 | `MainWindow::open` builds the main window's controls before the window shows. | #98 | 1,700–1,930 million cycles on the window thread before the window shows, in one build on a loaded processor: `Controls::create` with the fonts 500–560, `show_filters` 230–280, fonts and icons 170–190, the File operations window 140–170, the title and menu bar 130–160, the show 110, columns and images 100 | Not decided | One run with timers inside `Controls::create` and `show_filters`; the File operations window is made on first use by the UI plan's session |
| 2 | `TaskbarProgress::show` makes two COM calls to the taskbar every 500 ms, also when nothing changed. | #93 | Not measured | Send only changes, and send again after `TaskbarButtonCreated` | The UI plan's session does it |

## Known remaining costs

- A 2,000-character Cyrillic path with a path term costs about 49 ms a
  paint, three frames.
- An operation of 100,000 sources pauses the window once, for 272 ms, when
  it appears, while the window computes its common folder.
- `DRAG_LIMIT` (1,000) no longer has a measured reason: naming 1,000 files
  for a drag takes 9.6 ms. The owner decides its value.
