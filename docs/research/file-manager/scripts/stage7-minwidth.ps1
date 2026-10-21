# The smallest client width of Two panels in each language, in DIP: the
# window is sized to 100 by 100 pixels, its WM_GETMINMAXINFO answer keeps it
# at its minimum, and the client rectangle is read back. Writes minwidth.txt.
param([string] $Exe = 'C:\SynoSoftware\TinyTorrent2\3rdParty\goz\target\release\gozler-ui.exe')
. (Join-Path $PSScriptRoot 'common.ps1')
$root = $PSScriptRoot

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Size {
    public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr h);
    /// Sizes `h` to 100 by 100 without moving or activating it, and returns
    /// its client width in DIP.
    public static double Least(IntPtr h) {
        // SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE
        SetWindowPos(h, IntPtr.Zero, 0, 0, 100, 100, 0x0002 | 0x0004 | 0x0010);
        RECT r;
        GetClientRect(h, out r);
        return (r.R - r.L) * 96.0 / GetDpiForWindow(h);
    }
}
'@

$lines = [Collections.Generic.List[string]]::new()
$placement = @{ left = 60; top = 60; right = 1180; bottom = 700; maximized = $false }
foreach ($language in 'en', 'es', 'nl', 'tr') {
    $process, $main = Start-Window $Exe (Join-Path $root "data-minwidth-$language") @{
        settings = @{ language = $language; keep_running = $false; check_for_updates = $false }
        window   = @{
            mode = 'two_panels'; placement = $placement
            panels = @{ left = @{ folder = 'C:\SynoSoftware\goz-file-manager\crates' }; right = @{ folder = 'C:\SynoSoftware\goz-file-manager\docs' }; active = 'left' }
        }
    }
    try {
        Wait-For { (Text $main $Panel.Left.Counts) -ne '' -and (Text $main $Panel.Right.Counts) -ne '' }
        $lines.Add(('{0}: {1} DIP' -f $language, [Size]::Least($main)))
    }
    finally { Stop-Window $process $main }
}
$lines
$lines | Set-Content (Join-Path $root 'minwidth.txt') -Encoding utf8NoBOM
