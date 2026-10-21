# Screenshots of stage 6's Viewer, from the running window, without taking
# the foreground: the window starts minimized (--sign-in), is shown without
# activation, is driven with posted messages, and each Viewer is captured
# with PrintWindow. The run uses its own --data-dir under this folder, never
# the owner's ui.json. Each wait reads the state it waits for, never a fixed
# time.
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
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] static extern bool RedrawWindow(IntPtr h, IntPtr r, IntPtr g, uint flags);
    [DllImport("user32.dll")] static extern bool UpdateWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr h, EnumProc f, IntPtr l);

    /// The visible top-level windows of process `pid` whose class is
    /// `name`, or starts with it when `prefix`.
    public static List<IntPtr> Windows(uint pid, string name, bool prefix) {
        var found = new List<IntPtr>();
        EnumWindows((h, l) => {
            uint owner;
            GetWindowThreadProcessId(h, out owner);
            var cls = new StringBuilder(256);
            GetClassName(h, cls, 256);
            if (owner == pid && IsWindowVisible(h) && (prefix ? cls.ToString().StartsWith(name) : cls.ToString() == name)) found.Add(h);
            return true;
        }, IntPtr.Zero);
        return found;
    }

    /// Has `window` and each of its children paint what they hold now,
    /// because PrintWindow copies what they last painted. UpdateWindow
    /// sends WM_PAINT, so it returns when the window has painted.
    static void Paint(IntPtr window) {
        RedrawWindow(window, IntPtr.Zero, IntPtr.Zero, 0x1 | 0x80);
        UpdateWindow(window);
        EnumChildWindows(window, (h, l) => { UpdateWindow(h); return true; }, IntPtr.Zero);
    }

    static Bitmap Print(IntPtr w) {
        Paint(w);
        RECT o; GetWindowRect(w, out o);
        var bmp = new Bitmap(o.R - o.L, o.B - o.T);
        using (var g = Graphics.FromImage(bmp)) {
            IntPtr dc = g.GetHdc();
            PrintWindow(w, dc, 2);
            g.ReleaseHdc(dc);
        }
        return bmp;
    }

    public static void Save(string path, IntPtr window) {
        using (var bmp = Print(window)) bmp.Save(path, ImageFormat.Png);
    }

    /// The dark pixels that `child` of `window` shows: text drawn on the
    /// window background.
    public static int Ink(IntPtr window, IntPtr child) {
        RECT w, c;
        GetWindowRect(window, out w);
        GetWindowRect(child, out c);
        int ink = 0;
        using (var bmp = Print(window)) {
            for (int y = c.T - w.T; y < c.B - w.T; y += 2)
                for (int x = c.L - w.L; x < c.R - w.L; x += 2) {
                    var p = bmp.GetPixel(x, y);
                    if (p.R + p.G + p.B < 300) ink++;
                }
        }
        return ink;
    }
}
'@

$WM_KEYDOWN = 0x100; $WM_CHAR = 0x102; $WM_COMMAND = 0x111; $WM_CLOSE = 0x10
$IDC_LEFT_PANEL = 79
$IDC_VIEWER_MESSAGE = 3501; $IDC_VIEWER_AREA = 3503
# MenuItem::Run(command).id(): 0x100 and the command's place in CommandId::ALL.
$VIEW = 0x122
$VIEWER_CLASS = 'gozler.ui.viewer'

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

function Viewers($process) { [Shot]::Windows($process.Id, $VIEWER_CLASS, $false) }

# Focuses `$name` in the left panel, opens it in the Viewer, waits until
# `$ready` answers true for the Viewer, saves it as `$file` and closes it.
function Shoot($name, $file, [scriptblock]$ready) {
    Chars $left $name
    Key $left 0x1B
    Command $main $VIEW
    Wait-For { (Viewers $process).Count -eq 1 }
    $script:viewer = (Viewers $process)[0]
    Wait-For { & $ready $script:viewer }
    [Shot]::Save((Join-Path $root $file), $script:viewer)
    [void][Shot]::PostMessage($script:viewer, $WM_CLOSE, [IntPtr]0, [IntPtr]0)
    Wait-For { (Viewers $process).Count -eq 0 }
}

$fixture = Join-Path $root 'fixture'
Remove-Item -Recurse -Force $fixture -ErrorAction SilentlyContinue
New-Item -ItemType Directory $fixture | Out-Null
$utf8 = [Text.UTF8Encoding]::new($false)
$text = @'
; Settings for the backup task.
; Each section names a folder to copy and where the copy goes.

[Documents]
source = C:\Users\Public\Documents
target = D:\Backup\Documents
schedule = daily 02:00
keep = 30

[Photos]
source = C:\Users\Public\Pictures
target = D:\Backup\Photos
schedule = weekly Sunday 03:00
keep = 12
exclude = *.tmp; *.thumb; Thumbs.db; desktop.ini; a very long list of patterns that the task skips, so this line is wider than the window and wraps onto the next row

[Log]
file = D:\Backup\backup.log
level = info
'@
[IO.File]::WriteAllText((Join-Path $fixture 'backup.ini'), $text.Replace("`r`n", "`n").Replace("`n", "`r`n"), $utf8)
$unicode = @'
Kişiler — Contacts — Contactos — Contacten
Ad: Ayşe Yılmaz
Şehir: İstanbul
Nombre: José Núñez, Señorita Peña
Plaats: Den Haag, Zuid-Holland
Πόλη: Αθήνα
Город: Москва
Notes: café, naïve, façade, smörgåsbord, Ærøskøbing
'@
[IO.File]::WriteAllText((Join-Path $fixture 'contacts.txt'), $unicode.Replace("`r`n", "`n").Replace("`n", "`r`n"), [Text.UnicodeEncoding]::new($false, $true))
Copy-Item 'C:\Windows\System32\whoami.exe' (Join-Path $fixture 'whoami.exe')
[IO.File]::WriteAllText((Join-Path $fixture 'locked.log'), "in use`r`n", $utf8)

$data = Join-Path $root 'data'
Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
New-Item -ItemType Directory $data | Out-Null
@{
    settings = @{ keep_running = $false; check_for_updates = $false }
    window   = @{
        mode = 'two_panels'; placement = @{ left = 40; top = 40; right = 1100; bottom = 680; maximized = $false }
        panels = @{ left = @{ folder = $fixture }; right = @{ folder = $fixture }; active = 'left' }
        viewer = @{ placement = @{ left = 120; top = 90; right = 900; bottom = 560; maximized = $false }; wrap = $true }
    }
} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $data 'ui.json') -Encoding utf8NoBOM

# Another program holds locked.log open and shares nothing, so the Viewer
# cannot open it.
$lock = [IO.File]::Open((Join-Path $fixture 'locked.log'), 'Open', 'Read', 'None')
$process = Start-Process $Exe -ArgumentList '--data-dir', "`"$data`"", '--sign-in' -PassThru
try {
    $script:main = [IntPtr]::Zero
    # The main window's class is gozler.ui and a suffix for its data folder.
    Wait-For { $w = [Shot]::Windows($process.Id, 'gozler.ui.', $true); if ($w.Count -gt 0) { $script:main = $w[0] }; $w.Count -gt 0 }
    $main = $script:main
    # SW_SHOWNOACTIVATE: restored, and the program in front stays in front.
    [void][Shot]::ShowWindow($main, 4)
    $left = [Shot]::GetDlgItem($main, $IDC_LEFT_PANEL)
    Wait-For { [Shot]::SendMessage($left, 0x1004, [IntPtr]0, [IntPtr]0) -eq 5 }

    # A page shows once the area holds text.
    $paged = { param($v) [Shot]::Ink($v, [Shot]::GetDlgItem($v, $IDC_VIEWER_AREA)) -gt 200 }
    Shoot 'backup' '01-text.png' $paged
    Shoot 'contacts' '02-utf16.png' $paged
    Shoot 'whoami' '03-executable-hex.png' $paged
    Shoot 'locked' '04-cannot-open.png' { param($v) [Shot]::IsWindowVisible([Shot]::GetDlgItem($v, $IDC_VIEWER_MESSAGE)) }
}
finally {
    $lock.Dispose()
    if ($main) { [void][Shot]::PostMessage($main, $WM_CLOSE, [IntPtr]0, [IntPtr]0) }
    if (-not $process.WaitForExit(10000)) { $process.Kill() }
}
Remove-Item -Recurse -Force $fixture, $data
