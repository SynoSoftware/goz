# Plan: make a change touch fewer places

Adding Show Commander and its shortcut changed about 15 files, and one new
field in the visibility work changed 8 files that do not use it. Those
changes prompted this plan. The goal is clear ownership and fewer
places that must agree on one rule, not a target number of edited files.
The catalogues, resources and real consumers still earn their own edits.
No stage changes what a person sees.

## Schedule

**Scheduled:** pending maintenance, separate from the
[Commander feature roadmap](commander-features.md). Not started. The
visibility work that originally held this plan back is committed.

The stage numbers remain stable for references from other plans; they no
longer prescribe one sequence. Each stage is its own coherent change and
commit. Prepare the affected owner before the feature that needs it; do
not combine structural moves with new behavior or gate every feature on
all six stages. A new background task uses the existing work owner even
when stage 2 has not run.

| Stage | What | Schedule |
|---|---|---|
| 0 | Recover missing design decisions | Independent documentation work |
| 1 | Shared test constructors | Before a record change that would spread irrelevant fixture edits |
| 2 | The work owner holds each task's lifecycle state | Separate infrastructure work; prioritize when task additions would repeat forwarding |
| 3 | One owner for the Show commands' shortcuts | Before changing global shortcut behavior, or before moving the Keyboard page in stage 4 |
| 4 | One module per Settings page | When page work justifies the split and the file is free of other edits |
| 5 | One row for each command | First preparation for the next batch of Commander commands |

Stage 3 comes before stage 4, so that the Keyboard page code is already
fixed when it moves.

## How the work runs

- One implementer keeps its context through the selected stages and fixes the
  review's findings. A reviewer reads each stage and reports; it does not
  fix or build.
- Every code stage keeps behavior the same. Review the final change, then
  run `build.ps1` with the existing tests and the justified cases below.
  Rerun only to resolve failures under `AGENTS.md`'s build rules.
  Stage 0 is documentation only and needs no build.
- No screenshots and no After install items, because no stage changes the
  window.
- The owner asks for each commit.

**Design standard.** Apply [Code style](../../AGENTS.md#code-style) and
[Naming](../../AGENTS.md#naming). The stages remove duplicated rules and
caller obligations within the existing Rust and Win32 architecture. Each
new abstraction must make the affected operation easier to read and use.

## What stays as it is

These cost files on every change, and each one is worth that cost:

- The 4 language catalogues (owner ruling).
- `gozler-ui.rc` and `ids.h` for dialog templates. `build.rs` generates
  `ControlId` from `ids.h`, so each ID has one source.
- The split into `core` (rules tested without Windows), `win` (Win32
  `unsafe`) and `app` (connects them).
- The command table in `core/commands.rs`: the menus, the accelerators and
  the Keyboard page read each command's row, so they cannot disagree.
- The settings binding tables, such as `CHECK_BOXES`.
- Results pass from a task to `MainWindow` and on to their consumer as typed
  values, such as `take_account_names` then `accept_account_names`. A
  forgotten step shows at once, because the feature does not work.

## Stage 0: decisions lost when the window architecture plan closed

`c698a56` deleted `docs/plans/window-architecture.md`. Its rules 14 to 21
reached `DESIGN.md`, but its "Work ownership" paragraph did not: that Work
groups own their task handles and that no "task trait, registry, generic
bus or new scheduler" is added. That paragraph also gave no reason. So the
decision now exists only in Git history, and nobody can tell whether it
still holds.

- Compare the deleted plan's "Decisions to keep" table and its "Module
  ownership and file map" section
  (`git show c698a56^:docs/plans/window-architecture.md`) with `DESIGN.md`.
- For each decision that still holds and is missing, add it to `DESIGN.md`
  at its owner, with its reason. A decision whose reason cannot be found is
  reported to the owner, not copied.
- The "Work ownership" decision is not copied. Stage 2 replaces it and
  writes the new rule with the code.

**Done when** each decision of the deleted plan is in `DESIGN.md`, replaced
by a stage of this plan, or reported.

## Stage 1: test constructors

Tests and fixtures spell out every field of three records by hand, so each
new field changes files that do not care about it:

- `ResultItem` and `LayoutFile`, including `LayoutFile::security`, which
  now carries an optional `LayoutSecurity`.
- `VolumeStatus`, including its `unread_folders` count.

Inventory the current literals before changing them. Separate fixtures from
real producers and change only records with repeated fixture setup. Include
fixtures in all workspace crates, including the daemon and command line;
the constructor belongs to `gozler-core`, not to each consuming crate.

**Change.** Give each record one test constructor in `gozler-core` that
takes the fields a test usually sets and leaves the rest empty. Tests set
any other field with `..`. Tests in other crates use them too, but
`layout::fixtures` is `#[cfg(test)]` and other crates cannot see it, so the
constructors go behind a `gozler-core` cargo feature that those crates turn
on in their dev-dependencies. A feature adds no dependency, so the
`gozler-core purity` step in `ci.yml` does not change.

**Constraints.**
- Add no `Default` to these types. Real code that builds them, such as
  `server.rs`, `settle.rs` and `listing.rs`, keeps naming every field, so
  the compiler still shows each producer when a field is added.
- Change no assertion. A literal that a test compares against stays a
  literal.

**Done when** a new optional field on any of the three records changes only
the record, its constructor and its real producers.

## Stage 2: the work owner holds each task's lifecycle state

The window architecture plan put the task rules in one owner: replacement,
slowness, cancelling and exit live in `core::work` and `win::work::Task`.
But each `Task` keeps the state the owner acts on to itself: its pile of
replaced inputs, its slowness clock and its unread replies. The owner can
reach that state only through every group, so each group forwards `tick`,
`take_retired` and `close_delivery` by hand. A forgotten call compiles:

- Without `tick`, a background task never reaches its time limit, so it is
  never cancelled, and the next request for that purpose waits behind it.
- Without `take_retired`, replaced inputs stay in memory until exit.

These are failure modes of the manual contract, not evidence that a current
forwarding call is missing. Remove the obligation from each group rather
than adding another checklist for new tasks.

The tasks are one type, `Task<I, O>`, with the same lifecycle code. A
trait over them would only hide their types. It would express no difference
in behavior (`DESIGN.md` window rule 17: "A trait earns its place where
behavior actually varies"), so it is not the fix. The fix is to move the state to its owner:
"the owner, not the task, notices that a task is slow".

**Change.** The owner gives each task its handles at construction, as each
task already receives its `OpenWaker`:

1. **One retirement pile.** `Retirement` already holds `Box<dyn Send>`. A
   task pushes replaced inputs straight into the owner's pile. Delete every
   `take_retired` and `retire_groups`. `Work` empties the pile off the
   window's thread at the moments it does today.
2. **Close through `Drop`.** `Task`'s `Drop` already cancels its running
   work. It also moves its pending input and its replies into the pile, so
   nothing large is freed on the window's thread. Delete every
   `close_delivery`.
3. **The owner holds the clocks.** Each task's clock (`since`, `interest`,
   cancel handle) is an entry in one list that the owner keeps. `Work::tick`
   walks that list and cancels expired tasks. A group whose `tick` returns
   results of its own keeps its call: `requests` returns expired purposes
   and `drops` returns the expired hover.

Afterwards construction registers a task's common lifecycle once. Typed
result routing and domain-specific completion still belong to their group.

**State and lifetime.** Keep admission and replacement in `core::work::Slot`.
Cancellation must not free an executing slot before its completion. Bind
the owner's clock and cancellation handle to the current task identity;
an old reply or progress report must not reset or cancel its successor.
Completion, cancellation and drop must remove the corresponding tracking
entry, so repeated replacement does not grow the owner's list. Clock
expiry still triggers the group's domain-specific result where needed.
Keep ownership explicit and the window thread's work bounded; add no lock
that a worker can hold while the window waits for it.

**Exit order.** On `WM_DESTROY`, `MainWindow` calls `Work::close`; `Work`
itself is dropped later with the window. Close admission and distinguish
disposable reads from essential writes before dropping task handles:

- Cancel disposable tasks and retire their pending inputs and unread
  replies before the final handoff of their retirement pile.
- Preserve the settings and queue groups' explicit finalization closures.
  Settings saving uses `finish`, an admitted registry write uses
  `finish_current`, and queue session saving uses `finish`. These tasks
  move off the window thread and finish in the established persistence
  order; a blanket cancellation in `Drop` must not discard accepted writes.
- The retirement owner must outlive every task that can still publish to
  it. Define who drains values produced by those finalization closures and
  when the last producer ends. Do not hand off a pile and leave later
  task drops appending to an unserviced replacement pile. A reply arriving
  after delivery closes is disposed of off the window thread.
- Preserve the existing recovery and shutdown contract. No handler joins
  workers or waits for persistence on the window thread. Domain engines
  retain their scheduling and recovery owners.

**Docs.** Write the rule into `DESIGN.md`, beside rule 15: a task's
lifecycle state belongs to the work owner and reaches it through handles
given at construction; there is no trait over tasks, because there is one
implementation. Update the `app/work.rs` module comment.

**Tests.**
- Adapt the `win::work` tests that call `take_retired` or `close_delivery`.
  Reuse their replacement, cancellation, progress and disposal cases.
- New: dropping a `Task` that holds a reply moves the reply into the pile.
  It watches for a large reply freed on the window's thread at exit, which
  a plausible simplification to a plain drop would bring back.
- Before restructuring, identify coverage for admitted settings and queue
  writes surviving close, the last retirement producer, and stale progress
  not extending a successor's clock. Add a representative test only where
  an existing check does not cover the invariant. Use controlled replies
  and fixed instants, not fixed sleeps or a running service. A lost final
  save or a large drop on the window thread is the failure to watch.

**Done when** common lifecycle forwarding has disappeared, domain-specific
expiry and essential persistence still have their explicit owners, no
retirement producer outlives its consumer, and `build.ps1` is green.

## Stage 3: one owner for the Show commands' shortcuts

Ordinary commands keep their shortcuts in `CommandShortcuts`. Show Search
and Show Commander keep theirs in two separate `Choices` fields,
`show_shortcut` and `commander_shortcut`, so every read and every write must
choose a field. The code asks "is this a Show command?" or "which field is
this mode?" in `core/shortcuts.rs` (`command_place`, `command_rows`),
`app/settings.rs` (`fill_keys`, `update_key_buttons`, `set_shortcut`),
`core/settings.rs` (`Choices::show_shortcut`, `Default`, `restore_defaults`)
and `app/main_window/shortcuts.rs`. These are `matches!` and `if`, not
exhaustive matches, so the compiler finds none of them. `DESIGN.md` window
rule 17 forbids this: change the representation "rather than spreading
agreement checks among callers".

**Change.**
- One value holds both global shortcuts by `Mode`, with one `get` and one
  `set`. Serde keeps the `ui.json` keys `show_shortcut` and
  `commander_shortcut`, so saved settings still load.
- One `CommandId` method answers which mode a Show command shows, or
  `None`. Every special case above calls it. Run the naming review on its
  name.

**Tests.** A `ui.json` that holds both keys loads both shortcuts. Add it
only if no existing test in `core/settings.rs` covers it. It watches for
a saved shortcut lost on upgrade.

**Done when** the two field names appear only in that value's serde
attributes, and nothing outside the new `CommandId` method names
`ShowSearch` or `ShowCommander` to test which kind a command is.

## Stage 4: one module per Settings page

`app/settings.rs` has grown beyond 3,000 lines. `SettingsDialog` is one dialog
procedure for all its pages, and `clicked` and `edited` are long `if/else`
chains that cross page boundaries. Each page already has its own dialog template, but
not its own code, so every settings change lands in the same file, and
parallel sessions conflict there.

**Change.**
- One module per page under `app/settings/`. It owns its page's fill,
  edited and clicked code and its rows of the binding tables.
- `SettingsDialog` keeps what all pages share: the page list, the search
  box, OK, Apply, Cancel, Reset all, the status bar, the draft, the waits
  and the elevated save.
- `SettingsDialog` routes each message to its page with a plain `match` on
  `SettingsPage`. No trait: the enum already lists every page.
- `SettingsPage`'s `template`, `title` and `glyph` become one `row()`, as
  `CommandId::row` is.
- `restore_defaults` stays in `core/settings.rs`, where its tests are.

**Constraints.** Move code; do not rewrite it. The reviewer reads the diff
with `git diff --color-moved` and checks that each handler moved unchanged.
Schedule this stage when no other session edits `app/settings.rs`.

**Done when** page-specific behavior lives in that page's module and shared
draft, validation and save behavior has one owner. A page-only change does
not edit unrelated page handlers; a new shared setting may still need its
type, persistence, defaults, template and catalogue changes.

## Stage 5: one row for each command

A new command needs a variant, its place in `ALL`, `row()`, `glyph()`,
`belongs_to()` and its menu position. These are exhaustive matches, so they
are safe, but they are four edits where one would do.

**Change.**
- `row()` also returns the glyph and the modes the command belongs to, so a
  command's attributes live in one match arm.
- `ALL` stays: menu IDs are built from its order (`0x100 + position`).
- The menu bar's layout stays in `menu_bar`, because it orders menus, not
  commands.
- `BUTTON_GLYPHS` and `SETTINGS_GROUPS` become slices, so adding a row does
  not change a count. `SettingsPage::ALL` stays an array, because
  `SettingsDialog` sizes `pages` by its length.

**Done when** each command's metadata has one row, with its enum variant,
stable `ALL` entry, menu placement, dispatch and catalogues remaining
explicit. Existing saved shortcut names, menu IDs, enablement and mode
availability keep their meanings.

## Not in this plan

- Splitting `main_window.rs`. It is the largest and riskiest
  split. Decide on it after stage 4 shows how much a per-page split helps.
- A trait or a registry for task results. Results stay typed and pass
  through `MainWindow` as they do now.
- `Default` on wire types (stage 1 says why).
