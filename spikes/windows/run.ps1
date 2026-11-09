# Runs the wry spike in the interactive session, collects its REPORT and the idle CPU of its tree.
$ErrorActionPreference = 'Stop'
$d = 'C:\spike'
Remove-Item "$d\out.txt","$d\err.txt","$d\result.json","$d\done.txt" -ErrorAction SilentlyContinue
$env:SPIKE_AUTOPICK = "$d\pick.bin"
$p = Start-Process -FilePath "$d\spike.exe" -PassThru -RedirectStandardOutput "$d\out.txt" -RedirectStandardError "$d\err.txt" -WorkingDirectory $d
$report = $null
for ($i = 0; $i -lt 120 -and -not $report; $i++) {
  Start-Sleep 1
  if (Test-Path "$d\out.txt") { $report = Get-Content "$d\out.txt" | Where-Object { $_ -like 'REPORT *' } | Select-Object -First 1 }
}
function Tree($id) {
  $all = Get-CimInstance Win32_Process
  $ids = @($id); $frontier = @($id)
  while ($frontier.Count) { $next = @($all | Where-Object { $frontier -contains $_.ParentProcessId } | ForEach-Object { $_.ProcessId }); $ids += $next; $frontier = $next }
  $ids
}
function Cpu($ids) { ($ids | ForEach-Object { (Get-Process -Id $_ -ErrorAction SilentlyContinue).TotalProcessorTime.TotalSeconds } | Measure-Object -Sum).Sum }
$settle = 60; $window = 60
Start-Sleep $settle
$ids = Tree $p.Id
$names = $ids | ForEach-Object { (Get-Process -Id $_ -ErrorAction SilentlyContinue).ProcessName }
$c0 = Cpu $ids; $t0 = Get-Date
Start-Sleep $window
$c1 = Cpu (Tree $p.Id); $t1 = Get-Date
$pct = [math]::Round(100 * ($c1 - $c0) / ($t1 - $t0).TotalSeconds, 3)
$imports = $null
[pscustomobject]@{
  report = if ($report) { ($report -replace '^REPORT ', '') | ConvertFrom-Json } else { $null }
  idleCpuPercentOfOneCore = $pct
  processCount = $ids.Count
  processes = ($names | Group-Object | ForEach-Object { "$($_.Name) x$($_.Count)" }) -join ', '
  exited = $p.HasExited
} | ConvertTo-Json -Depth 8 | Set-Content -Encoding utf8 "$d\result.json"
Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
'done' | Set-Content "$d\done.txt"
