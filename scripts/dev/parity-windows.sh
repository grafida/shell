#!/usr/bin/env bash
# Grafida Shell — the automatable half of the Windows parity checklist (grafida/grafida#82, #91).
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# Runs on the HOST and drives a Windows guest (scripts/dev/winvm.sh, docs/dev/windows-test-vm.md):
# the debug shell with the automation channel runs in the guest's desktop session, against the
# test apps in tests/apps/ and against Grafida, with APPDATA, LOCALAPPDATA and TEMP in the guest's
# C:\winvm\run, and the channel is tunnelled here. It prints one PASS/FAIL line per check plus a
# Markdown table, like parity-linux.sh and parity-macos.sh.
#
#   scripts/dev/parity-windows.sh                    # windows-11, Grafida in dev mode from C:\src\grafida
#   WINVM=windows-10 QUICK=1 scripts/dev/parity-windows.sh
#   GRAFIDA_PACKAGED='C:\Users\Quickemu\AppData\Local\Programs\Grafida' scripts/dev/parity-windows.sh
#
# Environment:
#   WINVM            the guest (default windows-11)
#   SHELL_BIN        the debug automation shell to copy into the guest (default: this tree's
#                    target/x86_64-pc-windows-msvc/debug/grafida-shell.exe, built with
#                    `cargo xwin build --target x86_64-pc-windows-msvc --features automation`)
#   GRAFIDA_GUEST    Grafida's source tree in the guest, for dev mode (default C:\src\grafida; put it
#                    there with `winvm.sh push`); its dev worker is `php bin/worker.php` (dev-tools)
#   GRAFIDA_PACKAGED an installed Grafida in the guest: it is copied to C:\winvm\packaged and only its
#                    shell is swapped for SHELL_BIN (a release shell has no automation channel)
#   QUICK=1          a 10 s idle-CPU sample instead of 60 s after 60 s
#
# The guest needs the `dev` snapshot's Python (the fake worker, the database seed). The checks that
# need a human (the context menu, spelling, real dialogs, a real drag from Explorer, the title bar,
# AI streaming, …) are in the issue; this script does not pretend to cover them.

set -uo pipefail

SHELL_DIR=$(cd "$(dirname "$0")/../.." && pwd)
VM=${WINVM:-windows-11}
W="$SHELL_DIR/scripts/dev/winvm.sh"
BIN=${SHELL_BIN:-$SHELL_DIR/target/x86_64-pc-windows-msvc/debug/grafida-shell.exe}
GG=${GRAFIDA_GUEST:-'C:\src\grafida'}
PACKAGED=${GRAFIDA_PACKAGED:-}
GS='C:\src\shell'
RUN='C:\winvm\run'
CLIENT="python3 -I $SHELL_DIR/scripts/dev/automation.py"
export SHELL_AUTOMATION_REMOTE=1
RESULTS=()
TUNNEL_DIR=

gps() { "$W" ps "$VM" "$1" 2>/dev/null | tr -d '\r'; }

record() { # status, item, detail
    RESULTS+=("$1|$2|$3")
    printf '%-4s %s — %s\n' "$1" "$2" "$3"
}

close_tunnel() {
    [ -n "$TUNNEL_DIR" ] || return 0
    local port
    port=$(python3 -I -c 'import json,sys,glob; print(json.load(open(glob.glob(sys.argv[1] + "/*.automation.json")[0]))["port"])' "$TUNNEL_DIR" 2>/dev/null)
    [ -n "$port" ] && ps -C ssh -o pid=,args= | grep -- "-L 127.0.0.1:$port:" | awk '{print $1}' | xargs -r kill
    rm -rf "$TUNNEL_DIR"
    TUNNEL_DIR=
}

SHELL_PID=
start_shell() { # program app_id keep(0|1) [args…]; WINVM_ENV for variables
    local program=$1 id=$2 keep=$3
    shift 3
    close_tunnel
    "$W" stop-run "$VM" >/dev/null 2>&1
    export SHELL_APP_ID=$id
    SHELL_PID=$(WINVM_KEEP=$keep "$W" run "$VM" "$program" "$@" --automation | tr -d '\r' | tail -1)
    TUNNEL_DIR=$("$W" tunnel "$VM" "$id")
    export XDG_RUNTIME_DIR=$TUNNEL_DIR
    # The app's own document, not the blank one the window shows until the worker is ready.
    $CLIENT wait 'return location.origin === "boson://app" && document.readyState === "complete"' 60 >/dev/null
}

stop_shell() { # through the shell's own stop path
    [ -n "${SHELL_PID:-}" ] || return 0
    $CLIENT eval 'fetch("/_native/quit", {method: "POST"}); return true' >/dev/null 2>&1
    gps "for (\$i = 0; \$i -lt 50 -and (Get-Process -Id $SHELL_PID -ErrorAction SilentlyContinue); \$i++) { Start-Sleep -Milliseconds 100 }" >/dev/null
    close_tunnel
    "$W" stop-run "$VM" >/dev/null 2>&1
    SHELL_PID=
}

alive() { [ "$(gps "[bool](Get-Process -Id $1 -ErrorAction SilentlyContinue)")" = True ]; }
log_worker() { gps "(Select-String -Path '$1' -Pattern 'worker started, pid (\\d+)' | Select -Last 1).Matches.Groups[1].Value"; }

js() { $CLIENT eval-file "$1"; }

ok_field() { python3 -I -c 'import json,sys; a=json.load(sys.stdin); r=a.get("result") or {}; print("yes" if a.get("ok") and (r.get("ok", True) is True) and not r.get("failures") else "no: " + json.dumps(r.get("failures", a))[:300])'; }

trap 'close_tunnel; "$W" stop-run "$VM" >/dev/null 2>&1' EXIT

[ -f "$BIN" ] || { echo "No shell binary at $BIN" >&2; exit 1; }
"$W" wait "$VM" || exit 1

# ── Into the guest: the shell, the test apps (their dev worker is `python`: `python3` is the
# Store's alias there), and the files the dialog checks pick ───────────────────────────────────
STAGE=$(mktemp -d -t parity-windows-XXXXXX)
mkdir -p "$STAGE/tests"
cp -r "$SHELL_DIR/tests/apps" "$SHELL_DIR/tests/workers" "$STAGE/tests/"
rm -rf "$STAGE"/tests/workers/__pycache__ "$STAGE"/tests/apps/*/logs
for app in echo silent; do sed -i 's/"python3"/"python"/' "$STAGE/tests/apps/$app/shell.json"; done
cp "$BIN" "$STAGE/grafida-shell.exe"
cat >"$STAGE/seed-site.py" <<'SEED'
# Adds a site to Grafida's database, so the Articles screen and the editor open (parity-windows.sh).
import sqlite3, sys, time
db = sqlite3.connect(sys.argv[1], timeout=5)
now = time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime())
db.execute("INSERT INTO sites (title, base_url, created_at, updated_at) VALUES (?, ?, ?, ?)", ("Parity", "https://example.invalid", now, now))
db.commit()
SEED
"$W" stop-run "$VM" >/dev/null 2>&1   # a running shell would hold its .exe open
"$W" push "$VM" "$STAGE" "$GS" || exit 1
rm -rf "$STAGE"
gps "New-Item -ItemType Directory -Force C:\\winvm\\files\\exports | Out-Null; Set-Content -NoNewline -Path C:\\winvm\\files\\pick.png -Value PNG; Set-Content -Path C:\\winvm\\files\\note.md -Value '# Hello'; [IO.File]::WriteAllBytes('C:\\winvm\\files\\dropped.png', [Convert]::FromBase64String('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='))" >/dev/null
if [ -n "$PACKAGED" ]; then
    GRAFIDA_EXE=$(gps "Remove-Item -Recurse -Force C:\\winvm\\packaged -ErrorAction SilentlyContinue; Copy-Item -Recurse '$PACKAGED' C:\\winvm\\packaged; \$exe = (Get-ChildItem C:\\winvm\\packaged\\*.exe | ? { \$_.Name -notmatch 'worker|uninstall' } | Select -First 1).FullName; Copy-Item $GS\\grafida-shell.exe \$exe -Force; \$exe" | tail -1)
    [ -n "$GRAFIDA_EXE" ] || { echo "GRAFIDA_PACKAGED=$PACKAGED has no shell executable" >&2; exit 1; }
fi

"$W" watch-start "$VM" >/dev/null || exit 1

# ── The bridge, the navigation policy and the token flow, against the echo app ────────────────
ECHO="$GS\\tests\\apps\\echo"
ECHO_LOG="$ECHO\\logs\\shell.log"
start_shell "$GS\\grafida-shell.exe" test.shell.echo 0 --dev "$ECHO"

answer=$(js "$SHELL_DIR/tests/apps/echo/bridge.js")
record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
    "Origin, methods, bodies 0 B → 50 MiB, repeated headers, no-store, 413" \
    "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("origin %s; 50 MiB in %s ms; x-multi=%r; /api no-store=%r; over limit %s; %s; failures %s" % (r["origin"], r["bodies"][-1]["ms"], r["multi"], r["apiCacheControl"], r["overLimit"], r["info"]["webview"], r["failures"]))' <<<"$answer")"

answer=$(js "$SHELL_DIR/tests/apps/echo/blob.js")
record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
    "Blob, File and FormData-with-a-file bodies reach the worker (WebView2 hands them over)" "$(ok_field <<<"$answer")"

answer=$(js "$SHELL_DIR/tests/apps/echo/navigation.js")
# A script's el.click() on a target=_blank link is not user-initiated for WebView2 (a same-window
# one is): the new-window case gets a real click, as a user's would be (grafida/grafida#91).
$CLIENT eval 'const a = document.createElement("a"); a.href = "https://example.com/blank"; a.target = "_blank"; a.id = "blank-link"; a.textContent = "blank"; Object.assign(a.style, {position: "fixed", left: "20px", top: "20px", width: "120px", height: "40px", display: "block", zIndex: 99999, background: "#ccc"}); document.body.append(a); return true' >/dev/null
$CLIENT click 80 40 >/dev/null
sleep 1
opened=$($CLIENT opened | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
record "$( [ "$(ok_field <<<"$answer")" = yes ] && [ "$opened" = "['https://example.com/clicked', 'https://example.com/blank']" ] && echo PASS || echo FAIL)" \
    "Off-origin link → OS browser; target=_blank (a real click) → OS browser; nothing navigates the app away" \
    "to the browser: $opened; $(ok_field <<<"$answer")"

$CLIENT pick 'C:\winvm\files\pick.png' >/dev/null
$CLIENT pick --cancel >/dev/null
answer=$(js "$SHELL_DIR/tests/apps/echo/dialogs.js")
record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
    "File token round trip, cancel, injected path stripped, forged token refused" "$(ok_field <<<"$answer")"

answer=$(js "$SHELL_DIR/tests/apps/echo/responsive.js")
record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
    "UI stays responsive while a 5 s request is in flight" "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("request %s ms, longest frame gap %s ms" % (r["tookMs"], r["maxFrameGapMs"]))' <<<"$answer")"

# Kill the worker under an in-flight request.
$CLIENT eval 'window.__slow = fetch("/sleep?ms=4000", {cache: "no-store"}).then(async r => ({status: r.status, body: await r.json()})); return true' >/dev/null
sleep 0.5
# One guest script finds the worker and kills it: two SSH round trips can outlast the request.
gps "Stop-Process -Force -Id (Select-String -Path '$ECHO_LOG' -Pattern 'worker started, pid (\\d+)' | Select -Last 1).Matches.Groups[1].Value" >/dev/null
answer=$($CLIENT eval 'const a = await window.__slow; const b = await fetch("/api/after", {cache: "no-store"}); return {first: a, second: b.status}')
record "$(grep -q '"worker_restarted"' <<<"$answer" && grep -q '"second": 200' <<<"$answer" && echo PASS || echo FAIL)" \
    "Kill the worker → in-flight request gets 502 worker_restarted, app recovers" \
    "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("in flight: %s %s; next request: %s" % (r["first"]["status"], r["first"]["body"], r["second"]))' <<<"$answer")"

# Three more kills inside the window (four in all): the supervisor gives up with an alert.
for _ in 1 2 3; do
    $CLIENT eval 'return (await fetch("/pid")).ok' >/dev/null 2>&1
    gps "Stop-Process -Force -Id (Select-String -Path '$ECHO_LOG' -Pattern 'worker started, pid (\\d+)' | Select -Last 1).Matches.Groups[1].Value" >/dev/null
    sleep 0.5
done
sleep 1
gave_up=$(gps "(Select-String -Path '$ECHO_LOG' -Pattern 'giving up on the worker' | Select -Last 1).Line")
record "$( [ -n "$gave_up" ] && alive "$SHELL_PID" && echo PASS || echo FAIL)" \
    "4 kills in 60 s → native alert with the stderr tail" "log: $(cut -c26-150 <<<"$gave_up")"
close_tunnel
"$W" stop-run "$VM" >/dev/null

# The shell killed outright: the Job Object takes the worker with it, idle or mid-request.
for busy in 0 1; do
    start_shell "$GS\\grafida-shell.exe" test.shell.echo 0 --dev "$ECHO"
    worker=$(log_worker "$ECHO_LOG")
    [ "$busy" = 1 ] && { $CLIENT eval 'fetch("/sleep?ms=5000", {cache: "no-store"}); return true' >/dev/null; sleep 1; }
    gps "Stop-Process -Id $SHELL_PID -Force" >/dev/null
    t0=$(date +%s.%N)
    for _ in $(seq 1 30); do alive "$worker" || break; sleep 0.1; done
    took=$(python3 -I -c "import time; print(f'{time.time() - $t0:.1f}')")
    state=$(alive "$worker" && echo "alive after ${took} s" || echo "gone within ${took} s")
    record "$(alive "$worker" && echo FAIL || echo PASS)" \
        "Kill the shell outright, worker $( [ "$busy" = 1 ] && echo mid-request || echo idle) → the worker goes with it (Job Object)" \
        "worker $worker $state"
    close_tunnel; SHELL_PID=
done

# A worker that never says `ready`.
SILENT="$GS\\tests\\apps\\silent"
close_tunnel
"$W" stop-run "$VM" >/dev/null 2>&1
silent=$("$W" run "$VM" "$GS\\grafida-shell.exe" --dev "$SILENT" | tr -d '\r' | tail -1)
sleep 4
line=$(gps "(Select-String -Path '$SILENT\\logs\\shell.log' -Pattern 'did not send .ready. within 2 s' | Select -Last 1).Line")
record "$( [ -n "$line" ] && alive "$silent" && echo PASS || echo FAIL)" \
    "A worker that never sends ready → native alert" "$(cut -c26-150 <<<"$line")"
"$W" stop-run "$VM" >/dev/null

# ── Grafida: in dev mode, or packaged (GRAFIDA_PACKAGED) ───────────────────────────────────────
LOG="$RUN\\Local\\grafida\\Logs\\shell.log"
DATA="$RUN\\Roaming\\Grafida"
grafida() { # keep [env…]: start Grafida
    local keep=$1
    if [ -n "$PACKAGED" ]; then
        start_shell "$GRAFIDA_EXE" app.grafida "$keep"
    else
        start_shell "$GS\\grafida-shell.exe" app.grafida "$keep" --dev "$GG"
    fi
    $CLIENT wait 'return !!document.querySelector("#sidebar")' 30 >/dev/null
}
grafida 0
for p in 'C:\winvm\files\pick.png' --cancel 'C:\winvm\files\note.md' 'C:\winvm\files\exports'; do $CLIENT pick "$p" >/dev/null; done
answer=$($CLIENT eval-file "$SHELL_DIR/scripts/dev/parity/grafida.js")
opened=$($CLIENT opened | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
exports=$(gps "(Get-ChildItem C:\\winvm\\files\\exports | Select -First 1).Name")
record "$( [ "$(ok_field <<<"$answer")" = yes ] && [ -n "$exports" ] && echo PASS || echo FAIL)" \
    "Grafida: SPA loads at boson://app; file open (image/markdown), cancel, select folder → export" \
    "$(ok_field <<<"$answer"); export: $exports"
record "$( [ "$opened" != "[]" ] && echo PASS || echo FAIL)" \
    "Grafida: Help screen renders the bundled docs; an external Help link → OS browser" "to the browser: $opened"
token=$(python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"]["folderToken"])' <<<"$answer")

# The editor: paste as plain text, the real Ctrl+Shift+V chord, and an image file dropped on it.
gps "& python $GS\\seed-site.py '$DATA\\grafida.sqlite'" >/dev/null
$CLIENT eval 'location.reload(); return true' >/dev/null
sleep 2
$CLIENT wait 'return !!document.querySelector("#sidebar") && State.sites && State.sites.length > 0' 30 >/dev/null
answer=$($CLIENT eval-file "$SHELL_DIR/scripts/dev/parity/grafida-editor.js")
record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
    "Grafida: the Ctrl+Shift+V path pastes the clipboard as plain text (the shell reads CF_UNICODETEXT)" "$(ok_field <<<"$answer")"
read -r dx dy < <(python3 -I -c 'import json,sys; print(*json.load(sys.stdin)["result"]["dropAt"])' <<<"$answer")
$CLIENT eval 'await navigator.clipboard.writeText("typed chord"); const ed = State.tinyMCEEditor; ed.setContent("<p>start</p>"); ed.focus(); ed.selection.select(ed.getBody(), true); ed.selection.collapse(false); return true' >/dev/null
keyed=$($CLIENT key ctrl+shift+v 2>&1 | tr -d '\n ')
sleep 1.5
chord=$($CLIENT eval 'return State.tinyMCEEditor.getContent()' | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("result"))')
record "$( [ "$chord" = "<p>starttyped chord</p>" ] && echo PASS || echo FAIL)" \
    "Grafida: the real Ctrl+Shift+V chord in the editor reaches the page and pastes plain text" "$chord; key: $(cut -c1-100 <<<"$keyed")"
$CLIENT drop "$dx" "$dy" 'C:\winvm\files\dropped.png' >/dev/null
sleep 2
# TinyMCE's own drop handling inserts the image (no data-grafida-media-id, which only Grafida's
# shell:drop path adds); publishing uploads every offline image by its URL, tagged or not.
dropped=$($CLIENT eval 'return [...State.tinyMCEEditor.getBody().querySelectorAll("img[src^=\"boson://app/api/media/\"]")].map(i => i.getAttribute("src"))')
record "$(grep -q 'boson://app/api/media/' <<<"$dropped" && echo PASS || echo FAIL)" \
    "Grafida: an image file dropped on the editor is inserted as offline media (File objects, as WebView2 delivers a drop)" \
    "$(tr -d '\n ' <<<"$dropped" | cut -c1-120)"
$CLIENT eval 'State.tinyMCEEditor.setDirty(false); showScreen("articles"); document.activeElement && document.activeElement.blur(); return true' >/dev/null

# Idle CPU of the whole tree, on the Articles screen with a site, the focus out of the editor.
settle=60 sample=60
[ -n "${QUICK:-}" ] && settle=5 sample=10
$CLIENT eval 'document.querySelector("#main-nav [data-screen=\"articles\"]").click(); document.body.focus(); return true' >/dev/null
sleep "$settle"
cpu=$(gps "
\$all = Get-CimInstance Win32_Process
\$ids = @($SHELL_PID); \$frontier = @($SHELL_PID)
while (\$frontier.Count) { \$next = @(\$all | ? { \$frontier -contains \$_.ParentProcessId } | % { \$_.ProcessId }); \$ids += \$next; \$frontier = \$next }
function sample { \$h = @{}; foreach (\$i in \$ids) { \$p = Get-Process -Id \$i -ErrorAction SilentlyContinue; if (\$p) { \$h[\$i] = @(\$p.ProcessName, \$p.TotalProcessorTime.TotalSeconds) } }; \$h }
\$a = sample; \$t = [Diagnostics.Stopwatch]::StartNew(); Start-Sleep $sample; \$b = sample; \$s = \$t.Elapsed.TotalSeconds
\$by = @{}; \$total = 0
foreach (\$i in \$b.Keys) { if (\$a.ContainsKey(\$i)) { \$d = 100 * (\$b[\$i][1] - \$a[\$i][1]) / \$s; \$total += \$d; \$by[\$b[\$i][0]] += \$d } }
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture
'{0:N2}|{1}' -f \$total, ((\$by.GetEnumerator() | Sort-Object Value -Descending | % { '{0} {1:N2}' -f \$_.Key, \$_.Value }) -join ', ')")
record "$(python3 -I -c "print('PASS' if ${cpu%%|*} < 1 else 'FAIL')" 2>/dev/null || echo FAIL)" \
    "Idle CPU < 1% (whole tree on the Articles screen, ${sample} s after ${settle} s; debug shell)" "${cpu%%|*} % of one core: ${cpu#*|}"

# Close → the worker exits within 3 s, nothing is orphaned, the WAL does not grow.
# Through the shell's stop path (the window's close button takes the same one), timed from the
# shell's own log: "stopping", then the supervisor's "worker stopped" (or "worker killed").
worker=$(log_worker "$LOG")
stop_shell
closed=$(gps "\$l = Get-Content '$LOG'; \$a = (\$l | Select-String 'stopping \\(exit' | Select -Last 1).Line; \$b = (\$l | Select-String 'worker (stopped|killed)' | Select -Last 1).Line; \$t = { param(\$x) [DateTime]::Parse(\$x.Substring(0, 24)).ToUniversalTime() }; [Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture; '{0:N2}|{1}' -f ((& \$t \$b) - (& \$t \$a)).TotalSeconds, (\$b -replace '^.*\\] ', '')" | tail -1)
took=${closed%%|*}
gone=$(grep -q 'worker stopped' <<<"${closed#*|}" && ! alive "$worker" && echo yes || echo no)
wal=()
for _ in 1 2 3; do
    grafida 1
    $CLIENT eval 'return (await fetch("/api/settings/spell-check", {method: "POST", body: "{\"enabled\":true}"})).ok' >/dev/null
    stop_shell
    sleep 0.5
    wal+=("$(gps "(Get-Item '$DATA\\grafida.sqlite-wal' -ErrorAction SilentlyContinue).Length")")
done
orphans=$(gps "(Get-CimInstance Win32_Process | ? { \$_.CommandLine -match 'bin.worker\\.php|grafida-worker' -and \$_.CommandLine -notmatch 'Get-CimInstance' } | % { \$_.ProcessId }) -join ' '")
record "$( [ "$gone" = yes ] && [ -z "$orphans" ] && python3 -I -c "import sys; sys.exit(0 if float('${took:-99}'.replace(',', '.')) <= 3.5 else 1)" && echo PASS || echo FAIL)" \
    "Close the window → worker exits within 3 s; no orphan worker; no -wal growth" \
    "${closed#*|} after ${took} s; gone: $gone; orphans: ${orphans:-none}; -wal bytes after each of 3 restarts: ${wal[*]:-none}"

# A token from the previous session.
grafida 1
answer=$($CLIENT eval "const r = await fetch('/api/request-log/export', {method: 'POST', headers: GrafidaShell.fileHeaders('$token'), body: '{}'}); return {status: r.status, error: (await r.json()).error}")
record "$(grep -q stale_file_token <<<"$answer" && echo PASS || echo FAIL)" \
    "A token from a previous session is rejected" "$(tr -d '\n ' <<<"$answer" | cut -c1-120)"
stop_shell

# DevTools follow GRAFIDA_DEBUG.
off=$(gps "(Select-String -Path '$LOG' -Pattern '\\(devtools off\\)').Count")
WINVM_ENV="GRAFIDA_DEBUG=1" grafida 1
stop_shell
on=$(gps "(Select-String -Path '$LOG' -Pattern '\\(devtools on\\)').Count")
record "$( [ "${off:-0}" -ge 1 ] && [ "${on:-0}" -ge 1 ] && echo PASS || echo FAIL)" \
    "DevTools only with GRAFIDA_DEBUG=1 (the WebView2 setting; Inspect itself is a manual check)" \
    "launches with devtools off: ${off:-0}, on with GRAFIDA_DEBUG=1: ${on:-0}"

# Every console window that appeared on the desktop during the whole run.
shown=$("$W" watch-stop "$VM" | tr -d '\r')
record "$( [ -z "$shown" ] && echo PASS || echo FAIL)" \
    "No console window appeared on the desktop during the run (shell, workers, their children; a WinEvent hook)" \
    "$( [ -z "$shown" ] && echo none || tr '\n' ';' <<<"$shown" | cut -c1-300)"

echo
echo "| Result | Check | Evidence |"
echo "|---|---|---|"
for row in "${RESULTS[@]}"; do
    IFS='|' read -r status item detail <<<"$row"
    echo "| $status | $item | ${detail//|/\\|} |"
done
echo
versions=$("$W" versions "$VM" 2>/dev/null | tr -d '\r' | sed -n '1,2p' | tr '\n' ';')
echo "shell $(cd "$SHELL_DIR" && git rev-parse --short HEAD 2>/dev/null)$( (cd "$SHELL_DIR" && git diff --quiet 2>/dev/null) || echo '+dirty'), grafida ${PACKAGED:+packaged }${PACKAGED:-$GG (dev)}, $VM: $versions $(date -u +%Y-%m-%dT%H:%MZ)"
grep -q '^FAIL' <(printf '%s\n' "${RESULTS[@]}") && exit 1 || exit 0
