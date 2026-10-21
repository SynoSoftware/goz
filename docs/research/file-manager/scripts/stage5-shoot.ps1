# Screenshots of stage 5's new screens, from the running window, without
# taking the foreground: the window starts minimized (--sign-in), is shown
# without activation, is driven with posted messages, and is captured with
# PrintWindow. Each run uses its own --data-dir under this folder, never the
# owner's ui.json or operations.json. Each wait reads the state it waits
# for, never a fixed time.
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
    [DllImport("user32.dll")] static extern bool RedrawWindow(IntPtr h, IntPtr r, IntPtr g, uint flags);
    [DllImport("user32.dll")] static extern bool UpdateWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr h, EnumProc f, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);

    /// Has `window` and each of its children paint what they hold now,
    /// because PrintWindow copies what they last painted. UpdateWindow
    /// sends WM_PAINT, so it returns when the window has painted.
    static void Paint(IntPtr window) {
        RedrawWindow(window, IntPtr.Zero, IntPtr.Zero, 0x1 | 0x80);
        UpdateWindow(window);
        EnumChildWindows(window, (h, l) => { UpdateWindow(h); return true; }, IntPtr.Zero);
    }

    public static void Save(string path, IntPtr main, IntPtr[] printed, IntPtr[] screened) {
        Paint(main);
        foreach (var w in printed) Paint(w);
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

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes

$WM_KEYDOWN = 0x100; $WM_CHAR = 0x102; $WM_COMMAND = 0x111; $WM_CLOSE = 0x10; $WM_SETTEXT = 0xC
$IDOK = 1; $IDCANCEL = 2
$IDC_STATUS = 16; $IDC_LEFT_PANEL = 79
$IDC_NEW_FOLDER_NAME = 3402
# MenuItem::Run(command).id(): 0x100 and the command's place in CommandId::ALL.
$COPY_TO_FOLDER = 0x11A; $MOVE_TO_FOLDER = 0x11B; $NEW_FOLDER = 0x124

function Key($hwnd, $vk) { [void][Shot]::PostMessage($hwnd, $WM_KEYDOWN, [IntPtr]$vk, [IntPtr]1) }
function Chars($hwnd, $text) { foreach ($c in $text.ToCharArray()) { [void][Shot]::PostMessage($hwnd, $WM_CHAR, [IntPtr][int]$c, [IntPtr]1) } }
function Command($hwnd, $id) { [void][Shot]::PostMessage($hwnd, $WM_COMMAND, [IntPtr]$id, [IntPtr]0) }

# Waits until `test` answers true, up to `seconds`, and fails otherwise.
function Wait-For([scriptblock]$test, [int]$seconds = 20) {
    $until = (Get-Date).AddSeconds($seconds)
    while (-not (& $test)) {
        if ((Get-Date) -gt $until) { throw "timed out: $test" }
        Start-Sleep -Milliseconds 50
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

# The dialogs of the process: the window's own and the message boxes.
function Dialogs($process) { [Shot]::Windows($process.Id, '#32770') }

# The status bar's texts, as UI Automation reads its parts.
function Status-Texts($main) {
    $bar = [System.Windows.Automation.AutomationElement]::FromHandle([Shot]::GetDlgItem($main, $IDC_STATUS))
    $parts = $bar.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
    @($parts | ForEach-Object { $_.Current.Name })
}

# The left panel shows the fixture, the right one an empty backup folder.
$fixture = Join-Path $root 'fixture'
$backup = Join-Path $root 'backup'
foreach ($folder in $fixture, $backup) {
    Remove-Item -Recurse -Force $folder -ErrorAction SilentlyContinue
    New-Item -ItemType Directory $folder | Out-Null
}
foreach ($folder in 'Invoices', 'Projects', 'Scans') { New-Item -ItemType Directory (Join-Path $fixture $folder) | Out-Null }
$files = [ordered]@{ 'budget 2026.xlsx' = 84000; 'notes.txt' = 4000; 'report.pdf' = 812000; 'tax return 2025.pdf' = 2311000 }
foreach ($file in $files.Keys) { [IO.File]::WriteAllBytes((Join-Path $fixture $file), [byte[]]::new($files[$file])) }
# Enough files that the copy runs long enough to show its progress.
$block = [byte[]]::new(256KB)
foreach ($n in 1..3000) { [IO.File]::WriteAllBytes((Join-Path $fixture "Projects\part$n.bin"), $block) }

$process, $main = Start-Window 'data' @{
    settings = @{ keep_running = $false; check_for_updates = $false }
    window   = @{
        mode = 'two_panels'; placement = @{ left = 60; top = 60; right = 1120; bottom = 700; maximized = $false }
        panels = @{ left = @{ folder = $fixture }; right = @{ folder = $backup }; active = 'left' }
    }
}
try {
    $left = [Shot]::GetDlgItem($main, $IDC_LEFT_PANEL)
    $LVM_GETITEMCOUNT = 0x1004
    Wait-For { [Shot]::SendMessage($left, $LVM_GETITEMCOUNT, [IntPtr]0, [IntPtr]0) -eq 8 }

    # 1. and 2. Copy to folder and Move to folder of one entry.
    Chars $left 'report.'
    Key $left 0x1B
    foreach ($shot in @(@($COPY_TO_FOLDER, '01-copy-to-folder.png'), @($MOVE_TO_FOLDER, '02-move-to-folder.png'))) {
        Command $main $shot[0]
        Wait-For { (Dialogs $process).Count -eq 1 }
        $dialog = (Dialogs $process)[0]
        [Shot]::Save((Join-Path $root $shot[1]), $main, @($dialog), @())
        Command $dialog $IDCANCEL
        Wait-For { (Dialogs $process).Count -eq 0 }
    }

    # 3. New folder, and 4. a name that a row shows.
    Command $main $NEW_FOLDER
    Wait-For { (Dialogs $process).Count -eq 1 }
    $dialog = (Dialogs $process)[0]
    $name = [Shot]::GetDlgItem($dialog, $IDC_NEW_FOLDER_NAME)
    [void][Shot]::SendMessage($name, $WM_SETTEXT, [IntPtr]0, 'Photos 2026')
    [Shot]::Save((Join-Path $root '03-new-folder.png'), $main, @($dialog), @())
    [void][Shot]::SendMessage($name, $WM_SETTEXT, [IntPtr]0, 'projects')
    Command $dialog $IDOK
    Wait-For { (Dialogs $process).Count -eq 2 }
    $message = (Dialogs $process) | Where-Object { $_ -ne $dialog } | Select-Object -First 1
    [Shot]::Save((Join-Path $root '04-new-folder-exists.png'), $main, [IntPtr[]]@($dialog, $message), @())
    [void][Shot]::PostMessage($message, $WM_CLOSE, [IntPtr]0, [IntPtr]0)
    Wait-For { (Dialogs $process).Count -eq 1 }
    Command $dialog $IDCANCEL
    Wait-For { (Dialogs $process).Count -eq 0 }

    # 5. A copy running in the status bar's middle group, and 6. its line when
    # it ended.
    Chars $left 'proj'
    Key $left 0x1B
    Command $main $COPY_TO_FOLDER
    Wait-For { (Dialogs $process).Count -eq 1 }
    Command (Dialogs $process)[0] $IDOK
    # UI Automation reads only the status bar's Cancel, so the progress bar's
    # position (PBM_GETPOS) and the copied files say how far the copy got.
    $progress = [Shot]::GetDlgItem([Shot]::GetDlgItem($main, $IDC_STATUS), 70)
    $copied = Join-Path $backup 'Projects'
    function Copied { @(Get-ChildItem $copied -File -ErrorAction SilentlyContinue).Count }
    Wait-For { [int][Shot]::SendMessage($progress, 0x408, [IntPtr]0, [IntPtr]0) -gt 0 }
    [Shot]::Save((Join-Path $root '05-copy-running.png'), $main, @(), @())
    Wait-For { (Status-Texts $main) -notcontains 'Cancel' -and (Copied) -eq 3000 } 120
    [Shot]::Save((Join-Path $root '06-copy-ended.png'), $main, @(), @())
}
finally {
    Stop-Window $process $main
}
Remove-Item -Recurse -Force $fixture, $backup
