# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this> [args]`.
# Brings PROCESS's main window to the front and sends it KEYS in SendKeys notation (^j = Ctrl+J,
# {ENTER}, %{F4}), as typed keys. Positional: PROCESS KEYS.
param([Parameter(Mandatory)] [string] $Process, [Parameter(Mandatory)] [string] $Keys)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -Namespace W -Name F -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint p);
[DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
[DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
'@
$p = Get-Process $Process -ErrorAction SilentlyContinue | ? { $_.MainWindowHandle -ne 0 } | Select -First 1
if (-not $p) { "no window for $Process"; exit 1 }
$me = [W.F]::GetCurrentThreadId()
$pid0 = [uint32]0
$fg = [W.F]::GetWindowThreadProcessId([W.F]::GetForegroundWindow(), [ref]$pid0)
[void][W.F]::AttachThreadInput($me, $fg, $true)
[void][W.F]::SetForegroundWindow($p.MainWindowHandle)
[void][W.F]::AttachThreadInput($me, $fg, $false)
Start-Sleep -Milliseconds 400
[System.Windows.Forms.SendKeys]::SendWait($Keys)
"sent $Keys to $($p.ProcessName) ($($p.Id))"
