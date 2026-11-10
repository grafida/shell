# Grafida Shell — development tooling.
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT licence; see LICENSE.
#
# The console-window watcher (grafida/grafida#91): records every console window shown on the
# desktop — conhost's (ConsoleWindowClass) and Windows Terminal's (CASCADIA_HOSTING_WINDOW_CLASS) —
# with the time, the process and its command line, until C:\winvm\watch.stop appears. Started and
# stopped by scripts/dev/winvm.sh watch-start / watch-stop, in the desktop session (a hook in the
# SSH session sees nothing: nothing is ever shown there).
#
# A process cannot tell from its own console whether a window appeared (with Windows Terminal as
# the default terminal every console has a "visible" 0x0 PseudoConsoleWindow), so the desktop is
# watched instead, through a WinEvent hook on EVENT_OBJECT_SHOW.
#
# Output: C:\winvm\watch.log, one line per window: "<UTC time>  <class>  <pid> <name>  <command line>".

$ErrorActionPreference = 'Stop'
$log = 'C:\winvm\watch.log'
$stop = 'C:\winvm\watch.stop'
Remove-Item $stop -ErrorAction SilentlyContinue
Set-Content -Path $log -Value $null

Add-Type -TypeDefinition @'
using System; using System.Collections.Concurrent; using System.Runtime.InteropServices; using System.Text;
public static class ConsoleWatch {
    public delegate void WinEventProc(IntPtr hook, uint ev, IntPtr hwnd, int obj, int child, uint thread, uint time);
    [DllImport("user32.dll")] public static extern IntPtr SetWinEventHook(uint min, uint max, IntPtr mod, WinEventProc proc, uint pid, uint tid, uint flags);
    [DllImport("user32.dll")] public static extern bool UnhookWinEvent(IntPtr hook);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool PeekMessage(out MSG m, IntPtr h, uint min, uint max, uint remove);
    [DllImport("user32.dll")] public static extern bool TranslateMessage(ref MSG m);
    [DllImport("user32.dll")] public static extern IntPtr DispatchMessage(ref MSG m);
    [StructLayout(LayoutKind.Sequential)] public struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam, lParam; public uint time; public int x, y; }
    public static readonly ConcurrentQueue<string> Seen = new ConcurrentQueue<string>();
    public static readonly WinEventProc Proc = OnShow;
    static void OnShow(IntPtr hook, uint ev, IntPtr hwnd, int obj, int child, uint t, uint time) {
        if (obj != 0 || child != 0 || hwnd == IntPtr.Zero) return;
        var cls = new StringBuilder(256); GetClassName(hwnd, cls, 256);
        var name = cls.ToString();
        if (name != "ConsoleWindowClass" && name != "CASCADIA_HOSTING_WINDOW_CLASS") return;
        uint pid; GetWindowThreadProcessId(hwnd, out pid);
        Seen.Enqueue(DateTime.UtcNow.ToString("HH:mm:ss.fff") + "  " + name + "  " + pid);
    }
}
'@

$hook = [ConsoleWatch]::SetWinEventHook(0x8002, 0x8002, [IntPtr]::Zero, [ConsoleWatch]::Proc, 0, 0, 0)
$msg = New-Object ConsoleWatch+MSG
while (-not (Test-Path $stop)) {
    while ([ConsoleWatch]::PeekMessage([ref]$msg, [IntPtr]::Zero, 0, 0, 1)) {
        [void][ConsoleWatch]::TranslateMessage([ref]$msg); [void][ConsoleWatch]::DispatchMessage([ref]$msg)
    }
    $line = $null
    while ([ConsoleWatch]::Seen.TryDequeue([ref]$line)) {
        $pid2 = [int]($line -split '  ')[2]
        $p = Get-CimInstance Win32_Process -Filter "ProcessId=$pid2" -ErrorAction SilentlyContinue
        Add-Content -Path $log -Value "$line $($p.Name)  $($p.CommandLine)"
    }
    Start-Sleep -Milliseconds 20
}
[void][ConsoleWatch]::UnhookWinEvent($hook)
Add-Content -Path $log -Value '# stopped'
