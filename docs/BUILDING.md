# Build Gozler

Build on Windows with Rust 1.94 or newer and PowerShell 7. The installer
also needs Inno Setup 7.1 or newer.

From the repository root, validate the workspace, then build the release
programs:

```powershell
pwsh ./build.ps1
pwsh ./build.ps1 -Release
```

The programs are written to `target\release`:

| Program | Purpose |
| --- | --- |
| `gozler-ui.exe` | The Windows search app and file operations. |
| `gozler.exe` | The command-line search client. |
| `gozlerd.exe` | The elevated indexing service. |

## Run the service

In an elevated terminal, run the daemon in the console:

```powershell
.\target\release\gozlerd.exe run --console
```

Or install it as an automatic Windows service:

```powershell
.\target\release\gozlerd.exe install
.\target\release\gozlerd.exe status
```

To remove that service:

```powershell
.\target\release\gozlerd.exe uninstall
```

Keep all three programs from the same build together. The daemon serves
only its matching protocol version. Gozler has its own service and pipe;
leave an installed upstream `goz` service unchanged.

## Build an installer

Double-click `Release.cmd`, or run:

```powershell
pwsh ./installer/Build.ps1
```

The installer is written to
`target\installer\gozler-<version>-x64-Setup.exe`. Each local installer
build raises the patch version in `Cargo.toml`, so it upgrades the previous
installation. GitHub releases build the version named by their tag.

## Further reading

- [Design](DESIGN.md): the workspace, index, command line and window.
- [Benchmarks](BENCHMARKS.md): performance workloads and measurements.
- [Security](SECURITY.md): the service and file-operation trust boundaries.
- [Repository instructions](../AGENTS.md): contribution and validation rules.
