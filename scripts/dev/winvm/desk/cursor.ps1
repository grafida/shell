# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this> [args]`.
# Samples the pointer's shape for -Seconds and prints how often it was the "working in background"
# (IDC_APPSTARTING) or busy (IDC_WAIT) cursor, with the pointer parked at -X,-Y (over the window:
# a pointer elsewhere shows that window's cursor). Prints the longest unbroken "working" stretch too.
param([double] $Seconds = 10, [int] $X = 640, [int] $Y = 400)
Add-Type -Namespace W -Name C -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct CURSORINFO { public int cbSize, flags; public System.IntPtr hCursor; public POINT pt; }
[DllImport("user32.dll")] public static extern bool GetCursorInfo(ref CURSORINFO i);
[DllImport("user32.dll")] public static extern System.IntPtr LoadCursor(System.IntPtr h, int id);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
'@
$starting = [W.C]::LoadCursor([IntPtr]::Zero, 32650); $wait = [W.C]::LoadCursor([IntPtr]::Zero, 32514)
[void][W.C]::SetCursorPos($X, $Y)
$n = 0; $s = 0; $w = 0; $run = 0; $longest = 0
$end = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $end) {
    $i = New-Object W.C+CURSORINFO; $i.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($i)
    [void][W.C]::GetCursorInfo([ref]$i); $n++
    if ($i.hCursor -eq $starting) { $s++; $run++ } elseif ($i.hCursor -eq $wait) { $w++; $run++ } else { $run = 0 }
    if ($run -gt $longest) { $longest = $run }
    Start-Sleep -Milliseconds 100
}
"samples $n, appstarting $s, wait $w, longest $longest"
