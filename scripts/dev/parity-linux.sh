#!/usr/bin/env bash
# Grafida Shell — the automatable half of the Linux parity checklist (grafida/grafida#82, #85).
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# Runs the shell (a debug build with the automation channel) on THIS desktop session, against the
# test apps in tests/apps/ and against Grafida (GRAFIDA_DIR, default ../grafida) in dev mode, and
# prints one PASS/FAIL line per check plus a Markdown table at the end. Everything per-user (HOME,
# XDG_*) points at a throwaway directory, so neither your Grafida data nor your browser is touched:
# while the automation channel is open the shell records external URLs instead of opening them.
#
#   scripts/dev/parity-linux.sh            # everything, including the 2-minute idle-CPU sample
#   QUICK=1 scripts/dev/parity-linux.sh    # a 10 s idle sample instead
#
# Against a packaged Grafida (grafida/grafida#86), e.g. on a clean VM, with the CI-built debug shell:
#   GRAFIDA_PACKAGED=~/grafida SHELL_BIN=~/grafida-shell-debug scripts/dev/parity-linux.sh
# Needs only python3 there (no Rust, no PHP).
#
# The checks that need a human (context menu, spelling, real dialogs, drag and drop, title bar,
# Ctrl+Shift+V, AI streaming, …) are listed in the Phase 2 issue; this script does not pretend to
# cover them.

set -uo pipefail

SHELL_DIR=$(cd "$(dirname "$0")/../.." && pwd)
GRAFIDA_DIR=${GRAFIDA_DIR:-$SHELL_DIR/../grafida}
WORK=$(mktemp -d -t shell-parity-XXXXXX)
# SHELL_BIN: a prebuilt debug build with the automation channel (CI uploads one built on Ubuntu
# 22.04, which runs on every supported distribution); otherwise it is built here.
BIN=${SHELL_BIN:-$SHELL_DIR/target/debug/grafida-shell}
# GRAFIDA_PACKAGED: an extracted shell tarball (its grafida/ directory). Grafida then runs as
# installed — the packaged launcher's neighbours, the SFX worker and its PHAR, the manifest beside
# the binary — with only grafida-shell swapped for $BIN, since a release build has no automation
# channel (grafida/grafida#86). The tarball itself is copied, never modified.
PACKAGED=${GRAFIDA_PACKAGED:-}
CLIENT="python3 -I $SHELL_DIR/scripts/dev/automation.py"
RESULTS=()

trap 'stop_shell; rm -rf "$WORK"' EXIT

record() { # status, item, detail
    RESULTS+=("$1|$2|$3")
    printf '%-4s %s — %s\n' "$1" "$2" "$3"
}

isolated() {
    env HOME="$WORK/home" XDG_DATA_HOME="$WORK/home/data" XDG_CONFIG_HOME="$WORK/home/config" \
        XDG_CACHE_HOME="$WORK/home/cache" XDG_STATE_HOME="$WORK/home/state" "$@"
}

SHELL_PID=
start_shell() { # app_dir app_id [extra env...]
    local dir=$1 id=$2
    shift 2
    export SHELL_APP_ID=$id
    local channel="${XDG_RUNTIME_DIR:-/tmp}/$id.automation.json"
    rm -f -- "$channel" # a shell killed by a signal leaves its file behind
    if [ "$id" = app.grafida ] && [ -n "$PACKAGED" ]; then
        (cd / && isolated "$@" "$WORK/packaged/grafida-shell" --automation >>"$WORK/shell-$id.log" 2>&1) &
    else
        (cd "$dir" && isolated "$@" "$BIN" --dev . --automation >>"$WORK/shell-$id.log" 2>&1) &
    fi
    for _ in $(seq 1 100); do
        [ -f "$channel" ] && break
        sleep 0.1
    done
    SHELL_PID=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$channel")
    # The app's own document, not the blank one the window shows until the worker is ready: on a
    # slow machine an eval would otherwise land in that placeholder.
    $CLIENT wait 'return location.origin === "boson://app" && document.readyState === "complete"' 60 >/dev/null
}

stop_shell() {
    [ -n "${SHELL_PID:-}" ] || return 0
    $CLIENT eval 'fetch("/_native/quit", {method: "POST"}); return true' >/dev/null 2>&1
    for _ in $(seq 1 50); do kill -0 "$SHELL_PID" 2>/dev/null || break; sleep 0.1; done
    kill "$SHELL_PID" 2>/dev/null
    SHELL_PID=
}

# Every pid in the process tree under $1.
tree() {
    local pid=$1
    echo "$pid"
    for child in $(pgrep -P "$pid"); do tree "$child"; done
}

js() { # file → the result object, or exit 1 with the answer
    $CLIENT eval-file "$1"
}

ok_field() { python3 -I -c 'import json,sys; a=json.load(sys.stdin); r=a.get("result") or {}; print("yes" if a.get("ok") and (r.get("ok", True) is True) and not r.get("failures") else "no: " + json.dumps(r.get("failures", a))[:300])'; }

if [ -z "${SHELL_BIN:-}" ]; then
    echo "Building the shell with the automation channel…"
    (cd "$SHELL_DIR" && cargo build -q --features automation) || exit 1
fi
[ -x "$BIN" ] || { echo "No shell binary at $BIN" >&2; exit 1; }
if [ -n "$PACKAGED" ]; then
    [ -f "$PACKAGED/shell.json" ] && [ -x "$PACKAGED/grafida-worker" ] \
        || { echo "GRAFIDA_PACKAGED=$PACKAGED is not an extracted Grafida shell tarball" >&2; exit 1; }
    cp -a "$PACKAGED" "$WORK/packaged"
    cp "$BIN" "$WORK/packaged/grafida-shell"
    GRAFIDA_DIR=$WORK/packaged
fi
mkdir -p "$WORK/home" "$WORK/exports"
printf 'PNG' >"$WORK/pick.png"
printf '# Hello\n' >"$WORK/note.md"

# ── The bridge, the navigation policy and the token flow, against the echo app ────────────────
ECHO=$SHELL_DIR/tests/apps/echo
SILENT=$SHELL_DIR/tests/apps/silent
mkdir -p "$ECHO/logs" "$SILENT/logs"
: >"$ECHO/logs/shell.log"
: >"$SILENT/logs/shell.log"
start_shell "$ECHO" test.shell.echo

answer=$(js "$ECHO/bridge.js")
record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
    "Origin, methods, bodies 0 B → 50 MiB, repeated headers, no-store, 413" \
    "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("origin %s; 50 MiB in %s ms; x-multi=%r; /api no-store=%r; over limit %s; failures %s" % (r["origin"], r["bodies"][-1]["ms"], r["multi"], r["apiCacheControl"], r["overLimit"], r["failures"]))' <<<"$answer")"

answer=$(js "$ECHO/navigation.js")
opened=$($CLIENT opened | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
record "$( [ "$(ok_field <<<"$answer")" = yes ] && [ "$opened" = "['https://example.com/clicked', 'https://example.com/blank']" ] && echo PASS || echo FAIL)" \
    "Off-origin link → OS browser; window.open → OS browser; nothing navigates the app away" \
    "to the browser: $opened; $(ok_field <<<"$answer")"

$CLIENT pick "$WORK/pick.png" >/dev/null
$CLIENT pick --cancel >/dev/null
answer=$(js "$ECHO/dialogs.js")
record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
    "File token round trip, cancel, injected path stripped, forged token refused" "$(ok_field <<<"$answer")"

answer=$(js "$ECHO/responsive.js")
record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
    "UI stays responsive while a 5 s request is in flight" "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("request %s ms, longest frame gap %s ms" % (r["tookMs"], r["maxFrameGapMs"]))' <<<"$answer")"

# Kill the worker under an in-flight request.
$CLIENT eval 'window.__slow = fetch("/sleep?ms=4000", {cache: "no-store"}).then(async r => ({status: r.status, body: await r.json()})); return true' >/dev/null
sleep 0.5
worker=$(pgrep -P "$SHELL_PID" -f fake_worker.py | head -1)
kill -9 "$worker"
answer=$($CLIENT eval 'const a = await window.__slow; const b = await fetch("/api/after", {cache: "no-store"}); return {first: a, second: b.status}')
record "$(grep -q '"worker_restarted"' <<<"$answer" && grep -q '"second": 200' <<<"$answer" && echo PASS || echo FAIL)" \
    "Kill the worker → in-flight request gets 502 worker_restarted, app recovers" \
    "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("in flight: %s %s; next request: %s" % (r["first"]["status"], r["first"]["body"], r["second"]))' <<<"$answer")"

# Three more kills inside the window (four in all): the supervisor gives up with an alert.
for _ in 1 2 3; do
    for _ in $(seq 1 50); do
        worker=$(pgrep -P "$SHELL_PID" -f fake_worker.py | head -1)
        [ -n "$worker" ] && $CLIENT eval 'return (await fetch("/pid")).ok' >/dev/null 2>&1 && break
        sleep 0.1
    done
    kill -9 "$worker"
    sleep 0.3
done
sleep 1
gave_up=$(grep -c "giving up on the worker" "$ECHO/logs/shell.log" | tail -1)
$CLIENT screenshot "$WORK/alert-webview.png" >/dev/null 2>&1
record "$( [ "$gave_up" -ge 1 ] && kill -0 "$SHELL_PID" 2>/dev/null && echo PASS || echo FAIL)" \
    "4 kills in 60 s → native alert with the stderr tail" \
    "log: $(grep "giving up on the worker" "$ECHO/logs/shell.log" | tail -1 | cut -c26-150)"
kill "$SHELL_PID" 2>/dev/null
SHELL_PID=

# A worker that never says `ready`.
(cd "$SILENT" && exec env HOME="$WORK/home" XDG_STATE_HOME="$WORK/home/state" "$BIN" --dev . >>"$WORK/shell-silent.log" 2>&1) &
silent_pid=$!
sleep 3
record "$(grep -q "did not send \`ready\` within 2 s" "$SILENT/logs/shell.log" && kill -0 $silent_pid 2>/dev/null && echo PASS || echo FAIL)" \
    "A worker that never sends ready → native alert" \
    "$(grep "giving up" "$SILENT/logs/shell.log" | tail -1 | cut -c26-150)"
kill $silent_pid 2>/dev/null

# ── Grafida: in dev mode, or packaged (GRAFIDA_PACKAGED) ───────────────────────────────────────
if [ ! -f "$GRAFIDA_DIR/shell.json" ]; then
    record SKIP "Grafida checks" "no Grafida clone with shell.json at $GRAFIDA_DIR"
else
    LOG="$WORK/home/state/grafida/logs/shell.log"
    start_shell "$GRAFIDA_DIR" app.grafida
    $CLIENT wait 'return !!document.querySelector("#sidebar")' 30 >/dev/null
    for p in "$WORK/pick.png" --cancel "$WORK/note.md" "$WORK/exports"; do $CLIENT pick "$p" >/dev/null; done
    answer=$($CLIENT eval-file "$SHELL_DIR/scripts/dev/parity/grafida.js")
    opened=$($CLIENT opened | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
    record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
        "Grafida: SPA loads at boson://app; file open (image/markdown), cancel, select folder → export" \
        "$(ok_field <<<"$answer"); export: $(ls "$WORK/exports" | head -1)"
    record "$( [ "$opened" != "[]" ] && echo PASS || echo FAIL)" \
        "Grafida: Help screen renders the bundled docs; an external Help link → OS browser" "to the browser: $opened"
    token=$(python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"]["folderToken"])' <<<"$answer")
    $CLIENT screenshot "$WORK/grafida.png" >/dev/null

    # The editor: paste as plain text, and an image file dropped on it.
    # Python, not PHP: a clean VM running the packaged app has no PHP.
    python3 -I - "$WORK/home/data/grafida/grafida.sqlite" <<'PY'
import sqlite3, sys, time
db = sqlite3.connect(sys.argv[1], timeout=5)
now = time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime())
db.execute("INSERT INTO sites (title, base_url, created_at, updated_at) VALUES (?, ?, ?, ?)", ("Parity", "https://example.invalid", now, now))
db.commit()
PY
    $CLIENT eval 'location.reload(); return true' >/dev/null
    sleep 2
    $CLIENT wait 'return !!document.querySelector("#sidebar") && State.sites && State.sites.length > 0' 30 >/dev/null
    echo 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=' | base64 -d >"$WORK/dropped.png"
    answer=$($CLIENT eval-file "$SHELL_DIR/scripts/dev/parity/grafida-editor.js")
    record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
        "Grafida: Ctrl+Shift+V pastes the clipboard as plain text (the shell reads it; no wl-paste needed)" "$(ok_field <<<"$answer")"
    read -r dx dy < <(python3 -I -c 'import json,sys; print(*json.load(sys.stdin)["result"]["dropAt"])' <<<"$answer")
    $CLIENT drop "$dx" "$dy" "$WORK/dropped.png" >/dev/null
    sleep 2
    dropped=$($CLIENT eval 'return [...State.tinyMCEEditor.getBody().querySelectorAll("img[data-grafida-media-id]")].map(i => i.getAttribute("src"))')
    record "$(grep -q 'boson://app/api/media/' <<<"$dropped" && echo PASS || echo FAIL)" \
        "Grafida: an image file dropped on the editor is inserted as offline media (delivered as in a real drop)" \
        "$(tr -d '\n ' <<<"$dropped" | cut -c1-120)"
    # Leaving by script keeps the keyboard focus in the hidden editor's iframe, whose caret then
    # blinks unseen: 0.85-1.13 % of a core between the shell and the web process (grafida/grafida#92),
    # as parity-macos.sh measured on macOS. A real click on the sidebar moves the focus out, so do that.
    $CLIENT eval 'State.tinyMCEEditor.setDirty(false); showScreen("sites"); document.activeElement.blur(); window.focus(); return true' >/dev/null

    # Idle CPU of the whole tree on a quiet screen.
    settle=60 sample=60
    [ -n "${QUICK:-}" ] && settle=5 sample=10
    $CLIENT eval 'document.querySelector("#main-nav [data-screen=\"sites\"]").click(); return true' >/dev/null
    sleep "$settle"
    pids=$(tree "$SHELL_PID" | tr '\n' ' ')
    # One line: the total, then each process's share (so a failure names where the time went).
    idle=$(python3 -I - "$sample" $pids <<'PY'
import os, sys, time
sample, pids = float(sys.argv[1]), sys.argv[2:]
def ticks():
    out = {}
    for pid in pids:
        try:
            head, tail = open(f"/proc/{pid}/stat").read().rsplit(")", 1)
            fields = tail.split()
            out[pid] = (head.split("(", 1)[1], int(fields[11]) + int(fields[12]))
        except OSError:
            pass
    return out
a = ticks(); time.sleep(sample); b = ticks()
hz = os.sysconf('SC_CLK_TCK')
share = {pid: (b[pid][1] - a[pid][1]) / hz / sample * 100 for pid in a if pid in b}
print(f"{sum(share.values()):.2f}", "; ".join(f"{a[pid][0]} {v:.2f}" for pid, v in share.items()))
PY
)
    cpu=${idle%% *} shares=${idle#* }
    record "$(python3 -I -c "print('PASS' if $cpu < 1 else 'FAIL')")" \
        "Idle CPU < 1% (whole process tree, ${sample} s after ${settle} s)" "$cpu % of one core over $(wc -w <<<"$pids") processes: $shares"

    # Close → the worker exits within 3 s, nothing is orphaned, the WAL does not grow.
    worker=$(grep -o "worker started, pid [0-9]*" "$LOG" | tail -1 | grep -o "[0-9]*$")
    started=$(date +%s.%N)
    stop_shell
    for _ in $(seq 1 40); do kill -0 "$worker" 2>/dev/null || break; sleep 0.1; done
    took=$(python3 -I -c "import time; print(f'{time.time() - $started:.2f}')")
    alive=$(kill -0 "$worker" 2>/dev/null && echo yes || echo no)
    wal=()
    for round in 1 2 3; do
        start_shell "$GRAFIDA_DIR" app.grafida
        $CLIENT eval 'return (await fetch("/api/settings/spell-check", {method: "POST", body: "{\"enabled\":true}"})).ok' >/dev/null
        stop_shell
        sleep 0.5
        wal+=("$(stat -c %s "$WORK/home/data/grafida/"*-wal 2>/dev/null | head -1 || echo 0)")
    done
    orphans=$(pgrep -f "bin/worker.php|grafida-worker" -u "$(id -u)" | while read -r p; do grep -qz "HOME=$WORK/home" "/proc/$p/environ" 2>/dev/null && echo "$p"; done)
    record "$( [ "$alive" = no ] && [ -z "$orphans" ] && python3 -I -c "import sys; sys.exit(0 if $took <= 3.5 else 1)" && echo PASS || echo FAIL)" \
        "Close the window → worker exits within 3 s; no orphan worker; no -wal growth" \
        "worker gone after ${took} s; orphans: ${orphans:-none}; -wal bytes after each of 3 restarts: ${wal[*]:-none}"

    # A token from the previous session.
    start_shell "$GRAFIDA_DIR" app.grafida
    answer=$($CLIENT eval "const r = await fetch('/api/request-log/export', {method: 'POST', headers: GrafidaShell.fileHeaders('$token'), body: '{}'}); return {status: r.status, error: (await r.json()).error}")
    record "$(grep -q stale_file_token <<<"$answer" && echo PASS || echo FAIL)" \
        "A token from a previous session is rejected" "$(tr -d '\n ' <<<"$answer" | cut -c1-120)"

    # DevTools follow GRAFIDA_DEBUG.
    stop_shell
    off=$(grep -c "(devtools off)" "$LOG")
    start_shell "$GRAFIDA_DIR" app.grafida GRAFIDA_DEBUG=1
    stop_shell
    on=$(grep -c "(devtools on)" "$LOG")
    record "$( [ "$off" -ge 1 ] && [ "$on" -ge 1 ] && echo PASS || echo FAIL)" \
        "DevTools only with GRAFIDA_DEBUG=1 (WebKit setting; the menu entry itself is a manual check)" \
        "launches with devtools off: $off, on with GRAFIDA_DEBUG=1: $on"
fi

echo
echo "| Result | Check | Evidence |"
echo "|---|---|---|"
for row in "${RESULTS[@]}"; do
    IFS='|' read -r status item detail <<<"$row"
    echo "| $status | $item | ${detail//|/\\|} |"
done
echo
if [ -n "$PACKAGED" ]; then
    grafida_rev="packaged $(basename "$(dirname "$PACKAGED")")"
else
    grafida_rev="$(cd "$GRAFIDA_DIR" 2>/dev/null && git rev-parse --short HEAD)$( (cd "$GRAFIDA_DIR" 2>/dev/null && git diff --quiet) || echo '+dirty')"
fi
webkit=$(grep -ho 'on WebKitGTK [0-9.]*' "$WORK"/home/state/grafida/logs/shell.log 2>/dev/null | tail -1)
echo "shell $(cd "$SHELL_DIR" && git rev-parse --short HEAD 2>/dev/null)$( (cd "$SHELL_DIR" && git diff --quiet 2>/dev/null) || echo '+dirty'), grafida $grafida_rev, ${webkit#on }, $(. /etc/os-release && echo "$PRETTY_NAME"), $(date -u +%Y-%m-%dT%H:%MZ)"
grep -q '^FAIL' <(printf '%s\n' "${RESULTS[@]}") && exit 1 || exit 0
