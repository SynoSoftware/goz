# What's new

## Gozler 0.5.5 — Faster copying and folder synchronization

Possibly the fastest copy algorithm on the planet (educated guess—we haven't tested that ranking): 17% less waiting after further optimizing our previous AngelCopy/FastCopy-inspired algorithm.

### Copy and move

- **Faster mixed-file copying.** Our mixed SSD copy test dropped from 14.32 to 11.84 seconds, including the final flush. We haven’t rerun comparisons with other copiers, but our educated guess is that Gozler may now be the fastest file copier on the planet. [See the benchmark](https://github.com/SynoSoftware/goz/blob/main/docs/BENCHMARKS.md#ssd-early-dispatch-before-and-after).
- **Transfers take turns on busy disks.** Work on independent disks can run together. Transfers that need the same physical disk wait their turn, even across different drive letters. Waiting work does not hold up unrelated transfers.
- **Choose how to handle existing files once.** Update changed files, Update newer files, Keep both or Skip existing files. Gozler follows your choice without more conflict questions. The default, Update changed files, also replaces newer destination files.
- **Put urgent work first.** Drag to reorder the queue, or use Run now. Conflicting transfers pause and return afterward. Transfers you paused yourself stay paused.
- **Set the number of transfers per device.** Choose one, two or three under Settings > File operations. The default is one.

### Synchronize folders

- **Sync directly from Copy.** In Two panels, select one folder and press F5. Choose Sync to keep extra files, or Mirror to remove them. Copy stays the default; neither extra action opens a preview.
- **Compare before you copy.** File > Synchronize folders previews what will be added, updated or left unchanged. Changes go from Source to Destination. In Two panels, the active folder is the source.
- **Keep extra files, or choose Mirror.** Destination-only files stay by default. Mirror permanently deletes approved extras after additions and updates succeed and the required checks pass.
- **Pause, cancel and review progress.** Synchronization uses the same queue and reports as Copy and Move. After an interruption, compare again before continuing.

### File transfers in the main window

- **Progress, the queue and reports stay together.** The File transfers pane replaces the separate operation window. Open it from View > File transfers or click the operation name in the status bar.
- **Keep browsing while work runs.** Transfers do not open the pane or take focus. Maximize it when you need more room, then restore it to return to your files.
- **Review failures when convenient.** A failed file does not stop other possible work. Reports show the affected files and what went wrong.

### A more responsive window

- **Large lists and slow reads do less to interrupt your work.** More selection, folder and report processing runs in the background. Slow reads show what Gozler is waiting for, with Cancel where possible.
- **Exit without waiting for settings writes.** The interface closes while Gozler finishes saving in the background.
- **See which drives are still indexing.** Progress appears for each drive, and narrow status bars still indicate when results may be incomplete.

### Before upgrading

**A major app refactoring.** Background work, selection, event handling and shutdown have been reworked. Manual testing has been limited. **Expect regressions. Please report them.**

The installer upgrades Gozler 0.5.0 in place and updates all three programs together. Keep `gozler.exe`, `gozlerd.exe` and `gozler-ui.exe` from the same release.

## Earlier releases

- [0.5.0: the two-panel file manager and Viewer](https://github.com/SynoSoftware/goz/releases/tag/v0.5.0).
- [0.4.0: the Windows app, fast file operations and ReFS](https://github.com/SynoSoftware/goz/releases/tag/v0.4.0).
- [0.3.0: a fuller command line and easier installation](https://github.com/SynoSoftware/goz/releases/tag/v0.3.0).
