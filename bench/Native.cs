// Black-box latency measurement helpers: synthesized input + screen polling.
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Threading;

public static class Native
{
    [StructLayout(LayoutKind.Explicit, Size = 40)]
    struct INPUT
    {
        [FieldOffset(0)] public uint type;
        [FieldOffset(8)] public ushort vk;
        [FieldOffset(10)] public ushort scan;
        [FieldOffset(12)] public uint flags;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    [DllImport("user32.dll")] static extern uint SendInput(uint n, INPUT[] inputs, int size);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr h);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleBitmap(IntPtr dc, int w, int h);
    [DllImport("gdi32.dll")] static extern IntPtr SelectObject(IntPtr dc, IntPtr obj);
    [DllImport("gdi32.dll")] static extern bool BitBlt(IntPtr dst, int x, int y, int w, int h, IntPtr src, int sx, int sy, uint rop);
    [DllImport("gdi32.dll")] static extern int GetBitmapBits(IntPtr bmp, int count, byte[] bits);

    const uint KEYUP = 2, UNICODE = 4, SRCCOPY = 0x00CC0020, CAPTUREBLT = 0x40000000;

    static INPUT Key(ushort vk, ushort scan, uint flags)
    {
        return new INPUT { type = 1, vk = vk, scan = scan, flags = flags };
    }

    static void Send(params INPUT[] inputs)
    {
        SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(INPUT)));
    }

    /// Presses `vk` while holding `mods`.
    public static void Chord(ushort[] mods, ushort vk)
    {
        var seq = new System.Collections.Generic.List<INPUT>();
        foreach (var m in mods) seq.Add(Key(m, 0, 0));
        seq.Add(Key(vk, 0, 0));
        seq.Add(Key(vk, 0, KEYUP));
        for (int i = mods.Length - 1; i >= 0; i--) seq.Add(Key(mods[i], 0, KEYUP));
        Send(seq.ToArray());
    }

    public static void Type(string text)
    {
        var seq = new System.Collections.Generic.List<INPUT>();
        foreach (char c in text)
        {
            seq.Add(Key(0, c, UNICODE));
            seq.Add(Key(0, c, UNICODE | KEYUP));
        }
        Send(seq.ToArray());
    }

    public static uint ForegroundPid()
    {
        uint pid;
        GetWindowThreadProcessId(GetForegroundWindow(), out pid);
        return pid;
    }

    /// A screen region polled for visible repaints. Changes nobody could see
    /// (backdrop dithering of a level or two) are ignored, so translucent
    /// windows aren't penalized for their shimmer.
    public sealed class Region
    {
        const int MinChannelDelta = 24;      // summed over B, G, R
        const double MinChangedFraction = 0.001;

        readonly int x, y, w, h;
        readonly IntPtr screen, mem, bmp;
        byte[] prev, cur;

        public Region(int x, int y, int w, int h)
        {
            this.x = x; this.y = y; this.w = w; this.h = h;
            screen = GetDC(IntPtr.Zero);
            mem = CreateCompatibleDC(screen);
            bmp = CreateCompatibleBitmap(screen, w, h);
            SelectObject(mem, bmp);
            prev = new byte[w * h * 4];
            cur = new byte[w * h * 4];
            Capture(prev);
        }

        void Capture(byte[] into)
        {
            BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT);
            GetBitmapBits(bmp, into.Length, into);
        }

        /// Resets the reference frame to what's on screen now.
        public void Reset() { Capture(prev); }

        /// True if the screen visibly changed since the last call (or Reset).
        public bool Changed()
        {
            Capture(cur);
            int changed = 0, threshold = (int)(w * h * MinChangedFraction);
            for (int i = 0; i < cur.Length; i += 4)
            {
                int d = Math.Abs(cur[i] - prev[i]) + Math.Abs(cur[i + 1] - prev[i + 1]) + Math.Abs(cur[i + 2] - prev[i + 2]);
                if (d >= MinChannelDelta && ++changed > threshold) break;
            }
            if (changed <= threshold) return false;
            var t = prev; prev = cur; cur = t;
            return true;
        }
    }

    /// Runs `action`, then polls `region` until it has been unchanged for `quietMs`.
    /// Returns [first change ms, last change ms, foreground-acquired ms] (-1 = never).
    public static double[] Measure(Action action, Region region, uint pid, int quietMs, int timeoutMs)
    {
        region.Reset();
        double first = -1, lastChange = -1, focus = -1;
        var sw = Stopwatch.StartNew();
        action();
        while (sw.ElapsedMilliseconds < timeoutMs)
        {
            double t = sw.Elapsed.TotalMilliseconds;
            if (focus < 0 && pid != 0 && ForegroundPid() == pid) focus = t;
            if (region.Changed())
            {
                if (first < 0) first = t;
                lastChange = t;
            }
            if (first >= 0 && t - lastChange > quietMs) break;
        }
        return new[] { first, lastChange, focus };
    }

    /// Runs `action` and lists the times (ms) at which `region` changed, for debugging.
    public static string Timeline(Action action, Region region, int durationMs)
    {
        var changes = new System.Collections.Generic.List<string>();
        region.Reset();
        var sw = Stopwatch.StartNew();
        action();
        while (sw.ElapsedMilliseconds < durationMs)
        {
            if (region.Changed()) changes.Add(sw.Elapsed.TotalMilliseconds.ToString("0"));
        }
        return string.Join(" ", changes.ToArray());
    }
}
