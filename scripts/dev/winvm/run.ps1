# Grafida Shell — development tooling.
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT licence; see LICENSE.
#
# Starts a program in the logged-on user's desktop session (scripts/dev/winvm.sh run runs this
# through an interactive scheduled task: a process started over SSH cannot show a window), with
# every per-user directory a Windows app writes to pointed at a fresh C:\winvm\run, so neither the
# app's data, nor WebView2's profile, nor the automation channel's file touch the real profile.
#
# winvm.sh writes C:\winvm\run.json beside this: {"program": "…", "args": ["…"], "env": {"K": "V"}}.
# Output: C:\winvm\run\{stdout,stderr}.txt and C:\winvm\run\pid.

$ErrorActionPreference = 'Stop'
$spec = Get-Content C:\winvm\run.json -Raw | ConvertFrom-Json
$root = 'C:\winvm\run'
Remove-Item -Recurse -Force $root -ErrorAction SilentlyContinue
foreach ($d in 'Roaming', 'Local', 'Temp') { New-Item -ItemType Directory -Force (Join-Path $root $d) | Out-Null }
$env:APPDATA = "$root\Roaming"
$env:LOCALAPPDATA = "$root\Local"
$env:TEMP = "$root\Temp"
$env:TMP = "$root\Temp"
if ($spec.env) {
    foreach ($p in $spec.env.PSObject.Properties) { Set-Item -Path "env:$($p.Name)" -Value $p.Value }
}
$arguments = @($spec.args | ForEach-Object { if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ } })
$start = @{
    FilePath               = $spec.program
    PassThru               = $true
    RedirectStandardOutput = "$root\stdout.txt"
    RedirectStandardError  = "$root\stderr.txt"
    WorkingDirectory       = Split-Path $spec.program
}
if ($arguments.Count) { $start.ArgumentList = $arguments }
$p = Start-Process @start
$p.Id | Set-Content "$root\pid"
