# Times the two ways of putting a 16 px icon on a DC, as gozler-ui does:
# DrawText with an ANTIALIASED_QUALITY icon font (draw_lone_glyph today), and
# AlphaBlend of a cached 16x16 premultiplied bitmap (the proposal).
Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
public static class DrawCost {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int l, t, r, b; }
    [StructLayout(LayoutKind.Sequential)] public struct BITMAPINFOHEADER { public int size, w, h; public short planes, bits; public int comp, img, xp, yp, used, imp; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] public struct LOGFONT { public int h, w, esc, ori, weight; public byte it, un, st, cs, op, cp, q, pf; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string face; }
    [StructLayout(LayoutKind.Sequential)] public struct BLEND { public byte op, flags, alpha, format; }
    [DllImport("gdi32")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32")] static extern bool DeleteDC(IntPtr dc);
    [DllImport("gdi32")] static extern IntPtr SelectObject(IntPtr dc, IntPtr o);
    [DllImport("gdi32")] static extern IntPtr CreateDIBSection(IntPtr dc, ref BITMAPINFOHEADER bi, uint u, out IntPtr bits, IntPtr s, uint o);
    [DllImport("gdi32", CharSet = CharSet.Unicode)] static extern IntPtr CreateFontIndirectW(ref LOGFONT lf);
    [DllImport("gdi32")] static extern int SaveDC(IntPtr dc);
    [DllImport("gdi32")] static extern bool RestoreDC(IntPtr dc, int n);
    [DllImport("gdi32")] static extern uint SetTextColor(IntPtr dc, uint c);
    [DllImport("gdi32")] static extern int SetBkMode(IntPtr dc, int m);
    [DllImport("gdi32")] static extern bool GdiFlush();
    [DllImport("user32", CharSet = CharSet.Unicode)] static extern int DrawTextW(IntPtr dc, string s, int n, ref RECT r, uint f);
    [DllImport("msimg32")] static extern bool AlphaBlend(IntPtr d, int x, int y, int w, int h, IntPtr s, int sx, int sy, int sw, int sh, BLEND b);
    static IntPtr Dib(int w, int h, out IntPtr bits) {
        var bi = new BITMAPINFOHEADER { size = 40, w = w, h = -h, planes = 1, bits = 32 };
        return CreateDIBSection(IntPtr.Zero, ref bi, 0, out bits, IntPtr.Zero, 0);
    }
    public static string Run(int n) {
        IntPtr bits;
        IntPtr target = CreateCompatibleDC(IntPtr.Zero);
        SelectObject(target, Dib(400, 400, out bits));
        var lf = new LOGFONT { h = -16, q = 4, face = "Segoe Fluent Icons" };
        IntPtr font = CreateFontIndirectW(ref lf);
        string glyph = "";
        var rect = new RECT { l = 10, t = 10, r = 26, b = 26 };
        // DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX
        uint flags = 0x1 | 0x4 | 0x20 | 0x800;
        Action text = () => {
            int saved = SaveDC(target);
            SelectObject(target, font);
            SetTextColor(target, 0);
            SetBkMode(target, 1);
            var r = rect;
            DrawTextW(target, glyph, 1, ref r, flags);
            RestoreDC(target, saved);
        };
        IntPtr icon = Dib(16, 16, out bits);
        int[] pixels = new int[256];
        for (int i = 0; i < 256; i++) pixels[i] = (i % 3 == 0) ? unchecked((int)0x80000000) : 0;
        Marshal.Copy(pixels, 0, bits, 256);
        var blend = new BLEND { op = 0, alpha = 255, format = 1 };
        IntPtr kept = CreateCompatibleDC(IntPtr.Zero);
        SelectObject(kept, icon);
        Action blendKept = () => AlphaBlend(target, 10, 10, 16, 16, kept, 0, 0, 16, 16, blend);
        Action blendFresh = () => {
            IntPtr dc = CreateCompatibleDC(target);
            IntPtr old = SelectObject(dc, icon);
            AlphaBlend(target, 10, 10, 16, 16, dc, 0, 0, 16, 16, blend);
            SelectObject(dc, old);
            DeleteDC(dc);
        };
        Func<Action, double> time = a => {
            for (int i = 0; i < 200; i++) a();
            GdiFlush();
            var w = Stopwatch.StartNew();
            for (int i = 0; i < n; i++) a();
            GdiFlush();
            return w.Elapsed.TotalMilliseconds * 1000 / n;
        };
        return string.Format(
            "DrawText, antialiased font: {0:F2} us an icon\nAlphaBlend, DC kept:          {1:F2} us an icon\nAlphaBlend, DC made per icon: {2:F2} us an icon",
            time(text), time(blendKept), time(blendFresh));
    }
}
'@
[DrawCost]::Run(20000)
[DrawCost]::Run(20000)
