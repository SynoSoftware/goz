# Window painting: one presenter

## Schedule and owner decision

On 2026-10-27 the owner asked for one owner of the main window's painting,
built now: "a single owner needs to control drawing and need to draw it
optimal". This replaces the 2026-10-25 decision to stop painting work.

Stage 2 shows a copy of the window's pixels over the window. The test
`win::painting` bans `BitBlt` outside its allow-list, and "Windows draws the
window" in AGENTS.md needs the owner's direct permission for it. Stage 1
needs no permission. It ends with the recordings that the owner needs for
that decision. The owner gave stage 2's permission on 2026-10-28.

## Required outcome

The owner's goal, set on 2026-10-27:

> In a 60 fps recording of each of these interactions (mode switch both
> ways, Show in Commander, resize, splitter drag, typing in Search,
> scrolling, transfer queue appearing), the window goes from the old picture
> to the new one in one step. No frame shows part of a change. Each update
> marks only the controls or list rows it changed, and a paint trace shows
> each of them painted once. Each update's time is logged. An update over
> 16 ms still appears whole and gets an issue that names its slow step. If
> native controls cannot give one step, stop and report the recordings.

The scope is the main window, with its transfer pane. Dialogs and the
Viewer keep their own painting.

An update is everything that one message to the main window changes,
until its controls have painted. When that message posts `DEFERRED` to
finish an interrupted update, the update includes the `DEFERRED` message
too. A reply that arrives later, such as a folder listing after a mode
switch, is a new update.

## Why the window shows pieces now

1. **Each control paints into the window's surface on its own, and the
   screen shows that surface at every frame.** A list view with
   `LVS_EX_DOUBLEBUFFER` paints its whole client in one step. Several
   controls do not. A paint pass of 32 to 266 ms, as measured for a mode
   switch, therefore spans 2 to 16 frames, and each frame shows the
   controls painted so far. The recordings support this: under
   `WS_EX_COMPOSITED`, one frame still split a single control
   ([the native buffering evaluation](../research/native-window-buffering.md)).
   A pass of d ms crosses a frame boundary with a probability of about
   d / 16.7. So painting fewer controls, or painting them faster, makes
   pieces rarer, but it cannot remove them.
2. **More than one owner decides when pixels appear.**
   - `MainWindow::layout` paints at once on a mode change
     (`Hwnd::draw_now_after`). It runs inside `Effect::Layout`, before
     `refresh_areas`, the panels' start and the focus. Each of those then
     paints its control again, in later frames.
   - `FileOperations::show_view` paints the transfer pane at once
     (`draw_now_after` on the pane). `place_controls` calls it through
     `prepare_place`, so the pane paints at its old rectangle, then moves
     and paints again.
   - `Font::set_font` sends `WM_SETFONT` with redraw TRUE to about 80
     controls when the DPI changes, outside any batch.
   - The status bar paints a part on each `SB_SETTEXT`, and the whole bar
     on each `SB_SETPARTS`. One render sends `SB_SETPARTS` up to three
     times.
   - Some controls paint at once when a value is set: a progress bar on
     `PBM_SETPOS`, an edit on `WM_SETTEXT`, a combo box on `CB_SETCURSEL`.
3. **Batches repaint what did not change.** `Hwnd::redraw_after` turns
   drawing off on the main window, which clears `WS_VISIBLE` and drops the
   invalidations made meanwhile. It therefore invalidates the frame and
   every child afterwards (`RDW_FRAME | RDW_INVALIDATE | RDW_ALLCHILDREN`),
   so every control and the menu bar repaint, changed or not. The callers
   on the main window are:
   - `layout`;
   - the language and regional-format changes;
   - Settings OK and Apply (`app/main_window/settings.rs`).
   On a single list, the selection writes (`app/main_window/selection.rs`)
   use the same batch, so they repaint the whole list, not only the
   changed rows.

Measured in one release build on 2026-10-27, with six mode switches posted
as `WM_COMMAND`:
- `place_controls` took 12 to 42 ms;
- the paint pass took 32 to 266 ms;
- the whole update took 99 to 302 ms.
A per-window trace of one switch found:
- Search's list painting for 85.5 ms, with 332 `WM_NOTIFY` and 996
  `LVM_GETHEADER` calls;
- a panel list painting for 82.9 ms;
- the menu bar and frame repainting for about 10 ms.
The trace's hook adds its own cost, so these numbers rank the steps but do
not give their exact cost. Issue #154 holds the slow step.

## Design

### The presenter

`win::present` owns when the main window's pixels appear. It works in
three parts:
- **Start.** Each window procedure of the window's thread opens an update
  for each message it handles: `win::window::procedure`, which serves the
  main window, the transfer pane and the Viewer, and the dialog procedures
  in `win::dialog`. Settings OK and Apply and the transfer pane's own
  timer then run inside an update.
- **Change.** The handler only marks what it changes.
- **End.** When the outermost update ends, the presenter publishes what
  was marked in the main window.
One owner decides, so no caller has to remember to open an update. Every
entry point is covered, including `Wake`, `Timer` and operation progress.

1. **Start: which messages open an update.** Every message opens one,
   except the messages that Windows sends while it paints, hit-tests or
   activates, which change nothing:
   - `WM_PAINT`, `WM_ERASEBKGND`, `WM_NCPAINT`, `WM_SYNCPAINT`,
     `WM_PRINTCLIENT`, `WM_CTLCOLOR*`;
   - `WM_NCHITTEST`, `WM_SETCURSOR`, `WM_GETMINMAXINFO`, `WM_NCCALCSIZE`,
     `WM_NCACTIVATE`, `WM_WINDOWPOSCHANGING`;
   - the notifications a list sends during its paint: `NM_CUSTOMDRAW`,
     `LVN_GETDISPINFO`, `LVN_ODCACHEHINT`, `LVN_ODFINDITEM` and
     `LVN_GETEMPTYMARKUP`.

   The presenter keeps its own depth, not the watchdog's. A message that
   arrives while an update is open is part of it. The depth stays above
   zero until the presenter's own paint pass and the cover's cloak have
   finished, so a message that a control sends during that pass cannot
   publish again.
2. **Change: a change only marks.** A setter, a placement or a new row
   invalidates its control, or the changed rows of a list
   (`ListView::redraw_rows`). Windows already keeps each control's update
   region, so the marks need no new data structure.
   - **Setters.** Many native setters paint at once, inside the message,
     outside `WM_PAINT`, for example:
     - a static control on `WM_SETTEXT` and `STM_SETIMAGE`;
     - a button on `WM_SETTEXT` and `BM_SETCHECK`;
     - any control on `WM_ENABLE`;
     - the status bar on `SB_SETTEXT`, `SB_SETPARTS` and `SB_SETICON`;
     - a progress bar on `PBM_SETPOS`, `PBM_SETRANGE32` and
       `PBM_SETMARQUEE`;
     - a combo box on `CB_SETCURSEL`;
     - an edit on `EM_SETSEL`;
     - a toolbar on `TB_CHECKBUTTON` and `TB_ENABLEBUTTON`, and a header
       on `HDM_SETITEMW`, which may paint at once (not established);
     - a scroll bar on `SetScrollInfo` with redraw TRUE. The filter
       pane's `set_vertical_scroll` keeps redraw TRUE and runs through
       `mark_after`, which marks the pane's frame, where its scroll bar
       is. Scrolling the filters moves only the pane's children, so no
       placement marks that frame;
     - a list on `LVM_ENSUREVISIBLE`, `LVM_SETITEMCOUNT` and `LVM_SCROLL`,
       which move its scroll bar and copy its rows at once.

     The `win` wrappers that send these messages call
     `present::mark_after` themselves, so no caller can bypass it:
     - `Hwnd::set_text`, `set_enabled` and `focus`;
     - the image, check, progress, marquee, combo and status bar setters,
       the edit's selection, and the toolbar and header setters;
     - the list's `set_count`.

     A list's scroll (`ensure_visible`, the edge scroll, `LVM_SCROLL`) is
     the exception: the list copies its rows on the screen inside the
     message even while its drawing is off, so `present::scroll_now` runs
     it with drawing on and paints the list at once. With `mark_after`,
     the owner's 30 fps recording showed the copied rows for one or two
     frames before the rows they uncovered.

     `mark_after` turns drawing off on that one control, runs the
     setter, turns drawing on, and invalidates that control's client and
     frame, with `RDW_ALLCHILDREN`: turning drawing off drops the marks
     the children already had. Without it, the filter pane's scroll bar
     setter left the pane's labels and boxes unmarked after a switch to
     Search, and Commander's old pixels stayed between them. It
     skips a hidden control, as `Hwnd::redraw_after` does, because
     `WM_SETREDRAW` TRUE would show it. For the status bar,
     `mark_rect_after` marks only the part that changed: it saves the
     bar's update region before drawing goes off, puts it back after, and
     adds the part's rectangle. Marking the whole bar repainted every part
     for each `SB_SETTEXT`, and the parts flashed; dropping the saved
     region lost the marks of parts set earlier in the same update. The
     build test enforces the
     wrappers (see "What the build test catches").

     `mark_after` acts only on a child window inside the main window
     (`WS_CHILD`, and `GetAncestor(GA_ROOT)` is the main window). On any
     other window it runs the setter as it is, so:
     - dialogs and the Viewer keep their own painting, as the scope says;
     - a top-level window's title is set without a mark. DWM draws the
       caption, so the window's surface does not change. Search sets the
       main window's title on each keystroke.

     Two more cases mark less, because marking the whole control would
     repaint what did not change:
     - **A list's row into view.** `ensure_visible` first checks whether
       the row is already visible (`LVM_GETTOPINDEX`,
       `LVM_GETCOUNTPERPAGE`). If it is, it sends nothing, and the
       selection change marks only its rows. If it is not, the list
       scrolls and paints at once (`scroll_now`).
     - **Focus on a list.** `focus` uses `mark_after` on the control that
       loses the focus and on the one that gets it, because a button and
       other controls paint their focus cue in the message. For a list,
       it does not: the list marks only its focused and selected rows
       itself.

     While `mark_after` has drawing off on a control, that control has
     no `WS_VISIBLE`. Whether UI Automation still announces focus on it
     is not established. Stage 1 checks it once with a UI Automation
     focus listener. If the announcement is lost, `focus` marks only the
     control that loses the focus, and the new control's focus cue paints
     at once.
   - **Placements.** `win::place_children` and `Hwnd::set_visible` use
     `SWP_NOREDRAW` and keep `SWP_NOCOPYBITS`. The `SWP_NOREDRAW`
     documentation requires the caller to invalidate what needs
     redrawing, so:
     - each moved, resized or shown child gets `RedrawWindow` with
       `RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN`, which
       reaches its frame and its own children;
     - each moved or hidden child's old rectangle is invalidated in its
       parent.
     The erase then waits for `BeginPaint`.
   - **Fonts.** `Font::set_font` sends `WM_SETFONT` with redraw FALSE and
     invalidates the control.
   - **Selection writes.** These mark the changed rows (`redraw_rows`)
     instead of the whole list.
3. **End: publish once.** When the outermost update ends, the presenter
   first asks `GetQueueStatus(QS_PAINT)` whether any window of the thread
   needs painting. If none does, it stops there. Otherwise it counts the
   windows in the main window with a non-empty update region:
   - the main window's client;
   - every descendant, through `EnumChildWindows`, with `GetUpdateRect`.

   Then it paints the marked regions in one pass, with one
   `RedrawWindow(RDW_UPDATENOW | RDW_ALLCHILDREN)`, without
   `RDW_INVALIDATE` or `RDW_FRAME`, so a window that did not change does
   not paint, and the frame and menu bar do not paint. The pass runs when
   it is due:
   - nothing waits in the queue (`QS_INPUT` or `QS_POSTMESSAGE`);
   - or the oldest change that no pass has shown is a frame, 1/60 s, old;
   - or the cover went up during the changes;
   - or a `PRESENT` arrived.

   Otherwise the marks wait, and the presenter posts `PRESENT` (`WAKE +
   11`), one at a time, whose update paints them. Input and posted messages
   go before `WM_PAINT`, so marks left to `WM_PAINT` waited for seconds
   behind a stream of search results, and the results showed late. Under
   load, keys still go first, and the marks of several updates paint in
   one pass. Messages to a native control, such as the mouse over a list,
   end no update, which is why a waiting mark needs `PRESENT` and cannot
   wait for the next update.

   The pass raises the cover when two or more windows are marked. One
   window paints in one step without it: a list is double-buffered, and
   the other controls paint in well under a frame.

   A request thread's results wake the window with one posted message for
   each purpose until the window takes it (`OpenWaker::deliver`). Each
   result used to post its own, and those messages, which go before input
   and paint, kept both waiting.

   These rules decide where the outermost update ends:
   - **`DEFERRED`.** If the update posted `DEFERRED` to finish an
     interrupted update, the update ends in the `Deferred` handler
     instead, after `apply_pending` and before `run_modal_steps`. If
     `apply_pending` posts `DEFERRED` again, the update extends to that
     one too. Windows delivers a posted message before input and before
     `WM_PAINT`. A reply that was posted before `DEFERRED` arrives first,
     so it becomes part of the update. If the post fails, the update ends
     at once, so `Hwnd::post` returns whether it posted.
   - **Modal loops.** The presenter keeps a stack of levels. Each level
     holds the depth of the message that runs its loop. For the loops
     that `DefWindowProc` runs, that is the message whose `DefWindowProc`
     call runs the loop, one below the `WM_ENTER*` message that pushes
     the level. An update is
     outermost when the depth is one above the top level's depth. Each
     call records, when it starts, whether it opened an outermost update,
     and only that same call runs the end check. So the `ENTER` and `EXIT`
     messages, which push or pop a level while they run, end nothing they
     did not open.
     - When a loop starts, the presenter ends the open update, then
       pushes a level. Messages inside the loop open their own updates,
       so Settings Apply publishes while Settings stays open.
     - When the loop ends, the presenter pops the level and opens a new
       update, timed from the pop. It holds what the outer handler changes
       after the loop, such as the command a popup menu returned.
     - Gozler's own loops push and pop through a guard around the loop,
       in `win::dialog`, `win::menu` and `win::shell`: dialogs, message
       boxes, the file dialog, popup menus and drag-and-drop.
     - The loops that `DefWindowProc` runs push at `WM_ENTERMENULOOP` and
       `WM_ENTERSIZEMOVE`, and pop at the matching `WM_EXIT*`. These
       messages push only when no `win::menu` guard is active, because a
       popup menu that Gozler shows has its own guard. So each loop has
       one trigger, including the window menu that Windows shows from the
       title bar. This covers the menu bar, `open_menu_bar`'s
       `SC_KEYMENU` and dragging the frame, so each `WM_SIZE` during a drag
       is an update of its own.
   - **`WM_DESTROY`** of the main window ends the update.
4. **Measure and log each update.** For each update that marked something,
   the presenter records:
   - its start and end, as `QueryPerformanceCounter` times;
   - the window thread's cycles, from `gozler_winfs::thread_cycles`;
   - the message that opened it;
   - the windows it found marked, with their rectangles;
   - whether it published at once, and whether the cover showed;
   - the time of each step: the change, the cover and the paint pass.

   It hands the record to a bounded channel whose send never waits. When
   the channel is full, it counts the record as dropped. The watchdog
   thread, which already writes `ui-slow.log`, drains the channel in its
   100 ms cycle. It appends the records to `ui-frames.log` beside it in
   one write per cycle, with the count of dropped records. The log is
   capped by `retain_log`, as `ui-slow.log` is. The window's thread never
   writes the file.

### The cover (stage 2, needs the owner's permission)

A pass of several controls that takes longer than a frame can appear in one
step only if the screen keeps the old picture until the pass ends.
Rejected mechanisms rules out each native way that the evidence covers:
- `WS_EX_COMPOSITED`;
- turning drawing off for the update;
- `SWP_NOREDRAW` with restoration.
`LVS_EX_DOUBLEBUFFER` buffers one list only. A child window cannot hold the
old picture either:
- a transparent child paints after its siblings;
- a hidden container's children paint one by one when it shows;
- cloaking applies to a top-level window only.
So the old picture has to be in a top-level window of its own.

The cover is an owned popup that holds a copy of the main window's client
area. Gozler creates it once, shows it, and cloaks it with `DWMWA_CLOAK`, so
it costs nothing while unused. Its content is a native static control
(`SS_BITMAP`). DWM transitions are off for it
(`DWMWA_TRANSITIONS_FORCEDISABLED`). It answers `WM_NCHITTEST` with
`HTTRANSPARENT`.

The presenter raises the cover when an update ends with two or more
windows marked. In this order:
1. Copy the client rectangle from the main window's DC (`BitBlt`). The
   surface still holds the old picture, because every change only marked.
   The copy has no scroll bar tracks: Windows 11 draws a list's track
   above the window's surface, and a probe on 2026-10-28 read white there
   from the window's DC and the track's grey from the screen. A cover
   with that copy showed each list's border and no track, and the tracks
   flashed on every covered update, also while typing a search. So the
   cover has a hole over each shown scroll bar, out to its control's outer
   edge, and the real scroll bars show through it. A copy from the screen
   DC still showed no track under the cover, and `PrintWindow` with
   `PW_RENDERFULLCONTENT` has the track but took 90 to 112 ms, so both
   were rejected on 2026-10-28.
   The cover's shape also follows the main window's rounded bottom
   corners, 8 px at 96 DPI, and Windows' own rounding of the cover is off
   (`DWMWCP_DONOTROUND`). Windows 11 rounds a popup's corners and then
   draws a border and a shadow around it, which showed as a line under the
   menu bar each time the cover went up.
2. Give the copy to the static control, and paint the cover now.
3. Place the cover directly above the main window, on each use, with
   `SetWindowPos` after the window that is above the main window
   (`GW_HWNDPREV`) when that window is in the same band, topmost or not,
   and otherwise at the top of the main window's band (`HWND_TOPMOST` or
   `HWND_NOTOPMOST`). So a window above the main window, such as an open
   Settings dialog, stays above the cover, and the cover never pulls its
   owner out of "always on top". Then uncloak it.
4. Run the paint pass. Each control still paints itself, under the cover.
5. Call `GdiFlush`, so the surface is finished.
6. Cloak the cover.

A layout batch inside the main window raises the cover before it moves or
shows controls (`present::cover_now`), and the update's paint pass then
runs under that cover. Inside `EndDeferWindowPos` a combo box that moves or
shows calls `UpdateWindow` on itself, and a list view repaints its header
when its drawing comes back on, so these controls paint before the update
ends.

The cover is skipped while the main window is hidden or minimized. It is
hidden together with the main window, so the watchdog, which counts
visible windows, does not count it while Gozler sits in the notification
area.

The cover covers the client area only, because the frame and the caption
are drawn by DWM and are not in the window's DC. A mode switch keeps the
menu titles, so `replace_menu_bar` replaces only the submenus and the menu
bar does not repaint.

Gozler draws nothing of its own: the picture is the window as Windows
painted it, in every theme, high contrast and DPI. The window's thread is
busy during the update anyway, so only the cursor shape is affected while
the cover shows.

### What the build test catches

The test `win::painting` gains a second list, with the same rule as its
first: a token may stand only in the functions the list names.
- `RDW_UPDATENOW` and `UpdateWindow(` may stand only in `win::present`.
- `WM_SETREDRAW` may stand only in `win::present`, which holds
  `mark_after`, and in `Hwnd::redraw_after`, which dialogs and lists'
  column widths use.
- The setters that paint at once may stand only in the `win` wrappers
  that call `mark_after`: `SetWindowTextW`, `EnableWindow`, `SetFocus`,
  `STM_SETIMAGE`, `BM_SETCHECK`, `PBM_SETPOS`, `PBM_SETRANGE32`,
  `PBM_SETMARQUEE`, `CB_SETCURSEL`, `EM_SETSEL`, `SB_SETTEXTW`,
  `SB_SETPARTS`, `SB_SETICON`, `TB_CHECKBUTTON`, `TB_ENABLEBUTTON`,
  `HDM_SETITEMW`, `SetScrollInfo(`, `LVM_ENSUREVISIBLE`,
  `LVM_SETITEMCOUNT` and `LVM_SCROLL`. The Viewer's own functions that
  call `SetScrollInfo` and `SetFocus` (`win::viewer`) are allowed too,
  because the Viewer keeps its own painting.
So a new forced paint, or a setter that paints at once outside its
wrapper, fails the build. A native message that paints at once and is not
on this list is found by the paint trace and the recording, and then
joins the list.

### Speed

The cover makes a slow update whole, not fast. Each update over 16 ms in
`ui-frames.log` gets an issue that names its slow step, or a comment on
the issue that already names it, such as #154. Known costs:
- the 996 `LVM_GETHEADER` calls and 332 `WM_NOTIFY` answers in one paint of
  Search's list (#148 also covers `Icons::index` in `fill_cell` and
  `draw_cell`);
- the panels reading their folders again on each mode switch;
- `follow_dates` repainting every visible row on each `WM_SIZE` in the
  Relative date format;
- the cover's own copy on each splitter step, which the frame log
  measures;
- each transfer progress step, about twice a second. It marks the
  progress bar and a status bar part, which are two windows, so it
  publishes at once, under the cover in stage 2. If the frame log shows
  that this costs too much, the count can treat a parent and its own
  children as one window.

## Stages

1. **Presenter without the cover.** This stage builds:
   - the presenter's start, its levels and its end;
   - the marks, with `mark_after` inside the `win` wrappers;
   - the removal of `draw_now_after` and of the main window's
     `draw_once_after` callers;
   - the frame log;
   - the build test.
   Each update then publishes once, but a pass longer than a frame can
   still show pieces. Proof:
   - the paint trace shows each marked control painted once and each
     unmarked control not painted;
   - `ui-frames.log` has a line for each update;
   - a UI Automation focus listener receives the focus moves, once;
   - one recording of each listed interaction.
   These recordings answer the goal's last sentence: they show whether
   native controls give one step without the cover. They go to the owner
   with the request for stage 2's permission.
2. **The cover**, after the owner's permission. It starts with one run that
   uncloaks and cloaks the cover with no change in the window, and requires
   zero changed frames. That run shows that the copy holds the child
   controls. Whether a `DwmFlush` is needed after the uncloak, at a cost
   of up to one frame, is decided from the recordings of the listed
   interactions: without it first, and with it if a recording shows
   pieces. Proof: the recordings and the paint trace again.
3. **Speed**, in the order of the frame log's slowest steps. Proof: matched
   cycle counts before and after, from `ui-frames.log`.

One implementer keeps its context through the stages. Each stage builds
once, through `build.ps1 -Release`, and the full `build.ps1` runs before a
commit.

## Verification

- **Recording.** ffmpeg `ddagrab` (Desktop Duplication), which delivers
  each frame that DWM composes, at 60 fps over the window's monitor area.
  The window is placed topmost without activation. The interactions are
  posted to the window, never sent as keystrokes:
  - the mode switch as `WM_COMMAND`;
  - Show in Commander as its command;
  - typing as `WM_CHAR` to the search box;
  - scrolling as `WM_VSCROLL`;
  - resize as `SetWindowPos`;
  - the splitter drag as the splitter's mouse messages;
  - the transfer queue as a copy of a test folder in the data directory.
- **Pass criterion.** The first posted message gives one known event. It
  aligns the recording's frame times with the `QueryPerformanceCounter`
  times in `ui-frames.log`. For each update, the frames are judged inside
  the rectangles it marked: each frame equals either the picture before
  the update or the picture at its end. Changes outside every marked
  rectangle are left out, such as a hover. The caret's rectangle is left
  out too, because the caret blinks on its own timer. `ui-frames.log` is
  copied after each interaction, because a drag can fill it.
- **Paint trace.** A temporary hook pair, `WH_GETMESSAGE` for dispatched
  `WM_PAINT` and `WH_CALLWNDPROC` for sent paints, counts the paints of
  each window during one update. The frame log holds the rectangles the
  presenter found marked. The hooks are removed afterwards and never
  committed. A control that paints inside a setter, outside `WM_PAINT`,
  is invisible to the trace. The build test's setter list covers those,
  and the recording shows any it misses.

## Risks

- **Resize.** Windows resizes the frame inside `SetWindowPos`, before
  `WM_SIZE` arrives. So DWM can show the new frame with the old client for
  a frame before Gozler can act. This is the case that the goal's last
  sentence names. The presenter still makes each size step's client
  content appear in one step. The report gives the resize recording.
- **Scrolling.** The list handles a scroll message itself, outside the
  presenter: it moves its scroll bar and copies its rows at once, then
  paints the new rows double-buffered. A frame can show the new scroll bar
  with the old rows. If the scrolling recording shows that, it is the
  goal's last sentence for scrolling, unless the list's scroll is
  subclassed into the presenter, which the report then proposes.
- **A native message that paints at once and is not on the setter
  list.** For example, a toolbar can redraw a button on `TB_SETSTATE`.
  The recording shows it, and the message then joins the list.
- **`WM_SETREDRAW` on single controls.** These are not established:
  - that an edit and a combo box stop painting while their drawing is
    off;
  - that `SetFocus` behaves the same while drawing is off on the two
    controls;
  - that a list's scroll and scroll bar stop painting while the list's
    own redraw flag is off;
  - that `DefWindowProc`, answering `WM_SETREDRAW` TRUE, does not itself
    invalidate the control's children. If it does, a status text change
    marks the transfer controls as well. If, the other way round, it
    does not restore the children's marks that `WM_SETREDRAW` FALSE
    cleared, a child inside a moved status bar keeps stale pixels.
  The paint trace and the frame log's marked windows show each of these.
- **Windows' own erase.** `WM_SYNCPAINT`, which Windows sends after
  another window moves over Gozler, erases at once. The recordings post
  their interactions, so it does not occur in them.
- **Frame-only marks.** `GetUpdateRect` reports a window's client only.
  An update that marks one client and another window's frame alone counts
  as one window and paints at the natural `WM_PAINT`, in two writes. The
  wrappers mark client and frame together, so this needs a native frame
  change alone.
- **The copy.** That a `BitBlt` from the main window's DC copies its child
  controls, which share its surface, is not established. Stage 2's first
  run shows it.
- **Z-order.** The cover sits directly above the main window, so another
  program's window or a Gozler dialog above it stays above.

## Current evidence

Stage 1 is built (uncommitted, release build on 2026-10-28). Its frame
log and recordings answered the goal's last sentence for the mode switch:
without the cover, native controls do not give one step. Six posted mode
switches each published once (`published=1`, 21 to 26 windows marked),
yet the 60 fps `ddagrab` recording shows each switch in pieces:
- Commander to Search showed Search's toolbar and filter pane over the
  panels' rows for one frame;
- Search to Commander showed the left panel, then the right panel with a
  half-changed key bar, over three frames.

The paint pass took 34 to 104 ms. The other interactions:
- Show in Commander is in pieces, like the mode switch: over four frames,
  the toolbar, then the left panel, then the right panel replace Search.
- Resize showed the toolbar laid out for the new width over the old list.
  Each size step painted for 87 to 102 ms.
- The splitter drag is in pieces: torn rows and missing inspector buttons.
  Each move marks the results list, the filter pane and the inspector, and
  paints for 29 to 82 ms.
- Typing: the title bar shows a keystroke one frame before the search box
  does, and the results' rows and status row arrive in different frames.
- Scrolling animates whole frames while the rows are cached. Once it
  stops animating, the top row paints a frame before the others. The list
  handles the scroll itself, so no update is logged.
- The transfer queue appears whole in one frame. In that frame the Copy
  dialog goes blank, then fades out empty.
In one splitter run the filter pane's labels and buttons never painted and
showed stale pixels, before and after the drag. Other runs of the same
build painted them. Its cause is not found.

The recordings and logs are in the session scratchpad, `s1-*`. The
measured times are on #154.

The retained implementation batches geometry and visibility per native
parent, skips unchanged setters and uses native clipping and ListView
buffering. Changed child rectangles use `SWP_NOCOPYBITS`.

Earlier recordings showed partial Search toolbar and underline,
panel-count, Inspector, and mode or transfer publication. Their media and
native reports are under `V:/temp/gozler-window-paint-cause/`. The
remaining-source audit is
`V:/temp/gozler-window-remaining-paint-source-audit.md`. One Search-only
attribution check reached its image cap and was inconclusive. Its report is
`V:/temp/gozler-completion-paint-cause.md`.

Stage 2's cover is built (uncommitted, 2026-10-28) and waits for its
first run and recordings. It differs from the design above in three ways:
- The cover is a top-level static control (`SS_BITMAP`), hidden and
  cloaked between passes, so it needs no class of its own, and it does
  not need hiding with the main window.
- It skips a pass after the client's size changed, because the part of
  the surface that the window gained holds nothing yet.
- No `DwmFlush` precedes the cloak. With it, `ui-slow.log` showed messages
  blocked for 200 to 877 ms with almost no CPU: the changes lie under the
  opaque cover, so DWM presented nothing until something else on the
  screen changed. At start, the window then showed empty outlines for
  about two seconds.
The panel status line and key bar changed one update after the rows on
a switch to Commander: the panel reads its list's selection on a posted
`DEFERRED`. `Panels::list_changed` now extends the update to that
message.

A run on 2026-10-28 recorded the start and six mode switches at 60 fps,
with the frame log's new `cover=` and `early=` fields:
- With the cover raised only at the end of the update, the switch to
  Search still appeared over about ten frames. `early=` listed 13
  controls that painted during the changes, and a backtrace named
  `EndDeferWindowPos` (nine combo boxes), list column changes (three
  headers) and `SetFocus` (the search box). The cover then copied that
  half-built picture.
- With `present::cover_now` before the layout batch, every switch shows
  `cover=early` and appears in one step in the recording.
- The cover had sent Gozler behind other windows when Gozler was "always
  on top", because it was placed at the top of the normal windows. Step 3
  above now keeps it in the main window's band.
- After each switch to Commander, the panels' folder reads and folder sizes
  updated each list up to three times within about 0.4 s, and each update
  repainted the whole list, its header and its frame, even with nothing
  changed: `LVM_SETITEMCOUNT` with the same count, and the list disabled
  around selection writes that then finished at once. Two results that
  each changed one list also raised the cover, because the presenter
  counted the window the first had left for its `WM_PAINT`, and the cover
  showed the lists without their scroll bar tracks, so the tracks and the
  lists' borders flashed. Now an unchanged count redraws only the rows in
  view, the list is disabled only while writes wait for a later slice,
  only windows that the update itself marked raised the cover, and the
  cover leaves the scroll bars uncovered (step 1). The frame clock later
  replaced that cover rule: a pass covers when two or more windows are
  marked, whichever update marked them.

The run that followed, on 2026-10-28, measured where a mode switch's
time goes, from `ui-frames.log`'s `raise_us` and `redraw_us`:
- the controls' own paint pass (`redraw_us`) took about 38 ms at the
  median of a switch; our custom-draw and `LVN_GETDISPINFO` handlers took
  2 % to 30 % of that pass, median about 4.5 ms, and the rest is comctl32
  drawing the 24 to 50 controls that the switch shows, each of which must
  paint once when it appears;
- raising the cover took about 20 ms.

The paint costs are on #154 (comment of 2026-10-28), and making the paint
itself faster is left there.

## Rejected mechanisms

Reuse this evidence rather than measuring these again:

- Geometry and visibility batching is retained, but did not pass the
  all-frame proof.
- These still showed pieces:
  - root class redraw;
  - root preserved-region changes;
  - pending-paint flush;
  - `SWP_NOREDRAW` plus restoration;
  - toolbar buffering;
  - a fixture DWM-transition override on the main window.
  Structural parent redraw scopes also did not make independent child
  publication atomic. None of these was tested under a cover.
- Whole-window `WS_EX_COMPOSITED` was tested and rejected:
  - about five times the measured UI cycles overall;
  - a Commander mode resize over 200 ms;
  - a remaining toolbar counterexample.
  See [the native buffering evaluation](../research/native-window-buffering.md).
  Turning it on for one update only is a narrower form of the same
  mechanism, with the same toolbar counterexample.
- `WM_SETREDRAW` FALSE on the main window around the layout batch, or
  around a list's column changes, stopped the early paints, but clears the
  top-level window's `WS_VISIBLE`. In the one run with it, on 2026-10-28,
  Gozler had lost "always on top" at the end. Drawing stays on in the main
  window.
- Turning drawing off for the whole update of a mode switch (its layout,
  panels, texts and focus), then one `RDW_UPDATENOW`, was recorded on
  2026-10-27. Commander to Search still published the Search list over the
  panels' rows, across about three frames at 30 fps.

## Constraints

- Windows continues to draw every control.
- Keep the existing native controls, ownership, input, focus, selection and
  Work lifetimes.
- Keep expensive work off the window's thread.
- Claims of a speed-up need matched CPU-cycle evidence.
- Remove each temporary hook and each unsuccessful candidate. Stop an
  inconclusive experiment instead of treating it as a pass or retrying it
  automatically.

Large Include edits remain a separate, suspected and unmeasured UI cost in
[#153](https://github.com/SynoSoftware/goz/issues/153).
