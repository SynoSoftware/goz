param([string]$Out)
# Draws the same icons with real GDI two ways, black on white, as gozler-ui
# would: A is DrawText with an ANTIALIASED_QUALITY icon font
# (draw_lone_glyph today); B is glyph_coverage's 4x drawing averaged down,
# then blended (the proposal). Then times each way.
Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
public static class Compare {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int l, t, r, b; }
    [StructLayout(LayoutKind.Sequential)] public struct BIH { public int size, w, h; public short planes, bits; public int comp, img, xp, yp, used, imp; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] public struct LOGFONT { public int h, w, esc, ori, weight; public byte it, un, st, cs, op, cp, q, pf; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string face; }
    [StructLayout(LayoutKind.Sequential)] public struct BLEND { public byte op, flags, alpha, format; }
    [DllImport("gdi32")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32")] static extern bool DeleteDC(IntPtr dc);
    [DllImport("gdi32")] static extern bool DeleteObject(IntPtr o);
    [DllImport("gdi32")] static extern IntPtr SelectObject(IntPtr dc, IntPtr o);
    [DllImport("gdi32")] static extern IntPtr CreateDIBSection(IntPtr dc, ref BIH bi, uint u, out IntPtr bits, IntPtr s, uint o);
    [DllImport("gdi32", CharSet = CharSet.Unicode)] static extern IntPtr CreateFontIndirectW(ref LOGFONT lf);
    [DllImport("gdi32")] static extern int SaveDC(IntPtr dc);
    [DllImport("gdi32")] static extern bool RestoreDC(IntPtr dc, int n);
    [DllImport("gdi32")] static extern uint SetTextColor(IntPtr dc, uint c);
    [DllImport("gdi32")] static extern int SetBkMode(IntPtr dc, int m);
    [DllImport("gdi32")] static extern bool GdiFlush();
    [DllImport("user32", CharSet = CharSet.Unicode)] static extern int DrawTextW(IntPtr dc, string s, int n, ref RECT r, uint f);
    [DllImport("msimg32")] static extern bool AlphaBlend(IntPtr d, int x, int y, int w, int h, IntPtr s, int sx, int sy, int sw, int sh, BLEND b);
    const uint Flags = 0x1 | 0x4 | 0x20 | 0x800;

    static IntPtr Dib(int w, int h, out IntPtr bits) {
        var bi = new BIH { size = 40, w = w, h = -h, planes = 1, bits = 32 };
        return CreateDIBSection(IntPtr.Zero, ref bi, 0, out bits, IntPtr.Zero, 0);
    }
    static IntPtr Font(int px) {
        var lf = new LOGFONT { h = -px, q = 4, face = "Segoe Fluent Icons" };
        return CreateFontIndirectW(ref lf);
    }
    // A: as draw_lone_glyph.
    static void DrawA(IntPtr dc, IntPtr font, char glyph, int x, int y, int side) {
        int saved = SaveDC(dc);
        SelectObject(dc, font);
        SetTextColor(dc, 0);
        SetBkMode(dc, 1);
        var r = new RECT { l = x, t = y, r = x + side, b = y + side };
        DrawTextW(dc, glyph.ToString(), 1, ref r, Flags);
        RestoreDC(dc, saved);
    }
    // B, built once: as glyph_coverage, then a premultiplied black bitmap
    // as menu_bitmap makes.
    static IntPtr BuildB(IntPtr bigFont, char glyph, int side) {
        int large = side * 4;
        IntPtr bits;
        IntPtr big = Dib(large, large, out bits);
        IntPtr dc = CreateCompatibleDC(IntPtr.Zero);
        IntPtr ob = SelectObject(dc, big);
        IntPtr of = SelectObject(dc, bigFont);
        SetTextColor(dc, 0xffffff);
        SetBkMode(dc, 1);
        var r = new RECT { r = large, b = large };
        DrawTextW(dc, glyph.ToString(), 1, ref r, Flags);
        GdiFlush();
        int[] px = new int[large * large];
        Marshal.Copy(bits, px, 0, px.Length);
        SelectObject(dc, of); SelectObject(dc, ob); DeleteDC(dc); DeleteObject(big);
        IntPtr outBits;
        IntPtr icon = Dib(side, side, out outBits);
        int[] o = new int[side * side];
        for (int y = 0; y < side; y++) for (int x = 0; x < side; x++) {
            int sum = 0;
            for (int b = 0; b < 16; b++) sum += (px[(y * 4 + b / 4) * large + x * 4 + b % 4] >> 8) & 0xff;
            o[y * side + x] = (sum / 16) << 24;
        }
        Marshal.Copy(o, 0, outBits, o.Length);
        return icon;
    }
    static void DrawB(IntPtr dc, IntPtr source, int x, int y, int side) {
        var blend = new BLEND { alpha = 255, format = 1 };
        AlphaBlend(dc, x, y, side, side, source, 0, 0, side, side, blend);
    }

    public static int[] Sheet(string glyphs) {
        int[] sides = { 16, 20, 24 };
        int cell = 30, zoom = 8;
        int width = 120 + glyphs.Length * cell;
        int zoomTop = 3 * 2 * cell + 20;
        int height = zoomTop + 2 * (16 * zoom + 12) + 10;
        IntPtr bits;
        IntPtr sheet = Dib(width, height, out bits);
        IntPtr dc = CreateCompatibleDC(IntPtr.Zero);
        SelectObject(dc, sheet);
        int[] white = new int[width * height];
        for (int i = 0; i < white.Length; i++) white[i] = unchecked((int)0xffffffff);
        Marshal.Copy(white, 0, bits, white.Length);
        IntPtr src = CreateCompatibleDC(IntPtr.Zero);
        for (int s = 0; s < sides.Length; s++) {
            int side = sides[s];
            IntPtr fa = Font(side), fb = Font(side * 4);
            int rowA = 10 + s * 2 * cell, rowB = rowA + cell;
            for (int g = 0; g < glyphs.Length; g++) {
                int x = 120 + g * cell;
                DrawA(dc, fa, glyphs[g], x, rowA, side);
                IntPtr icon = BuildB(fb, glyphs[g], side);
                IntPtr old = SelectObject(src, icon);
                DrawB(dc, src, x, rowB, side);
                SelectObject(src, old); DeleteObject(icon);
            }
            DeleteObject(fa); DeleteObject(fb);
        }
        GdiFlush();
        // Enlarged 16 px copies of the first eight icons, A over B.
        int[] all = new int[width * height];
        Marshal.Copy(bits, all, 0, all.Length);
        for (int row = 0; row < 2; row++) {
            int srcY = 10 + row * cell;
            int dstY = zoomTop + row * (16 * zoom + 12);
            for (int g = 0; g < Math.Min(8, glyphs.Length); g++)
            for (int y = 0; y < 16; y++) for (int x = 0; x < 16; x++) {
                int v = all[(srcY + y) * width + 120 + g * cell + x];
                for (int dy = 0; dy < zoom; dy++) for (int dx = 0; dx < zoom; dx++) {
                    int X = 4 + g * (16 * zoom + 4) + x * zoom + dx, Y = dstY + y * zoom + dy;
                    if (X < width && Y < height) all[Y * width + X] = v | unchecked((int)0xff000000);
                }
            }
        }
        for (int i = 0; i < all.Length; i++) all[i] |= unchecked((int)0xff000000);
        DeleteDC(src); DeleteDC(dc); DeleteObject(sheet);
        Width = width; Height = height;
        return all;
    }
    public static int Width, Height;

    public static string Time(int n) {
        IntPtr bits;
        IntPtr target = CreateCompatibleDC(IntPtr.Zero);
        IntPtr surface = Dib(400, 400, out bits);
        SelectObject(target, surface);
        IntPtr fa = Font(16), fb = Font(64);
        char glyph = (char)0xE711;
        IntPtr src = CreateCompatibleDC(IntPtr.Zero);
        IntPtr icon = BuildB(fb, glyph, 16);
        SelectObject(src, icon);
        Func<Action, int, double> time = (a, count) => {
            for (int i = 0; i < 100; i++) a();
            GdiFlush();
            var w = Stopwatch.StartNew();
            for (int i = 0; i < count; i++) a();
            GdiFlush();
            return w.Elapsed.TotalMilliseconds * 1000 / count;
        };
        double a1 = time(() => DrawA(target, fa, glyph, 10, 10, 16), n);
        double b1 = time(() => DrawB(target, src, 10, 10, 16), n);
        double build = time(() => DeleteObject(BuildB(fb, glyph, 16)), n / 20);
        return string.Format("A, each icon drawn: {0:F1} us\nB, each icon drawn: {1:F1} us\nB, building one cached image once: {2:F1} us", a1, b1, build);
    }
}
'@
$pixels = [Compare]::Sheet("`u{E8C8}`u{E70D}`u{E8A0}`u{E75C}`u{E774}`u{E7E8}`u{E721}`u{EDA2}`u{E8B7}`u{E950}`u{E737}`u{E9F5}`u{E74D}`u{E711}`u{E8FB}`u{E738}")
Add-Type -AssemblyName System.Drawing
$bmp = [System.Drawing.Bitmap]::new([Compare]::Width, [Compare]::Height, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$data = $bmp.LockBits([System.Drawing.Rectangle]::new(0, 0, $bmp.Width, $bmp.Height), 'WriteOnly', $bmp.PixelFormat)
[System.Runtime.InteropServices.Marshal]::Copy($pixels, 0, $data.Scan0, $pixels.Length)
$bmp.UnlockBits($data)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$f = [System.Drawing.Font]::new('Segoe UI', 9)
foreach ($s in 0..2) {
    $side = 16 + 4 * $s
    $g.DrawString("A today, $side px", $f, [System.Drawing.Brushes]::Black, 2, 10 + $s * 60)
    $g.DrawString("B 4x, $side px", $f, [System.Drawing.Brushes]::Black, 2, 40 + $s * 60)
}
$g.Dispose()
$bmp.Save($Out)
[Compare]::Time(20000)
[Compare]::Time(20000)
