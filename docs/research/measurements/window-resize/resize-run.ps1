param([string]$Query = 'a', [string]$Shot)
$ErrorActionPreference = 'Stop'
$scratch = Split-Path -Parent $MyInvocation.MyCommand.Path
$exe = 'C:\SynoSoftware\TinyTorrent2\3rdParty\goz\target\release\gozler-ui.exe'
$data = Join-Path $scratch 'gozdata'
New-Item -ItemType Directory -Force $data | Out-Null
$timing = Join-Path $scratch 'resize-timing.txt'
Remove-Item $timing -ErrorAction SilentlyContinue

Add-Type @'
using System; using System.Runtime.InteropServices; using System.Text;
public static class W {
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, string l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint f);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
'@
Add-Type -AssemblyName System.Drawing

$p = Start-Process $exe -ArgumentList '--data-dir', "`"$data`"" -PassThru
try {
  $deadline = (Get-Date).AddSeconds(15)
  do { Start-Sleep -Milliseconds 200; $p.Refresh() } until ($p.MainWindowHandle -ne 0 -or (Get-Date) -gt $deadline)
  $h = $p.MainWindowHandle
  if ($h -eq 0) { throw 'no main window' }
  [W]::SetWindowPos($h, 0, 100, 100, 1300, 850, 0x14) | Out-Null
  $search = [W]::GetDlgItem($h, 10)
  [W]::SendMessage($search, 0x000C, [IntPtr]::Zero, $Query) | Out-Null
  Start-Sleep -Seconds 4
  Remove-Item $timing -ErrorAction SilentlyContinue
  [W]::SendMessage($h, 0x0231, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null  # WM_ENTERSIZEMOVE
  $widths = (1300..700 | Where-Object { $_ % 10 -eq 0 } | Sort-Object -Descending) + (700..1300 | Where-Object { $_ % 10 -eq 0 })
  $sw = [Diagnostics.Stopwatch]::StartNew()
  foreach ($w in $widths) {
    [W]::SetWindowPos($h, 0, 0, 0, $w, 850, 0x16) | Out-Null  # NOMOVE|NOZORDER|NOACTIVATE
    Start-Sleep -Milliseconds 16
  }
  $total = $sw.ElapsedMilliseconds
  [W]::SendMessage($h, 0x0232, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null  # WM_EXITSIZEMOVE
  "steps $($widths.Count) total $total ms (includes 16 ms sleeps)"
  if ($Shot) {
    [W]::SetForegroundWindow($h) | Out-Null
    Start-Sleep -Milliseconds 800
    $r = New-Object W+RECT; [W]::GetWindowRect($h, [ref]$r) | Out-Null
    $bmp = New-Object Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
    $g = [Drawing.Graphics]::FromImage($bmp)
    $dc = $g.GetHdc(); [W]::PrintWindow($h, $dc, 2) | Out-Null; $g.ReleaseHdc($dc)
    $bmp.Save($Shot); $g.Dispose(); $bmp.Dispose()
  }
} finally {
  Stop-Process -Id $p.Id -ErrorAction SilentlyContinue
}
