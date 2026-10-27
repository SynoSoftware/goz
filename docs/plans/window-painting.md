# Native window painting

## Schedule and owner decision

On 2026-10-25 the owner decided: "Close architecture; track painting separately."
The architectural implementation is complete. This follow-up retains its
unmet painting acceptance; moving the work here does not mark that proof passed.
No installation or further painting experiment is scheduled by this decision.
Resume only with a concrete new causal distinction and bounded native correction,
or a further owner decision about proof. Do not repeat rejected flag trials.

Implemented ownership, work, shutdown, update ordering and painting mechanisms
live in [DESIGN.md](../DESIGN.md), not in a proposed module checklist. The closed
architecture plan and execution history remain in Git; its final open-gate
ledger is available at `git show 0b0b5a1:docs/plans/window-architecture.md`.

## Required outcome

At normal interaction speed the window must not visibly flash or show cleared,
half-drawn or doubled controls. The existing acceptance remains: record the same
interaction before and after a correction, and inspect every acquired frame.
A frame showing a cleared, half-drawn or doubled control fails that recording.
Correct settled controls, a green build and native invalidation alone do not
pass it. This is not a frame-rate target or an atomic-publication guarantee.

Cover Search and Commander mode resizing, splitter movement, typing/status updates,
scrolling, mode changes and revealing/changing transfer content. Reuse the
recording actions and coordinates in the saved fixture rather than inventing
an easier workload. Theme, high contrast, uncommon fonts and mixed-monitor DPI
checks remain on the [After install list](ui-after-install.md); do not mark them
passed or change the owner's whole system for an unattended check.

## Current evidence

The retained implementation batches geometry and visibility per native parent,
skips unchanged setters and uses native clipping and ListView buffering.
Changed child rectangles already use `SWP_NOCOPYBITS`, including fallback
placement; structural redraw restores descendant invalidation. The current
source does not establish a missing repaint obligation.

Earlier recordings positively sampled partial Search toolbar/underline,
panel-count, Inspector and mode/transfer publication. Their media and native
reports are under `V:/temp/gozler-window-paint-cause/`; the remaining-source
audit is `V:/temp/gozler-window-remaining-paint-source-audit.md`. These are
acquired-pixel observations, not proof of physical scanout. Native controls
paint independently; that fact is neither a gate pass nor an impossibility
proof.

One subsequent Search-only attribution check ran through
`build.ps1 -Test timing_completion_paint_attribution`. Compilation succeeded;
the 6.62-second disposable test reached its image cap and failed as inconclusive:
659 events, 256 images, 78 dropped images and no dropped events. It was not
retried. The saved early counterexamples join new seven-pixel gaps to old child
pixels while the moved controls have correct geometry/button bounds and full
pending dirty regions. Native child paints subsequently clear those regions.
This narrows that Search case to exposure followed by independent sibling
repaint, without justifying a new correction.

The observer's BitBlt/GdiFlush calls perturb timing, so its timestamps do not
measure ordinary latency or prove presentation. The cap prevents exhaustive
acceptance. Report: `V:/temp/gozler-completion-paint-cause.md`. Command log and
evidence: `V:/temp/gozler-completion-paint-check/run.log` and
`run-18616-1792899622162733700/` beneath that directory. Both temporary module
includes are removed and the native process ended. No diagnostic code or changed
paint policy remains in the product.

## Rejected mechanisms

Reuse the existing evidence rather than measuring these again without a new
supported distinction:

- Geometry/visibility batching is retained, but did not pass all-frame proof.
- Root class redraw, root preserved-region changes, pending-paint flush,
  `SWP_NOREDRAW` plus restoration, toolbar buffering and a fixture DWM-transition
  override retained counterexamples. Existing structural parent redraw scopes
  also did not make independent child publication atomic.
- Whole-window `WS_EX_COMPOSITED` was explicitly tested and rejected: about five
  times the measured UI cycles overall, a Commander mode resize over 200 ms and a
  remaining toolbar counterexample. See
  [the native buffering evaluation](../research/native-window-buffering.md).
- Child `SWP_NOCOPYBITS`, native clipping and structural descendant restoration
  are already present. Adding them again is not a correction.

## Constraints on further work

Windows continues to draw the controls. Preserve existing native controls,
ownership, input/focus/selection and Work lifetimes. No framework, universal
backbuffer, custom-painted control substitute or new worker policy is authorized.
A new painted surface still requires the owner's direct permission under
AGENTS.md. Coordinate geometry by its existing owner; keep expensive work off UI.

A further change must name the exact failing action, its relevant native
state/publication boundary and why the proposed mechanism differs causally from
the rejected attempts. Record the same action after the change. Preserve CPU
responsiveness; claims of performance improvement need matched CPU-cycle evidence.
Retain no unsuccessful candidate, temporary product hook or misleading display
regression suite. Stop an inconclusive experiment instead of silently treating
it as acceptance or starting an automatic retry loop.

## Verified implementation baseline and other work

`58ba378` fixes New folder name scans, synchronous creation-error logging and
repeated Include-summary work. Full57 is green in one full run: formatting,
clippy, workspace tests and private docs; UI 294 passed and 14 ignored. Its
independent affected-contract review is clean. All 311 inputs in
`V:/temp/gozler-completion-full57-inputs.json` match after diagnostic removal;
log: `V:/temp/gozler-window-integrated-build57.log`. Reuse that evidence only
while its source/build inputs match. The earlier broad source reviews and
correction rechecks remain valid within their recorded scopes.

Large actual Include edits remain a separate suspected, unmeasured UI cost in
[#153](https://github.com/SynoSoftware/goz/issues/153). Physical safe-eject,
provider behavior and the owner's installation/environment checks remain
unverified; they are not certified by closing the architecture implementation.
Ubuntu, exact MSRV and supply-chain checks remain with CI.
