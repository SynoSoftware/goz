#Requires -Version 7
# Builds the release binaries and packages them with Setup.iss into
# target\installer\gozler-<version>-x64-Setup.exe.
param(
    # ISCC.exe from Inno Setup 7.1 or later. By default, the one on PATH or in
    # a standard Inno Setup 7 installation folder.
    [string] $Compiler
)

$ErrorActionPreference = 'Stop'

if (-not $Compiler) {
    $Compiler = @(
        (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
        Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 7\ISCC.exe'
        Join-Path $env:ProgramFiles 'Inno Setup 7\ISCC.exe'
        Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 7\ISCC.exe'
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
    if (-not $Compiler) { throw 'Inno Setup 7 was not found. Install it or pass -Compiler.' }
}

$process = Get-Process -Id $PID
$priority = $process.PriorityClass
$rustFlags = $env:RUSTFLAGS
Push-Location (Split-Path $PSScriptRoot)
try {
    $package = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages |
        Where-Object name -eq 'gozler-daemon'
    if (-not $package) { throw 'cargo metadata did not list gozler-daemon.' }

    # Below normal, as in check.ps1, so the build does not slow interactive
    # work. RUSTFLAGS would replace the static CRT flag in .cargo/config.toml,
    # and the installed service would then need vcruntime140.dll.
    $process.PriorityClass = 'BelowNormal'
    $env:RUSTFLAGS = $null
    cargo build --release --workspace
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    & $Compiler /Qp "/DAppVersion=$($package.version)" "/DAppPublisher=$($package.authors -join ', ')" `
        "/DAppUrl=$($package.repository)" installer\Setup.iss
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
finally {
    $process.PriorityClass = $priority
    $env:RUSTFLAGS = $rustFlags
    Pop-Location
}
