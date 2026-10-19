#Requires -Version 7
<#
.SYNOPSIS
Drives a built gozler-ui window the way a person does, and prints what it
finds as JSON (docs/plans/ui-plan.md: The probe).

.DESCRIPTION
Each window it starts uses --data-dir in the probe's fixture folder, so the
owner's ui.json and ui-error.log are never read or written. It closes only
the windows it started and never uses the clipboard. It reads the window
through UI Automation and the Win32 menu functions, and runs menu items
with WM_COMMAND, none of which needs the keyboard focus. It opens a menu
through UI Automation, presses a key or drags the mouse only while its own
window is in front, because keys and the mouse act on the window in front;
otherwise those checks report "not run". Its exports go to the fixture
folder.

Every check reports "passed", "failed" or "not run" with what it saw.
#>
param(
    # The window to test. The gozler.exe beside it lists the fixture folder.
    [string] $Exe = (Join-Path $PSScriptRoot '..\..\target\release\gozler-ui.exe'),
    # Set when -Exe was built with check.ps1 -Release -Pseudo; the
    # pseudo-language checks run only then.
    [switch] $Pseudo,
    # A folder for the screenshots; none are taken without it.
    [string] $Screenshots,
    # How many files the fixture holds besides its named ones (plan: The
    # fixture folder).
    [int] $FixtureFiles = 40000
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$Exe = (Resolve-Path $Exe).Path
$gozler = Join-Path (Split-Path $Exe) 'gozler.exe'

Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading.Tasks;

namespace GozlerProbe {

[ComImport, Guid("30cbe57d-d9d0-452a-ab13-7ac5ac4825ee"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IUIAutomation {
    void CompareElements(); void CompareRuntimeIds(); void GetRootElement();
    IUIAutomationElement ElementFromHandle(IntPtr hwnd);
    void ElementFromPoint(); void GetFocusedElement(); void GetRootElementBuildCache();
    void ElementFromHandleBuildCache(); void ElementFromPointBuildCache();
    void GetFocusedElementBuildCache(); void CreateTreeWalker();
    IUIAutomationTreeWalker ControlViewWalker { get; }
    IUIAutomationTreeWalker ContentViewWalker { get; }
    IUIAutomationTreeWalker RawViewWalker { get; }
}

[ComImport, Guid("d22108aa-8ac5-49a5-837b-37bbb3d7591e"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IUIAutomationElement {
    void SetFocus(); void GetRuntimeId(); void FindFirst(); void FindAll();
    void FindFirstBuildCache(); void FindAllBuildCache(); void BuildUpdatedCache();
    [return: MarshalAs(UnmanagedType.Struct)] object GetCurrentPropertyValue(int propertyId);
    void GetCurrentPropertyValueEx(); void GetCachedPropertyValue();
    void GetCachedPropertyValueEx(); void GetCurrentPatternAs(); void GetCachedPatternAs();
    [return: MarshalAs(UnmanagedType.IUnknown)] object GetCurrentPattern(int patternId);
}

[ComImport, Guid("4042c624-389c-4afc-a630-9df854a541fc"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IUIAutomationTreeWalker {
    IUIAutomationElement GetParentElement(IUIAutomationElement e);
    IUIAutomationElement GetFirstChildElement(IUIAutomationElement e);
    IUIAutomationElement GetLastChildElement(IUIAutomationElement e);
    IUIAutomationElement GetNextSiblingElement(IUIAutomationElement e);
}

[ComImport, Guid("fb377fbe-8ea6-46d5-9c73-6499642d3059"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IUIAutomationInvokePattern { void Invoke(); }

[ComImport, Guid("619be086-1f4e-4ee4-bafa-210128738730"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IUIAutomationExpandCollapsePattern { void Expand(); void Collapse(); }

[ComImport, Guid("94cf8058-9b8d-4ab9-8bfd-4cd0a33c8c70"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IUIAutomationTogglePattern { void Toggle(); int CurrentToggleState { get; } }

[ComImport, Guid("a94cd8b1-0844-4cd6-9d2d-640537ab39e9"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IUIAutomationValuePattern { void SetValue([MarshalAs(UnmanagedType.BStr)] string value); [return: MarshalAs(UnmanagedType.BStr)] string get_CurrentValue(); }

/// One element as the probe reads it.
public class Element {
    internal IUIAutomationElement Raw;
    public string Name, AutomationId, ClassName, AccessKey, AcceleratorKey;
    public int ControlType;
    public bool Enabled;
    public long Hwnd;
    public double[] Rect;

    public override string ToString() { return ControlType + " '" + Name + "' id=" + AutomationId + " class=" + ClassName; }
}

public static class Ui {
    static IUIAutomation automation = (IUIAutomation)Activator.CreateInstance(
        Type.GetTypeFromCLSID(new Guid("ff48dba4-60ef-4201-aa87-54103eef594e")));

    public const int Button = 50000, CheckBox = 50002, ComboBox = 50003, Edit = 50004,
        Hyperlink = 50005, ListItem = 50007, List = 50008, Menu = 50009, MenuBar = 50010,
        MenuItem = 50011, StatusBar = 50017, Text = 50020, DataItem = 50029, Window = 50032,
        Header = 50034, HeaderItem = 50035, Separator = 50038;

    static Element Wrap(IUIAutomationElement raw) {
        if (raw == null) return null;
        Func<int, object> p = id => raw.GetCurrentPropertyValue(id);
        return new Element {
            Raw = raw,
            ControlType = (int)p(30003),
            Name = p(30005) as string,
            AcceleratorKey = p(30006) as string,
            AccessKey = p(30007) as string,
            Enabled = p(30010) is bool b && b,
            AutomationId = p(30011) as string,
            ClassName = p(30012) as string,
            Hwnd = Convert.ToInt64(p(30020)),
            Rect = p(30001) as double[],
        };
    }

    public static Element FromHandle(IntPtr hwnd) { return Wrap(automation.ElementFromHandle(hwnd)); }

    public static List<Element> Children(Element e) {
        // The raw view, because the control view leaves out menu separators.
        var walker = automation.RawViewWalker;
        var list = new List<Element>();
        for (var c = walker.GetFirstChildElement(e.Raw); c != null; c = walker.GetNextSiblingElement(c))
            list.Add(Wrap(c));
        return list;
    }

    /// The cells of the list's first `count` rows, one array a row. The walk
    /// asks for one row at a time, because a virtual list holds as many items
    /// as the results and reading them all takes minutes.
    public static List<string[]> Rows(Element list, int count) {
        var walker = automation.RawViewWalker;
        var rows = new List<string[]>();
        for (var c = walker.GetFirstChildElement(list.Raw); c != null && rows.Count < count; c = walker.GetNextSiblingElement(c)) {
            var row = Wrap(c);
            if (row.ControlType == ListItem || row.ControlType == DataItem) rows.Add(Cells(row));
        }
        return rows;
    }

    /// The list's header, which comes before its rows.
    public static Element HeaderOf(Element list) {
        var walker = automation.RawViewWalker;
        for (var c = walker.GetFirstChildElement(list.Raw); c != null; c = walker.GetNextSiblingElement(c)) {
            var child = Wrap(c);
            if (child.ControlType == Header) return child;
            if (child.ControlType == ListItem || child.ControlType == DataItem) return null;
        }
        return null;
    }

    /// The cells of the list's last row.
    public static string[] LastRow(Element list) {
        var row = Wrap(automation.RawViewWalker.GetLastChildElement(list.Raw));
        return row != null && (row.ControlType == ListItem || row.ControlType == DataItem) ? Cells(row) : null;
    }

    static string[] Cells(Element row) {
        var cells = new List<string>();
        foreach (var cell in Children(row)) cells.Add(cell.Name);
        return cells.Count > 0 ? cells.ToArray() : new[] { row.Name };
    }

    public static void Focus(Element e) { e.Raw.SetFocus(); }
    public static void SetValue(Element e, string value) { ((IUIAutomationValuePattern)e.Raw.GetCurrentPattern(10002)).SetValue(value); }
    public static void Toggle(Element e) { ((IUIAutomationTogglePattern)e.Raw.GetCurrentPattern(10015)).Toggle(); }
    public static void Invoke(Element e) { ((IUIAutomationInvokePattern)e.Raw.GetCurrentPattern(10000)).Invoke(); }

    public static void Expand(Element e) { ((IUIAutomationExpandCollapsePattern)e.Raw.GetCurrentPattern(10005)).Expand(); }
    public static void Collapse(Element e) { ((IUIAutomationExpandCollapsePattern)e.Raw.GetCurrentPattern(10005)).Collapse(); }
    public static string Value(Element e) { return ((IUIAutomationValuePattern)e.Raw.GetCurrentPattern(10002)).get_CurrentValue(); }
    public static int ToggleState(Element e) { return ((IUIAutomationTogglePattern)e.Raw.GetCurrentPattern(10015)).CurrentToggleState; }

    /// Invokes without waiting, because a menu item that opens a modal
    /// dialog may return only when the dialog closes.
    public static void InvokeLater(Element e) {
        var raw = e.Raw;
        Task.Run(() => ((IUIAutomationInvokePattern)raw.GetCurrentPattern(10000)).Invoke());
    }
}

public static class Win {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint flags);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int attribute, out RECT r, int size);

    /// The visible top-level windows of process `pid` with class `cls`.
    public static List<IntPtr> Windows(int pid, string cls) {
        var found = new List<IntPtr>();
        EnumWindows((h, l) => {
            uint owner;
            GetWindowThreadProcessId(h, out owner);
            if (owner == pid && IsWindowVisible(h) && ClassName(h) == cls) found.Add(h);
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static string ClassName(IntPtr h) { var s = new StringBuilder(256); GetClassNameW(h, s, 256); return s.ToString(); }
    public static string Text(IntPtr h) { var s = new StringBuilder(1024); GetWindowTextW(h, s, 1024); return s.ToString(); }

    public static RECT Rect(IntPtr h) { RECT r; GetWindowRect(h, out r); return r; }

    [StructLayout(LayoutKind.Sequential)]
    struct GUITHREADINFO { public uint cbSize, flags; public IntPtr hwndActive, hwndFocus, hwndCapture, hwndMenuOwner, hwndMoveSize, hwndCaret; public RECT rcCaret; }
    [DllImport("user32.dll")] static extern bool GetGUIThreadInfo(uint thread, ref GUITHREADINFO info);

    /// The control that has the keyboard focus in the thread of `window`,
    /// which Windows tracks whether or not the window is in front.
    public static IntPtr Focus(IntPtr window) {
        uint pid;
        var info = new GUITHREADINFO { cbSize = (uint)Marshal.SizeOf<GUITHREADINFO>() };
        GetGUIThreadInfo(GetWindowThreadProcessId(window, out pid), ref info);
        return info.hwndFocus;
    }

    /// The rectangle the window shows on screen, without its invisible
    /// resize borders.
    public static RECT Frame(IntPtr h) {
        RECT r;
        if (DwmGetWindowAttribute(h, 9, out r, Marshal.SizeOf<RECT>()) != 0) GetWindowRect(h, out r);
        return r;
    }


    [StructLayout(LayoutKind.Sequential)]
    struct INPUT { public uint type; public KEYBDINPUT ki; public long padding; }
    [StructLayout(LayoutKind.Sequential)]
    struct KEYBDINPUT { public ushort wVk, wScan; public uint dwFlags, time; public IntPtr dwExtraInfo; }
    [DllImport("user32.dll")] static extern uint SendInput(uint count, INPUT[] inputs, int size);

    /// Presses `keys` together and releases them, as a person does, but only
    /// while `window` is in front, because keys go to the window in front.
    public static bool PressKeys(IntPtr window, params ushort[] keys) {
        if (GetForegroundWindow() != window) return false;
        var inputs = new List<INPUT>();
        foreach (var k in keys) inputs.Add(new INPUT { type = 1, ki = new KEYBDINPUT { wVk = k } });
        for (int i = keys.Length - 1; i >= 0; i--) inputs.Add(new INPUT { type = 1, ki = new KEYBDINPUT { wVk = keys[i], dwFlags = 2 } });
        return SendInput((uint)inputs.Count, inputs.ToArray(), Marshal.SizeOf<INPUT>()) == inputs.Count;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, StringBuilder l);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] static extern IntPtr SendMessageTimeoutW(IntPtr h, uint m, IntPtr w, IntPtr l, uint flags, uint ms, out IntPtr result);

    /// How long `window` takes to answer WM_NULL, in milliseconds, which is
    /// how long its thread was busy; at most `limit`.
    public static long AnswerMs(IntPtr window, uint limit) {
        var clock = System.Diagnostics.Stopwatch.StartNew();
        IntPtr result;
        // WM_NULL, SMTO_NORMAL
        SendMessageTimeoutW(window, 0, IntPtr.Zero, IntPtr.Zero, 0, limit, out result);
        return clock.ElapsedMilliseconds;
    }
    [DllImport("user32.dll")] public static extern IntPtr GetMenu(IntPtr h);
    [DllImport("user32.dll")] static extern int GetMenuItemCount(IntPtr m);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool GetMenuItemInfoW(IntPtr m, uint item, bool byPosition, ref MENUITEMINFO info);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct MENUITEMINFO {
        public uint cbSize, fMask, fType, fState, wID;
        public IntPtr hSubMenu, hbmpChecked, hbmpUnchecked, dwItemData, dwTypeData;
        public uint cch;
        public IntPtr hbmpItem;
    }

    public class MenuItem {
        public string Text;
        public int Id;
        public bool Separator, Enabled, Checked, Default, RightJustify;
        public IntPtr SubMenu;
    }

    /// The items of `menu`, which the menu functions read across processes.
    public static List<MenuItem> MenuItems(IntPtr menu) {
        var items = new List<MenuItem>();
        int count = GetMenuItemCount(menu);
        for (uint i = 0; i < count; i++) {
            // MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_STRING | MIIM_FTYPE
            var info = new MENUITEMINFO { cbSize = (uint)Marshal.SizeOf<MENUITEMINFO>(), fMask = 0x1 | 0x2 | 0x4 | 0x40 | 0x100 };
            GetMenuItemInfoW(menu, i, true, ref info);
            string text = "";
            if (info.cch > 0) {
                info.cch++;
                info.dwTypeData = Marshal.AllocHGlobal((int)info.cch * 2);
                try {
                    GetMenuItemInfoW(menu, i, true, ref info);
                    text = Marshal.PtrToStringUni(info.dwTypeData);
                }
                finally { Marshal.FreeHGlobal(info.dwTypeData); }
            }
            items.Add(new MenuItem {
                Text = text,
                Id = (int)info.wID,
                Separator = (info.fType & 0x800) != 0,
                RightJustify = (info.fType & 0x4000) != 0,
                Enabled = (info.fState & 0x3) == 0,
                Checked = (info.fState & 0x8) != 0,
                Default = (info.fState & 0x1000) != 0,
                SubMenu = info.hSubMenu,
            });
        }
        return items;
    }

    /// A control's text, which Windows copies across processes for
    /// WM_GETTEXT; GetWindowText reads only a caption in another process.
    public static string ControlText(IntPtr control) {
        int length = (int)SendMessageW(control, 0x000E, IntPtr.Zero, IntPtr.Zero);
        var text = new StringBuilder(length + 1);
        SendMessageW(control, 0x000D, (IntPtr)(length + 1), text);
        return text.ToString();
    }

    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr parent, EnumProc f, IntPtr l);
    [DllImport("user32.dll")] static extern int GetDlgCtrlID(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "SendMessageW")] static extern IntPtr SendText(IntPtr h, uint m, IntPtr w, string l);

    /// The descendant of `parent` with class `cls` and control ID `id`.
    public static IntPtr ChildById(IntPtr parent, int id, string cls) {
        var found = IntPtr.Zero;
        EnumChildWindows(parent, (h, l) => {
            if (GetDlgCtrlID(h) != id || ClassName(h) != cls) return true;
            found = h;
            return false;
        }, IntPtr.Zero);
        return found;
    }

    /// Sets a control's text with WM_SETTEXT, which Windows copies across
    /// processes.
    public static void SetControlText(IntPtr control, string text) { SendText(control, 0x000C, IntPtr.Zero, text); }

    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern IntPtr VirtualAllocEx(IntPtr p, IntPtr address, UIntPtr size, uint type, uint protect);
    [DllImport("kernel32.dll")] static extern bool VirtualFreeEx(IntPtr p, IntPtr address, UIntPtr size, uint type);
    [DllImport("kernel32.dll")] static extern bool WriteProcessMemory(IntPtr p, IntPtr address, byte[] bytes, UIntPtr size, out UIntPtr written);

    /// Sets the state bits `mask` of the list's row `row`, or of every row
    /// for -1, to `state`, as a click does. LVM_SETITEMSTATE reads an LVITEMW
    /// in the list's own process, so the item is written there.
    public static void SetItemState(IntPtr list, int row, uint state, uint mask) {
        uint pid;
        GetWindowThreadProcessId(list, out pid);
        // PROCESS_VM_OPERATION | PROCESS_VM_WRITE
        var process = OpenProcess(0x0008 | 0x0020, false, pid);
        if (process == IntPtr.Zero) throw new InvalidOperationException("OpenProcess failed");
        try {
            // Larger than an LVITEMW; state and stateMask follow mask, iItem
            // and iSubItem.
            var item = new byte[128];
            BitConverter.GetBytes(state).CopyTo(item, 12);
            BitConverter.GetBytes(mask).CopyTo(item, 16);
            // MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE
            var remote = VirtualAllocEx(process, IntPtr.Zero, (UIntPtr)item.Length, 0x3000, 0x04);
            if (remote == IntPtr.Zero) throw new InvalidOperationException("VirtualAllocEx failed");
            try {
                UIntPtr written;
                if (!WriteProcessMemory(process, remote, item, (UIntPtr)item.Length, out written))
                    throw new InvalidOperationException("WriteProcessMemory failed");
                SendMessageW(list, 0x102B, (IntPtr)row, remote);
            }
            // MEM_RELEASE
            finally { VirtualFreeEx(process, remote, UIntPtr.Zero, 0x8000); }
        }
        finally { CloseHandle(process); }
    }

    [StructLayout(LayoutKind.Sequential)] struct POINT { public int X, Y; }
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] static extern void mouse_event(uint flags, int dx, int dy, uint data, IntPtr extra);

    /// Drags with the left button from (x1, y1) to (x2, y2), in screen
    /// pixels, and puts the pointer back, but only while `window` is in
    /// front, because the mouse acts on the window in front.
    public static bool Drag(IntPtr window, int x1, int y1, int x2, int y2) {
        if (GetForegroundWindow() != window) return false;
        POINT old;
        GetCursorPos(out old);
        SetCursorPos(x1, y1);
        System.Threading.Thread.Sleep(50);
        // MOUSEEVENTF_LEFTDOWN, then MOUSEEVENTF_LEFTUP
        mouse_event(0x2, 0, 0, 0, IntPtr.Zero);
        for (int i = 1; i <= 10; i++) {
            SetCursorPos(x1 + (x2 - x1) * i / 10, y1 + (y2 - y1) * i / 10);
            System.Threading.Thread.Sleep(30);
        }
        mouse_event(0x4, 0, 0, 0, IntPtr.Zero);
        SetCursorPos(old.X, old.Y);
        return true;
    }

    /// A list box's items, which Windows copies across processes for
    /// LB_GETTEXT.
    public static List<string> ListItems(IntPtr list) {
        var items = new List<string>();
        int count = (int)SendMessageW(list, 0x018B, IntPtr.Zero, IntPtr.Zero);
        for (int i = 0; i < count; i++) {
            int length = (int)SendMessageW(list, 0x018A, (IntPtr)i, IntPtr.Zero);
            var text = new StringBuilder(length + 1);
            SendMessageW(list, 0x0189, (IntPtr)i, text);
            items.Add(text.ToString());
        }
        return items;
    }

    /// A combo box's items, which Windows copies across processes for
    /// CB_GETLBTEXT.
    public static List<string> ComboItems(IntPtr combo) {
        var items = new List<string>();
        int count = (int)SendMessageW(combo, 0x0146, IntPtr.Zero, IntPtr.Zero);
        for (int i = 0; i < count; i++) {
            int length = (int)SendMessageW(combo, 0x0149, (IntPtr)i, IntPtr.Zero);
            var text = new StringBuilder(length + 1);
            SendMessageW(combo, 0x0148, (IntPtr)i, text);
            items.Add(text.ToString());
        }
        return items;
    }
}
}
'@

# Per-monitor DPI aware, so every rectangle is in the window's own pixels.
[void][GozlerProbe.Win]::SetThreadDpiAwarenessContext([IntPtr](-4))

$results = [ordered]@{}
function Report([string] $name, [string] $status, $detail) {
    $results[$name] = [ordered]@{ status = $status; detail = $detail }
}
function Check([string] $name, [bool] $passed, $detail) {
    Report $name ($passed ? 'passed' : 'failed') $detail
}

# --- The captions, as the window shows them --------------------------------

$english = Get-Content -Raw (Join-Path $PSScriptRoot 'resources\en.json') | ConvertFrom-Json -AsHashtable
function Caption([string] $key) {
    $group, $name = $key.Split('.', 2)
    return $english[$group][$name]
}
# The text without its access-key marker, as UI Automation names it.
function Plain([string] $text) { return ($text -replace '&(.)', '$1') }
function AccessKeyOf([string] $text) {
    if ($text -match '&([^&])') { return $Matches[1].ToLowerInvariant() }
    return ''
}

# --- Starting and closing windows -------------------------------------------

$started = [System.Collections.Generic.List[System.Diagnostics.Process]]::new()
$WM_CLOSE = 0x10

function Write-Settings([string] $json) {
    $file = Join-Path $dataDir 'ui.json'
    if ($json) { Set-Content -LiteralPath $file -Value $json -Encoding utf8NoBOM }
    elseif (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file }
}

function Start-Window {
    $process = Start-Process -PassThru -FilePath $Exe -ArgumentList '--data-dir', "`"$dataDir`""
    $started.Add($process)
    for ($i = 0; $i -lt 100 -and $process.MainWindowHandle -eq 0 -and -not $process.HasExited; $i++) {
        Start-Sleep -Milliseconds 100
        $process.Refresh()
    }
    if ($process.MainWindowHandle -eq 0) { throw "gozler-ui did not open its window" }
    [void]$process.WaitForInputIdle(5000)
    return $process
}

# Asks the window to close as its close button does, and waits for its exit.
function Close-Window($process) {
    if ($process.HasExited) { return $true }
    [void][GozlerProbe.Win]::PostMessageW($process.MainWindowHandle, $WM_CLOSE, 0, 0)
    return $process.WaitForExit(10000)
}

function Window-Element($process) { [GozlerProbe.Ui]::FromHandle($process.MainWindowHandle) }

function Child-ById($parent, [string] $id) {
    [GozlerProbe.Ui]::Children($parent) | Where-Object AutomationId -EQ $id | Select-Object -First 1
}

function Frame-Text($hwnd) {
    $r = [GozlerProbe.Win]::Frame($hwnd)
    return "$($r.Left),$($r.Top),$($r.Right),$($r.Bottom)"
}

function Save-Screenshot($hwnd, [string] $name) {
    if (-not $Screenshots) { return }
    New-Item -ItemType Directory -Force $Screenshots | Out-Null
    $window = [GozlerProbe.Win]::Rect($hwnd)
    $frame = [GozlerProbe.Win]::Frame($hwnd)
    $bitmap = [System.Drawing.Bitmap]::new($window.Right - $window.Left, $window.Bottom - $window.Top)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $dc = $graphics.GetHdc()
    # PW_RENDERFULLCONTENT, so the DWM frame is drawn as on screen.
    [void][GozlerProbe.Win]::PrintWindow($hwnd, $dc, 2)
    $graphics.ReleaseHdc($dc)
    $graphics.Dispose()
    $crop = [System.Drawing.Rectangle]::new($frame.Left - $window.Left, $frame.Top - $window.Top,
        $frame.Right - $frame.Left, $frame.Bottom - $frame.Top)
    $shown = $bitmap.Clone($crop, $bitmap.PixelFormat)
    $bitmap.Dispose()
    $path = Join-Path $Screenshots "$name-$([GozlerProbe.Win]::GetDpiForWindow($hwnd))dpi.png"
    $shown.Save($path)
    $shown.Dispose()
    $script:screenshotFiles += $path
}

# Copies the screen area of `hwnd` and the open menus `popups`, which
# PrintWindow cannot draw.
function Save-ScreenArea($hwnd, $popups, [string] $name) {
    if (-not $Screenshots) { return }
    $a = [GozlerProbe.Win]::Frame($hwnd)
    $menuLeft = $menuTop = [int]::MaxValue
    $menuRight = $menuBottom = [int]::MinValue
    foreach ($popup in $popups) {
        $r = [GozlerProbe.Win]::Rect($popup)
        $menuLeft = [Math]::Min($menuLeft, $r.Left); $menuTop = [Math]::Min($menuTop, $r.Top)
        $menuRight = [Math]::Max($menuRight, $r.Right); $menuBottom = [Math]::Max($menuBottom, $r.Bottom)
    }
    # The menus with up to 200 pixels of the window beside them.
    $left = [Math]::Min($menuLeft, [Math]::Max($a.Left, $menuLeft - 200))
    $top = [Math]::Min($a.Top, $menuTop)
    $right = [Math]::Max($menuRight, [Math]::Min($a.Right, $menuRight + 200))
    $bottom = [Math]::Max($menuBottom, [Math]::Min($a.Bottom, $menuBottom + 40))
    $bitmap = [System.Drawing.Bitmap]::new($right - $left, $bottom - $top)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.CopyFromScreen($left, $top, 0, 0, $bitmap.Size)
    $graphics.Dispose()
    $path = Join-Path $Screenshots "$name-$([GozlerProbe.Win]::GetDpiForWindow($hwnd))dpi.png"
    $bitmap.Save($path)
    $bitmap.Dispose()
    $script:screenshotFiles += $path
}

# --- Menus -----------------------------------------------------------------

# The menu bar in the spec's order and groups (spec: Menus). Each line is a
# catalogue key, or = and a literal text; two more spaces of indent put it in
# the submenu above it; after | come its shortcut, whether it is enabled
# after the first search with the search box empty and nothing selected, and
# whether it is marked.
$menuSpec = @'
menu.file
  command.open||off|
  command.open_file_location|Ctrl+Enter|off|
  command.properties|Alt+Enter|off|
  -
  command.export_results||on|
  -
  command.settings|Ctrl+,|off|
  -
  command.exit||on|
menu.edit
  command.copy|Ctrl+C|off|
  command.copy_as_path|Ctrl+Shift+C|off|
  command.copy_name||off|
  -
  command.select_all|Ctrl+A|on|
  -
  command.find|Ctrl+F|on|
menu.view
  command.filters||on|marked
  command.inspector||off|marked
  command.columns||on|
    column.name||off|marked
    column.path||on|marked
    column.size||on|marked
    column.date_modified||on|marked
    column.extension||on|
    column.attributes||on|
  -
  command.sort_by||on|
    sort.name||on|marked
    sort.path||on|
    sort.size||on|
    sort.date_modified||on|
    sort.extension||on|
    -
    sort.ascending||on|marked
    sort.descending||on|
  -
  command.refresh|F5|on|
menu.help
  command.search_syntax|F1|on|
  -
  command.about||on|
menu.language|||right
  command.windows_default||off|marked
  -
  =&English||off|
'@
if ($Pseudo) { $menuSpec += "`n  =&qps-ploc||off|" }

# The expected menus as lines: indent, name, shortcut, access key, enabled,
# mark. A top-level menu is always enabled.
function Expected-MenuLines {
    foreach ($line in $menuSpec -split "`r?`n") {
        if (-not $line.Trim()) { continue }
        $indent = $line.Length - $line.TrimStart().Length
        $key, $shortcut, $enabled, $marked = $line.Trim().Split('|')
        if ($key -eq '-') { (' ' * $indent) + '-'; continue }
        $text = $key.StartsWith('=') ? $key.Substring(1) : (Caption $key)
        if ($indent -eq 0) { $enabled = 'on' }
        (' ' * $indent) + "$(Plain $text) | $shortcut | key $(AccessKeyOf $text) | $enabled | $marked"
    }
}

# The window's menus as lines, read through the Win32 menu functions, which
# need no keyboard focus. WM_INITMENUPOPUP first asks the window to set each
# item's state, as it does when a person opens the menu.
function Read-Menus($process, $menu = [GozlerProbe.Win]::GetMenu($process.MainWindowHandle), [int] $indent = 0) {
    $index = 0
    foreach ($item in [GozlerProbe.Win]::MenuItems($menu)) {
        if ($item.Separator) { (' ' * $indent) + '-'; continue }
        if ($item.SubMenu -ne 0) {
            [void][GozlerProbe.Win]::SendMessageW($process.MainWindowHandle, 0x0117, $item.SubMenu, [IntPtr]$index)
        }
        $label, $shortcut = $item.Text.Split("`t", 2)
        $mark = $item.RightJustify ? 'right' : ($item.Checked ? 'marked' : '')
        (' ' * $indent) + "$(Plain $label) | $shortcut | key $(AccessKeyOf $label) | $($item.Enabled ? 'on' : 'off') | $mark"
        if ($item.SubMenu -ne 0) { Read-Menus $process $item.SubMenu ($indent + 2) }
        $index++
    }
}

# The menu item ID of the item whose label is `label`, in any menu.
function Menu-Id($process, [string] $label, $menu = [GozlerProbe.Win]::GetMenu($process.MainWindowHandle)) {
    foreach ($item in [GozlerProbe.Win]::MenuItems($menu)) {
        if ($item.SubMenu -ne 0) {
            $id = Menu-Id $process $label $item.SubMenu
            if ($id) { return $id }
        }
        elseif ($item.Text -and $item.Text.Split("`t")[0] -eq $label) { return $item.Id }
    }
}

# Runs a menu item as a click on it does: WM_COMMAND with the item's ID.
function Run-MenuItem($process, [string] $label) {
    $id = Menu-Id $process $label
    if (-not $id) { throw "no menu item $label" }
    [void][GozlerProbe.Win]::PostMessageW($process.MainWindowHandle, 0x0111, [IntPtr]$id, 0)
}

function Menu-Bar($process) {
    $window = Window-Element $process
    # The window has two menu bars: the system menu's and its own, which
    # is the one with more than one item.
    [GozlerProbe.Ui]::Children($window) | Where-Object ControlType -EQ ([GozlerProbe.Ui]::MenuBar) |
        Where-Object { ([GozlerProbe.Ui]::Children($_)).Count -gt 1 } | Select-Object -First 1
}

# Whether the window is in front. The probe never brings it there, because
# the person at the machine would then type into it.
function Is-InFront($process) {
    return [GozlerProbe.Win]::GetForegroundWindow() -eq $process.MainWindowHandle
}

# The children of the window `hwnd`. A menu window exists before UI
# Automation can read it, and a read can time out while the window is busy,
# so a failed read is tried again.
function Children-Of($hwnd) {
    for ($i = 0; ; $i++) {
        try { return @([GozlerProbe.Ui]::Children([GozlerProbe.Ui]::FromHandle($hwnd))) }
        catch { if ($i -ge 20) { return @() } }
        Start-Sleep -Milliseconds 250
    }
}

# Opens the menu bar's menu at position `top` through UI Automation, and the
# submenu at position `sub` in it when `sub` is given, saves a screenshot of
# what is open, and closes it. Returns the names UI Automation reads in the
# last menu it opened. Runs only while the window is in front.
function Show-Menu($process, [int] $top, [int] $sub, [string] $name) {
    $item = @([GozlerProbe.Ui]::Children((Menu-Bar $process)))[$top]
    try {
        # UI Automation refuses to open a menu of a window that has left the
        # front; the caller then reports the check as not run.
        try { [GozlerProbe.Ui]::Expand($item) } catch { return [ordered]@{ opened = $false } }
        $menu = Wait-Popup $process @()
        if (-not $menu) { return [ordered]@{ opened = $false } }
        $popups = @($menu)
        if ($sub -ge 0) {
            $subItem = @(Children-Of $menu)[$sub]
            try { [GozlerProbe.Ui]::Expand($subItem) } catch { return [ordered]@{ opened = $false } }
            $menu = Wait-Popup $process $popups
            if (-not $menu) { return [ordered]@{ opened = $false } }
            $popups += $menu
        }
        Save-ScreenArea $process.MainWindowHandle $popups $name
        $items = @(Children-Of $menu | ForEach-Object Name)
        return [ordered]@{ opened = $true; items = $items }
    }
    finally { try { [GozlerProbe.Ui]::Collapse($item) } catch { } }
}

# A menu window of the process that is not in `known`, once it opens.
function Wait-Popup($process, $known) {
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 100
        $popup = [GozlerProbe.Win]::Windows($process.Id, '#32768') | Where-Object { $_ -notin $known } | Select-Object -First 1
        if ($popup) { return $popup }
    }
    return $null
}

# Opens every menu of the menu bar, and View's Columns and Sort by
# submenus, for their screenshots. `language` ends each file name.
function Show-Menus($process, [string] $language) {
    $shown = [ordered]@{}
    $menus = 'file', 'edit', 'view', 'help', 'language'
    for ($top = 0; $top -lt $menus.Count; $top++) {
        $shown[$menus[$top]] = Show-Menu $process $top -1 "menu-$($menus[$top])-$language"
    }
    # Columns and Sort by are View's third and fifth items; a separator is
    # an item too.
    $shown['view > columns'] = Show-Menu $process 2 2 "menu-view-columns-$language"
    $shown['view > sort by'] = Show-Menu $process 2 4 "menu-view-sort-$language"
    return $shown
}

# Opens Help > About Gozler, reads it, takes its screenshot, and closes it
# with its Close button.
function Open-About($process, [string] $screenshot) {
    # About Gozler is the last item of Help, the fourth menu.
    $about = [GozlerProbe.Win]::MenuItems((Find-Submenu $process 3))[-1]
    return Open-Dialog $process $about.Id $screenshot
}

# Runs the menu item with ID `id`, which opens a dialog, then reads the
# dialog, which control has the focus, and takes its screenshot. Esc closes
# it, posted to the focused control as the keyboard queues a key, so the
# dialog need not be in front; when it does not, its Close button does.
function Open-Dialog($process, [int] $id, [string] $screenshot) {
    [void][GozlerProbe.Win]::PostMessageW($process.MainWindowHandle, 0x0111, [IntPtr]$id, 0)
    $dialog = $null
    for ($i = 0; $i -lt 50 -and -not $dialog; $i++) {
        Start-Sleep -Milliseconds 100
        $dialog = [GozlerProbe.Win]::Windows($process.Id, '#32770') | Select-Object -First 1
    }
    if (-not $dialog) { return [ordered]@{ opened = $false } }
    Start-Sleep -Milliseconds 300
    $controls = Children-Of $dialog
    Save-Screenshot $dialog $screenshot
    $title = [GozlerProbe.Win]::Text($dialog)
    # UI Automation calls a multiline edit control a document, so the text
    # is found by its ID.
    $edit = $controls | Where-Object AutomationId -EQ $ids.IDC_SEARCH_SYNTAX_TEXT | Select-Object -First 1
    $editText = $edit ? [GozlerProbe.Win]::ControlText([IntPtr]$edit.Hwnd) : $null
    $close = $controls | Where-Object AutomationId -EQ '2' | Select-Object -First 1
    $focus = [GozlerProbe.Win]::Focus($dialog)
    $focusOnClose = $close -and $focus -eq [IntPtr]$close.Hwnd
    # WM_KEYDOWN with VK_ESCAPE.
    [void][GozlerProbe.Win]::PostMessageW(($focus -ne 0 ? $focus : $dialog), 0x0100, [IntPtr]0x1B, 0)
    for ($i = 0; $i -lt 30 -and [GozlerProbe.Win]::IsWindow($dialog); $i++) { Start-Sleep -Milliseconds 100 }
    $escClosed = -not [GozlerProbe.Win]::IsWindow($dialog)
    if (-not $escClosed) {
        [GozlerProbe.Ui]::InvokeLater($close)
        for ($i = 0; $i -lt 50 -and [GozlerProbe.Win]::IsWindow($dialog); $i++) { Start-Sleep -Milliseconds 100 }
    }
    return [ordered]@{
        title          = $title
        texts          = @($controls | ForEach-Object Name)
        edit           = $editText
        focus_on_close = $focusOnClose
        esc_closed     = $escClosed
        closed         = -not [GozlerProbe.Win]::IsWindow($dialog)
    }
}

# The submenu of the menu bar's item at `position`.
function Find-Submenu($process, [int] $position) {
    ([GozlerProbe.Win]::MenuItems([GozlerProbe.Win]::GetMenu($process.MainWindowHandle)))[$position].SubMenu
}

# --- Searching ---------------------------------------------------------------

# The main window's control IDs, from resources/ids.h, as UI Automation
# gives them.
$ids = @{}
foreach ($line in Get-Content (Join-Path $PSScriptRoot 'resources\ids.h')) {
    if ($line -match '^#define (ID\w+) (\d+)') { $ids[$Matches[1]] = $Matches[2] }
}
$SearchBoxId = $ids.IDC_SEARCH; $MatchLabelId = $ids.IDC_MATCH_LABEL; $MatchModeId = $ids.IDC_MATCH_MODE
$MatchCaseId = $ids.IDC_MATCH_CASE; $MatchPathId = $ids.IDC_MATCH_PATH; $ListId = $ids.IDC_LIST
$StatusBarId = $ids.IDC_STATUS; $MessageId = $ids.IDC_MESSAGE

# The menu item ID of `command`, a CommandId variant, as MenuItem::id gives
# it: the base of the commands, then the command's place in CommandId::ALL.
# It reaches a command that only a context menu shows.
function Command-Id([string] $command) {
    $table = Get-Content -Raw (Join-Path $PSScriptRoot 'src\core\commands.rs')
    if ($table -notmatch '(?s)const ALL: \[CommandId; \d+\] = \[(.*?)\];') { throw 'commands.rs has no CommandId::ALL' }
    $order = @([regex]::Matches($Matches[1], 'CommandId::(\w+)') | ForEach-Object { $_.Groups[1].Value })
    if ($table -notmatch 'MenuItem::Run\(command\) => \{\s*(0x[0-9a-f]+) \+ index\(CommandId::ALL') { throw 'commands.rs numbers its commands otherwise' }
    $index = [array]::IndexOf($order, $command)
    if ($index -lt 0) { throw "CommandId::ALL has no $command" }
    [Convert]::ToInt32($Matches[1], 16) + $index
}

function Control-Hwnd($process, [string] $id) { [IntPtr](Child-ById (Window-Element $process) $id).Hwnd }

# The filter pane's control `id`. The pane is a window of its own, so its
# controls are its children, not the main window's.
function Pane-Child($process, [string] $id) {
    Child-ById (Child-ById (Window-Element $process) $ids.IDC_FILTER_PANE) $id
}
function Pane-Hwnd($process, [string] $id) { [IntPtr](Pane-Child $process $id).Hwnd }

# Selects the item at `index` of the pane's list `id`, a drop-down list when
# `dropDown`, as a click or an arrow key does: the list changes, and the
# window receives LBN_SELCHANGE or CBN_SELCHANGE, which are both 1.
function Select-PaneItem($process, [string] $id, [int] $index, [bool] $dropDown) {
    $control = Pane-Hwnd $process $id
    # CB_SETCURSEL or LB_SETCURSEL
    [void][GozlerProbe.Win]::SendMessageW($control, ($dropDown ? 0x014E : 0x0186), [IntPtr]$index, 0)
    $command = [IntPtr]((1 -shl 16) -bor [int]$id)
    [void][GozlerProbe.Win]::SendMessageW($process.MainWindowHandle, 0x0111, $command, $control)
}

# The selected position of the pane's list `id`, a drop-down list when
# `dropDown`.
function Pane-Selected($process, [string] $id, [bool] $dropDown) {
    # CB_GETCURSEL or LB_GETCURSEL
    [long][GozlerProbe.Win]::SendMessageW((Pane-Hwnd $process $id), ($dropDown ? 0x0147 : 0x0188), 0, 0)
}

# The Location list's selected item.
function Selected-Location($process) {
    $items = [GozlerProbe.Win]::ListItems((Pane-Hwnd $process $ids.IDC_LOCATION))
    $items[(Pane-Selected $process $ids.IDC_LOCATION $false)]
}

# Selects `folder` in the Location list, an item since Search in this
# folder chose it.
function Set-Location($process, [string] $folder) {
    $items = @([GozlerProbe.Win]::ListItems((Pane-Hwnd $process $ids.IDC_LOCATION)))
    Select-PaneItem $process $ids.IDC_LOCATION ([array]::IndexOf($items, $folder)) $false
}

# The captions of the list message's buttons that show.
function Message-Buttons($process) {
    $element = Window-Element $process
    @($ids.IDC_MESSAGE_BUTTON_1, $ids.IDC_MESSAGE_BUTTON_2 | ForEach-Object { Child-ById $element $_ } | Where-Object { $_ } | ForEach-Object Name)
}

# The window's first dialog once it opens, or $null.
function Wait-Dialog($process) {
    for ($i = 0; $i -lt 100; $i++) {
        Start-Sleep -Milliseconds 100
        $dialog = [GozlerProbe.Win]::Windows($process.Id, '#32770') | Select-Object -First 1
        if ($dialog) { return $dialog }
    }
    return $null
}

# Clicks the dialog's button `id` through UI Automation and waits until the
# dialog closes; returns whether it closed.
function Close-Dialog($dialog, [int] $id) {
    $button = [GozlerProbe.Win]::ChildById($dialog, $id, 'Button')
    if ($button -ne 0) { [GozlerProbe.Ui]::InvokeLater([GozlerProbe.Ui]::FromHandle($button)) }
    for ($i = 0; $i -lt 50 -and [GozlerProbe.Win]::IsWindow($dialog); $i++) { Start-Sleep -Milliseconds 100 }
    $closed = -not [GozlerProbe.Win]::IsWindow($dialog)
    # A dialog left open would block the window's close at the end.
    if (-not $closed) { [void][GozlerProbe.Win]::PostMessageW($dialog, $WM_CLOSE, 0, 0) }
    return $closed
}

# Opens the custom range dialog of the pane's drop-down list `id`: Custom…
# is selected as an arrow key selects it, which opens nothing, then Enter
# is posted to the closed list as the keyboard queues it, so the window
# need not be in front. Returns whether a dialog opened after the arrow
# key, and the dialog that Enter opened.
function Open-Custom($process, [string] $id) {
    $list = Pane-Hwnd $process $id
    $custom = [GozlerProbe.Win]::ComboItems($list).Count - 1
    Select-PaneItem $process $id $custom $true
    Start-Sleep -Milliseconds 500
    $openedByArrow = [bool]([GozlerProbe.Win]::Windows($process.Id, '#32770') | Select-Object -First 1)
    # WM_KEYDOWN with VK_RETURN.
    [void][GozlerProbe.Win]::PostMessageW($list, 0x0100, [IntPtr]0x0D, 0)
    return $openedByArrow, (Wait-Dialog $process)
}

# The status bar's part at `index`, from 0. UI Automation sees a new window's
# controls a moment after the window, so a read is tried again.
function Status-Part($process, [int] $index) {
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $children = @([GozlerProbe.Ui]::Children((Window-Element $process)))
        $bar = $children | Where-Object AutomationId -EQ $StatusBarId | Select-Object -First 1
        if ($bar) { return @([GozlerProbe.Ui]::Children($bar))[$index].Name }
        Start-Sleep -Milliseconds 10
    } until ([DateTime]::UtcNow -gt $deadline)
    throw "UI Automation does not see the status bar among: $($children -join '; ')"
}

# The message that replaces the rows, or $null while it is hidden, which UI
# Automation leaves out.
function Message-Text($process) { (Child-ById (Window-Element $process) $MessageId).Name }

# "Searching…" is the one text of status part 1 with an ellipsis, in every
# language.
function Is-Searching([string] $text) { $text -like '*…*' }

# Waits until status part 1 shows an answer: a count, or nothing when a
# message replaces the rows.
function Wait-Answer($process, [int] $seconds = 30) {
    $deadline = [DateTime]::UtcNow.AddSeconds($seconds)
    while ($true) {
        $text = Status-Part $process 0
        if (-not (Is-Searching $text) -or [DateTime]::UtcNow -gt $deadline) { return $text }
        Start-Sleep -Milliseconds 100
    }
}

# Types `text` into the search box through UI Automation and returns status
# part 1 once the search has answered. The window searches 150 ms after the
# last change.
function Search-For($process, [string] $text) {
    [GozlerProbe.Ui]::SetValue((Child-ById (Window-Element $process) $SearchBoxId), $text)
    Start-Sleep -Milliseconds 300
    return Wait-Answer $process
}

# The count in "3 results", or $null for any other text.
function Count-Of([string] $text) {
    if ($text -match '^\[?([\d.,\s]+)\s\D') { return [long]($Matches[1] -replace '\D', '') }
    return $null
}

function Row-Count($process) {
    # LVM_GETITEMCOUNT
    [long][GozlerProbe.Win]::SendMessageW((Control-Hwnd $process $ListId), 0x1004, 0, 0)
}

# The full paths of the list's first `count` rows, from their Name and Path
# cells, as `gozler` prints them.
function Visible-Paths($process, [int] $count) {
    $list = Child-ById (Window-Element $process) $ListId
    @([GozlerProbe.Ui]::Rows($list, $count) | ForEach-Object { Join-Path $_[1] $_[0] })
}

# Chooses the match mode at `index` as a person's choice in the list does:
# the list changes, and the window receives CBN_SELCHANGE.
function Set-MatchMode($process, [int] $index) {
    $combo = Control-Hwnd $process $MatchModeId
    # CB_SETCURSEL
    [void][GozlerProbe.Win]::SendMessageW($combo, 0x014E, [IntPtr]$index, 0)
    $command = [IntPtr]((1 -shl 16) -bor [int]$MatchModeId)
    [void][GozlerProbe.Win]::SendMessageW($process.MainWindowHandle, 0x0111, $command, $combo)
}

# Sets the check box `id` to `on` through UI Automation's Toggle, a click.
function Set-CheckBox($process, [string] $id, [bool] $on) {
    $box = Child-ById (Window-Element $process) $id
    if (([GozlerProbe.Ui]::ToggleState($box) -eq 1) -ne $on) { [GozlerProbe.Ui]::Toggle($box) }
}

function Header-Names($process) {
    $listView = Child-ById (Window-Element $process) $ListId
    $header = [GozlerProbe.Ui]::HeaderOf($listView)
    @([GozlerProbe.Ui]::Children($header) | ForEach-Object Name)
}

# Clicks the header of the column at `index` through UI Automation.
function Click-Header($process, [int] $index) {
    $listView = Child-ById (Window-Element $process) $ListId
    $header = [GozlerProbe.Ui]::HeaderOf($listView)
    [GozlerProbe.Ui]::Invoke(@([GozlerProbe.Ui]::Children($header) | Where-Object ControlType -EQ ([GozlerProbe.Ui]::HeaderItem))[$index])
}

# The marked items of the submenu `key` of View, as Read-Menus prints them.
function Marked-Items($process, [string] $key) {
    $lines = @(Read-Menus $process)
    $start = [array]::IndexOf(($lines | ForEach-Object { $_.Trim().Split(' | ')[0] }), (Plain (Caption $key)))
    $marked = for ($i = $start + 1; $i -lt $lines.Count -and $lines[$i].StartsWith('    '); $i++) {
        if ($lines[$i].EndsWith('| marked')) { $lines[$i].Trim().Split(' | ')[0] }
    }
    @($marked)
}

# The process's memory, as the spec's budget counts it.
function Memory-Of($process) {
    $process.Refresh()
    [ordered]@{
        private_bytes      = $process.PrivateMemorySize64
        peak_private_bytes = $process.PeakPagedMemorySize64
        working_set        = $process.WorkingSet64
    }
}

# --- The selection and the actions -----------------------------------------

# Selects the list's rows `rows`, from 0, and no other, as clicks do. Each
# change notifies the window, which reads the selection once they stop.
function Select-Rows($process, $rows) {
    $list = Control-Hwnd $process $ListId
    # LVIS_SELECTED
    [GozlerProbe.Win]::SetItemState($list, -1, 0, 2)
    foreach ($row in $rows) { [GozlerProbe.Win]::SetItemState($list, $row, 2, 2) }
}

# Whether each item of the menu bar's menu at `position` is enabled, by its
# label, after WM_INITMENUPOPUP asks the window to set the states.
function Item-States($process, [int] $position) {
    $menu = Find-Submenu $process $position
    [void][GozlerProbe.Win]::SendMessageW($process.MainWindowHandle, 0x0117, $menu, [IntPtr]$position)
    $states = [ordered]@{}
    foreach ($item in [GozlerProbe.Win]::MenuItems($menu)) {
        if (-not $item.Separator) { $states[(Plain $item.Text.Split("`t")[0])] = $item.Enabled }
    }
    return $states
}

# A regular expression for the whole catalogue text `key`, with `first` and
# `second`, which are regular expressions, in place of {0} and {1}.
function Caption-Pattern([string] $key, [string] $first = '.+', [string] $second = '.+') {
    '^' + ([regex]::Escape((Caption $key)) -replace '\\\{0}', $first -replace '\\\{1}', $second) + '$'
}

# Runs File > Export results… and saves to `path` in the Windows save
# dialog. The path is typed in quotation marks, so the dialog adds no
# extension. The item is found by its ID, which is the same in every
# language. Returns whether the dialog closed.
function Export-To($process, [string] $path) {
    [void][GozlerProbe.Win]::PostMessageW($process.MainWindowHandle, 0x0111, [IntPtr]$menuIds.export, 0)
    $dialog = $null
    for ($i = 0; $i -lt 100 -and -not $dialog; $i++) {
        Start-Sleep -Milliseconds 100
        $dialog = [GozlerProbe.Win]::Windows($process.Id, '#32770') | Select-Object -First 1
    }
    if (-not $dialog) { return $false }
    # The file name box and Save, by the IDs the Windows dialog gives them.
    $edit = $save = [IntPtr]::Zero
    for ($i = 0; $i -lt 50 -and ($edit -eq 0 -or $save -eq 0); $i++) {
        Start-Sleep -Milliseconds 100
        $edit = [GozlerProbe.Win]::ChildById($dialog, 1001, 'Edit')
        $save = [GozlerProbe.Win]::ChildById($dialog, 1, 'Button')
    }
    if ($edit -ne 0 -and $save -ne 0) {
        [GozlerProbe.Win]::SetControlText($edit, "`"$path`"")
        [GozlerProbe.Ui]::InvokeLater([GozlerProbe.Ui]::FromHandle($save))
        for ($i = 0; $i -lt 100 -and [GozlerProbe.Win]::IsWindow($dialog); $i++) { Start-Sleep -Milliseconds 100 }
    }
    $closed = -not [GozlerProbe.Win]::IsWindow($dialog)
    # A dialog left open would block the window's close at the end.
    if (-not $closed) { [void][GozlerProbe.Win]::PostMessageW($dialog, $WM_CLOSE, 0, 0) }
    return $closed
}

# Status part 3 once an export has started and no longer runs.
function Wait-Export($process, [int] $seconds = 60) {
    $running = (Caption 'status.exporting_other').Split('{')[0]
    $deadline = [DateTime]::UtcNow.AddSeconds($seconds)
    do {
        $text = Status-Part $process 2
        if ($text -and -not $text.StartsWith($running)) { return $text }
        Start-Sleep -Milliseconds 100
    } until ([DateTime]::UtcNow -gt $deadline)
    return $text
}

# The texts of the window's message box once it opens; the probe takes its
# screenshot and closes it with its button.
function Close-MessageBox($process, [string] $screenshot) {
    $box = $null
    for ($i = 0; $i -lt 100 -and -not $box; $i++) {
        Start-Sleep -Milliseconds 100
        $box = [GozlerProbe.Win]::Windows($process.Id, '#32770') | Select-Object -First 1
    }
    if (-not $box) { return $null }
    Start-Sleep -Milliseconds 300
    $controls = Children-Of $box
    Save-Screenshot $box $screenshot
    $button = $controls | Where-Object ControlType -EQ ([GozlerProbe.Ui]::Button) | Select-Object -First 1
    if ($button) { [GozlerProbe.Ui]::InvokeLater($button) }
    for ($i = 0; $i -lt 50 -and [GozlerProbe.Win]::IsWindow($box); $i++) { Start-Sleep -Milliseconds 100 }
    return @($controls | ForEach-Object Name)
}

# Whether two files hold the same bytes.
function Same-Bytes([string] $a, [string] $b) {
    if (-not (Test-Path -LiteralPath $a) -or -not (Test-Path -LiteralPath $b)) { return $false }
    [Convert]::ToBase64String([System.IO.File]::ReadAllBytes($a)) -ceq [Convert]::ToBase64String([System.IO.File]::ReadAllBytes($b))
}

# Starts a window and takes its screenshot before the first search answers
# (S1). Returns the window and status part 1 just after the screenshot,
# read with one message, so it shows what the screenshot shows whatever the
# machine's speed; UI Automation would read it too late.
function Start-Searching([string] $language) {
    $process = Start-Process -PassThru -FilePath $Exe -ArgumentList '--data-dir', "`"$dataDir`""
    $started.Add($process)
    for ($i = 0; $i -lt 1000 -and $process.MainWindowHandle -eq 0 -and -not $process.HasExited; $i++) {
        Start-Sleep -Milliseconds 10
        $process.Refresh()
    }
    if ($process.MainWindowHandle -eq 0) { throw "gozler-ui did not open its window" }
    $status = [GozlerProbe.Win]::ChildById($process.MainWindowHandle, [int]$StatusBarId, 'msctls_statusbar32')
    Save-Screenshot $process.MainWindowHandle "searching-first-$language"
    return $process, [GozlerProbe.Win]::ControlText($status)
}

# Takes a screenshot of each state that stage 3 reaches in a fixture
# search, in `language`. `menuIds` holds the IDs of View > Columns >
# Extension and Help > Search syntax, which are the same in every language.
function Show-States($process, [string] $language, $menuIds) {
    $states = [ordered]@{
        results    = "$files\ ext:cs"
        bold       = "$files\ para 50"
        no_results = "$files\ zzzz"
        query      = 'dz:x'
        refused    = 'regex:('
        date       = "$files\ dm:yesterdy"
    }
    # The window paints a reply's rows and status a moment after it shows
    # them to UI Automation, so each screenshot waits for that paint.
    foreach ($state in $states.GetEnumerator()) {
        [void](Search-For $process $state.Value)
        Start-Sleep -Milliseconds 500
        Save-Screenshot $process.MainWindowHandle "$($state.Key)-$language"
    }
    [void](Search-For $process "$files\ ext:cs")
    # Size is the third column, and Name the first.
    Click-Header $process 2
    Start-Sleep -Milliseconds 300
    [void](Wait-Answer $process)
    Start-Sleep -Milliseconds 500
    Save-Screenshot $process.MainWindowHandle "sorted-size-$language"
    Click-Header $process 0
    Start-Sleep -Milliseconds 300
    [void](Wait-Answer $process)
    [void][GozlerProbe.Win]::PostMessageW($process.MainWindowHandle, 0x0111, [IntPtr]$menuIds.extension, 0)
    Start-Sleep -Milliseconds 300
    Save-Screenshot $process.MainWindowHandle "column-extension-$language"
    [void][GozlerProbe.Win]::PostMessageW($process.MainWindowHandle, 0x0111, [IntPtr]$menuIds.extension, 0)
    return Open-Dialog $process $menuIds.syntax "search-syntax-$language"
}

# --- The fixture folder ------------------------------------------------------

function Remove-Fixture([string] $folder) {
    $denied = Join-Path $folder 'files\denied'
    if (Test-Path -LiteralPath $denied) { icacls $denied /remove:d $env:USERNAME | Out-Null }
    Remove-Item -LiteralPath $folder -Recurse -Force
}

function New-Fixture([string] $files) {
    New-Item -ItemType Directory $files | Out-Null
    $many = Join-Path $files 'many'
    for ($f = 0; $f -lt [Math]::Ceiling($FixtureFiles / 1000); $f++) {
        $folder = [System.IO.Directory]::CreateDirectory((Join-Path $many ('part{0:d2}' -f $f))).FullName
        for ($i = $f * 1000; $i -lt [Math]::Min($FixtureFiles, ($f + 1) * 1000); $i++) {
            [System.IO.File]::WriteAllBytes((Join-Path $folder ('item{0:d5}.txt' -f $i)), [byte[]]::new($i % 7))
        }
    }
    $named = @{
        'docs\Paracetamol 500mg.pdf' = 1234
        'docs\comparator.txt'        = 0
        'docs\Comparator Notes.txt'  = 2048
        'docs\report final.docx'     = 4096
        'docs\reporting.docx'        = 8192
        'case\upper\README.md'       = 10
        'case\lower\readme.md'       = 20
        'src\Program.cs'             = 300
        'src\ProgramTests.cs'        = 400
        'src\deep\inner\Model.cs'    = 500
        'attributes\hidden.txt'      = 1
        'attributes\read-only.txt'   = 2
        'to-delete\gone.txt'         = 3
        'denied\kept.txt'            = 4
        # Directly in the fixture folder, for Search in this folder and the
        # filters; Large is above 1 MB.
        'large.bin'                  = 2MB
        'old.txt'                    = 5
    }
    foreach ($entry in $named.GetEnumerator()) {
        $path = Join-Path $files $entry.Key
        [void][System.IO.Directory]::CreateDirectory((Split-Path $path))
        [System.IO.File]::WriteAllBytes($path, [byte[]]::new($entry.Value))
    }
    [System.IO.File]::SetLastWriteTime((Join-Path $files 'docs\Paracetamol 500mg.pdf'), [datetime]'2024-02-29 13:14:15')
    [System.IO.File]::SetLastWriteTime((Join-Path $files 'docs\comparator.txt'), [datetime]'2020-01-01 00:00:00')
    # Outside Last 7 days and Last 30 days.
    [System.IO.File]::SetLastWriteTime((Join-Path $files 'old.txt'), (Get-Date).AddDays(-40))
    [System.IO.File]::SetAttributes((Join-Path $files 'attributes\hidden.txt'), 'Hidden, Archive')
    [System.IO.File]::SetAttributes((Join-Path $files 'attributes\read-only.txt'), 'ReadOnly, Archive')
    # NTFS ignores the read-only attribute on a folder, so the failed export
    # of later stages uses a folder whose ACL denies creating files and
    # folders. (W) would also deny SYNCHRONIZE, which listing the folder needs.
    icacls (Join-Path $files 'denied') /deny "$($env:USERNAME):(WD,AD)" | Out-Null
}

# The rows `gozler -csv -size -dm -attributes` gives for the folder, sorted.
function Expected-Rows([string] $files) {
    $items = Get-ChildItem -LiteralPath $files -Recurse -Force
    $sizes = @{}
    foreach ($file in $items | Where-Object { -not $_.PSIsContainer }) {
        for ($folder = $file.DirectoryName; $folder.Length -gt $files.Length; $folder = Split-Path $folder) {
            $sizes[$folder] = $sizes[$folder] + $file.Length
        }
    }
    $rows = foreach ($item in $items) {
        $size = $item.PSIsContainer ? [long]$sizes[$item.FullName] : $item.Length
        '{0},{1},{2},"{3}"' -f $size, $item.LastWriteTime.ToString('yyyy-MM-ddTHH:mm:ss'), [int]$item.Attributes, $item.FullName
    }
    return $rows | Sort-Object -CaseSensitive
}

# =============================================================================

$mutex = [System.Threading.Mutex]::new($false, 'Local\gozler.probe')
try { $owned = $mutex.WaitOne(0) } catch [System.Threading.AbandonedMutexException] { $owned = $true }
if (-not $owned) { throw 'Another probe is running; it holds Local\gozler.probe.' }

$fixture = $null
$screenshotFiles = @()
try {
    $running = Get-Process gozler-ui -ErrorAction SilentlyContinue
    if ($running) {
        throw "A gozler-ui is running (process $($running.Id -join ', ')). It is not the probe's, and a second start would bring it forward, so the probe stops."
    }

    # A folder that an interrupted probe left; this probe holds the mutex, so
    # no running probe owns it.
    Get-ChildItem -LiteralPath $env:TEMP -Directory -Filter 'gozler-probe-*' | ForEach-Object { Remove-Fixture $_.FullName }

    $fixture = Join-Path $env:TEMP "gozler-probe-$([System.IO.Path]::GetRandomFileName().Replace('.', ''))"
    $files = Join-Path $fixture 'files'
    $dataDir = Join-Path $fixture 'data'
    New-Item -ItemType Directory $dataDir | Out-Null
    # The window's exports go beside the fixture, so no search of it finds
    # them.
    $exports = Join-Path $fixture 'exports'
    New-Item -ItemType Directory $exports | Out-Null
    $built = Measure-Command { New-Fixture $files }
    $results['fixture'] = [ordered]@{ folder = $files; files = $FixtureFiles; seconds = [int]$built.TotalSeconds }

    # --- The window's controls and menus, in English ------------------------
    $window = Start-Window
    $hwnd = $window.MainWindowHandle
    $results['monitor_dpi'] = [GozlerProbe.Win]::GetDpiForWindow($hwnd)
    Start-Sleep -Seconds 2
    $window.Refresh()
    $results['memory'] = [ordered]@{
        private_bytes      = $window.PrivateMemorySize64
        peak_private_bytes = $window.PeakPagedMemorySize64
        working_set        = $window.WorkingSet64
    }
    $element = Window-Element $window
    $search = Child-ById $element $SearchBoxId
    Check 'search box' ($search.ControlType -eq [GozlerProbe.Ui]::Edit) "$search"

    $match = Child-ById $element $MatchModeId
    $modes = 'start_of_words', 'start_of_name', 'anywhere', 'whole_words', 'regular_expression' | ForEach-Object { Caption "match.$_" }
    $chosen = [GozlerProbe.Ui]::Value($match)
    # A closed drop-down list shows UI Automation no items, so its items are
    # read with CB_GETLBTEXT.
    $items = [GozlerProbe.Win]::ComboItems([IntPtr]$match.Hwnd)
    Check 'match list' ($match.ControlType -eq [GozlerProbe.Ui]::ComboBox -and $chosen -eq $modes[0] -and
        (($items -join '|') -eq ($modes -join '|'))) "label '$($match.Name)', chosen '$chosen', items: $($items -join ', ')"

    $boxes = foreach ($id in $MatchCaseId, $MatchPathId) {
        $box = Child-ById $element $id
        [ordered]@{ name = $box.Name; type = $box.ControlType; toggle = [GozlerProbe.Ui]::ToggleState($box) }
    }
    Check 'match check boxes' ($boxes[0].name -eq (Plain (Caption 'match.case')) -and
        $boxes[1].name -eq (Plain (Caption 'match.path')) -and
        @($boxes | Where-Object { $_.type -eq [GozlerProbe.Ui]::CheckBox -and $_.toggle -eq 0 }).Count -eq 2) $boxes

    $results['match controls enabled'] = @($MatchLabelId, $MatchModeId, $MatchCaseId, $MatchPathId | ForEach-Object { (Child-ById $element $_).Enabled })

    $listView = Child-ById $element $ListId
    $header = [GozlerProbe.Ui]::HeaderOf($listView)
    $headers = if ($header) { [GozlerProbe.Ui]::Children($header) | ForEach-Object Name } else { @() }
    $expectedHeaders = 'name', 'path', 'size', 'date_modified' | ForEach-Object { Caption "header.$_" }
    Check 'list columns' (($headers -join '|') -eq ($expectedHeaders -join '|')) ($headers -join ', ')

    $status = Child-ById $element $StatusBarId
    $parts = @([GozlerProbe.Ui]::Children($status))
    $englishPartWidths = @($parts | ForEach-Object { [int]$_.Rect[2] })
    Check 'status bar parts' ($parts.Count -eq 4) ([ordered]@{ count = $parts.Count; widths = $englishPartWidths; texts = @($parts | ForEach-Object Name) })

    Save-Screenshot $hwnd 'main-en'

    $expected = @(Expected-MenuLines)
    $actual = @(Read-Menus $window)
    $diff = Compare-Object $expected $actual -SyncWindow 0 | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }
    Check 'menus' (-not $diff) ([ordered]@{ menus = $actual; differences = @($diff) })

    # UI Automation reads the menu bar without opening it; an open menu
    # only while the window is in front.
    $bar = @([GozlerProbe.Ui]::Children((Menu-Bar $window)) | ForEach-Object { "$($_.Name) ($($_.AccessKey))" })
    $expectedBar = @('menu.file', 'menu.edit', 'menu.view', 'menu.help', 'menu.language' | ForEach-Object {
        $text = Caption $_
        "$(Plain $text) (Alt+$(AccessKeyOf $text))"
    })
    Check 'menu bar through UI Automation' (($bar -join '|') -eq ($expectedBar -join '|')) $bar
    if (Is-InFront $window) {
        $menus = Show-Menus $window 'en'
        $closed = @($menus.Keys | Where-Object { -not $menus[$_].opened -or $menus[$_].items.Count -eq 0 })
        if ($closed -and [GozlerProbe.Win]::GetForegroundWindow() -ne $window.MainWindowHandle) {
            Report 'open menu through UI Automation' 'not run' ([ordered]@{ note = 'the window left the front while its menus were opened'; menus = $menus })
        }
        else { Check 'open menu through UI Automation' (-not $closed) $menus }

        # IsDialogMessageW runs for the main window, which is not a dialog,
        # and must still let Alt and a menu's access key open the menu.
        Start-Sleep -Milliseconds 300
        $fileKey = [uint16][char](AccessKeyOf (Caption 'menu.file')).ToUpperInvariant()
        # VK_MENU (Alt), then the letter.
        $pressed = [GozlerProbe.Win]::PressKeys($window.MainWindowHandle, 0x12, $fileKey)
        $popup = $null
        for ($i = 0; $pressed -and $i -lt 20 -and -not $popup; $i++) {
            Start-Sleep -Milliseconds 100
            $popup = [GozlerProbe.Win]::Windows($window.Id, '#32768') | Select-Object -First 1
        }
        $first = if ($popup) { (Children-Of $popup | Select-Object -First 1).Name }
        if ($popup) {
            # VK_ESCAPE closes the menu, and a second leaves the menu bar.
            [void][GozlerProbe.Win]::PressKeys($window.MainWindowHandle, 0x1B)
            [void][GozlerProbe.Win]::PressKeys($window.MainWindowHandle, 0x1B)
        }
        if ($pressed) {
            Check 'Alt and an access key open a menu' ($first -eq (Plain (Caption 'command.open'))) "first item of the menu that opened: '$first'"
        }
        else { Report 'Alt and an access key open a menu' 'not run' 'the window was not in front' }
    }
    else {
        Report 'open menu through UI Automation' 'not run' 'the window was not in front, so no menu was opened'
        Report 'Alt and an access key open a menu' 'not run' 'the window was not in front, so no key was pressed'
    }

    # Help > About Gozler shows the version, and Close closes it (C23).
    $expectedVersion = (Caption 'about.version') -replace '\{0\}', (& $gozler -v | Select-Object -First 1)
    $about = Open-About $window 'about-en'
    Check 'about' ($about.title -eq (Caption 'about.title') -and $about.texts -contains $expectedVersion -and
        $about.focus_on_close -and $about.esc_closed -and $about.closed) ([ordered]@{ about = $about; expected_version = $expectedVersion })

    # File > Exit ends the process (C7).
    Run-MenuItem $window (Caption 'command.exit')
    Check 'exit' $window.WaitForExit(10000) "exit code $($window.HasExited ? $window.ExitCode : 'still running')"
    [void](Close-Window $window)

    # --- A second start ------------------------------------------------------
    # Windows lets the second process bring the first window forward only
    # when the process that started it may set the foreground window, as
    # File Explorer may when a person starts gozler. Whether the probe may
    # is read from whether the first window came to the front when it started.
    foreach ($state in @{ name = 'normal'; show = 1 }, @{ name = 'minimized'; show = 6 }, @{ name = 'maximized'; show = 3 }) {
        Write-Settings $null
        $first = Start-Window
        $firstHwnd = $first.MainWindowHandle
        $mayBringForward = Is-InFront $first
        [void][GozlerProbe.Win]::ShowWindow($firstHwnd, $state.show)
        Start-Sleep -Milliseconds 500
        $second = Start-Process -PassThru -FilePath $Exe -ArgumentList '--data-dir', "`"$dataDir`""
        $started.Add($second)
        $secondEnded = $second.WaitForExit(10000)
        Start-Sleep -Milliseconds 500
        $processes = @(Get-Process gozler-ui -ErrorAction SilentlyContinue)
        $detail = [ordered]@{
            probe_may_bring_forward = $mayBringForward
            second_ended            = $secondEnded
            processes               = $processes.Count
            in_front                = [GozlerProbe.Win]::GetForegroundWindow() -eq $firstHwnd
            minimized               = [GozlerProbe.Win]::IsIconic($firstHwnd)
            maximized               = [GozlerProbe.Win]::IsZoomed($firstHwnd)
        }
        $handedOver = $secondEnded -and $processes.Count -eq 1 -and -not $detail.minimized -and
            $detail.maximized -eq ($state.name -eq 'maximized')
        if ($handedOver -and -not $detail.in_front -and -not $mayBringForward) {
            Report "second start, $($state.name)" 'not run' ([ordered]@{
                    note   = 'one process, and the first window restored; whether it comes to the front was not checked, because the probe may not set the foreground window'
                    detail = $detail
                })
        }
        else { Check "second start, $($state.name)" ($handedOver -and $detail.in_front) $detail }
        [void](Close-Window $first)
    }

    # --- The saved placement -----------------------------------------------
    Write-Settings $null
    $window = Start-Window
    # SWP_NOZORDER | SWP_NOACTIVATE
    [void][GozlerProbe.Win]::SetWindowPos($window.MainWindowHandle, 0, 160, 140, 1100, 720, 0x14)
    Start-Sleep -Milliseconds 300
    $closedAt = Frame-Text $window.MainWindowHandle
    [void](Close-Window $window)
    $window = Start-Window
    $openedAt = Frame-Text $window.MainWindowHandle
    Check 'placement kept' ($closedAt -eq $openedAt) "closed at $closedAt, opened at $openedAt"
    [void](Close-Window $window)

    Write-Settings '{ "window": { "placement": { "left": -32000, "top": -32000, "right": -30900, "bottom": -31300, "maximized": false } } }'
    $window = Start-Window
    # MONITOR_DEFAULTTONULL
    $onMonitor = [GozlerProbe.Win]::MonitorFromWindow($window.MainWindowHandle, 0) -ne 0
    Check 'off-screen placement moved' $onMonitor "now at $(Frame-Text $window.MainWindowHandle)"
    [void](Close-Window $window)

    # --- The pseudo-language, and an unknown tag ----------------------------
    function Read-Captions($process) {
        $element = Window-Element $process
        $bar = Menu-Bar $process
        $listView = Child-ById $element $ListId
        $header = [GozlerProbe.Ui]::HeaderOf($listView)
        [ordered]@{
            menus       = @([GozlerProbe.Ui]::Children($bar) | ForEach-Object Name)
            match       = @($MatchLabelId, $MatchCaseId, $MatchPathId | ForEach-Object { (Child-ById $element $_).Name })
            filters     = @($ids.IDC_KIND_LABEL, $ids.IDC_LOCATION_LABEL, $ids.IDC_DATE_LABEL, $ids.IDC_SIZE_LABEL, $ids.IDC_ATTRIBUTES_LABEL | ForEach-Object { (Pane-Child $process $_).Name })
            headers     = @([GozlerProbe.Ui]::Children($header) | ForEach-Object Name)
            status_bars = @([GozlerProbe.Ui]::Children((Child-ById $element $StatusBarId)) | ForEach-Object { [int]$_.Rect[2] })
            results     = @([GozlerProbe.Ui]::Children((Child-ById $element $StatusBarId)))[0].Name
        }
    }
    $englishMenus = 'menu.file', 'menu.edit', 'menu.view', 'menu.help', 'menu.language' | ForEach-Object { Plain (Caption $_) }

    if ($Pseudo) {
        Write-Settings '{ "settings": { "language": "qps-ploc" } }'
        $window = Start-Window
        $captions = Read-Captions $window
        $pseudoText = @($captions.menus + $captions.match + $captions.filters + $captions.headers + $captions.results | Where-Object { $_ -notmatch '^\[.*\]$' })
        # The longest text a part can show sets its width.
        $wider = $captions.status_bars.Count -eq 4 -and $captions.status_bars[0] -gt $englishPartWidths[0]
        Check 'pseudo-language' ($pseudoText.Count -eq 0 -and $wider) ([ordered]@{ captions = $captions; not_pseudo = $pseudoText; english_status_widths = $englishPartWidths })
        # As before the English screenshot, so the window has painted.
        Start-Sleep -Seconds 2
        Save-Screenshot $window.MainWindowHandle 'main-pseudo'
        if (Is-InFront $window) {
            $results['open menu, pseudo-language'] = Show-Menus $window 'pseudo'
        }
        $results['about, pseudo-language'] = Open-About $window 'about-pseudo'
        [void](Close-Window $window)

        Write-Settings '{ "settings": { "language": "zz-ZZ" } }'
        $window = Start-Window
        $captions = Read-Captions $window
        Check 'unknown language tag shows English' (($captions.menus -join '|') -eq ($englishMenus -join '|')) $captions
        [void](Close-Window $window)
    }
    else {
        Report 'pseudo-language' 'not run' 'run with -Pseudo on a check.ps1 -Release -Pseudo build'
        Report 'unknown language tag shows English' 'not run' 'run with -Pseudo on a check.ps1 -Release -Pseudo build'
    }

    # --- gozler lists the fixture -------------------------------------------
    # The daemon reads a new file's size and date after its name, and a
    # folder's size after those, so the wait ends when every row matches,
    # or when the file rows match and no row changed in the last poll.
    # The file rows are also compared alone, which tells a folder size from
    # a file that is missing.
    $fileRows = { param($all) @($all | Where-Object { -not ([int]$_.Split(',')[2] -band 16) }) }
    $expectedRows = @(Expected-Rows $files)
    $expectedFiles = & $fileRows $expectedRows
    $deadline = [DateTime]::UtcNow.AddMinutes(3)
    $previous = $null
    do {
        $output = @(& $gozler -path "$files\" -n all -size -dm -attributes -csv -no-header 2>&1)
        $code = $LASTEXITCODE
        if ($code -ne 0 -and $code -ne 2) { break }
        $rows = @($output | Where-Object { $_ -is [string] } | Sort-Object -CaseSensitive)
        $listed = ($rows -join "`n") -ceq ($expectedRows -join "`n")
        $filesListed = ((& $fileRows $rows) -join "`n") -ceq ($expectedFiles -join "`n")
        $settled = $filesListed -and ($rows -join "`n") -ceq $previous
        $previous = $rows -join "`n"
        if (-not $listed -and -not $settled) { Start-Sleep -Seconds 2 }
    } until ($listed -or $settled -or [DateTime]::UtcNow -gt $deadline)
    if ($code -ne 0 -and $code -ne 2) {
        Report 'gozler lists the fixture' 'not run' "gozler exited $code`: $($output -join ' ')"
    }
    else {
        $listedSet = [System.Collections.Generic.HashSet[string]]::new([string[]]$rows)
        $expectedSet = [System.Collections.Generic.HashSet[string]]::new([string[]]$expectedRows)
        $differences = @($expectedRows | Where-Object { -not $listedSet.Contains($_) } | ForEach-Object { "expected $_" }) +
            @($rows | Where-Object { -not $expectedSet.Contains($_) } | ForEach-Object { "listed $_" })
        Check 'gozler lists the fixture' $listed ([ordered]@{
                expected          = $expectedRows.Count
                listed            = $rows.Count
                file_rows_match   = $filesListed
                differences       = $differences.Count
                first_differences = @($differences | Where-Object { $_ -like 'expected *' } | Select-Object -First 3) +
                    @($differences | Where-Object { $_ -like 'listed *' } | Select-Object -First 3)
            })
    }

    # --- Search and results -------------------------------------------------
    Write-Settings $null
    $window, $firstPart = Start-Searching 'en'
    $firstDetail = "status part 1 after the screenshot: '$firstPart'"
    # The first search is the empty one, which shows the limit (S4). That
    # answer after the screenshot means the reply came first, which says
    # nothing about the window.
    if ($firstPart -match (Caption-Pattern 'status.showing')) {
        Report 'first screenshot before the first reply' 'not run' "the first reply arrived before the screenshot was taken; $firstDetail"
    }
    else { Check 'first screenshot before the first reply' (Is-Searching $firstPart) $firstDetail }
    $menuIds = @{
        extension = Menu-Id $window (Caption 'column.extension')
        syntax    = Menu-Id $window (Caption 'command.search_syntax')
        export    = Menu-Id $window (Caption 'command.export_results')
    }

    # The empty search lists every entry, up to the limit (S4).
    $answer = Wait-Answer $window
    Save-Screenshot $window.MainWindowHandle 'limit-en'
    $showing = '^' + ([regex]::Escape((Caption 'status.showing')) -replace '\\\{[01]}', '([\d.,\s]+)') + '$'
    $numbers = @(if ($answer -match $showing) { $Matches[1], $Matches[2] | ForEach-Object { [long]($_ -replace '\D', '') } })
    Check 'empty search shows the limit' ($numbers.Count -eq 2 -and $numbers[0] -eq 100000 -and $numbers[1] -gt 100000 -and
        (Row-Count $window) -eq 100000) ([ordered]@{
            status = $answer; rows = Row-Count $window
        })

    $list = Child-ById (Window-Element $window) $ListId
    $firstRows = @([GozlerProbe.Ui]::Rows($list, 5))
    Check 'UI Automation reads the visible rows' ($firstRows.Count -eq 5 -and -not ($firstRows | Where-Object { -not $_[0] -or -not $_[1] })) @($firstRows | ForEach-Object { $_ -join ' | ' })

    # The last row's cells, once the list has fetched them.
    function Wait-LastRow($process) {
        for ($i = 0; $i -lt 50; $i++) {
            $last = [GozlerProbe.Ui]::LastRow((Child-ById (Window-Element $process) $ListId))
            if ($last -and $last[0]) { return , $last }
            Start-Sleep -Milliseconds 100
        }
        return $null
    }
    function Press-End($process) {
        [GozlerProbe.Ui]::Focus((Child-ById (Window-Element $process) $ListId))
        # VK_END
        return [GozlerProbe.Win]::PressKeys($process.MainWindowHandle, 0x23)
    }
    if ((Is-InFront $window) -and (Press-End $window)) {
        $last = Wait-LastRow $window
        Check 'End in the empty search shows a row' ([bool]$last) ($last -join ' | ')
    }
    else { Report 'End in the empty search shows a row' 'not run' 'the window was not in front' }
    $memory = Memory-Of $window
    Check 'memory budget' ($memory.private_bytes -lt 16MB) $memory

    # The searches below read names, file sizes and file dates, so they need
    # the file rows only.
    if (-not $filesListed) {
        Report 'fixture searches' 'not run' 'gozler did not list the fixture'
    }
    else {
        $F = "$files\"
        $csCount = @(Get-ChildItem -LiteralPath $files -Recurse -Filter '*.cs').Count
        $answer = Search-For $window "$F ext:cs"
        $cli = @(& $gozler $F ext:cs -n 20)
        $cliCount = [long](& $gozler -get-result-count $F ext:cs)
        $paths = @(Visible-Paths $window 20)
        Check 'ext:cs count and rows' ((Count-Of $answer) -eq $csCount -and $cliCount -eq $csCount -and
            ($paths -join '|') -eq ($cli -join '|')) ([ordered]@{
                status = $answer; fixture = $csCount; gozler = $cliCount; rows = $paths; gozler_rows = $cli
            })

        # Down in the search box moves to the first row (J1).
        if (Is-InFront $window) {
            [GozlerProbe.Ui]::Focus((Child-ById (Window-Element $window) $SearchBoxId))
            # VK_DOWN
            [void][GozlerProbe.Win]::PressKeys($window.MainWindowHandle, 0x28)
            Start-Sleep -Milliseconds 300
            $listHwnd = Control-Hwnd $window $ListId
            # LVM_GETNEXTITEM from the start with LVNI_SELECTED
            $selected = [long][GozlerProbe.Win]::SendMessageW($listHwnd, 0x100C, [IntPtr](-1), [IntPtr]2)
            $focused = [GozlerProbe.Win]::Focus($window.MainWindowHandle) -eq $listHwnd
            Check 'Down selects the first row' ($focused -and $selected -eq 0) ([ordered]@{ list_focused = $focused; selected = $selected })
        }
        else { Report 'Down selects the first row' 'not run' 'the window was not in front' }

        # A click on Size sorts by size, largest first (C20).
        Click-Header $window 2
        Start-Sleep -Milliseconds 300
        [void](Wait-Answer $window)
        $paths = @(Visible-Paths $window 20)
        $cli = @(& $gozler $F ext:cs -n 20 -sort size-descending)
        $marks = @(Marked-Items $window 'command.sort_by')
        $expectedMarks = @('sort.size', 'sort.descending' | ForEach-Object { Plain (Caption $_) })
        Check 'Size header sorts' (($paths -join '|') -eq ($cli -join '|') -and ($marks -join '|') -eq ($expectedMarks -join '|')) ([ordered]@{
                rows = $paths; gozler_rows = $cli; sort_by_marks = $marks
            })
        Click-Header $window 0
        Start-Sleep -Milliseconds 300
        [void](Wait-Answer $window)

        $answer = Search-For $window $F
        if ((Is-InFront $window) -and (Press-End $window)) {
            $last = Wait-LastRow $window
            $cliLast = @(& $gozler $F -n all)[-1]
            $lastPath = $last ? (Join-Path $last[1] $last[0]) : $null
            Check 'End shows the last row' ($lastPath -eq $cliLast) ([ordered]@{ status = $answer; last_row = $lastPath; gozler_last = $cliLast })
        }
        else { Report 'End shows the last row' 'not run' 'the window was not in front' }

        # Each match mode and check box against the same search of gozler
        # (C19), with the fixture's own count.
        $regex = [regex]::Escape($F) + '.*\.cs$'
        $cases = @(
            @{ name = 'Start of words'; mode = 0; text = "$F para"; cli = @($F, 'wordstart:para'); fixture = 1 }
            @{ name = 'Start of name'; mode = 1; text = "$F rep"; cli = @($F, 'startwith:rep'); fixture = 2 }
            @{ name = 'Anywhere'; mode = 2; text = "$F para"; cli = @($F, 'para'); fixture = 3 }
            # parent: limits the search to the fixture, because -ww also
            # applies to a path term such as F\.
            @{ name = 'Whole words'; mode = 3; text = "parent:$($F)docs report"; cli = @('-ww', "parent:$($F)docs", 'report'); fixture = 1 }
            @{ name = 'Match case'; mode = 2; case = $true; text = "$F README"; cli = @('-case', $F, 'README'); fixture = 1 }
            @{ name = 'Match path'; mode = 2; path = $true; text = "$F upper"; cli = @('-p', $F, 'upper'); fixture = 2 }
            @{ name = 'Regular expression'; mode = 4; text = $regex; cli = @('-r', $regex); fixture = $csCount }
        )
        $matched = foreach ($case in $cases) {
            Set-MatchMode $window $case.mode
            Set-CheckBox $window $MatchCaseId ([bool]$case.case)
            Set-CheckBox $window $MatchPathId ([bool]$case.path)
            $answer = Search-For $window $case.text
            $cliArgs = $case.cli
            [ordered]@{
                case    = $case.name
                window  = Count-Of $answer
                gozler  = [long](& $gozler -get-result-count @cliArgs)
                fixture = $case.fixture
            }
        }
        Set-MatchMode $window 0
        Set-CheckBox $window $MatchCaseId $false
        Set-CheckBox $window $MatchPathId $false
        Check 'match modes and check boxes' (-not ($matched | Where-Object { $_.window -ne $_.gozler -or $_.gozler -ne $_.fixture })) $matched

        # Alt and each access key reach the match controls; a check box's
        # key also toggles it, and a second press puts it back.
        if (Is-InFront $window) {
            $reached = foreach ($control in @{ id = $MatchModeId; key = 'match.label' }, @{ id = $MatchCaseId; key = 'match.case' }, @{ id = $MatchPathId; key = 'match.path' }) {
                $box = Child-ById (Window-Element $window) $control.id
                $isBox = $control.id -ne $MatchModeId
                $before = $isBox ? [GozlerProbe.Ui]::ToggleState($box) : 0
                $letter = [uint16][char](AccessKeyOf (Caption $control.key)).ToUpperInvariant()
                # VK_MENU (Alt), then the letter.
                [void][GozlerProbe.Win]::PressKeys($window.MainWindowHandle, 0x12, $letter)
                Start-Sleep -Milliseconds 300
                $after = $isBox ? [GozlerProbe.Ui]::ToggleState($box) : 0
                $focused = [GozlerProbe.Win]::Focus($window.MainWindowHandle) -eq [IntPtr]$box.Hwnd
                if ($after -ne $before) {
                    [void][GozlerProbe.Win]::PressKeys($window.MainWindowHandle, 0x12, $letter)
                    Start-Sleep -Milliseconds 300
                }
                [ordered]@{ key = "Alt+$([char]$letter)"; focused = $focused; toggled = $after -ne $before; wanted_toggle = $isBox }
            }
            [void](Wait-Answer $window)
            Check 'Alt and an access key reach the match controls' (-not ($reached | Where-Object { -not $_.focused -or $_.toggled -ne $_.wanted_toggle })) $reached
        }
        else { Report 'Alt and an access key reach the match controls' 'not run' 'the window was not in front' }

        # Ten changes within one second: the rows stay while part 1 says
        # Searching…, and the last text's results end it (S2, T1, T2). The
        # search starts 150 ms after the last change and the fixture answers
        # within tens of milliseconds, so part 1 and the rows are read with
        # one message each to their window handles, which takes far less time
        # than a read through UI Automation. WM_GETTEXT gives a status bar's
        # first part.
        [void](Search-For $window "$F ext:cs")
        $statusHwnd = Control-Hwnd $window $StatusBarId
        $listHwnd = Control-Hwnd $window $ListId
        # LVM_GETITEMCOUNT
        $rowCount = { [long][GozlerProbe.Win]::SendMessageW($listHwnd, 0x1004, 0, 0) }
        $rowsBefore = & $rowCount
        $box = Child-ById (Window-Element $window) $SearchBoxId
        $word = 'paracetamol'
        $samples = [System.Collections.Generic.List[object]]::new()
        $clock = [Diagnostics.Stopwatch]::StartNew()
        for ($i = 1; $i -le 10; $i++) {
            [GozlerProbe.Ui]::SetValue($box, "$F $($word.Substring(0, $i))")
            $samples.Add([ordered]@{ ms = $clock.ElapsedMilliseconds; rows = & $rowCount })
            while ($clock.ElapsedMilliseconds -lt $i * 90) { Start-Sleep -Milliseconds 5 }
        }
        $typed = $clock.ElapsedMilliseconds
        $previous = $null
        do {
            $sample = [ordered]@{ ms = $clock.ElapsedMilliseconds; part = [GozlerProbe.Win]::ControlText($statusHwnd); rows = & $rowCount }
            # Only the samples where part 1 or the rows change, so the
            # detail stays short.
            $key = "$($sample.part)|$($sample.rows)"
            if ($key -ne $previous) { $samples.Add($sample) }
            $previous = $key
        } until (($sample.ms -gt $typed + 200 -and -not (Is-Searching $sample.part)) -or $sample.ms -gt $typed + 30000)
        $last = "$F $($word.Substring(0, 10))"
        $cliCount = [long](& $gozler -get-result-count $F "wordstart:$($word.Substring(0, 10))")
        $paths = @(Visible-Paths $window 20)
        $cli = @(& $gozler $F "wordstart:$($word.Substring(0, 10))" -n 20)
        $kept = @($samples | Where-Object { (Is-Searching $_.part) -and $_.rows -eq $rowsBefore })
        Check 'ten changes in one second' ($typed -lt 1000 -and $kept.Count -gt 0 -and (Count-Of $sample.part) -eq $cliCount -and
            ($paths -join '|') -eq ($cli -join '|')) ([ordered]@{
                last_text = $last; typed_ms = $typed; rows_before = $rowsBefore; samples = $samples; gozler = $cliCount; rows = $paths
            })

        # The bold parts show in the screenshot of Show-States; this checks
        # the row they are drawn on.
        $answer = Search-For $window "$F para 50"
        $paths = @(Visible-Paths $window 20)
        Check 'para 50 finds the Paracetamol file' (($paths -join '|') -eq (Join-Path $files 'docs\Paracetamol 500mg.pdf')) ([ordered]@{ status = $answer; rows = $paths })

        $answer = Search-For $window "$F zzzz"
        $message = Message-Text $window
        $none = (Caption 'status.results_other') -replace '\{0\}', '0'
        Check 'no results' ($answer -eq $none -and $message -eq (Caption 'list.no_results')) ([ordered]@{ status = $answer; message = $message })

        # View > Columns > Extension, and the header's menu (C15).
        [void](Search-For $window "$F ext:cs")
        Run-MenuItem $window (Caption 'column.extension')
        Start-Sleep -Milliseconds 300
        $shown = @(Header-Names $window)
        $columnMarks = @(Marked-Items $window 'command.columns')
        $headerMarks = $null
        $inFront = Is-InFront $window
        if ($inFront) {
            $listView = Child-ById (Window-Element $window) $ListId
            $header = [GozlerProbe.Ui]::HeaderOf($listView)
            $x = [int]($header.Rect[0] + 20); $y = [int]($header.Rect[1] + $header.Rect[3] / 2)
            # WM_CONTEXTMENU, as a right-click on the header sends it.
            [void][GozlerProbe.Win]::PostMessageW($window.MainWindowHandle, 0x007B, [IntPtr]$listView.Hwnd, [IntPtr]((($y -band 0xFFFF) -shl 16) -bor ($x -band 0xFFFF)))
            $popup = Wait-Popup $window @()
            if ($popup) {
                # MN_GETHMENU
                $menu = [GozlerProbe.Win]::SendMessageW($popup, 0x01E1, 0, 0)
                $headerMarks = @([GozlerProbe.Win]::MenuItems($menu) | Where-Object Checked | ForEach-Object { Plain $_.Text })
                Save-ScreenArea $window.MainWindowHandle @($popup) 'header-menu-en'
                # VK_ESCAPE
                [void][GozlerProbe.Win]::PressKeys($window.MainWindowHandle, 0x1B)
                Start-Sleep -Milliseconds 300
            }
        }
        Run-MenuItem $window (Caption 'column.extension')
        Start-Sleep -Milliseconds 300
        $hidden = @(Header-Names $window)
        $expectedMarks = @('name', 'path', 'size', 'date_modified', 'extension' | ForEach-Object { Plain (Caption "column.$_") })
        $detail = [ordered]@{ shown = $shown; hidden = $hidden; columns_marks = $columnMarks; header_menu_marks = $headerMarks }
        $columnsPassed = $shown[-1] -eq (Caption 'header.extension') -and $hidden.Count -eq 4 -and
            ($columnMarks -join '|') -eq ($expectedMarks -join '|')
        if (-not $inFront) {
            Report 'View > Columns and the header menu' ($columnsPassed ? 'not run' : 'failed') ([ordered]@{ note = 'the header menu was not opened, because the window was not in front'; detail = $detail })
        }
        else { Check 'View > Columns and the header menu' ($columnsPassed -and $null -ne $headerMarks -and ($headerMarks -join '|') -eq ($expectedMarks -join '|')) $detail }

        # Find focuses the search box and selects its text (C17).
        if (Is-InFront $window) {
            [GozlerProbe.Ui]::Focus((Child-ById (Window-Element $window) $ListId))
            Run-MenuItem $window (Caption 'command.find')
            Start-Sleep -Milliseconds 300
            $searchHwnd = Control-Hwnd $window $SearchBoxId
            # EM_GETSEL
            $selection = [long][GozlerProbe.Win]::SendMessageW($searchHwnd, 0x00B0, 0, 0)
            $length = [GozlerProbe.Ui]::Value((Child-ById (Window-Element $window) $SearchBoxId)).Length
            $detail = [ordered]@{
                focused = [GozlerProbe.Win]::Focus($window.MainWindowHandle) -eq $searchHwnd
                start   = $selection -band 0xFFFF
                end     = ($selection -shr 16) -band 0xFFFF
                length  = $length
            }
            Check 'Find' ($detail.focused -and $detail.start -eq 0 -and $detail.end -eq $length) $detail
        }
        else { Report 'Find' 'not run' 'the window was not in front' }

        # Refresh shows a file that was added after the search (C18).
        $added = Join-Path $files 'src\Added.cs'
        [System.IO.File]::WriteAllBytes($added, [byte[]]::new(1))
        $deadline = [DateTime]::UtcNow.AddMinutes(1)
        while ([long](& $gozler -get-result-count $F ext:cs) -ne $csCount + 1 -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 500 }
        $stale = Status-Part $window 0
        Run-MenuItem $window (Caption 'command.refresh')
        Start-Sleep -Milliseconds 300
        $answer = Wait-Answer $window
        Check 'Refresh' ((Count-Of $stale) -eq $csCount -and (Count-Of $answer) -eq $csCount + 1) ([ordered]@{ before = $stale; after = $answer })
        Remove-Item -LiteralPath $added
        $deadline = [DateTime]::UtcNow.AddMinutes(1)
        while ([long](& $gozler -get-result-count $F ext:cs) -ne $csCount -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 500 }

        # Help > Search syntax opens and Close closes it (C22).
        $syntax = Show-States $window 'en' $menuIds
        Check 'Search syntax' ($syntax.title -eq (Caption 'search_syntax.title') -and $syntax.edit -like '*ext:*' -and
            $syntax.focus_on_close -and $syntax.esc_closed -and $syntax.closed) ([ordered]@{
                title = $syntax.title; text_length = "$($syntax.edit)".Length; focus_on_close = $syntax.focus_on_close
                esc_closed = $syntax.esc_closed; closed = $syntax.closed
            })

        # --- The selection and the actions ----------------------------------
        $listHwnd = Control-Hwnd $window $ListId
        $label = @{}
        foreach ($key in 'open', 'open_file_location', 'properties', 'copy_as_path', 'copy_name', 'export_results', 'cancel_export') {
            $label[$key] = Plain (Caption "command.$key")
        }
        $running = (Caption 'status.exporting_other').Split('{')[0]

        # One, two and 16 selected rows enable Open, Open file location and
        # Properties as the spec says. The probe reads the menus and runs none
        # of the commands (C1, C2, C3).
        [void](Search-For $window $F)
        $counted = foreach ($n in 1, 2, 16) {
            Select-Rows $window (0..($n - 1))
            Start-Sleep -Milliseconds 300
            $file = Item-States $window 0
            [ordered]@{
                selected = $n; status = Status-Part $window 1
                open = $file[$label.open]; open_file_location = $file[$label.open_file_location]; properties = $file[$label.properties]
            }
        }
        Save-Screenshot $window.MainWindowHandle 'selection-en'
        $wanted = @{ 1 = 'True True True'; 2 = 'True True False'; 16 = 'False False False' }
        Check 'commands follow the number selected' (-not ($counted | Where-Object {
                    "$($_.open) $($_.open_file_location) $($_.properties)" -ne $wanted[$_.selected] -or
                    $_.status -notmatch (Caption-Pattern 'status.selected' "$($_.selected)") })) $counted

        # A selection with a row that has left the cache is read from the
        # service: part 2 says so and the selection commands wait (S12). A row
        # of the second chunk, then the last row, are shown, so the first
        # chunk is the one used least recently and leaves the cache.
        Select-Rows $window @(0)
        Start-Sleep -Milliseconds 300
        $rows = Row-Count $window
        foreach ($row in 20000, ($rows - 1)) {
            # LVM_ENSUREVISIBLE
            [void][GozlerProbe.Win]::SendMessageW($listHwnd, 0x1013, [IntPtr]$row, 0)
            Start-Sleep -Milliseconds 1500
        }
        [GozlerProbe.Win]::SetItemState($listHwnd, $rows - 1, 2, 2)
        $reading = Status-Part $window 1
        $edit = Item-States $window 1
        $deadline = [DateTime]::UtcNow.AddSeconds(10)
        do {
            Start-Sleep -Milliseconds 100
            $read = Status-Part $window 1
        } until ($read -ne (Caption 'status.reading') -or [DateTime]::UtcNow -gt $deadline)
        $after = Item-States $window 1
        Save-Screenshot $window.MainWindowHandle 'selection-read-en'
        Check 'a selection outside the cache is read' ($reading -eq (Caption 'status.reading') -and
            -not $edit[$label.copy_as_path] -and -not $edit[$label.copy_name] -and
            $read -match (Caption-Pattern 'status.selected' '2') -and $after[$label.copy_as_path] -and $after[$label.copy_name]) ([ordered]@{
                rows = $rows; while_reading = $reading; copy_as_path_while_reading = $edit[$label.copy_as_path]
                copy_name_while_reading = $edit[$label.copy_name]; after = $read; copy_as_path_after = $after[$label.copy_as_path]
            })

        # Edit > Select all selects the search box's text when it has the
        # focus, and every row when the list has it (C11).
        if (Is-InFront $window) {
            $searchElement = Child-ById (Window-Element $window) $SearchBoxId
            [GozlerProbe.Ui]::Focus($searchElement)
            Run-MenuItem $window (Caption 'command.select_all')
            Start-Sleep -Milliseconds 300
            # EM_GETSEL
            $selection = [long][GozlerProbe.Win]::SendMessageW((Control-Hwnd $window $SearchBoxId), 0x00B0, 0, 0)
            $length = [GozlerProbe.Ui]::Value($searchElement).Length
            [GozlerProbe.Ui]::Focus((Child-ById (Window-Element $window) $ListId))
            Run-MenuItem $window (Caption 'command.select_all')
            Start-Sleep -Milliseconds 300
            $deadline = [DateTime]::UtcNow.AddSeconds(10)
            do {
                Start-Sleep -Milliseconds 100
                $read = Status-Part $window 1
            } until ($read -ne (Caption 'status.reading') -or [DateTime]::UtcNow -gt $deadline)
            $detail = [ordered]@{
                text_start = $selection -band 0xFFFF; text_end = ($selection -shr 16) -band 0xFFFF; text_length = $length
                # LVM_GETSELECTEDCOUNT
                rows = $rows; selected_rows = [long][GozlerProbe.Win]::SendMessageW($listHwnd, 0x1032, 0, 0); status = $read
            }
            $statusRows = ($read -match (Caption-Pattern 'status.selected' '([\d.,\s]+)')) ? [long]($Matches[1] -replace '\D', '') : $null
            Check 'Select all' ($detail.text_start -eq 0 -and $detail.text_end -eq $length -and $detail.selected_rows -eq $rows -and
                $statusRows -eq $rows) $detail
        }
        else { Report 'Select all' 'not run' 'the window was not in front, so the probe did not move the focus' }

        # Shift+F10 opens the row's context menu, with Open as its default
        # item (spec: Results list).
        if (Is-InFront $window) {
            Select-Rows $window @(0)
            # LVIS_FOCUSED | LVIS_SELECTED
            [GozlerProbe.Win]::SetItemState($listHwnd, 0, 3, 3)
            [GozlerProbe.Ui]::Focus((Child-ById (Window-Element $window) $ListId))
            # The first row can be out of the cache, so the menu waits until
            # the selection is read.
            $deadline = [DateTime]::UtcNow.AddSeconds(10)
            do {
                Start-Sleep -Milliseconds 100
                $read = Status-Part $window 1
            } until ($read -match (Caption-Pattern 'status.selected' '1') -or [DateTime]::UtcNow -gt $deadline)
            # VK_SHIFT, VK_F10
            $pressed = [GozlerProbe.Win]::PressKeys($window.MainWindowHandle, 0x10, 0x79)
            $popup = $pressed ? (Wait-Popup $window @()) : $null
            $items = $null
            if ($popup) {
                # MN_GETHMENU
                $menu = [GozlerProbe.Win]::SendMessageW($popup, 0x01E1, 0, 0)
                $items = @([GozlerProbe.Win]::MenuItems($menu) | ForEach-Object {
                        $_.Separator ? '-' : "$(Plain $_.Text) | $($_.Enabled ? 'on' : 'off')$($_.Default ? ' | default' : '')"
                    })
                Save-ScreenArea $window.MainWindowHandle @($popup) 'row-menu-en'
                # VK_ESCAPE
                [void][GozlerProbe.Win]::PressKeys($window.MainWindowHandle, 0x1B)
                Start-Sleep -Milliseconds 300
            }
            $expectedItems = @(
                "$($label.open) | on | default", "$($label.open_file_location) | on", '-',
                "$(Plain (Caption 'command.copy')) | on", "$($label.copy_as_path) | on", "$($label.copy_name) | on", '-',
                "$(Plain (Caption 'command.search_in_this_folder')) | on", '-', "$($label.properties) | on"
            )
            if ($pressed) { Check 'Shift+F10 opens the row menu' (($items -join '|') -eq ($expectedItems -join '|')) ([ordered]@{ items = $items; expected = $expectedItems }) }
            else { Report 'Shift+F10 opens the row menu' 'not run' 'the window left the front before the keys were pressed' }
        }
        else { Report 'Shift+F10 opens the row menu' 'not run' 'the window was not in front, so no key was pressed' }

        # The marquee selects rows in the virtual list: a drag from below the
        # last row to the first (design: Not verified).
        [void](Search-For $window "$F ext:cs")
        Select-Rows $window @()
        if (Is-InFront $window) {
            $listElement = Child-ById (Window-Element $window) $ListId
            $rowElements = @([GozlerProbe.Ui]::Children($listElement) | Where-Object { $_.ControlType -in [GozlerProbe.Ui]::ListItem, [GozlerProbe.Ui]::DataItem })
            $first = $rowElements[0].Rect; $last = $rowElements[-1].Rect
            $x = [int]($first[0] + 60)
            $dragged = [GozlerProbe.Win]::Drag($window.MainWindowHandle, $x, [int]($last[1] + $last[3] + 40), $x + 40, [int]($first[1] + $first[3] / 2))
            Start-Sleep -Milliseconds 500
            $detail = [ordered]@{ rows = $rowElements.Count; selected_rows = [long][GozlerProbe.Win]::SendMessageW($listHwnd, 0x1032, 0, 0); status = Status-Part $window 1 }
            if ($dragged) { Check 'the marquee selects rows' ($detail.selected_rows -eq $rowElements.Count -and $detail.status -match (Caption-Pattern 'status.selected' "$($rowElements.Count)")) $detail }
            else { Report 'the marquee selects rows' 'not run' 'the window left the front before the drag' }
            Select-Rows $window @()
        }
        else { Report 'the marquee selects rows' 'not run' 'the window was not in front, so the mouse was not moved' }

        # Export results… writes the bytes gozler writes with the switches
        # of the shown columns, and part 3 says how many rows (C4, J6).
        $extension = Caption 'column.extension'; $attributes = Caption 'column.attributes'
        $formats = @(
            @{ file = 'cs.csv'; switch = '-export-csv' }
            @{ file = 'cs.tsv'; switch = '-export-tsv' }
            @{ file = 'cs.txt'; switch = '-export-txt' }
            @{ file = 'cs-columns.csv'; switch = '-export-csv'; columns = $true }
        )
        $exported = foreach ($format in $formats) {
            $columns = @('-size', '-dm')
            if ($format.columns) {
                Run-MenuItem $window $extension
                Run-MenuItem $window $attributes
                Start-Sleep -Milliseconds 300
                $columns += '-ext', '-attributes'
            }
            $path = Join-Path $exports $format.file
            $closed = Export-To $window $path
            $status = $closed ? (Wait-Export $window) : $null
            $cliPath = Join-Path $exports "gozler-$($format.file)"
            & $gozler $F ext:cs $format.switch $cliPath @columns
            [ordered]@{
                file = $format.file; dialog_closed = $closed; status = $status; same_bytes = Same-Bytes $path $cliPath
                bytes = (Test-Path -LiteralPath $path) ? (Get-Item -LiteralPath $path).Length : $null
                gozler_bytes = (Test-Path -LiteralPath $cliPath) ? (Get-Item -LiteralPath $cliPath).Length : $null
                gozler_switches = "$($format.switch) FILE $($columns -join ' ')"
            }
        }
        Save-Screenshot $window.MainWindowHandle 'exported-en'
        Run-MenuItem $window $extension
        Run-MenuItem $window $attributes
        Check 'Export results writes what gozler writes' (-not ($exported | Where-Object {
                    -not $_.same_bytes -or $_.status -notmatch (Caption-Pattern 'status.exported_other' "$csCount" ([regex]::Escape($_.file))) })) $exported

        # During an export of the empty search, part 3 shows its progress and
        # File shows Cancel export in place of Export results…; Cancel export
        # ends it and deletes its file (C5, S11).
        [void](Search-For $window '')
        $all = Join-Path $exports 'all.csv'
        $closed = Export-To $window $all
        Start-Sleep -Milliseconds 1000
        $progress = Status-Part $window 2
        $file = Item-States $window 0
        Save-Screenshot $window.MainWindowHandle 'exporting-en'
        $existed = Test-Path -LiteralPath $all
        Run-MenuItem $window (Caption 'command.cancel_export')
        for ($i = 0; $i -lt 20 -and (Test-Path -LiteralPath $all); $i++) { Start-Sleep -Milliseconds 100 }
        $detail = [ordered]@{
            dialog_closed = $closed; progress = $progress; file_menu = @($file.Keys); cancel_export_enabled = $file[$label.cancel_export]
            file_while_running = $existed; file_after = Test-Path -LiteralPath $all; status_after = Status-Part $window 2
        }
        Check 'Cancel export' ($closed -and $progress -match (Caption-Pattern 'status.exporting_other') -and $file[$label.cancel_export] -and
            -not $file.Contains($label.export_results) -and $existed -and -not $detail.file_after -and -not $detail.status_after) $detail

        # A search runs when an export of the empty search ends: part 3 shows
        # the export's result while part 1 says Searching…, and the export
        # ran on through every search's start (T1, T8). Each change of the
        # sort direction starts a search of the empty search. The probe times
        # one, then changes the direction three times as often during the
        # export, so a search runs at every moment of it on any machine.
        $directions = @((Menu-Id $window (Caption 'sort.descending')), (Menu-Id $window (Caption 'sort.ascending')))
        $clock = [Diagnostics.Stopwatch]::StartNew()
        [void][GozlerProbe.Win]::PostMessageW($window.MainWindowHandle, 0x0111, [IntPtr]$directions[0], 0)
        while (-not (Is-Searching (Status-Part $window 0)) -and $clock.ElapsedMilliseconds -lt 2000) { }
        [void](Wait-Answer $window)
        $searchMs = $clock.ElapsedMilliseconds
        [void][GozlerProbe.Win]::PostMessageW($window.MainWindowHandle, 0x0111, [IntPtr]$directions[1], 0)
        Start-Sleep -Milliseconds 300
        [void](Wait-Answer $window)
        $interval = [Math]::Max(10, [int]($searchMs / 3))
        $all = Join-Path $exports 'all-sorted.csv'
        $closed = Export-To $window $all
        $clock.Restart()
        while (-not "$(Status-Part $window 2)".StartsWith($running) -and $clock.ElapsedMilliseconds -lt 5000) { }
        $samples = [System.Collections.Generic.List[object]]::new()
        $previous = $null
        $changes = 0
        $nextChange = 0
        do {
            if ($clock.ElapsedMilliseconds -ge $nextChange) {
                [void][GozlerProbe.Win]::PostMessageW($window.MainWindowHandle, 0x0111, [IntPtr]$directions[$changes % 2], 0)
                $changes++
                $nextChange = $clock.ElapsedMilliseconds + $interval
            }
            $sample = [ordered]@{ ms = $clock.ElapsedMilliseconds; part1 = Status-Part $window 0; part3 = Status-Part $window 2 }
            $key = "$($sample.part1)|$($sample.part3)"
            if ($key -ne $previous) { $samples.Add($sample) }
            $previous = $key
        } until (-not "$($sample.part3)".StartsWith($running) -or $clock.ElapsedMilliseconds -gt 120000)
        [void][GozlerProbe.Win]::PostMessageW($window.MainWindowHandle, 0x0111, [IntPtr]$directions[1], 0)
        Start-Sleep -Milliseconds 300
        [void](Wait-Answer $window)
        $exportedPattern = Caption-Pattern 'status.exported_other' '.+' ([regex]::Escape('all-sorted.csv'))
        $overlap = @($samples | Where-Object { (Is-Searching $_.part1) -and $_.part3 -match $exportedPattern })
        $detail = [ordered]@{ dialog_closed = $closed; search_ms = $searchMs; interval_ms = $interval; sort_changes = $changes; samples = $samples }
        Check 'an export ends while a search runs' ($closed -and $changes -gt 1 -and $overlap.Count -gt 0) $detail
        if (Test-Path -LiteralPath $all) { Remove-Item -LiteralPath $all }

        # A selection of every row of the empty search holds its files while
        # it lasts, and no message waits more than 100 ms meanwhile (spec:
        # Memory budget). The probe selects every row as Select all does,
        # and asks the window for WM_NULL every 50 ms until part 2 shows the
        # files; it reports the memory, which the spec estimates.
        $rows = Row-Count $window
        $before = Memory-Of $window
        [GozlerProbe.Win]::SetItemState($listHwnd, -1, 2, 2)
        $longest = 0
        $selected = $null
        $deadline = [DateTime]::UtcNow.AddSeconds(30)
        do {
            $longest = [Math]::Max($longest, [GozlerProbe.Win]::AnswerMs($window.MainWindowHandle, 5000))
            Start-Sleep -Milliseconds 50
            $part = Status-Part $window 1
            if ($part -match (Caption-Pattern 'status.selected' '([\d.,\s]+)')) { $selected = [long]($Matches[1] -replace '\D', '') }
        } until ($selected -or [DateTime]::UtcNow -gt $deadline)
        $held = Memory-Of $window
        Select-Rows $window @()
        Start-Sleep -Milliseconds 500
        $detail = [ordered]@{
            rows = $rows; status = $part; longest_wait_ms = $longest; before = $before; while_held = $held; after = Memory-Of $window
        }
        Check 'a selection of every row' ($rows -eq 100000 -and $selected -eq $rows -and $longest -le 100) $detail

        # An export into the folder whose ACL denies creating files shows the
        # spec's message box and leaves no file (T9).
        [void](Search-For $window "$F ext:cs")
        $denied = Join-Path $files 'denied\export.csv'
        $closed = Export-To $window $denied
        $texts = $closed ? (Close-MessageBox $window 'export-failed-en') : $null
        $prefix = ((Caption 'message.export_failed') -split '\{1\}')[0].Replace('{0}', $denied)
        Check 'export into a denied folder' ($closed -and ($texts | Where-Object { "$_".StartsWith($prefix) }) -and -not (Test-Path -LiteralPath $denied)) ([ordered]@{
                dialog_closed = $closed; message = $texts; expected_start = $prefix; file_exists = Test-Path -LiteralPath $denied
            })

        # --- The filter pane ------------------------------------------------
        # Search in this folder on a file directly in the fixture folder
        # sets Location to it (C12).
        $searchHere = Command-Id 'SearchInThisFolder'
        [void](Search-For $window "$F large")
        Select-Rows $window @(0)
        Start-Sleep -Milliseconds 500
        [void][GozlerProbe.Win]::SendMessageW($window.MainWindowHandle, 0x0111, [IntPtr]$searchHere, 0)
        [void](Wait-Answer $window)
        $location = Selected-Location $window
        Save-Screenshot $window.MainWindowHandle 'search-here-en'
        Check 'Search in this folder' ($location -eq $files) ([ordered]@{ location = $location; expected = $files })

        # Each filter gives the fixture's own count and the count of the
        # gozler query it stands for (J3, J4).
        $entries = @(Expected-Rows $files | ForEach-Object {
                $size, $date, $attributes, $path = $_.Split(',', 4)
                [pscustomobject]@{ size = [long]$size; date = [datetime]$date; attributes = [int]$attributes; path = $path.Trim('"') }
            })
        # The spec's Documents list.
        $documents = 'pdf;doc;docx;xls;xlsx;ppt;pptx;odt;ods;odp;rtf;txt;md;csv'
        $documentList = $documents.Split(';')
        $weekAgo = (Get-Date).AddDays(-7)
        $isFile = { -not ($_.attributes -band 16) }
        $filterCases = @(
            @{ name = 'no filter'; set = {}; reset = {}; cli = @('-path', $files); fixture = $entries.Count }
            @{
                name = 'Kind Documents'; set = { Select-PaneItem $window $ids.IDC_KIND 5 $false }; reset = { Select-PaneItem $window $ids.IDC_KIND 0 $false }
                cli = @('-path', $files, "ext:$documents")
                fixture = @($entries | Where-Object $isFile | Where-Object { [System.IO.Path]::GetExtension($_.path).TrimStart('.').ToLowerInvariant() -in $documentList }).Count
            }
            @{
                name = 'Only directly in this folder'; set = { [GozlerProbe.Ui]::Toggle((Pane-Child $window $ids.IDC_ONLY_DIRECTLY)) }
                reset = { [GozlerProbe.Ui]::Toggle((Pane-Child $window $ids.IDC_ONLY_DIRECTLY)) }
                cli = @('-parent', $files); fixture = @($entries | Where-Object { (Split-Path $_.path) -eq $files }).Count
            }
            @{
                name = 'Last 7 days'; set = { Select-PaneItem $window $ids.IDC_DATE 3 $true }; reset = { Select-PaneItem $window $ids.IDC_DATE 0 $true }
                cli = @('-path', $files, 'dm:7days'); fixture = @($entries | Where-Object { $_.date -gt $weekAgo }).Count
            }
            @{
                name = 'Large'; set = { Select-PaneItem $window $ids.IDC_SIZE 5 $true }; reset = { Select-PaneItem $window $ids.IDC_SIZE 0 $true }
                cli = @('-path', $files, 'size:large'); fixture = @($entries | Where-Object { $_.size -gt 1MB -and $_.size -le 16MB }).Count
            }
            @{
                name = 'Hidden Exclude'; set = { Select-PaneItem $window $ids.IDC_HIDDEN 2 $true }; reset = { Select-PaneItem $window $ids.IDC_HIDDEN 0 $true }
                cli = @('-path', $files, '!attrib:h'); fixture = @($entries | Where-Object { -not ($_.attributes -band 2) }).Count
            }
        )
        [void](Search-For $window '')
        $filtered = foreach ($case in $filterCases) {
            & $case.set
            $answer = Wait-Answer $window
            $cliArgs = $case.cli
            [ordered]@{ case = $case.name; window = Count-Of $answer; gozler = [long](& $gozler -get-result-count @cliArgs); fixture = $case.fixture }
            & $case.reset
            [void](Wait-Answer $window)
        }
        Check 'each filter against gozler and the fixture' (-not ($filtered | Where-Object { $_.window -ne $_.gozler -or $_.gozler -ne $_.fixture })) $filtered

        # With a filter set and no match, the message offers Clear filters
        # and Search everywhere, and each does what it says; so does the
        # pane's Clear filters (C21, S5).
        $clearCaption = Plain (Caption 'command.clear_filters')
        $everywhereCaption = Plain (Caption 'list.search_everywhere')
        Select-PaneItem $window $ids.IDC_KIND 5 $false
        [void](Search-For $window "$F zzzz")
        Save-Screenshot $window.MainWindowHandle 'no-results-filtered-en'
        $offered = @(Message-Buttons $window)
        [GozlerProbe.Ui]::Invoke((Child-ById (Window-Element $window) $ids.IDC_MESSAGE_BUTTON_1))
        Start-Sleep -Milliseconds 300
        [void](Wait-Answer $window)
        $cleared = [ordered]@{ kind = Pane-Selected $window $ids.IDC_KIND $false; location = Selected-Location $window; buttons = @(Message-Buttons $window) }
        Set-Location $window $files
        [void](Wait-Answer $window)
        $locationOffered = @(Message-Buttons $window)
        [GozlerProbe.Ui]::Invoke((Child-ById (Window-Element $window) $ids.IDC_MESSAGE_BUTTON_2))
        Start-Sleep -Milliseconds 300
        [void](Wait-Answer $window)
        $everywhere = [ordered]@{ location = Selected-Location $window; buttons = @(Message-Buttons $window) }
        Select-PaneItem $window $ids.IDC_KIND 5 $false
        [void](Wait-Answer $window)
        [GozlerProbe.Ui]::Invoke((Pane-Child $window $ids.IDC_CLEAR_FILTERS))
        Start-Sleep -Milliseconds 300
        [void](Wait-Answer $window)
        $paneCleared = [ordered]@{ kind = Pane-Selected $window $ids.IDC_KIND $false; clear_enabled = (Pane-Child $window $ids.IDC_CLEAR_FILTERS).Enabled }
        $everywhereItem = Caption 'filter.everywhere'
        Check 'no-results buttons and Clear filters' (($offered -join '|') -eq "$clearCaption|$everywhereCaption" -and
            $cleared.kind -eq 0 -and $cleared.location -eq $everywhereItem -and $cleared.buttons.Count -eq 0 -and
            ($locationOffered -join '|') -eq "$clearCaption|$everywhereCaption" -and
            $everywhere.location -eq $everywhereItem -and $everywhere.buttons.Count -eq 0 -and
            $paneCleared.kind -eq 0 -and -not $paneCleared.clear_enabled) ([ordered]@{
                offered = $offered; after_clear = $cleared; with_location = $locationOffered; after_search_everywhere = $everywhere; after_pane_clear = $paneCleared
            })

        # View > Filters hides the pane, and with a filter set part 1 adds
        # "· Filtered" (C13, S10).
        $paneHwnd = Control-Hwnd $window $ids.IDC_FILTER_PANE
        Select-PaneItem $window $ids.IDC_KIND 5 $false
        [void](Search-For $window "$F ext:pdf")
        Run-MenuItem $window (Caption 'command.filters')
        Start-Sleep -Milliseconds 300
        $hidden = [ordered]@{ pane_visible = [GozlerProbe.Win]::IsWindowVisible($paneHwnd); status = Status-Part $window 0 }
        Save-Screenshot $window.MainWindowHandle 'filters-hidden-en'
        Run-MenuItem $window (Caption 'command.filters')
        Start-Sleep -Milliseconds 300
        $shown = [ordered]@{ pane_visible = [GozlerProbe.Win]::IsWindowVisible($paneHwnd); status = Status-Part $window 0 }
        Select-PaneItem $window $ids.IDC_KIND 0 $false
        [void](Wait-Answer $window)
        $filteredPattern = Caption-Pattern 'status.filtered'
        Check 'View > Filters and Filtered' (-not $hidden.pane_visible -and $hidden.status -match $filteredPattern -and
            $shown.pane_visible -and $shown.status -notmatch $filteredPattern) ([ordered]@{ hidden = $hidden; shown = $shown })

        # Custom… reached with the arrow keys opens nothing and keeps the
        # previous filter; Enter opens the dialog, a Minimum above the
        # Maximum shows the spec's balloon and keeps the dialog open, and
        # Cancel shows the previous choice again.
        [void](Search-For $window '')
        Select-PaneItem $window $ids.IDC_SIZE 5 $true
        $large = Wait-Answer $window
        $openedByArrow, $dialog = Open-Custom $window $ids.IDC_SIZE
        $afterArrow = Status-Part $window 0
        $balloon = $null
        $stayedOpen = $false
        $closed = $false
        if ($dialog) {
            Start-Sleep -Milliseconds 300
            [GozlerProbe.Win]::SetControlText([GozlerProbe.Win]::ChildById($dialog, [int]$ids.IDC_CUSTOM_SIZE_MINIMUM, 'Edit'), '2')
            [GozlerProbe.Win]::SetControlText([GozlerProbe.Win]::ChildById($dialog, [int]$ids.IDC_CUSTOM_SIZE_MAXIMUM, 'Edit'), '1')
            [GozlerProbe.Ui]::InvokeLater([GozlerProbe.Ui]::FromHandle([GozlerProbe.Win]::ChildById($dialog, 1, 'Button')))
            $tip = $null
            for ($i = 0; $i -lt 30 -and -not $tip; $i++) {
                Start-Sleep -Milliseconds 100
                $tip = [GozlerProbe.Win]::Windows($window.Id, 'tooltips_class32') | Select-Object -First 1
            }
            if ($tip) {
                $balloon = [GozlerProbe.Ui]::FromHandle($tip).Name
                Save-ScreenArea $dialog @($tip) 'custom-size-balloon-en'
            }
            $stayedOpen = [GozlerProbe.Win]::IsWindow($dialog)
            $closed = Close-Dialog $dialog 2
        }
        Start-Sleep -Milliseconds 300
        $afterCancel = Pane-Selected $window $ids.IDC_SIZE $true
        Select-PaneItem $window $ids.IDC_SIZE 0 $true
        [void](Wait-Answer $window)
        # Custom date opens the same way; its screenshot shows it.
        $null, $dateDialog = Open-Custom $window $ids.IDC_DATE
        $dateTitle = $null
        if ($dateDialog) {
            Start-Sleep -Milliseconds 300
            $dateTitle = [GozlerProbe.Win]::Text($dateDialog)
            Save-Screenshot $dateDialog 'custom-date-en'
            [void](Close-Dialog $dateDialog 2)
        }
        $dateAfter = Pane-Selected $window $ids.IDC_DATE $true
        Check 'Custom… and its dialogs' (-not $openedByArrow -and (Count-Of $afterArrow) -eq (Count-Of $large) -and $dialog -and
            $balloon -eq (Caption 'custom_size.order') -and $stayedOpen -and $closed -and $afterCancel -eq 5 -and
            $dateTitle -eq (Caption 'custom_date.title') -and $dateAfter -eq 0) ([ordered]@{
                large = $large; opened_by_arrow = $openedByArrow; after_arrow = $afterArrow; dialog_opened = [bool]$dialog; balloon = $balloon
                stayed_open = $stayedOpen; cancelled = $closed; size_after_cancel = $afterCancel; date_dialog = $dateTitle; date_after_cancel = $dateAfter
            })

        # Choose folder… then Cancel keeps the previous Location.
        Set-Location $window $files
        [void](Wait-Answer $window)
        [GozlerProbe.Ui]::InvokeLater((Pane-Child $window $ids.IDC_CHOOSE_FOLDER))
        $picker = Wait-Dialog $window
        $pickerClosed = $picker ? (Close-Dialog $picker 2) : $false
        Start-Sleep -Milliseconds 300
        $kept = Selected-Location $window
        Check 'Choose folder then Cancel' ($picker -and $pickerClosed -and $kept -eq $files) ([ordered]@{ picker_opened = [bool]$picker; picker_closed = $pickerClosed; location = $kept })

        # The filter choices are the same after a restart; the window saves
        # them in the probe's data folder.
        Select-PaneItem $window $ids.IDC_KIND 5 $false
        [GozlerProbe.Ui]::Toggle((Pane-Child $window $ids.IDC_ONLY_DIRECTLY))
        Select-PaneItem $window $ids.IDC_SIZE 5 $true
        Select-PaneItem $window $ids.IDC_HIDDEN 2 $true
        [void](Wait-Answer $window)
        $readChoices = {
            [ordered]@{
                kind = Pane-Selected $window $ids.IDC_KIND $false; location = Selected-Location $window
                only_directly = [GozlerProbe.Ui]::ToggleState((Pane-Child $window $ids.IDC_ONLY_DIRECTLY))
                date = Pane-Selected $window $ids.IDC_DATE $true; size = Pane-Selected $window $ids.IDC_SIZE $true
                hidden = Pane-Selected $window $ids.IDC_HIDDEN $true
            }
        }
        $before = & $readChoices
        [void](Close-Window $window)
        $window = Start-Window
        [void](Wait-Answer $window)
        $after = & $readChoices
        Check 'filter choices after a restart' (($before | ConvertTo-Json -Compress) -eq ($after | ConvertTo-Json -Compress) -and
            $before.kind -eq 5 -and $before.location -eq $files) ([ordered]@{ before = $before; after = $after })
        [GozlerProbe.Ui]::Invoke((Pane-Child $window $ids.IDC_CLEAR_FILTERS))
        Start-Sleep -Milliseconds 300
        [void](Wait-Answer $window)

        # A Location folder deleted after it was chosen: F5 shows the
        # message with Choose folder… and Search everywhere, and part 1 never
        # says Searching… (S8). Status part 1 is read with one message to its
        # handle each time, which is fast enough to see a search start.
        $missing = Join-Path $files 'to-delete'
        [void](Search-For $window "$F gone")
        Select-Rows $window @(0)
        Start-Sleep -Milliseconds 500
        [void][GozlerProbe.Win]::SendMessageW($window.MainWindowHandle, 0x0111, [IntPtr]$searchHere, 0)
        [void](Wait-Answer $window)
        $chosen = Selected-Location $window
        Remove-Item -LiteralPath $missing -Recurse -Force
        $statusHwnd = Control-Hwnd $window $StatusBarId
        Run-MenuItem $window (Caption 'command.refresh')
        $parts = [System.Collections.Generic.List[string]]::new()
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($clock.ElapsedMilliseconds -lt 1000) {
            $part = [GozlerProbe.Win]::ControlText($statusHwnd)
            if ($parts.Count -eq 0 -or $parts[-1] -ne $part) { $parts.Add($part) }
        }
        $message = Message-Text $window
        $offered = @(Message-Buttons $window)
        Save-Screenshot $window.MainWindowHandle 'missing-folder-en'
        $expectedMessage = (Caption 'list.missing_folder').Replace('{0}', $missing)
        Check 'a missing Location folder' ($chosen -eq $missing -and $message -eq $expectedMessage -and
            ($offered -join '|') -eq "$(Plain (Caption 'filter.choose_folder'))|$everywhereCaption" -and
            -not ($parts | Where-Object { Is-Searching $_ })) ([ordered]@{
                location = $chosen; message = $message; buttons = $offered; part_1 = $parts
            })
        [GozlerProbe.Ui]::Invoke((Child-ById (Window-Element $window) $ids.IDC_MESSAGE_BUTTON_2))
        Start-Sleep -Milliseconds 300
        [void](Wait-Answer $window)
    }

    # The parser's message (S7), and the daemon's refusal (S9).
    $answer = Search-For $window 'dz:x'
    $message = Message-Text $window
    Check 'dz:x shows the parser message' ($message -eq ((Caption 'query.unknown_function') -replace '\{0\}', 'dz:') -and (Row-Count $window) -eq 0) ([ordered]@{
            status = $answer; message = $message; note = 'that no status poll starts is pinned by a core test; the probe cannot see the poll'
        })
    $answer = Search-For $window 'regex:('
    $message = Message-Text $window
    $refused = ((Caption 'failure.refused') -split "`n`n")[0]
    Check 'regex:( shows the refusal' ("$message".StartsWith($refused) -and (Row-Count $window) -eq 0) ([ordered]@{ status = $answer; message = $message })

    # A typing mistake in a date, then its correction (J7).
    $answer = Search-For $window 'dm:yesterdy'
    $wrong = Message-Text $window
    $answer = Search-For $window 'dm:yesterday'
    $corrected = Message-Text $window
    Check 'dm:yesterdy, then dm:yesterday' ("$wrong" -like '*yesterdy*' -and $null -eq $corrected -and $null -ne (Count-Of $answer)) ([ordered]@{
            wrong = $wrong; corrected_status = $answer; corrected_message = $corrected
        })

    # Closing while the empty search runs (T14).
    [GozlerProbe.Ui]::SetValue((Child-ById (Window-Element $window) $SearchBoxId), '')
    Start-Sleep -Milliseconds 200
    $part = Status-Part $window 0
    $clock = [Diagnostics.Stopwatch]::StartNew()
    [void][GozlerProbe.Win]::PostMessageW($window.MainWindowHandle, $WM_CLOSE, 0, 0)
    $ended = $window.WaitForExit(10000)
    Check 'closing during a search' ($ended -and $clock.ElapsedMilliseconds -lt 1000) ([ordered]@{
            status_when_closed = $part; ended = $ended; milliseconds = $clock.ElapsedMilliseconds
        })

    # Closing 0.5 s into an export of the empty search ends the process
    # within two seconds and deletes the partial file (T14). The export takes
    # several seconds, so one that ignores its cancel token fails the check.
    Write-Settings $null
    $window = Start-Window
    [void](Wait-Answer $window)
    $closing = Join-Path $exports 'closing.csv'
    if (Export-To $window $closing) {
        Start-Sleep -Milliseconds 500
        $existed = Test-Path -LiteralPath $closing
        $clock = [Diagnostics.Stopwatch]::StartNew()
        [void][GozlerProbe.Win]::PostMessageW($window.MainWindowHandle, $WM_CLOSE, 0, 0)
        $ended = $window.WaitForExit(2000)
        Check 'closing during an export' ($ended -and -not (Test-Path -LiteralPath $closing)) ([ordered]@{
                ended = $ended; milliseconds = $clock.ElapsedMilliseconds; file_while_running = $existed; file_after = Test-Path -LiteralPath $closing
            })
    }
    else { Check 'closing during an export' $false 'the save dialog did not close' }
    [void](Close-Window $window)

    if ($Pseudo) {
        Write-Settings '{ "settings": { "language": "qps-ploc" } }'
        $window, $firstPart = Start-Searching 'pseudo'
        [void](Wait-Answer $window)
        Save-Screenshot $window.MainWindowHandle 'limit-pseudo'
        if ($filesListed) {
            [void](Show-States $window 'pseudo' $menuIds)
            # Show-States ends on the ext:cs search.
            Select-Rows $window 0, 1
            Start-Sleep -Milliseconds 500
            Save-Screenshot $window.MainWindowHandle 'selection-pseudo'
            Select-Rows $window @()
            if (Export-To $window (Join-Path $exports 'pseudo.csv')) {
                [void](Wait-Export $window)
                Start-Sleep -Milliseconds 300
                Save-Screenshot $window.MainWindowHandle 'exported-pseudo'
            }
        }
        [void](Close-Window $window)
    }
}
catch {
    # The error alone names only the line that threw, not the check.
    $results['probe stopped'] = "$_`n$($_.ScriptStackTrace)"
    throw
}
finally {
    foreach ($process in $started) {
        if (-not $process.HasExited -and -not (Close-Window $process)) {
            $process.Kill()
            Report "closed $($process.Id)" 'failed' 'the window did not close, so the probe ended its own process'
        }
    }
    if ($fixture -and (Test-Path -LiteralPath $fixture)) {
        Remove-Fixture $fixture
        $results['fixture removed'] = -not (Test-Path -LiteralPath $fixture)
    }
    $results['screenshots'] = $screenshotFiles
    $mutex.ReleaseMutex()
    $mutex.Dispose()
    $results | ConvertTo-Json -Depth 6
}
