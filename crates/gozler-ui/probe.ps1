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
through UI Automation, or presses a key, only while its own window is in
front, because keys go to the window in front; otherwise those checks
report "not run".

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
public interface IUIAutomationValuePattern { void SetValue(); [return: MarshalAs(UnmanagedType.BStr)] string get_CurrentValue(); }

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
        MenuItem = 50011, StatusBar = 50017, Text = 50020, Window = 50032, Header = 50034,
        HeaderItem = 50035, Separator = 50038;

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
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
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
        public bool Separator, Enabled, Checked, RightJustify;
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
                SubMenu = info.hSubMenu,
            });
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
# the submenu above it; after | come its shortcut, whether it is enabled in
# stage 2 with the search box focused and empty, and whether it is marked.
$menuSpec = @'
menu.file
  command.open||off|
  command.open_file_location|Ctrl+Enter|off|
  command.properties|Alt+Enter|off|
  -
  command.export_results||off|
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
  command.find|Ctrl+F|off|
menu.view
  command.filters||off|marked
  command.inspector||off|marked
  command.columns||on|
    column.name||off|marked
    column.path||off|marked
    column.size||off|marked
    column.date_modified||off|marked
    column.extension||off|
    column.attributes||off|
  -
  command.sort_by||on|
    sort.name||off|marked
    sort.path||off|
    sort.size||off|
    sort.date_modified||off|
    sort.extension||off|
    -
    sort.ascending||off|marked
    sort.descending||off|
  -
  command.refresh|F5|off|
menu.help
  command.search_syntax|F1|off|
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

function Is-InFront($process) {
    $front = [GozlerProbe.Win]::GetForegroundWindow()
    if ($front -eq $process.MainWindowHandle) { return $true }
    [void][GozlerProbe.Win]::SetForegroundWindow($process.MainWindowHandle)
    Start-Sleep -Milliseconds 200
    return [GozlerProbe.Win]::GetForegroundWindow() -eq $process.MainWindowHandle
}

# The element of `hwnd`. A menu window exists before UI Automation can read
# it, so a failed read is tried again.
function Element-Of($hwnd) {
    for ($i = 0; ; $i++) {
        try { return [GozlerProbe.Ui]::FromHandle($hwnd) }
        catch { if ($i -ge 20) { return $null } }
        Start-Sleep -Milliseconds 250
    }
}

# Opens the menu bar's menu at position `top` through UI Automation, and the
# submenu at position `sub` in it when `sub` is given, saves a screenshot of
# what is open, and closes it. Returns the names UI Automation reads in the
# last menu it opened. Runs only while the window is in front.
function Show-Menu($process, [int] $top, [int] $sub, [string] $name) {
    $item = @([GozlerProbe.Ui]::Children((Menu-Bar $process)))[$top]
    [GozlerProbe.Ui]::Expand($item)
    try {
        $menu = Wait-Popup $process @()
        if (-not $menu) { return [ordered]@{ opened = $false } }
        $popups = @($menu)
        if ($sub -ge 0) {
            $subItem = @([GozlerProbe.Ui]::Children((Element-Of $menu)))[$sub]
            [GozlerProbe.Ui]::Expand($subItem)
            $menu = Wait-Popup $process $popups
            if (-not $menu) { return [ordered]@{ opened = $false } }
            $popups += $menu
        }
        $element = Element-Of $menu
        Save-ScreenArea $process.MainWindowHandle $popups $name
        $items = if ($element) { @([GozlerProbe.Ui]::Children($element) | ForEach-Object Name) } else { @() }
        return [ordered]@{ opened = $true; items = $items }
    }
    finally { [GozlerProbe.Ui]::Collapse($item) }
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

# Opens Help > About gozler, reads it, takes its screenshot, and closes it
# with its Close button.
function Open-About($process, [string] $screenshot) {
    # About gozler is the last item of Help, the fourth menu.
    $about = [GozlerProbe.Win]::MenuItems((Find-Submenu $process 3))[-1]
    [void][GozlerProbe.Win]::PostMessageW($process.MainWindowHandle, 0x0111, [IntPtr]$about.Id, 0)
    $dialog = $null
    for ($i = 0; $i -lt 50 -and -not $dialog; $i++) {
        Start-Sleep -Milliseconds 100
        $dialog = [GozlerProbe.Win]::Windows($process.Id, '#32770') | Select-Object -First 1
    }
    if (-not $dialog) { return [ordered]@{ opened = $false } }
    Start-Sleep -Milliseconds 300
    $controls = [GozlerProbe.Ui]::Children((Element-Of $dialog))
    Save-Screenshot $dialog $screenshot
    $title = [GozlerProbe.Win]::Text($dialog)
    $close = $controls | Where-Object AutomationId -EQ '2' | Select-Object -First 1
    [GozlerProbe.Ui]::InvokeLater($close)
    for ($i = 0; $i -lt 50 -and [GozlerProbe.Win]::IsWindow($dialog); $i++) { Start-Sleep -Milliseconds 100 }
    return [ordered]@{
        title  = $title
        texts  = @($controls | ForEach-Object Name)
        closed = -not [GozlerProbe.Win]::IsWindow($dialog)
    }
}

# The submenu of the menu bar's item at `position`.
function Find-Submenu($process, [int] $position) {
    ([GozlerProbe.Win]::MenuItems([GozlerProbe.Win]::GetMenu($process.MainWindowHandle)))[$position].SubMenu
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
    }
    foreach ($entry in $named.GetEnumerator()) {
        $path = Join-Path $files $entry.Key
        [void][System.IO.Directory]::CreateDirectory((Split-Path $path))
        [System.IO.File]::WriteAllBytes($path, [byte[]]::new($entry.Value))
    }
    [System.IO.File]::SetLastWriteTime((Join-Path $files 'docs\Paracetamol 500mg.pdf'), [datetime]'2024-02-29 13:14:15')
    [System.IO.File]::SetLastWriteTime((Join-Path $files 'docs\comparator.txt'), [datetime]'2020-01-01 00:00:00')
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
    $search = Child-ById $element '10'
    Check 'search box' ($search.ControlType -eq [GozlerProbe.Ui]::Edit) "$search"

    $match = Child-ById $element '12'
    $modes = 'start_of_words', 'start_of_name', 'anywhere', 'whole_words', 'regular_expression' | ForEach-Object { Caption "match.$_" }
    $chosen = [GozlerProbe.Ui]::Value($match)
    # A closed drop-down list shows UI Automation no items, and this one is
    # disabled until stage 3, so its items are read with CB_GETLBTEXT.
    $items = [GozlerProbe.Win]::ComboItems([IntPtr]$match.Hwnd)
    Check 'match list' ($match.ControlType -eq [GozlerProbe.Ui]::ComboBox -and $chosen -eq $modes[0] -and
        (($items -join '|') -eq ($modes -join '|'))) "label '$($match.Name)', chosen '$chosen', items: $($items -join ', ')"

    $boxes = foreach ($id in '13', '14') {
        $box = Child-ById $element $id
        [ordered]@{ name = $box.Name; type = $box.ControlType; toggle = [GozlerProbe.Ui]::ToggleState($box) }
    }
    Check 'match check boxes' ($boxes[0].name -eq (Plain (Caption 'match.case')) -and
        $boxes[1].name -eq (Plain (Caption 'match.path')) -and
        @($boxes | Where-Object { $_.type -eq [GozlerProbe.Ui]::CheckBox -and $_.toggle -eq 0 }).Count -eq 2) $boxes

    $results['match controls enabled'] = @('11', '12', '13', '14' | ForEach-Object { (Child-ById $element $_).Enabled })

    $listView = Child-ById $element '15'
    $header = [GozlerProbe.Ui]::Children($listView) | Where-Object ControlType -EQ ([GozlerProbe.Ui]::Header) | Select-Object -First 1
    $headers = if ($header) { [GozlerProbe.Ui]::Children($header) | ForEach-Object Name } else { @() }
    $expectedHeaders = 'name', 'path', 'size', 'date_modified' | ForEach-Object { Caption "header.$_" }
    Check 'list columns' (($headers -join '|') -eq ($expectedHeaders -join '|')) ($headers -join ', ')

    $status = Child-ById $element '16'
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
        $first = if ($popup) { ([GozlerProbe.Ui]::Children((Element-Of $popup)) | Select-Object -First 1).Name }
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
        Report 'open menu through UI Automation' 'not run' 'the window could not come to the front, so no menu was opened'
        Report 'Alt and an access key open a menu' 'not run' 'the window could not come to the front, so no key was pressed'
    }

    # Help > About gozler shows the version, and Close closes it (C23).
    $expectedVersion = (Caption 'about.version') -replace '\{0\}', (& $gozler -v | Select-Object -First 1)
    $about = Open-About $window 'about-en'
    Check 'about' ($about.title -eq (Caption 'about.title') -and $about.texts -contains $expectedVersion -and
        $about.closed) ([ordered]@{ about = $about; expected_version = $expectedVersion })

    # File > Exit ends the process (C7).
    Run-MenuItem $window (Caption 'command.exit')
    Check 'exit' $window.WaitForExit(10000) "exit code $($window.HasExited ? $window.ExitCode : 'still running')"
    [void](Close-Window $window)

    # --- A second start ------------------------------------------------------
    # Windows lets the second process bring the first window forward only
    # when the process that started it may set the foreground window, as
    # File Explorer may when a person starts gozler. Whether the probe may
    # is tried first.
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
        $listView = Child-ById $element '15'
        $header = [GozlerProbe.Ui]::Children($listView) | Where-Object ControlType -EQ ([GozlerProbe.Ui]::Header) | Select-Object -First 1
        [ordered]@{
            menus       = @([GozlerProbe.Ui]::Children($bar) | ForEach-Object Name)
            match       = @('11', '13', '14' | ForEach-Object { (Child-ById $element $_).Name })
            headers     = @([GozlerProbe.Ui]::Children($header) | ForEach-Object Name)
            status_bars = @([GozlerProbe.Ui]::Children((Child-ById $element '16')) | ForEach-Object { [int]$_.Rect[2] })
        }
    }
    $englishMenus = 'menu.file', 'menu.edit', 'menu.view', 'menu.help', 'menu.language' | ForEach-Object { Plain (Caption $_) }

    if ($Pseudo) {
        Write-Settings '{ "settings": { "language": "qps-ploc" } }'
        $window = Start-Window
        $captions = Read-Captions $window
        $pseudoText = @($captions.menus + $captions.match + $captions.headers | Where-Object { $_ -notmatch '^\[.*\]$' })
        # The status bar is empty until stage 3, so its parts show the
        # language by their widths, which the longest pseudo text sets.
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
    # folder's size after those, so the wait ends when every row matches.
    # The file rows are also compared alone, which tells a folder size from
    # a file that is missing.
    $fileRows = { param($all) @($all | Where-Object { -not ([int]$_.Split(',')[2] -band 16) }) }
    $expectedRows = @(Expected-Rows $files)
    $expectedFiles = & $fileRows $expectedRows
    $deadline = [DateTime]::UtcNow.AddMinutes(3)
    do {
        $output = @(& $gozler -path "$files\" -n all -size -dm -attributes -csv -no-header 2>&1)
        $code = $LASTEXITCODE
        if ($code -ne 0 -and $code -ne 2) { break }
        $rows = @($output | Where-Object { $_ -is [string] } | Sort-Object -CaseSensitive)
        $listed = ($rows -join "`n") -ceq ($expectedRows -join "`n")
        if (-not $listed) { Start-Sleep -Seconds 2 }
    } until ($listed -or [DateTime]::UtcNow -gt $deadline)
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
                file_rows_match   = ((& $fileRows $rows) -join "`n") -ceq ($expectedFiles -join "`n")
                differences       = $differences.Count
                first_differences = @($differences | Select-Object -First 6)
            })
    }
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
