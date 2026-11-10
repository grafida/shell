# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this> [args]`.
# The visible top-level windows of a class (CabinetWClass: Explorer), one per line: title, pid.
# A second argument `close` closes each one afterwards, through its own window pattern (as its close
# button would). Arguments are positional: `winvm.sh desktop` passes them in order.
param([Parameter(Mandatory)] [string] $Class, [string] $Then = '')
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$c = New-Object System.Windows.Automation.PropertyCondition ($A::ClassNameProperty, $Class)
foreach ($w in $A::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $c)) {
    "$($w.Current.Name) | pid $($w.Current.ProcessId)"
    if ($Then -eq 'close') { $w.GetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern).Close() }
}
