# File manager: working notes

These are the lead's working notes from building the file manager (Two
panels, search in a panel, the Viewer, file commands, and the native
drawing of the window's controls), kept so that no reasoning, rejected
design, measurement or plan is lost. They describe how the work went,
not how gozler works now: `docs/DESIGN.md` ("Two panels", "File
operations") holds the current rules, and where a note disagrees with
it, DESIGN.md is right. The plan itself is in history before commit
66ed9ef deleted `docs/plans/file-manager-spec.md`. Open work is in the
GitHub issues the notes name (#104, #106-#114).

| File | What it holds |
| --- | --- |
| `file-manager-plan.md` | the plan file: owner rulings, requests in order, confirmed findings, build rules, estimates, the pause and resume |
| `stage4-round4.md` | stage 4's selection and focus rules, round 4 |
| `stage5-design-review.md` | the Copy/Move dialog's design review |
| `stage6-viewer-wiring.md` | how the Viewer reads, caches and survives a removed drive |
| `stage7-design.md`, `stage7-design-corrections.md` | search in a panel: one state per list, and corrections |
| `stage7-after-merge-fixes.md` | stage 7's review findings and their fixes |
| `stage7b-brief.md` | the key bar and panel rows move to native drawing |
| `custom-controls-report.md`, `native-controls-review.md` | which hand-drawn controls Windows can draw instead, and why the rest stay drawn |
| `merge-main-notes.md`, `merge-into-main.md` | the merges with main: conflicts and what to keep |
| `stage8-context-and-help.md` | where the spec's rules and citations moved |
| `final-review-brief.md`, `final-review-fixes.md` | the final adversarial review and its fix round |
| `memory.txt`, `minwidth.txt` | stage 7's memory budget and minimum window width measurements |
| `scripts/` | the probe scripts that took the screenshots and the measurements; they expect the `.work` folder layout they ran in |
