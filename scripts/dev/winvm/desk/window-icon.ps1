# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this> [args]`.
# The icons a process's window shows (WM_GETICON: the title bar's small one, Alt+Tab's big one),
# the big one compared with ICO's 32 px image. Prints `big=True|False|none small=yes|none`.
# Positional: PROCESS_ID ICO [TIMEOUT].
param([Parameter(Mandatory)] [int] $ProcessId, [Parameter(Mandatory)] [string] $Ico, [int] $Timeout = 20)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing
Add-Type -Namespace W -Name I -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SendMessage(System.IntPtr h, uint m, System.IntPtr w, System.IntPtr l);
'@
$A = [System.Windows.Automation.AutomationElement]
$condition = New-Object System.Windows.Automation.PropertyCondition ($A::ProcessIdProperty, $ProcessId)
$deadline = (Get-Date).AddSeconds($Timeout)
$win = $null
while (-not $win -and (Get-Date) -lt $deadline) {
    $win = $A::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Children, $condition)
    if (-not $win) { Start-Sleep -Milliseconds 300 }
}
if (-not $win) { "no window for pid $ProcessId"; exit 1 }
$hwnd = [IntPtr]$win.Current.NativeWindowHandle
# WM_GETICON = 0x7F; ICON_BIG = 1, ICON_SMALL = 0.
$big = [W.I]::SendMessage($hwnd, 0x7F, [IntPtr]1, [IntPtr]::Zero)
$small = [W.I]::SendMessage($hwnd, 0x7F, [IntPtr]0, [IntPtr]::Zero)
$hash = { param($b) $m = New-Object IO.MemoryStream; $b.Save($m, [System.Drawing.Imaging.ImageFormat]::Png); [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($m.ToArray())) }
$want = & $hash ([System.Drawing.Icon]::new($Ico, 32, 32).ToBitmap())
$bigOk = if ($big -eq [IntPtr]::Zero) { 'none' } else { (& $hash ([System.Drawing.Icon]::FromHandle($big).ToBitmap())) -eq $want }
'big={0} small={1}' -f $bigOk, $(if ($small -eq [IntPtr]::Zero) { 'none' } else { 'yes' })
