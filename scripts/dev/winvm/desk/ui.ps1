# Grafida Shell — development tooling. Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
# A desktop-session helper (grafida/grafida#91), run by `scripts/dev/winvm.sh desktop VM <this> [args]`.
# Reads and drives any window through UI Automation, for the screens nobody scripted for us (Edge's
# download warnings, SmartScreen, an installer's pages, Settings ▸ Apps), in whatever language the
# guest speaks: dump a window first, then act on a name it printed.
#
#   dump   TARGET [page] [DEPTH]   every top-level window of TARGET, and its controls to DEPTH (12):
#                                  type, "name", automation id, class, enabled/offscreen, rectangle.
#                                  A web page's content (a Document) is walked only with `page`.
#   invoke TARGET NAME [TIMEOUT]   waits up to TIMEOUT s (15) for an enabled control named NAME (a
#                                  -like pattern) and presses it as a user would: BM_CLICK for a
#                                  classic Button, else Invoke/Toggle/SelectionItem/ExpandCollapse,
#                                  else a real mouse click on its centre.
#   click  TARGET NAME [TIMEOUT]   the same, always with the real mouse.
#   wait   TARGET NAME [TIMEOUT]   waits for the control and prints it, pressing nothing.
#   set    TARGET NAME TIMEOUT VALUE   puts VALUE in the field named NAME (its value pattern), as
#                                  typing it would leave it.
#
# TARGET: a process name without .exe (msedge, smartscreen, an installer's file name), a pid, or *.
param(
    [Parameter(Mandatory)] [ValidateSet('dump', 'invoke', 'click', 'wait', 'set')] [string] $Mode,
    [Parameter(Mandatory)] [string] $Target,
    [string] $Name = '',
    [int] $Limit = 0,
    [string] $Value = ''
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type -Namespace W -Name U -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool PostMessage(System.IntPtr h, uint m, System.IntPtr w, System.IntPtr l);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint f, int x, int y, uint d, System.UIntPtr e);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint p);
[DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
[DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
'@
$A = [System.Windows.Automation.AutomationElement]
$CT = [System.Windows.Automation.ControlType]
$walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker

function Windows {
    $all = $A::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($w in $all) {
        $id = $w.Current.ProcessId
        if ($Target -eq '*') { $w; continue }
        if ($Target -match '^\d+$') { if ($id -eq [int]$Target) { $w }; continue }
        $p = Get-Process -Id $id -ErrorAction SilentlyContinue
        if ($p -and $p.ProcessName -like $Target) { $w }
    }
}

function Describe($e) {
    $c = $e.Current
    $r = $c.BoundingRectangle
    $rect = if ($r.IsEmpty) { '-' } else { '{0},{1} {2}x{3}' -f [int]$r.X, [int]$r.Y, [int]$r.Width, [int]$r.Height }
    $flags = @()
    if (-not $c.IsEnabled) { $flags += 'disabled' }
    if ($c.IsOffscreen) { $flags += 'offscreen' }
    '{0} "{1}"{2}{3} [{4}]{5}' -f $c.ControlType.ProgrammaticName.Replace('ControlType.', ''), $c.Name,
        $(if ($c.AutomationId) { " id=$($c.AutomationId)" } else { '' }),
        $(if ($c.ClassName) { " class=$($c.ClassName)" } else { '' }), $rect,
        $(if ($flags) { ' (' + ($flags -join ', ') + ')' } else { '' })
}

$script:lines = 0
function Walk($e, $depth, $max) {
    if ($script:lines -ge 600) { return }
    $script:lines++
    ('  ' * $depth) + (Describe $e)
    if ($depth -ge $max) { return }
    if ($e.Current.ControlType -eq $CT::Document -and $depth -gt 0 -and $Name -ne 'page') { ('  ' * ($depth + 1)) + '(page content not walked)'; return }
    $child = $walker.GetFirstChild($e)
    while ($child) { Walk $child ($depth + 1) $max; $child = $walker.GetNextSibling($child) }
}

function Find {
    foreach ($w in @(Windows)) {
        $hits = $w.FindAll([System.Windows.Automation.TreeScope]::Subtree, [System.Windows.Automation.Condition]::TrueCondition)
        foreach ($e in $hits) {
            $c = $e.Current
            if ($c.Name -like $Name -and $c.IsEnabled -and -not $c.IsOffscreen -and -not $c.BoundingRectangle.IsEmpty) { return , @($w, $e) }
        }
    }
    $null
}

function Foreground($hwnd) {
    $me = [W.U]::GetCurrentThreadId()
    $pid0 = [uint32]0
    $fg = [W.U]::GetWindowThreadProcessId([W.U]::GetForegroundWindow(), [ref]$pid0)
    [void][W.U]::AttachThreadInput($me, $fg, $true)
    [void][W.U]::SetForegroundWindow($hwnd)
    [void][W.U]::AttachThreadInput($me, $fg, $false)
}

function MouseClick($w, $e) {
    Foreground ([IntPtr]$w.Current.NativeWindowHandle)
    Start-Sleep -Milliseconds 300
    $r = $e.Current.BoundingRectangle
    $x = [int]($r.X + $r.Width / 2); $y = [int]($r.Y + $r.Height / 2)
    [void][W.U]::SetCursorPos($x, $y)
    Start-Sleep -Milliseconds 200
    [W.U]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 80; [W.U]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
    "real click at $x,$y"
}

if ($Mode -eq 'dump') {
    $max = if ($Limit -gt 0) { $Limit } else { 12 }
    $found = @(Windows)
    if (-not $found) { "no window for $Target"; exit 1 }
    foreach ($w in $found) {
        $p = Get-Process -Id $w.Current.ProcessId -ErrorAction SilentlyContinue
        "== window of $($p.ProcessName) (pid $($w.Current.ProcessId), hwnd $($w.Current.NativeWindowHandle))"
        Walk $w 0 $max
    }
    exit 0
}

$timeout = if ($Limit -gt 0) { $Limit } else { 15 }
$deadline = (Get-Date).AddSeconds($timeout)
$hit = $null
while (-not $hit -and (Get-Date) -lt $deadline) {
    $hit = Find
    if (-not $hit) { Start-Sleep -Milliseconds 300 }
}
if (-not $hit) { "no enabled control named '$Name' in $Target within $timeout s"; exit 1 }
$w, $e = $hit
"found: " + (Describe $e)
if ($Mode -eq 'wait') { exit 0 }
if ($Mode -eq 'set') {
    $pattern = $null
    if (-not $e.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) { "no value pattern"; exit 1 }
    $e.SetFocus(); $pattern.SetValue($Value); "set to '$Value'"; exit 0
}
if ($Mode -eq 'click') { MouseClick $w $e; exit 0 }

$c = $e.Current
$pattern = $null
if ($c.ClassName -eq 'Button' -and $c.NativeWindowHandle -ne 0) {
    [void][W.U]::PostMessage([IntPtr]$c.NativeWindowHandle, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero); 'BM_CLICK'
} elseif ($e.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$pattern)) {
    $pattern.Invoke(); 'Invoke'
} elseif ($e.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
    $pattern.Toggle(); 'Toggle'
} elseif ($e.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
    $pattern.Select(); 'Select'
} elseif ($e.TryGetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern, [ref]$pattern)) {
    $pattern.Expand(); 'Expand'
} else {
    MouseClick $w $e
}
