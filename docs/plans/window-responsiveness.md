# Window responsiveness

The window does avoidable work on its message thread: quadratic text work
while painting, repeated work on every paint, every reply and every
progress tick, and filesystem, shell and process waits inside command
handlers. This plan removes that work in the existing Win32 window. It does
not change the renderer. Under the owner ruling "the window never lags"
(`AGENTS.md`), each wait found here is a defect, and each one is filed as
an issue (#85–#98); the rows marked "new" still need one.

## Schedule

**Not scheduled.** The owner decides when. Stage 3 builds on pages that
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
- Most of the waits below are not drawing. A faster renderer does not
  shorten a filesystem call, a shell call, a prompt or a quadratic loop.

Reconsider a GPU surface only if, after stages 1 and 2, warm scrolling of
cached rows is still slow and a profile shows that drawing dominates. A
production replacement of the list is about 3,000–6,000 lines; a
rendering-only experiment is about 500–1,000.

## Findings

Each finding names the slow step and what it reads. Every elapsed time
here is suspected, not measured.

| Issue | Slow step | What it reads | Who notices |
| --- | --- | --- | --- |
| #85 | `Compared::new` folds every prefix of a non-ASCII text, so mapping one match is quadratic in the text's length. `draw_cell` runs it for the Name and the Path cell, and `column_fits` again. | The full path, when the query has path terms | Paths with non-ASCII names, searched by path |
| #86 | `Model::shown_row` calls `is_taken`, which encodes the full path before `Taken::contains` sees that the set is empty; both `fill_cell` (`LVN_GETDISPINFO`) and `draw_cell` call it for every cell. `set_taken` does the same for every selected file on each engine wake. Custom draw asks for every cell even when the query has no terms. `draw_text` encodes the text again for each measure and each draw. `apply` runs `update_inspector` for every change, `Change::Nothing` included, and `InspectorCaptions::new` sums every selected file through `Sums::of`, on each status reply and engine wake. | Each visible path, every paint; every selected file, every reply | Everyone on every scroll; more with a large selection |
| #93 | The 500 ms progress refresh calls `folder_of` through `card_ends` and, for every waiting row, `ends_of`, which converts every source path to compute a common folder that never changes. It runs while the File operations window is hidden. `TaskbarProgress::show` makes two COM calls each tick, also when nothing changed. | Every source path of each operation, twice a second | Anyone who searches during a large copy or delete |
| #88 | `MainWindow::search` calls `gozler_client::folder_is_missing` (`std::fs::metadata`) before each scoped search, also after a sort, F5, and at startup before the window shows. | The Location folder on disk | A Location on a sleeping disk or an offline network drive; at startup, a window that appears only after the wait |
| #91 | `cached_rows` clones rows until its first miss, so Select all can clone and drop up to 32,768 rows. Copy walks the data three times: `ClipboardData::len` in `ClipboardBlock::new`, the `assert_eq!` in `write_into`, and the write. `path_units` allocates a buffer per path. | Every selected row | Copy or Select all over many rows |
| #94 | `Results::accept` compiles the matcher, regex included, for every new search, also when only the sort changed. | The query | Regex queries |
| #87 | `shell::drag` calls `SHParseDisplayName` for each path, up to `DRAG_LIMIT` (1,000); DESIGN measured 1.1–2.1 ms per path. Which part of that is the disk was not measured. `Icons::index` calls `SHGetFileInfoW` once per extension for the life of the process; that is not a repeated wait. | The shell namespace and each entry | Drags of many items |
| new | Open, Explore and Open file location call `ShellExecuteW`, `ILCreateFromPathW` and `SHOpenFolderAndSelectItems` on the window thread (`shell::open_entry`, `explore`, `show_in_folder`). `present` before them reads at most `OPEN_LIMIT` (15) paths. | The shell and the target program | A target that answers slowly, such as one started through DDE |
| #89 | `IndexedFolders::list` enumerates a folder on the window thread, from expanding a folder, from `init` for every drive and from `reveal` for every level of a path; `subfolders` follows links with `is_dir`. Saving runs `start_as_administrator`, which returns only when the person answers the permission prompt, then waits up to 30 s for the helper. | Folders on disk, links to shares, the prompt | Include settings |
| #90 | `Queue::submit` rebuilds the taken set over every source under the queue lock, then `Shared::start` writes the whole `operations.json` before it starts the operation's thread. `Queue::cancel` rebuilds the taken set too, and writes the file when the operation was saved. | Every source path; the saved file | Submitting or cancelling a large operation |
| #92 | `requests::rows` collects all four 4,096-row pages of a 16,384-row chunk before it returns. gozlerd sorts the first offset + limit matches before its first page, then reads each page from the index, so publishing pages earlier saves the building, transfer and decoding of up to three pages. A chunk can come back short. | The pipe | Everyone, by an amount not measured |
| #95 | `next_chunk` refuses a new chunk while one is in flight, and `InFlight::Chunk` keeps only its generation. A scrollbar jump during a miss waits for the old chunk. | The pipe | Scrollbar jumps in large results |
| #96 | `ClipboardBlock::set` sleeps 4 × 20 ms when another program holds the clipboard; DESIGN and the comment say 100 ms. Paste calls `OleGetClipboard` and `GetData` on the window thread. | Another program | Rare |
| #97 | `Settings::save` writes and renames `ui.json` after Apply, a language change, the tray notice, Exit and session end. | One small file | Rare; a slow `--data-dir` |
| #98 | `MainWindow::open` builds the File operations window before the main window shows; `SettingsDialog::init` builds and fills every page. | Nothing on disk except `operations.json` | Startup and opening Settings |
| new | `run_fix` for `Fix::UseLocal` calls `local_service::is_paired`, which reads `gozlerd.exe` and computes its SHA-256 on the window thread. | The executable | Rare |
| new | `fill_cell` formats Size with `GetNumberFormatEx` and Date modified with `GetDateFormatEx` and `GetTimeFormatEx` on every `LVN_GETDISPINFO`, because the list is virtual. | Nothing | Everyone on every scroll; cost not known |

## Work, ranked by return

Effort includes one green `check.ps1`. Lines are code lines added or
rewritten, without tests. The gain is the share of that operation's
window-thread time the fix removes; every figure is an estimate.

| Rank | Fix | Issue | Effort | Lines | Estimated gain | Needs |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `is_taken` returns before encoding when the taken set is empty, which covers `fill_cell`, `draw_cell` and `set_taken`; answer `DrawAnswer::Default` at the paint stage when `MatchedParts` has no terms, through a new method on it; encode once for measure and draw; skip `update_inspector` and the bound controls for `Change::Nothing` | #86 | 3–5 h | 50–100 | Removes the work per selected file on every reply and wake (100,000 steps for a 100,000-row selection); the share of repaint time is not known until the locale calls are measured | The issue |
| 2 | Keep each operation's common folder in the window, per `OperationId`, computed once, for the card and for every row; send taskbar progress only when it changed | #93 | 3–5 h | 50–100 | About 100% of that cost | The issue |
| 3 | Run `folder_is_missing` on the Results request thread, keeping `gozler-client` as its only owner. Exit cancels the thread but does not join it while that call runs, so it does not query after the call returns; it wakes the window only while the window is open, as the update check does (window rule 5), because `std::fs::metadata` has no cancel point and Exit's join is bounded today. A missing folder gets its own reply that shows the missing-folder message without a retry poll. | #88 | 0.5–1 day | 60–120 | 0 ms locally; removes a freeze of seconds on a sleeping or offline drive, at startup too | The issue; update DESIGN's "Shared code" row and its "Requests: Exit" sentence |
| 4 | Make the second `len()` a `debug_assert`; encode without a buffer per path; check cache coverage before cloning | #91 | 2–4 h | 30–60 | About 30–60% of a large Copy | The issue |
| 5 | Reuse the compiled matcher when the parsed query and Match case are unchanged | #94 | 2–3 h | 20–50 | Microseconds for words, milliseconds for regex | The issue |
| 6 | Map only the two ends of each matched range back to the source, instead of every boundary | #85 | 1 day | 40–100 | Over 95% per cell for long non-ASCII paths; 0% for ASCII | The issue |
| 7 | Build each dragged item's ID list from what the index holds, through a bind context with `STR_FILE_SYS_BIND_DATA`, so the shell does not read the entry from the disk | #87 | 1–2 days | 100–200 | Unknown until the drag is timed before and after | The `windows` crate's `implement` feature, which the workspace does not enable, for the `IFileSystemBindData` object; a decision on DESIGN "A missing entry", because every path then parses and Explorer, not the window, reports a deleted file |
| 8 | Save at the start of the operation's own thread, before its first file change | #90 | 1 day | 60–120 | Tens to hundreds of ms per large submit | A short plan: today `Queue::close` relies on "every operation that runs is already in the saved file", and the Exit question tells the person they can continue it. With the save on the thread, an Exit between start and save loses an operation shown as running. Either `close` waits for threads that have not saved yet, or D19 and the Exit text accept that loss. The taken-set rebuild stays and is measured. |
| 9 | Enumerate Include folders on a worker, `reveal` included; save on its own thread and channel, with the dialog waiting for the answer and staying open when the prompt is refused | #89 | 2–3 days | 200–400 | 100 ms or more per large folder or link; after the prompt, the helper's start and registry write. With UAC's secure desktop, the window may not show while the prompt does. | A short note in the issue. Not the main window's fix slot, because one fix runs at a time and a running Update would block the save. |
| 10 | Run Open, Explore and Open file location on a worker that initialises COM as STA itself, through `ShellExecuteExW` with `SEE_MASK_NOASYNC` and `SEE_MASK_FLAG_NO_UI`; the error message and `report_missing` come back as a reply | new | 1–2 days | 100–200 | Depends on the target program | A new issue |
| 11 | Publish the first page early; retarget a request whose range left the viewport | #92, #95 | 3–5 days | 250–500 | Not known; measure the time to the first page against the time to the whole chunk first | A plan for stage 3: partial chunks, failures and the selection |
| 12 | Clipboard retry on a timer; Paste on a COM worker | #96 | 1–2 days | 80–150 | Up to 80 ms, only under contention | Measure first |
| 13 | Create Settings pages and the File operations window on first use | #98 | 1–2 days | 200–400 | Perhaps 10–50 ms at startup | Measure first, then a short plan: Settings search reads every page's controls |
| 14 | Save settings from a background owner | #97 | 1 day | 80–150 | 1–5 ms locally | Measure first; Exit and session end must still write before the process ends |
| 15 | Hash `gozlerd.exe` off the window thread | new | 2–3 h | 20–40 | Tens of ms, rare | A new issue |
| 16 | Cache each row's formatted Size and Date modified | new | 2–3 h | 20–40 | Not known | Measure first, beside rank 1 |

## Stages

One implementer does the stages and fixes the review's findings. Each stage
ends with `check.ps1` green and a commit.

| Stage | Change | Proof |
| --- | --- | --- |
| 1 | Ranks 1–6 | #85: a proptest that the new mapping equals the current one over random non-ASCII text, including combining marks, and a criterion bench of `MatchedParts::ranges` on a long non-ASCII path, run before and after. #88: a unit test that a missing folder gives the missing-folder message and no retry. The rest remove repeated work that the existing tests cover. |
| 2 | Ranks 7, 9, 10 and 15; rank 8 once its short plan is decided | #87: one run that times a drag of 1,000 items into Explorer before and after, and checks that files and folders copy. If the bind-context ID lists do not drag, report it, and keep the current wait until the owner decides. #90: the queue test that submits, waits for Running and drops the queue still finds one saved operation, so the thread saves also when the queue is closing. |
| 3 | Rank 11 | Unit tests of the chunk cache with partial chunks and a retargeted range |

## Measured

Release builds on the owner's laptop, median of 15 runs unless one run is
named, on synthetic rows; the tests are `crates/gozler-ui/src/timing.rs` on
branch `window-timing`. A paint is 40 rows. Each fix below was built only
because its cost was measured; the rest stay unbuilt for the reason given.

**Done** (stages 1 and 2, commit `da51c90`):

| Rank | Before | After |
| --- | --- | --- |
| 6 (#85) | Path term over a 110-character Cyrillic path: 33 ms a paint; 2,000 characters: 11.9 s | 1.5 ms; 49 ms |
| 1 (#86), the empty-taken check only | `set_taken` over 100,000 selected files: 44 ms a wake | 0.05 ms |
| 2 (#93), the cached folder only | 221 ms every 500 ms at 100,000 sources | 272 ms once, when the operation appears |
| 4 (#91) | Copy of 100,000 paths: 162–194 ms | 59 ms |
| 7 (#87) | Naming 1,000 files for a drag: 685–1,130 ms | 9.6 ms; the shell's copy engine copied 990 files and 10 folders named this way intact |

**Disproved:**

| Rank | Measured |
| --- | --- |
| 1, the rest | Inspector sums of 100,000 rows: 0.5 ms; a paint's custom draw and refreshes were not material |
| 2, the taskbar part | Not measured; it left progress blank after a taskbar button was recreated, so it was reverted |
| 3 (#88) | `metadata` of a local folder and of an unreachable share: 0.07 ms each |
| 5 (#94) | Compiling two words: 0.001 ms; a regex: 0.75 ms |
| 9 (#89) | Listing subfolders of `C:\`, Program Files, Windows, System32 and a profile: 0.1–3.4 ms; only `C:\Windows\WinSxS` takes 727 ms |
| 10 | Starting a program through `ShellExecuteW`: 30 ms |
| 12 (#96) | The Copy retry under contention: four 20 ms sleeps, 82 ms |
| 14 (#97) | Saving `ui.json`: 2 ms |
| 15 | Hashing `gozlerd.exe`: 19 ms, one run |
| 16 | Size and Date cells of a paint: 0.08 ms |

**Proven, needs a plan:** rank 13 (#98). One run: `MainWindow::open`
takes 627 ms before the window shows, 75 ms of it the File operations
window, and opening Settings takes 325 ms in `SettingsDialog::init`. Ranks
8 and 11 still need their plans.

**Remaining costs of the done fixes:** a 2,000-character Cyrillic path
still costs about three frames a paint, and an operation of 100,000
sources pauses the window once for 272 ms. `DRAG_LIMIT` no longer has a
measured reason; the owner decides its value.
