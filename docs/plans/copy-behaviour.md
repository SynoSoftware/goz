# Copy behaviour: engine and progress UI

**Status: design only. Do not implement until the owner starts that work.**
This is the single plan for the copy-engine fix and its progress UI. It
replaces the copy-policies plan, the file-operations architecture plan and
the copy-user-experience analysis. It requires no window-architecture rewrite.

## Purpose and owner decisions

- The queue prevents competing copies and moves from overloading devices
  and making transfers slower. It does not plan the final filesystem,
  reserve paths or infer dependencies between requests.
- Delete deletes what is there, immediately and at full deletion
  parallelism, on background threads. It neither enters the transfer queue
  nor cancels, pauses or resumes copying. Copies continue against the drive
  as it exists when they process each item.
- Stopping the queue cancels its accepted active and pending transfers.
  Delete, completion notifications and late preparation replies cannot
  restart them. New explicit requests remain new work.
- Routine progress stays inside the main window. The separate File
  operations window opens only when requested; background work never
  activates it or takes the keyboard away from browsing or typing.
- Use existing Win32 controls and the person's permissions. Every delete
  remains permanent. Keep the existing engine and one lifecycle owner.

## Current evidence and bug to reproduce

The reported sequence is **Stop the queue -> Delete a just-copied folder ->
unexpected copying**. The exact button and operation ids were not captured.
Folder recreation by a deliberately continuing copy is allowed; restarting
accepted work after explicit queue cancellation is the defect to pin.

| Current source | Established behavior |
| --- | --- |
| `app/file_operations.rs::on_control` / `act` | Card Cancel affects its displayed operation; queue-header Cancel affects the selected row. Neither cancels the queue. |
| `core/model.rs::cancel_job`, `core/file_operations.rs::StatusOperation::of` | Status Cancel targets the newest running operation. The card can show another operation. |
| `queue.rs::cancel`, `Shared::enqueue`, `Shared::start` | Cancelling one operation or finishing it can schedule another. Submitting Delete also schedules. No queue-wide cancellation or pause exists. |
| `queue.rs::set_paused`, `operation.rs::Pause` | Pause belongs to one operation; its explicit Resume clears it. Delete has no path that clears that flag. |
| `queue.rs::close` and run completion | Cancel immediately followed by Exit can retain the cancelled request as Interrupted because removal waits for run completion. This race is source-traced, not reproduced. |
| `app/main_window.rs::paste` | A second pending Paste replaces the first intake record. |
| `core/operations.rs::submitted` | A cut Paste acknowledges movement at submission rather than completion. |
| `core/operations.rs::window_step`, `app/file_operations.rs::show` | Progress automatically shows after two seconds, also for later work after Hide; restored operations show at startup. The owner's focus interruption is reported, not measured. |

The existing `Queue` is the operation owner as well as its scheduler. Keep
that authority; distinguish lifetime/status from device admission inside
it rather than creating another queue or file-operation implementation.
Code anchors are under `crates/gozler-ui/src` and `crates/gozler-fileops/src`.

## Entry points and acceptance

| Command | Route to the existing owner |
| --- | --- |
| Search Copy/Move to folder | Selection -> destination picker -> `transfer_into` -> `start_operation` |
| Two panels F5/F6, key bar and menus | Selection -> Copy/Move dialog -> `panels::transfer` -> `start_operation`; F6 can rename |
| Paste, including Explorer Copy/Cut | Captured destination -> asynchronous clipboard read -> `finish_paste` -> `transfer_into` |
| Drop onto a Search folder, including Ctrl-drag/right-drag | Captured data object and destination -> asynchronous paths -> `finish_drops` -> `transfer_into` |
| F8/Delete | Normal permanent-delete confirmation -> independent deletion through the same owner |
| F7 New folder | `panels::new_folder` -> same owner and existing executor; no transfer-slot wait |
| Continue | Fresh acceptance of an Interrupted request, retaining its recovery identity and saved policy |

Two panels currently offers no clipboard or drag/drop commands. Clipboard
Copy alone and Copy name/folder/path start no engine work. Pasting or
dropping into Explorer belongs to Explorer's executor and cancellation UI.

Accept a request when its command is committed, before slow intake or
preparation. Give it one stable operation id and immediate status. Each
accepted Paste/drop retains its own source, destination and reply. A newer
clipboard must not silently supply different files to an older Paste;
report unavailable original data instead. Cancelling a picker/dialog before
submission accepts nothing. Repeating a committed command creates another
visible operation; copying into its own folder and Keep both can be deliberate.

Allocate the operation id and cancellation state in the existing owner
before intake. Resolving paths completes that same accepted operation;
it must not call preparation as a fresh request after cancellation. Keep
COM data objects in the reader's apartment and report through the existing
source wrapper, not by moving a raw COM interface into the engine.

Preparation and device discovery run off the window thread. Earlier work
still preparing reserves nothing. Acceptance sequence is priority among
ready transfers, not a promise about the eventual order of filesystem changes.

## Cancellation, pause and recovery

One submitted selection is one operation. Its Cancel stops all internal
transfers, workers, unvisited folders and unanswered entries. Show
Cancelling immediately on that same id and disable repeated cancellation
there. Stop new dispatch, wake paused/answer waits and cancel applicable
I/O; publish the final outcome after workers release their files. Retain
device use until dispatched I/O and a move's source cleanup finish.

**Cancel all** beside the transfer summary captures all accepted copies
and moves at that moment, including preparing, waiting, paused and pending
intake. Mark them cancelled in one owner transition before rescheduling.
A loop over today's per-id `cancel` is insufficient: it can start another
request between calls. Reject late replies for the captured ids. Delete
has its own Cancel; Cancel all in the transfer summary does not stop it.
Dormant Interrupted rows retain Continue/Dismiss and are not silently removed.

Pause is per operation and remains until explicit Resume or cancellation.
Do not add or imply a queue-wide pause. Hide, navigation, Delete and a
progress notification change no cancellation or pause decision.

Cancel keeps completed additions and leaves unrelated destination contents
alone. It cannot restore originals already changed by Replace. Unfinished
files follow the existing cleanup rules; report cleanup failures, including
the removal error currently discarded by `copy/ring.rs::give_up`. A move
may finish deleting the source of a file already copied whole. A completed
rename stays completed. Cancelled deletion restores no deleted file.

Preserve explicit cancellation through Exit without restoring that work as
Interrupted. The existing saved file remains the recovery authority; exclude
cancelled requests from later saves, including a delayed first-run save.
Persist on background threads and preserve save-before-first-file-mutation.
Continue is explicit, never automatic; Dismiss leaves partial files on disk.
Existing Exit rules still explain that unstarted waiting work is not saved.
Cancellation and delayed first-save eligibility must consult the same
authoritative operation state, so a stale save cannot reinsert a cancelled
id after its record was removed. Report an inability to preserve recovery
intent rather than claiming successful persistence after a disk error.

Hold clipboard/drop ownership until the actual outcome. Only a fully
completed cut move acknowledges all files moved, and only if the clipboard
sequence still belongs to it. Partial/cancelled movement does not consume
the original Cut. Notify a drop source once, after its workers finish.

## Independent Delete and live files

After confirmation, begin the existing streaming delete walk immediately.
No copy/move backlog, path overlap or device limit admits it. Use its
configured delete workers at full parallelism; do not add a preliminary
walk or ask the index for permission to start. An actual lock or permission
failure uses bounded per-entry retry/error handling, not a queue wait.

Delete cancels no reader or writer. A copy encountering a missing source
uses normal missing-source handling; a destination conflict uses the
chosen policy against what exists then. A continuing copy may create later
contents inside a deleted destination. Already completed items are not
replayed to reconstruct the old result. In-flight I/O follows checked
Windows results; the UI reports what happened rather than claiming a
predetermined final directory state. Apply the same principle to Explorer
deletions, renamed folders and other external changes.

Preparation and cached name tables are observations, not current disk
state. Revalidate at actual create/open/merge/replace, using the existing
identity, no-follow and checked-create rules. Never follow a deleted link's
target, silently overwrite a new occupant without the conflict choice, or
delete another program's replacement as cleanup of our unfinished file.

Keep the taken-entry model and its completion refresh. Hide a selected
delete promptly, show its live status, and refresh from disk at completion
so survivors or later copy output return. Deletion ends with its walk; it
does not watch the path forever to suppress future writes. Source deletion
is immediate too: there is no implicit copy-then-delete dependency.

## Device admission

A transfer groups selected roots for one source/destination volume pair.
One copy has one destination; a Search selection can have sources on C:
and E: and therefore C: -> D: and E: -> D: transfers. Keep bounded batches,
workers and the ring inside each transfer, and choose its own copy plan.
Never count every worker/file as a grant or walk the tree to allocate it.
Keep one operation's destination-name ownership and lock shared across its
pair executors, including `Names` / `Holder::Ours` distinctions for selected
entries. Under Replace, C:\a.txt and E:\a.txt copied to D: must both survive;
one selected file cannot replace the other. Preserve this in the pair split,
before adding new policies. It is an executor-local collision rule, not a
scheduler reservation of paths against another operation or Delete.

The manager grants all distinct physical devices of a ready transfer
together or none. Use existing disk numbers, server identities for shares
and conservative volume fallback when topology cannot be read. Partitions
and mount aliases share their physical-device allowance; a spanned volume
can require more than two devices. Copies and moves, including same-volume
moves and SSD work, use admission. Delete and New folder do not.

Default to one transfer per device. Settings later permits 1, 2 or 3.
Derive use from admitted transfers; a blocked request holds no device.
Scan every ready transfer, including multiple pairs in one operation,
until none fits. Do not let waiting work reserve an idle device. Reconsider
on arrivals, completion, cancellation, conflict answers and limit increases.
Lowering the limit drains existing work without preemption. Pause retains
its existing grants; a transfer with only unanswered files releases them.
An answer requests a fresh grant. No path dependency modifies admission;
executor checks decide individual filesystem actions against live state.

Examples, assuming unrelated names for readability:

| Situation | Result |
| --- | --- |
| Limit 1: C: -> D: running; C: -> E: waiting; E: -> F: ready | E: -> F: starts; the waiting transfer reserves neither C: nor E: |
| D: and E: share disk H; C: -> D: running; E: -> F: ready | E: -> F: waits for H; a Delete anywhere starts independently |
| D: -> E: on that shared disk | Counts H once |
| One selection C: -> D: and E: -> D:, limit 2 | Both may run; one operation does not imply one grant |
| All transfer slots occupied; Delete on C: | Delete starts; transfer counters and cancellation decisions are unchanged |

## UI: progress without interrupting work

The default is an **inline strip**, using native static text, progress-bar
and push-button controls. In Search place it above the status bar; in Two
panels place it above the key bar, keeping F3-F8 available. It appears for
accepted work, including intake/preparation, and does not take focus or
move the focused row. Progress updates change neither selection nor focus.

```text
Transfers   1 copying, 2 waiting     [progress]   [Details] [Cancel all]
Deleting    Projects: 120 items                  [Details] [Cancel]
```

The deletion row identifies its own operation; with multiple deletions,
show their count and use Details for per-operation cancellation rather
than attaching one ambiguous Cancel to the aggregate. Keep transfer
Cancel all visibly inside its transfer group. Search's status bar retains
the last outcome after the strip disappears. A zero-item cancellation
still acknowledges the accepted command; it must not leave an earlier
success as its only visible status.

**Details** and View > File operations explicitly open the existing
modeless window and may give it focus because the person requested it.
Replace icon-only action ambiguity with native text buttons. The detail
area follows the selected operation id; Cancel beside it affects that
operation, while Cancel all belongs to the transfer-group heading. Preserve
selection when rows update; ending an operation must not retarget a second
click onto another operation's Cancel. Pause/Resume stays per operation.

Hide, title-bar Close and Esc hide Details and leave work unchanged.
New operations, long duration, cancellation, completion, failure and
restored Interrupted work never reopen it or activate another window.
Interrupted work appears inline as a count with Details; Continue/Dismiss
remain available there. Completion errors appear as an inline count with
Report opening the existing failure list on demand. Keep failed outcomes
until reported; removing automatic modal reports must not lose their data.
Decouple report acknowledgement from completed-operation release. Today
`Operations::reported` advances Reporting to Kept/Removed. Completion must
still refresh panels and release taken entries when each list has reread,
even if Report is never opened. `core::Operations` retains the immutable
request/outcome references needed for on-demand reports independently of
the engine entry's lifetime; do not duplicate large failure lists. Opening
or closing a report must not be required to reveal surviving files.
Taskbar progress remains useful while minimized; no foreground request,
window flash or automatic restore is made by background progress.

The new policy question below is also inline/in Details, not a new modal.
Until it exists, retain the old explicit pre-copy conflict question so no
entry can be stranded without an answer UI. Remove only routine automatic
progress/recovery/failure presentation in the first stage. All controls use
the existing font, DPI layout, theme/high-contrast behavior and tab rules.
No custom painting is proposed. No opt-in auto-popup setting is required.

## Conflict policies

Keep the earlier policy decisions in this same plan. A same-name folder
merges without a question. Settings > File operations adds:

| If a file exists | Action |
| --- | --- |
| Ask me (default) | Set the file aside, continue other files and show that an answer is needed |
| Replace if newer | Matching size/time: count done. Newer source: replace. Newer destination: keep and list that reason |
| Replace if different | Matching size/time: count done; otherwise replace, including a newer destination |
| Replace | Replace |
| Skip | Keep destination and list as skipped |
| Keep both | Choose the first free numbered name |

Use `conflicts::is_done`'s existing size/time tolerance: two seconds and one
hour for the daylight-saving difference. Damaged settings fall back to Ask.
The table's matching-metadata "count done" applies to copies. For moves,
keep the existing `Decision::Done` safety rule in `copy.rs` and `moves.rs`:
retain the source and report `SourceKept`, since equal size/time does not
prove equal bytes. This applies to the new comparison policies and Continue;
do not turn a metadata match into authority to delete a move's source.
Snapshot the policy on acceptance; defaults/Remember changes do not rewrite
other accepted requests. Continue retains its saved policy; device limits
remain the manager's current setting, not a saved per-operation allowance.

The Copy/Move dialog shows the policy with Options opening the existing
Settings page. Check `IFileDialogCustomize` for Search's picker; if it
cannot carry that line, Settings still owns the policy. Do not duplicate
the setting in another dialog implementation.

Set-aside entries expose source/destination size and date in a native list.
Offer Replace, Skip, Keep both, Replace if newer and Replace if different,
plus Remember my answer. One answer applies to the operation's current
set-aside entries and future conflicts; omit per-file answers initially.
Remember also updates the default for later requests. Recheck the live
files and names on answer; hold no queue/name lock or worker while waiting.
No device or path is reserved by an unanswered file. Answering late cannot
restart cancelled work or dispatch the same entry twice.

`Choice` gains the two comparison policies. No choice means Ask rather
than silently skip. Preserve same-selection collision safety: Replace
never overwrites another selected entry; the second gets a number. Save
the operation's choice, reading older saved entries as before. Exit saves
wanted unanswered work; Continue may encounter those conflicts again.
Disk full, access denied and locked files remain reported failures, not
questions that stall unattended work. Remove old conflict-count/question
code only when the replacement engine and answer UI are usable together.

## Implementation sequence and proof

**Not started.** Start only when the owner authorizes implementation and
the in-flight disk-full change no longer shares the engine files. One
implementer keeps context; reviewers read and report. Avoid staging an
unusable policy engine without its answer controls.

1. **Stop/cancel and quiet progress.** Reproduce the actual control -> owner
   path with active and waiting operation ids. Add atomic transfer Cancel
   all, stable per-operation controls, Cancelling and cancellation-through-
   Exit. Cover late intake and clipboard outcomes. Deliver the inline
   summary and explicit Details; disable automatic progress/recovery/error
   windows while preserving access to their information. Proof: controlled
   real-engine fixtures for cancelled work that otherwise starts after
   Delete, plus UI decision tests for no automatic show, stable targets,
   and retained reports. Do not assert that a deliberately continuing
   copy must leave a deleted destination absent.
2. **Device-only execution and independent Delete.** Replace path-based
   admission with volume-pair execution and the physical-device manager;
   dispatch Delete directly through the existing executor. Replace the
   old overlap-order rule rather than leaving a parallel scheduler.
   Proof: event-driven admission fixtures for the examples above, aliases,
   capacity changes and one selection's pairs; real-folder checks that
   Delete starts while transfer slots are held and does not cancel or
   resume copying. Review create/open/merge/cleanup against concurrent
   deletion so stale name caches cannot dictate the result. Include one
   selection's cross-volume, same-name files under Replace and concurrent
   pair grants; assert that both contents survive under distinct names.
3. **Policies and inline answers.** Implement the policy table, saved choice,
   settings/Options and native answer list together. Add the concurrency
   setting to the existing manager. Proof: representative policy decisions
   and distinct boundaries, old saved-file compatibility, and one
   real-folder conflict answered after the remaining work finishes.
   Test answer/cancel races and release/reacquisition of device grants.
   A move fixture with different bytes but equal size/time keeps and
   reports its source rather than silently deleting it or claiming success.
4. **Finish current documentation.** After each delivered behavior update
   DESIGN.md, the engine spec, CONTEXT.md and affected crate design comments.
   Remove this plan when complete. Keep no finished plan as a second spec.

Try the reported control sequence with operation ids before fixing it.
If its exact cause remains unconfirmed, say so; do not make an unknown
original reproduction a gate or mislabel a new-contract test as its proof.
Pin the source-established cancellation/Exit and intake paths separately,
and test aggregate cancellation with controlled accepted work. Run each
new regression red through `pwsh ./build.ps1 -Test <filter>` before its
production fix, using existing progress hooks/channels, not fixed sleeps
or the installed service. Every added test names the failure it watches.
Review each stage's final diff before one full
`pwsh ./build.ps1`; follow AGENTS.md on failures. No benchmark is required
without a measured performance claim. No new dependency is planned.

Use one bounded native run to check that typing/focus survives background
start, update, completion and failure, plus explicit Details/Hide. Prefer
UI decision tests for policy and state. Save screenshots only of changed
native states; check DPI and high contrast without repeated foreground
probes. The owner reviews visual captures after installation; a stage
does not wait for screenshot approval. Commits and installation wait for
the owner. No build or runtime test is required for this plan-only review.

## Remaining evidence limits

The user's exact Stop button sequence still needs a real-control
reproduction before claiming its precise cause fixed. Current focus theft
has not been captured; removing automatic presentation establishes the
new default regardless of that Win32 cause. Existing responsiveness issues
remain tracked separately: bulk preparation #138, clipboard formatting
#91, clipboard-open sleeps #144, drag-entry data reads #145 and unbounded
failure-row insertion #110. Their costs were not remeasured in this plan.
