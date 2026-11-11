#!/usr/bin/env bash
# Grafida Shell — the automatable half of the macOS parity checklist (grafida/grafida#82, #88).
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# Runs the shell (a debug build with the automation channel) in THIS desktop session, against the
# test apps in tests/apps/ and against Grafida (GRAFIDA_DIR, default ../grafida) in dev mode, and
# prints one PASS/FAIL line per check plus a Markdown table at the end. HOME and CFFIXED_USER_HOME
# point at a throwaway directory, so Grafida's data, its log and WebKit's website data (localStorage)
# are not yours; while the automation channel is open the shell records external URLs instead of
# opening them. One thing does escape: user defaults go through cfprefsd, which ignores both
# variables, so the shell's WebContinuousSpellCheckingEnabled lands in the real
# ~/Library/Preferences/<executable name>.plist (grafida-shell.plist in dev mode).
#
#   scripts/dev/parity-macos.sh            # everything, including the 2-minute idle-CPU sample
#   QUICK=1 scripts/dev/parity-macos.sh    # a 10 s idle sample instead
#
# Against a packaged Grafida (an .app, e.g. from the DMG on a clean VM), with a debug shell:
#   GRAFIDA_PACKAGED=/Applications/Grafida.app SHELL_BIN=~/grafida-shell-debug scripts/dev/parity-macos.sh
# The bundle is copied and only its shell swapped for SHELL_BIN, re-signed ad hoc with the hardened
# runtime and the original's entitlements (the worker keeps the packager's signature); the original
# is never modified, and the release binary never carries the channel.
#
# An x86_64 SHELL_BIN on Apple Silicon runs under Rosetta 2 (grafida/grafida#92: the guests'
# macos-*-rosetta clones, with the debug-automation-macos-x86_64 artifact and an x86_64 bundle); the
# first row then proves the shell, and a packaged bundle's worker, really run as x86_64.
#
# Over SSH, run it inside the desktop session (`sudo launchctl asuser $(id -u) sudo -u $USER …`):
# that strips TMPDIR, so the script sets it for the shell and the client alike.
#
# Menu shortcuts are sent as queued key presses (`automation.py key`), which activates the app: the
# run takes the keyboard focus. The checks that need a human (the menus by mouse, Spelling, the
# context menu, real dialogs, a real drag, the title bar, AI streaming, …) are listed in the Phase 4
# issue.

set -uo pipefail

SHELL_DIR=$(cd "$(dirname "$0")/../.." && pwd)
GRAFIDA_DIR=${GRAFIDA_DIR:-$SHELL_DIR/../grafida}
export TMPDIR=${TMPDIR:-$(getconf DARWIN_USER_TEMP_DIR)}
WORK=$(mktemp -d "${TMPDIR%/}/shell-parity-XXXXXX")
BIN=${SHELL_BIN:-$SHELL_DIR/target/debug/grafida-shell}
PACKAGED=${GRAFIDA_PACKAGED:-}
CLIENT="python3 -I $SHELL_DIR/scripts/dev/automation.py"
RESULTS=()

trap 'stop_shell; rm -rf "$WORK"' EXIT

record() { # status, item, detail
    RESULTS+=("$1|$2|$3")
    printf '%-4s %s — %s\n' "$1" "$2" "$3"
}

isolated() {
    env HOME="$WORK/home" CFFIXED_USER_HOME="$WORK/home" TMPDIR="$TMPDIR" "$@"
}

SHELL_PID=
STARTED_AT=
start_shell() { # app_dir app_id [extra env...]
    local dir=$1 id=$2
    shift 2
    export SHELL_APP_ID=$id
    local channel="$TMPDIR/$id.automation.json"
    rm -f -- "$channel" # a shell killed by a signal leaves its file behind
    STARTED_AT=$(date +%s)
    if [ "$id" = app.grafida ] && [ -n "$PACKAGED" ]; then
        (cd / && isolated "$@" "$PACKAGED_EXE" --automation >>"$WORK/shell-$id.log" 2>&1) &
    else
        (cd "$dir" && isolated "$@" "$BIN" --dev . --automation >>"$WORK/shell-$id.log" 2>&1) &
    fi
    for _ in $(seq 1 100); do
        [ -f "$channel" ] && break
        sleep 0.1
    done
    SHELL_PID=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$channel")
    # The app's own document, not the blank one the window shows until the worker is ready.
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

# WebKit's helpers (WebContent, Networking, GPU) are XPC services launched by launchd, not children
# of the shell: count the ones started since the shell was.
webkit_helpers() {
    ps -A -o pid=,lstart=,comm= | while read -r p _ mon day time year comm; do
        case $comm in *com.apple.WebKit.*) ;; *) continue ;; esac
        t=$(date -j -f '%b %d %H:%M:%S %Y' "$mon $day $time $year" +%s 2>/dev/null) || continue
        [ "$t" -ge "$STARTED_AT" ] && echo "$p"
    done
}

# CPU seconds of a set of pids, from ps's [[dd-]hh:]mm:ss.ss.
cpu_seconds() {
    local total=0 p
    for p in "$@"; do
        c=$(ps -o time= -p "$p" 2>/dev/null | awk '{ n = split($1, a, /[-:]/); s = 0; for (i = 1; i <= n; i++) s = s * 60 + a[i]; print s }')
        [ -n "$c" ] && total=$(echo "$total + $c" | bc)
    done
    echo "$total"
}

js() { # file → the result object
    $CLIENT eval-file "$1"
}

ok_field() { python3 -I -c 'import json,sys; a=json.load(sys.stdin); r=a.get("result") or {}; print("yes" if a.get("ok") and (r.get("ok", True) is True) and not r.get("failures") else "no: " + json.dumps(r.get("failures", a))[:300])'; }

if [ -z "${SHELL_BIN:-}" ]; then
    echo "Building the shell with the automation channel…"
    (cd "$SHELL_DIR" && cargo build -q --features automation) || exit 1
fi
[ -x "$BIN" ] || { echo "No shell binary at $BIN" >&2; exit 1; }
if [ -n "$PACKAGED" ]; then
    [ -f "$PACKAGED/Contents/Resources/shell.json" ] \
        || { echo "GRAFIDA_PACKAGED=$PACKAGED is not a shell-packaged .app" >&2; exit 1; }
    ditto "$PACKAGED" "$WORK/packaged.app"
    exe=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$WORK/packaged.app/Contents/Info.plist")
    PACKAGED_EXE=$WORK/packaged.app/Contents/MacOS/$exe
    # Only the shell is swapped, and it is re-signed (ad hoc) as the packager signed the original:
    # the hardened runtime and the same entitlements, then the bundle around it. The worker stub
    # keeps the packager's signature. Never --deep: that would re-sign the stub without either, and
    # the run would test a bundle under weaker restrictions than the one that ships.
    codesign -d --entitlements - --xml "$PACKAGED_EXE" >"$WORK/shell.entitlements" 2>/dev/null || :
    entitlements=()
    [ -s "$WORK/shell.entitlements" ] && entitlements=(--entitlements "$WORK/shell.entitlements")
    cp "$BIN" "$PACKAGED_EXE"
    for target in "$PACKAGED_EXE" "$WORK/packaged.app"; do
        codesign --force --options runtime ${entitlements[@]+"${entitlements[@]}"} --sign - "$target" 2>/dev/null \
            || { echo "could not re-sign $target" >&2; exit 1; }
    done
    codesign --verify --strict "$WORK/packaged.app" || { echo "the re-signed bundle does not verify" >&2; exit 1; }
    for bin in "$PACKAGED_EXE" "$WORK/packaged.app/Contents/MacOS/"*; do
        # Captured, not piped into grep -q: under pipefail, codesign's SIGPIPE would fail the test.
        signature=$(codesign -dvv "$bin" 2>&1)
        [[ $signature =~ flags=0x[0-9a-f]+\([^\)]*runtime ]] \
            || { echo "$bin is not signed with the hardened runtime" >&2; exit 1; }
    done
    after=$(codesign -d --entitlements - --xml "$PACKAGED_EXE" 2>/dev/null || :)
    [ "$after" = "$(cat "$WORK/shell.entitlements")" ] \
        || { echo "the swapped shell's entitlements differ from the packaged one's" >&2; exit 1; }
    echo "packaged: $PACKAGED, shell swapped for $BIN (hardened runtime, entitlements kept; worker untouched)"
    GRAFIDA_DIR=$WORK/packaged.app/Contents/Resources
fi
# The architecture under test: the shell's, and in a packaged bundle every executable's.
SHELL_ARCH=$(lipo -archs "$BIN")
if [ "$SHELL_ARCH" = x86_64 ] && [ "$(uname -m)" = arm64 ] && ! arch -x86_64 /usr/bin/true 2>/dev/null; then
    echo "$BIN is x86_64 and this Mac has no Rosetta 2 (a macos-*-rosetta guest has)" >&2; exit 1
fi
# What a shell under Rosetta 2 starts inherits its architecture: the test apps' fake worker would run
# the x86_64 slice of /usr/bin/python3, whose xcrun shim needs a libxcrun that macOS 27's Command
# Line Tools ship arm64 only ("missing compatible architecture"; 12's has both, grafida/grafida#92).
# The fake worker is not under test, so it stays native, like the parity client.
if [ "$SHELL_ARCH" = x86_64 ] && [ "$(uname -m)" = arm64 ]; then
    mkdir -p "$WORK/native-bin"
    printf '#!/bin/sh\nexec /usr/bin/arch -arm64 /usr/bin/python3 "$@"\n' >"$WORK/native-bin/python3"
    chmod +x "$WORK/native-bin/python3"
    export PATH="$WORK/native-bin:$PATH"
fi
mkdir -p "$WORK/home" "$WORK/exports"
printf 'PNG' >"$WORK/pick.png"
printf '# Hello\n' >"$WORK/note.md"

# ── The bridge, the navigation policy, the token flow and the Blob guard, against the echo app ──
ECHO=$SHELL_DIR/tests/apps/echo
SILENT=$SHELL_DIR/tests/apps/silent
mkdir -p "$ECHO/logs" "$SILENT/logs"
: >"$ECHO/logs/shell.log"
: >"$SILENT/logs/shell.log"
start_shell "$ECHO" test.shell.echo

answer=$(js "$ECHO/bridge.js")
record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
    "Origin, methods, bodies 0 B → 50 MiB, repeated headers, no-store, 413" \
    "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("origin %s; 50 MiB in %s ms; x-multi=%r; /api no-store=%r; over limit %s; %s; failures %s" % (r["origin"], r["bodies"][-1]["ms"], r["multi"], r["apiCacheControl"], r["overLimit"], r["info"]["webview"], r["failures"]))' <<<"$answer")"

# Rust's ARCH is fixed at compile time, so an x86_64 answer from a shell on an arm64 Mac is Rosetta 2.
running=$($CLIENT eval 'return (await (await fetch("/_native/info")).json()).arch' | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("result"))')
expected=$([ "$SHELL_ARCH" = arm64 ] && echo aarch64 || echo "$SHELL_ARCH")
translated=$([ "$running" = x86_64 ] && [ "$(uname -m)" = arm64 ] && echo ", under Rosetta 2 on $(uname -m)" || :)
bundle_archs=
bundle_ok=yes
if [ -n "$PACKAGED" ]; then
    for exe in "$WORK/packaged.app/Contents/MacOS/"*; do
        a=$(lipo -archs "$exe" 2>/dev/null || echo none)
        bundle_archs="$bundle_archs $(basename "$exe")=$a"
        [ "$a" = "$SHELL_ARCH" ] || bundle_ok=no
    done
fi
record "$( [ "$running" = "$expected" ] && [ "$bundle_ok" = yes ] && echo PASS || echo FAIL)" \
    "The shell (and a packaged bundle's executables) run as the architecture under test" \
    "binary $SHELL_ARCH; _native/info arch $running$translated${bundle_archs:+; bundle:$bundle_archs}"

answer=$(js "$ECHO/blob.js")
record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
    "Blob/File/FormData-with-file bodies → TypeError in the page; other bodies intact" \
    "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("refused: %s; delivered: %s; failures %s" % (", ".join(sorted(r["refused"])), ", ".join(sorted(r["delivered"])), r["failures"]))' <<<"$answer")"

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
visibility=$($CLIENT eval 'return document.visibilityState' | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
record "$( [ "$(ok_field <<<"$answer")" = yes ] && [ "$visibility" = visible ] && echo PASS || echo FAIL)" \
    "UI stays responsive while a 5 s request is in flight (page visible)" \
    "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("request %s ms, longest frame gap %s ms" % (r["tookMs"], r["maxFrameGapMs"]))' <<<"$answer"); visibilityState $visibility"

# Menu shortcuts, as real key presses: the page sees an Edit chord first and the menu acts on it
# when the page leaves it alone; a reserved chord reaches the page and nothing else.
reserved=$($CLIENT menu | python3 -I -c 'import json,sys; items=json.load(sys.stdin)["result"]; print([i["title"] for i in items if i["key"].lower()=="v" and "cmd" in i["modifiers"] and ("shift" in i["modifiers"] or i["key"]=="V")])')
printf 'stale' | pbcopy
$CLIENT eval 'document.body.insertAdjacentHTML("beforeend", "<textarea id=ka>copied by Cmd+C</textarea><textarea id=kb></textarea>"); window.__cmdKeys = []; document.addEventListener("keydown", (e) => { if (e.metaKey) window.__cmdKeys.push((e.shiftKey ? "shift+" : "") + e.key.toLowerCase()); }, true); const a = document.getElementById("ka"); a.focus(); a.select(); return true' >/dev/null
$CLIENT key cmd+c >/dev/null; sleep 0.7
copied=$(pbpaste)
$CLIENT eval 'document.getElementById("kb").focus(); return true' >/dev/null
$CLIENT key cmd+v >/dev/null; sleep 0.5
$CLIENT key cmd+shift+v >/dev/null; sleep 0.5
keys=$($CLIENT eval 'const r = {pasted: document.getElementById("kb").value, page: window.__cmdKeys}; document.getElementById("ka").remove(); document.getElementById("kb").remove(); return r' | python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(json.dumps(r))')
record "$( [ "$reserved" = "[]" ] && [ "$copied" = "copied by Cmd+C" ] && python3 -I -c 'import json,sys; r=json.loads(sys.argv[1]); sys.exit(0 if r == {"pasted": "copied by Cmd+C", "page": ["c", "v", "shift+v"]} else 1)' "$keys" && echo PASS || echo FAIL)" \
    "Menu shortcuts as key presses: the page sees Cmd+C/V first, then they copy and paste; Cmd+Shift+V reaches only the page" \
    "menu items on Cmd+Shift+V: $reserved; pasteboard after Cmd+C: '$copied'; $keys"

# About posts `shell:about` to the page; there is no native About panel.
$CLIENT eval 'window.__about = 0; addEventListener("shell:about", () => window.__about++); return true' >/dev/null
$CLIENT menu "About Shell echo test" >/dev/null; sleep 0.5
about=$($CLIENT eval 'return window.__about' | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
record "$( [ "$about" = 1 ] && echo PASS || echo FAIL)" \
    "App menu ▸ About posts a shell:about DOM event to the page" "events received: $about"

# Edit shortcuts go to the page first: a plain field gets the native command, a page that cancels
# the key keeps it to itself (an editor with its own undo history).
$CLIENT eval 'document.body.insertAdjacentHTML("beforeend", "<textarea id=ua></textarea>"); document.getElementById("ua").focus(); return true' >/dev/null
for k in a s a; do $CLIENT key "$k" >/dev/null; sleep 0.2; done
$CLIENT key cmd+z >/dev/null; sleep 0.6
undone=$($CLIENT eval 'return document.getElementById("ua").value' | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
$CLIENT key cmd+shift+z >/dev/null; sleep 0.6
redone=$($CLIENT eval 'return document.getElementById("ua").value' | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
$CLIENT eval 'window.__ownUndo = (e) => { if (e.metaKey && e.key === "z") e.preventDefault(); }; document.addEventListener("keydown", window.__ownUndo, true); return true' >/dev/null
$CLIENT key cmd+z >/dev/null; sleep 1.5
kept=$($CLIENT eval 'const v = document.getElementById("ua").value; document.removeEventListener("keydown", window.__ownUndo, true); document.getElementById("ua").remove(); return v' | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
record "$( [ -z "$undone" ] && [ "$redone" = asa ] && [ "$kept" = asa ] && echo PASS || echo FAIL)" \
    "Edit shortcuts reach the page first: native undo/redo in a plain field; a page that cancels Cmd+Z keeps it" \
    "typed 'asa'; Cmd+Z → '$undone'; Cmd+Shift+Z → '$redone'; Cmd+Z cancelled by the page (waited past the 1 s fallback) → '$kept'"

# Kill the worker under an in-flight request.
$CLIENT eval 'window.__slow = fetch("/sleep?ms=4000", {cache: "no-store"}).then(async r => ({status: r.status, body: await r.json()})); return true' >/dev/null
sleep 0.5
worker=$(pgrep -P "$SHELL_PID" -f fake_worker.py | head -1)
kill -9 "$worker"
answer=$($CLIENT eval 'const a = await window.__slow; const b = await fetch("/api/after", {cache: "no-store"}); return {first: a, second: b.status}')
record "$(grep -q '"worker_restarted"' <<<"$answer" && grep -q '"second": 200' <<<"$answer" && echo PASS || echo FAIL)" \
    "Kill the worker → in-flight request gets 502 worker_restarted, app recovers" \
    "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("in flight: %s %s; next request: %s" % (r["first"]["status"], r["first"]["body"], r["second"]))' <<<"$answer")"

# SIGKILL the shell mid-request: the worker has no parent-death signal on macOS (grafida/grafida#88)
# and ends when its request does, its reply hitting a closed pipe.
$CLIENT eval 'fetch("/sleep?ms=3000", {cache: "no-store"}); return true' >/dev/null
sleep 0.5
worker=$(pgrep -P "$SHELL_PID" -f fake_worker.py | head -1)
kill -9 "$SHELL_PID"
SHELL_PID=
sleep 1
during=$(kill -0 "$worker" 2>/dev/null && echo alive || echo gone)
for _ in $(seq 1 60); do kill -0 "$worker" 2>/dev/null || break; sleep 0.1; done
after=$(kill -0 "$worker" 2>/dev/null && echo alive || echo gone)
record "$( [ "$after" = gone ] && echo PASS || echo FAIL)" \
    "SIGKILL the shell mid-request → the worker exits when that request ends" \
    "1 s after the kill (request at 1.5 of 3 s): $during; 7 s after: $after"
kill -9 "$worker" 2>/dev/null

# Four kills inside the window: the supervisor gives up with an alert (a sheet on the window).
start_shell "$ECHO" test.shell.echo
for _ in 1 2 3 4; do
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
record "$( [ "$gave_up" -ge 1 ] && kill -0 "$SHELL_PID" 2>/dev/null && echo PASS || echo FAIL)" \
    "4 kills in 60 s → native alert with the stderr tail" \
    "log: $(grep "giving up on the worker" "$ECHO/logs/shell.log" | tail -1 | cut -c26-150)"
kill "$SHELL_PID" 2>/dev/null
SHELL_PID=

# Cmd+Q, as a key press: the shell's own stop path, the worker shut down gracefully.
start_shell "$ECHO" test.shell.echo
worker=$(pgrep -P "$SHELL_PID" -f fake_worker.py | head -1)
$CLIENT key cmd+q >/dev/null
for _ in $(seq 1 50); do kill -0 "$SHELL_PID" 2>/dev/null || break; sleep 0.1; done
quit=$(kill -0 "$SHELL_PID" 2>/dev/null && echo "still running" || echo "quit")
record "$( [ "$quit" = quit ] && ! kill -0 "$worker" 2>/dev/null && echo PASS || echo FAIL)" \
    "Cmd+Q quits through the shell's stop path; the worker exits" \
    "shell: $quit; worker: $(kill -0 "$worker" 2>/dev/null && echo running || echo gone); log: $(grep "worker stopped" "$ECHO/logs/shell.log" | tail -1 | cut -c26-80)"
kill "$SHELL_PID" 2>/dev/null
SHELL_PID=

# A worker that never says `ready`.
(cd "$SILENT" && exec env HOME="$WORK/home" CFFIXED_USER_HOME="$WORK/home" "$BIN" --dev . >>"$WORK/shell-silent.log" 2>&1) &
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
    LOG="$WORK/home/Library/Logs/grafida/shell.log"
    DATA="$WORK/home/Library/Application Support/Grafida"
    start_shell "$GRAFIDA_DIR" app.grafida
    $CLIENT wait 'return !!document.querySelector("#sidebar")' 30 >/dev/null
    for p in "$WORK/pick.png" --cancel "$WORK/note.md" "$WORK/exports"; do $CLIENT pick "$p" >/dev/null; done
    answer=$($CLIENT eval-file "$SHELL_DIR/scripts/dev/parity/grafida.js")
    opened=$($CLIENT opened | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
    record "$( [ "$(ok_field <<<"$answer")" = yes ] && echo PASS || echo FAIL)" \
        "Grafida: SPA loads at boson://app; file open (image/markdown), cancel, select folder → export" \
        "$(ok_field <<<"$answer"); export: $(find "$WORK/exports" -type f -exec basename {} \; | head -1)"
    record "$( [ "$opened" != "[]" ] && echo PASS || echo FAIL)" \
        "Grafida: Help screen renders the bundled docs; an external Help link → OS browser" "to the browser: $opened"
    token=$(python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"]["folderToken"])' <<<"$answer")
    $CLIENT screenshot "$WORK/grafida.png" >/dev/null

    # The editor: paste as plain text, and an image file dropped on it.
    python3 -I - "$DATA/grafida.sqlite" <<'PY'
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
        "Grafida: Cmd+Shift+V path pastes the clipboard as plain text (the shell reads NSPasteboard)" "$(ok_field <<<"$answer")"
    # The same chord as a real key press into the editor: no menu item takes it, the page does.
    printf 'typed chord' | pbcopy
    $CLIENT eval 'const ed = State.tinyMCEEditor; ed.setContent("<p>start</p>"); ed.focus(); ed.selection.select(ed.getBody(), true); ed.selection.collapse(false); return true' >/dev/null
    $CLIENT key cmd+shift+v >/dev/null
    sleep 1.5
    chord=$($CLIENT eval 'return State.tinyMCEEditor.getContent()' | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
    record "$( [ "$chord" = "<p>starttyped chord</p>" ] && echo PASS || echo FAIL)" \
        "Grafida: Cmd+Shift+V pressed in the editor reaches the page (no menu item takes it) and pastes plain text" "$chord"
    # The editor keeps its own undo history: Cmd+Z undoes a paste and a plain-text paste.
    printf 'PASTED' | pbcopy
    $CLIENT eval 'const ed = State.tinyMCEEditor; ed.setContent("<p>start</p>"); ed.undoManager.clear(); ed.undoManager.add(); ed.focus(); ed.selection.select(ed.getBody(), true); ed.selection.collapse(false); return true' >/dev/null
    content() { $CLIENT eval 'return State.tinyMCEEditor.getContent()' | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])'; }
    $CLIENT key cmd+v >/dev/null; sleep 1; pasted=$(content)
    $CLIENT key cmd+z >/dev/null; sleep 0.8; undo1=$(content)
    printf 'PLAIN' | pbcopy
    $CLIENT key cmd+shift+v >/dev/null; sleep 1.5; plain=$(content)
    $CLIENT key cmd+z >/dev/null; sleep 0.8; undo2=$(content)
    record "$( [ "$pasted" = "<p>startPASTED</p>" ] && [ "$undo1" = "<p>start</p>" ] && [ "$plain" = "<p>startPLAIN</p>" ] && [ "$undo2" = "<p>start</p>" ] && echo PASS || echo FAIL)" \
        "Grafida: Cmd+Z in the editor undoes a Cmd+V paste and a Cmd+Shift+V plain-text paste" \
        "Cmd+V → $pasted; Cmd+Z → $undo1; Cmd+Shift+V → $plain; Cmd+Z → $undo2"
    # Text another app put there: no prompt, no helper tool.
    printf 'from another app' | pbcopy
    pasted=$($CLIENT eval 'const r = await fetch("/api/clipboard/text", {headers: {"X-Shell-Clipboard": "read"}, cache: "no-store"}); return (await r.json()).data.text')
    record "$(grep -q '"from another app"' <<<"$pasted" && echo PASS || echo FAIL)" \
        "Grafida: the clipboard read gets another app's text without a prompt" "$(tr -d '\n' <<<"$pasted" | cut -c1-120)"
    read -r dx dy < <(python3 -I -c 'import json,sys; print(*json.load(sys.stdin)["result"]["dropAt"])' <<<"$answer")
    $CLIENT drop "$dx" "$dy" "$WORK/dropped.png" >/dev/null
    sleep 2
    dropped=$($CLIENT eval 'return [...State.tinyMCEEditor.getBody().querySelectorAll("img[data-grafida-media-id]")].map(i => i.getAttribute("src"))')
    record "$(grep -q 'boson://app/api/media/' <<<"$dropped" && echo PASS || echo FAIL)" \
        "Grafida: an image file dropped on the editor is inserted as offline media (delivered as in a real drop)" \
        "$(tr -d '\n ' <<<"$dropped" | cut -c1-120)"
    # Leaving by script keeps the keyboard focus in the hidden editor's iframe, whose caret then
    # blinks unseen while the app is frontmost (the key rows make it so): ~1.7 % of a core between
    # the shell and WebContent, measured. A real click on the sidebar moves the focus out, so do that.
    $CLIENT eval 'State.tinyMCEEditor.setDirty(false); showScreen("sites"); document.activeElement.blur(); window.focus(); return true' >/dev/null

    # Idle CPU of the shell, its worker and WebKit's helpers on a quiet screen.
    settle=60 sample=60
    [ -n "${QUICK:-}" ] && settle=5 sample=10
    $CLIENT eval 'document.querySelector("#main-nav [data-screen=\"sites\"]").click(); return true' >/dev/null
    sleep "$settle"
    visibility=$($CLIENT eval 'return document.visibilityState' | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"])')
    pids="$(tree "$SHELL_PID" | tr '\n' ' ')$(webkit_helpers | tr '\n' ' ')"
    # bash 3.2 (a clean macOS): no associative arrays, so the first sample is kept in a list.
    before=""
    for p in $pids; do before+="$p=$(cpu_seconds "$p") "; done
    s0=$(date +%s); sleep "$sample"; s1=$(date +%s)
    cpu=0 shares=""
    for p in $pids; do
        t0=$(tr ' ' '\n' <<<"$before" | sed -n "s/^$p=//p")
        d=$(echo "$(cpu_seconds "$p") - ${t0:-0}" | bc)
        cpu=$(echo "$cpu + $d" | bc)
        name=$(ps -o comm= -p "$p" 2>/dev/null | sed 's|.*/||; s/^com\.apple\.WebKit\.//')
        shares+="${name:-gone} $(printf "%.2f" "$(echo "scale=4; $d * 100 / ($s1 - $s0)" | bc)"); "
    done
    cpu=$(printf "%.2f" "$(echo "scale=4; $cpu * 100 / ($s1 - $s0)" | bc)")
    record "$(python3 -I -c "print('PASS' if $cpu < 1 and '$visibility' == 'visible' else 'FAIL')")" \
        "Idle CPU < 1% (shell, worker and WebKit helpers, ${sample} s after ${settle} s; page visible)" \
        "$cpu % of one core: ${shares%; }; visibilityState $visibility"

    # SIGKILL the shell while the worker is idle: it reads EOF and exits at once.
    worker=$(grep -o "worker started, pid [0-9]*" "$LOG" | tail -1 | grep -o "[0-9]*$")
    kill -9 "$SHELL_PID"
    SHELL_PID=
    for _ in $(seq 1 30); do kill -0 "$worker" 2>/dev/null || break; sleep 0.1; done
    record "$(kill -0 "$worker" 2>/dev/null && echo FAIL || echo PASS)" \
        "Grafida: SIGKILL the shell with the worker idle → the worker exits at once (stdin EOF)" \
        "worker $worker $(kill -0 "$worker" 2>/dev/null && echo 'still running after 3 s' || echo 'gone within 3 s')"
    kill -9 "$worker" 2>/dev/null

    # Close → the worker exits within 3 s, nothing is orphaned, the WAL does not grow.
    start_shell "$GRAFIDA_DIR" app.grafida
    worker=$(grep -o "worker started, pid [0-9]*" "$LOG" | tail -1 | grep -o "[0-9]*$")
    started=$(python3 -I -c 'import time; print(time.time())')
    stop_shell
    for _ in $(seq 1 40); do kill -0 "$worker" 2>/dev/null || break; sleep 0.1; done
    took=$(python3 -I -c "import time; print(f'{time.time() - $started:.2f}')")
    alive=$(kill -0 "$worker" 2>/dev/null && echo yes || echo no)
    wal=()
    for _ in 1 2 3; do
        start_shell "$GRAFIDA_DIR" app.grafida
        $CLIENT eval 'return (await fetch("/api/settings/spell-check", {method: "POST", body: "{\"enabled\":true}"})).ok' >/dev/null
        stop_shell
        sleep 0.5
        wal+=("$(stat -f %z "$DATA/"*-wal 2>/dev/null | head -1 || echo 0)")
    done
    orphans=$(pgrep -u "$(id -u)" -f "bin/worker.php|grafida-worker" | while read -r p; do ps eww -p "$p" -o command= 2>/dev/null | grep -q "HOME=$WORK/home" && echo "$p"; done)
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
        "DevTools only with GRAFIDA_DEBUG=1 (WebKit setting; Inspect Element itself is a manual check)" \
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
    grafida_rev="packaged $(basename "$PACKAGED")"
else
    grafida_rev="$(cd "$GRAFIDA_DIR" 2>/dev/null && git rev-parse --short HEAD)$( (cd "$GRAFIDA_DIR" 2>/dev/null && git diff --quiet) || echo '+dirty')"
fi
webkit=$(grep -ho 'on WebKit [0-9.]*' "$WORK"/home/Library/Logs/grafida/shell.log 2>/dev/null | tail -1)
safari=$(defaults read /Applications/Safari.app/Contents/Info CFBundleShortVersionString 2>/dev/null)
echo "shell $(cd "$SHELL_DIR" && git rev-parse --short HEAD 2>/dev/null)$( (cd "$SHELL_DIR" && git diff --quiet 2>/dev/null) || echo '+dirty'), grafida $grafida_rev, ${webkit#on } (Safari ${safari:-?}), macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion)), $(uname -m), shell $SHELL_ARCH, $(date -u +%Y-%m-%dT%H:%MZ)"
grep -q '^FAIL' <(printf '%s\n' "${RESULTS[@]}") && exit 1 || exit 0
