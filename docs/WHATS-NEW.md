# What's new

## Unreleased, after 0.5.0

### 17% less waiting in our mixed SSD copy test

Large files can get moving while other workers set up folders and copy
small files. Starting a folder's known large files earlier reduced median
elapsed time from **14.32 to 11.84 seconds**, including the final flush:
**17.3% less time than Gozler's previous scheduling** in this test.

The test copied 6,000 small files in 1,000 subfolders, with one 1,600 MiB
file beside those subfolders. Five runs before and five after the change
copied within one NVMe SSD on NTFS. Small source files were warmed in cache;
the large file used unbuffered copying. Results vary with the drive, file
mix and cache conditions.

[Benchmark conditions, ranges and raw samples](BENCHMARKS.md#ssd-early-dispatch-before-and-after).

### Keep transfers beside your files

File operations now opens a pane in the main window. See the queue and
the selected transfer's progress together, pause or resume work, reorder
transfers and review any failures. Expand the pane when you need more room,
then return to browsing.

### Choose your copy policy once

Choose how to handle existing files before the copy or move starts: update
changed files, update only newer files, keep both or skip existing files.
The transfer follows that choice without stopping for more conflict questions.

### Compare folders before synchronizing

Preview what will be added, updated or left unchanged, then synchronize
from the source folder to the destination. Keep extra destination files,
or turn on Mirror to include their permanent deletion in the comparison.
You review the comparison before anything is synchronized.

### Watch each drive's indexing progress

See which drives are being indexed from the status bar, even when your
search is limited to another drive. NTFS shows an estimated scanning
percentage when available; otherwise Gozler shows entries found. A
Preparing message covers the final step, and the entry clears when the
drive is ready or fails. Settings shows the same progress.

## Earlier releases

- [0.5.0: the two-panel file manager and Viewer](https://github.com/SynoSoftware/goz/releases/tag/v0.5.0).
- [0.4.0: the Windows app, fast file operations and ReFS](https://github.com/SynoSoftware/goz/releases/tag/v0.4.0).
- [0.3.0: a fuller command line and easier installation](https://github.com/SynoSoftware/goz/releases/tag/v0.3.0).
