// test-support/WinDrive.cs —— 驱动真实 TUI 窗口的工具。
//
// # 为什么不用 stdout 重定向
//
// TUI 走 crossterm 的备用屏 (alternate screen) 直接用控制台 API 绘制, **不**
// 经过 stdout 管道。用 `-RedirectStandardOutput` 启动会拿不到真控制台: 程序
// 起来了但窗口一片黑 (实测踩过)。所以这里: 正常启动 (继承真控制台) +
// win32 抢前台 + 发按键。
//
// # 为什么不截图
//
// 判断"它到底连上了没有"最可靠的信号是 **TCP 连接**(用 Get-NetTCPConnection
// 看进程的 Established 连接), 比读像素稳得多, 也不需要 System.Drawing。
using System;
using System.Runtime.InteropServices;

public static class WinDrive
{
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int c);
    [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] static extern bool AttachThreadInput(uint a, uint b, bool f);
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte sc, uint f, UIntPtr e);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);

    /// 不依赖 System.Threading 的忙等 (Add-Type 的引用集会盖掉默认引用)。
    static void Spin(int ms)
    {
        var until = DateTime.UtcNow.AddMilliseconds(ms);
        while (DateTime.UtcNow < until) { }
    }

    /// 抢前台。AttachThreadInput 是必须的 —— 只调 SetForegroundWindow 的话
    /// Windows 会拒绝把前台交给一个非前台进程 (实测: 返回 true 但前台没变,
    /// 于是按键全发到别的窗口上了)。
    public static bool Focus(IntPtr h)
    {
        if (h == IntPtr.Zero) return false;
        if (IsIconic(h)) ShowWindow(h, 9);          // SW_RESTORE
        uint fg = GetWindowThreadProcessId(GetForegroundWindow(), IntPtr.Zero);
        uint me = GetCurrentThreadId();
        AttachThreadInput(fg, me, true);
        BringWindowToTop(h);
        bool ok = SetForegroundWindow(h);
        AttachThreadInput(fg, me, false);
        return ok;
    }

    public static bool IsFg(IntPtr h) { return GetForegroundWindow() == h; }
    public static bool Ready(IntPtr h) { return h != IntPtr.Zero && IsWindowVisible(h) && !IsIconic(h); }

    /// 按一个虚拟键码 n 次 (按下+抬起)。
    ///
    /// **不要在这里忙等**: 早先用一个 `while (DateTime.UtcNow < until) {}` 的
    /// 忙等代替 sleep, 结果合成的按键**只剩 Release 事件** —— 按键日志里
    /// 每个 `Press` 都不见了, 于是选择屏的 ↓/Space 完全没生效, 看起来像"程序
    /// 不响应按键"。间隔交给调用方的 `Start-Sleep` (真 sleep) 才对。
    public static void Key(byte vk, int n)
    {
        for (int i = 0; i < n; i++)
        {
            keybd_event(vk, 0, 0, UIntPtr.Zero);   // KEYEVENTF_EXTENDEDKEY=0 → 按下
            keybd_event(vk, 0, 2, UIntPtr.Zero);   // KEYEVENTF_KEYUP → 抬起
        }
    }

    /// 逐个字符敲一段 ASCII。
    public static void Text(string s)
    {
        foreach (char c in s) Key((byte)char.ToUpperInvariant(c), 1);
    }
}
