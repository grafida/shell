# Grafida Shell — development tooling.
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT licence; see LICENSE.
#
# Idle CPU of a running app's whole process tree, per process (grafida/grafida#91): the shell, its
# worker and the WebView2 processes it started. For the shipped release binary, measured on the
# Articles screen with a site and the focus out of every field (a lesson from #86 and #88).
#
#   idle-cpu.ps1 -ProcessId <the shell's pid> [-Settle 60] [-Sample 60]
#
# Prints "total <percent of one core>" and one "<name> <percent>" line per process name,
# invariant-culture numbers (a Greek guest writes 0,94).

param(
    [Parameter(Mandatory)] [int] $ProcessId,
    [int] $Settle = 60,
    [int] $Sample = 60
)
$ErrorActionPreference = 'Stop'
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture

Start-Sleep $Settle
$all = Get-CimInstance Win32_Process
$ids = @($ProcessId); $frontier = @($ProcessId)
while ($frontier.Count) {
    $next = @($all | Where-Object { $frontier -contains $_.ParentProcessId } | ForEach-Object { $_.ProcessId })
    $ids += $next; $frontier = $next
}
function Measure-Tree {
    $h = @{}
    foreach ($i in $ids) {
        $p = Get-Process -Id $i -ErrorAction SilentlyContinue
        if ($p) { $h[$i] = @($p.ProcessName, $p.TotalProcessorTime.TotalSeconds) }
    }
    $h
}
$a = Measure-Tree
$clock = [Diagnostics.Stopwatch]::StartNew()
Start-Sleep $Sample
$b = Measure-Tree
$seconds = $clock.Elapsed.TotalSeconds
$by = @{}; $total = 0.0
foreach ($i in $b.Keys) {
    if ($a.ContainsKey($i)) {
        $d = 100 * ($b[$i][1] - $a[$i][1]) / $seconds
        $total += $d
        $by[$b[$i][0]] += $d
    }
}
'total {0:N2}' -f $total
$by.GetEnumerator() | Sort-Object Value -Descending | ForEach-Object { '{0} {1:N2}' -f $_.Key, $_.Value }
