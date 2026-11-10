# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this> [args]`.
# Clicks a window's own close button (the X in its title bar) with the real mouse. The button is
# found the way Windows finds it, by hit-testing (WM_NCHITTEST → HTCLOSE) along the top of the
# window: UI Automation's managed client does not expose a non-client title bar here.
param([Parameter(Mandatory)] [int] $ProcessId)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
Add-Type -Namespace W -Name K -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern System.IntPtr SendMessage(System.IntPtr h, uint m, System.IntPtr w, System.IntPtr l);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint f, int x, int y, uint d, System.UIntPtr e);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
'@
$A = [System.Windows.Automation.AutomationElement]
$win = $A::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Children, (New-Object System.Windows.Automation.PropertyCondition ($A::ProcessIdProperty, $ProcessId)))
if (-not $win) { "no window for pid $ProcessId"; exit 1 }
$hwnd = [IntPtr]$win.Current.NativeWindowHandle
$r = New-Object W.K+RECT
[void][W.K]::GetWindowRect($hwnd, [ref]$r)
$screen = [System.Windows.Forms.SystemInformation]::VirtualScreen
$hit = { param($x, $y) [int][W.K]::SendMessage($hwnd, 0x0084, [IntPtr]::Zero, [IntPtr](($y -shl 16) -bor ($x -band 0xFFFF))) }
$xs = @()
foreach ($y in ($r.Top + 8)..($r.Top + 40)) {
    foreach ($x in ([Math]::Min($r.Right, $screen.Right) - 1)..($r.Right - 160)) { if ((& $hit $x $y) -eq 20) { $xs += $x } }
    if ($xs) { $cy = $y + 6; break }
}
if (-not $xs) { "no close button (no HTCLOSE along the top of the window)"; exit 1 }
$cx = [int](($xs | Measure-Object -Average).Average)
[void][W.K]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 300
[void][W.K]::SetCursorPos($cx, $cy)
Start-Sleep -Milliseconds 300
[W.K]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 80; [W.K]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
"clicked the close button at $cx,$cy"
