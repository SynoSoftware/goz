# What stage 7's screenshot and memory scripts share: starting the window on
# its own --data-dir without taking the foreground, driving it with posted
# and sent messages, reading its controls' texts and an owner-data list's
# cells across the process, and saving it with PrintWindow. Every wait reads
# the state it waits for; the give-up time only reports a failure.
$ErrorActionPreference = 'Stop'

Add-Type -ReferencedAssemblies System.Drawing.Common, System.Drawing.Primitives, System.Collections, System.Runtime, System.Runtime.InteropServices, System.Linq, System.Private.Windows.GdiPlus, System.Private.Windows.Core -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

public static class Win {
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
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, string l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, StringBuilder l);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern IntPtr VirtualAllocEx(IntPtr p, IntPtr a, UIntPtr size, uint type, uint protect);
    [DllImport("kernel32.dll")] static extern bool VirtualFreeEx(IntPtr p, IntPtr a, UIntPtr size, uint type);
    [DllImport("kernel32.dll")] static extern bool WriteProcessMemory(IntPtr p, IntPtr a, byte[] b, UIntPtr n, out UIntPtr done);
    [DllImport("kernel32.dll")] static extern bool ReadProcessMemory(IntPtr p, IntPtr a, byte[] b, UIntPtr n, out UIntPtr done);

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

    public static string Text(IntPtr h) {
        var text = new StringBuilder(4096);
        SendMessage(h, 0x000D, (IntPtr)text.Capacity, text);
        return text.ToString();
    }

    /// The text of `row`'s first cell in the list `list` of process `pid`,
    /// which an owner-data list asks its window for, so it is empty while
    /// the row's chunk has not arrived.
    public static string Cell(uint pid, IntPtr list, int row) {
        // PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE
        IntPtr process = OpenProcess(0x0008 | 0x0010 | 0x0020, false, pid);
        if (process == IntPtr.Zero) throw new Exception("OpenProcess failed");
        const int textAt = 128, chars = 260;
        var size = (UIntPtr)(textAt + chars * 2);
        // MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE
        IntPtr remote = VirtualAllocEx(process, IntPtr.Zero, size, 0x3000, 0x04);
        try {
            // LVITEMW on 64 bits: iSubItem at 8, pszText at 24, cchTextMax at 32.
            var item = new byte[textAt];
            BitConverter.GetBytes((long)remote + textAt).CopyTo(item, 24);
            BitConverter.GetBytes(chars).CopyTo(item, 32);
            UIntPtr done;
            WriteProcessMemory(process, remote, item, (UIntPtr)item.Length, out done);
            // LVM_GETITEMTEXTW
            int length = (int)SendMessage(list, 0x1073, (IntPtr)row, remote);
            var text = new byte[chars * 2];
            ReadProcessMemory(process, remote + textAt, text, (UIntPtr)text.Length, out done);
            return Encoding.Unicode.GetString(text, 0, Math.Max(0, Math.Min(length, chars)) * 2);
        } finally {
            VirtualFreeEx(process, remote, UIntPtr.Zero, 0x8000);
            CloseHandle(process);
        }
    }

    /// Saves `main` as `path` with PrintWindow, which draws it also when
    /// another window covers it.
    public static void Save(string path, IntPtr main) {
        RECT r;
        GetWindowRect(main, out r);
        using (var bmp = new Bitmap(r.R - r.L, r.B - r.T))
        using (var g = Graphics.FromImage(bmp)) {
            IntPtr dc = g.GetHdc();
            PrintWindow(main, dc, 2);
            g.ReleaseHdc(dc);
            bmp.Save(path, ImageFormat.Png);
        }
    }
}
'@

$WM_KEYDOWN = 0x100; $WM_COMMAND = 0x111; $WM_CLOSE = 0x10; $WM_SETTEXT = 0xC
$CB_SETCURSEL = 0x14E; $CBN_SELCHANGE = 1; $LVM_GETITEMCOUNT = 0x1004; $LVM_ENSUREVISIBLE = 0x1013
# MenuItem::Run(CommandId::Find).id() and MenuItem::Run(CommandId::TwoPanels).id():
# 0x100 plus the command's place in CommandId::ALL.
$FIND = 0x113; $TWO_PANELS = 0x120
# resources/ids.h
$IDC_LIST = 15
$Panel = @{
    Left  = @{ List = 79; Message = 83; Counts = 85; Free = 87; Search = 101; LookIn = 103; Folder = 104 }
    Right = @{ List = 80; Message = 90; Counts = 92; Free = 94; Search = 105; LookIn = 107; Folder = 108 }
}

function Key($hwnd, $vk) { [void][Win]::PostMessage($hwnd, $WM_KEYDOWN, [IntPtr]$vk, [IntPtr]1) }
function Item($main, [int] $id) { [Win]::GetDlgItem($main, $id) }
function Text($main, [int] $id) { [Win]::Text((Item $main $id)) }
function Count($hwnd) { [int][Win]::SendMessage($hwnd, $LVM_GETITEMCOUNT, [IntPtr]0, [IntPtr]0) }

# Waits until `test` answers true, and fails after `seconds`.
function Wait-For([scriptblock] $test, [int] $seconds = 30) {
    $until = (Get-Date).AddSeconds($seconds)
    while (-not (& $test)) {
        if ((Get-Date) -gt $until) { throw "timed out: $test" }
        Start-Sleep -Milliseconds 100
    }
}

# Starts `$exe` on the new folder `$data` with `$settings` as its ui.json,
# shown without activation; returns the process and its main window.
function Start-Window([string] $exe, [string] $data, $settings) {
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
    New-Item -ItemType Directory $data | Out-Null
    $settings | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $data 'ui.json') -Encoding utf8NoBOM
    $process = Start-Process $exe -ArgumentList '--data-dir', "`"$data`"", '--sign-in' -PassThru
    $script:main = [IntPtr]::Zero
    Wait-For { $w = [Win]::Windows($process.Id, 'gozler.ui.'); if ($w.Count -gt 0) { $script:main = $w[0] }; $w.Count -gt 0 }
    # SW_SHOWNOACTIVATE: restored, and the program in front stays in front.
    [void][Win]::ShowWindow($script:main, 4)
    return @($process, $script:main)
}

function Stop-Window($process, $main) {
    [void][Win]::PostMessage($main, $WM_CLOSE, [IntPtr]0, [IntPtr]0)
    if (-not $process.WaitForExit(10000)) { $process.Kill() }
}

# Find in Two panels on the active panel, which becomes a search panel.
function Find($main) { [void][Win]::PostMessage($main, $WM_COMMAND, [IntPtr]$FIND, [IntPtr]0) }

# Types `$text` into `$side`'s search box, as one change.
function Search-For($main, $side, [string] $text) {
    [void][Win]::SendMessage((Item $main $Panel[$side].Search), $WM_SETTEXT, [IntPtr]0, $text)
}

# Chooses Look in's item `$index` of `$side`, as the person does.
function Look-In($main, $side, [int] $index) {
    $id = $Panel[$side].LookIn
    $combo = Item $main $id
    [void][Win]::SendMessage($combo, $CB_SETCURSEL, [IntPtr]$index, [IntPtr]0)
    $wparam = [IntPtr]($id -bor ($CBN_SELCHANGE -shl 16))
    [void][Win]::SendMessage($main, $WM_COMMAND, $wparam, $combo)
}

# Whether `$side` shows its search row.
function Is-Searching($main, $side) { [Win]::IsWindowVisible((Item $main $Panel[$side].Search)) }
