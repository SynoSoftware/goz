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

**Not scheduled.** The owner decides when. Rank 3 builds on pages that
gozlerd reads from the index one at a time, as "Rows on demand" in
[DESIGN.md](../DESIGN.md) describes.

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

Effort includes one green `check.ps1`. Lines are code lines added or
rewritten, without tests.

| Rank | Slow step | Issue | Measured | Fix | Effort | Needs |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `MainWindow::open` builds the File operations window before the main window shows; `SettingsDialog::init` builds and fills every page. | #98 | One run: `MainWindow::open` takes 627 ms before the window shows, 75 ms of it the File operations window; opening Settings takes 325 ms in `SettingsDialog::init`. Where the other 550 ms of `open` goes was not measured. | Create Settings pages and the File operations window on first use | 1–2 days | Measure the rest of `open` first, then a short plan: Settings search reads every page's controls |
| 2 | `Queue::submit` rebuilds the taken set over every source under the queue lock, then `Shared::start` writes the whole `operations.json` before it starts the operation's thread. `Queue::cancel` rebuilds the taken set too, and writes the file when the operation was saved. | #90 | Not measured; suspected tens to hundreds of ms per large submit | Save at the start of the operation's own thread, before its first file change | 1 day | A short plan: today `Queue::close` relies on "every operation that runs is already in the saved file", and the Exit question tells the person they can continue it. With the save on the thread, an Exit between start and save loses an operation shown as running. Either `close` waits for threads that have not saved yet, or D19 and the Exit text accept that loss. The taken-set rebuild stays and is measured. |
| 3 | `requests::rows` collects all four 4,096-row pages of a 16,384-row chunk before it returns, and `next_chunk` refuses a new chunk while one is in flight, so a scrollbar jump during a miss waits for the old chunk. | #92, #95 | Not measured | Publish the first page early; retarget a request whose range left the viewport | 3–5 days | Measure the time to the first page against the time to the whole chunk first; then a plan for partial chunks, failures and the selection |
| 4 | Saving Include settings runs `start_as_administrator`, which returns only when the person answers the permission prompt, then waits up to 30 s for the helper. Listing `C:\Windows\WinSxS` takes 727 ms. | #89 | The listing of other large folders took 0.1–3.4 ms, so only WinSxS is slow; the prompt and helper wait was not measured | Save on its own thread and channel, with the dialog waiting for the answer and staying open when the prompt is refused; list WinSxS-sized folders on a worker | 1–2 days | With UAC's secure desktop, the window may not show while the prompt does. Not the main window's fix slot, because one fix runs at a time and a running Update would block the save. |
| 5 | `TaskbarProgress::show` makes two COM calls to the taskbar every 500 ms, also when nothing changed. | #93 | Not measured. Sending only changes left the progress blank after the taskbar button was recreated, so it was reverted. | Send only changes, and send again after `TaskbarButtonCreated` | 2–3 h | Measure first |
| 6 | Paste calls `OleGetClipboard` and `GetData` on the window thread, so the program that owns the clipboard decides how long it takes. | #96 | Not measured. The Copy retry under contention was measured at 82 ms and stays. | Paste on a COM worker | 1 day | Measure first |

## Known remaining costs

- A 2,000-character Cyrillic path with a path term costs about 49 ms a
  paint, three frames.
- An operation of 100,000 sources pauses the window once, for 272 ms, when
  it appears, while the window computes its common folder.
- `DRAG_LIMIT` (1,000) no longer has a measured reason: naming 1,000 files
  for a drag takes 9.6 ms. The owner decides its value.
