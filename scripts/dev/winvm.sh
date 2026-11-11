#!/usr/bin/env bash
# Grafida Shell — the Windows test VMs (grafida/grafida#91; set up as in docs/dev/windows-test-vm.md).
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# Usage: scripts/dev/winvm.sh COMMAND VM [ARGS…]
#   start VM                 boot it with quickemu (a window on this desktop), then wait for SSH
#   wait VM                  wait until SSH answers
#   ssh VM [CMD…]            a PowerShell session or command (as the auto-logon user)
#   stop VM                  shut Windows down and wait until QEMU has exited
#   snapshot VM create|apply|delete|list [NAME]   internal qcow2 snapshots (the VM must be off)
#   update VM                Windows Update + Defender + Edge/WebView2, pass after pass, rebooting
#                            between them, until nothing is left (scripts/dev/winvm/update.ps1)
#   reboot VM                restart Windows and wait until it is back (a new boot time)
#   versions VM              what a clean-machine record notes: Windows, Edge, WebView2, Defender,
#                            Smart App Control, locale
#   get VM GUEST HOST        copy a file out of the guest
#   put VM HOST GUEST        copy a file into the guest
#   desktop VM SCRIPT [ARG…] run a PowerShell script in the logged-on user's desktop session (the
#                            SSH session sees no window) and print its output; scripts/dev/winvm/desk/
#   clean-up VM              remove C:\winvm and its scheduled tasks (before a `clean` snapshot)
#   dev-tools VM             PHP, Git for Windows and Python, pinned (what makes a `dev` snapshot;
#                            never on `clean`): scripts/dev/winvm/dev-tools.ps1
#   push VM SRC DEST         copy a host directory to a guest path (a tarball, unpacked there)
#   run VM PROGRAM [ARG…]    start a guest program in the desktop session with APPDATA,
#                            LOCALAPPDATA and TEMP in a fresh C:\winvm\run (WINVM_ENV='K=V K2=V2'
#                            adds variables); prints its pid. Output: C:\winvm\run\std{out,err}.txt
#                            WINVM_KEEP=1 keeps the previous run's data (a restart of the same app)
#   ps VM SCRIPT             run one PowerShell script in the guest (as -EncodedCommand)
#   idle VM PID [SETTLE] [SAMPLE]   idle CPU of PID's process tree, per process (winvm/idle-cpu.ps1;
#                            default 60 s after 60 s)
#   stop-run VM              kill what `run` started, and everything it started
#   watch-start VM / watch-stop VM   record every console window shown on the guest desktop
#                            (winvm/watch.ps1); watch-stop prints them, one per line (none = none)
#   tunnel VM [APP_ID]       wait for the shell's automation channel in the guest, forward its port
#                            and write the runtime file here; prints the XDG_RUNTIME_DIR to export
#                            (with SHELL_AUTOMATION_REMOTE=1) for scripts/dev/automation.py
#
# VMs: windows-11 (port 22220, ssh host `winvm`) and windows-10 (port 22224, `winvm10`). They live
# in $WINVM_DIR (default ~/VMs), next to the Linux ones (linuxvm.sh uses 22221–22223).

set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
VMS=${WINVM_DIR:-$HOME/VMs}

die() { echo "winvm: $*" >&2; exit 1; }

# The dev tooling, pinned by the SHA-256 of the downloaded bytes (checked here and again in the
# guest). Bump them only with a hash computed from what was downloaded, never typed.
DEV_TOOLS=(
    "https://downloads.php.net/~windows/releases/php-8.4.27-1-nts-Win32-vs17-x64.zip dbae154a0a89791f36c3d48e68bae797878333e7d6cb5721e74617de59b63ab7"
    "https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.4/Git-2.56.0.4-64-bit.exe 4961a4a8cc778d8b6e00a5819e5fe4d078b754555aff7aedc2d77a5d74c2de80"
    "https://www.python.org/ftp/python/3.15.2/python-3.15.2-amd64.exe 06e225a228191b0f4d1dcf6905fbb21bf9f6970d13a2ec63137ab9bee76f15aa"
)
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/grafida-winvm

port() {
    case $1 in
        windows-11) echo 22220 ;;
        windows-10) echo 22224 ;;
        *) die "unknown VM '$1' (windows-11, windows-10)" ;;
    esac
}
host() { case $1 in windows-11) echo winvm ;; windows-10) echo winvm10 ;; esac; }
rssh() { local vm=$1; shift; ssh -o ConnectTimeout=10 "$(host "$vm")" "$@"; }

# Runs a PowerShell script in the guest. ⚠️ Windows 10's OpenSSH (7.7, the optional feature) strips
# the double quotes out of the command it hands its shell, so a script never travels as text:
# -EncodedCommand (UTF-16LE, base64) has nothing in it to lose.
# The program `run` started, if it is still running: the process with the recorded pid, provided it
# started before the pid file was written. ⚠️ Windows hands a dead process's pid to a new one quickly
# (seen on Windows 10), and a bare pid check then calls an unrelated process ours: `run` refused to
# start, and `stop-run` would have killed that process's tree.
# ⚠️ With 2 s of slack: the pid file can be written a fraction of a millisecond before Windows
# records the process's start time (seen on Windows 10), and a reused pid starts far later anyway.
OURS='function Ours { if (-not (Test-Path C:\winvm\run\pid)) { return $null }; $f = Get-Item C:\winvm\run\pid; $p = Get-Process -Id ([int](Get-Content $f).Trim()) -ErrorAction SilentlyContinue; if ($p -and $p.StartTime -le $f.LastWriteTime.AddSeconds(2)) { $p } else { $null } }'

rps() {
    local vm=$1 script=$2 encoded
    encoded=$(printf "\$ProgressPreference = 'SilentlyContinue'\n[Console]::OutputEncoding = [Text.Encoding]::UTF8\n%s" "$script" | iconv -f UTF-8 -t UTF-16LE | base64 -w0)
    rssh "$vm" "powershell -NoProfile -NonInteractive -EncodedCommand $encoded"
}

# quickemu writes the QEMU pid to <vm>/<vm>.pid and leaves the file behind after a shutdown.
running() {
    local pid
    pid=$(cat "$VMS/$1/$1.pid" 2>/dev/null) || return 1
    [ -n "$pid" ] && [ "$(ps -o comm= -p "$pid" 2>/dev/null)" != "" ] \
        && ps -o args= -p "$pid" | grep -q qemu-system
}

wait_ssh() {
    local vm=$1 i
    for i in $(seq 1 120); do
        rssh "$vm" 'exit 0' >/dev/null 2>&1 && return 0
        sleep 5
    done
    die "$vm: no SSH after 10 minutes"
}

boot_time() { rps "$1" '(Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToString("o")' 2>/dev/null | tr -d '\r'; }

# ⚠️ sshd keeps answering while Windows shuts down, so "SSH answers" right after a restart may still
# be the old boot: wait until the guest reports a different boot time.
reboot() {
    local vm=$1 before i now
    before=$(boot_time "$vm")
    rssh "$vm" 'Restart-Computer -Force' >/dev/null 2>&1 || true
    for i in $(seq 1 120); do
        sleep 5
        now=$(boot_time "$vm") || true
        [ -n "$now" ] && [ "$now" != "$before" ] && { settle "$vm"; return 0; }
    done
    die "$vm: did not come back from a restart within 10 minutes"
}

# ⚠️ A cumulative update installs over two boots: the first one answers SSH, then Windows restarts by
# itself to finish. Back means the same boot time for 2 minutes (a pass started in between died with
# the second boot, and its host waited 2 hours for a result that never came).
settle() {
    local vm=$1 last now same=0 i
    last=$(boot_time "$vm") || true
    for i in $(seq 1 120); do
        sleep 10
        now=$(boot_time "$vm") || true
        if [ -n "$now" ] && [ "$now" = "$last" ]; then
            same=$((same + 1))
            [ $same -ge 12 ] && return 0
        else
            same=0
            [ -n "$now" ] && last=$now
        fi
    done
    die "$vm: kept restarting for 20 minutes"
}

wait_off() {
    local vm=$1 i
    for i in $(seq 1 120); do
        running "$vm" || return 0
        sleep 5
    done
    die "$vm: QEMU still running after 10 minutes"
}

# One update pass as SYSTEM (the Windows Update Agent refuses a network logon), then its JSON status.
update_pass() {
    local vm=$1 i boot
    settle "$vm"
    boot=$(boot_time "$vm")
    rssh "$vm" 'New-Item -ItemType Directory -Force C:\winvm | Out-Null'
    scp -q "$HERE/winvm/update.ps1" "$(host "$vm"):C:/winvm/update.ps1"
    rps "$vm" 'schtasks /Create /TN winvm-update /RU SYSTEM /SC ONCE /ST 23:59 /RL HIGHEST /F /TR "powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\winvm\update.ps1" | Out-Null; schtasks /Run /TN winvm-update | Out-Null' >&2
    for i in $(seq 1 720); do
        sleep 10
        # Restarted under the pass (Windows finishing an update by itself): the pass is gone, so run
        # it again rather than wait for its result.
        now=$(boot_time "$vm") || true
        if [ -n "$now" ] && [ "$now" != "$boot" ]; then
            echo "== the guest restarted during the pass; running it again" >&2
            update_pass "$vm"
            return
        fi
        if rssh "$vm" 'Test-Path C:\winvm\update.json' 2>/dev/null | grep -q True; then
            rssh "$vm" 'Get-Content C:\winvm\update.json -Raw'
            return 0
        fi
    done
    die "$vm: an update pass did not finish within 2 hours"
}

versions() {
    rps "$1" '
        $v = Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion"
        $os = Get-CimInstance Win32_OperatingSystem
        "Windows:  $($os.Caption) $($v.DisplayVersion), build $($v.CurrentBuild).$($v.UBR)"
        "WebView2: " + (Get-ItemProperty "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" -ErrorAction SilentlyContinue).pv
        "Edge:     " + (Get-Item "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe" -ErrorAction SilentlyContinue).VersionInfo.ProductVersion
        $s = Get-MpComputerStatus
        "Defender: engine $($s.AMEngineVersion), signatures $($s.AntivirusSignatureVersion) of $($s.AntivirusSignatureLastUpdated)"
        $sac = (Get-ItemProperty HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy -ErrorAction SilentlyContinue).VerifiedAndReputablePolicyState
        "Smart App Control: " + @{0 = "off"; 1 = "on"; 2 = "evaluation"}[[int]$sac]
        "Locale:   $((Get-Culture).Name), UI $((Get-UICulture).Name)"'
}

[ $# -ge 2 ] || { sed -n "/^# Usage/,/^$/p" "$0"; exit 64; }
cmd=$1 vm=$2; shift 2
port "$vm" >/dev/null

case $cmd in
    start)
        running "$vm" && die "$vm is already running"
        # Both streams to a file: QEMU keeps quickemu's stderr open for as long as the VM runs, so
        # `winvm.sh start VM | tail` used to wait for the VM to shut down.
        log="$VMS/$vm/start.log"
        (cd "$VMS" && quickemu --vm "$vm.conf" --ssh-port "$(port "$vm")" >"$log" 2>&1) || { cat "$log" >&2; die "quickemu failed"; }
        wait_ssh "$vm"
        ;;
    wait) wait_ssh "$vm" ;;
    ssh) exec ssh "$(host "$vm")" "$@" ;;
    stop)
        rssh "$vm" 'Stop-Computer -Force' >/dev/null 2>&1 || true
        wait_off "$vm"
        ;;
    snapshot)
        running "$vm" && die "$vm is running: snapshots are of the disk, power it off first"
        disk="$VMS/$vm/disk.qcow2"
        case ${1:-list} in
            create) qemu-img snapshot -c "${2:?name}" "$disk" ;;
            apply) qemu-img snapshot -a "${2:?name}" "$disk" ;;
            delete) qemu-img snapshot -d "${2:?name}" "$disk" ;;
            list) qemu-img snapshot -l "$disk" ;;
            *) die "snapshot create|apply|delete|list [NAME]" ;;
        esac
        ;;
    update)
        for pass in $(seq 1 10); do
            echo "== pass $pass" >&2
            status=$(update_pass "$vm")
            echo "$status" >&2
            reboot=$(python3 -I -c 'import json,sys; print(json.loads(sys.stdin.read().lstrip("\ufeff"))["rebootRequired"])' <<<"$status")
            remaining=$(python3 -I -c 'import json,sys; s=json.loads(sys.stdin.read().lstrip("\ufeff")); print(s["remaining"] + s["installed"] + s["failed"])' <<<"$status")
            if [ "$reboot" = True ]; then
                echo "== rebooting" >&2
                reboot "$vm"
            elif [ "$remaining" = 0 ]; then
                echo "== up to date" >&2
                versions "$vm"
                exit 0
            fi
        done
        die "$vm: still not up to date after 10 passes"
        ;;
    versions) versions "$vm" ;;
    reboot) reboot "$vm" ;;
    desktop)
        # A guest script in the logged-on user's desktop session, where windows exist (the SSH
        # session sees none): UI Automation, the clipboard, screenshots, the cursor, the registry
        # of the user who is logged on. Started through the headless console host, so its own
        # PowerShell never shows a window. Prints what the script printed.
        script=${1:?script}; shift
        name=$(basename "$script" .ps1)
        rssh "$vm" 'New-Item -ItemType Directory -Force C:\winvm\desk | Out-Null'
        scp -q "$script" "$(host "$vm"):C:/winvm/desk/$name.ps1"
        args_json=$(python3 -I -c 'import json, sys; print(json.dumps(sys.argv[1:]))' "$@")
        printf '%s' "$args_json" | rssh "$vm" "\$input | Set-Content -Encoding utf8 C:\\winvm\\desk\\$name.args.json"
        rps "$vm" "Remove-Item C:\\winvm\\desk\\$name.out, C:\\winvm\\desk\\$name.done -ErrorAction SilentlyContinue
\$runner = 'C:\\winvm\\desk\\$name.run.ps1'
Set-Content -Encoding utf8 \$runner @'
\$a = [object[]](Get-Content -Raw C:\\winvm\\desk\\$name.args.json | ConvertFrom-Json)
try { & C:\\winvm\\desk\\$name.ps1 @a *> C:\\winvm\\desk\\$name.out } catch { \$_ | Out-String | Add-Content C:\\winvm\\desk\\$name.out }
Set-Content C:\\winvm\\desk\\$name.done done
'@
schtasks /Create /TN winvm-desk-$name /TR \"conhost.exe --headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File \$runner\" /SC ONCE /ST 23:59 /IT /RU \$env:USERNAME /F | Out-Null
schtasks /Run /TN winvm-desk-$name | Out-Null
for (\$i = 0; \$i -lt 1800 -and -not (Test-Path C:\\winvm\\desk\\$name.done); \$i++) { Start-Sleep -Milliseconds 100 }
schtasks /Delete /TN winvm-desk-$name /F | Out-Null
if (Test-Path C:\\winvm\\desk\\$name.out) { Get-Content C:\\winvm\\desk\\$name.out }
if (-not (Test-Path C:\\winvm\\desk\\$name.done)) { throw 'the desktop script did not finish within 3 minutes' }"
        ;;
    put)
        # A host file into the guest.
        from=${1:?host path}; guest=${2:?guest path}
        rps "$vm" "New-Item -ItemType Directory -Force (Split-Path -Parent '$guest') | Out-Null"
        scp -q "$from" "$(host "$vm"):${guest//\\//}"
        ;;
    get)
        # A guest file to the host (a screenshot a desktop helper wrote).
        guest=${1:?guest path}; to=${2:?host path}
        scp -q "$(host "$vm"):${guest//\\//}" "$to"
        ;;
    clean-up)
        rps "$vm" 'schtasks /Query /FO CSV | ConvertFrom-Csv | Where-Object { $_.TaskName -like "\winvm-*" } | ForEach-Object { schtasks /Delete /TN $_.TaskName /F | Out-Null }; Remove-Item -Recurse -Force C:\winvm -ErrorAction SilentlyContinue; "cleaned"'
        ;;
    dev-tools)
        mkdir -p "$CACHE"
        pins=()
        rssh "$vm" 'New-Item -ItemType Directory -Force C:\winvm\dl | Out-Null'
        for entry in "${DEV_TOOLS[@]}"; do
            url=${entry% *} sha=${entry#* } file=${url##*/}
            if [ ! -f "$CACHE/$sha" ]; then
                curl -fsSL -o "$CACHE/$sha.part" "$url" && mv "$CACHE/$sha.part" "$CACHE/$sha"
            fi
            [ "$(sha256sum "$CACHE/$sha" | cut -d' ' -f1)" = "$sha" ] || { rm -f "$CACHE/$sha"; die "$file does not match its pin"; }
            scp -q "$CACHE/$sha" "$(host "$vm"):C:/winvm/dl/$file"
            pins+=("$file=$sha")
        done
        scp -q "$HERE/winvm/dev-tools.ps1" "$(host "$vm"):C:/winvm/dev-tools.ps1"
        rssh "$vm" "powershell -NoProfile -ExecutionPolicy Bypass -File C:\winvm\dev-tools.ps1 -Pins $(IFS=,; echo "${pins[*]}")"
        ;;
    push)
        src=${1:?source directory} dest=${2:?guest path, e.g. C:\src\grafida}
        tarball=$(mktemp --suffix=.tar)
        # Plain ustar, which Windows' own tar.exe (bsdtar) unpacks; nothing else is needed there.
        tar -C "$src" --exclude=.git --exclude=node_modules --exclude=./build --exclude=./target -cf "$tarball" .
        rssh "$vm" 'New-Item -ItemType Directory -Force C:\winvm | Out-Null'
        scp -q "$tarball" "$(host "$vm"):C:/winvm/push.tar"
        rm -f "$tarball"
        rps "$vm" "Remove-Item -Recurse -Force '$dest' -ErrorAction SilentlyContinue; New-Item -ItemType Directory -Force '$dest' | Out-Null; tar -xf C:\winvm\push.tar -C '$dest'; if (\$LASTEXITCODE) { exit \$LASTEXITCODE }; Remove-Item C:\winvm\push.tar"
        ;;
    run)
        program=${1:?guest program}; shift
        spec=$(python3 -I -c 'import json, os, sys
env = dict(kv.split("=", 1) for kv in os.environ.get("WINVM_ENV", "").split() if "=" in kv)
print(json.dumps({"program": sys.argv[1], "args": sys.argv[2:], "env": env, "keep": os.environ.get("WINVM_KEEP") == "1"}))' "$program" "$@")
        rssh "$vm" 'New-Item -ItemType Directory -Force C:\winvm | Out-Null'
        printf '%s' "$spec" | rssh "$vm" '$input | Set-Content -Encoding utf8 C:\winvm\run.json'
        scp -q "$HERE/winvm/run.ps1" "$(host "$vm"):C:/winvm/run.ps1"
        # One at a time: a second program would delete the first one's C:\winvm\run, and two
        # WebView2 apps on one profile share a browser process (killing one kills both webviews).
        rps "$vm" "$OURS"'
if (Ours) { throw "the previous program is still running: winvm.sh stop-run first" }
Remove-Item C:\winvm\run\pid -ErrorAction SilentlyContinue
schtasks /Create /TN winvm-run /TR "conhost.exe --headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\winvm\run.ps1" /SC ONCE /ST 23:59 /IT /RU $env:USERNAME /F | Out-Null
schtasks /Run /TN winvm-run | Out-Null
for ($i = 0; $i -lt 600 -and -not (Test-Path C:\winvm\run\pid); $i++) { Start-Sleep -Milliseconds 100 }
schtasks /Delete /TN winvm-run /F | Out-Null
if (-not (Test-Path C:\winvm\run\pid)) { throw "the program did not start" }
(Get-Content C:\winvm\run\pid).Trim()'
        ;;
    ps) rps "$vm" "${1:?script}" ;;
    idle)
        rssh "$vm" 'New-Item -ItemType Directory -Force C:\winvm | Out-Null'
        scp -q "$HERE/winvm/idle-cpu.ps1" "$(host "$vm"):C:/winvm/idle-cpu.ps1"
        rssh "$vm" "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\winvm\\idle-cpu.ps1 -ProcessId ${1:?pid} -Settle ${2:-60} -Sample ${3:-60}"
        ;;
    stop-run)
        # Waits until it is gone: the next `run` refuses while it is still exiting.
        rps "$vm" "$OURS"'
$p = Ours; if ($p) { taskkill /T /F /PID $p.Id 2>&1 | Out-Null; for ($i = 0; $i -lt 100 -and (Ours); $i++) { Start-Sleep -Milliseconds 100 } }; "stopped"'
        ;;
    watch-start)
        rssh "$vm" 'New-Item -ItemType Directory -Force C:\winvm | Out-Null'
        scp -q "$HERE/winvm/watch.ps1" "$(host "$vm"):C:/winvm/watch.ps1"
        rps "$vm" 'Remove-Item C:\winvm\watch.log -ErrorAction SilentlyContinue
schtasks /Create /TN winvm-watch /TR "conhost.exe --headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\winvm\watch.ps1" /SC ONCE /ST 23:59 /IT /RU $env:USERNAME /F | Out-Null
schtasks /Run /TN winvm-watch | Out-Null
for ($i = 0; $i -lt 600 -and -not (Test-Path C:\winvm\watch.log); $i++) { Start-Sleep -Milliseconds 100 }
schtasks /Delete /TN winvm-watch /F | Out-Null
if (-not (Test-Path C:\winvm\watch.log)) { throw "the watcher did not start" }
"watching"'
        ;;
    watch-stop)
        rps "$vm" 'New-Item -ItemType File -Force C:\winvm\watch.stop | Out-Null
for ($i = 0; $i -lt 100 -and -not (Select-String -Quiet -Path C:\winvm\watch.log -Pattern "^# stopped"); $i++) { Start-Sleep -Milliseconds 100 }
Get-Content C:\winvm\watch.log | Where-Object { $_ -and $_ -notmatch "^# stopped" }'
        ;;
    tunnel)
        app=${1:-app.grafida}
        local_dir=$(mktemp -d -t winvm-automation-XXXXXX)
        json=""
        for i in $(seq 1 120); do
            json=$(rps "$vm" "if (Test-Path C:\winvm\run\Temp\\$app.automation.json) { Get-Content -Raw C:\winvm\run\Temp\\$app.automation.json }" 2>/dev/null | tr -d '\r')
            [ -n "$json" ] && break
            sleep 1
        done
        [ -n "$json" ] || die "no automation channel in the guest (was the shell started with --automation?)"
        guest_port=$(python3 -I -c 'import json,sys; print(json.loads(sys.argv[1])["port"])' "$json")
        local_port=$(python3 -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
        # Its output goes nowhere: with -f the forwarder keeps it open, and a caller capturing this
        # command's output would otherwise wait for the tunnel to end.
        ssh -f -N -o ExitOnForwardFailure=yes -L "127.0.0.1:$local_port:127.0.0.1:$guest_port" "$(host "$vm")" >/dev/null 2>&1 </dev/null
        python3 -I -c 'import json,sys; d=json.loads(sys.argv[1]); d["port"]=int(sys.argv[2]); print(json.dumps(d))' "$json" "$local_port" >"$local_dir/$app.automation.json"
        chmod 600 "$local_dir/$app.automation.json"
        echo "$local_dir"
        ;;
    *) die "unknown command '$cmd'" ;;
esac
