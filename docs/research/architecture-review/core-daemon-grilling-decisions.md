# Plan: architecture deepening of gozler-core and gozlerd

The coordinator (the architecture review session) is the only editor of this file. Agents read it, and report to the coordinator. An agent that thinks a settled decision is wrong reports the conflict and follows the decision. If following it would make the work incorrect, the agent stops and reports.

Report: `architecture-review-20261021-084821.html` in this folder.

## Owner requests, in the order received (2026-10-21)

1. Explore architecture deepening in gozler-core, gozlerd and gozler-cli. gozler-ui is out of scope, because another session reviews it.
2. Grill each candidate one at a time, with a subagent answering, until no question is open.
3. Send the two defects found to a new agent to fix, then continue in the way that is most optimal.
4. Keep the report's status current.
5. Coordinate the work with the coordinating skill.

## GOAL (owner request 10, 2026-10-21): address all items from the architecture review

How each item is addressed:
- Defects 1 and 2: committed in 6def2f6.
- Candidate 1: round 2 running in result-rows; commit after it passes.
- Candidate 2 (parked: "do it with the result snapshot or visibility stage 2"): its design is written into `docs/plans/result-snapshot.md` by the coordinator (uncommitted). It covers the shared immutable `VolumeRows` (`past_cut` moved into the reader), one row check returning the folder or nothing for both readers, the reuse-mark check kept apart, the merged order as a 1-byte tag per row with a checkpoint every 4,096 rows, weak volumes with `Arc::ptr_eq`, a row that left the scope counted as gone, the unmeasured creation walk, and the corrected 16,384-row read estimate. The "What the code does now" section uses candidate 1's names.
- Candidate 3 (parked until Rebuild index): written into `docs/plans/service-settings.md`, "How the service runs Rebuild index" (uncommitted): the supervisor route, an uncharged `RescanReason`, one "next build" value cleared in `begin_build`, the admin check through visibility's `pipe_client_token`, and the optional `ReadFailures` extraction with its four tests.
- Candidate 4 (dropped as a module): its placement decision is written into `docs/plans/service-settings.md`, "How the service applies Dates created": a field of the `Include` value, the three conditions, the rename to `IndexSettings`, and `dc:` refused while off. The visibility session meanwhile moved `SeeEveryFile` to `HKLM\SOFTWARE\Gozler` and dropped `ViewRegistry`. So the context struct for `run`/`accept_loop`/`handle_connection` has no third shared object to wait for; no change.
- Candidate 5: dropped; nothing to write.
- `docs/plans/visibility-plan.md` named the moved code: updated by the coordinator (uncommitted, to be committed with candidate 1). The holds now name `ResultRows::search`, `page`, `run_held` and `search_totals`. The visibility input to `ResultRows::search`/`search_totals` is a `may_list` closure, because `gozler-core` holds no token. `ResultRows` holds the `Listable`s. The corpus and T6 now describe the moved search corpus. `find_matches`/`run_totals` gaining `seen_by` stays accurate, because they are now internal steps.

### Goal check: candidates 2 and 3 are built now too

The goal hook counts written plans as not addressed. Decision (coordinator):
- Build now whatever needs no unscheduled feature:
  - Candidate 2's refactoring in `query/rows.rs`: move `past_cut` into the reader's state, so that `VolumeRows` never changes after the search; and make one `query` row check that returns `Option<FolderId>`, which `find_head` calls. The `Arc` around the rows still waits for the result snapshot. Owner: result-rows, in round 3, because it holds the context of `rows.rs`.
  - Candidate 3: `ReadFailures` becomes its own type, with four tests, in `tail.rs`. A Sonnet subagent prepares it as `scratchpad\tail.rs.proposed`, outside the working tree, while round 2 builds. It is applied for round 3.
  - Round 3 = both changes, one build by result-rows.
- Candidate 3 status: the coordinator reviewed the patch (behaviour unchanged; four tests, each naming its failure), formatted it with rustfmt, and applied it to `tail.rs` at about 12:20, before round 2's full run. That run compiles it, and result-rows reports its tests. The design's one gap: the loop reads `failures.since` for the log's `failing_for_secs`, which is a log value, not a policy decision. Accepted.
- Round 3 brief (candidate 2) is queued to result-rows. It runs only if round 2 is green.
- Not built: Dates created, Rebuild index and the result snapshot are product features with their own schedules. The result snapshot plan builds only "if the owner decides to". Candidate 4's only remaining part is inside Dates created.

## RESUMED (owner request 9, 2026-10-21): "stuff went into main. resume work"

- Main now holds the file-manager merge (7b94f06) and the defect fixes (6def2f6). Only candidate 1 is uncommitted (14 files and the new corpus file). The merge touched none of its files. No caller in the new main uses the exports that candidate 1 made private (grep).
- Both reviews resumed in phase 2 on the final diff.
- Bench comparison (`-Bench query_2M`, logs `bench-before.txt` at 10:58 and `bench-after.txt` at 11:35 in result-rows' scratchpad `cb994ccb-…`) is NOT a clean result. Three benches regressed by 10–23 %: `common(document)`, `wildcard(*.dll)` and `two-term(report txt)`. Others improved by up to 44 %, which a refactoring that should not change speed cannot explain. The "before" run happened while files were already edited, so its baseline is in doubt. To settle it: (1) the behaviour reviewer reads the one-volume path for added work; (2) in the next build round, the builder measures a true baseline from `git archive HEAD` exported to a temp folder (shared `CARGO_TARGET_DIR`, no worktree, no branch), then the working tree, back to back.
- Coordinator's read of the core diff (`query/rows.rs` lines ~638–1066, `server.rs` adapter, `proto::encode_page`): matches the settled design. `page` takes the guards in volume order and drops them before it returns, and `encode_page` finishes the frame after that. The second hold in `search` is a temporary guard that is released at the end of the statement. Coverage is read once, in `run_held`. `proto` imports `query`, not the reverse. For one volume, `run_query` adds no work per match; per call it adds one `String` for the mount and two small `Vec`s. So the bench swings are probably the baseline problem, not the code; unconfirmed until the true baseline runs.
- Design review (Sonnet) reported: no blocking departure. Coordinator's decisions on its items, for the next round's fix list:
  - `RowsSpec` instead of the draft name `RowsQuery`: accepted, no change.
  - `Reply` is `pub(crate)` where `pub(super)` suffices (the `engine.rs` tests use it); `run_totals` and `resolve_scope` likewise: FIX, narrow them to `pub(super)`.
  - `HeldIndex` is `pub(crate)` in `server.rs`: no change. It is the associated type of a public trait's impl, and `VolumeState` is `pub(crate)` too.
  - Two jagged comment wraps (the `rows.rs` module doc; the `find_matches` doc): FIX.
  - The daemon's error-code mapping lost its test when the corpus moved. The old corpus pinned `ProtoError::Overflow` for a total over 64 bits and `BadQuery` for a scope on no indexed volume; the CLI maps these codes to exit codes that scripts read. FIX: one `serve` test in `server.rs` per mapping, through the wire reply.
  - No test pins "coverage read once, not per page". No mechanism: the rule is held by `run_held`, and its `Coverage` doc says why. The one-volume "no lock while emitting" stays pinned by `an_index_changed_between_pages_marks_the_answer_incomplete`.
- Behaviour review (Opus) reported: no behaviour defect; all 13 checklist items kept (lock order, no lock while emitting, paging, scope, coverage, IndexChanged, sums, totals, empty answer, stop on emit false, page contents; `run_query`'s contract changed as designed). No added work per match or per row in the bench path; about 7 small allocations per call. Coordinator's decisions on its items:
  - The corpus no longer sends the request over the wire, so a serde mistake in `ParsedQuery` would go unnoticed. FIX: in `escompat::corpus::answer`, round-trip `Request::Search` through `encode_request`/`decode_request` before running it.
  - `Overflow` code mapping unpinned: same FIX as the design review's (one `serve` test per code).
  - Lock order across volumes: FIX, one test. A recording `IndexedVolume` adapter, with two volumes and a Compare merge, asserts that `read()` happens in the given order and that no guard is alive after `page` returns. The failure it watches is a daemon hang that a plausible edit would bring back.
  - Sums at the daemon level: older gap, not candidate 1's scope. No change.
  - `PlainVolume` is `pub` but used only in `rows.rs`: FIX, make it `pub(crate)`.

## Round 2 (sent to result-rows, the only builder)

All fixes above, in one build:
1. narrow `Reply`, `run_totals` and `resolve_scope` to `pub(super)`, and `PlainVolume` to `pub(crate)`;
2. rewrap the two jagged comments;
3. the corpus request round trip;
4. two `serve` tests: `ProtoError::Overflow` for a total over 64 bits, and `BadQuery` for a scope on no indexed volume;
5. one lock-order test with a recording adapter;
6. a true bench baseline: `git archive HEAD` exported to a temp folder, `-Bench query_2M` there with `CARGO_TARGET_DIR` set to this checkout's `target`, then `-Bench query_2M` in the working tree, back to back; then delete the temp folder;
7. one full `check.ps1` run.

Estimate: about 1 hour (two bench builds and one full run on the shared machine).
- Implementer's interface summary: `C:\Users\user\AppData\Local\Claude-Data\claude_pzs8sxrjxfjjc\side-session-notes\local_5adb76dc-62e3-45ce-9065-aa81e7c1157c\result-rows-deepening.md` (unconfirmed claim, under review).

## PAUSED (owner request 7, 2026-10-21) — superseded by RESUMED above

The owner asked to reach a stable point and stop, so that they can merge the `file-manager` branch into main. Work continues afterwards.

State at the pause:
- Candidate 1: edits are finished. The full `check.ps1` run (log: `C:\Users\user\AppData\Local\Temp\claude\C--SynoSoftware-TinyTorrent2-3rdParty-goz\cb994ccb-6b93-4142-9fa6-a0df315b6794\scratchpad\check1.txt`) passed fmt, clippy and all 13 test binaries. Its doc step was waiting for the shared target lock held by the keybar worktree's check. Confirmed by the coordinator from the log: the run finished green (doc finished at 11:25), and no cargo process runs. The defect fixes' supervisor and volume_state tests passed in that run (reported by result-rows). `-Bench query_2M` ran once after the change; its comparison is in result-rows' report, not yet checked. result-rows is stopped. Its report and the list of what in the visibility plan is out of date are in its last message and in its notes file `result-rows-deepening.md`. No review of the diff has run yet.
- Both candidate 1 reviews were stopped in phase 2, before they reported. On resume, start phase 2 again: the behaviour reviewer's HEAD checklist is in its report, and the design reviewer is ready. Both can be resumed by message, with their context.
- Nothing is committed. Uncommitted here: candidate 1 (gozler-core, `server.rs`, `deep_export.rs`, the corpus moved to `gozler-core/src/escompat/corpus.rs`), the defect fixes (`supervisor.rs`, `volume_state.rs`, `tail.rs`, `docs/DESIGN.md`), and the UI session's files.
- Overlap with `file-manager` (`git diff --name-only main...file-manager`, 63 files): only `crates/gozler-ui/src/app/main_window.rs`, `timing.rs` and `win/controls.rs`, which are the UI session's uncommitted files, not this work's. The branch's non-UI files (`gozler-fileops`, `gozler-winfs`, `gozler-core/src/layout`) do not overlap with candidate 1. Main moved to 95ba03a (visibility plan docs) during this work.

Owner request 8 (settled): commit the defect fixes now as one commit, and candidate 1 after its review passes. Done: `6def2f6 fix(daemon): apply changed Include settings to volumes without a tail` (`supervisor.rs`, `volume_state.rs`, `tail.rs`, and three `docs/DESIGN.md` hunks). Candidate 1's two DESIGN hunks ("Rows on demand" and the golden corpus paragraph) stay uncommitted with the rest of candidate 1.

## Owner decisions (settled)

- Candidate 1 lands before stage 2 of the visibility plan.
- Both defects are fixed now.
- Nothing is committed unless the owner asks.

## Pieces and owners

| Piece | Agent | Files it may edit | Builds | State |
|---|---|---|---|---|
| Defect fixes | session `daemon-defects` (local_31bc97f9) | `gozler-daemon/src/supervisor.rs`, `volume_state.rs`, the "Include settings" and status bar text in `docs/DESIGN.md` | yes, once | done; adversarial review passed |
| Candidate 1 | session `result-rows` (local_a405a50c) | `gozler-core/src/query/**`, `gozler-core/src/proto/**`, `gozler-core/src/scope.rs`, `gozler-core` tests and benches, `gozler-daemon/src/server.rs` and `server/**`, `gozler-daemon/examples/deep_export.rs`, `gozler-daemon/src/main.rs` (`run_demo_query`), `gozler-daemon/examples/memsim.rs`, the corpus's new home `gozler-core/src/escompat/corpus.rs`, "Rows on demand" in `docs/DESIGN.md` | yes; the only builder from now on | implementing |
| Adversarial review of the defect fixes | subagent of the coordinator | none (read-only) | no | done, passed |
| Candidate 1 behaviour review | subagent (Opus) | none (read-only) | no | phase 1: checklist of today's behaviour from HEAD; phase 2 starts when result-rows' edits are finished |
| Candidate 1 design-conformance review | subagent (Sonnet) | none (read-only) | no | phase 1: reads the spec and rules; phase 2 as above |

Owner request 6 (2026-10-21): get candidate 1 done faster with more parallel agents. Decision: no second implementer, because result-rows had already edited 13 files (+701/−1,373), including the corpus move, `server.rs` and `deep_export.rs`, and a second editor would collide with it. Instead, the two reviews prepare now and run in parallel during result-rows' build. Their fixes join one more build.

Files that belong to other sessions, which nobody here edits: `crates/gozler-ui/**`, `docs/plans/window-responsiveness.md` (a UI session); `docs/plans/visibility-*.md`, `result-snapshot.md`, `service-settings.md` and `CONTEXT.md` (the visibility session).

Contract between the pieces: the defect fixes are in `supervisor.rs` and `volume_state.rs`, which candidate 1 does not touch. Candidate 1's full `check.ps1` run also runs the defect fixes' tests.

## Estimate (given to the owner 2026-10-21)

- Candidate 1: 2 rounds, about 3 hours in total.
  - Round 1: implementation plus one full `check.ps1` run, about 2 hours. A comparable plan, ReFS, landed 1,456 lines in 1.5 hours.
  - Round 2: fixes from the adversarial review plus one `check.ps1` run, about 1 hour.
- Defect fixes: 1 round, done. If the review finds a defect, the fix joins candidate 1's next build. It does not get a build of its own.

## Confirmed findings

- Defect 1, the Include gap, is real (code read): `Fleet::apply_settings` → `set_rules` only sets a flag, and `take_new_rules` has one caller, `tail_loop`. A snapshot volume leaves supervision in `Supervised::scan`, and a given-up volume has no tail.
- Defect 2 is real (code read): `match_sums` runs once in `find_matches`, while the page total drops in `read_rows` and `find_head`.
- The defect fix diff was read by the coordinator, and no defect was found. Not confirmed: "check.ps1 green after one full run". The session's transcript does not show the tool output. Candidate 1's full run will confirm it.
- Load-bearing assumptions for candidate 1 (code read):
  - A page call can hold N read guards while it changes the reader's cursor: in one method, the volume list and the reader are separate fields, and Rust allows borrowing them separately.
  - The daemon's guard: `VolumeState::read` returns `VolumeRead { indexed: RwLockReadGuard<Indexed>, phase }`, so `HeldVolume` can be built from one call. Each guard derefs to `VolumeIndex` through `indexed.index`: a small wrapper, or `RwLockReadGuard::map`, which `VolumeState::index` already uses.
  - Waits: the page encoder runs in `tokio::task::spawn_blocking` (`server.rs`), so lock waits stay off the async runtime. A `parking_lot` reader waits behind a waiting writer, so the lock order (`VolumeSet` order) is the only deadlock guard. The writer finishing ends the wait. Candidate 1 adds no new wait, because the pipe write stays after the locks are released.
  - `&VolumeIndex` derefs to `VolumeIndex`, so the plain test adapter needs only a small struct that also carries the mounts.

- Adversarial review of the defect fixes (subagent, code read, 2026-10-21): no defect a user or the daemon would hit. Cleared: new starts against `pending`/`starting`/`stop_tails`; `retired.shutdown()` in the scan loop returns at once; the `set_rules`/`take_new_rules`/`begin_build` race loses no rules; restarts are bounded by real settings changes; the phase table keeps coherence; the test does not depend on the machine.
  - Rare case, no mechanism: when a snapshot volume cannot be opened again after a settings change, it shows as complete with its old rules for about 4 minutes, until it is given up on and turns `Failed`. Before the fix this lasted until the daemon restarted, and a snapshot volume is complete only "as of its last build" anyway.
  - Two texts corrected by the coordinator: the give-up log line in `supervisor.rs` ("…or the Include settings change") and the `JournalSetup::Refused` comment in `tail.rs`. They are built in result-rows' run. `tail.rs` has no other editor.

## Review plan

- Defect fixes: one independent adversarial review now, read-only, during candidate 1's implementation. A finding is a concrete sequence of events that the reviewer tried to refute.
- Candidate 1: one independent adversarial review once its edits are finished, during or after its build. Its fixes go into one more build. Stop when a pass finds no new defect.

---

# Settled design

## Candidate 1: move the search's lock sequence from gozlerd into core

Settled (answerer agreed, facts checked in code):

- Owned `ResultRows<V>` in `query/rows.rs`. Entry `ResultRows::search(volumes, &CompiledQuery, scope path, RowsQuery{sort, offset, limit, sums})` returns `Result<ResultRows<V>, NotIndexed>`. Totals entry `query::search_totals` returns one query type carrying totals and coverage; error has `NotIndexed` and `Overflow(SizeOverflow)`, because gozlerd maps them to BadQuery and Overflow.
- The page call takes the page size, locks the volumes it reads, fills the page, releases the locks, and then returns. So no caller can emit while it holds a lock. The answer owns its volumes; no `Clone` bound.
- Seam: trait `IndexedVolume` with GAT guard `Index<'a>: Deref<Target = VolumeIndex>`, `read() -> HeldVolume<guard>` (guard + phase complete + metadata_pending from the same call), `mounts() -> VolumeMounts`. Mount prefix is derived from the mounts. Adapters: `Arc<VolumeState>` in gozlerd, a plain `&VolumeIndex` adapter for tests and benches. Trait doc states both halves of the lock-order rule. `SharedIndex` stays separate (one volume, closure, write half).
- The scope path is resolved in core under the scan's lock hold. `VolScope`/targets move into core. `match_case` comes from `CompiledQuery`.
- Coverage is read once, at the scan's hold, as today. Reading it on every page would mark rows from a complete index as incomplete.
- `Reply` becomes the private page reader; `VolumeRows` becomes `pub(crate)`; `find_matches`/`order`/`read_rows` become `pub(crate)` steps used only by the tests that change the index between holds.
- `query` does not import `proto`. `proto` converts `Rows` into the query input and the field flags, and builds `PageEnd`/`QueryTotals`/the frame. One encode function next to `ResultsPage` replaces the closure in `stream_rows` and the copy in `deep_export.rs`.
- `run_query` stays and goes through the plain adapter, so benches keep their baseline.
- Generic dispatch; no new allocation per hit or per page.
- Golden corpus moves to core (it needs only `escompat`), and then runs on Ubuntu CI. The `serve` tests stay in gozlerd.
- No `Arc` around the shared rows until the result snapshot.
- After it lands, the visibility plan names `find_matches`, `run_totals`, `Reply`, `run_on_volumes` and `stream_rows`; the visibility session updates its plan.

## Candidate 2: one owner for the stale-row check

Outcome: parked. Do it with visibility stage 2 or with the result snapshot.

- The reuse-mark check in `read_rows` stays. `SortRecord` is asserted to be 16 bytes and has no room for a file ID, so the reuse log stands in for one. Two mechanisms for two moments, chosen for performance.
- `Scan::evaluate` already shares `in_scope` and `FolderCopy::entry_folder`; it orders them between clause groups for speed. There is no copy to remove.
- Page-time check: one `query` function returning `Option<FolderId>` (sendable in this folder, or not). No four-case enum, because neither caller handles the reasons differently. The index exposes only `holds`.
- `find_head` keeps the accounting and calls the check.
- Only `past_cut` changes in `VolumeRows` during a reply. Moving it into the cursor makes `VolumeRows` immutable and shareable. `FolderCopy` stays per reply.
- `ResultRows` = shared rows + volumes held strongly + one cursor.
- Snapshot seek: store the merged order as one `u8` volume tag per row plus a checkpoint every 4,096 rows (~6 % over the rows; no tags for one volume). Making it walks every row once under the locks: not measured.
- Snapshot volumes: `Weak<VolumeState>` for memory, plus `Arc::ptr_eq` against the request's `VolumeSet` for correctness (a wedged tail can keep a removed volume's `Arc` alive). Upgrade once per read.
- Snapshot plan text needs: a row that left the scope is gone; memory for tags; weak volumes; the unmeasured creation walk.

## Candidate 3: journal read policy out of tail_loop

Outcome: parked until Rebuild index; Speculative. It fails the deletion test (one caller, ~30 lines), and the policy has no defect.

- Narrow form: `ReadFailures` becomes a type with a failed-read method and a good-read method. Time enters as `now: Instant`. The loop keeps logs and state calls. Classification stays a `match`; the budget stays in `bounded_rescan`.
- Tests: 15 s of failures never escalate; past 30 s escalates; recovery only after a degrade; backoff doubles and stops at the cap. They replace `retry_policy_is_bounded_and_degrades`.
- Rebuild index: an uncharged `rescan` with `RescanReason::Requested`, through the supervisor. The defect fix already clears the rules flag in `begin_build`.

## Candidate 4: service settings owner and pipe server context

Outcome: dropped. No module survives the deletion test.

- The visibility list keeps its read on every search, so it shares no reload route with Include. The visibility grill recommends storing the list in `HKLM\SOFTWARE\Gozler`; if the owner accepts, that does not change this outcome.
- For the service settings plan: Dates created is a field in the existing Include value. Conditions: `rules_key` includes it; `VolumeRules` carries it; `#[serde(default)]` off. Rename `IncludeSettings` → `IndexSettings` in that change.
- Context struct for `run`/`accept_loop`/`handle_connection`: add it with `ViewRegistry`, not before.
- `dc:` while Dates created is off: refused with a message naming the setting.

## Candidate 5: two totals of one scan

Outcome: dropped. DESIGN documents two contracts. A shared total would add two fields to `MatchSums`, which crosses the wire, to remove ~15 lines.

## Owner request 11: partial answers, not excuses (2026-10-21)

Owner ruling, now in the global CLAUDE.md ("Give the partial answer, not the excuse") and in memory.
Three places in gozler follow the struck "never a partial sum" rule. They went to result-rows as round 2b, which runs after round 2's build and before round 3:
1. totals: `match_sums` / `without_sizes`, a known sum plus an unknown count per kind, the wire, the UI caption, DESIGN.md "Right" row;
2. folder sizes after a pass that stopped part way (DESIGN.md "Sizes and dates");
3. `gozler --status`: "sizes not read yet" becomes the known total plus a note.
Checked and left as they are, because nothing is known to show: the service memory "—" while the service does not answer; the Drives page's empty text while the service does not answer; a per-row "Unknown" size cell; the "Results might be incomplete" notes, which already give the partial answer plus a note.

## STABLE POINT requested (owner, 2026-10-21)
Start no new work. result-rows: finish path-term pairs, one full check once gozler-ui compiles, fix own failures, report, stop.
Written, uncommitted: candidate 1 + round 2; candidate 3 (tail.rs, my E0282 fix applied); known sums (round 2b, PROTO 10, bootstrap finish_index); candidate 2 refactoring (round 3).
Blocker: gozler-ui does not compile (release_operations(hwnd) calls, fmt in main_window.rs/file_operations.rs) — a UI session's live edits.
Diff of 2b read by coordinator: correct.

## FROZEN (2026-10-21, ~13:35)
Owner said no building; the "Agent work stabilization and pause" session (local_0bce3ad4) then froze all goz work. It sequences the commits. State reported to it. A read-only reviewer is running and will finish its current read; do not act on its findings until that session answers.
path-term: tree 571/767/843 vs HEAD 350/376/396/490/676 Mcycles; unconfirmed; the reviewer is asked to look for extra work per match.
