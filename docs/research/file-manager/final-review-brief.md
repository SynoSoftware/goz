# Final adversarial verdict: the file manager on main

Run after these are on main: the merge (stages 3-7, key bar), the native
panel rows, the native Settings sections and other conversions (fm-native,
including #109), and the stage 8 docs pass. The reviewer took part in no
earlier review of this work.

## Question

Does main now do what `docs/plans/file-manager-spec.md` asks (or, once
stage 8 deleted it, what `docs/DESIGN.md`'s Two panels chapter says), with
no defect a person would see? Read the spec, `.work/file-manager-plan.md`
("Settled owner decisions" and "Owner requests"), then the code.

## Standard for a finding

A concrete sequence of events, with the code line that causes it, that you
tried to refute and could not. A "could" without such a sequence is not a
finding. Report also what can be removed with no change in behavior.

## Where defects were found before

- Selection and focus across rereads, renames and taken entries (stage 4
  took 7 rounds here).
- Each list's own state: Search's list and the two panels keep separate
  results, selection, taken entries and requests (stage 7).
- Merge seams: code from main's window work (toolbar, drop target, column
  fill, Clear's icon) meeting the panels' code.
- Native-control conversions: what the removed drawing used to show
  (taken entries, folder sizes, `..`, section headings) must still show.
- Waits in the window: by owner ruling each wait is a defect; a suspected
  one goes to a GitHub issue (`--repo SynoSoftware/goz`) if not fixed.

## Rules

Read only; no builds, no window probes. Write plainly, no closing
suggestions. Report findings by severity with file, symbol and sequence.
