# Service settings

This plan adds the three service settings that the window does not have
yet, all on the Drives and service page:

- Users and groups who see every file, a list, which the
  [visibility plan](visibility-plan.md) stores;
- Dates created, a check box, which adds the Date created column and the
  `dc:` search function;
- Rebuild index, a button.

The last two each need a service request that does not exist yet.

**Stages.** The list is its own stage, the visibility plan's stage 3. It
starts at once and lands right after that plan's stage 2, ahead of Dates
created and Rebuild index, because the public release that turns the
visibility filter on for the other users of a shared PC also carries the
page where an administrator changes who sees every file (visibility plan,
"Schedule"). Dates created and Rebuild index follow, scheduled after the
Include page, which `docs/DESIGN.md` now describes ("Include settings" and
"The Include page").

These settings follow the Include page's rules: they are saved for the whole
machine in `HKLM\SOFTWARE\Gozler`, only an administrator changes them, OK and
Apply show the administrator shield, and the service applies a change without
a restart. OK and Apply save every changed service setting through one
elevated copy of the window, as `gozler-ui --save-include` saves Include
today, which then writes each changed value; so a setting adds a value or,
like Dates created, a field of the `Include` value, never a save path of its
own. The plan owns how a request that acts at once,
such as Rebuild index, proves that an administrator sent it.

The approved prototype of these screens, with their English words, is in git
history: `git show b43f935:docs/plans/ui-prototype.html`, a page that opens in
a browser. Its Settings dialog shows the Index page, now Drives and service,
with both controls.

## The Drives and service page

- **Users and groups who see every file** is a list with Add… and Remove and
  an "Every user sees every file" check box above it, as the
  [visibility spec](visibility-spec.md) `V14` describes. Add… opens Windows'
  "Select Users or Groups" dialog, the one Windows' own permission editors
  use. The check box is checked when the list holds Everyone, and checking
  or clearing it adds or removes Everyone; it stores nothing of its own.
  Anyone may read the list. It is the `REG_MULTI_SZ` value `SeeEveryFile`
  under `HKLM\SOFTWARE\Gozler`, beside `Include`, one SID per line, the same
  value that `gozlerd see-every-file` and the setup write. The service reads
  it for each search, so the page sends no request after saving it. The
  stage's After install list holds the screenshots of the list, empty and
  with entries, and of the Select Users or Groups dialog it opens.
- **Dates created** is off by default, because it adds memory per entry to the
  service. The page shows the service's memory, so the person sees the cost
  after they turn it on. It is a search feature, so `gozler` gets `dc:` in the
  same change, because the window adds no search feature that `gozler` lacks.
- **Rebuild index** makes the service read every included drive again, as
  after a restart. It acts at once, so the button itself has the
  administrator shield and asks Windows for permission. It is for an index
  the person no longer trusts; the service keeps itself current without it.
  While it runs, the drives show "indexing" and searches give incomplete
  results, as the status bar's service group says.

They take over two of Everything's options, on its Indexes page: Force
Rebuild becomes Rebuild index, and Index date created becomes Dates created.

## How the service applies Dates created

Dates created decides what the index holds, as Include does, so it is a field
of the saved Include settings, inside the `Include` value, and not a value of
its own. Then `Request::SettingsSaved` reloads it with Include, and a change
reaches every volume by Include's route: the supervisor gives each volume its
new rules, and the volume's tail applies them, or a new tail for a volume
without one. Turning it off drops the dates in place; turning it on rebuilds
the volume, because only a read of the volume brings the dates. The list of
who sees every file stays a value of its own, because the service reads it
for each search and nothing in the index depends on it.

Three conditions, each a silent defect if missed:

- `IncludeSettings::rules_key`, which its `PartialEq` compares, includes the
  field; otherwise `Include::reload` sees no change and returns early.
- `VolumeRules` carries the field, so that `VolumeRules::adds_to` reports
  turning it on as adding and `VolumeState::set_rules` flags the change.
- The field is `#[serde(default)]`, off, so a value saved before it still
  loads.

The type then holds more than Include, so this change renames
`IncludeSettings` to `IndexSettings`. The registry value keeps its name
`Include`, because renaming it would need a migration, and the wire carries
the fields, not the type's name, so neither saved data nor the protocol
changes.

**`dc:` while Dates created is off** is refused, with a message that names
the setting, as a scope on no indexed volume is refused: an empty answer
would read as "no such file". It is not the parser's "unknown function"
(exit 4), because the function exists.

## How the service runs Rebuild index

- **The request goes to the supervisor,** which asks each volume's tail for a
  new build between journal batches, as it gives a tail changed Include
  rules, and starts a tail for a volume without one: a snapshot volume, or
  one it gave up on, which gets a new revival budget, as after a restart.
  The route for changed Include rules already does this
  (`Fleet::apply_settings`).
- **The build is a `RescanReason` of its own** that does not count against
  the rescan budget (`MAX_RESCANS`), as the build for changed Include rules
  does not, because the person asked for it.
- **One value says what the next build must do,** next to the rules in
  `VolumeState`, and `VolumeState::begin_build` clears it, as it clears the
  changed-rules flag. So a rules change and a rebuild that arrive together
  cause one build, and a request that arrives during a build causes one more.
- **The administrator check** can use the visibility plan's
  `pipe_client_token` when this lands after that plan's stage 2; before it,
  this plan needs its own way to read the caller.
- **The journal read policy.** This change edits `tail_loop` anyway, so it
  may also move `ReadFailures` into a type of its own that takes the time
  (`now: Instant`) and returns what to do: retry after a sleep, and whether
  this failure crossed the degrade line, or escalate. The loop keeps the log
  lines and the state calls. Its tests replace
  `retry_policy_is_bounded_and_degrades`, which pins only constants: 15 s of
  failures never escalate, failures past 30 s escalate, recovery is reported
  only after a degrade, and the backoff stops at its cap. On its own the move
  is not worth a change, because the loop is its only caller and the policy
  works.

## The Date created column

The change that adds the Date created column also gives each column one row,
as `CommandId::row` does for a command. Three of `Column`'s facts in
`core/model.rs` (`shown_by_default`, `can_hide`, `right_aligned`) are
`matches!` or `==`, so a new column silently gets "shown by default, can be
hidden, left-aligned". Date created must choose its default on purpose,
because it costs service memory and its index setting is off by default.
`Column::row(self) -> ColumnRow` is one exhaustive match, and
`ColumnRow { header, menu_label, width: Dip, shown_by_default: bool, can_hide:
bool, right_aligned: bool, sort_key: Option<SortKey>, export_column:
Option<EsColumn> }` replaces the per-column functions. `text::cell` stays
apart, because it formats data. The names that `ui.json` saves do not change,
so saved settings still load. No new test: the compiler forces each row.

## After install: the Include page

The Include page shipped with protocol 8, and the installed service speaks an
older protocol, so the owner checks these after installing, with Windows'
permission prompt:

- the Include page and Indexed folders look as the prototype shows them, and
  the tree's partial check boxes show where folders below differ;
- unchecking a folder removes its entries, and `gozler` finds none of them;
- checking it again shows the drive indexing, then finds them again;
- a name pattern and Exclude hidden take effect at once;
- unchecking a drive shows it as "Left out" and removes it from Location;
- saying no to the permission prompt keeps the dialog open with the edits.
