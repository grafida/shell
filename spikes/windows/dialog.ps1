param([string]$Tag)
$d = 'C:\spike'
Remove-Item "$d\$Tag.out","$d\$Tag.err","$d\$Tag.done" -ErrorAction SilentlyContinue
Remove-Item Env:\SPIKE_AUTOPICK -ErrorAction SilentlyContinue
$env:SPIKE_QUIT_AFTER_REPORT = '1'
Start-Process -FilePath "$d\spike.exe" -Wait -RedirectStandardOutput "$d\$Tag.out" -RedirectStandardError "$d\$Tag.err" -WorkingDirectory $d
'done' | Set-Content "$d\$Tag.done"
