# Grafida Shell — development tooling.
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT licence; see LICENSE.
#
# Brings a test guest up to date: Windows Update (software, not drivers), Defender's signatures,
# and Edge + the WebView2 runtime through EdgeUpdate. One pass; scripts/dev/winvm.sh update runs
# passes until nothing is left, rebooting between them. See docs/dev/windows-test-vm.md.
#
# ⚠️ It must run as SYSTEM through a scheduled task, never in the SSH session: the Windows Update
# Agent refuses to download or install from a network logon (E_ACCESSDENIED, 0x80070005).
#
# Writes C:\winvm\update.log (appended) and C:\winvm\update.json:
#   {"pass": "done", "installed": N, "failed": N, "remaining": N, "rebootRequired": bool}

$ErrorActionPreference = 'Stop'
$dir = 'C:\winvm'
$log = Join-Path $dir 'update.log'
$status = Join-Path $dir 'update.json'
function Say([string]$text) { Add-Content -Path $log -Value ("{0:u} {1}" -f (Get-Date), $text) }

Remove-Item $status -ErrorAction SilentlyContinue
$result = [ordered]@{ pass = 'failed'; installed = 0; failed = 0; remaining = 0; rebootRequired = $false; error = $null }
try {
    Say 'pass started'

    try {
        Update-MpSignature -ErrorAction Stop
        Say ("Defender signatures {0}" -f (Get-MpComputerStatus).AntivirusSignatureVersion)
    } catch { Say "Defender update failed: $_" }

    # EdgeUpdate: /ua runs the same check its own scheduled task does, for Edge and WebView2.
    $eu = "${env:ProgramFiles(x86)}\Microsoft\EdgeUpdate\MicrosoftEdgeUpdate.exe"
    if (Test-Path $eu) {
        Start-Process -FilePath $eu -ArgumentList '/ua', '/installsource', 'scheduler' -Wait
        Say 'EdgeUpdate /ua finished'
    }

    $session = New-Object -ComObject Microsoft.Update.Session
    $session.ClientApplicationID = 'grafida-winvm'
    $searcher = $session.CreateUpdateSearcher()
    $found = $searcher.Search("IsInstalled=0 and IsHidden=0 and Type='Software'").Updates
    Say "found $($found.Count) update(s)"
    $todo = New-Object -ComObject Microsoft.Update.UpdateColl
    foreach ($u in $found) {
        if (-not $u.EulaAccepted) { $u.AcceptEula() }
        Say "  - $($u.Title)"
        [void]$todo.Add($u)
    }
    if ($todo.Count -gt 0) {
        $dl = $session.CreateUpdateDownloader(); $dl.Updates = $todo; [void]$dl.Download()
        $inst = $session.CreateUpdateInstaller(); $inst.Updates = $todo
        $r = $inst.Install()
        for ($i = 0; $i -lt $todo.Count; $i++) {
            $code = $r.GetUpdateResult($i).ResultCode   # 2 succeeded, 3 succeeded with errors, 4 failed, 5 aborted
            if ($code -eq 2 -or $code -eq 3) { $result.installed++ } else { $result.failed++ }
            Say ("  result {0}: {1}" -f $code, $todo.Item($i).Title)
        }
        $result.rebootRequired = [bool]$r.RebootRequired
    }
    $result.remaining = $searcher.Search("IsInstalled=0 and IsHidden=0 and Type='Software'").Updates.Count
    $sysinfo = New-Object -ComObject Microsoft.Update.SystemInfo
    $result.rebootRequired = $result.rebootRequired -or [bool]$sysinfo.RebootRequired
    $result.pass = 'done'
    Say ("pass done: installed {0}, failed {1}, remaining {2}, reboot {3}" -f $result.installed, $result.failed, $result.remaining, $result.rebootRequired)
} catch {
    $result.error = "$_"
    Say "pass failed: $_"
}
$result | ConvertTo-Json | Set-Content -Path $status -Encoding utf8
