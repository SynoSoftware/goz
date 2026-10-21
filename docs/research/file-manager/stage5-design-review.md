# Stage 5 design review: findings and lead decisions

Review of `fm-implementer-stage5-design.md` (adversarial, Opus, 2026-10-21).
Verdict: needs revision. Revise the design for every item below before
writing stage 5's code. Each item is a claim from reading; reproduce cheaply
where you can and tell the lead any you find wrong, with the reason.

Main simplification the reviewer proposes, accepted by the lead: the
dialog applies row 1 of prepare's table (the box left as it started, or a
path whose last component is the selected entry's own name, exactly)
itself, because it is text only, and sends `Folder(parent)`. Prepare then
decides only between rows 2 and 3. This removes M1's and M2's failures.

## High

- **H1. The middle group must show a running operation.** The approved
  prototype (screens "Copy starting" and "Copy running") draws "Copying
  Projects to D:\Backup · 38 %" with a bar and Cancel in the status bar's
  middle group, and spec "File operations" step 5 says a short operation
  shows only there. Extend `Job` to cover a running file operation (text, a
  measured bar or a moving one for a delete, Cancel), fed from the
  progress the File operations window's POLL timer already reads (it runs
  while the window is hidden). With several running, the existing
  `Model::job` rule picks the newest. Add the running texts to
  `StatusWidest::running`. Both modes.
- **H2. A second operation into the same new folder is refused.** A
  creation of a missing destination folder that gets
  `ERROR_ALREADY_EXISTS` on a folder must succeed and read the folder's
  names, as `copy.rs` `folder_target` already does; a conflict that
  appears after the question then follows the run's existing rules.

## Medium

- **M1. A rename that changes only the case.** Row 1 must compare the
  selected entry's name exactly, not folded. In prepare, row 2 ("opens as
  a folder") does not apply when the opened folder's identity equals the
  selected entry's identity: that goes to row 3 (`Named`) and then to
  `Work::Rename`. Example: F6 on folder `Photos`, typed `photos`.
- **M2. Texts naming the typed path as a folder.** Removed by the dialog
  applying row 1.
- **M3. A trailing `\` is lost before the dialog decides.**
  `core::panel::typed_path` (through `winpath::absolute`) drops it, so a
  file copied to `D:\Backup\2026\` becomes a file named `2026`. Read the
  trailing `\` or `/` from the raw text first; pin it in the core test of
  the dialog's resolution.
- **M4. "Opens as a folder" and the walk up need the directory
  attribute.** `Access::List` opens a file too. Row 2 and the walk up
  require `FILE_ATTRIBUTE_DIRECTORY`; walk up only on `is_gone` (not
  found); any other error refuses with that error, as today.
- **M5. The 2-second opening is per operation.** Keep a start instant for
  each operation (`known` becomes a map id → Instant), excluding the time
  its conflict question is open (`State::Prepared` counts as not started).
  The timer is set to the earliest deadline among live operations; the
  window shows for an operation only when its own deadline passed and it
  has not ended; operations present when the person pressed Hide do not
  show it again.
- **M6. The ended line's texts.** Lead decisions:
  - The line keeps the prototype's form, "Copied 3,310 items to
    D:\Backup", with the count of items the operation finished. A long
    path is cut with an ellipsis and the tooltip shows it whole, as the
    middle group does today.
  - Draft one text per kind (Copy, Move, Delete, Rename, New folder) and per
    ending (complete; complete with failures, naming how many failed;
    cancelled), following `docs/WRITING.md`. A refused operation shows no
    line: its message box already said why. Put the texts in `en.json`
    and the four catalogues, add them to `StatusWidest::done`, and write
    the rule into the spec's "File operations" step 6.
- **M7. Names Windows refuses.** The engine opens paths with `\\?\`, which
  turns off Windows' trimming and reserved names. Lead decision: New
  folder and the Copy/Move box resolve each typed component as Windows'
  own path rules do: trailing dots and spaces are trimmed, as File
  Explorer does; a reserved device name (`CON`, `PRN`, `AUX`, `NUL`,
  `COM1`-`COM9`, `LPT1`-`LPT9`, with or without an extension) is refused
  with a message box. One core rule, used by both dialogs; add it to the
  spec's New folder and Copy and Move sections.

## Low

- **L1.** Ctrl+V gets past a typed-character refusal: apply the same core
  name rule at Create.
- **L2.** Continue after a crash: `run_transfer`'s loop over `missing`
  checks arrival under the source's name; use the `Named` name there.
- **L3.** `core::panel::ends_in` tests only "equal to the panel's folder or
  its parent"; a copy that creates `X:\Backup\2026\Oct\` must reach a
  panel on `X:\Backup`: use `is_within` for the destination.
- **L4.** New folder's remembered name must be tied to its folder: drop it
  on navigation or when the NewFolder operation fails.
- **L5.** `Window::report` removes a copy from the queue before the main
  window reads it: say where the main window reads the ended request and
  outcome (for example a "last ended" value in `Window`).
- **L6.** `Delete` and `Shift+Delete` are keys of the focused panel list
  (`PANEL_KEYS`), as Search's list has them in `control_key`; only F8 is an
  accelerator, so Delete in the path box edits text.
- **L7.** `Saved::restore` refuses a `name` key with more than one source,
  or on a Delete.
- **L8.** The `DestinationMissing` test uses a volume GUID path that
  cannot exist, not an unmapped drive letter.
- **L9.** Include the one-entry dialog line, "Copy report.pdf to:".

## What the design adds that the spec does not need

- Row 1 in the engine: it belongs in the dialog.
- An owner check of the `DestinationMissing` text: not needed; step 7 and
  `docs/WRITING.md` cover it, and nothing waits for the owner.
