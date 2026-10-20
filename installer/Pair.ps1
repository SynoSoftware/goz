#Requires -Version 7
# Pairs the built gozler-ui.exe with the gozlerd.exe beside it: writes
# gozlerd.exe's SHA-256 into gozler-ui.exe after the marker that the window's
# win::pairing module holds, so the window offers Use local only for that
# gozlerd.exe (DESIGN.md: Service fixes). It runs after the build, because
# cargo cannot make the window's link wait for gozlerd.exe. It only hashes and
# writes, and it pairs again after either program is rebuilt. cargo may
# hard-link gozler-ui.exe to its copy in deps; the write changes both, which
# keeps them equal.
param(
    # The folder that holds both programs.
    [string] $Folder = (Join-Path (Split-Path $PSScriptRoot) 'target\release')
)

$ErrorActionPreference = 'Stop'

# The same text as win::pairing's MARKER; the hash's 32 bytes follow it.
$marker = 'gozler-ui: paired gozlerd sha256'
$window = Join-Path $Folder 'gozler-ui.exe'
$service = Join-Path $Folder 'gozlerd.exe'

$sha256 = [Convert]::FromHexString((Get-FileHash -LiteralPath $service -Algorithm SHA256).Hash)
$bytes = [IO.File]::ReadAllBytes($window)
# Latin-1 maps each byte to one character, so a character index is a byte
# index.
$text = [Text.Encoding]::Latin1.GetString($bytes)
$at = $text.IndexOf($marker, [StringComparison]::Ordinal)
if ($at -lt 0 -or $at -ne $text.LastIndexOf($marker, [StringComparison]::Ordinal)) {
    throw "$window does not hold the pairing marker exactly once."
}
[Array]::Copy($sha256, 0, $bytes, $at + $marker.Length, $sha256.Length)
# An antivirus scan can hold a freshly linked exe for a few seconds.
for ($try = 1; ; $try++) {
    try {
        [IO.File]::WriteAllBytes($window, $bytes)
        break
    }
    catch {
        if ($try -ge 20) { throw }
        Start-Sleep -Milliseconds 500
    }
}
Write-Host "Paired $window with $service ($([Convert]::ToHexString($sha256)))"
