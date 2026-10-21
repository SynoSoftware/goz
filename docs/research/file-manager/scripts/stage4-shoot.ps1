# Screenshots of stage 4's new screens, from the running window, without
# taking the foreground: the window starts minimized (--sign-in), is shown
# without activation, is driven with posted messages, and is captured with
# PrintWindow. Each run uses its own --data-dir under this folder, never the
# owner's ui.json or operations.json.
param([string]$Exe = 'C:\SynoSoftware\TinyTorrent2\3rdParty\goz\target\debug\gozler-ui.exe')
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot

Add-Type -ReferencedAssemblies System.Drawing.Common, System.Drawing.Primitives, System.Collections, System.Runtime, System.Runtime.InteropServices, System.Linq, System.Private.Windows.GdiPlus, System.Private.Windows.Core -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

public static class Shot {
    public struct RECT { public int L, T, R, B; }
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, string l);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);

    /// The visible top-level windows of process `pid` whose class starts
    /// with `prefix`.
    public static List<IntPtr> Windows(uint pid, string prefix) {
        var found = new List<IntPtr>();
        EnumWindows((h, l) => {
            uint owner;
            GetWindowThreadProcessId(h, out owner);
            var name = new StringBuilder(256);
            GetClassName(h, name, 256);
            if (owner == pid && IsWindowVisible(h) && name.ToString().StartsWith(prefix)) found.Add(h);
            return true;
        }, IntPtr.Zero);
        return found;
    }

    /// Saves `main` as `path`, with each window of `printed` drawn over it
    /// where it stands, and each of `screened` copied from the screen,
    /// because a menu draws only there.
    public static void Save(string path, IntPtr main, IntPtr[] printed, IntPtr[] screened) {
        // The union of the windows, because a message box whose owner is
        // not in front stands in the middle of the screen.
        RECT r;
        GetWindowRect(main, out r);
        foreach (var w in printed) {
            RECT o; GetWindowRect(w, out o);
            r.L = Math.Min(r.L, o.L); r.T = Math.Min(r.T, o.T);
            r.R = Math.Max(r.R, o.R); r.B = Math.Max(r.B, o.B);
        }
        RECT m;
        GetWindowRect(main, out m);
        using (var bmp = new Bitmap(r.R - r.L, r.B - r.T))
        using (var g = Graphics.FromImage(bmp)) {
            g.Clear(Color.FromArgb(0xd6, 0xdd, 0xe5));
            Print(g, main, m.L - r.L, m.T - r.T);
            foreach (var w in printed) {
                RECT o; GetWindowRect(w, out o);
                Print(g, w, o.L - r.L, o.T - r.T);
            }
            foreach (var w in screened) {
                RECT o; GetWindowRect(w, out o);
                g.CopyFromScreen(o.L, o.T, o.L - r.L, o.T - r.T, new Size(o.R - o.L, o.B - o.T));
            }
            bmp.Save(path, ImageFormat.Png);
        }
    }

    static void Print(Graphics g, IntPtr w, int x, int y) {
        RECT o; GetWindowRect(w, out o);
        using (var part = new Bitmap(o.R - o.L, o.B - o.T))
        using (var pg = Graphics.FromImage(part)) {
            IntPtr dc = pg.GetHdc();
            PrintWindow(w, dc, 2);
            pg.ReleaseHdc(dc);
            g.DrawImage(part, x, y);
        }
    }
}
'@

$WM_KEYDOWN = 0x100; $WM_CHAR = 0x102; $WM_COMMAND = 0x111; $WM_CLOSE = 0x10; $WM_SETTEXT = 0xC
$IDC_LEFT_PANEL = 79; $IDC_LEFT_PATH = 82
# MenuItem::Panel(PanelAction::Drives(Side::Left)).id()
$DRIVES_LEFT = 0x801

function Key($hwnd, $vk) { [void][Shot]::PostMessage($hwnd, $WM_KEYDOWN, [IntPtr]$vk, [IntPtr]1) }
function Chars($hwnd, $text) { foreach ($c in $text.ToCharArray()) { [void][Shot]::PostMessage($hwnd, $WM_CHAR, [IntPtr][int]$c, [IntPtr]1) } }

# Waits until `test` answers true, up to `seconds`, and fails otherwise.
function Wait-For([scriptblock]$test, [int]$seconds = 10) {
    $until = (Get-Date).AddSeconds($seconds)
    while (-not (& $test)) {
        if ((Get-Date) -gt $until) { throw "timed out: $test" }
        Start-Sleep -Milliseconds 100
    }
}

# Starts the window on `$data` with `$settings` as its ui.json, shown
# without activation; returns the process and its main window.
function Start-Window($name, $settings) {
    $data = Join-Path $root $name
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
    New-Item -ItemType Directory $data | Out-Null
    $settings | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $data 'ui.json') -Encoding utf8NoBOM
    $process = Start-Process $Exe -ArgumentList '--data-dir', "`"$data`"", '--sign-in' -PassThru
    $script:main = [IntPtr]::Zero
    Wait-For { $w = [Shot]::Windows($process.Id, 'gozler.ui'); if ($w.Count -gt 0) { $script:main = $w[0] }; $w.Count -gt 0 }
    # SW_SHOWNOACTIVATE: restored, and the program in front stays in front.
    [void][Shot]::ShowWindow($script:main, 4)
    return @($process, $script:main)
}

function Stop-Window($process, $main) {
    [void][Shot]::PostMessage($main, $WM_CLOSE, [IntPtr]0, [IntPtr]0)
    if (-not $process.WaitForExit(10000)) { $process.Kill() }
}

# The left panel's folder: folders, files of a few sizes, and a hidden file.
$fixture = Join-Path $root 'fixture'
Remove-Item -Recurse -Force $fixture -ErrorAction SilentlyContinue
New-Item -ItemType Directory $fixture | Out-Null
foreach ($folder in 'Invoices', 'Projects', 'Scans') { New-Item -ItemType Directory (Join-Path $fixture $folder) | Out-Null }
$files = [ordered]@{
    'Invoices\2026-09.pdf' = 1200000; 'Projects\plan.txt' = 4400000; 'Scans\scan1.png' = 220000;
    'budget 2026.xlsx' = 84000; 'desktop.ini' = 300; 'notes.txt' = 4000; 'report.pdf' = 812000;
    'report-draft.pdf' = 790000; 'tax return 2025.pdf' = 2311000
}
foreach ($file in $files.Keys) { [IO.File]::WriteAllBytes((Join-Path $fixture $file), [byte[]]::new($files[$file])) }
(Get-Item (Join-Path $fixture 'desktop.ini')).Attributes = 'Hidden'

$placement = @{ left = 60; top = 60; right = 1120; bottom = 700; maximized = $false }
$choices = @{ keep_running = $false; check_for_updates = $false }

# 1. Two panels with sizes, a selection and the focus bar; 2. quick search;
# 3. the drive menu.
$process, $main = Start-Window 'data' @{
    settings = $choices
    window   = @{
        mode = 'two_panels'; placement = $placement
        panels = @{ left = @{ folder = $fixture }; right = @{ folder = 'C:\SynoSoftware\goz-file-manager\crates' }; active = 'left' }
    }
}
$left = [Shot]::GetDlgItem($main, $IDC_LEFT_PANEL)
# Folder sizes come from gozlerd a moment after the rows.
Start-Sleep -Seconds 3
# `..`, Invoices, Projects, Scans, budget, desktop.ini, notes, report, ...
foreach ($vk in 0x28, 0x28, 0x28, 0x28, 0x2D, 0x28, 0x28, 0x2D, 0x24, 0x28, 0x28) { Key $left $vk }
Start-Sleep -Milliseconds 500
[Shot]::Save((Join-Path $root '01-two-panels-with-sizes.png'), $main, @(), @())
Chars $left 'rep'
Start-Sleep -Milliseconds 500
[Shot]::Save((Join-Path $root '02-quick-search.png'), $main, @(), @())
Key $left 0x1B
# A space in the quick search box is a character, not a selection: "tax "
# then "r" finds "tax return 2025.pdf", and the selection stays as it was.
$LVM_GETSELECTEDCOUNT = 0x1032
$before = [Shot]::SendMessage($left, $LVM_GETSELECTEDCOUNT, [IntPtr]0, [IntPtr]0)
Chars $left 'tax'
Key $left 0x20
Chars $left 'r'
Start-Sleep -Milliseconds 500
$after = [Shot]::SendMessage($left, $LVM_GETSELECTEDCOUNT, [IntPtr]0, [IntPtr]0)
"space in quick search: selected $before before, $after after"
[Shot]::Save((Join-Path $root '02b-quick-search-space.png'), $main, @(), @())
Key $left 0x1B
[void][Shot]::PostMessage($main, $WM_COMMAND, [IntPtr]$DRIVES_LEFT, [IntPtr]0)
Wait-For { [Shot]::Windows($process.Id, '#32768').Count -gt 0 }
Start-Sleep -Milliseconds 300
$menu = [Shot]::Windows($process.Id, '#32768')
foreach ($m in $menu) { $mr = New-Object Shot+RECT; [void][Shot]::GetWindowRect($m, [ref]$mr); "menu $($mr.L),$($mr.T),$($mr.R),$($mr.B)" }
[Shot]::Save((Join-Path $root '03-drive-menu.png'), $main, $menu.ToArray(), @())
Key $menu[0] 0x1B
Wait-For { [Shot]::Windows($process.Id, '#32768').Count -eq 0 }
Stop-Window $process $main

# 4. A drive that is not ready; 5. and 6. paths that cannot be opened.
$used = (Get-PSDrive -PSProvider FileSystem).Name
$missing = [char[]](70..90) | Where-Object { $used -notcontains [string]$_ } | Select-Object -First 1
$process, $main = Start-Window 'data-failures' @{
    settings = $choices
    window   = @{
        mode = 'two_panels'; placement = $placement
        panels = @{ left = @{ folder = $fixture }; right = @{ folder = "${missing}:\DCIM" }; active = 'left' }
    }
}
Start-Sleep -Seconds 2
[Shot]::Save((Join-Path $root '04-drive-not-ready.png'), $main, @(), @())
$path = [Shot]::GetDlgItem($main, $IDC_LEFT_PATH)
$n = 5
foreach ($typed in "$fixture\Old", 'C:\System Volume Information') {
    [void][Shot]::SendMessage($path, $WM_SETTEXT, [IntPtr]0, $typed)
    Key $path 0x0D
    Wait-For { [Shot]::Windows($process.Id, '#32770').Count -gt 0 }
    Start-Sleep -Milliseconds 300
    $box = [Shot]::Windows($process.Id, '#32770')
    foreach ($m in $box) { $mr = New-Object Shot+RECT; [void][Shot]::GetWindowRect($m, [ref]$mr); "box $($mr.L),$($mr.T),$($mr.R),$($mr.B)" }
    [Shot]::Save((Join-Path $root ('{0:D2}-cannot-open.png' -f $n)), $main, $box.ToArray(), @())
    [void][Shot]::PostMessage($box[0], $WM_CLOSE, [IntPtr]0, [IntPtr]0)
    Wait-For { [Shot]::Windows($process.Id, '#32770').Count -eq 0 }
    $n++
}
Start-Sleep -Milliseconds 300
[Shot]::Save((Join-Path $root '07-typed-path-kept.png'), $main, @(), @())
Stop-Window $process $main

# 8. First start: no saved folders, so the panels show Documents and
# Downloads.
$process, $main = Start-Window 'data-first' @{
    settings = $choices
    window   = @{ mode = 'two_panels'; placement = $placement }
}
Start-Sleep -Seconds 2
[Shot]::Save((Join-Path $root '08-first-start.png'), $main, @(), @())
Stop-Window $process $main

# 9. An empty folder in the active right panel.
$empty = Join-Path $root 'empty'
Remove-Item -Recurse -Force $empty -ErrorAction SilentlyContinue
New-Item -ItemType Directory $empty | Out-Null
$process, $main = Start-Window 'data-empty' @{
    settings = $choices
    window   = @{
        mode = 'two_panels'; placement = $placement
        panels = @{ left = @{ folder = $fixture }; right = @{ folder = $empty }; active = 'right' }
    }
}
Start-Sleep -Seconds 2
[Shot]::Save((Join-Path $root '09-empty-folder.png'), $main, @(), @())
Stop-Window $process $main
