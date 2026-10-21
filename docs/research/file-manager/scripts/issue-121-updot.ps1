param([string]$Out)
# Mock-up of the panels' `..` row for issue #121, from the real shell icons
# (SHGetStockIconInfo) and the real glyph (Segoe Fluent Icons U+E74A),
# placed as CustomDraw::draw_over_icon places it.
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Shell {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct SHSTOCKICONINFO {
    public uint cbSize; public IntPtr hIcon; public int iSysImageIndex; public int iIcon;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string szPath;
  }
  [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
  static extern int SHGetStockIconInfo(uint siid, uint flags, ref SHSTOCKICONINFO info);
  [DllImport("user32.dll")] public static extern bool DestroyIcon(IntPtr h);
  public static IntPtr Small(uint siid) {
    var info = new SHSTOCKICONINFO();
    info.cbSize = (uint)Marshal.SizeOf(typeof(SHSTOCKICONINFO));
    SHGetStockIconInfo(siid, 0x100 | 0x1, ref info); // SHGSI_ICON | SHGSI_SMALLICON
    return info.hIcon;
  }
}
"@
$SIID_FOLDER = 3; $SIID_FOLDEROPEN = 4
function Icon16($siid) {
  $h = [Shell]::Small($siid)
  $bmp = [System.Drawing.Icon]::FromHandle($h).ToBitmap()
  [Shell]::DestroyIcon($h) | Out-Null
  $bmp
}
$folder = Icon16 $SIID_FOLDER
$open = Icon16 $SIID_FOLDEROPEN
$glyphFont = 'Segoe Fluent Icons'
if (-not ([System.Drawing.FontFamily]::Families | Where-Object Name -eq $glyphFont)) { $glyphFont = 'Segoe MDL2 Assets' }
$arrow = [string][char]0xE74A

function DrawArrow($g, $rx, $ry, $rw, $rh, $side, $color) {
  # draw_over_icon: the icon cell's lower three quarters, the glyph `side`
  # pixels square, centred in it.
  $top = $ry + [int][Math]::Floor($rh / 4)
  $ah = $rh - [int][Math]::Floor($rh / 4)
  $x = $rx + [int](($rw - $side) / 2); $y = $top + [int](($ah - $side) / 2)
  $f = New-Object System.Drawing.Font($glyphFont, $side, [System.Drawing.GraphicsUnit]::Pixel)
  $b = New-Object System.Drawing.SolidBrush($color)
  $fmt = [System.Drawing.StringFormat]::GenericTypographic
  $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
  $g.DrawString($arrow, $f, $b, [single]$x, [single]$y, $fmt)
}

# One list row: `rowH` pixels tall, the 16 px shell icon centred in a cell as
# wide as the icon, ".." after it.
function Row($option, $theme, $rowH) {
  $w = 90; $bmp = New-Object System.Drawing.Bitmap($w, $rowH)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $bg = if ($theme -eq 'hc') { [System.Drawing.Color]::Black } else { [System.Drawing.Color]::White }
  $fg = if ($theme -eq 'hc') { [System.Drawing.Color]::White } else { [System.Drawing.Color]::Black }
  $g.Clear($bg)
  $ix = 4; $iy = [int](($rowH - 16) / 2)
  switch ($option) {
    'A' { $g.DrawImage($open, $ix, $iy, 16, 16) }
    'B' {
      # One image made once at 16 px: the folder with the arrow baked in, in
      # a fixed dark colour, as an image list holds it.
      $g.DrawImage($folder, $ix, $iy, 16, 16)
      DrawArrow $g $ix $iy 16 16 9 ([System.Drawing.Color]::FromArgb(32, 32, 32))
    }
    'C' {
      $g.DrawImage($folder, $ix, $iy, 16, 16)
      $side = [int][Math]::Floor($rowH * 9 / 16)
      DrawArrow $g $ix 0 16 $rowH $side $fg
    }
  }
  $tf = New-Object System.Drawing.Font('Segoe UI', [single]($rowH * 0.45), [System.Drawing.GraphicsUnit]::Pixel)
  $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
  $g.DrawString('..', $tf, (New-Object System.Drawing.SolidBrush($fg)), [single]($ix + 16 + 6), [single](($rowH - $tf.Height) / 2))
  $g.Dispose(); $bmp
}

function Scaled($bmp, $k) {
  $out = New-Object System.Drawing.Bitmap(($bmp.Width * $k), ($bmp.Height * $k))
  $g = [System.Drawing.Graphics]::FromImage($out)
  $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
  $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
  $g.DrawImage($bmp, 0, 0, $out.Width, $out.Height); $g.Dispose(); $out
}

$cols = @(
  @{ k = 'A'; t = 'A  Windows open-folder icon, no arrow' },
  @{ k = 'B'; t = 'B  folder + arrow merged into one image' },
  @{ k = 'C'; t = 'C  today: arrow drawn over the folder' })
$rows = @(
  @{ theme = 'light'; h = 20; t = 'Light, 100 %' },
  @{ theme = 'hc'; h = 20; t = 'High contrast (Night sky), 100 %' },
  @{ theme = 'light'; h = 40; t = '200 % monitor on a 100 % system (row 40 px, icon stays 16 px)' })
$k = 4; $colW = 90 * $k + 40; $labelW = 0; $head = 40
$rowBlock = @(); foreach ($r in $rows) { $rowBlock += ($r.h * $k + $r.h + 44) }
$H = $head + ($rowBlock | Measure-Object -Sum).Sum + 20
$W = 20 + $colW * 3
$sheet = New-Object System.Drawing.Bitmap($W, $H)
$g = [System.Drawing.Graphics]::FromImage($sheet)
$g.Clear([System.Drawing.Color]::FromArgb(243, 243, 243))
$g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::ClearTypeGridFit
$hf = New-Object System.Drawing.Font('Segoe UI Semibold', 13, [System.Drawing.GraphicsUnit]::Pixel)
$lf = New-Object System.Drawing.Font('Segoe UI', 12, [System.Drawing.GraphicsUnit]::Pixel)
$ink = [System.Drawing.Brushes]::Black
for ($c = 0; $c -lt 3; $c++) { $g.DrawString($cols[$c].t, $hf, $ink, [single](20 + $c * $colW), [single]12) }
$y = $head
foreach ($r in $rows) {
  $g.DrawString($r.t + '   (actual size, then 4x)', $lf, $ink, [single]20, [single]$y)
  for ($c = 0; $c -lt 3; $c++) {
    $row = Row $cols[$c].k $r.theme $r.h
    $x = 20 + $c * $colW
    $g.DrawImage($row, $x, $y + 18)
    $big = Scaled $row $k
    $g.DrawImage($big, $x, $y + 18 + $r.h + 4)
  }
  $y += $r.h * $k + 20 + 24 + $r.h
}
$g.Dispose()
$sheet.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
"saved $Out ($W x $H), glyph font $glyphFont"
