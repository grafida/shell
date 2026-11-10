# Grafida Shell — development tooling.
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT licence; see LICENSE.
#
# Installs what the dev-mode work needs in a test guest (grafida/grafida#91): PHP (the dev worker,
# `php bin/worker.php`, and tests/Pipe), Git for Windows (Git Bash, for phpmicro's
# check-windows-sfx.sh) and Python (the shell's fake worker in tests/apps). Never run on a `clean`
# snapshot: this is what makes the `dev` one. scripts/dev/winvm.sh dev-tools downloads the
# installers on the host, checks them against the pins there, copies them to C:\winvm\dl and runs
# this, which checks the pins again before running anything.
param([Parameter(Mandatory)] [string] $Pins)   # "file=sha256,file=sha256,…"

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$dl = 'C:\winvm\dl'

foreach ($pin in $Pins -split ',') {
    $file, $want = $pin -split '=', 2
    $got = (Get-FileHash (Join-Path $dl $file) -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $want) { throw "$file is $got, pinned $want" }
    Write-Host "pinned: $file"
}
$files = $Pins -split ',' | ForEach-Object { ($_ -split '=', 2)[0] }
$php = $files | Where-Object { $_ -like 'php-*.zip' }
$git = $files | Where-Object { $_ -like 'Git-*.exe' }
$py  = $files | Where-Object { $_ -like 'python-*.exe' }

# Git for Windows: silent, for all users, Git Bash included.
$p = Start-Process (Join-Path $dl $git) -ArgumentList '/VERYSILENT', '/NORESTART', '/NOCANCEL', '/SP-', '/SUPPRESSMSGBOXES' -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "Git for Windows exited $($p.ExitCode)" }
Write-Host 'installed: Git for Windows'

# Python: for all users, on PATH.
$p = Start-Process (Join-Path $dl $py) -ArgumentList '/quiet', 'InstallAllUsers=1', 'PrependPath=1', 'Include_test=0' -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "Python exited $($p.ExitCode)" }
Write-Host 'installed: Python'

# PHP: unpacked to C:\tools\php, with the extensions the worker SFX is built with.
$dest = 'C:\tools\php'
Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue
Expand-Archive (Join-Path $dl $php) $dest
$ini = Get-Content (Join-Path $dest 'php.ini-development')
$ini += ''
$ini += '; grafida/grafida#91: the worker SFX''s extensions (phpmicro build-sfx.yml EXTENSIONS)'
$ini += 'extension_dir = "ext"'
foreach ($e in 'curl', 'ffi', 'mbstring', 'openssl', 'pdo_sqlite', 'sqlite3', 'sodium', 'sockets', 'shmop') {
    $ini += "extension=$e"
}
$ini += 'ffi.enable = true'
$ini += 'memory_limit = 1G'
$ini | Set-Content -Encoding ascii (Join-Path $dest 'php.ini')
$path = [Environment]::GetEnvironmentVariable('Path', 'Machine')
if ($path -notlike "*$dest*") { [Environment]::SetEnvironmentVariable('Path', "$path;$dest", 'Machine') }
Write-Host 'installed: PHP'
& (Join-Path $dest 'php.exe') -r 'echo PHP_VERSION, " ", implode(",", get_loaded_extensions()), PHP_EOL;'
