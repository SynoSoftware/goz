# gozler

[![CI](https://github.com/mustafaahci/goz/actions/workflows/ci.yml/badge.svg)](https://github.com/mustafaahci/goz/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6.svg)](#requirements)
[![Rust](https://img.shields.io/badge/rust-1.94%2B-orange.svg)](rust-toolchain.toml)

**gozler** *(gözler, Turkish for "eyes")* finds any file on Windows as you type: every name on your NTFS and ReFS drives, in milliseconds. It is an open-source alternative to voidtools Everything, written in Rust.

Gozler-UI, its app, searches the same way, and copies, moves and deletes what you find blazingly fast, without File Explorer. It deletes small files 3 times faster than `rmdir`, and copies them 1.8 times faster than `robocopy`.

**[Download the installer](https://github.com/SynoSoftware/goz/releases/latest)** for Windows 10 and 11, x64.

[Synapse N.V.](http://synapse.sr) develops Gozler at [SynoSoftware/goz](https://github.com/SynoSoftware/goz). Gozler started as a fork of [goz](https://github.com/mustafaahci/goz) *(göz, "eye")* 0.2.0 by [Mustafa Ahci](https://github.com/mustafaahci), as vim started from vi.

## Highlights

### Gozler-UI

#### Search

- **Results as you type.** Filters narrow the list by kind, folder, date, size and attributes.
- **Search by folder as fast as by name.** With Match path on, part of a folder name lists every file below it.
- **Act on a result where it is.** Open it, copy its path, export the list, or drag it into another program. The inspector shows the selected file's details, and a folder's size and item count.
- **No waiting.** Copying 100,000 paths to the clipboard takes 59 ms. A drag of 1,000 files starts in 10 ms. The app stays under 16 MB of private memory.
- **One-click repair.** When the service is stopped, missing or older than the app, a button in the app repairs it.
- **In your language:** English, Spanish, Dutch and Turkish.

#### Copy, move and delete

- **Delete 3 times faster than `rmdir`.** 30,000 files of 1 to 64 KB in 5,000 folders took 6.4 seconds, against 19.1 seconds for `rmdir /s /q`. Gozler deletes the way [rmbrr](https://github.com/mtopolski/rmbrr) does: POSIX delete, on one worker per processor. It also deletes faster than robocopy.
- **Copy 1.8 times faster than robocopy,** comparable with AngelCOPY and FastCopy. The 1.8 is for small files, against `robocopy /e /mt:16` ([benchmarks](docs/BENCHMARKS.md#stage-e-final-names-and-resume)). A move is faster than robocopy too, and a move within one drive is a rename, so it takes milliseconds at any size.
- **Work starts at once.** Gozler takes the totals from its index, so it never counts the files before it starts.
- **Drop files on a folder** in the results to copy or move them there. Drop with the right button to choose Copy here or Move here.
- **Every operation in view.** The File operations window shows the current file, its progress, and the speed in bytes and in files per second. Pause, cancel, or drag an operation to another place in the queue.
- **Picks up after a crash.** When a copy, move or delete is interrupted, for example because Windows restarted, Gozler offers to continue it the next time it starts. A half-copied file continues where it stopped.

### gozlerd, the service

- **Faster than Everything.** 1.8x to 6.1x quicker across every query in our [benchmarks](docs/BENCHMARKS.md), on the same machine with the same result sets.
- **NTFS and ReFS.** Dev Drives and other ReFS volumes are searched and kept live just like NTFS. ReFS has no MFT to read, so gozler walks the volume's folders once, then follows its USN journal.
- **Ready sooner after a restart.** The index of 2 million files builds in 10 seconds, down from 22.
- **Live.** Changes show in the results within 113 ms, including hard-link renames that Everything 1.4 misses. The USN journal is sized to the volume automatically: a 2 TB disk gets about four days of changes, not the 32 MB Windows default that Everything leaves you to raise by hand.
- **Light.** About 12 MB resident when idle and 215 MB under heavy querying, with an index of about 4 million entries.
- **Unfazed by deep folders.** A search over 5.5 million files 200 folders deep needed more than 12 GB. A full export of that tree now needs 183 MB beyond the index ([details](#deep-folders)).
- **A large export never holds up other searches.** On a tree of 5.6 million entries, an export holds the index for at most about 0.04 seconds at a time, so other searches and live updates keep running.
- **Honest about state.** Every volume reports whether it is provably in sync, rescanning, or offline. It never serves stale results silently.

### Command line

- **Scriptable.** A script written for Everything's `es.exe` needs only the program name changed to `gozler`. `--json` prints the results as JSON, and each outcome has its own exit code.
- **Folder sizes.** A folder's size is the total of the files below it, in `-size`, the exports, `size:` and the size sort.
- **Starts with an overview.** `gozler` alone shows the version, whether the service runs, and how to start it. `-h` prints help that fits on one screen.

## Requirements

- Windows 10 / 11 (reads the NTFS MFT, or walks the folders of a ReFS volume, and follows the USN journal; NTFS and ReFS volumes only)
- Rust 1.94 or newer, only if you build from source

## Install

Run `gozler-<version>-x64-Setup.exe` from the [gozler releases](https://github.com/SynoSoftware/goz/releases). It installs the three programs to `C:\Program Files\Gozler`, adds a Gozler shortcut to the Start menu, adds that folder to the machine `PATH`, and installs and starts the service. It upgrades an earlier gozler in place, and always installs `gozler.exe` and `gozlerd.exe` together, because the service answers only programs from its own release. When it finds goz, or a gozler installed by hand in another folder, it offers to remove them once gozler runs: their service, files and `PATH` entries, and goz's logs. A silent install removes nothing. Or download `gozler.exe`, `gozlerd.exe` and `gozler-ui.exe` in the release's zip, or build from source:

```
cargo build --release
```

Either way you get three binaries (in `target\release` when built locally):

| Binary          | Role                                                                            |
| --------------- | ------------------------------------------------------------------------------- |
| `gozlerd.exe`   | The indexing daemon. Runs elevated; owns the index and serves queries.          |
| `gozler.exe`    | The command-line client. Runs unelevated; talks to the daemon.                  |
| `gozler-ui.exe` | The Gozler window. Searches through the daemon, and copies, moves and deletes files itself. Runs unelevated. |

Run the daemon in an elevated terminal:

```
.\target\release\gozlerd.exe run --console
```

Or install it as an auto-start Windows service (elevated):

```
.\target\release\gozlerd.exe install     # register and start
.\target\release\gozlerd.exe status      # check state
.\target\release\gozlerd.exe uninstall   # stop and remove
```

To build an installer instead, double-click `Release.cmd`, or run `pwsh ./installer/Build.ps1`, with Inno Setup 7.1 or later installed. It writes `target\installer\gozler-<version>-x64-Setup.exe`, the installer the release publishes. Each local run first raises the patch version in `Cargo.toml`, such as 0.2.0 to 0.2.1, so each setup upgrades the one before; the release builds the version its tag names. Uninstalling removes the binaries, the Start menu shortcut, the `PATH` entry and the service, and, for the person who uninstalls, the window's sign-in entry and the "Search with Gozler" command that earlier versions added to File Explorer.

## Usage

Open Gozler from the Start menu, type, and open what it finds. Help > Search syntax lists the search terms.

To copy, move or delete what it finds, select the results and choose File > Copy to folder…, Move to folder… or Delete. Edit > Cut and Paste work too, and so does dropping files on a folder row. View > File operations shows each operation's progress and lets you pause, cancel or reorder it. A delete is permanent: Gozler does not use the Recycle Bin.

Or search from any terminal. `gozler` alone shows whether the service runs and how to start. `gozler -h` prints a quick help that fits on one screen, and `gozler --help` the full reference: every option, search function and exit code.

```
gozler report                      # names that contain report
gozler readme*.txt                 # .txt files that start with readme
gozler report !draft               # names with report but not draft
gozler C:\repo\ *.cs               # .cs files anywhere in C:\repo
gozler *.log dm:7days              # .log files changed in the last 7 days
gozler "size:>1gb"                 # files and folders larger than 1 GB
gozler *.iso -sort size -n 10      # the 10 largest .iso files
gozler -n all report               # every match, not just the first 20
gozler -export-csv out.csv ext:cs  # save the .cs files to out.csv
gozler --json report               # the matches as JSON
gozler --status                    # whether the index is ready
```

gozler shows 20 results unless you pass `-n`. Put quotes around a term that contains `|`, `<`, `>` or `;`. gozler exits 0 when it is done, 2 when results may be incomplete, and 10 when the limit left some out.

A script written for Everything's `es.exe` or for goz 0.2.0 needs only the program name changed to `gozler`: every option keeps its name and purpose. gozler's own options start with `--`, as `--json` does.

## Performance

goz vs Everything 1.4.1.1032 (`es.exe` 1.1.0.30), same machine, each warm. Single substring term, `-sort name` on both, full result set, median of 6 runs after 2 warmups, interleaved. Median milliseconds; **bold** is faster.

| Query                    | Matches   | Everything (file) | goz (file) | Everything (pipe) | goz (pipe) |
| ------------------------ | --------: | ----------------: | ---------: | ----------------: | ---------: |
| `kernel32.dll`           | 38        | 47.5              | **27.0**   | 32.0              | **14.0**   |
| `.pdf`                   | 424       | 51.0              | **27.0**   | 39.5              | **14.5**   |
| `.config`                | 4,287     | 71.5              | **35.0**   | 44.0              | **18.5**   |
| `.mui`                   | 21,789    | 148.0             | **39.0**   | 93.5              | **26.5**   |
| `.dll`                   | 141,666   | 724.0             | **139.0**  | 416.5             | **119.0**  |
| `e` (~73% of all files)  | 2,884,148 | 14,700            | **2,394**  | 7,511             | **2,470**  |

Faster in every cell: 1.8x to 6.1x to a file, 2.3x to 3.5x piped.

Memory, same box, each index warm at ~3.97M entries:

| Metric                                          | Everything | goz        |
| ----------------------------------------------- | ---------: | ---------: |
| Binaries shipped                                | **2.3 MB** | 4.1 MB     |
| Committed memory (private bytes)                | 527 MB     | **525 MB** |
| Resident RAM, idle after indexing               | 369 MB     | **12 MB**  |
| Resident RAM, steady after heavy querying       | 554 MB     | **215 MB** |
| Resident RAM, peak (2.88M-match sorted export)  | 1.2 GB     | 1.2 GB     |
| Committed memory, peak (same query)             | **1.2 GB** | 1.5 GB     |

goz wins the resident-RAM rows that matter for a background daemon and edges out committed memory at rest. It loses two: a larger (statically linked) binary, and ~0.3 GB more peak committed memory during a full sorted export. Hardware and methodology are in [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

### Deep folders

A search result used to carry its full path, so a deep folder tree multiplied the memory of every result. Now the daemon keeps each result as a 16-byte row, reads each page's names from its index, sends each folder once, and the client rebuilds the paths. A sort compares numbers: a rank for each name, and for a path sort a rank for each folder.

The test tree is a copy that copied into itself: a chain of 190 folders, then folders of 9 files each, so a path is about 2.1 KB. The figures are the daemon's peak heap above its index and its time, for an export of every entry, measured on the same laptop before and after the changes:

| Export of 280,330 entries | Before | Now |
| ------------------------- | -----: | --: |
| Sorted by name            | 1,189 MB, 4.4 s | **8 MB, 0.04 s** |
| Sorted by path            | 2,208 MB, 10.4 s | **8 MB, 0.06 s** |
| Bytes through the pipe    | 571 MB | **7.3 MB** |

At 5.6 million entries, the old daemon would need about 25 GB, which this laptop does not have. The daemon now sorts numbers instead of names and reads each page's rows from the index as it sends them. So a full export needs 0.2 GB instead of 1.0 GB, and the daemon's part takes 1.2 s by name and 1.5 s by path instead of 6.1 s and 5.2 s. The method and every figure are in [deep folder rows](docs/research/deep-folder-rows.md).

## How it works

gozler is a small workspace of seven crates: a pure, OS-agnostic core (parsing, index, query engine), a thin `unsafe` Win32 layer, an indexing daemon, a named-pipe client library, a CLI client, the window, and the window's copy, move and delete engine. The design notes, including how the index stays coherent and how hard-link reconciliation works, are in [docs/DESIGN.md](docs/DESIGN.md).

## Security

The daemon runs elevated and indexes filenames from a raw MFT read, or a ReFS folder walk with backup semantics, which both bypass per-file ACLs. Any authenticated local user can query the resulting index (filenames, sizes, and timestamps; not file contents), the same trust model as Everything. The window copies, moves and deletes files in its own process, with your own permissions, never through the elevated daemon. See [docs/SECURITY.md](docs/SECURITY.md) for the full model and how to report a vulnerability.

## Contributing

Issues and pull requests welcome.

## Credits

[Synapse N.V.](http://synapse.sr) develops gozler. [Mustafa Ahci](https://github.com/mustafaahci) created goz ([mustafaahci/goz](https://github.com/mustafaahci/goz)) and wrote it up to version 0.2.0, the start of gozler: its first MFT index and daemon, and the benchmarks above.

## License

Licensed under the [MIT License](LICENSE).
