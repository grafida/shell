# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this> [args]`.
# Drives the next native file dialog a process opens, as a user would: waits for it, reports its
# title and buttons, then puts -Path in its name box and presses its default button (Open, Save,
# Select Folder), or presses Cancel. Positional: PROCESS_ID accept|cancel [PATH] [TIMEOUT].
#
# ⚠️ UI Automation's managed client exposes the dialog's classic Win32 controls (the name box, Open,
# Cancel) as bare panes with no patterns, so those are driven by their window handles: WM_SETTEXT
# into the box, BM_CLICK on the button (what a click on it sends). The DirectUI ones (a Save
# dialog's name box) take UIA's value pattern.
param([Parameter(Mandatory)] [int] $ProcessId, [ValidateSet('accept', 'cancel')] [string] $Action = 'accept', [string] $Path = '', [int] $Timeout = 30)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -Namespace W -Name D -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern System.IntPtr SendMessage(System.IntPtr h, uint m, System.IntPtr w, string l);
[DllImport("user32.dll")] public static extern bool PostMessage(System.IntPtr h, uint m, System.IntPtr w, System.IntPtr l);
'@
$A = [System.Windows.Automation.AutomationElement]
$P = [System.Windows.Automation.PropertyCondition]
$S = [System.Windows.Automation.TreeScope]
# A button of the dialog itself (a file-list item has an AutomationId of 1 too).
$button = { param($e, $i) $e.FindFirst($S::Children, (New-Object System.Windows.Automation.AndCondition (New-Object $P ($A::AutomationIdProperty, $i)), (New-Object $P ($A::ClassNameProperty, 'Button')))) }
$deadline = (Get-Date).AddSeconds($Timeout)
$dialog = $null
while (-not $dialog -and (Get-Date) -lt $deadline) {
    foreach ($w in $A::RootElement.FindAll($S::Children, (New-Object $P ($A::ProcessIdProperty, $ProcessId)))) {
        $d = if ($w.Current.ClassName -eq '#32770') { $w } else { $w.FindFirst($S::Children, (New-Object $P ($A::ClassNameProperty, '#32770'))) }
        # Built: its default button exists.
        if ($d -and (& $button $d '1')) { $dialog = $d; break }
    }
    if (-not $dialog) { Start-Sleep -Milliseconds 200 }
}
if (-not $dialog) { "no dialog within $Timeout s"; exit 1 }
Start-Sleep -Milliseconds 500
"title: $($dialog.Current.Name)"
$buttons = $dialog.FindAll($S::Children, (New-Object $P ($A::ClassNameProperty, 'Button')))
"buttons: " + (($buttons | % { $_.Current.Name }) -join ' | ')
$press = { param($e)
    $hwnd = [IntPtr]$e.Current.NativeWindowHandle
    if ($hwnd -ne [IntPtr]::Zero) { [void][W.D]::PostMessage($hwnd, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero) }
    else { $e.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke() }
}
if ($Action -eq 'cancel') {
    $b = & $button $dialog '2'
} else {
    $box = $null
    foreach ($i in '1148', '1001', '1152') {
        $candidates = $dialog.FindAll($S::Descendants, (New-Object $P ($A::AutomationIdProperty, $i)))
        $box = $candidates | ? { $_.Current.ClassName -eq 'Edit' -or $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Edit } | Select -First 1
        if ($box) { break }
    }
    if (-not $box) { "no name box"; exit 1 }
    $value = $null
    if ($box.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$value)) { $value.SetValue($Path) }
    else { [void][W.D]::SendMessage([IntPtr]$box.Current.NativeWindowHandle, 0x000C, [IntPtr]::Zero, $Path) }
    "name box ($($box.Current.AutomationId)): $Path"
    $b = & $button $dialog '1'
}
"pressing: $($b.Current.Name)"
& $press $b
"done"
