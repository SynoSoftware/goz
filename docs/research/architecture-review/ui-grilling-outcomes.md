# Grilling outcomes (working notes)

## C1 File-operation lifecycle — rounds 1–2 settled (round 3 pending)
- Fold into Stage 3's "Taken entries split"; the same seam serves Stage 5 (Rename/New folder end makes a panel re-read).
- app keeps `Queue` (cfg(windows) only; starts no threads until prepare); core reads `Operation` snapshots, returns actions.
- Each list owns the ended operations it keeps; queue's set = waiting + running only. `kept` today mixes "reported" (lifecycle's fact) and "kept" (lists'). End stamped when first seen, from the listing-start counter (errs safe).
- Part of Model (Change/Then), dialogs follow `confirm_setup` steps (short borrow, ask, answer method, apply). Interface: `file_operation_started(id, source)`, `accept_operations(&[Operation]) -> Accepted`, `answered(id, Answer) -> Accepted`; `Change::Taken` per list; one `Then::FileOperations(Steps)` because `Accepted.then` is `Option<Then>` (one action) — Steps = queue calls, source endings, at most one dialog. Replaces `asking`, `TAKEN_CHANGED`, `set_taken`.
- Engine wakes only on state changes (prepare, submit, cancel, resume, dismiss, pause, move, prepare end, run end, gozlerd total) — never on progress; lifecycle runs on each wake (microseconds, inferred), never on the 500 ms poll.
- Own core module file, private Model field (Fixer pattern); rules tested on the type, cross-state cases through Model.
- Shutdown: `ManuallyDrop<Queue>` in the owning app type; WM_DESTROY calls close(); delete the caller's `mem::forget` (dropping cancels CopyFile2 which deletes partial files; joining without cancel hangs exit).
- gozler-fileops: public `OperationId` constructor ("An id that did not come from this queue names no operation, or one it issues later; build one only for snapshots in tests"); public Taken constructor from one operation's sources. No test-feature pattern exists in workspace. Docs to change: fast-copy-delete-spec "Taken entries", `Taken` doc, `fn taken` doc, file-manager-spec 834–836.
- Show/hide of File operations window, interrupted-at-start, select_first_interrupted → candidate 8 (display rules).
- "taken" stays out of CONTEXT.md (code/plan word; person sees a struck-through row, DESIGN uses the verb).
- Exit-question counts: core fn over snapshot, one test. OWNER (parked, keep as shipped): prepared counts as waiting.
- Tests: Stage 3 spec list + asks once / no second ask while open / kept iff delete-or-move with items>0 / reported once / cut paste moved only when submitted / drop succeeded only on Complete with nothing listed.

### C1 round 3 (closed, frontier empty)
- Queue posts its own message (TAKEN_CHANGED's freed slot, WAKE+5) to the main window, not WAKE (WAKE drains the request channel on every page/status/fix reply). Handler: lifecycle → apply Steps → refresh File operations view. File operations window created at start (first instance), not lazily.
- Steps order: queue calls, source endings, dialog last; accept_operations in its own statement; release file_operations borrow before the dialog. Never Remove and ReportFailure for one id in one Steps: remove only after answered(FailureSeen) and no list keeps it (tested).
- Dialog steps carry data (AskConflicts{id, request, prepared}, ReportFailure{id, request, outcome}); failure text stays in app (win::error_text).
- Cut paste counts as moved when core first sees the move Waiting/Running (core cannot see submit's result).
- Test "every drop gets one DropEnded" (incl. cancelled at question, refused at prepare): forgotten ending holds an async source's extracted files until exit; a completed copy would be reported aborted.
- B4 defect (read): Model offers file operations (line 834) before the queue opens (891); if open fails, commands stay enabled and do nothing. Offer and build lifecycle only after the queue opened.

## C3 Include page state — closed (1 round + settlement)
- service-settings.md later pages (Dates created, Rebuild index) are machine settings saved as administrator; need no count reply; they reuse "edited differs from applied" → C7.
- (b): dialog owns its Include page value (rule 1), formats with its own Strings and WindowsNumbers; Model keeps each latest Settings reply (count, Total Commander), Change carries no data, dialog reads via one MainWindow method. Removes `included_count`, `total_commander` fields and take_ methods; goes: indexed_entries, include_summary, include_preview, take_included_count; stays: include_drives (Win32 volume labels), count_included, save_include, other 11.
- Staleness: cancel the live count on each edit (Then::Cancel(Purpose::IncludeCount)), so generation is the only check; DESIGN's 300 ms debounce stays.
- Defect (read, not reproduced): no cancel/forget of the Include count when Settings closes; a late reply is stored in included_count and shows as "After OK" in the next session when any Reply (Fix, TotalCommander) arrives first.
- One Model "Settings closed" method forgets both replies and cancels both requests.
- Drives read when the page fills and after Modify…/status change (today: every edit, Win32 label per drive).
- Indexed folders drops &MainWindow, takes summary from page state.
- C3-B1 (settled, not owner): dialog's preview/summary use model.strings() while captions use the dialog's copy → mixed languages after Apply with new language. One dialog shows its own language until closed.
- Tests: stale count never shows "After OK"; edited==applied asks nothing; disconnected drive keeps its row with prefix from saved name. No wording tests.

## C4 Every change through Accepted; one save owner — round 1 settled (round 2 pending)
- Option (c): Model keeps last-saved Settings copy; compare at end of `apply`; SAVE timer when differs. Needs `Clone` on Settings/WindowState. Compare cheap (no alloc). apply runs per keystroke/chunk/status/selection.
- Copy taken when a save is attempted (success or not), else a failing disk is rewritten after every keystroke.
- Gaps to close: shown columns (`toggle_column` must keep `settings.window.columns` current, display order); column order (HDN_ENDDRAG → Message::Deferred reads order → Model method); placement.
- DESIGN "Where settings are saved": written a second after state changes, so it survives an end without Exit; lists placement, column order, visibility. Code departs for reorder and placement → fixes, not owner questions.
- Settings Apply: run all four refreshes (language, icons, memory, size format) inside one draw_once_after; no loss of search/scroll/selection/widths; flicker of apply_glyphs unverified (run once).
- Setters: show_index_page, set_language, set_destination_folder → Accepted; set_clipboard_holds_files, set_app_working_set, first_hide, update_check_due, offer_file_operations stay plain.
- Defect (read, verified): Language menu `save_settings` has no Instance::First check → a second window writes ui.json, against DESIGN "Window lifetime".

### C4 round 2 (closed, frontier empty)
- Placement: record on WM_WINDOWPOSCHANGED (only message covering maximize, restore, snap, Win+Arrow, Win+Shift+Arrow); handler returns None so DefWindowProc still makes WM_SIZE/WM_MOVE. Not through `apply` (its footer runs update_inspector etc.); a small compare-and-start-timer step shared by apply's end and the placement handler.
- Commits: (1) behaviour-free restructure, Language menu keeps its direct save; SAVE handler skips when nothing differs (else extra writes). (2) three fix commits: Language menu in a second window; save after column reorder; save after placement change.
- Stage 3: no collision; gain — panels' remembered folder/sort/active panel save with no new trigger.

## C5 Inspector — closed, downgraded to Speculative
- Two panels has no inspector; no stage changes its placement; fit_inspector changed in 2 commits (fed5604 own rules, 114d80c side effect), not 3.
- Shape when next touched: Layout::compute keeps InspectorLayout; a method on it takes measured widths + focused field index + shown facts (plain), returns rects, visibility, whole-or-shorten per value; app shortens with compact_path (PathCompactPathW needs final width, stays in win).
- B1: InspectorCaptions uses stand-ins: icon Option = "exactly one entry", modified Option = "row has age", note String empty = "no note" (empty-string sentinel against AGENTS code style). Make the inspector's state an enum (nothing, reading, one entry, several).
- Tests: final rects at min size & 150% no overlap; several entries no copy icons/age; short pane hides age; focused field keeps whole text.

## C6 Filter pane mapping — closed, Speculative
- Nothing scheduled touches the pane (issues #97 #88 #90 #91 unrelated; visibility "filter" = gozlerd permission filter; Two panels has no pane).
- range_closing/range_closed already typed (Option<(RangeFilter, ListClosing)>, Option<RangeFilter>); no defect; ordering settled by Message::Deferred; stays in app (one-enum tidy-up optional).
- When done: core pair in query.rs (positions + (list, index) → outcome: set filters / open Custom date|size / nothing); both directions use the same current filters (custom row shifts indexes). App pane module like app/status_bar.rs, returns outcomes, does not run dialogs; only_directly mentions in main_window drop 11 → 0.
- B1 echo check duplicated (on_kind_selected vs set_filters returns Nothing); B2 Match list same pattern (match_mode_index / MatchMode::ALL.get).
- Tests: custom-is-last with set custom range; location index follows volumes.

## C7 SettingsDraft — closed
- Deepen the Apply end; 9 of 13 edit sites are plain field sets. Typed outcome: refused shortcut applies nothing; failed registry write reverts that value (applied and edited); declined/failed Include save stays edited. `SettingsDraft::applied(outcome)` → new applied, still-edited, refill pages?, messages. Logic today spread over SettingsDialog::apply, MainWindow::apply_settings, write_registry.
- Apply blocks the UI thread through the admin copy (ShellExecuteExW runas, then WaitForSingleObject up to 30 s) — already #89 (proposes fix_thread pattern). Pending admin state only when #89 lands.
- min(64): core::speed clamp driven by SettingError::OutOfRange (engine exposes no constants; core learns ranges from errors). ring_blocks() hardcodes 16..=26 → filter powers RingBlock::from_bytes accepts.
- Page rules to core with tests: Total Commander link; update_apply (refused number enables Apply; shield when machine settings differ — becomes general when service-settings' Dates created lands); read_speed's is_enabled() dependency; per-processor clamp. Display one-liners stay.
- B1: draft keeps refused number boxes (by SpeedNumber, with text) so can_apply is one core rule, not Win32 state.
- Order: C4 first (no "which fields changed" value).
- Tests: refused shortcut applies nothing; failed sign-in write reverts + refill; declined Include stays edited with shield; per-processor off clamps to top; disabled box not checked; Total Commander link.

## C8 File operations window display — closed, Worth exploring
- A core display value the File operations window owns (not Model): meters, source folders, shown ids, show/hide rules (moved from C1), interrupted-at-start selection. Window passes selected id, visibility, now, text-measure closures as plain values.
- landing, step, drag_to's "unchanged" test are pure → beside RowPhase with one table test; can_step stays in app (asks engine check_move).
- Delete win::TaskbarState; TaskbarProgress::show takes core Taskbar (win already imports core types).
- show_hint loop already uses PathCompactPathW; it shrinks the title's room by 1/10 per pass because the title sits inside a sentence. Replace with room = width − width(sentence with empty title), then RowLine::fitted once (one final measured check).
- B1: hint_shown() reads control text; window height depends on it. Display value keeps Option<OrderRefused>; "hint shown" derived.
- Tests: new op shows window, later state change does not re-show after Hide; last op leaving hides; interrupted at start shows + selects first interrupted; paused meter clears / non-running meter dropped; reorder table; hint fits.

## C9 "Use local" decision — closed, Worth exploring
- I/O order is a rule (pairing per DESIGN "Daemon failures", not SECURITY.md): run service exe --version; same length then same bytes → SameAsService (local not run); not paired → Mismatched (nothing runs); only paired → run --version and protocol. Bytes read only when lengths match.
- One core::fetch function taking closures (bytes_equal, paired, print(program, arg)), like RowLine::fitted takes width_of; version()/filetime parsing moves with it.
- Cache (static Mutex keyed by paths + (mtime, len)) stays in app.
- Defect (read): a timed-out print (PRINT_LIMIT 10 s) is cached as Check::Done → one slow first run (antivirus) hides Use local until restart. Fix: report timeout, do not cache it.
- Tests: unpaired → Mismatched and print never called with its path; same bytes → SameAsService, local not run; different lengths never read bytes; timeout not cached.

## C10 run reads pending selection — closed, Speculative
- No caller must avoid the read; read_list_selection returns on one flag check when nothing pending. No path wrong today (posted DEFERRED runs before input; T11 holds).
- Rule 1: read first in its own statement, before `focus` (read calls apply, which can move focus), then the enabled borrow. Removes six direct calls.
- Do in Stage 3 when "active list" exists; run asks the active list's controller.
- B1: Message::Deferred serves four unrelated flags (selection, select_search, range_closed, shortcut failure); with per-list controllers, Stage 3 must choose: each controller's own deferred work, or Deferred asks every controller.

## C11 ControlId-keyed rectangles — dropped as a candidate
- Layout hides whole groups with Option fields; Two panels hides/adds whole groups → fits existing "one Option per group" shape.
- ControlId is generated from 250 IDC_ lines (every dialog) and includes non-windows (toolbar buttons, status parts): exhaustive match = ~190 None arms. Many controls placed outside a plain rect list (filter pane scroll, inspector 2nd pass, message area, status bar, list in draw_once_after, variable field rows).
- Better existing pattern: `controls.status.place(&layout.status, …)` (app/status_bar.rs): each pane module places its own group; compiler still checks fields.
- ×10 vs ×12 is NOT a defect: Match ≤5 items, Size ≤10 (Date ≤8): each factor fits its longest list. Reason unwritten and in app; a core fn (row height, item count) when a list changes. Inferred: CBS_DROPDOWNLIST without CB_SETMINVISIBLE — one run would show whether the factor matters.
- B2: placed_with's "is a drop-down" bool goes into the filter pane's own placement.

## C2 Column sizing and cell content — frontier empty (2 rounds)
- Panels share fill-on-width-change, measure-once-per-drag, DPI discard, visible-rows measuring; differ in fill rule (Search: proportional `fill_widths`; panels: Name takes the rest) and panels hide no columns. Module carries a fill rule per list.
- Timing: inside Stage 3 restructuring, after "Two lists in Layout::compute"; cell function after "One state for each list" (takes a list's state). Add item to spec's Stage 3 list first. Reason: AGENTS "Writing a plan" item 4 (restructure before features), else Stage 4 copies five fields 3x.
- State: new `core::columns` type held per list in app, not in Model (no Model decision reads widths; SavedColumn holds no width). Keeps no copy of current widths; list view owns them.
- Seam: app reads list facts (visible rows, label offset, count) first as plain data; lazy fits provider only borrows Model and measures via screen DC (TextMeter, DrawTextW DT_CALCRECT) — rule 1 safe. Measure-when-stale stays: DESIGN documents it; formatting+measuring is the cost (#85 quadratic Compared::new).
- Pure fits function (headers, cells+bold, label offset, measure) -> fits; serves divider double-click and Fit columns without touching sizing state.
- Move ColumnFit, fill_widths, scale_to, cell_width, column_width to core::columns; cell_text_inset stays in layout.
- Cell function in core::captions (reads model facts), calls text::cell.
- Taken-row over-wide fit (bold measured, plain drawn): fixed by cell function returning no bold for taken rows — a behaviour change, own commit, reported as fix.
- B1: `win::controls::ReportList::fit_columns` is a second fit-and-fill (File operations failure list); route through pure fits function; keep its scroll-bar subtraction difference as is or settle deliberately.
- DESIGN "How normal changes land" row "A new column ... arm in LVN_GETDISPINFO handler" is out of date (match is in text::cell).
- Tests: 5 sizing-rule tests via events; 1 cell test (taken row keeps text/icon, no bold, struck); 1 `within` boundary test; fill_widths test moves; Stage 4 adds panel fill test.
- OWNER (parked, Stage 4): refit panel Ext/Size/Date per folder listing?; first-widths rule once per window or per listing? Interface does not depend on them (adds an event).

---

# Re-check on merged main 7b94f06 (independent reviewers)

## R1: C1, C3, C7, C8, C9
- C1: the life of a file operation grew in app, not core. `Model::keeps` and `Model::operations_ended` moved the keep decision into core. Objections 1, 2, 6, 7 still valid; 3, 4, 5 obsolete.
- New: two owners for "when an ended operation leaves the queue" (`Model::keeps` and app's `reported`/`told` plus `Window::release`). Untested window rules in app `RefCell` fields: the 2 s open (`OPEN_AFTER`), Hide (`hidden`), the status feed.
- C8 objection 1 stronger: `refresh` posts `OPERATION_CHANGED` (WAKE+8) to feed the status bar.
- C3, C7, C9 and their defects unchanged.
- Defender (fresh) running.

## R2: C2, C4, C10 and two defects
- C2 drops to Worth exploring: panels do not share Search's sizing (`fill_panel_columns`, `widest()` samples). `captions::panel_cell` is the precedent for one cell function.
- New defect (spec mismatch): panel Ext column fitted to sample `x.xlsx` (verified `panels.rs` `widest`), so longer extensions are cut, against file-manager-spec "A panel".
- C4: new saved state (mode, panels, viewer). Panels arm `timer::SAVE` directly in `read` (verified `panels.rs` 701). Not saved: active panel, search-panel sort, Viewer placement and Wrap. Compare cannot sit only at the end of `apply`.
- C10 half built: panels' `run` reads the selection; Search's four callers still read first. Raise to Worth exploring.
- ui.json defect worse: whole-file write now also overwrites panels and Viewer state.
- C5, C6, C11 and the bold-measuring defect unchanged.
- Defender (fresh) running.

## R3: P1, P2 and two defects
- P1 not built and no longer stands as written; panels are `impl MainWindow` split by `Side`. Rule 1 kept by convention only (`list.focused()` under `borrow_mut`, harmless today).
- P2 not built; a second return channel grew beside `Accepted` (`set_taken`, `operations_ended`, direct `Panels` calls). Re-entry into `apply` is now synchronous (`follow_panel_kind` -> `set_text("")` -> EN_CHANGE -> `apply`).
- New: "what ends when a panel stops" written three times (`follow_panel_kind`, `stop_panels`, `find_in_panel`); `show_panel` changes state under a "show" name.
- New defect by reading: `on_panel_events` loops over drained events; a Refused read opens `error_box` inside the loop (verified `panels.rs` 711, 847, 879); a nested wake applies newer events; the outer loop then applies older ones, so a panel can show a deleted entry.
- T2/T3 still present, also reached by search panels. Setup question still before its change.
- Defender (fresh) running.

## P3–P5 round 2 on 7b94f06 (answerer; facts checked by coordinator)
- P3 built as planned: capture with `place`, marks follow entries in core (`Panel::select`, `Panel::merge`), set again with `ListView::change_selected` (verified controls.rs). Corrections: Modified/Gone names match case-insensitively once (the built rule is right). The top row keeps its index, not its name (`LVSICF_NOSCROLL`, verified). Shift+End per-row loop is #108 (open, verified). Open: should the top row follow its name when entries are added above? Needs the behaviour of an established app; not a pattern question.
- P4 partly solved: the outer loop routes by top-level window (`own_keys`, `set_modeless`, verified); the Viewer handles its own keys in `area_procedure`. No `WH_MSGFILTER` (verified). The File operations window still gets no Tab/Esc/Enter while a dialog owned by the main window is open. Revised fix: a message-filter hook for MSGF_DIALOGBOX and MSGF_MESSAGEBOX, with a re-entry guard, calling the same routing function. Frontier empty; one check run per modal loop only if the hook is built.
- P5 withdrawn: Stage 4 already gave the panels' disk threads one typed channel and one wake. Merging the rest fixes no visible failure. The wake flag and the two tests are withdrawn. DESIGN rule 4's "the only staleness check" no longer describes the panels (three checks).
- Order of work is now P2, then P1. The panel event order defect (found independently by R3 and this answerer) is P2's concrete case: accept every event of one wake before any modal step.

## Defender D1 on R1 (C1, C3, C7, C8, C9) — conceded except two third answers
- C1 revised: Queue created at start by the main window (first instance); File operations window created on first use (open rule, interrupted at start, View > File operations). Core lifecycle (private Model field) owns the 2 s open rule, Hide memory, interrupted at start, status-bar operation, taskbar state, "took", "reported", remove decision (calls Model::keeps), tells lists inside accept_operations (mailbox ended/take_ended/told/TAKEN_CHANGED goes). 500 ms poll moves to main window while an operation runs. Per-operation phase (not asked/asking/submitted; unreported/reporting/reported); modal step pushed only on entering asking/reporting. answered() returns a Submit step; app calls submitted(id, Result) → Keep both recorded, paste Moved. Queue waker closed in main window WM_DESTROY before queue.close(). Offer file ops follows the queue; window failure logged.
- C3 revised: each Include edit calls one Model method that cancels the live count and forgets the stored count.
- C7: Strong; one machine-settings case (saved/declined/failed); engine exposes Count::MIN/MAX; ring_blocks from RingBlock's exposed range.
- C8 (2) third answer: compute common folder once per operation, reuse in conflict question, rewrite common_folder allocation-free, rerun the timing test; prepare thread only if > 16.7 ms. (sent to R1)
- C8 (3) third answer: taskbar state is a core function in the C1 module; main window shows it; adapter caches last state, forgets on TaskbarButtonCreated. (sent to R1)
- C9: one retry after a timeout per build; printed() typed (timeout vs failure). Defect (c) conceded.
- R1 agrees with both C8 third answers. Conditions: keep Unicode case comparison (char case-mapping iterators, not eq_ignore_ascii_case); the cached folder lives with the C1 lifecycle, not the window. C1/C3/C7/C8/C9 group: agreed, closed.

## Defender D3 on R3 (P1, P2, N1–N3)
- P1 withdrawn. Rule-1 breaches (four, incl. release_operations holding borrow across PostMessageW) harmless today.
- P2 narrowed (third answer): no reshape of Accepted. Rule: a wake opens no dialog until every reply and panel event drained with it is applied; dialogs from replies/disk events go to Message::Deferred, one at a time, with a guard. T2/T3 fixed in apply's NewSearch and Message arms (requests.cancel(Purpose::Selection(list))).
- N1 (third answer): three transitions, each kind's list written twice → one app method per kind; stop_panels calls both. Name operations_ended's bools.
- N2 conceded: accept_read's result says the search ended; on_read ends it; show_panel only shows.
- N3 conceded: hold the refusal until all events and pending batches are applied.
- Sent to R3 for agreement.
- R3 agrees with D3 except: Open-first (open_first_or_selected from NewSearch inside on_wake) breaks the narrowed rule. Coordinator settles: Open-first goes on the deferred path too (one posted message). Addition accepted: the named type from set_taken/operations_ended also carries show_taken's "lost" fact instead of matches!(accepted.change, ...). Deferred refusal keeps path, failure, reason, typed text. P1/P2/N group: closed.

## Defender D2 on R2 (C2, C4, C10, defects)
- C2 Strong (8da09ba sixth reset site); core sizing type for Search with events; one core bold-ranges function, none for taken rows.
- Ext/Size samples too narrow; spec sentence and doc comment describe fixed fits to wider samples.
- C4: compare by reference, no mirrors; panels outside Accepted; compare step after apply, read, sort_panel, activate_panel, Viewer remember_placement/toggle_wrap.
- New defect: placement saved before first show (start in notification area) loses maximized.
- Viewer to WM_WINDOWPOSCHANGED.
- ui.json: each window replaces only the groups it changed, re-reading the file at save time.
- Sent to R2 for agreement.
