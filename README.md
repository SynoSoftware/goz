# Gozler

[![CI](https://github.com/SynoSoftware/goz/actions/workflows/ci.yml/badge.svg)](https://github.com/SynoSoftware/goz/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Find files instantly. Copy, move and delete them blazingly fast.**

Free and open source, for Windows 10 and 11.

- *Instant file search across NTFS and ReFS drives, inspired by voidtools Everything.*
- *Copy engine inspired by [FastCopy and AngelCOPY](docs/research/fast-copy-delete.md#copiers),
  but faster (Gozler's SSD scheduling cut mixed-file copy time even further
  by [17% in our test](docs/BENCHMARKS.md#ssd-early-dispatch-before-and-after)).*
- *[Folder synchronization](docs/DESIGN.md#synchronize-folders) inspired by
  Robocopy: preview changes before copying, with optional mirroring.*
- *Deletion inspired by [rmbrr](docs/research/fast-copy-delete.md#rmbrr).*
- *UI inspired by Norton Commander, with two panels for convenient copy,
  move and delete.*

Search your Windows drives, then act on what you find. Copy, move or delete
directly from results, or use two panels to keep the source and destination
in view.

**[Download Gozler for Windows x64](https://github.com/SynoSoftware/goz/releases/latest)**
· [What's new](https://github.com/SynoSoftware/goz/releases)

![Instant file search with filters and the selected file's details](docs/images/search.png)

## Find files across your drives

Search filenames across NTFS and ReFS drives as you type, including Dev
Drives. Narrow results by folder, file type, size, date or attributes.
Combine terms, exclude names, or use regular expressions.

- **Go straight to its folder.** Press Enter on a result to open its folder.
  Ctrl+F searches from either panel.
- **Search by folder as fast as by name.** Match part of a folder path to
  find the files below it, without slowing a broad search.
- **Browse millions of results.** Scroll through the full list, sort it,
  or inspect a file's details.
- **Use the results where you need them.** Copy paths, export to CSV, or
  drag files straight into another program.
- **See what takes up space.** Check folder sizes and item counts, or sort
  by size to find the largest folders and files.
- **Big exports don't hold up other searches.** Keep searching while an
  export runs, even with millions of files in deeply nested folders.
- **Results stay current.** New files, changes and renames appear
  automatically, including hard-link renames. Gozler tells you when a
  drive's results may still be incomplete.

The [0.4.0 measurements](https://github.com/SynoSoftware/goz/releases/tag/v0.4.0)
recorded a two-million-file index building in **10 seconds instead of 22**,
and changes appearing within **113 ms**. Copying 100,000 paths to the
clipboard took **59 ms**; dragging 1,000 files started in **10 ms**.

## Fast copy, move and delete

Copy, move or delete what you find directly from search results. Totals
come from the index, so work can start without walking the whole tree to
count files first.

- **Copy small files about 1.8 times faster than robocopy** in our
  [published copy tests](docs/BENCHMARKS.md#stage-e-final-names-and-resume).
- **Delete about 3 times faster than `rmdir`.** 30,000 files in 5,000 folders
  took **6.4 seconds instead of 19.1** in our
  [deletion test](docs/BENCHMARKS.md#copy-move-and-delete-engine).
- **Keep browsing while files transfer.** Check progress, pause, cancel or
  change the queue order from File operations.
- **Continue after an interruption.** After a restart or crash, Gozler offers
  to continue copies, moves and deletes. A half-copied file resumes where it
  stopped.
- **Automatic or your choice.** Automatic adapts copying to the file size
  and drive type. Choose the Windows engine or Gozler's ring copy through
  the speed presets, or adjust the settings yourself.
- **Move within a drive in moments.** A move on the same volume uses a rename
  when possible, without copying the file's contents.

Deletes are permanent: Gozler does not use the Recycle Bin.

## Keep source and destination side by side

The two panels provide a familiar Norton Commander layout for Gozler's
fast file operations. Copy and Move suggest the other panel's folder
automatically. Folder changes appear live, and slow network folders load
without freezing the window.

![Source and destination folders side by side, with F3 to F8 commands below](docs/images/two-panels.png)

The built-in Text and Hex Viewer opens even multi-gigabyte files at once,
without blocking other programs from writing to, renaming or deleting them.

## Coming in the next release

These improvements are in development after
[Gozler 0.5.0](https://github.com/SynoSoftware/goz/releases/tag/v0.5.0):

- **17% less waiting in our mixed SSD copy test.** Starting large files
  earlier, alongside small-file work, reduced median time from **14.32 to
  11.84 seconds**, including the final flush, compared with Gozler's previous
  scheduling. The test copied 6,000 small files and one 1,600 MiB file within
  one NVMe SSD. [Conditions and raw results](docs/BENCHMARKS.md#ssd-early-dispatch-before-and-after).
- **Compare folders before synchronizing.** Preview additions and updates,
  then synchronize. Optional Mirror also removes extra destination files
  after you review the comparison.
- **Transfers stay in the main window.** An integrated pane keeps the queue,
  current file, progress and reports together.
- **Choose how conflicts are handled once.** Update, keep both or skip
  existing files without another question interrupting the transfer.
- **See indexing progress for each drive.** The status bar shows scanning
  progress and when an index is being prepared.

Gozler can start large files while small files are still being copied.
[AngelCOPY's published scheduling](https://github.com/vangel76/AngelCOPY/blob/b5c63a721073a6574667a625bc681d54af3f37df/AngelCopyRunner/NativeCopy.cpp)
finishes small files first. The [benchmark record](docs/BENCHMARKS.md#competitor-context)
explains that difference and the comparisons measured so far.

[Read the draft release notes](docs/WHATS-NEW.md).

## Install and start searching

Download the installer from the
[latest release](https://github.com/SynoSoftware/goz/releases/latest), run
it, and open Gozler from the Start menu. The installer sets up the search
service and upgrades an earlier Gozler installation.

- Windows 10 or 11, x64.
- Instant search indexes fixed NTFS and ReFS drives. Folder panels can also
  browse removable drives and network folders.
- Available in English, Spanish, Dutch and Turkish.

Choose the folders to index in Settings. If the service needs attention,
Gozler offers a repair action in the window. A ZIP download is also
available on the release page; keep all three programs from the same release
together.

<p>
  <img src="docs/images/settings.png" alt="Settings: language, sizes, icons and startup options" width="49%">
  <img src="docs/images/indexed-folders.png" alt="Choose which folders Gozler indexes" width="49%">
</p>

## Search from scripts and terminals

Search from a terminal, or change `es.exe` to `gozler` in an existing
Everything script:

```
gozler report                       # filenames containing report
gozler "*.iso" -sort size -n 10      # the 10 largest ISO files
gozler --json report                # JSON for scripts and agents
gozler -export-csv files.csv ext:cs  # export matching files
```

The default is 20 results; `-n all` returns every match. Scripts and agents
can tell whether an answer is complete from its exit code. Invalid search
terms produce an error. Run `gozler -h` for quick examples or `gozler --help`
for the full reference.

## What's changed since goz v0.2.0?

Since the original command-line index, Gozler has added a Windows search app,
fast file operations, recovery, a Viewer and two panels. See the release
notes for each version:

| Release | Main additions |
| --- | --- |
| [0.5.0](https://github.com/SynoSoftware/goz/releases/tag/v0.5.0) | Two panels for file operations, Text and Hex Viewer, browsing millions of results, creation-date search, and more responsive controls. |
| [0.4.0](https://github.com/SynoSoftware/goz/releases/tag/v0.4.0) | Windows search app, fast copy/move/delete with recovery, ReFS support, folder sizes and a faster indexing service. |
| [0.3.0](https://github.com/SynoSoftware/goz/releases/tag/v0.3.0) | Fuller search language, useful errors and exit codes, bounded command-line output, and an installer that upgrades in place. |

## Performance records

[Benchmarks](docs/BENCHMARKS.md) record the tests behind the speed claims.
The original search and memory comparison with Everything was measured on
**goz v0.2.0**: searches were **1.8 to 6.1 times faster than Everything** in
those tests. Those figures describe that version. Later records cover copy
speed and [reduced memory use for deep-folder exports](docs/research/deep-folder-rows.md).

## Security

The search service indexes file names, sizes and dates with administrator
access. Those names are visible to other local users, as with Everything.
File operations run with your own permissions. Read the
[security model](docs/SECURITY.md) for shared-PC use and reporting vulnerabilities.

## Build and contribute

[Build from source](docs/BUILDING.md) · [Design](docs/DESIGN.md) ·
[Report an issue](https://github.com/SynoSoftware/goz/issues)

[Synapse N.V.](http://synapse.sr) develops Gozler at
[SynoSoftware/goz](https://github.com/SynoSoftware/goz).
It began as a fork of [goz v0.2.0](https://github.com/mustafaahci/goz)
by Mustafa Ahci, who created the original index, daemon and historical
benchmarks. Gozler means “eyes” in Turkish.

Licensed under the [MIT License](LICENSE).
