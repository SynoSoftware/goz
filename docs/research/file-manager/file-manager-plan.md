# File manager: coordination plan

The spec is `C:\SynoSoftware\goz-file-manager\docs\plans\file-manager-spec.md`
(branch `file-manager`, worktree `C:\SynoSoftware\goz-file-manager`). It
settles every behavior; this file holds only what coordinating adds. The lead
(the "File manager Norton Commander plan" session) edits this file; agents
read it and report to the lead. This folder (`.work`, ignored through
`.git/info/exclude`) replaces the lead's Temp scratchpad, which was emptied
at 00:50 on 2026-10-21.

## Settled owner decisions

- 2026-10-21: the prototype is approved
  (`crates/gozler-ui/two-panels.prototype.html`, look A; screenshots in
  `.work\shots`). Its V choices are written into the spec: F6 on one entry
  shows the folder and the name (V1 b); the File operations window opens
  after 2 seconds (V2 b); the status bar's middle group says what an ended
  operation did (V3 b); `Esc` and Edit > Clear selection clear the selection
  (V4 a). Settled.
- 2026-10-21: the File operations window, the conflict question and the
  Delete question are built for Search; Two panels uses them unchanged and
  never redraws or redesigns them. V2 and V3 change the window's opening
  and the status bar for both modes (stage 5). Settled.
- 2026-10-21 (goal): implement the whole plan and get it passed by the
  adversarial reviewer. Settled.
- Standing: stage commits on branch `file-manager` after the adversarial
  review passes; never push, never merge into `main` without the owner.
  No attribution trailer in commits.
- Standing: never stop, replace or elevate the installed goz or gozler
  services; no UAC; do not touch the owner's `ui.json` or `operations.json`
  (use `--data-dir`); do not use the owner's clipboard.

## Owner requests, in order

1. 2026-10-21: after review rounds 5-6, stop iterating on the prototype and
   implement with the coordinating skill. (Prototype review ended at round 4:
   nothing material left.)
2. 2026-10-21: the implementer for stages 4-7 runs as a visible session
   (`fm-implementer`), not a background subagent. Settled.
3. 2026-10-21: the lead cleans up its own temporary data (done: the 11 GB
   `gzcap-fixture` from a stopped capture agent, and unused screenshots).
4. 2026-10-21 (/goal): implement the file manager plan and get it passed
   by an adversarial reviewer.
5. 2026-10-21: merge main after stage 7 — replaced by 6.
6. 2026-10-21: do not merge main; finish the plan on this branch first.
   Settled.
8. 2026-10-21: merge to main as soon as convenient — then: the owner
   finishes work on main first; the lead finishes every task on this
   branch (stage 7, 7b key bar + panel-row run, stage 8, final verdict)
   and tells the owner, who then says when to merge. Settled. The merge
   plan (merge main into file-manager with `.work/merge-main-notes.md`,
   check, adversarial review of the result, fast-forward main) waits
   for the owner. Owner: will pause main once the 3 tasks running there
   finish; main is ramping down. So the merge is one merge of a paused
   main, no repeat merge needed.
   Owner then: stopping main's tasks, asked for a sooner stable point.
   Lead chose 9d46d3e (stages 3-6, reviewed, green; owner's answer to the
   choice did not arrive): session `fm-merge` (own worktree at 9d46d3e,
   branch fm-merge) merges main, fixes 3B, check, commits on its branch.
   Then an independent review of the merge, then main fast-forwards with
   the owner's go-ahead. Stage 7 + key bar follow with a second merge.
   Owner then (settled): merge everything now, even without stage 7's
   adversarial review; one merge. Stage 7 committed 86bdfe4 as written
   (check green, its review: not ready, M1 taken-set order, M2 restart on
   status; fixes listed in `.work/stage7-after-merge-fixes.md`, done after
   the merge). Key bar (fm-keybar) left out: its check was not done; it
   follows on main. Owner: every session on main stopped; go to merge.
   fm-merge told to ff to 86bdfe4 and merge main.
   Owner: keep the key bar work going (settled): fm-keybar finishes and
   commits on its branch synosoft/fm-keybar-a76e71 (worktree
   `.claude\worktrees\fm-keybar-a76e71`); the lead brings that commit
   onto main after the merge lands.
   Stage 7 fix round (items in `.work/stage7-after-merge-fixes.md`) runs
   now in fm-implementer on file-manager, committed after 86bdfe4; the
   stage 7 reviewer rechecks by reading; fm-merge then merges that commit
   on top of its merge. After the merge commit: independent adversarial
   review of the merged branch, fixes, then main fast-forwards (owner's
   go given). Then key bar onto main, stage 8 docs, final verdict.
   Lead mistake, corrected (owner noticed, 2026-10-21): three sessions
   each built their own worktree in the shared target. Rule now: one
   builder, fm-merge. fm-implementer and fm-keybar commit without a full
   check (`-Test` only for fm-implementer); fm-merge merges both commits
   on top of its merge, then one full check.ps1 over everything.
   Owner (settled): stop the release-style ceremony. No screenshots after
   the merge, no -Release builds, no reruns of untouched code; one full
   check.ps1 over the merge, then commit and fast-forward main (owner's
   go already given). The merge's review runs on main afterwards and
   does not hold the merge. Key bar committed cb2466d (its check green).
9. **Owner ruling (settled, 2026-10-21): no custom drawing for little
   gain** ("many things to take care of, the experience is not the same,
   and speed is not either"). Replaces the spec's look A for panel rows:
   the list view draws the rows (selection fill, focus outline); only a
   font or colour may be set at item prepaint, as 114d80c does; `..` gets
   an icon; the Folder column's middle ellipsis goes with the row
   drawing (`draw_panel_row`, `PanelRow`, `RowBar`, `RowFonts`,
   `CellFit`). Done after the merge lands, on main. The native-controls
   review (running) judges every "keep" item by this rule.
   Owner saw a mock of look A vs native and confirmed (settled): use
   native, accepting that the inactive panel shows no focused row and a
   focused selected row is only an outline on the fill.
   Native-controls review (`.work/native-controls-review.md`): after the
   merge 5 drawn controls have no conversion planned (dividers, Settings
   group boxes with icon, Location list, Failed list path column, File
   operations card); keep 2 (bold matched parts, Viewer area); covered
   10 (#109, key bar cb2466d, 114d80c's 8). Merge hazards sent to
   fm-merge. Main moved: 6def2f6 after 95ba03a; sessions on main still
   benchmarking (owner told).
   Owner (settled): Settings sections use the best of Win32, no other
   framework: no group box; each section is a heading made of a native
   static icon (`SS_ICON`, as 114d80c's static icons) and a static title
   in a heading font, with its controls indented below, and a native
   etched line (`SS_ETCHEDHORZ`) between sections where a page has
   several. The icons stay; the drawn group box (`show_group_glyph`,
   `group_procedure`, `paint_group`, `redraw_group_caption`) goes. Native-controls work after
   the merge, on main: panel rows (+ `..` icon, Folder column), dividers,
   Settings group boxes, Location list, Failed list path column, File
   operations card. Status bar right group: #109.
   Owner: use idle workers during the merge. Session `fm-native`
   (worktree from main, branch fm-native) writes the five main-only
   conversions now and builds only after fm-merge's check finishes (lead
   tells it). Panel rows wait for the merge (branch code).
   Owner: start the status bar work too (#109). Given to fm-native as
   item 6 (it owns `win/controls.rs` and `win/gdi.rs`; one editor per
   file), own commit with `Fixes #109`.
   fm-merge: four merges committed, tip 5c85cd5 (main 4fa93ba, 6def2f6
   aa63d37, stage 7 fix 54c5e6c, key bar 5c85cd5); full check running.
   main moved again: 8da09ba (window: Clear's icon, column fill), a098e86,
   a6d4ed0 (docs) to merge before main fast-forwards.
   Stage 7 fixes committed f5573d9 (full check green; memory B1-B0 6.96
   MB, B2-B0 14.43, B3-B2 -0.46): sent to fm-merge with cb2466d, 6def2f6.
7. 2026-10-21: report where hand-drawn controls are used where native
   ones give almost the same, using the patterns 114d80c corrected on
   main as the pattern book. Report: `.work/custom-controls-report.md`
   (2 switch, 1 unsure, 7 keep, 8 done on main). Lead confirmed:
   `Button::draw_key` added in ba3b0c9; DESIGN "Not planned" excludes
   custom-drawn buttons; status bar right group is `SBT_OWNERDRAW`.
   Scope asked of the owner; no answer reached the lead, so the
   recommended scope applies until the owner says otherwise (lead,
   2026-10-21): stage 7b converts only this branch's own hand-drawn
   controls: the key bar (plain push buttons, Windows draws them), and
   the panel rows only if one run shows the Explorer-themed list draws
   its own selection with font and colours set at item prepaint. The
   status bar's right group and main's 8 conversions stay for main.
   Owner confirmed (settled, 2026-10-21): the status bar's right group is
   converted on main, filed as #109; this branch gets it by the merge.
   Owner (2026-10-21): fix in parallel with another agent where it does
   not conflict with stage 7. Checked: stage 7 does not touch the key
   bar's drawing, but changes the row drawing (`CellFit`, middle ellipsis
   for the Folder column). So: key bar now in session `fm-keybar`
   (Sonnet, own worktree from file-manager, branch fm-keybar, shared
   target, commits on its branch; lead merges it into file-manager after
   stage 7's commit, owner-requested). Panel rows wait for stage 7; the
   Folder column's middle ellipsis is a reason to keep them drawn, decide
   with the one run after stage 7.

## Who does what

- Implementer and only builder in the worktree: session `fm-implementer`,
  on stage 4 since stage 3's commit. Its taken entries split (patches) was
  read by the lead before applying: design accepted, full review in stage 4.
- Lead: this file, reviews, stage commits.
- Stage 3: committed as cff3b32 on 2026-10-21. Independent final review:
  ready, no findings. Full check.ps1 green apart from one fmt line, fixed
  and rechecked with `cargo fmt --all --check`. The note that the
  Two-panels test additions in `core::commands` check nothing yet is
  accepted: stages 4-5 add Two-panels-only commands that they then check.

## Confirmed findings

- Folder sizes: `gozler -parent <folder> folder: -size` answers in 38-149 ms
  on the owner's gozlerd (measured in the spec, "Folder sizes"). Confirmed.
- Waits: every disk thread (reading, watching, drive labels, free space,
  Viewer) is ended with `CancelSynchronousIo` and never joined (spec, "A
  panel"). Settled by the spec.
- No code yet for `ReadDirectoryChangesW` or drive-removal notification in
  the workspace (grep, 2026-10-21). Stage 4's implementer chooses and checks
  the removal notification once (spec: "Stage 4 chooses the Win32
  notification").
- `gozler-fileops::Taken` (`crates/gozler-fileops/src/taken.rs`, used by
  `queue.rs`) is the set stage 4 splits first.
- Stage 3's earlier full check: only `gozler-client`'s
  `a_cancelled_request…` and `a_cancelled_connect…` timing tests failed, at
  about 200 ms under load. Stage 3 does not touch `gozler-client`.
- The main checkout (`C:\SynoSoftware\TinyTorrent2\3rdParty\goz`) holds
  other sessions' uncommitted edits in `gozler-ui` files that later stages
  edit (`main_window.rs`, `file_operations.rs`, `status_bar.rs`,
  `dialogs.rs`, `captions.rs`, the catalogues). `origin/main` had no commit
  past this branch's base (492175d) on 2026-10-21. Merging back needs those
  sessions' work committed first.
- Other sessions build in the shared target folder at the same time (seen:
  `cargo doc`, `cargo test`, `cargo build --release` at 00:48-00:52), so a
  timing test can fail under load.

- Stage 5's engine changes are both in `gozler-fileops`' prepare
  (`crates/gozler-fileops/src/prepare.rs`, `Refusal::DestinationMissing` in
  `operation.rs`): open the nearest existing folder above a missing
  destination; one entry to a full path checks conflicts under the new
  name, and the saved operation records it (an older saved file loads as
  before). The engine session is not running; the implementer makes these
  changes on this branch. Settled by the spec, "Copy and Move" step 4.

## Build rules

- One builder at a time in this worktree: `$env:CARGO_TARGET_DIR =
  'C:\SynoSoftware\TinyTorrent2\3rdParty\goz\target'`, then `pwsh
  ./check.ps1` from the worktree. Cargo's lock serializes builds with other
  sessions.
- During a stage: `check.ps1 -Test <filter>` only for code that changed.
  One full `check.ps1` per stage, after review. An exe for the owner comes
  from `check.ps1 -Release`.
- Do not edit `Cargo.toml`, `check.ps1` or `AGENTS.md`'s build lines.
- Never build into the worktree's own `target\` (deleted 2026-10-21, 3.6
  GB, after stage 4's release builds went there). Screenshots use the debug
  `gozler-ui.exe` in the shared `target\debug`. `-Release` fails while the
  owner's release exes run; that is expected, and only an exe the owner
  asks to try needs it.
- Unconfirmed: a stage 4 release build at 02:09 may have replaced the
  shared `target\release\gozler.exe` (dated 02:21) with this branch's CLI.

## Stage 4 review (2026-10-21)

- Adversarial review: not ready, 3 high (H1 window-thread work per
  change grows with the folder; H2 a reread loses changes; H3 Space in
  quick search), 5 medium (M1 reread cancels a navigation; M2 ended copies
  never reach the panels; M3 watch thread misses its cancel; M4 known-folder
  lookup verifies on the window thread; M5 ended operations kept forever by
  Search's list), 8 low. Lead confirmed in code: `sort_entries` keys and
  sort on the window thread; `Removal::Query` waits on the window thread up
  to RELEASE_STEPS x RELEASE_STEP; free-space and label threads are plain
  spawns. Others relayed as claims to reproduce.
- Lead decisions: M5, Search's list keeps an ended operation only when, at
  its end, it takes a row of Search's fetched results. Native rendering of
  Windows controls is accepted (19 px rows, the drop-down caret, Retry
  glyph placement, the standard error MessageBox, quick search as a
  read-only edit), because Search uses the same controls. The path box
  reverting on focus loss is accepted and goes into the spec. The Alt
  check and the drive-removal check go to the after-install list.
- Prototype differences to fix: `..` up-folder icon; drive menu opens with
  the drive highlighted; a visible inactive focus bar; rows fill to the
  divider; vertical column lines (fix unless Search shares the cause);
  status bar middle group takes the room.
- All sent to fm-implementer as one message; it fixes, then one build.
- Fix round 1 (07:06): all fixed; check.ps1 green (553 tests) on its
  second full run. Recheck by the reviewer: not ready. New: N1 (high) a
  failed navigation leaves the old folder unwatched; N2 memory about 170
  bytes per entry (lead decision: keep the spec's ~100, use a 64-bit hash
  of the folded name instead of a stored key); N3 Safely Remove during a
  listing; N4 merge copies the whole Vec per batch; N5 Shift+Space; N6 `~`
  names vanish when final_path fails; N7 search_kept race. Sent with the
  screenshot work; round 2 running.
- Fix round 2 (08:14): all N fixed except N3's last gap (a removal request
  during a listing call may be refused once; accepted by the lead and
  written into the spec, because closing it needs a window-thread wait or
  closing a handle in use). Entry is 48 bytes (asserted), about 100-116
  bytes per entry. check.ps1 green, 555 tests. Screenshots retaken, plus
  First start, empty folder, quick search with Space. Vertical lines: Search
  shows the same (same ListView, Explorer theme), left as is. Final
  verdict by a fresh reviewer running.
- Final verdict (fresh Opus reviewer, 08:45): not ready. M1 a refused
  removal during a Go listing leaves "Reading..." with nothing running; L2
  fold joins names NTFS keeps apart; L3 release reads taken without ended;
  L4 an end during a Go can leave an entry hidden; suspected window waits
  W1 header-click sort, W2 full passes per change batch, W3 lock held across
  RegisterDeviceNotificationW. All sent as round 3 (the waits are fixed, not
  filed, because this task can fix them). The same reviewer rechecks.
- Fix round 3 (08:55): all fixed, check.ps1 green (559 tests); one wait
  filed as SynoSoftware/goz#105 (re-sort of folders on the window thread
  when sizes arrive under Size sort). Lead: the task can fix it, so the
  implementer fixes it now; the verdict reviewer rechecks round 3 in
  parallel.
- Recheck of round 3: M1, L3, L4, W1, W3, totals confirmed. New N1
  (medium, from W2) a hidden entry keeps its selection mark and is later
  re-selected by itself; N2 an added name matched case-insensitively
  removes a case-twin. Sent as round 4 with #105.
- Lead rulings (settled): an added name, or the new name of a rename, is
  the name as stored on disk and matches exactly; the case-ignoring
  fallback is only for removed names and old names of renames. Window
  budget per change batch: simple passes over the entries without
  allocation or folding per entry are acceptable; allocation or folding
  per entry is not.
- #105 fixed by fm-implementer (09:17) and closed: the Size re-sort runs
  on a short-lived thread (`FolderSort`, `PanelEvent::FolderOrder`); rereads
  pass known sizes to the reading thread. check.ps1 green.
- Messaging to fm-implementer paused by the app at 09:05 (10 automatic
  messages without an owner message). Round 4 instructions saved in
  `.work/stage4-round4.md`; an Opus subagent does round 4 while the
  implementer is idle (one builder at a time). Owner asked to send any
  message to unblock.
- Round 4 (Opus subagent, 09:40): N1 and N2 fixed in `core/panel.rs`
  (`filter` clears hidden marks; `merge` returns `Merged { unplaced_short,
  twins }`; focus follows a rename only on an exact or merge-matched twin);
  check.ps1 green, 562 tests. Its report: a chained rename in one batch
  (a->b->c) loses the selection: fixed (`renamed` maps c->a; test
  `a_chain_of_renames_keeps_the_selection`), check.ps1 green, 563 tests.
  Recheck of #105 + round 4 by a6f2d08: ready to commit, three low
  findings. Round 5 (same Opus subagent) fixes: reread carry-over by exact
  name; FolderSort refuses another listing's result; modified names match
  exact first, then case-ignoring (watch may report the opener's spelling,
  unconfirmed). Lead ruling (settled): a rename does NOT carry the
  selection (spec: the selection keeps its names; a backup save would
  otherwise select two entries).
  Round 5 done, check green 564; recheck: ready, one low finding (Word's
  save renames away and back, the focus follows the first rename and
  leaves the document). Lead ruling (settled), round 6: the focus does not
  follow a rename either; a focused name that goes is remembered until the
  person moves the focus or the folder changes/rereads, and the focus
  returns when the name comes back.
  Round 6 + last fix (focus returns from `..`) done, check green 565;
  recheck ready; lead read the last fix. **Stage 4 committed ba3b0c9.**
- Stage 5 started in fm-implementer (revised design + design review);
  its own F6 must reselect and refocus its result.
  Code written (~32 files); fm-implementer's full check.ps1 running;
  adversarial review (fresh Opus subagent) of `git diff ba3b0c9` running
  at the same time.
  fm-implementer: check.ps1 green on run 3 (runs 1-2: fmt, clippy
  enum size / type complexity, example signature); screenshots
  `.work/stage5/01-06`; fixed the running line running under the bar
  (`job_text` + `compact_path`).
  Lead's screenshot observation for the fix round: 04 New folder "exists"
  message names the full path, and MessageBox breaks it mid-word
  ("...\fixture\proje / cts already exists."); a folder named in a message
  should be its name (memory: paths never size dialogs). Dialog
  positions in the shots are DS_CENTER (monitor work area), as all
  existing dialogs: not a stage 5 finding.
  Stage 5 review (fresh Opus): not ready. Confirmed by lead in code:
  M-a F6 awaits a name a row already shows; M-b nothing builds
  `Request::Rename`, rename line unreachable; M-c `present()` stats on the
  window thread (Two panels fixed in stage; Search filed as #106). Low:
  L-a stale ended line during failure dialog, L-b FocusChanged walks the
  selection, L-c misplaced doc comment, L-d Hide during a question.
  H1-L9 of the design review: all done except M6 (rename line, = M-b).
  Fix round 1 sent to fm-implementer with the lead's 04 message finding.
  Fix round 1 done (check green, run 2 of the round). Recheck: all fixed,
  new N1 (cancelled rename/new folder says "Renamed"/"Created") and N2
  (ended line picked by id; a kept delete hides a later copy's line),
  low: Keep both rename names the typed name. Lead ruling: a cancelled
  rename or new folder shows no line. Fix round 2 sent.
  Fix round 2 done, check green (1 full run); recheck: ready.
  **Stage 5 committed 95035a8.**
- Stage 6 sent to fm-implementer (apply wiring list, compile, own the
  code). Lead rulings: open from the wide path; message + Retry where the
  prototype puts them; a new Viewer cascades by the caption height.
  Accepted: wrap by characters, no horizontal bar, 3 rows per notch,
  moves dropped while a block is awaited.
  Stage 6 wired by fm-implementer, check green (2 full runs, 607 tests),
  screenshots `.work/stage6/01-04` (lead looked: match the spec). Reviews
  running in parallel: adversarial (Opus) + spec/prototype (Sonnet).
  Adversarial: not ready, H1 (re-read of block 0 sent as Opened resets
  the document and stalls reads; confirmed by lead) + 4 low + removals.
  Spec/prototype: nothing missing; three spec sentences to add; native
  bar, font, status alignment accepted as built. Fix round 1 sent.
  Fix round 1 done (green, 608 tests; note missed 14 min during a lead
  turn). Recheck by the adversarial reviewer: ready (one comment wrap,
  fixed by lead). **Stage 6 committed 9d46d3e** (DESIGN.md and prototype
  left out).
- Stage 7 sent to fm-implementer with the design, the corrections file
  and the plan's rulings.
  Stage 7 written: check green (2 full runs; gozler-ui 161). Minimum
  client width en 833, es 859, nl 853, tr 853 DIP (measured, release exe,
  96 DPI). Memory: B1-B0 7.55 / 7.04 MB vs 7.5 budget (run 1 over by
  0.05; the cost is the two row chunks, ~6.9 MB), B2-B0 14.1/13.6 vs 15,
  B3-B2 0.33/0.38 < 1. Screenshots `.work/stage7/01-05` (lead looked).
  Reviews running in parallel: adversarial (Opus), spec/prototype
  (Sonnet).
- **Owner decision (settled, 2026-10-21, replaces "merge after stage
  7"): do not merge main.** Finish the plan on this branch first; main
  is going through speed optimisation and architectural reorganisation.
  Stage 8's last pass and the final review cover this branch as it is.
  (Earlier: merge main into file-manager after stage 7's commit.) main is 7 commits ahead of the branch point 492175d (native
  controls 114d80c, folder-row drop e2df95a, 0.4.0); 33 files overlap.
  Nothing is pushed.
  Merge notes `.work/merge-main-notes.md` (Sonnet, read-only): hidden
  compile breaks (section 3A), wrong-but-compiling behaviour (3B: Tab,
  drop in Two panels, timer ids 6/7 vs SAVE/DROP_SCROLL, memory group,
  "Show sizes in", Esc, save on mode change), spec/CONTEXT reconciling
  (4), stage 7 call sites (3E). Estimate for the merge: 1.5-2 h incl. its
  fix round, build and review. Owner asked (2026-10-21) for a report of
  hand-drawn controls that native ones could replace: Opus subagent
  writing `.work/custom-controls-report.md`.
- Stage 7 design: Opus subagent writing `.work/stage7-design.md`
  (read-only); a design review follows before fm-implementer codes it.
  Design written (999 lines). Lead answers (settled): Q1 "Searching…"
  then empty; Q2 entries below a selected folder left out of the
  selection's size; Q4 Ctrl+L focuses the search box; Q5 F6 on a result
  uses its parent; Q6 a search panel drops rows when Search shows and
  searches again on return; Q7 "not indexed" rechecked on status; Q8
  implementer writes es/nl/tr. Q3 (minimum width +200 DIP) given to the
  design reviewer; lead prefers the box to shrink first. Design review
  (Opus): needs revision. B1 failure recovery across lists, B2 leave a
  deleted searched folder (`nearest: true`), B3 selection in a search
  panel, B4 panel message height. Q3 settled: minimum width of Two panels
  grows to ~840 DIP (exact figure in the stage report for the owner),
  whatever the panels show. Window-thread costs: designed out where stage
  7 adds them; pre-existing ones filed. Designer revised (1,192 lines;
  minimum ~835 DIP). Pre-existing costs: #104 commented (per list), #107
  (selection encode on start/end), #108 (Shift+End per-row calls).
  Design recheck: B2-B4 and all non-blocking fixed; B1 must search again
  only for a failure that `recovers()`; Insert/Space use `selectable`;
  panel messages re-placed in `layout()`; selection-size walk limited.
  Written as `.work/stage7-design-corrections.md` (lead, settled). Design
  ready for fm-implementer after stage 6's commit.
- Stage 8, first pass (Sonnet subagent, now): stages 3-6 rules into
  `docs/DESIGN.md` only; CONTEXT.md and help proposals into
  `.work/stage8-context-and-help.md`. fm-implementer must not edit
  `docs/DESIGN.md` meanwhile. Stage 7's rules and the spec's deletion
  come in the last pass, after stage 7.
  First pass done: `## Two panels` chapter in DESIGN.md with a stage 7
  placeholder; proposals file written. Spec/code differences settled:
  selection cleared on submit and the Keyboard page's Two panels note are
  defects, done with stage 7 (corrections 5-6); copy source reread and
  Exit wording follow the code. Last pass also moves stage 2's Search
  commands into DESIGN and retargets ~125 code comments citing the spec.
- Stage 6 Viewer window: an Opus subagent writes it now as new files only
  (`win/viewer.rs`, `app/viewer.rs`), unwired and unbuilt, plus a wiring
  list `.work/stage6-viewer-wiring.md` for fm-implementer to apply after
  stage 5's commit. Exclude these files from stage 5's commit. Accepted
  as is: `row_of`'s case-ignoring focus fallback (case-sensitive folders
  only).
- Stage 5 design revised for every design-review item (08:26), no
  disagreement; `DestinationMissing` text becomes "The destination drive
  isn't available."
- Session notes from fm-implementer wait while the lead's own turn runs
  (one long goal turn), so the lead checks `get_session` every 10 minutes.

## Stage 5 design review (2026-10-21)

- Design: `C:\Users\user\AppData\Local\Claude-Data\claude_pzs8sxrjxfjjc\side-session-notes\local_77081e65-b86b-47b9-80a6-5e84b6048b61\fm-implementer-stage5-design.md`.
- Review: needs revision, 2 high, 7 medium, 9 low. Findings and lead
  decisions (ended-line texts, Windows' name rules, row 1 in the dialog) in
  `.work\stage5-design-review.md`. The implementer revises the design
  before stage 5's code.

## Stage 7 code map (2026-10-21, Sonnet, unverified)

- `ResultItem.path` is the full path (`path_u16` only when lossy), so the
  Folder column is computed at cell time; middle-shortening belongs where
  the width is known (`draw_panel_row`, `TextMeter`).
- Per-list state to split: `Model.text`, `search_due`, `open_on_answer`,
  `results: Results`, `selection: Selection`, `search_kept`; purposes
  `Results` and `Selection` become per list (mirror `FolderSizes(Side)`);
  `Purpose::ALL` and `Requests::slot` grow with them; `show_failure` and
  `accept_status` write Search's results and selection and need a per-list
  decision.
- A search panel uses `Matching::default()`, `Filters { location, .. }`
  (This folder and subfolders = location, not only_directly; All drives =
  None), the panel's own sort, a per-side 150 ms timer, a Folder column
  with no sort key, and `ListMessage` texts.
- Every match on `panel::View` must decide the search variant (`count`,
  `row`, `has_up`, `read_again`, `accept_read`, `apply_changes`,
  `ask_sizes`, `quick_char`, `panel_coverage`, `PanelCaptions::new`,
  `on_panel_char`). `CommandId::Find` needs `belongs_to(TwoPanels)` and
  Alt+F7.

## Coordination improvements (owner asked twice, 2026-10-21)

- Each stage's design gets an adversarial design review before code
  (stage 5's design is under review during stage 4's fixes).
- Verified findings go to the implementer as each reviewer finishes; its
  one build waits for all fixes.
- While waiting on a session, check its state (`get_session` isRunning and
  its last message), not the count of changed files: on 2026-10-21 the
  implementer waited 47 minutes for an answer the lead missed.
- Isolated pieces run in parallel with a second writer that does not
  build: the Viewer's core (`core/viewer.rs`, new file, unwired) is being
  written now; the implementer wires and builds it in stage 6.
- `core/viewer.rs` written (Sonnet, 2026-10-21), never compiled. Wire with
  `pub(crate) mod viewer;` in `core/mod.rs`. It decides: Text/Hex choice,
  encoding (BOM, valid UTF-8 in 64 KB, else ANSI), decoding with U+FFFD
  (ANSI through a caller-supplied decoder), tabs, line starts reading back
  with 10,000-character pieces, Up/Down, a jump to the first line at or after
  a byte, Top/percent/End, hex rows, 32-bit scroll mapping beyond 2^31
  bytes. Its own resolutions of spec gaps (a jump goes "at or after"; UTF-16
  without a BOM reads as Hex; pieces after a jump are anchored at the jump)
  are reviewed with stage 6. Not committed with stage 4 or 5: leave it out
  of those commits.
- `core/viewer.rs` adversarial review (Opus): needs fixes (H1 a jump between
  CR and LF made an empty line; H2 runs of non-characters never ended a
  piece; M1 last piece cut differently up and down; M2 no malformed-input
  test). Writer fixed all; recheck by the reviewer: ready to wire, every
  literal checked by hand. Two low items sent back (a doc sentence, the
  proptest's byte distribution). Known residual, accepted: a line of 40,000
  bytes or more can be cut again at its last piece going up, which the
  spec's "reading back" rule implies.
- Wiring in stage 6: `pub(crate) mod viewer;` in `core/mod.rs`, and
  `proptest.workspace = true` under `[dev-dependencies]` in
  `crates/gozler-ui/Cargo.toml` (crate manifest, not the workspace one).

- Reviews of a stage run in parallel: an adversarial correctness reviewer
  (Opus) and a spec/prototype comparison (Sonnet), launched together.
- While a stage is implemented or fixed, the lead prepares the next one
  (code map, assumptions, brief) instead of idling.
- No sleep loops: act on session notes and agent notices.

## Pieces and estimate

The spec requires one implementer through stages 4 to 7, keeping its
context, and a reviewer that reads and reports. Stages run in order, so
there is no parallel split.

| Stage | Work | Rounds | Hours |
| --- | --- | --- | --- |
| 3 | rerun check.ps1, final review of the stage diff, commit | 1 | 0.5 |
| 4 | taken entries split; panels: reading, watching, sorting, selecting incl. Clear selection, quick search, drive menu, folder sizes, USB removal, status lines, key bar | 2-3 | 4 |
| 5 | Copy/Move dialog (V1), engine's two changes, New folder, Delete, menus; V2 and V3 in both modes | 2 | 2.5 |
| 6 | the Viewer | 2 | 2 |
| 7 | one state per list; search in a panel; Esc order | 2 | 2 |
| 8 | rules into DESIGN.md, CONTEXT.md, help; delete the spec | 1 | 0.5 |
| | total | 10-11 | about 11.5 |

### Re-estimate (2026-10-21, after stage 6's commit)

Spent so far: stages 3-6 committed; stage 4 took 7 review rounds instead
of 2-3 (selection and focus rules), stage 5 took 3 fix rounds, stage 6
took 1. About 12 h since stage 3's commit, against 9 h estimated for
stages 3-6. Remaining, done in order by fm-implementer (one builder):

| Piece | Rounds | Hours |
| --- | --- | --- |
| 7: code (running), review (adversarial + spec/prototype in parallel), fixes, recheck, commit | 2 | 2.5 |
| 7b: native-control corrections from `.work/custom-controls-report.md` (owner request 7); size known when the report lands | 1-2 | 1.5-3 |
| 8 last pass: stage 7 and 7b rules into DESIGN.md, CONTEXT.md and help, ~125 code comments retargeted, spec deleted (docs agent; no code) | 1 | 1 |
| final adversarial verdict on the whole feature, its fixes, recheck | 1-2 | 1-1.5 |
| total | 5-7 | about 6-8 |

Risk: 7b's size depends on whether the panel rows and the key bar move to
native controls; the panel rows' custom draw is load-bearing for the
taken-entries strike/blank and folder-size columns.

Each stage: implementer writes all code, one `-Test` run per new test,
reports; reviewer reads (spec, prototype screens, adversarial); implementer
fixes; a reviewer that took no part rechecks the fixes by reading; one
full `check.ps1`; commit on `file-manager`. Stage screenshots go to
`.work\stage<N>\`.

## Spikes

- Stage 4: the drive-removal notification (`RegisterDeviceNotification` /
  `DBT_DEVICEQUERYREMOVE` on the watch handle, or another), run once by the
  implementer. Not started.

## Status 2026-10-21 (after the merge tip 5c85cd5)

- `file-manager` fast-forwarded to 5c85cd5. Stage 8's DESIGN.md first pass: stash 5eb0509 and `.work/stage8-design-first-pass.patch`.
- Stage 7 recheck of f5573d9 (Opus, a4ab63ee): settled, every after-merge fix done. L5 recorded on #108. One new cost confirmed in code: a search panel's selection read calls `ListView::selected` twice and runs the deselect pass with an empty taken set; fix given to fm-implementer.
- fm-implementer: native panel rows (look B) plus that fix, on 5c85cd5; no build until fm-merge's check ends.
- fm-native: Settings section headings (SS_ICON + semibold static + SS_ETCHEDHORZ), dividers, Location list, Failed list path column, File operations card, #109; no build until told.
- Owner ruling 2026-10-21: no more tests. Main fast-forwarded to 7b94f06 (stages 3-7, key bar, main's 8da09ba/a098e86). The merge's last full run was stopped by that ruling; the run before it failed only at one clippy line, fixed in 5d6f7f3. main's 15 uncommitted entries were untouched (no overlap).
- Owner 2026-10-21: "finish all your work in here" — all remaining work lands on `file-manager`; main takes it in one merge after the owner commits on main.
- On `file-manager`: 6bb243a panel rows, a680253 native Settings/dividers/lists/card (lead resolved conflicts: DIVIDERS 4, ids 1720/1721, EraseDc removed, Viewer Background::Window), 8b02d66 status bar #109. Each compiled with zero tests, no warnings.
- Final review (Opus): does not pass. Fix round 1 (`.work/final-review-fixes.md`): fm-implementer. Waits filed #110, lows #111.
- Stage 8: fm-native. Part A (DESIGN.md, CONTEXT.md, resources json) now; Part B (156 code citations, delete spec) after fix round 1 commits.
- Then: recheck of the fix round by a reviewer that took no part; final verdict.
- Owner 2026-10-21: "stop waiting for commit on main. your task is here only." Done means: everything committed on `file-manager` and passed by the final review. The merge into main is the owner's.
- Owner 2026-10-21: "no building. we will build when we are done." No compile runs from here on; the owner builds at the end.

## Paused 2026-10-21 (owner: "pause it but not in an unstable state")

- `file-manager` tip after fm-native's docs commit: stages 3-7, 6bb243a, a680253, 8b02d66, 7be252f (final-review fixes, compiled once, no tests), and the stage 8 docs commit (comments and DESIGN.md only; no code line changed).
- Left to resume, in order:
  1. Stage 8 rest (fm-native, 30-45 min): DESIGN.md "The File operations page" (comments already cite it) and the Settings page list; Search's Copy to folder…, Move to folder…, Delete, Cut, Paste, File operations and Two panels rows in Commands and its Menus changes; Cut and Paste in "Copy"; CONTEXT.md terms; delete docs/plans/file-manager-spec.md and crates/gozler-ui/two-panels.prototype.html (check the prototype against git history first).
  2. Status line rule: a panel's status line leaves out the size when one folder size is unknown, and DESIGN.md's status bar says "size unknown". Both break the owner ruling "give the partial answer". Not written into DESIGN.md; decide fix or issue.
  3. Final verdict on 7be252f: Opus recheck (read-only) — see its result below when recorded. If it finds defects: one fix round with no build, then a reader.
  4. Owner builds (full check.ps1), then merges `file-manager` into main.
- stash 5eb0509 (fm-stage8-design-first-pass) is superseded by the docs commit; drop it when resuming.
- Recheck of 7be252f (Opus, read-only): **does not pass**. Fixed: H1, M1, M2, M3, M5 (no longer appends to the system list), M4 (within the 2 s check; no check when FileIdInfo fails at listing). Still open when resuming:
  - W1 partly fixed: taken entries still fold and allocate per batch, plus a new fold per taken name; filed as #112.
  - `..` row now shows Windows' open-folder icon with no up arrow (gdi.rs `SIID_FOLDEROPEN`), and 7be252f rewrote the spec's line to match without an owner ruling. The prototype and the plan ask for a folder with an up arrow. Fix: draw the up-arrow glyph into the window's own image list, not the shared system list, or ask the owner.
  - Remove `Glyph::Up` if the fix does not use it.

## Resumed 2026-10-21 (owner's ruling via the pause coordinator: finish the whole task)

- f34fbc0: stage 8 docs and comment retargets (lead committed; fm-native's session refuses commits). dbc2f10: Two panels prototype kept in history.
- Fix round 2 (fm-implementer, no build): #112, the `..` row's up-arrow look in a window-owned image list, the panel status line's "size unknown" (DESIGN.md: "never as a partial sum").
- Stage 8 rest (fm-native, no build, lead commits): File operations page, Search's Commands and Menus rows, Cut and Paste, CONTEXT.md, delete spec and prototype.
- Then: one read-only Opus review of fix round 2 and stage 8; then ask the pause coordinator (session "Agent work stabilization and pause") for the build slot; one check.ps1; write `.work/merge-into-main.md` (open findings, known conflicts with main, After install list).
