# Plan: the window's architecture

The shape `gozler-ui` should have for the code it holds now, and the
stages that get it there. First of all, no background operation may ever
stop the window: Gozler exists because Everything's window stops
responding while it works, and in Total Commander, changing to a
`\\server\share` that does not answer freezes the whole window until it
answers. Beyond that, the design stays: `core` decides, `win` wraps Win32,
`app` connects them, over native controls (DESIGN.md: "Where the unsafe
lives"). What changes: one owner for all background work, how the window
proves that it never waits, how it avoids flashing, how a change in the
model reaches the controls, where each part of the window keeps its
state, and the rules for writing window code.

The bar is decency, not smoothness: the window never freezes, never makes
a person wait for what should be instant, and never visibly flashes or
shows garbage. 60 or 120 frames a second is no goal, and work whose only
gain is smoothness a regular user would not notice is cut or made
conditional on evidence.

## Schedule

**Implementation authorized on 2026-10-23.** Execute stages in order. The lead
owns shared contracts, coupled changes and the only shared-checkout build.
Delegate substantial independent batches with exclusive file ownership and
retain agents across related work. Use codebase-design, domain-modeling and
rust-win32-ui; the owner has dropped implementing-plans. Keep coupled work with
one implementer. Each completed feature batch or integration milestone gets one
independent adversarial review of requirements, execution paths, integration,
ownership and material complexity; stages need not wait on unrelated preparation.
Reserve code-review deep and its six specialists for final integration.
Batch compatible fixes for the same reviewer; follow-ups cover fixes and
affected contracts, expanding only on evidence. Reuse existing valid evidence. Commit each
completed stage, as the owner's active goal authorizes.
Installation remains a separate owner decision.

**Module structure integrated on 2026-10-23.** This plan incorporates the
reviewed `window-code-shape.md` proposal and is the single execution authority.
Its ownership rules guide new work now; existing responsibilities move at their
scheduled stages below. This adds no separate restructuring stage or duplicate
migration. The proposal's independent architecture review was clean; its source
observations establish design constraints, not completed implementation or
runtime verification.

### Execution status

**Stages1–3 and5–7 are complete;4 and8–9 remain incomplete.** The architecture
is implemented and independently reviewed, but the visual gate is open.
Retained commits:5fc49ca (architecture),7e0bfb1 (native layout),570711d
(idle caption and structural mode/card redraw). Full51 is green: formatting,
clippy, workspace tests and docs; UI282 passed,14 ignored. Full attempts:51.
Log: `V:/temp/gozler-window-integrated-build51.log`.
Previous resumption used three full runs:48 failed only under the restricted
token;49 and50 passed outside it. This continuation used one full run,51,
outside the restricted token, after the retained batch and source reviews.

**Causal investigation completed.** Three disposable focused attempts:
cause1 stopped before READY because keyboard foreground was refused;
cause2 completed the sequence/copy but exposed a recorder-stop deadlock;
cause3 passed17.22seconds, including a real8MiB copy. The corrected fixture
uses only its own temporary topmost HWND, verifies crop ownership/bounds at
96DPI, and stops desktop recording before that window closes. No foreground
policy, service or installation changed. The trace is saved before stdout.

Cause3 records163 HWND-WGC frames and570 independent screen-DC frames,
135 exact-pixel duplicate groups, across the same finite resize/splitter/
typing/status/mode/copy sequence. All changing frames were inspected.
Desktop ROI confirms partial publication: toolbar fragments30/37, duplicate
panel counts327/333, shared-pane opening414, completion caption clearing502.
These are not explained solely by WGC frame-pool resize. WGC alone still does
not prove monitor scanout; the bounded desktop ROI does not cover Clear.
No claim of atomic capture, physical scanout or a speedup is made.
Reports:
- `V:/temp/gozler-window-paint-cause/paired-media-audit.md`
- `V:/temp/gozler-window-paint-cause/native-state-audit.md`
- `V:/temp/gozler-window-presentation-constraints.md`
- `V:/temp/gozler-window-cause-visual-wgc-cause3.log`

All164 native snapshots/1,393 HWND records were parsed,0 dropped. Sampled
structural mode/view exits are native-correct and clean; ordinary layout/
progress can return with dirty regions. The81ms cardNone state is a genuine
model transition with a correctly shown idle caption. No persistent native/
model corruption is demonstrated. Earlier pending-only paint flush gave no
overall correction and remains removed. Before this new production batch,
all448 Full50 input hashes matched again except this plan; no probe hook
remains in the product source. Historical capture artifacts remain saved.

#### Current batch and ownership

**Settled lead contract:** combine desired geometry and visibility in one
per-parent DeferWindowPos application, using native SHOWWINDOW/HIDEWINDOW.
A visibility change must survive unchanged-rectangle filtering. There is one
positioning implementation; existing move_children delegates to it. Feature
owners compute visibility once and MainWindow coordinates layout. No stored
presentation, model copy, dispatcher, compositor, custom paint or display
test is added. NOREDRAW is conditional on a demonstrated positioning paint
cause and complete affected-region restoration, never an unowned bit change.
Also restore WS_CLIPSIBLINGS on the transfer child-window constructor,
consistent with the other child windows.

The retained native-control implementer completed the coupled batch in win/mod.rs,
win/window.rs child creation, MainWindow layout, required Panel/Inspector
visibility helpers, Search's message placement, and app/file_operations.rs
placement/view code. MainWindow's existing message geometry gains a collection
form reused by its callers; no message/focus rules are duplicated. Native pane
WM_SIZE reentry and readiness before reveal remain required contracts. Root owns
shared contracts, docs, integration and the only build. Foreign status work
remains intact. The seven-file batch is frozen and root's source read plus
the independent six-pass affected-contract delta are clean after one correction:
`V:/temp/gozler-window-native-publication-review2.md`.
Publication1 exposed a real first-open regression: optional View availability
incorrectly gated pane visibility. The one-line correction restores visibility
from the requested layout; an available current View is still applied first.
No copied idle state, Queue snapshot query or follower scheduling change was added.
Publication2 passed19.14seconds, including the real8MiB copy and completed shared
pane. The two disposable module includes were removed before green Full51.
No permanent test or Bench run was added. The native observation watches first
open, actual transfer completion and layout publication, rather than getters.

All516 desktop frames/126 changing groups and169 HWND frames were inspected.
Final idle and Completed views are coherent; the earlier white control outlines
and caption-cleared completion gap were not sampled. First reveal has one blank
gray sample (33ms movie retention); preparation has one partial sample (50ms).
Resize toolbar/count fragments remain. These captures establish intermediate
publication, not persistent wrong native endpoints or physical scanout. Practical
normal-speed acceptance remains unverified; do not claim all captured frames
became atomic or delete the plan on the strength of a functional pass alone.
Media: `V:/temp/gozler-window-paint-cause/publication2-media-audit.md`.
Permanent Painting documentation describes the implemented rules, independently
reviewed clean in `V:/temp/gozler-window-native-publication-doc-review.md`.
No further diagnostic expansion or unproven paint mechanism is authorized by
these observations alone; use the existing evidence for the next decision.

Existing proofs remain reusable within their scope: native attempt6 selected2
by Ctrl+click and100,000 by Shift+End, and verified an ended move's Summary,
destination and absent progress bar
(`V:/temp/gozler-window-final-native-proof6.log`). Its layout handler timings
are not a claim about the upcoming batch. The earlier successful physical OLE
drag/watchdog evidence remains valid; do not repeat it. Attempt5's nonreturning
native boundary remains unknown. Suspected shell-icon waits remain in#148.

Stages4/8 still need the visual criterion satisfied. Stage9 permanent ownership
rules are already recorded; delete this plan only after all required proof,
review, build and stage commits pass. Environment checks remain explicitly in
ui-after-install.md; installation is a separate owner decision. No owner
question or UAC step is needed for the current batch.
All six final specialist
reviews finished: five clean; flow found one Panel cancellation defect. Its owner
fixed recovery after navigation cancellation and explicit Retry after cancelling
that recovery; both app entry points now share the Panel cancellation owner.
The first delta found three related paths (clean folder watch, Search observation,
later operation end after Cancel); their batched correction is frozen and clean
in `V:/temp/gozler-window-final-cancel-delta-review2.md` (all six affected-contract
passes, including the documentation corrections). Small corrections are batched before
the next full build. Exact own-only candidate blobs preserved foreign status/date
work while producing the coherent architecture commit `5fc49ca`.

- **Stage 6:** feature/control ownership, private Panel row operations and typed
  Work groups are implemented and source-reviewed. Areas own controls and native
  interaction; shared facts stay in Model. Accepted Location/label facts belong
  to consumers, while Work retains execution inputs. Queue retains engine
  scheduling and recovery. Source reviews remain valid; Full46 verifies the
  integrated code; Full47 and `5fc49ca` complete the retained source batch.
- **Stage 7:** Model has one `Accepted { touched, effects }` contract; no
  Change/Then compatibility path. MainWindow applies structure, refreshes touched
  areas, then delivers ordered actions. Native reentry coalesces repair through
  the existing Deferred wake. Explicit Viewer state transfer replaces refresh
  mutation. Settings, Viewer and synchronization keep their own scoped state.
  Prerewrite source: `V:/temp/gozler-stage7-before/`, repository-relative paths.
  Final handoff: `V:/temp/gozler-window-feature-owners.md`.
- **Stage 7 review is clean:** `V:/temp/gozler-window-stage7-review3.md` merges
  reviews 1–2 and their hashes. It resolves off-UI late retirement after Destroy,
  hidden-focus repair after unrelated Status, unchanged Transfers unmaximize,
  explicit HomeFocus on ordinary mode switches, and preservation of a newer
  visible focus. Subsequent narrow records correct compile imports and test
  generations, and restore Polling/NoteSavedState after accepted rows:
  `V:/temp/gozler-window-stage7-compile-correction-review1.md`,
  `V:/temp/gozler-window-stage7-row-tail-review1.md`,
  `V:/temp/gozler-window-stage7-full45-generation-review1.md`.
  Full46 verifies compilation and tests; native measurements passed attempt6
  with the fixture/boundary limits recorded above.
- **Reentry identities:** FirstRow retains shown-results Generation; deferred
  Cancel compares Requests.current with Model's awaited identity. Repair dispatch
  and replies share one Model intent Generation, replacing Work's separate
  temp-folder counter. A cancelled A reply cannot advance B before B starts.
  Rejected Ready uses the existing Event cleanup; late nonempty retirement uses
  existing win::work::spawn after close transfers the normal retirement Task.
  Empty retirement starts no thread. No new queue, focus generation or framework.
- **Stage 8:** native getter guards and unreadable marquee/shield flags are
  implemented. Search caches only shield state; glyph-only buttons use native
  readback. No new painting surface or visual-success claim. Reports:
  `V:/temp/gozler-window-stage8-shield-glyph.md`,
  `V:/temp/gozler-window-stage8-shield-review1.md` (clean).
- **Stage 1 source-completion correction:** one second total from source-cleanup
  start is allowed only by the off-UI finalizer, using the remaining budget.
  Source feedback never delays essential writes or UI close; frozen COM and
  sources behind it remain best effort. Audit and clean review:
  `V:/temp/gozler-window-source-completion-exit-audit.md`,
  `V:/temp/gozler-window-source-completion-review1.md`. Full46 passes its test.
- **New pure regressions, all passed Full46:**
  `source_completion_gets_one_grace_period_without_delaying_later_exit`,
  `a_nested_update_supersedes_presentation_but_keeps_a_file_command_once`,
  `a_cancelled_fix_reply_cannot_advance_the_next_fix`. No display/getter tests.
  The earlier Inspector age invariant also passes. Formatting policy reuses an
  existing test; no display test was added.
- **Stage 9:** DESIGN contains actual owners, bounded selection, work policy,
  shutdown/source grace, three-phase updates, rules 14–21 and native painting
  rules. AGENTS points to that implemented owner, preserving engine scheduling.
  Crate/module comments describe retained responsibilities. The latest doc audit
  confirmed native buffers, changed-rectangle batching and redraw erasure/frame/
  child flags from source. Watchdog documentation describes the retained mechanism.
  Final review fixes, required full verification and own source/doc commit are
  complete. Native timing/recordings and deletion of this incomplete plan remain.

#### Verification gate and foreign changes

**Verification resumed with an explicit owner format decision.** Full44 passed
formatting and earlier compile fixes; 270 UI tests passed, 14 were ignored, and
one foreign test expected `1 KB` while production returned `1.00 KB`.
Log: `V:/temp/gozler-window-feature-owners-build44.log`.

**Settled October 24:** compact bytes and KB use whole numbers; decimals begin
at MB. Root changed the existing canonical text::bytes precision branch, leaving
MB+ precision, truncation and unit promotion intact. Existing compact-size and T7
expectations follow the deliberate behavior change; no test was added. The
foreign status assertion remains untouched and now matches the requested rule.
SizeFormat's fixed-unit columns retain whole Bytes/KB and two-decimal MB.
No additional precision setting is implemented. Narrow source review precedes
verification: `V:/temp/gozler-window-size-precision-review1.md` (clean).

Full45 passed fmt/clippy/docs; UI 274 passed, two failed, 14 ignored. One own
FirstRow expectation used the older generation and is corrected/reviewed. The
other failure was foreign date-header work introduced during the run: its owner
subsequently added the missing catalogue keys and formatting. Root restored the
two attempted short-header edits so that foreign work stays intact. Permission
failures in fixture pipes, ACL manipulation and working-set privileges match the
earlier restricted-runtime failure; no client/engine/winfs code was changed.

Full46 ran outside that restriction and is **green**, including fmt/clippy/tests/
docs, UI **277 passed / 14 ignored**. Log:
`V:/temp/gozler-window-integrated-build46.log`.

The owner confirmed readiness after a green build. The physical OLE check then
**passed**: down/movement, GiveFeedback entry, acknowledgment 81 to 82 while
inside the loop, final DRAGDROP_S_CANCEL, return and no timeout. Log:
`V:/temp/gozler-window-watchdog-interactive-focused2.log` (12.72 s).
Its first compile lacked windows-rs' WindowsProgramming feature; enabling that
existing dependency feature fixed it. The disposable include is removed; no test
window/process remains. No stall log was produced. The retained watchdog is
integrated and covered by the six source reviews; its three pure policy tests
await Full47. The transport evidence does not prove posted-input readiness in
every cross-thread sent-message wait; DESIGN records that diagnostic limit.

Native proof: the first disposable timing run compiled but failed creating its
window, before measurements: its fixture omitted pane-class registration. The
combined `-Timing timing_window_` then opened and closed its visual host, but
the timing test failed on duplicate pane registration in that same test process.
Log: `V:/temp/gozler-window-final-native-proof2.log`; original failure:
`V:/temp/gozler-window-native-timing.log`. Both attempts are terminal failures;
no timing measurements passed. A third attempt then caught three fixture-only
compile errors (DialogItem/PCWSTR imports and a UTF-16 assertion); all corrected
together in the disposable files. The fourth attempt is running:
`V:/temp/gozler-window-final-native-proof4.log`. Run the two filters in separate cached
test processes, without registration-sharing machinery. Foreground visual input
proved unreliable and consumed time; its partial recording is not proof. The
existing fixture owner prepares the plan's bounded handler-driven paired visual
sequence under V:/temp, with no product scaffolding. The historical source is an
immutable archive of `74128aa` under V:/temp. Its ordinary `-Release` attempt
could not publish over running normal executables; those processes were left
alone. An isolated test host avoids replacing them. No timings or visual success
are claimed yet. `V:/temp/gozler-window-own-stage/` holds reviewed own-only
candidate blobs, excluding foreign status/date work; an isolated exact-HEAD copy
with those blobs is running `-Test core::`, log
`V:/temp/gozler-window-own-only-focused.log`. This verifies the independent commit
contract rather than relying on the mixed worktree's earlier green build.
All old fixture includes are removed. Full47 is **green** against retained
integrated source: fmt, clippy, workspace tests and docs; UI **282 passed / 14
ignored**. Log: `V:/temp/gozler-window-integrated-build47.log`. The two Panel
cancellation regressions and three watchdog regressions passed. Only this plan
changed during the run; all other 448-input snapshot entries remained stable.
Reuse that full
result only after later disposable includes are removed and retained source hashes
match `V:/temp/gozler-window-full47-inputs.json`.

Own-only `core::` compiled and ran 260 tests: 259 passed, including the two Panel
cancellation and three watchdog regressions. Its sole failure was a foreign
status expectation still present in the reconstructed candidate; restore HEAD's
`fixing.done` assertion only in that candidate. The focused recheck passed:
`V:/temp/gozler-window-own-only-focused2.log`. The integrated foreign behavior is
untouched. Candidate formatting is checked; no new product fix/review cycle.

Focused runs since Full44: `compact_sizes_limit_precision` first failed on own
Stage 7 compile omissions, then passed after the corrections;
`core::` passed 258 tests; `native_watchdog_interactive_drag_evidence` first
failed compilation for the feature above, then passed. Logs use respectively
`gozler-window-size-precision-focused[2].log`,
`gozler-window-stage7-core-focused.log` and
`gozler-window-watchdog-interactive-focused[2].log`, all under V:/temp.

Full44 also found two own lints and a moved doc link. These are corrected:
private Panels helper, one MainWindow transfer orchestration body with its
original argument count, InspectorFit's comment beside its owner. Correction
review `V:/temp/gozler-window-feature-integration-review4.md` is clean; corrections
passed Full46. Source hashes stayed stable during Full43 and Full44.

Another session owns status-bar/catalogue/resource edits, its Model test hunks,
MainWindow's indexing-entry map and DESIGN's status-bar descriptions. Preserve
and exclude them from commits. Later staging must select own hunks in Model,
MainWindow and DESIGN. Commit `5fc49ca` contains the own-only candidate blobs and
plan checkpoint, including the verified watchdog; foreign contracts remain in
the worktree. The candidate-only status assertion
correction and formatting are included. Foreign date-column work also touches text.rs, dates.rs,
the Search/panel captions and controls, and Model's short_header/.full hunks;
preserve and exclude those alongside the earlier status-bar changes.

#### Completed verification and history

| Retained milestone | Commit | Full runs and result | Evidence |
| --- | --- | --- | --- |
| Work policy, bounded passes, drop intake and shutdown | `a86ff43` | 33–35; 35 green, 260 UI passed / 14 ignored | `V:/temp/gozler-window-stage1-integrated-review2.md`; Stage 1 logs |
| Worker migration and shutdown ownership | `f66011f` | 36–38; 38 green, 262 / 14 | `V:/temp/gozler-window-stage2-build38.log`; worker review 3 |
| Slow-work waiting and cancellation | `74128aa` | 39 green first run, 268 / 14 | `V:/temp/gozler-window-stage3-waiting-build39.log` |
| Native waits and control owners | `47f06cc` | 40 green first run, 269 / 14 | `V:/temp/gozler-window-native-wait-controls-build40.log` |
| Model state, feature captions and marquee ownership | `f5c8ec3` | 41–42; 42 green, 269 / 14 | `V:/temp/gozler-window-state-areas-build42.log` |
| Feature owners, Work groups, Panel rows, explicit updates and watchdog | `5fc49ca` | 43–45 corrections; 46 green; 47 green, 282 / 14 | `V:/temp/gozler-window-integrated-build47.log`; six final specialist reviews and cancellation delta review2 |

Current retained source reviews are clean:
`V:/temp/gozler-window-work-groups-review1.md`,
`V:/temp/gozler-window-panel-row-review1.md`,
`V:/temp/gozler-window-feature-owner-review2.md`,
`V:/temp/gozler-window-feature-integration-review3.md` and review4 above.
Work contract: `V:/temp/gozler-window-work-groups-contract.md`.
Setter audit: `V:/temp/gozler-window-stage8-remaining-setters.md`.
These prove reviewed source, not unobserved runtime behavior.

Detailed historical status through Full44 is retained at
`V:/temp/gozler-window-execution-history-through44.md`; earlier checkpoint:
`35919d6`, with `V:/temp/gozler-window-stage1-*` logs. Full33–35 include the
restricted-environment pipe ACL/working-set failure corrected by an elevated
Build35. Full41 caught two panel integration omissions fixed before Full42.
Full43's import, caller, private threshold and timing reexport corrections passed
focused source review before Full44. Later verification is recorded above.

Accounting beyond full builds: six historical focused attempts through Stage 1,
then one focused native-watchdog attempt and five later focused attempts
(twelve total, including compile failures), plus the current own-only `core::`
attempt (thirteenth) and its successful single-test recheck (fourteenth); one legitimate
`-Timing timing_large_panel_selection`, two disposable watchdog timing attempts,
and native attempts1–6 (attempt6 passed; earlier failures retained above).
Paired GDI and WGC runs each passed both real-copy sequences; GDI appearance
outside the client is not used as proof. Correction1's first compile failed
only in disposable diagnostics, then its recorded run passed behavior but
failed the visual bar. Correction2's source delta/recheck precedes its one
final recording; one baseline
`-Release` attempt failed at locked executable publication; **no Criterion runs**.
Stage1's successful product-focused run:
`-Test a_change_batch_keeps_the_taken_names_folded_once`,
`V:/temp/gozler-window-stage1-cache-focused6.log`.
The selection timing observed chosen-source medians 0.969 / 70.565 ms and counts
0.017 / 1.861 ms at 1,000 / 100,000 entries, supporting `INLINE_ITEMS = 1,000`,
not a before/after speed claim. Baseline and review:
`V:/temp/gozler-window-stage1-pass-baseline.log`,
`V:/temp/gozler-window-stage1-pass-timing-review1.md`.

Lasting regressions through Build35 cover admission/stale replies, disposal
affinity, inline/deferred equivalence, settings merge/coalescing, early WM_QUIT
versus finalizer completion, clipboard retry, panel reconciliation/overflow/end
evidence, native capture replacement, quick-search focus, operation receipt
facts, id reservation and terminal Queue delivery. Stage 2 adds only
`a_cancelled_ready_setup_is_removed_only_by_retirement` and
`window_code_has_no_unowned_launcher_or_recognized_wait` (passed 37–38).
Slow-work/native batches add lasting cancellation and hover-expiry cases; the
Inspector age-timer invariant passed Full44. Existing caption/row tests moved
with their owners; no display/getter tests added.

#### Settled contracts and deferred proof

- `core::work` owns policy and inline size; `win::work` alone constructs worker
  capabilities and owns bounded execution/native cancellation/live delivery;
  `app::work` routes typed purposes and results. Cancelling blocked work keeps
  its slot; pending replaceable work coalesces. Large inputs, replies and final
  drops retire off UI. Queue owns operation scheduling, cancellation and recovery.
- Saves serialize changed-key/removal/scope intent and preserve unrelated disk
  keys and later edits. Accepted mutations/export cleanup retain off-UI process
  lifetime independently of essential settings/recovery writes. Normal Exit
  destroys UI immediately; session end pumps at most two seconds, checks explicit
  finalizer completion and preserves early WM_QUIT for the outer loop. Windows
  sign-out may terminate unfinished persistence.
- Queue is adopted only after restoration. Its session serializes commands FIFO,
  reserves ids atomically and separates ordered/terminal acknowledgments from
  ordinary reply pressure. Instance eligibility never substitutes for readiness.
  Persistent Viewer/panel sessions keep blocked slots until actual cleanup;
  native interruption alone does not end a session. Device registration uses a
  bounded UI acknowledgment while the worker retains and closes native resources.
- Shared snapshots and bounded commits preserve selection, Taken, epochs,
  receipt-time operation facts and listing cutoffs. Panel watch overflow reports
  terminal Lost after its accepted FIFO prefix. Native selection capture/apply/
  deselect yields; commands/menus retain original side/list/focus. Filtering
  blocks commands until accepted; transfers apply incrementally and keep displayed
  targets. Program parsing precedes publication locks; slow work stays outside.
- Shell purposes retain and pump their STA without borrowing the main HWND.
  Clipboard retry is finite and sequence-aware. Drop data and capability COM
  references remain distinct; accepted setup/GetData are FIFO with setup ack
  first, right-drop destination is frozen, pending cancellation cannot restart
  late, and async source completion has one obligation. Unknown in-process STA
  providers may dispatch onto UI; failed marshaling has no UI GetData fallback.
  Native drag imagery stays UI-affine. Do not claim universal provider safety.
  Contracts: `V:/temp/gozler-window-stage1-drop-intake-contract.md` and
  `V:/temp/gozler-window-stage1-drop-integration.md`; shell lifetime/contract/probe
  reports use the same Stage 1 prefix. Disposable shell fixture was removed.
- Awaited work reports waiting/Cancel after three seconds without expiry;
  background observations expire silently at ten seconds while retaining their
  execution slot. Settings cancellation stays scoped to page and purpose; active
  noncancelable repair is not obscured. User cancellation never cancels accepted
  protected writes or unrelated file operations.
- Watchdog transport is **accepted by native evidence**. Synthetic attempt 3
  proved menu, dialogs, sizing/moving and controlled-block behavior; the physical
  check above proved OLE GiveFeedback and heartbeat delivery during the drag.
  Reuse both records; no synthetic retry or recurring native regression suite.
  The retained implementation is integrated and source-reviewed; removal of the
  transport probe's disposable include is complete. Earlier evidence:
  `V:/temp/gozler-window-stage3-watchdog-evidence3.log`.
- Stage 4 conditional painting needs the same observed interaction before/after,
  baseline `74128aa`; existing approved painting remains, no new painting added.
  Environment/visual checks remain in `ui-after-install.md`, unrun. Native edit
  clipboard wait is suspected and tracked as #147. A scan is only a backstop.
- Ubuntu/platform cfg, core purity, MSRV and affected cargo-deny checks remain
  with CI. Latest batches add no package dependency or wire layout. Existing
  narrow `fetch_update` allowance remains until its replacement is available at
  the project's MSRV (1.94 versus 1.95).
- Preserve engine/shared transfer, synchronization commit, indexing and early
  large-file contracts. The original source analysis was against `ab0e9b0` on
  October 23; recheck present callers rather than treating that analysis as proof.

## Why

Historical measurements and source observations on `main` at `5146bc2`:

- **The window still waits on disks and other programs.** Five waits on
  the window's thread are filed: Settings reads every drive's label, every
  2 seconds on the Drives and service page (#140); a search with a
  Location checks the folder on disk 150 ms after each key (#141); Open,
  Explore, Properties, Open file location and the open commands call the
  shell (#142); `ui.json` is saved (#143); opening the clipboard sleeps
  while another program holds it (#144). More are listed in stage 1. Each
  can freeze the window on a sleeping disk or a share that stopped
  answering.
- **Nothing enforces that the window never waits,** and background work
  has no owner. About 19 places in 12 files start a thread, each with its
  own rules for cancelling, waking the window and ending at exit: the
  requests, the panels' folder threads, the Viewer's reader, the Include
  save, Paste, the entry checks, the update check, the service check,
  Indexed folders, the clipboard, drops and the keyboard hook. What a slow
  task shows, whether a person can cancel it, and what happens at exit
  differ from window to window, and a new window would repeat them again.
- `app/main_window.rs` and `app/main_window/panels.rs` hold 7,900 lines;
  `core/model.rs` holds 3,700 and `core/panel.rs` 4,050. Every feature
  adds to `MainWindow` and `Model`.
- A model method answers `Accepted { change, then }`. `Change` has 23
  arms that are not exclusive: `Status` refreshes the status bar, the
  locations, the message and both panels, most arms end in
  `update_status`, and most arms also act, such as `Message`, which clears
  the list's selection and count. By the rule for modeling state, arms
  that overlap and repeat a condition are the wrong model.
- `then: Option<Then>` allows one follow-up, so compound follow-ups hide
  inside one variant (`ReadSelectionSoon` cancels a request and starts a
  timer) or inside `Change` arms.
- Many setters in `win` set a control only when its value differs, by
  reading the control: `Hwnd::set_enabled`, `Hwnd::set_visible`,
  `CheckBox::set_checked`, `Toolbar::set_checked` and `set_enabled`,
  `StatusBar::set_text`, `set_parts` and `set_icon`,
  `IconList::set_items`, `move_children`. Others set every time:
  `Hwnd::set_text` (labels, the title), `Toolbar::set_text`, which also
  sizes the buttons again, `Toolbar::show_images`, `StatusBar::set_tip`,
  `IconList::set_icons`, `ListView::set_small_images`,
  `ListView::set_header`, `Edit::set_cue`, `DatePicker::set_day`,
  `Button::set_icon`, which also sends `BCM_SETSHIELD`, and the progress
  bar. The old File operations window set every card text on each poll.
  The current transfer pane's `show_text` already compares text, and
  `ProgressBar::set_measured` already checks its position. Stage 4 audits
  remaining setters, including `set_moving`, instead of repeating those fixes.

## What DESIGN.md says that does not hold

1. **"A view diffed against the controls would keep a second copy of the
   selection and the scroll position."** A second copy is the wrong
   mechanism, but not for that reason: a copy of what was set drifts from
   the control. A check box and a toolbar check button toggle themselves
   on a click, an icon's bitmap depends on the DPI, the system colours
   and Show icons, and a Settings page is made from its template each
   time it first shows. The control itself is the record of what it
   shows. The code relies on that in many setters; use cheap native reads
   or a caller-established change instead of a second diffed view.
2. **"`WM_SETREDRAW` is turned off only around a change that would draw a
   control several times"**, and **"the main window erases only its own
   background between controls, once after each layout."** Both may cost a
   visible flash on a resize step: `draw_once_after` erases the
   double-buffered list before it paints, and the whole window is erased
   after each layout. This is suspected, not measured. A list view keeps
   its `WS_VISIBLE` bit while its drawing is off (tested in a comctl32 v6
   window), so the main window still clips it while it moves. The list
   moves inside `draw_once_after` on purpose, so a narrower window does
   not flash its horizontal scroll bar, and a resize step measures about
   12 ms of layout and 22 ms of list paint
   (`docs/research/measurements/window-resize`). The painting session
   tested the `WS_VISIBLE` claim on 2026-10-22 in a small comctl32 v6 test
   window: after `WM_SETREDRAW` off, a list view still had the bit and a
   static did not. So stage 4's first
   recording decides: these change only if it shows a flash a regular
   user would see.

These hold and stay: native controls, no framework, no
`WS_EX_COMPOSITED`, no free functions over a global `RefCell`, captions as
functions of facts (window rule 12), dialogs queued as modal steps
(window rule 13), one thread and connection per request (window rule 3),
one crate. `core` builds and runs its tests on Ubuntu on each push to
`main` and each pull request (`ci.yml`), so the rule that it holds no
Win32 type is already enforced; a crate of its own would save compile time
only by an amount nobody has measured, and costs the dead-code warnings a
library's `pub` items lose, the catalogue-key test's reach into `app` and
`win`, and new entries in `deny.toml` and CI.

## Decisions to keep, and why

Each decision below was reached by checking the code, after three rounds
of review and the owner's answers. When this plan is merged with another
version, keep each one, or replace it only with something that prevents
the same failure.

| Decision | Why | What breaks without it |
| --- | --- | --- |
| Nothing stops the window is the first rule and the first stages | Gozler exists because Everything's window stops responding; in Total Commander a `\\server\share` that does not answer freezes the window (owner ruling "the window never lags") | The one failure that makes Gozler pointless comes back through any single call |
| The bar is decency, not smoothness (owner) | A regular office user, not a gamer | Effort spent on smoothness nobody notices, while freezes wait |
| One UI work-policy owner used by every window, observing the existing engine owner | UI launch sites repeat slowness, replacement and exit rules; Queue already owns engine work | Each new window repeats those rules or duplicates the engine's lifecycle |
| `core::work` holds UI policy, `win::work` launches UI workers, `app::work` connects areas; Queue retains its internals | Pure rules run on Ubuntu and native worker launch stays below app | Untested rules, layering reversed, or two engine owners |
| Cancel by doing something else in the same place (owner) | A new search or another folder replaces the work; the person never waits for work they left | Stale work runs on, or a click elsewhere cancels work the person wants |
| Work that changes files is cancelled only by its Cancel (owner) | A half-done copy lost to a typed search is worse than any wait | Lost or half-done copies, moves, deletes and exports |
| Cancel shows only when a task the person waits for is slow, where its result will appear (owner) | A fast task needs no button, and a slow one needs a way out | Flashing buttons, or no way out of a dead share |
| No automatic time limit for work the person started; background work gives up silently (owner) | A slow share may still answer and the person decides; nobody waits for background work | Results thrown away, or messages about work nobody asked for |
| Known waits move first, onto the owner (stage 1) | They are what a person feels; the tooling protects later work but gives nothing alone | Months of tooling while the freezes stay |
| A privately constructed, `!Send` `OffThread` token at the UI worker entry point | UI code cannot construct it; marked waiting calls require it. Bounded inline CPU passes receive no I/O capability | `!Send` alone permits UI-thread construction, or an inline pass gains permission to block |
| A test scans `app`, `win` and `core` for waiting `std` calls, a named list of waiting Win32 calls and of other crates' waiting functions; thread code only in `…_thread` modules or under `work` | The token guards only marked functions; the Location check lives in `gozler-client`, the settings save in `core` | Unmarked waits, such as #141 and #143, ship unseen |
| No join of a thread that may still run; at exit the window hides, tasks are cancelled and released through the `OpenWaker` flag | A join waits for whatever the thread waits for, such as a permission prompt or a dead share | Exit hangs |
| The watchdog's dedicated-window heartbeat is verified in each applicable native loop before use | A main-loop hook alone misses native loops; pure tests cannot prove modal dispatch | Every open dialog or border drag logged as a hang, and real hangs mislabelled |
| The watchdog logs CPU-cycle and wall-time deltas separately, repeats while a stop lasts, records only message/control IDs, writes from its own thread and caps the log | The measurements help investigation but their units cannot be compared directly; window text is private and logging must not block the UI | False diagnoses, short readings of long hangs, private text in a log, or a log that is a wait |
| The owner's day of use and the field checks go to `ui-after-install.md` | AGENTS.md: what only the owner can check waits for no stage | A stage that stalls on the owner |
| Painting starts with a recording, and its riskier changes happen only if the recording shows a flash; one recording before and one after, each run once and saved for the owner, never a loop (owner) | The list moves inside `draw_once_after` to stop a scroll bar flash; the suspected flashes are not filmed; a resize step is about 34 ms; paint counts cannot tell a visible flash from the list's own erase and paint on every step | A flash brought back to fix one nobody saw, or windows flashing in front of the owner for an hour |
| `RDW_ERASE` stays in the main window's `draw_once_after` | A mode or language switch must erase the other mode's pixels | Garbage after a mode or language switch |
| Toolbar double buffering is checked in light, high contrast and 150 % DPI, and dropped if no blink is filmed | A double-buffered transparent toolbar can paint a wrong background | Black or wrong toolbar backgrounds |
| A list's changed column widths are set together, inside one `draw_once_after` on that list | Each `LVM_SETCOLUMNWIDTH` paints the whole list and header before it returns, twice when the scroll bar comes or goes (5–8 paints per panel list before `set_panel_widths`) | The paints that f96343c removed come back |
| One `DeferWindowPos` batch for each parent | A batch takes only windows of one parent; the filter pane's controls are the pane's children | The batch fails and every control moves and draws on its own |
| The owner has an inline size beside its slow constant; every pass over the selection, the fetched rows, a folder's entries or an operation's sources goes through `work::pass`, inline at or under the size and as a task above it | The token and the scan see calls, not loops, and the watchdog sees only stops over 200 ms; a 60 ms pass over 100,000 rows is a wait for what should be instant | Each feature decides again whether its loop is too long, and the next pass over 100,000 rows ships unseen |
| Large passes use shared snapshots and `work::pass`; inline callbacks have bounded CPU data, not the I/O capability | Preparing, applying and retiring a pass must also fit its UI budget; arbitrary loops still need review | A clone, destruction or unchecked loop over 100,000 rows holds the UI |
| A selection whose taken-file filter is a task has its own state, which unhighlights nothing until the answer arrives; the answer is filtered again if the taken entries changed meanwhile | `Selection::Reading` already means a gozlerd request, and `deselect` maps it to the row-by-row pass; the taken entries can change while the task runs | About 300,000 messages for a whole-selection delete, or a command acting on files a running operation has taken (window rule 10) |
| A pass that sends messages to a control stays on the window's thread in a one-message form or bounded by `SELECTION_LIMIT` | Only the thread that owns a control may send it messages | A row-by-row pass over 100,000 rows, as before 730f93a's `Deselect::All` |
| Recovery registration stays at engine admission, conversion on its worker; a write of `operations.json` waits for its initial read | Current Queue fixes the start time and immutable request before first mutation; its close acknowledgment includes explicit cancellation | UI path conversion returns, or recovery loses interrupted or cancelled operations |
| Exit closes the UI immediately; essential settings/recovery completion may retain the invisible process. Session-end acknowledgment dispatches for at most two seconds and preserves an earlier quit | Owner decision: the UI never waits unnecessarily for a writer; native I/O has no guaranteed deadline and Windows may end the process after session acknowledgment | A blocked UI, a lost earlier quit, a false durable-write promise at sign-out, or nonessential work holding persistence |
| Remove remaining unchanged sets early; retain the transfer pane's existing text guard | The shared pane already compares its texts; other native setters still need inspection | An obsolete surface is rebuilt or an already-fixed path obscures remaining repaint work |
| Setters read the control; no saved copy of what was set, no `Screen` type | Controls change themselves, icons depend on DPI and colours, pages are made anew | The window shows a value the model does not hold, or never sets one after a DPI or theme change |
| The inspector's file icon, the shield and the marquee are kept beside their control in `Controls` | They cannot be read back: a new icon copy each time, no getter for `BCM_SETSHIELD` or `PBM_SETMARQUEE`; the wrappers are `Copy` handles | Sets on every refresh, or a restarted marquee |
| A refresh never sets the selection, focus, scroll position, typed text, column widths or order; the item count is an effect | The control or the person owns them; setting the count also redraws every row | A refresh undoes a column drag, or a new search with the same total shows old rows |
| Check boxes, toolbar check buttons and the Match: list are set from the model | A toggle the model refuses must go back | The control shows checked while the model says unchecked |
| Menus stay out of refreshes | Their states are set when a menu opens (window rule 9) | Work on every update for menus nobody sees |
| Three steps: structure effects, refresh, other effects | A column must exist before its header is set; Windows drops the focus of a control it disables | Lost header texts, or a lost focus |
| Reentrant updates invalidate stale presentation and state-dependent effects; accepted irreversible commands still execute exactly once | A native set may dispatch a newer update before the outer sequence resumes | Old searches, focus or structure effects undo newer state, or a committed file command is lost or repeated |
| A command's own dialog or menu runs at the end of step 3, not as a `ModalStep` | Modal steps run one at a time, and Settings starts fixes whose questions are steps; Copy/Move has no later conflict question | A fix's question cannot be answered while Settings is open |
| A dialog that a reply, disk event or operation starts is a `ModalStep` | Window rule 13 | A dialog's loop applies newer events before older ones |
| Effects stop once the window is destroyed | Window rule 5 | A reply reaches a reused handle |
| A notification that only records control-owned state is not an update | Shift+End over 100,000 rows sends a storm of `LVN_ITEMCHANGED` | One full update per notification stalls the window |
| The panels and operations keep their ordering rules | COM delivers messages while an operation's sources are read | Reentrancy into a half-prepared operation |
| Refresh values come from the model plus measures that `app` takes | Layout needs measured widths, numbers the regional format, paths `PathCompactPathW` | `core` cannot compute them; tests would need a window |
| The inspector keeps the facts it last showed, in `core`, not its captions | It decides when to restart the 60-second age timer; captions are display text (window rule 12) | Every poll restarts the timer, so "modified … ago" freezes |
| Areas make their own controls | So #98 can make them when an area first shows | The start-up wait gets harder to remove |
| The maintenance stages come after the user-visible ones, labelled as such, with a measured run against `8db8f37`'s figures | They give a person nothing and carry the regression risk | A regression in focus or selection speed ships unnoticed |
| Areas move before what an update returns changes; `Model` forwards meanwhile | AGENTS.md: restructuring is its own stage; the tests prove the move | Every area edited twice, and no proof |
| No `Request<T>` | The status and a panel's sizes keep their last answer while they ask again | The status blanks on every poll |
| `Instance` moves into `core`; no `Role` enum; the queue stays in its `OnceCell`, the drop target in its `RefCell` | The queue and the drop target are `app` objects made after the window; the drop target must be revoked at `WM_DESTROY` | Win32 types in `core`, or a registration never revoked and an `Rc` cycle |
| Settings' page in `core`, its handle in `app`; `SearchArea` for Search's flags | Two facts with two owners; a search panel's list has neither flag | Copies, or always-false fields |
| No `WS_EX_COMPOSITED` | It buffers every child again and clashes with the list views' buffering | Slower paints and list view glitches |
| Measured runs report numbers, with no threshold | AGENTS.md: checks must not depend on the machine's speed | A check that fails on a busy machine |
| No crate split for `core` | CI already tests `core` on Ubuntu; the split has costs and an unmeasured saving | Cost without a measured benefit |
| New rules are numbered 14 and up | Code and docs cite window rules 1 to 13 by number | Every citation points at the wrong rule |
| No thread per top-level window; the model stays on the window's thread | `LVN_GETDISPINFO` needs each cell's text before it returns | A lock and a thread hop in every painted cell |

## The shape

### Background work: one owner

UI work that can wait runs through one policy owner, used by Search, the
panels, the Viewer, Settings and later windows. A window supplies the work's
purpose, area and policy. Existing specialized readers retain their
apartment and lifetime requirements behind this owner. Accepted file
operations remain in `gozler-fileops::Queue`; the UI owner observes their
snapshots and routes explicit actions through its existing interface.
It owns neither a second engine scheduler nor engine worker threads.

- **`core::work` holds the rules,** with no Win32 type, tested on Ubuntu:
  the kind of each task, when a task counts as slow (one constant, a few
  seconds without an answer or progress), what then happens, the inline
  size of "Passes over many items" below, and the one captions function
  that words "Waiting for …".
- **`win::work` is the only UI task launcher.** Only its worker entry
  point may construct the private `!Send` `OffThread` token; UI code
  cannot obtain one. `!Send` alone proves no construction rule. Marked
  waiting calls require this capability, never a token supplied to an
  inline CPU pass. The worker also receives a cancel token and a
  registration for best-effort `CancelSynchronousIo`. Cancellation does not
  imply completion or make native resources safe to release; see Microsoft's
  [I/O cancellation contract](https://learn.microsoft.com/en-us/windows/win32/fileio/canceling-pending-i-o-operations). It
  delivers the task's answers through the replies channel and an
  `OpenWaker`, which wakes the window only while it is open, so no task is
  joined. It lives in `win`, so `win`'s own threads (folder readings, the
  Viewer's reads, drops, the clipboard) use it without `win` calling
  `app`.
- **`app::work` connects the areas:** it starts a task for an area,
  cancels the area's task that a new one replaces, and routes Cancel and
  Esc to the task shown where the person is.

Replacement, visibility and mutation are independent policy dimensions.
A search is replaceable and awaited; an export is awaited and changes files.
Do not make these overlapping rows mutually exclusive enum variants:

| Kind | Examples | When it is slow |
| --- | --- | --- |
| Replaced by the next action in the same place | a search as you type, a folder reading, a selection read, a Viewer's reads | A new task of the same purpose in the same area cancels it, silently |
| Waited for by the person | a search, a folder on a share, opening a file, an export, a copy | After the slow constant with no answer, its area's message place, where the result will appear, says "Waiting for \\server\share…" with Cancel. No automatic time limit |
| Background, nobody watching | the status, folder sizes, drive labels and free space, the update check | It gives up silently after its limit and runs again at the next natural moment |
| Changing files | a copy, move, synchronization, delete or export | Never cancelled by an unrelated action. Queue owns engine operations; Requests owns export until its explicit migration. Observe their existing progress/Cancel instead of introducing a second cancellation owner |

Bound replaceable UI work before migrating its launch sites. Each area and
purpose has at most one executing request and one pending replacement; a
new replacement supersedes that pending request. A cancelled native call
still occupies its execution slot until it actually returns. Pending work
has its own cancellation/generation and visible waiting state when awaited.
Unrelated purposes have separate admission so a blocked share does not
consume every search, Settings or service slot. Worker registrations and
pending replies are bounded too. Preserve dedicated STA/message-pump work
where required; this is policy around existing workers, not one global
worker thread or a new execution framework. Engine admission stays in Queue.

A task reports progress when it can, but the owner, not the task,
notices that a task is slow: it times how long each task has gone
without an answer or progress, because a task blocked inside a call
cannot report that it is blocked.
Engine Paused, Waiting and Compared states keep their existing meaning;
absence of progress in those states is not a second generic stall state.
Show a task's progress only in its owning surface: Settings shows its own
work, while engine progress remains in the main window's shared presentation.

**Passes over many items.** Time is not the only way to stop the window:
a pass over 100,000 selected rows takes 58–80 ms with no call that waits.
So the owner has the size as it has the time. `core::work` holds the
inline size, how many items a pass may walk on the window's thread, set
by one measurement so that a pass at that size takes a few milliseconds.
Every pass over the selection, the fetched rows, a folder's entries or an
operation's sources goes through `work::pass(items, f)`, which runs `f`
inline at or under the size and as a task above it, and delivers the
answer the same way in both cases: as an effect of the current update, at
once or on a later wake. A feature has one code path and never chooses.

- The collections a pass walks are shared snapshots: `Selection::Files`
  holds an `Arc<[ResultItem]>`, and a filter yields a new one, because a
  task must take them without copying 100,000 rows on the window's
  thread. Their type offers `len`, `get` and the range of rows on screen,
  which `LVN_GETDISPINFO` and the cell fill need, and iteration over the
  whole collection through `work::pass` or on a worker. The inline callback
  gets only bounded CPU data, never `OffThread`; preparing that data, applying
  its result and releasing its last large allocation follow the same size
  rule. This does not prove that arbitrary loops through `get` are bounded;
  review the affected callers rather than inventing a general loop checker.
  A panel's entries change in place
  with each disk event, so while a task holds a panel's snapshot, the
  panel's changes wait in a bounded batch in the order its generation,
  moment counter and `ReadChange` already keep; otherwise the next change
  would copy the whole listing on the window's thread inside `Arc::make_mut`.
  Success, error, cancellation and supersession all release this ownership.
  A folder replacement discards the old generation's events. If the pending
  batch fills, cancel/invalidate the pass and request a fresh listing through
  the existing reader, rather than growing a backlog. Replay/application is
  itself bounded; dropping a stale answer must not leave a panel frozen.
  Large snapshot retirement runs on the work owner; synchronization previews
  retain Queue's existing `release_comparison` route.
- A pass that sends messages to a control cannot leave the thread that
  owns the control. It takes a one-message form where Windows offers one,
  as `Deselect::All` clears every highlight with one `LVM_SETITEMSTATE`
  (730f93a), and is otherwise bounded by `SELECTION_LIMIT`. This is the
  one place where the rule stays per control.
- While a pass runs as a task, the data it works on is in a state of its
  own, so nothing acts on the old data meanwhile. For the selection's
  taken-file filter, that state unhighlights nothing, a command waits for
  the answer (`MainWindow::run` settles a pending selection before it
  enables a command, so no command acts on files a running operation has
  taken, window rule 10), and an answer filtered against older taken
  entries than the current ones is filtered again. A delete or move of
  Search's whole selection needs no filter: the window knows the
  selection loses every file.

Scrolling, opening Settings, switching to the other panel or opening the
Viewer cancel nothing unrelated. Cancellation requests do not prove a native
call has ended; a detached call remains accounted for until it returns, with
no UI join or unsafe resource reclamation. There is never a modal "please wait".

### Nothing stops the window

The window's thread runs the message loop and nothing that can wait:

- **No call that waits on a disk, the network, the service, another
  process or another thread:** no file or registry read or write, no pipe,
  no service manager query, no shell call on a path, no `WaitFor…`, no
  `recv` that blocks, no `sleep`, and no `join` of a thread that may still
  run. That work is a task of the owner.
- **No lock that another thread holds while it works.** A thread that
  shares a value with the window holds its lock only to copy the value in
  or out, never across a read, a write or a call; a channel is preferred.
- **At exit** the window hides, closes delivery while its HWND is valid,
  requests cancellation and releases nonessential workers without UI joins.
  Preserve Queue's existing asynchronous close acknowledgment before process
  exit, including cancellation persistence. Serialize the final `ui.json`
  read/merge/write on its persistence worker too. Completion or a reported
  write failure, not a forced deadline pretending to save, ends essential
  finalization. Keep the last valid file on failure and use the existing
  failure report without touching a destroyed HWND. Arbitrary native storage
  calls are not guaranteed to finish within seconds; UI responsiveness and
  process termination are different guarantees. Normal Exit closes the UI without
  waiting for its writer; the essential writer can keep the invisible process
  alive. Actual completion has its own signal: an earlier setup WM_QUIT cannot
  acknowledge it. At session end, dispatch messages for at most two seconds,
  preserving an earlier quit for outer loops, then return to Windows. The writer
  may still run, but the newest state is not guaranteed to survive sign-out.
  Export checks cancellation between pages, but a page write or removing its
  partial file can still block. Keep that cleanup off the UI thread and report
  its actual outcome; never infer a cleanup deadline from page cancellation.
  Drag-source completion attempts have a finite wait budget and are released
  without blocking essential writes if a frozen source cannot answer.
  Apply the same ordering to setup/reset and session end; `WM_ENDSESSION`
  cannot guarantee a write survives Windows forcibly terminating the process.
- **Exit no longer waits for a permission prompt.** Today `WM_DESTROY`
  joins the Include save and the fix thread, which wait for the prompt's
  answer. With the owner, the window closes without joining that prompt;
  process completion still follows essential persistence. An elevated helper the
  person allows still runs to its end on its own, and the Include save's
  temporary file, which its thread removes after the helper ends, can be
  left behind.

What enforces it, stated as what each part catches:

- **The token:** marked UI blocking boundaries require the privately
  constructed worker capability; compile-check that UI code cannot create
  or transfer it. Keep engine internals behind Queue's existing asynchronous
  interface; do not change every engine function to take a UI token.
- **The scan:** a `core` test reads the sources of `app`, `win` and `core`
  (as the catalogue-key test reads them), skipping `#[cfg(test)]` code,
  and fails on a waiting call outside the thread modules. It matches the
  forms that wait, not bare names, because `Path::join`, a row's `is_dir`
  field and `std::process::id()` wait for nothing: `fs::` paths, `File::`
  calls, the `Path` methods `exists`, `is_dir`, `is_file`, `metadata`,
  `symlink_metadata`, `canonicalize` and `read_dir` called on a path, a
  `join()` with no arguments, `thread::sleep`, a `recv()` with no
  timeout, and `Command` or `Child::wait`; a named list of waiting Win32
  calls (`ShellExecuteW`, `SHOpenFolderAndSelectItems`,
  `GetVolumeInformationW`, the registry calls, `OpenClipboard`,
  `Shell_NotifyIconW`, which waits for Explorer's tray and fails only
  after a timeout when Explorer hangs), seeded by one pass over `win` for
  calls that reach another process; and a
  named list of other crates' waiting functions
  (`gozler_client::folder_is_missing` and `gozler_fileops::Queue::new`).
  Queue's existing asynchronous `close` is an audited interface, not a
  blanket ban. The scan also flags `thread::spawn` or `thread::Builder` outside
  `win::work` (rule 15). The code a task runs lives only in modules named
  `…_thread` or under `work`, which hold nothing else, so a file that
  mixes the two splits. The few exceptions are listed in the test with
  their reasons: the panic hook's join, which keeps the window's thread
  from dispatching while its state is half changed (window rule 11), the
  keyboard hook's thread, which needs its own message loop, start-up
  before the window exists, essential finalization on non-UI workers and
  bounded nonessential completion attempts, plus engine-owned threads behind
  Queue's asynchronous interface.
  A recognized-form scan is a reviewed backstop, not proof about aliases,
  indirect calls, native modal loops or unknown APIs. Trace those boundaries
  in review; do not build a general Rust parser for this check.
  `core` holds no file call: `core` builds and parses the
  settings text, `app` reads the file at start-up and a task writes it.
- **The watchdog** observes stalls the reviewed boundaries miss. A worker
  posts a heartbeat to a dedicated window on the UI thread every 100 ms
  while a Gozler window is on screen, through a lifetime-safe wake endpoint.
  The mechanism must demonstrate dispatch in the native loops listed below;
  a genuinely blocked UI thread cannot acknowledge it. When an unsuppressed
  heartbeat goes unanswered past the limit, one
  constant in `core::work`, 200 ms to start, the watchdog writes a line,
  and again each second while the stop lasts: how long, the window
  thread's CPU-cycle delta over the same interval (`QueryThreadCycleTime`),
  as separate observations with different units, and the message in progress,
  from a stack that the window procedure wrappers in `win::window` and
  `win::dialog` push and pop around each message of a Gozler window or
  dialog, whichever loop dispatched it, so a nested loop never
  overwrites the outer record. A message to a system dialog leaves the
  record of the message that opened it on top,
  which is the one to name. Do not compare raw cycles with elapsed time or
  automatically classify a stall from them. Post to a dedicated window on
  the UI thread, not a bare thread message or a main-loop-only hook. Before
  relying on this mechanism, one bounded native check must observe delivery
  during a menu, application/system dialog, size/move and drag/drop loop,
  and absence of acknowledgment during a controlled blocked handler. If a
  native loop does not dispatch it, correct the mechanism or suppress that
  known loop; do not log ordinary modal interaction as a hang. This is a
  prerequisite of stage 3, not a native behavior already proven by core tests.
  Microsoft's [modal-loop guidance](https://devblogs.microsoft.com/oldnewthing/20130905-00/?p=3313)
  explains why a thread-message or main-loop-only hook is insufficient.
  It records message IDs and control IDs or classes, never window text,
  which holds typed searches and file names. Only the watchdog thread
  writes `ui-slow.log`, which keeps its newest 64 KB, in the settings
  folder or `--data-dir`. A line names the message, not the slow call;
  the issue names the call after reading the code.

### The window's own work

Some slow work is not background work: it handles the controls, or it is
a pass over many items. The rules above cover it; what is measured
today, and what happens to each:

**Fixed in stage 1:**

- The old File operations window's first open was 536 ms (#110). That
  window is gone; the shared transfer pane is lazy. Trace its current first
  open and retain a targeted measurement only if a remaining synchronous
  step is suspected. Do not rebuild the old window or rerun its benchmark.
- A delete or move of 100,000 selected entries (#107):
  - `selected_paths`, about 15 ms when the command runs, and a panel's
    `chosen()`, 67 ms, go through `work::pass`. `transfer` builds
    `chosen()` twice today, through `selected_paths` and again in
    `transfer_start`, which needs only the count and the single entry
    (`chosen_count`), so the second build goes first and the rest is
    measured again.
  - `retain_untaken`, 58–80 ms at 100,000, which `SearchList::drop_taken`
    runs on the wake after the taken entries change and `Model::select`
    on every selection read while they are not empty, goes through
    `work::pass` with the selection's own state of "Passes over many
    items". The answer sets `Selection::Files` directly; it does not run
    `Model::select`, which would filter on the window's thread again.
  - The old `Saved::new` cost of about 20 ms has moved to the engine worker.
    Preserve compact recovery registration at admission and save-before-first-
    mutation; no UI-thread serialization work is scheduled here.
- Copying 100,000 paths, 59 ms (#91): the clipboard block is built by
  `work::pass`. Reading the selected positions from the list stays, a
  message pass bounded by `SELECTION_LIMIT` (15.6 ms at 100,000).
- A resize step with 100,000 selected, 33–52 ms: `layout` calls
  `show_panel` for both panels, and `Panel::counts` walks every selected
  entry each time. The counts go through `work::pass` and are kept until
  the panel's selection or entries change.
- An ended delete or move scans every fetched row (#104), through
  `work::pass`.

**Only on evidence,** when the watchdog's log or a measured run shows a
person waits, each in one sentence until then:

- The window waits 59–183 ms while Windows creates the folder picker.
- The first paint of `C:\Windows\Fonts` (387 entries, 104 ms) spent 37 ms
  filling cells, inferred from the code to be icon lookups; not measured.
  Source tracing still finds synchronous icon lookup in panel callbacks;
  remaining icon work is tracked in #148, separating it from closed #87's
  migrated drag/open paths. The native fixture hang has no proven icon cause.
- A panel list's whole paint takes 28–30 ms, of which Gozler's cell
  filling is 0.65 ms (#110); one run turns off the list's settings in
  turn to find the cost.
- A 2,000-character Cyrillic path with a path term costs 49 ms a paint
  (#120).

Measured as cheap, so not listed: saving `ui.json` (2 ms, but it moves as
a disk write), compiling a regex (0.75 ms) and the inspector's sums
(0.5 ms at 100,000), in
`git show f6a8e7e:docs/plans/window-responsiveness.md`. Of #126's
benchmarks, stage 1's measured run takes the submit of 100,000 sources;
the rest of #126 stays in that issue.

### Setters that read the control

For a refresh-owned value, use a cheap native getter to skip unchanged
sets unless the caller already establishes the change. Reading a control
is not a decision about the model, so `win` still decides nothing. A
value that the control changes in its own way when it is set, such as a
shortened path, is compared in that form.

| Value | How the setter reads it |
| --- | --- |
| Text of a label, an edit or the title | `WM_GETTEXT` |
| Check box | `BM_GETCHECK` |
| Toolbar button checked, enabled, text | `TB_ISBUTTONCHECKED`, `TB_ISBUTTONENABLED`, `TB_GETBUTTONINFOW` |
| Status bar part text, tip, edges, icon | `SB_GETTEXTW`, `SB_GETTIPTEXTW`, `SB_GETPARTS`, `SB_GETICON` |
| Tooltip text | `TTM_GETTEXTW` |
| List header text | `LVM_GETCOLUMNW` |
| Edit cue banner | `EM_GETCUEBANNER` |
| Date picker day | `DTM_GETSYSTEMTIME` |
| Icon or image list | the handle, exact because `glyph_icon` and the image lists cache by glyph, size and colour |
| Enabled | `IsWindowEnabled` |
| Visible | the `WS_VISIBLE` bit, not `IsWindowVisible`, which walks the parents |
| Rectangle | `is_placed_at`, which compares only position and width for a combo box |
| Progress bar position and range | `PBM_GETPOS`, `PBM_GETRANGE` |
| Combo box selection set from the model | `CB_GETCURSEL` |

Three values cannot be read back: the inspector's file icon, because
`show_file_icon` sets a new copy of the icon each time; the administrator
shield, because `BCM_SETSHIELD` has no getter and `Button::set_icon` sends
it on every call; and whether a progress bar's marquee runs, because
`PBM_SETMARQUEE` has no getter and setting it again restarts the
animation. The wrappers are `Copy` handles, so each window's `Controls`
struct keeps the last such value beside the control, made and dropped
with it.

A control that leaves `WM_SETREDRAW` to `DefWindowProc`, such as a
static, loses its `WS_VISIBLE` bit inside its own `draw_once_after`,
while a list view keeps it (tested), so no setter reads a window's
visibility inside its own `draw_once_after`.

### What a refresh never sets

Some values belong to the control or the person, and a refresh never sets
them: the selection, the focus, the scroll position, the text a person
types, and the column widths and order, which a person drags. They change
only through an effect: selecting rows, moving the focus, setting the
search box's text for Clear, filling the columns. Filling sets all of a
list's changed widths inside one `draw_once_after` on that list. The
list's item count is an effect too, because setting it also redraws
every row, which a new search with the same total needs.

A value that the person changes and the model then decides, such as a
check box, a toolbar check button or the Match: list, is set from the
model by a refresh: a toggle the model refuses goes back.

Menus stay out of refreshes: their item states are set when a menu opens
(`WM_INITMENUPOPUP`, window rule 9), and `replace_menu_bar` compares the
bar's titles.

### What an update returns

```rust
pub(crate) struct Accepted {
    /// The areas whose shown values may have changed: independent
    /// facts, so a set, not an enum.
    touched: Areas,
    /// What to do, in this order.
    effects: Vec<Effect>,
}
```

`app` handles an `Accepted` in three steps, after the model's borrow has
ended, with each area's values computed into owned data before its
setters run:

1. **Structure effects:** the effects that change what the controls are
   or hold, such as inserting a column with its header, setting a list's
   item count or preparing a panel's controls/data, because a value
   can be set only on a control part that exists, and some values, such as
   the rows in view, need the new count.
2. **Refresh** each touched area: compute its values in `core` from the
   model and the measured inputs, and call the setters.
3. **The other effects, in order:** tasks for the owner, timers, focus
   moves, which come after the sets because Windows drops the focus of a
   control it disables, cancels, and posts to Settings.

The rules around the steps:

- **An effect that changes the model** returns an `Accepted`, which is
  handled at once with its own three steps.
- **A message that a set or an effect sends back** runs its handler and
  its own steps outside model borrows. Tag precomputed presentation and
  state-dependent effects with the model revision or relevant purpose
  generation. After reentry, abandon stale presentation and revalidate or
  recompute remaining structure, navigation, focus and task-start effects
  before applying them. An already accepted irreversible command keeps its
  operation id and executes once; invalidating a refresh cannot discard it
  or replay it. Use direct feature transitions rather than a generic event
  framework. If refresh cannot converge in a bounded pass, post one deferred
  refresh and stop the old pass; a debug assertion alone is not the release
  behavior. Actual task starts/cancels stay in the ordered effects, outside
  structural control creation.
- **A dialog or menu that a command opens** runs at the end of step 3,
  outside every borrow, as Settings and the context menu run now, so a
  dialog that a reply or an operation starts can still run inside it.
- **A dialog that a reply, a panel's disk event or a file operation
  starts** is queued as a `ModalStep` and runs from `Message::Deferred`
  (window rule 13), as now.
- **Effects stop once the window is destroyed** (window rule 5).

### What the loop covers

- **The main window.** Its model goes through this loop. A notification
  that only records state a control owns, such as each `LVN_ITEMCHANGED`,
  which records the told rows and posts one `Message::Deferred`, is not an
  update. Each message it handles runs one update, not one per
  notification; an effect's own `Accepted` and a repeated refresh run
  inside that update.
- **Settings, synchronization's comparison dialog and each Viewer** keep
  their existing local state and a small loop of the same shape. Settings
  edits its draft; a Viewer owns its file; the comparison dialog reads an
  immutable engine preview and explicitly commits it. File transfers stays
  shared inspector content routed through the main window and its existing
  `core::Operations`/`app::FileOperations` owners, not another loop or window.
  All use the UI work policy where applicable and the native setters.
- **The panels and the operations** keep their ordering rules: a
  listing's changes apply in the order `ReadChange` gives, and an
  operation's sources come last in `OperationSteps`, because COM delivers
  messages while the window waits.

### Values computed from the model and from measures

A refresh is not a function of the model alone. The layout takes the
measured width of each caption (`win::gdi::TextMeter`), the inspector fits
its texts, the numbers follow `WindowsNumbers`, and paths are shortened by
`PathCompactPathW`. `app` measures, `core` computes from the model and
those measures, as `core::captions` and `core::layout` do now, and the
Ubuntu tests give fixed measures.

Where a change of a shown value also starts an effect, `core` decides it
from facts, not by reading a control. The inspector is the case: it keeps
the facts it last showed, not its captions (window rule 12), and restarts
its 60-second age timer and fits its texts again only when they change.

### Areas

The main window is made of areas: Search's list with its search box, the
filter pane, the inspector, the status bar, each panel, the key bar, the
file operations, the bridge to Settings, the tray and the instance. Each
area has an `app` module that owns its controls, their creation, interaction
and refresh. Where an area has private facts or transition rules, a `core`
type owns them. An area that only reads existing facts uses that existing
owner; it does not acquire an empty core type or another writable copy.
Owning control creation also lets #98 make an area's controls when it first
shows. `Model` holds the areas and the facts several of
them read: the strings, the settings, the service's status and the
instance. `Model` retains the authoritative mode in its saved window state.
`MainWindow` holds handles and layout measurements and routes each control
ID and timer to its area, reading the mode from `Model`.

### Module ownership and file map

**Owner ruling:** code and architecture have one responsibility and one owner
for each rule or mutable fact, with no parallel implementations. Prefer plain,
elegant code. Introduce no ceremony without an immediate planned use; the file
map does not justify empty types, placeholder modules or forwarding layers.

Keep the existing `core` / `app` / `win` layers and Rust's `name.rs` plus
`name/` convention. Split by the responsibility that owns a rule, its state
and its changes, not by line count. A child containing `impl Model` is valid
for cross-area coordination; moving feature methods alone does not transfer
their ownership. Do not reorganize the other crates for this file map.

Paths below are relative to `crates/gozler-ui/src`. They name the target for
responsibilities being split, not directories to create before their stage.
Recheck current callers and field ownership when each move starts.

```text
core/
  model.rs                    shared facts and cross-area transactions
  model/following.rs          cross-list operation receipts and acceptance
  search.rs                   Search-only transient state
  inspector.rs                inspected facts and age-timer decisions
  panel.rs                    Panel/Panels interface and accepted state
  panel/
    rows.rs                   private row algorithms and invariants
    calculation.rs            owned candidate and remapping
    counting.rs                immutable count pass
    quick.rs                   quick-search decisions and pass
  search_list.rs
  search_list/keeping.rs
  selection.rs
  selection/capture.rs
  operations.rs
  operations/following.rs     pure follower pass and presentation
  file_operations.rs
  file_operations/{batch,layout}.rs
  captions.rs                 shared caption primitives and aggregation
  captions/{search,filters,inspector,panels,status,settings}.rs
  layout.rs                   the one main-window geometry calculation
  layout/column_sizing.rs
  work.rs                     pure work policy and collection-size rule
app/
  mod.rs                      startup and process/message-loop entry
  main_window.rs              lifetime, composition and message routing
  main_window/
    search.rs                 Search controls and list notifications
    filters.rs                filter controls and range-list interaction
    inspector.rs              inspector controls, fitting and native icon
    panels.rs                 both panels' controls and interaction
    key_bar.rs                key-bar controls using core commands
    settings.rs               bridge to the existing Settings dialog
    tray.rs                   notification-area lifetime and events
    selection.rs              shared native selection capture
    commands.rs               route commands to their one implementation
    layout.rs                 measure and apply core's geometry
    apply.rs                  ordered effects, refresh and modal queue
    synchronize.rs            comparison orchestration
  status_bar.rs               existing shared status control owner
  file_operations.rs          Queue client and transfer content owner
  settings.rs                 existing Settings dialog owner
  viewer.rs                   existing Viewer window owner
  work.rs                     typed composition and finalization
  work/
    lists.rs                  capture/filter/kept CPU task group
    panels.rs                 calculation/count/quick-search task group
    clipboard.rs              publication and retry execution state
    settings.rs               serialized save/registry task group
    shell.rs                  shell invocation and execution payloads
    operations.rs             follower/report/path-preparation task group
    queue.rs                  ordered engine-command session
    requests.rs               service requests/export, migrated in Stage 2
win/
  controls.rs                 shared plumbing and small native wrappers
  controls/{list_view,folder_tree,status_bar}.rs
  work.rs                     Task execution and worker capability
  work/shell.rs               STA/pump execution support
  folder.rs                   window registration and reader interface
  folder/mailbox.rs           bounded folder delivery
  folder/work.rs              listing/watch/free-space/label worker bodies
  viewer.rs                   native surface and reader interface
  viewer/work.rs              file-reader worker body
  drop_target.rs              native drag/drop interface and registration
  drop_target/work.rs         supported background data intake
```

Unlisted leaf modules keep their existing responsibilities and paths. Crate
configuration, resources, catalogues, generated IDs, prototypes and timing
checks remain together. No general mandate splits Settings, dialogs, file
operations, layout or tests further. ProgressBar stays with the small native
wrappers; do not make a directory for each small control or single function.

**Core ownership.** `Model` keeps the single Settings value and save baselines,
shared strings, service observations, mode and cross-area transactions. Search
owns its list/debounce/Enter-on-answer state; each search panel retains its
own SearchList. Filters remain in Settings. Inspector state contains inspected
facts and age-timer decisions, never another selection. Status and key-bar
values may read existing facts without a new writable core owner. Add a core
Settings-bridge type only if page/request/count state earns its own interface;
do not create empty `core/tray`, `core/key_bar` or `core/filters` modules.

`Panel` alone accepts navigation, interaction and rows. Private row algorithms
own ordering, merge, filtering, exact-name reconciliation and kept-operation
storage; app cannot mutate their row storage. Calculation owns a candidate
until Panel accepts it against current identity and evidence. Cross-list
receipt coordination stays beside Model; follower-internal transitions stay
with the follower. Feature caption functions have one aggregation/translation
route through WindowCaptions, reuse shared text/date formatting, and do not
introduce another layout calculation.

**App ownership.** Move feature fields, control creation, interaction and
refresh together. Filter scrolling and range closing belong to FilterPane;
inspector fitting and its native icon to Inspector; typed-path and deferred
panel notifications to the panels. Controls becomes an aggregate of these
owners. MainWindow retains lifetime, accelerator/focus coordination, layout
composition and one ordered applier/modal drain. Its `apply` child implements
that same coordinator, not an independent update loop. End state borrows
before native calls that may reenter, including after extraction into an area.

Deepen the existing StatusBar and FileOperations owners. The Settings bridge
does not duplicate the Settings dialog, and command dispatch does not copy
feature implementations. All three lists use the same NativeSelection capture
and application logic; native controls retain live highlight/focus authority.
Preserve Rc-per-message lifetime and deferred-modal ordering during moves.

**Work ownership.** Work remains a typed composition/lifetime owner, with one
explicit close sequence. Groups own Task handles, retry/coalescing inputs and
execution-only state, using the shared admission, cancellation, retirement and
delivery rules. Accepted Location and label facts belong to their consumers,
not a second model inside Work. Invoke a group's narrow interface directly,
such as `work.panels.calculate(side, input)`; add no forwarding method to Work
for every group method, task trait, registry, generic bus or new scheduler.

Place new execution in its coherent group when that responsibility is already
being edited in Stage 1. Migrate the remaining groups with their owning work;
do not require a crate-wide move first. Location execution can join list/filter
work, and pairing/Settings drive labels can join Settings work. Panel drive
labels remain with the native folder reader. Typed file-command continuations,
including PanelTransfer and TransferStart, belong to command orchestration;
path-preparation workers carry them unchanged and return them for validation
and modal decisions, without an untyped token map or callback framework.

Queue retains scheduling, cancellation and recovery. The Queue group serializes
engine commands and returns snapshots/acknowledgments. Streaming requests and
folder watches retain bounded event/progress delivery; do not force them into
one-response tasks. Native readers use shared core policy and the native launcher
below their safe interface, without depending on app. Viewer registrations and
native resources retain their own lifetimes; app does not acquire duplicate
ownership of reader handles or accepted facts.

**Native ownership.** Split substantial ListView, FolderTree and StatusBar
wrappers into private children, re-exporting their existing types through
`win::controls` during the move. Keep helpers local to the control they serve;
the facade is one implementation, not a compatibility copy. Native worker
children separate waits from UI entry points while preserving COM apartments,
registration, revocation, provider lifetimes and streaming. A whole-worker
module may instead move under `win/work`. Move `fix_thread` under app's work
group when the whole worker migrates; replace `app/requests.rs` after its
owner-backed group takes over. Keep no old launch path alongside the new one.

Before each move, identify authoritative fields, mutating callers and
acceptance/staleness rules. Move the responsibility with its existing tests.
Keep fields private and imports explicit; do not widen everything to
`pub(crate)` to make extraction compile. A temporary forwarding method exists
only while real callers migrate and disappears within the completed batch.
Keep tests beside the behavior they protect, without tests of folder names,
forwarding, getters or arbitrary file-size quotas. The portable-core guard,
wait scan, catalogue coverage and intra-doc links must recognize moved paths.

### Painting

The window never visibly flashes and never shows stale pixels, with
Windows' own means only:

- **Double buffering where the control offers it:** list views
  (`LVS_EX_DOUBLEBUFFER`) and the tree (`TVS_EX_DOUBLEBUFFER`), as now,
  and the toolbars (`TBSTYLE_EX_DOUBLEBUFFER`) if stage 4's recording
  shows them blink and its screenshots show no wrong background.
- **A control is set only when its value differs,** by the setters above,
  because a label, a button, a toolbar and a status bar part draw again on
  every set, also of an unchanged value.
- **A control is moved only when its rectangle changes,** all of a
  layout's moves in one `DeferWindowPos` batch for each parent, because
  a batch takes only windows of one parent (`place_filters` keeps its
  own for the filter pane's children), every control with
  `WS_CLIPSIBLINGS` and every parent with `WS_CLIPCHILDREN`, as now.
- **`WM_SETREDRAW` only around a change that would draw one control
  several times,** such as the list's resize step or a language switch.
  The main window's redraw keeps `RDW_ERASE`, so a mode or language
  switch leaves none of the other mode's pixels.
- **No `WS_EX_COMPOSITED`:** it buffers every child again, clashes with
  the list views' own buffering and slows every paint.

## Rules for writing window code

These join DESIGN.md's window rules as 14 and up, because code and docs
cite rules 1 to 13 by number:

14. **Nothing stops the window.** The window's thread makes no call that
    can wait on a disk, the network, the service, another process or
    another thread, takes no lock another thread holds while it works,
    and joins no thread that may still run.
15. **UI background work goes through one policy owner.** `win::work`
    launches UI tasks and privately constructs their `OffThread` capability.
    Policy covers admission, replacement, slowness and exit; Queue retains
    engine workers, operation cancellation, device admission and recovery.
16. **A handler turns a message into an update and handles its
    `Accepted`.** It sets no control outside the three steps.
17. **Model the state by its rules.** A boolean for an independent fact;
    an enum that carries its data for states that exclude each other; an
    enum with one transition function for a state machine whose moves are
    constrained; a trait only where areas behave differently behind one
    call. Two fields that must agree, a combination that cannot happen,
    or the same condition tested in several places mean the model is
    wrong: change the representation instead of adding a branch.
18. **A fact has one owner,** the area that changes it; another area reads
    it through the model, never through a copy.
19. **A refresh skips unchanged native sets** using a cheap getter or a
    caller-established change; a value the control cannot return is
    kept beside it in its window's `Controls`. `app` keeps no copy of a
    shown value only to skip a set.
20. **A timer is a variant of `Timer`,** never a bare number.
21. **A core test asserts the behavior a caller would see,** the values an area
    shows for one representative state, given fixed measures, or the
    effects of an update in their order, against literals the scenario
    predicts. Pure rules need no window; uncertain native behavior gets one
    bounded check, not a permanent display-only regression suite.

## Stages

Keep coupled work with one implementer through coherent feature batches;
independent batches have exclusive file ownership and agreed interfaces.
At each completed stage, one adversarial reviewer challenges the delivered
requirements, paths, integration, ownership and material complexity. Independent
implementation may continue only on inputs outside that review. Batch compatible
fixes for the same reviewer; follow-ups cover fixes and affected contracts,
expanding on evidence. Final integration uses code-review deep with six
specialists. Self-read coherent changes before required builds;
build and commit each completed stage, preserving the existing verification
evidence. Installation remains a separate owner decision. A defect
found on the way is filed, or fixed in its own commit when it is small.
Stages 1 to 4 change what a person feels; stages 5 to 9 are maintenance,
change nothing a person should notice, and come after them. Their risk is
a regression in focus, selection or speed, which their proofs watch.

### Stage 1: the owner for background work, and the known waits

1. `core::work`: the independent task policies, finite admission and pending
   replacement rules, the slow constant, cancellation/give-up rules and the
   wording of "Waiting for …". Preserve the engine boundary above.
2. `win::work`: the UI launch boundary, the privately constructed `!Send`
   `OffThread` token, cancellation, best-effort `CancelSynchronousIo` and delivery
   through the replies channel and an `OpenWaker`. `app::work`: starting
   a task for an area and routing Cancel and Esc.
3. The known waits move onto the owner:
   - the Location check (#141): checked as a task when the Location
     changes and on F5; each search uses the latest answer, so typing
     never touches the disk;
   - the shell calls (#142): Open, Explore, Properties, Open file
     location and the open commands run as tasks on a thread with its
     own COM single-threaded apartment, with `SEE_MASK_NOASYNC`; a
     failure comes back as a `ModalStep`. A prompt the shell makes owned
     by a window of another thread attaches the two threads' input, so a
     hung shell call could still freeze the main window. One run on a
     share that stopped answering decides the owner: the main window if
     it keeps taking input, otherwise a window made on the shell's
     thread;
   - the `ui.json` save (#143): move the complete read/merge/write off-thread,
     preserving `Settings::save_changes`' per-window changed-key intent,
     `SaveScope::Choices` and the latest file's unrelated keys. `core` keeps
     parsing/merging pure. One writer serializes this window's attempts;
     coalescing keeps every pending key change/removal and scope, not merely
     the last text or last delta. Results identify their attempt/generation.
     Preserve the current attempted-baseline/failure rule: a failed attempt
     reports once and retries after another state change, not every timer.
     Advancing that baseline for queued work must not lose an earlier pending
     key. Read the latest disk file when writing, so another window's changes
     are merged as today; do not invent a cross-process transaction system.
     Finalization and `WM_ENDSESSION` follow the persistence contract above;
   - the drive labels (#140), read as a background task, the last labels
     kept for each refresh;
   - the clipboard open (#144): retried on a short timer, not a sleep;
   - `is_paired`, which hashes gozlerd.exe; the registry read and write
     for start at sign-in; `Queue::new`'s read of `operations.json` (17.6
     MB for 100,000 sources), and if it moves, a write of the file waits
     until the read has ended, because a write before it would replace
     the file with only the new operations. Hold new engine commands until
     Queue initialization is ready rather than exposing a half-restored
     queue; existing engine admission and recovery remain its own. The drop's
     `GetData` from
     another process; and `EmptyClipboard`'s message to another process:
     each moved onto the owner, or shown to return at once, with the
     evidence in the stage's report;
   - the locks shared with the window: Queue's snapshot, reorder/admission
     and action critical sections, and the service check's. Audit their
     current callers and work under lock; #146's old path-reservation walk
     is gone and must not return. Current `Queue::close` already starts its
     recovery write on an engine worker and returns a receiver, so preserve
     that interface instead of moving a nonexistent UI write lock. Show that
     UI-facing locks only copy bounded state, or move the remaining large
     work through the existing owner, with evidence in the stage report.
4. `work::pass`, its inline size from one measurement, and the snapshot
   types of "Passes over many items" for the selection and a panel's
   entries, a panel's changes waiting while a task holds its snapshot;
   then the items under "Fixed in stage 1" in "The window's own
   work".

**Proof:**
- `core` tests of the rules, with fixed times: a new task in the same
  area and purpose cancels the old one and one elsewhere does not; work
  that changes files is never cancelled by another action; a task the
  person waits for shows Cancel only after the slow constant; background
  work gives up silently; nothing is delivered after the window closes.
- One pure admission case holds an execution slot after cancellation and
  replaces its pending request repeatedly: only the newest pending request
  remains and an unrelated purpose can run. Compile-check capability creation
  and inline CPU passes without granting UI code the blocking capability.
- One settings-save case covers coalesced changes to different keys while
  another window changes an unrelated key; verify scope and failure/retry
  acknowledgment against the current behavior, using fixture text.
- For item 4, `core` tests that `work::pass` runs inline at the inline
  size and as a task above it and delivers the same answer both ways;
  that a filter answer for a selection that is no longer current changes
  nothing; and that an answer filtered against older taken entries is
  filtered again. A panel-pass cancellation/replacement case releases its
  snapshot, rejects old-folder events and retains current updates without
  unbounded replay. Measure the current selected-source path once at
  100,000 entries; remeasure first-open work only if the current pane trace
  justifies it. Attribute comparisons to the historical figures above.
- The After install items in `ui-after-install.md`: a Location on a
  sleeping disk; Enter, F3 and Properties on a share that stopped
  answering; a pulled USB drive; a clipboard another program holds;
  Settings > Drives and service and Settings > Include with a drive
  asleep or offline.

### Stage 2: UI task migration, exit without UI joins, the scan

A move before the stage that changes what a person sees (AGENTS.md:
restructuring is its own stage). The one change of behavior is at exit,
because the owner joins no task: Exit no longer waits for an open
permission prompt, as "Nothing stops the window" says, which reverses
what #126's check 2 expects today.

1. The UI task threads move onto the owner, each with the rules it has
   now: the requests, the panels' folder threads, the Viewer's reader,
   the Include save, Paste, the entry checks with `present()`, the update
   check, the service check, the fix thread, Indexed folders, the
   clipboard and drops. Queue and its internal workers remain engine-owned;
   the shared transfer pane and synchronization preview use their existing
   operation ids, explicit commits, retained reports and cancellation routes.
2. Remove UI joins of potentially running workers and close their delivery
   endpoints before HWND destruction. Essential persistence follows the
   asynchronous acknowledgment contract of "Nothing stops the window";
   do not force process exit before recovery/cancellation is saved. Export
   cleanup stays off-thread and drag-source notification cannot hold those
   writes. Cover ordinary Exit, setup/reset and session-end ordering without
   claiming a hard deadline for storage I/O.
3. The scan test, with its listed exceptions and the `…_thread` and
   `work` modules; files that mix window-thread code and thread code
   split. Use the native worker children in the file map, preserving each
   reader's streaming, apartment and resource lifetime contracts.

**Proof:** no existing test covers the moved threads' cancel, wake and
exit paths, so passing tests prove only that nothing else broke. The
proof is a code review that traces each moved thread's cancel, wake and
exit against the rules it had, and the scan test, which passes with only
its listed exceptions; its first run, before item 1 ends, lists what is
left. In `ui-after-install.md`: closing the window during
a search, a folder reading on a share that stopped answering and a copy,
and the window's place kept at the next start.

### Stage 3: slow tasks say so, and the watchdog

1. Each area shows its slow task in its message place, with Cancel where
   the rules say, and replaces or gives up its tasks by the kinds.
2. The watchdog and `ui-slow.log`.

**Proof:**
- `core` tests of the area rules, with fixed times, where stage 1's
  tests do not already cover them.
- A `core` test of the watchdog's rule, with fixed times: an unanswered
  heartbeat past the limit writes a line, a stop that lasts writes one
  each second, wall time and CPU-cycle deltas remain separate fields, and
  popping a nested record restores the outer one. Read/copy the current
  context under a short lock, then release it before logging or native
  queries; the UI never waits for log I/O.
- One bounded native check of the dedicated heartbeat window in the nested
  loops listed above, plus one controlled blocked handler. Observe actual
  acknowledgments, not fixed sleeps. Keep the result as API evidence, not a
  recurring display test. The mechanism is not accepted until this passes.
- In `ui-after-install.md`: a day of the owner's normal use with the
  build; the implementer then files one issue for each distinct message
  and cause in `ui-slow.log`, adding repeats to the existing issue. No
  stage waits for it.

### Stage 4: painting and unchanged sets

1. One screen recording before any change: a border drag in both modes,
   a splitter drag, typing a search, a mode switch, a status update and a
   copy with the shared transfer pane open. It runs once as part of the
   stage, never as a loop, and is saved for the owner to review later.
   The border drag is driven through the window's own handlers between
   `WM_ENTERSIZEMOVE` and `WM_EXITSIZEMOVE`, because a drag by
   `SetWindowPos` alone skips the path a person's drag takes
   (`column_sizing.is_dragging()`), and real input needs the foreground.
2. The setters that set every time compare first: `Hwnd::set_text`,
   button captions, `Toolbar::set_text`, the status bar's and the
   tooltips' tips, `ListView::set_header`, `Edit::set_cue`,
   `DatePicker::set_day`, and `Button::set_icon` with the shield kept in
   `Controls`. Retain the transfer pane's existing text comparison; audit
   remaining status/progress setters, including indexing activity, against
   the current source rather than adding another presentation cache.
3. Only if the first recording, at normal speed, shows a flash a regular
   user would see:
   - toolbar blink: `TBSTYLE_EX_DOUBLEBUFFER`, with its look in light,
     high contrast and at 150 % DPI in `ui-after-install.md`, because
     switching them changes the owner's whole system;
   - the list flashing on a resize step: the list moves with the other
     controls, only its column widths stay inside its `draw_once_after`,
     whose redraw for the list drops `RDW_ERASE` but keeps `RDW_FRAME` and
     `RDW_ALLCHILDREN`; then the whole-window `InvalidateRect` after a
     layout goes, with any area the window paints itself named; and if
     the horizontal scroll bar then flashes on a narrowing drag, the
     columns are fitted before the move when the list narrows, a panel's
     widths taken from the layout's planned rectangle because
     `set_panel_widths` reads the list's client size, which changes only
     with the move;
   - a half-drawn page on a Viewer scroll: its area paints through
     `BeginBufferedPaint`.

**Proof:** the same recording after the stage, which the implementer
judges frame by frame; the owner's later review is a check, and the
stage does not wait for it. A frame that shows a cleared, half-drawn or
doubled control is a defect of the stage.

### Stage 5: state types (maintenance)

1. `Instance` moves into `core`, `Model` gets it at creation and answers
   `offers_file_operations` from it, and `Model::file_operations` goes.
   `app` keeps the queue in its `OnceCell` and the drop target in its
   `RefCell`, which `WM_DESTROY` empties to revoke the registration while
   the window's handle is valid.
2. `Ending { Running, ForSetup, ForReset }` replaces `closing_for_setup`
   and `resetting`: a setup starts from a fix and Reset all from
   Settings, so a window never ends for both.
3. `core` keeps the shown Settings page as `Option<SettingsPage>` in
   place of `index_page_shown`; `app` keeps the dialog's handle.
4. Search's `search_due` and `enter_on_answer` move with Search's list
   into `SearchArea { list, due, enter_on_answer }`, because a search
   panel's list has neither.
5. `app` gets `IncludeSave { Idle, Saving(Saving), Saved(Result<(),
   NotSaved>) }` for `include_saving` and `include_saved`, and `RangeList
   { Idle, Closing(RangeFilter, ListClosing), Closed(RangeFilter) }` for
   `range_closing` and `range_closed`.
6. `enum Timer` with one function to and from Windows' timer ID.
7. `column_dragged` posts `Message::Deferred` once, as
   `selection_pending` and each panel's `ListChanged` already do.

The requests in flight stay as they are: the status and a panel's sizes
keep their last answer while they ask again, the Include count forgets it,
and `Model::accept` already drops a reply of another generation in one
place.

**Proof:** the existing tests pass, with only names changed. A new test
only where a rule moved into a new type and nothing watches it.

### Stage 6: areas (maintenance)

A move, with no change of behavior or of what an update returns:

1. Each area gets an `app` module owning its fields, controls, creation,
   interaction and refresh. A `core` type owns its private facts or transition
   rules only where they exist; an area reading shared facts uses their existing
   owner without an empty type or another writable copy. Move in the order of
   how separate the areas already are:
   the inspector, the filter pane, the status bar, Search's list, the key
   bar, the bridge to Settings, the tray and the instance, then the
   panels, whose code already lives in `core::panel` and
   `app/main_window/panels.rs`.
2. `Model` keeps thin methods that forward to the areas while callers
   move, and drops each once no caller uses it.
3. `MainWindow` routes each control ID and timer through one table.
4. `core/captions.rs` splits along the areas whose texts it holds, and
   `win/controls.rs` by control.
5. Apply the module ownership and file map above. Keep Work as typed composition,
   accepted facts with their consumers, and Model/MainWindow as coordinators.
   Extract private row algorithms without exposing mutable row storage. An
   impl-block split alone does not complete this ownership move.

**Proof:** the existing tests pass with only their paths and names changed.
Done when each named area's private facts and controls have one feature
owner, shared facts remain in `Model`, and `MainWindow` routes rather than
duplicates those responsibilities. Aggregate area fields may remain in their
owning window/model. Remove temporary forwarding methods once callers move.
Split along those responsibilities; line count is a review prompt, not a
completion quota or a reason to fragment coherent code and tests.

### Stage 7: what an update returns (maintenance)

Prerewrite inventory (the immutable Stage 7 snapshot named above):
`MainWindow::apply` executes `Then` before `Change` and refreshes bound controls,
Inspector, polling and saved state for every accepted reply. `search` separately
starts the naming timer; the save tail also copies Viewer state into Model.
Keep replacement of these coupled paths with one implementer; review the
retained three-phase implementation once the whole batch is complete.

1. `Accepted` becomes `{ touched: Areas, effects: Vec<Effect> }`; `Then`
   becomes `Effect`, and `Change` goes. The stage starts by listing, from
   `MainWindow::apply` and `search`, what each arm does besides setting
   shown values; each such action becomes an effect. Known today:
   `ColumnShown` and `ColumnHidden` insert or delete a column and update
   `column_sizing`; `NewSearch` selects, sets the count, shows and focuses
   a row, cancels its `Selection` request, releases the operations,
   fetches the rows in view, fills around the first rows and may queue the
   step that opens the first result; `Message` clears the list's selection
   and count and cancels its `Selection` request; `Rows` and
   `SelectionRead` set the count or redraw rows and fetch the rows in
   view, and `Rows` for Search's list also fills around the first rows;
   `Status` reads the window's memory; `Fix`, `TotalCommander` and
   `IncludedCount` post to Settings; `Settings` replaces the menu bar and
   updates the tray icon; `ExportFailed` queues a `ModalStep`;
   `Searching` starts the searching timer; `Mode` starts or stops the
   panels, replaces the menu bar and moves the focus; `Panes` moves the
   focus; and the tail of `apply` sets the age, status and save timers and
   calls `set_viewer`, which changes the model.
2. `app` handles an `Accepted` in the three steps, with the rules around
   them, and stops running effects once the window is destroyed.
3. `core` emits the modal steps it decides; `app` keeps its queue for the
   steps that its own threads, drops and checks raise.
4. Settings, synchronization's comparison dialog and Viewer loops retain
   their scopes while taking the same three steps. Shared inspector transfer
   content continues through the main window; create no independent loop.

**Proof:**
- The existing tests in `core/model.rs` that assert `change` or `then` are
  rewritten to assert the touched areas and the effects in their order,
  against what each scenario predicts.
- One reentrancy case supersedes an outer state-dependent effect with a
  newer nested update while preserving an accepted file command exactly
  once. Trace the native reentry path in review; test the pure decision.
- One measured run on the window's thread of a status reply, a key in
  the search box, a resize step, a splitter drag, Ctrl+click and
  Shift+End over 100,000 rows, and the end of a move, reported beside
  historical figures from `8db8f37` (Ctrl+click 15.6 ms, Shift+End 5.3 ms,
  end of a move 39 ms, a status reply under 15 ms). It sets no threshold,
  because a threshold depends on the machine.
- In `ui-after-install.md`: the focus after Enter, Esc, Tab and a mode
  switch; header texts after a language switch; Settings' Apply.

### Stage 8: the remaining setters and the areas' refresh (maintenance)

1. Audit the setters still setting unchanged values. Preserve existing
   position/style checks in `ProgressBar::set_measured`; inspect marquee
   updates separately and cache only values the native control cannot return.
2. Area by area, the refresh calls the setters with the values `core`
   computes, and `app`'s copies of shown values go (`status_right` and
   `status_memory` where the status bar's values can be computed again).
   The inspector keeps the facts it last showed in `core`, and its file
   icon beside the control.

**Proof:** the recording steps of stage 4 once more, and a `core` test
that the inspector restarts its age timer only when the facts it shows
change.

### Stage 9: the docs

DESIGN.md "The window" takes rules 14 to 21, the owner for background
work with its inline size and `work::pass`, the watchdog, the three
steps and the areas; window rule 5 is rewritten for threads released
through the `OpenWaker` flag instead of joined; window rule 4 lists the 8
kinds of request purpose the code has, `FolderSizes` among them, not 7;
its Painting item takes the painting rules; the paragraph on the rejected
diffed view says that the controls hold what they show and the setters
read it. The sentences this plan makes false are rewritten: "Shared
code", that `core`'s only filesystem calls load and save the settings
file; "Requests", that Exit joins every request thread; "The selection",
for the filter's own state and the rows unhighlighted when its answer
arrives; essential asynchronous persistence acknowledgment versus released
nonessential workers; and bounded drag-source notification attempts. Retain
the shared inspector/Queue and settings-merge contracts now documented.
Move the enduring module ownership and interface rules into DESIGN.md and
the relevant crate/module design comments, describing the implemented
structure rather than preserving a proposed file checklist.
The bullet in `AGENTS.md` "Writing code" that points at this plan
points at DESIGN.md instead. This plan is deleted.

## Not in this plan

- No framework, windows-reactor, data binding or MVVM: Win32 has no binding, and
  writing one means notifications that call back into the window, the
  panic window rule 1 exists to prevent.
- No new drawn surface: owner draw still needs the owner's permission.
- No change to the virtual lists, to how rows are fetched, or to the
  selection contract; the model stays on the window's thread, because
  `LVN_GETDISPINFO` needs each cell's text before it returns.
- No thread for each top-level window, and no restored standalone transfer
  window: file transfers remains shared inspector content and waits stay
  off the one UI thread. Engine workers remain in Queue.
- No crate of its own for `core`, unless a measured rebuild of an edit to
  `app` shows a large saving.
- The start-up cost of making the main window's controls before it shows
  stays in #98. Settings pages and transfer content are already lazy; do
  not schedule those completed first-show changes again. Stage 6's area
  ownership may make the remaining start-up work easier.
- The 2-second status poll (#139) is a protocol change, outside this
  plan.
- No feature and no change a person can see, other than waits moved off
  the window's thread, slow tasks that say so with Cancel, and the
  flashes and unchanged sets that stages 4 and 8 remove.
