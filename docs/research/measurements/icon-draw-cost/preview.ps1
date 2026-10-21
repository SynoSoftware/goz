param([string[]]$Glyphs, [string]$Out, [int]$Side = 16, [int]$Zoom = 8)
# Draws each glyph as glyph_coverage does: 4x the side, averaged down, then
# shows it black on white, enlarged without smoothing, beside its 1:1 copy.
Add-Type -AssemblyName System.Drawing
$Glyphs = $Glyphs -join "," -split ","
$large = $Side * 4
$font = [System.Drawing.Font]::new('Segoe Fluent Icons', $large, [System.Drawing.GraphicsUnit]::Pixel)
$format = [System.Drawing.StringFormat]::GenericTypographic
$format.Alignment = 'Center'; $format.LineAlignment = 'Center'
$cell = $Side * $Zoom + 8
$sheet = [System.Drawing.Bitmap]::new($cell * $Glyphs.Count, $cell + $Side + 8)
$sg = [System.Drawing.Graphics]::FromImage($sheet); $sg.Clear([System.Drawing.Color]::White)
$i = 0
foreach ($hex in $Glyphs) {
    $big = [System.Drawing.Bitmap]::new($large, $large)
    $g = [System.Drawing.Graphics]::FromImage($big); $g.Clear([System.Drawing.Color]::Black)
    $g.TextRenderingHint = 'AntiAlias'
    $g.DrawString([string][char][Convert]::ToInt32($hex, 16), $font, [System.Drawing.Brushes]::White, [System.Drawing.RectangleF]::new(0, 0, $large, $large), $format)
    $g.Dispose()
    for ($y = 0; $y -lt $Side; $y++) { for ($x = 0; $x -lt $Side; $x++) {
        $sum = 0
        for ($b = 0; $b -lt 16; $b++) { $sum += $big.GetPixel($x * 4 + $b % 4, $y * 4 + [int][Math]::Floor($b / 4)).R }
        $v = 255 - [int]($sum / 16)
        $c = [System.Drawing.Color]::FromArgb($v, $v, $v)
        $brush = [System.Drawing.SolidBrush]::new($c)
        $sg.FillRectangle($brush, $i * $cell + $x * $Zoom, $y * $Zoom, $Zoom, $Zoom)
        $sheet.SetPixel($i * $cell + $x, $cell + $y, $c)
        $brush.Dispose()
    } }
    $big.Dispose(); $i++
}
$sg.Dispose(); $sheet.Save($Out); $sheet.Dispose()
