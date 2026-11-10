# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this> [args]`.
# Opens File Explorer at a file, selected, on the left half of the screen, and prints the file's
# screen position (the middle of its name), for a real drag out of Explorer.
param([Parameter(Mandatory)] [string] $File)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
Get-Process explorer -ErrorAction SilentlyContinue | Out-Null
Start-Process explorer.exe "/select,`"$File`""
$A = [System.Windows.Automation.AutomationElement]
$P = [System.Windows.Automation.PropertyCondition]
$S = [System.Windows.Automation.TreeScope]
$folder = Split-Path (Split-Path $File) -Leaf
$win = $null
for ($i = 0; $i -lt 75 -and -not $win; $i++) {
    Start-Sleep -Milliseconds 200
    $win = $A::RootElement.FindAll($S::Children, (New-Object $P ($A::ClassNameProperty, 'CabinetWClass'))) | ? { $_.Current.Name -like "*$folder*" } | Select -First 1
}
if (-not $win) { "no Explorer window for $folder"; exit 1 }
Add-Type -Namespace W -Name M -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool MoveWindow(System.IntPtr h, int x, int y, int w, int hh, bool r);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool BringWindowToTop(System.IntPtr h);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, System.IntPtr p);
[DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
[DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
'@
$hwnd = [IntPtr]$win.Current.NativeWindowHandle
$wa = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
[void][W.M]::MoveWindow($hwnd, $wa.Left, $wa.Top, [int]($wa.Width / 2), $wa.Height, $true)
# Started from the background, Explorer opens behind the window in front: take the foreground
# through the front window's input queue (the foreground lock), as the shell's `key` does.
$theirs = [W.M]::GetWindowThreadProcessId([W.M]::GetForegroundWindow(), [IntPtr]::Zero); $ours = [W.M]::GetCurrentThreadId()
$attached = $theirs -ne 0 -and $theirs -ne $ours -and [W.M]::AttachThreadInput($ours, $theirs, $true)
[void][W.M]::BringWindowToTop($hwnd); [void][W.M]::SetForegroundWindow($hwnd)
if ($attached) { [void][W.M]::AttachThreadInput($ours, $theirs, $false) }
Start-Sleep -Milliseconds 300
if ([W.M]::GetForegroundWindow() -ne $hwnd) { "Explorer did not come to the front"; exit 1 }
Start-Sleep -Milliseconds 800
$name = Split-Path $File -Leaf
$item = $null
for ($i = 0; $i -lt 25 -and -not $item; $i++) {
    $item = $win.FindFirst($S::Descendants, (New-Object $P ($A::NameProperty, $name)))
    if (-not $item) { $item = $win.FindFirst($S::Descendants, (New-Object $P ($A::NameProperty, [IO.Path]::GetFileNameWithoutExtension($name)))) }
    if (-not $item) { Start-Sleep -Milliseconds 200 }
}
if (-not $item) { "no item $name in Explorer"; exit 1 }
$r = $item.Current.BoundingRectangle
# On the name, near the bottom of the item: pressing a large-icon tile on its blank thumbnail area
# (Windows 10) starts a rubber-band selection instead of a drag; in a details row it is the row.
"at {0} {1}" -f [int]($r.Left + $r.Width / 2), [int]($r.Bottom - 6)
