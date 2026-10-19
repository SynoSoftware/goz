# gozler

[![CI](https://github.com/mustafaahci/goz/actions/workflows/ci.yml/badge.svg)](https://github.com/mustafaahci/goz/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6.svg)](#requirements)
[![Rust](https://img.shields.io/badge/rust-1.94%2B-orange.svg)](rust-toolchain.toml)

**gozler** *(gözler, Turkish for "eyes")* is an instant file-search engine for Windows NTFS and ReFS, written in Rust. A fast, open-source alternative to voidtools Everything.

[Synapse N.V.](http://synapse.sr) develops gozler at [SynoSoftware/goz](https://github.com/SynoSoftware/goz). gozler started as a fork of [goz](https://github.com/mustafaahci/goz) *(göz, "eye")* 0.2.0 by [Mustafa Ahci](https://github.com/mustafaahci), as vim started from vi.

gozler indexes every filename on your fixed NTFS and ReFS volumes, Dev Drives included, and answers substring queries in single-digit milliseconds, in the Gozler window, from the command line, or from your own tools over a named pipe.

## Highlights

- **Fast.** 1.8x to 6.1x quicker than Everything across every query in our [benchmarks](docs/BENCHMARKS.md), on the same machine with the same result sets.
- **Light.** ~215 MB resident under heavy querying and ~12 MB idle, against a ~3.97M-entry index.
- **Live.** Tails the USN journal, so the index stays in sync as files change, including hard-link renames that Everything 1.4 misses. The journal is sized to the volume automatically (a 2 TB disk gets about four days of changes, not the 32 MB Windows default that Everything leaves you to raise by hand), and a tail thread that wedges on a sick disk or dies outright is cancelled or revived automatically.
- **Scriptable.** Native `--json` output and a subset of `es.exe`-compatible options with matching exit codes, so it drops into existing Everything workflows.
- **Honest about state.** Every volume reports whether it is provably in sync, rescanning, or offline. It never serves stale results silently.

## Requirements

- Windows 10 / 11 (reads the NTFS MFT, or walks the folders of a ReFS volume, and follows the USN journal; NTFS and ReFS volumes only)
- Rust 1.94 or newer, only if you build from source

## Install

Run `gozler-<version>-x64-Setup.exe` from the [gozler releases](https://github.com/SynoSoftware/goz/releases). It installs the three programs to `C:\Program Files\Gozler`, adds a Gozler shortcut to the Start menu, adds that folder to the machine `PATH`, and installs and starts the service. It upgrades an earlier gozler in place. When it finds goz, or a gozler installed by hand in another folder, it offers to remove them once gozler runs: their service, files and `PATH` entries, and goz's logs. A silent install removes nothing. Or download `gozler.exe`, `gozlerd.exe` and `gozler-ui.exe` in the release's zip, or build from source:

```
cargo build --release
```

Either way you get three binaries (in `target\release` when built locally):

| Binary          | Role                                                                            |
| --------------- | ------------------------------------------------------------------------------- |
| `gozlerd.exe`   | The indexing daemon. Runs elevated; owns the index and serves queries.          |
| `gozler.exe`    | The command-line client. Runs unelevated; talks to the daemon.                  |
| `gozler-ui.exe` | The Gozler window, a native Windows search window. Runs unelevated; talks to the daemon. |

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

To build an installer instead, double-click `Release.cmd`, or run `pwsh ./installer/Build.ps1`, with Inno Setup 7.1 or later installed. It writes `target\installer\gozler-<version>-x64-Setup.exe`, the installer the release publishes. Each local run first raises the patch version in `Cargo.toml`, such as 0.2.0 to 0.2.1, so each setup upgrades the one before; the release builds the version its tag names. Uninstalling removes the binaries, the Start menu shortcut, the `PATH` entry and the service, and, for the person who uninstalls, the window's sign-in entry and its "Search with Gozler" command in File Explorer.

## Usage

Open Gozler from the Start menu, type, and open what it finds. Help > Search syntax lists the search terms.

Or query the running daemon from any terminal:

```
# search everywhere
gozler report.pdf

# scope to a folder, cap results, add columns, sort, export
gozler -path C:\Users\me invoice -n 50 -size -dm -sort-path -export-csv out.csv

# machine-readable output
gozler --json kernel32.dll

# index and daemon status
gozler --status
```

`gozler` accepts a subset of `es.exe` options (`-path`, `-n` / `-max-results`, the `-sort` family, `-size`, `-dm`, `-export-csv`, and the CSV-shape options) and keeps `es.exe`'s exit codes 4 to 9; codes 1, 2, 3 and 10 are its own, and `gozler --help` lists them all. Unsupported options exit `6`, the same as `es.exe`.

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

## How it works

gozler is a small workspace of six crates: a pure, OS-agnostic core (parsing, index, query engine), a thin `unsafe` Win32 layer, an indexing daemon, a named-pipe client library, a CLI client, and the window. The design notes, including how the index stays coherent and how hard-link reconciliation works, are in [docs/DESIGN.md](docs/DESIGN.md).

## Security

The daemon runs elevated and indexes filenames from a raw MFT read, or a ReFS folder walk with backup semantics, which both bypass per-file ACLs. Any authenticated local user can query the resulting index (filenames, sizes, and timestamps; not file contents), the same trust model as Everything. See [docs/SECURITY.md](docs/SECURITY.md) for the full model and how to report a vulnerability.

## Contributing

Issues and pull requests welcome.

## Credits

[Synapse N.V.](http://synapse.sr) develops gozler. [Mustafa Ahci](https://github.com/mustafaahci) created goz ([mustafaahci/goz](https://github.com/mustafaahci/goz)) and wrote it up to version 0.2.0, the start of gozler: its first MFT index and daemon, and the benchmarks above.

## License

Licensed under the [MIT License](LICENSE).
