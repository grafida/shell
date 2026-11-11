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
#   QUICK=1          a 10 s idle-CPU sample after 30 s, instead of 60 s after 60 s
#
#   EVIDENCE         where the screenshots go (default target/parity-windows)
#
# The guest needs the `dev` snapshot's Python (the fake worker, the database seed). Nothing here is
# left to a human: real keys, clicks and drags go through SendInput (the automation channel's
# type/key/mouse/drag), native dialogs, File Explorer, the clipboard, the cursor and the Windows
# colour mode through desktop-session helpers (scripts/dev/winvm/desk/, UI Automation), and AI
# streaming through a test server on the host. The host needs ImageMagick (`magick`) for the
# screenshot checks.

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
    if [ -z "$SHELL_PID" ]; then
        record FAIL "Start $id" "winvm.sh run refused or failed; the rows that follow are not valid"
        return 1
    fi
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

DESK="$SHELL_DIR/scripts/dev/winvm/desk"
EVIDENCE=${EVIDENCE:-$SHELL_DIR/target/parity-windows}
rm -rf "$EVIDENCE"; mkdir -p "$EVIDENCE"
desk() { local s=$1; shift; "$W" desktop "$VM" "$DESK/$s.ps1" "$@" 2>&1 | tr -d '\r'; }
shot() { desk screen "C:\\winvm\\desk\\$1.png" >/dev/null && "$W" get "$VM" "C:\\winvm\\desk\\$1.png" "$EVIDENCE/$1.png"; }
# Red pixels (a spelling underline) in a page rectangle of a desktop screenshot: png x y w h.
red_in() {
    python3 -I - "$1" "$CX" "$CY" "$SCALE" "$2" "$3" "$4" "$5" <<'RED'
import subprocess, sys
png = sys.argv[1]; cx, cy, s, x, y, w, h = map(float, sys.argv[2:])
box = f"{int(w * s)}x{int(h * s)}+{int(cx + x * s)}+{int(cy + y * s)}"
raw = subprocess.run(["magick", png, "-crop", box, "+repage", "-depth", "8", "rgb:-"], capture_output=True, check=True).stdout
print(sum(1 for i in range(0, len(raw) - 2, 3) if raw[i] > 190 and raw[i + 1] < 110 and raw[i + 2] < 110))
RED
}

ok_field() { python3 -I -c 'import json,sys; a=json.load(sys.stdin); r=a.get("result") or {}; print("yes" if a.get("ok") and (r.get("ok", True) is True) and not r.get("failures") else "no: " + json.dumps(r.get("failures", a))[:300])'; }

trap 'close_tunnel; kill ${AI_SERVER:-} ${AI_TLS_SERVER:-} 2>/dev/null; "$W" stop-run "$VM" >/dev/null 2>&1' EXIT

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
    start_shell "$GS\\grafida-shell.exe" test.shell.echo 0 --dev "$ECHO" || continue
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
export WINVM_ENV="SHELL_WINDOWS_MENU_LOG=1"
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

# ── What used to be a by-hand pass (grafida/grafida#91): real input through SendInput, native
# windows through UI Automation, the desktop as a user sees it. Screenshots go to $EVIDENCE. ──────
read -r CX CY SCALE < <($CLIENT window | python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(r["client"][0], r["client"][1], r["dpi"] / 96)')
# The editor's paragraph (or one word of it) in page pixels: x y w h.
$CLIENT eval 'window.__rect = (w) => { const ed = State.tinyMCEEditor; const p = ed.getBody().querySelector("p"); const f = ed.iframeElement.getBoundingClientRect(); let r = p.getBoundingClientRect(); if (w) { const t = p.firstChild; const i = t.data.indexOf(w); const g = ed.getDoc().createRange(); g.setStart(t, i); g.setEnd(t, i + w.length); r = g.getBoundingClientRect(); } return [Math.round(f.left + r.left), Math.round(f.top + r.top), Math.round(r.width), Math.round(r.height)]; }; window.__fresh = (html) => { const ed = State.tinyMCEEditor; ed.setContent(html); ed.undoManager.clear(); ed.focus(); ed.selection.select(ed.getBody(), true); ed.selection.collapse(false); return true; }; return true' >/dev/null
content() { $CLIENT eval 'return State.tinyMCEEditor.getContent()' | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("result"))'; }
rect() { $CLIENT eval "return __rect(${1:-})" | python3 -I -c 'import json,sys; print(*json.load(sys.stdin)["result"])'; }
centre() { python3 -I -c 'import sys; x, y, w, h = map(float, sys.argv[1:]); print(round(x + w / 2), round(y + h / 2))' "$@"; }
menu_line() { gps "(Select-String -Encoding UTF8 -Path '$LOG' -Pattern '\\[menu\\] context menu items' | Select -Last 1).Line" | sed 's/^.*context menu items: //'; }

# Spelling: words typed with real keys are underlined; Ctrl+right-click (TinyMCE keeps a plain
# right click for its own menu) shows WebView2's menu with the suggestions; choosing one replaces
# the word. ⚠️ A real key press is what wakes the spell checker: Unicode injection alone
# (KEYEVENTF_UNICODE, a VK_PACKET) types the text but leaves it unchecked, hence the trailing space.
# ⚠️ WebView2 checks the user's primary language only (its profile's spellcheck.dictionaries is
# ["el"] on the Greek guest, with US English spell checking installed and in the Windows language
# list too), so the misspellings are in the guest's language.
if [[ $($CLIENT eval 'return navigator.languages[0]' | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("result"))') == el* ]]; then
    SPELL_TEXT=' το κείμενο είναι λάθως γραμένο' SPELL_WORD='λάθως' SPELL_MORE=' λέξι'
else
    SPELL_TEXT=' the wurdz are wrnog' SPELL_WORD='wurdz' SPELL_MORE=' qwertz'
fi
$CLIENT eval 'return __fresh("<p>Start</p>")' >/dev/null
$CLIENT type "$SPELL_TEXT" >/dev/null
$CLIENT key space >/dev/null
# ⚠️ After a cold boot the first text typed can stay unchecked (seen on both guests: the Windows
# spell checker still starting, the request lost); the next edit gets it all checked, earlier words
# included. So, as a user would, keep typing: another misspelling between looks.
for _ in 1 2 3 4; do
    sleep 2
    read -r lx ly lw lh < <(rect)
    shot spell-on
    red_on=$(red_in "$EVIDENCE/spell-on.png" "$lx" "$ly" "$lw" "$((lh + 6))")
    [ "${red_on:-0}" -gt 30 ] && break
    $CLIENT type "$SPELL_MORE" >/dev/null
    $CLIENT key space >/dev/null
    sleep 6
done
read -r wx wy < <(centre $(rect "\"$SPELL_WORD\""))
$CLIENT mouse "$wx" "$wy" ctrl+right >/dev/null
sleep 1.5
shot spell-menu
menu=$(menu_line)
# WebView2 gives a suggestion an empty label, so what was offered is counted, and what Enter chose
# (the first, highlighted) is read back from the article.
suggestions=$(grep -o 'spellcheck=' <<<"${menu#*kept: }" | wc -l)
# Enter takes the highlighted first item: only a suggestion (on a menu without any it is Emoji,
# whose panel would then swallow the typing of the next row).
if [ "$suggestions" -ge 1 ]; then $CLIENT key enter >/dev/null; else $CLIENT key esc >/dev/null; fi
sleep 1
# TinyMCE writes non-ASCII letters as named entities (&lambda;): compared as text.
fixed=$(content | python3 -I -c 'import html,sys; print(html.unescape(sys.stdin.read()).strip())')
replaced=$(python3 -I -c 'import sys; before, after = sys.argv[1].split(), sys.argv[2].replace("<p>", " ").replace("</p>", " ").replace("&nbsp;", " ").split(); print(next((a for b, a in zip(before, after) if b == sys.argv[3] and a != b), ""))' "Start$SPELL_TEXT" "$fixed" "$SPELL_WORD")
record "$( [ "${red_on:-0}" -gt 30 ] && [ "$suggestions" -ge 1 ] && [ -n "$replaced" ] && ! grep -qF -- "$SPELL_WORD" <<<"$fixed" && echo PASS || echo FAIL)" \
    "Spelling: misspellings typed with real keys are underlined; Ctrl+right-click offers WebView2's suggestions; choosing one replaces the word" \
    "red underline pixels ${red_on:-?}; ${suggestions} suggestions offered; '$SPELL_WORD' became '${replaced}': $fixed; menu kept: ${menu#*kept: }"

$CLIENT eval 'await applySpellCheckChange(false); return true' >/dev/null
$CLIENT eval "return __fresh('<p>Start$SPELL_TEXT</p>')" >/dev/null
$CLIENT type "$SPELL_MORE" >/dev/null
$CLIENT key space >/dev/null
sleep 2
read -r lx ly lw lh < <(rect)
shot spell-off
red_off=$(red_in "$EVIDENCE/spell-off.png" "$lx" "$ly" "$lw" "$((lh + 6))")
$CLIENT eval 'await applySpellCheckChange(true); return true' >/dev/null
record "$( [ "${red_off:-99}" -lt 5 ] && echo PASS || echo FAIL)" \
    "Spelling off (the setting): nothing is underlined, typed text included" "red underline pixels ${red_off:-?}"

# Real typing in the title field (a click, then Unicode text), and undo/redo in the editor.
read -r tx ty < <($CLIENT eval 'const r = document.getElementById("editor-title-input").getBoundingClientRect(); return [Math.round(r.left + r.width / 2), Math.round(r.top + r.height / 2)]' | python3 -I -c 'import json,sys; print(*json.load(sys.stdin)["result"])')
$CLIENT mouse "$tx" "$ty" >/dev/null
$CLIENT type 'Ελληνικά & ünïcode — ok' >/dev/null
title=$($CLIENT eval 'return document.getElementById("editor-title-input").value' | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("result"))')
$CLIENT eval 'return __fresh("<p>one</p>")' >/dev/null
for k in space t w o; do $CLIENT key "$k" >/dev/null; done
typed=$(content)
$CLIENT key ctrl+z >/dev/null; sleep 0.5; undone=$(content)
$CLIENT key ctrl+y >/dev/null; sleep 0.5; redone=$(content)
# Virtual keys go through the guest's keyboard layout (t w o types τ σ ο on a Greek one), so the
# check is that something was typed, undone and redone.
record "$( [ "$title" = 'Ελληνικά & ünïcode — ok' ] && [ "$typed" != "<p>one</p>" ] && grep -q '^<p>one ' <<<"$typed" && [ "$undone" = "<p>one</p>" ] && [ "$redone" = "$typed" ] && echo PASS || echo FAIL)" \
    "Real keys: a click into the title and typing (Greek, accents, a dash); in the editor Ctrl+Z undoes and Ctrl+Y redoes" \
    "title '$title'; typed $typed; Ctrl+Z $undone; Ctrl+Y $redone"

# The clipboard as another program leaves it (HTML + text, set in the desktop session): Ctrl+V
# keeps the formatting, the Ctrl+Shift+V chord reaches the page and pastes the text alone.
desk clipboard 'Pasted <strong>bold</strong> text' 'Pasted bold text' >/dev/null
$CLIENT eval 'return __fresh("<p>start</p>")' >/dev/null
$CLIENT key ctrl+v >/dev/null
sleep 1.5
formatted=$(content)
record "$(grep -q '<strong>bold</strong>' <<<"$formatted" && echo PASS || echo FAIL)" \
    "Grafida: Ctrl+V of formatted text from another program keeps its formatting" "$formatted"
$CLIENT eval 'return __fresh("<p>start</p>")' >/dev/null
keyed=$($CLIENT key ctrl+shift+v 2>&1 | tr -d '\n ')
sleep 1.5
chord=$(content)
record "$( [ "$chord" = "<p>startPasted bold text</p>" ] && echo PASS || echo FAIL)" \
    "Grafida: the real Ctrl+Shift+V chord in the editor reaches the page and pastes plain text" "$chord; key: $(cut -c1-100 <<<"$keyed")"

# The drop point now: the rows above have changed the page since grafida-editor.js measured it.
$CLIENT eval 'return __fresh("<p>drop here</p>")' >/dev/null
read -r dx dy < <($CLIENT eval 'const f = State.tinyMCEEditor.iframeElement.getBoundingClientRect(); return [Math.round(f.left + f.width / 2), Math.round(f.top + 40)]' | python3 -I -c 'import json,sys; print(*json.load(sys.stdin)["result"])')
$CLIENT drop "$dx" "$dy" 'C:\winvm\files\dropped.png' >/dev/null
sleep 2
# TinyMCE's own drop handling inserts the image (no data-grafida-media-id, which only Grafida's
# shell:drop path adds); publishing uploads every offline image by its URL, tagged or not.
dropped=$($CLIENT eval 'return [...State.tinyMCEEditor.getBody().querySelectorAll("img[src^=\"boson://app/api/media/\"]")].map(i => i.getAttribute("src"))')
record "$(grep -q 'boson://app/api/media/' <<<"$dropped" && echo PASS || echo FAIL)" \
    "Grafida: an image file dropped on the editor is inserted as offline media (File objects, as WebView2 delivers a drop)" \
    "$(tr -d '\n ' <<<"$dropped" | cut -c1-120)"

# A real drag of a file out of File Explorer onto the editor (an OLE drop, which WebView2 turns
# into the page's drop event).
$CLIENT eval 'return __fresh("<p>drop here</p>")' >/dev/null
images() { $CLIENT eval 'return State.tinyMCEEditor.getBody().querySelectorAll("img[src^=\"boson://app/api/media/\"]").length' | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("result"))'; }
before=$(images)
from=$(desk explorer 'C:\winvm\files\dropped.png' | grep -o 'at [0-9]* [0-9]*')
read -r px py < <($CLIENT eval 'const f = State.tinyMCEEditor.iframeElement.getBoundingClientRect(); return [Math.round(f.left + f.width * 0.8), Math.round(f.top + 120)]' | python3 -I -c 'import json,sys; print(*json.load(sys.stdin)["result"])')
[ -n "$from" ] && $CLIENT drag ${from#at } "$px" "$py" >/dev/null
sleep 2
shot drag
after=$(images)
desk windows CabinetWClass close >/dev/null
record "$( [ -n "$from" ] && [ "${after:-0}" -gt "${before:-0}" ] && echo PASS || echo FAIL)" \
    "Grafida: a file dragged out of File Explorer with the real mouse and dropped on the editor is inserted" \
    "Explorer item ${from:-not found}; offline images in the editor ${before} → ${after}"

# The real native dialogs, driven through UI Automation as a user would: Open (accept, cancel),
# Select Folder and Save. Their titles and buttons are recorded (a Greek guest shows Greek).
dialog() { # page call, then the dialog's answer (accept PATH | cancel)
    $CLIENT eval "window.__d = $1.then(r => r, e => ({error: String(e && (e.code || e.message) || e)})); return true" >/dev/null
    local seen
    seen=$(desk dialog "$SHELL_PID" "$2" "${3:-}" | tr '\n' ';')
    echo "$seen"
    $CLIENT eval 'return await window.__d' | python3 -I -c 'import json,sys; print(json.dumps(json.load(sys.stdin).get("result")))'
}
open_ok=$(dialog 'GrafidaShell.openFile({filters: []})' accept 'C:\winvm\files\dropped.png')
open_no=$(dialog 'GrafidaShell.openFile({filters: []})' cancel)
folder=$(dialog 'GrafidaShell.selectFolder()' accept 'C:\winvm\files\exports')
save=$(dialog 'GrafidaShell.saveFile({suggestedName: "parity.txt"})' accept 'C:\winvm\files\exports\parity.txt')
record "$(grep -q '"name": "dropped.png"' <<<"$open_ok" && grep -q '"token"' <<<"$open_ok" && grep -qi 'cancel' <<<"$(tail -1 <<<"$open_no")" && grep -q '"name": "exports"' <<<"$folder" && grep -q '"name": "parity.txt"' <<<"$save" && echo PASS || echo FAIL)" \
    "The real native dialogs (UI Automation): Open → a token for the file; Cancel → cancelled; Select Folder and Save → a token each" \
    "open: $(tr '\n' ' ' <<<"$open_ok"); cancel: $(tr '\n' ' ' <<<"$open_no"); folder: $(tr '\n' ' ' <<<"$folder"); save: $(tr '\n' ' ' <<<"$save")"

# Reveal a picked file: an Explorer window on its folder.
$CLIENT pick 'C:\winvm\files\dropped.png' >/dev/null
revealed=$($CLIENT eval 'const r = await GrafidaShell.openFile({filters: []}); return await GrafidaShell.reveal(r.files[0].token)' | tr -d '\n ')
sleep 2
shown=$(desk windows CabinetWClass close | tr '\n' ';')
record "$(grep -q '"ok":true' <<<"$revealed" && grep -qE '(^|;)files( - [^|;]*)? \|' <<<"$shown" && echo PASS || echo FAIL)" \
    "Reveal a picked file → File Explorer opens on its folder" "answer $revealed; Explorer windows: ${shown:-none}"

# A real link: recording off, the sidebar's Visit site button clicked with the real mouse, and
# the URL reaches the OS browser (Edge, on a clean guest).
$CLIENT record off >/dev/null
gps "Stop-Process -Name msedge -Force -ErrorAction SilentlyContinue" >/dev/null   # only the click may start it
read -r vx vy < <($CLIENT eval 'const b = [...document.querySelectorAll("#sidebar button")].find(b => b.textContent.includes(t("GRAFIDA_BTN_OPEN_SITE"))); const r = b.getBoundingClientRect(); return [Math.round(r.left + r.width / 2), Math.round(r.top + r.height / 2)]' | python3 -I -c 'import json,sys; print(*json.load(sys.stdin)["result"])')
$CLIENT mouse "$vx" "$vy" >/dev/null
browser=$(gps "for (\$i = 0; \$i -lt 100; \$i++) { \$p = Get-CimInstance Win32_Process -Filter \"Name='msedge.exe'\" | ? { \$_.CommandLine -match 'example\\.invalid' } | Select -First 1; if (\$p) { break }; Start-Sleep -Milliseconds 200 }; if (\$p) { \$p.Name + ' ' + (\$p.CommandLine -replace '^.*(https?://\\S+).*$', '\$1') }")
gps "Stop-Process -Name msedge -Force -ErrorAction SilentlyContinue" >/dev/null
$CLIENT record on >/dev/null
record "$(grep -q 'example.invalid' <<<"$browser" && echo PASS || echo FAIL)" \
    "Visit site (a real click) opens the site in the OS browser" "${browser:-no browser process with the URL within 20 s}"

# Theme: dark and light from the app, and auto following a Windows mode switch (the SPA's own
# 30 s re-probe); the page and the title bar (DWMWA_USE_IMMERSIVE_DARK_MODE) both follow.
theme_state() { echo "$($CLIENT eval 'return document.documentElement.dataset.theme' | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("result"))')/$($CLIENT window | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"]["darkTitleBar"])')"; }
wait_theme() { for _ in $(seq 1 45); do [ "$($CLIENT eval 'return document.documentElement.dataset.theme' | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("result"))')" = "$1" ] && break; sleep 1; done; sleep 0.5; theme_state; }
os_light=$(gps "(Get-ItemProperty HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize).AppsUseLightTheme")
$CLIENT eval 'await applyDisplayModeChange("dark", {silent: true}); return true' >/dev/null; sleep 1; dark=$(theme_state)
shot theme-dark
$CLIENT eval 'await applyDisplayModeChange("light", {silent: true}); return true' >/dev/null; sleep 1; light=$(theme_state)
desk theme light >/dev/null
$CLIENT eval 'await applyDisplayModeChange("auto", {silent: true}); return true' >/dev/null
desk theme dark >/dev/null; auto_dark=$(wait_theme dark)
desk theme light >/dev/null; auto_light=$(wait_theme light)
[ "$os_light" = 0 ] && desk theme dark >/dev/null
record "$( [ "$dark" = dark/True ] && [ "$light" = light/False ] && [ "$auto_dark" = dark/True ] && [ "$auto_light" = light/False ] && echo PASS || echo FAIL)" \
    "Theme: dark and light, and auto following a Windows mode switch; the title bar follows the page" \
    "page/dark title bar — dark $dark; light $light; auto with Windows dark $auto_dark, then light $auto_light"

# The editing keys a user reaches for: Ctrl+A, Ctrl+C, Ctrl+X and Ctrl+V of our own copy in the
# editor and the title (the clipboard read back from the desktop), and Ctrl+Z after a paste from
# another program.
$CLIENT eval 'return __fresh("<p>alpha beta</p>")' >/dev/null
$CLIENT key ctrl+a >/dev/null; $CLIENT key ctrl+c >/dev/null; sleep 0.5
copied=$(desk clipboard-read)
$CLIENT key ctrl+x >/dev/null; sleep 0.5; cut=$(content)
$CLIENT key ctrl+v >/dev/null; sleep 1; pasted=$(content)
read -r tx ty < <($CLIENT eval 'const i = document.getElementById("editor-title-input"); i.value = ""; const r = i.getBoundingClientRect(); return [Math.round(r.left + r.width / 2), Math.round(r.top + r.height / 2)]' | python3 -I -c 'import json,sys; print(*json.load(sys.stdin)["result"])')
$CLIENT mouse "$tx" "$ty" >/dev/null
$CLIENT type 'Title words' >/dev/null
$CLIENT key ctrl+a >/dev/null; $CLIENT key ctrl+x >/dev/null; sleep 0.3
title_cut=$($CLIENT eval 'return document.getElementById("editor-title-input").value' | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("result"))')
title_clip=$(desk clipboard-read)
$CLIENT key ctrl+v >/dev/null; sleep 0.3
title_back=$($CLIENT eval 'return document.getElementById("editor-title-input").value' | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("result"))')
desk clipboard 'Pasted <strong>bold</strong> text' 'Pasted bold text' >/dev/null
$CLIENT eval 'const ed = State.tinyMCEEditor; ed.setContent("<p>start</p>"); ed.undoManager.clear(); ed.undoManager.add(); ed.focus(); ed.selection.select(ed.getBody(), true); ed.selection.collapse(false); return true' >/dev/null
$CLIENT key ctrl+v >/dev/null; sleep 1; other=$(content)
$CLIENT key ctrl+z >/dev/null; sleep 0.5; unpasted=$(content)
record "$( [ "$copied" = '"alpha beta"' ] && ! grep -q alpha <<<"$cut" && grep -q 'alpha beta' <<<"$pasted" && [ -z "$title_cut" ] && [ "$title_clip" = '"Title words"' ] && [ "$title_back" = 'Title words' ] && grep -q '<strong>bold</strong>' <<<"$other" && [ "$unpasted" = '<p>start</p>' ] && echo PASS || echo FAIL)" \
    "Real keys: Ctrl+A/C/X/V in the editor and the title; Ctrl+Z after a paste from another program" \
    "copied $copied; after Ctrl+X '$cut'; Ctrl+V $pasted; title cut to '$title_cut', clipboard $title_clip, pasted back '$title_back'; pasted $other, Ctrl+Z $unpasted"

# ⚠️ Known, and not the shell's: Ctrl+Z after a Ctrl+X records no undo level, so the cut text cannot
# be brought back. It reproduces in plain Chromium 156 against the same SPA (grafida/grafida#91),
# so it is the editor on Chromium, Boson's WebView2 included. Reported, not counted.
$CLIENT eval 'return __fresh("<p>alpha beta</p>")' >/dev/null
$CLIENT key ctrl+a >/dev/null; $CLIENT key ctrl+x >/dev/null; sleep 0.7
$CLIENT key ctrl+z >/dev/null; sleep 0.7; after_cut=$(content)
record NOTE "Ctrl+Z after Ctrl+X brings the cut text back (a known editor bug on Chromium, not counted)" \
    "$( [ "$after_cut" = '<p>alpha beta</p>' ] && echo "now it does: $after_cut" || echo "still not: '$after_cut'")"

# AI streaming from a local model: test-server.py on the host, which the guest reaches at QEMU's
# 10.0.2.2, over plain HTTP. The panel's direct fetch must stream (many partial renders), not fall
# back to the PHP proxy.
python3 -I "$SHELL_DIR/scripts/dev/macguest/test-server.py" 8765 >"$EVIDENCE/ai-server.log" 2>&1 &
AI_SERVER=$!
$CLIENT eval 'const b = {name: "Parity AI", provider: "custom", endpoint: "http://10.0.2.2:8765/v1", model: "fake-model", params: {stream: true}, key: "parity-key"}; await createAiServiceWithInsecureFallback(b); State.tinyMCEEditor.setDirty(false); location.reload(); return true' >/dev/null 2>&1
sleep 2
$CLIENT wait 'return !!document.querySelector("#sidebar") && State.sites && State.sites.length > 0 && State.aiDefaultServiceId' 30 >/dev/null
$CLIENT eval 'openNewArticle(); for (let i = 0; i < 100 && !(State.tinyMCEEditor && State.tinyMCEEditor.initialized); i++) await new Promise(r => setTimeout(r, 100)); State.tinyMCEEditor.setContent("<p>An article.</p>"); window.__fetches = []; const of = window.fetch; window.fetch = function (u, o) { const url = String(u && u.url || u); const p = of.apply(this, arguments); p.then(r => __fetches.push(url + " " + r.status), e => __fetches.push(url + " " + e)); return p; }; GrafidaAIPanel.toggle(); return true' >/dev/null
sleep 1
read -r ax ay < <($CLIENT eval 'const r = document.getElementById("ai-input").getBoundingClientRect(); return [Math.round(r.left + r.width / 2), Math.round(r.top + r.height / 2)]' | python3 -I -c 'import json,sys; print(*json.load(sys.stdin)["result"])')
$CLIENT mouse "$ax" "$ay" >/dev/null
$CLIENT type 'Say hello' >/dev/null
$CLIENT key ctrl+enter >/dev/null
answer=$($CLIENT eval 'const seen = new Set(); const t0 = Date.now(); let text = ""; while (Date.now() - t0 < 25000) { const b = [...document.querySelectorAll("#ai-conversation .ai-bubble-assistant")].pop(); text = b ? b.textContent.trim() : ""; seen.add(text.length); if (text.includes("stream.") && !document.querySelector(".ai-bubble-streaming")) break; await new Promise(r => setTimeout(r, 100)); } return {partials: seen.size, text: text.slice(0, 160), fetches: __fetches.filter(f => !f.includes("/api/ai/render"))}')
shot ai
kill "$AI_SERVER" 2>/dev/null
record "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; ok = "streamed from 10.0.2.2" in r["text"] and r["partials"] >= 5 and any("10.0.2.2:8765/v1/chat/completions 200" in f for f in r["fetches"]) and not any("/api/ai/proxy" in f for f in r["fetches"]); print("PASS" if ok else "FAIL")' <<<"$answer" 2>/dev/null || echo FAIL)" \
    "AI: a reply from a local model over plain HTTP streams into the panel (the direct fetch, not the PHP proxy)" \
    "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("%d partial renders; %s; requests %s" % (r["partials"], r["text"][:90], r["fetches"]))' <<<"$answer" 2>/dev/null || tr -d '\n' <<<"$answer" | cut -c1-200)"
$CLIENT eval 'GrafidaAIPanel.onEditorLeave && GrafidaAIPanel.onEditorLeave(); return true' >/dev/null

# AI over HTTPS: the same host server with TLS, its throwaway CA trusted by the guest for the row
# (the machine store; the SSH session is elevated, and the user store would prompt).
CA_DIR=$(mktemp -d)
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$CA_DIR/ca.key" -out "$CA_DIR/ca.crt" -days 2 -subj '/CN=parity-windows test CA' \
    -addext 'basicConstraints=critical,CA:TRUE' -addext 'keyUsage=critical,keyCertSign,cRLSign' 2>/dev/null
openssl req -newkey rsa:2048 -nodes -keyout "$CA_DIR/server.key" -out "$CA_DIR/server.csr" -subj '/CN=10.0.2.2' 2>/dev/null
printf 'subjectAltName=IP:10.0.2.2\nextendedKeyUsage=serverAuth\n' > "$CA_DIR/ext"
openssl x509 -req -in "$CA_DIR/server.csr" -CA "$CA_DIR/ca.crt" -CAkey "$CA_DIR/ca.key" -CAcreateserial -out "$CA_DIR/server.crt" -days 2 -extfile "$CA_DIR/ext" 2>/dev/null
"$W" put "$VM" "$CA_DIR/ca.crt" 'C:\winvm\files\parity-ca.crt'
thumb=$(gps "(Import-Certificate -FilePath C:\\winvm\\files\\parity-ca.crt -CertStoreLocation Cert:\\LocalMachine\\Root).Thumbprint")
python3 -I "$SHELL_DIR/scripts/dev/macguest/test-server.py" 8766 --tls "$CA_DIR/server.crt" "$CA_DIR/server.key" >"$EVIDENCE/ai-https-server.log" 2>&1 &
AI_TLS_SERVER=$!
sleep 1
$CLIENT eval 'const b = {name: "Parity AI over HTTPS", provider: "custom", endpoint: "https://10.0.2.2:8766/v1", model: "fake-model", params: {stream: true}, key: "parity-key"}; await createAiServiceWithInsecureFallback(b); const s = await api.listAiServices(); await fetch("/api/ai/services/" + s[s.length - 1].id + "/default", {method: "POST"}); State.tinyMCEEditor.setDirty(false); location.reload(); return true' >/dev/null 2>&1
sleep 2
$CLIENT wait 'return !!document.querySelector("#sidebar") && State.sites && State.sites.length > 0 && State.aiDefaultServiceId' 30 >/dev/null
$CLIENT eval 'openNewArticle(); for (let i = 0; i < 100 && !(State.tinyMCEEditor && State.tinyMCEEditor.initialized); i++) await new Promise(r => setTimeout(r, 100)); State.tinyMCEEditor.setContent("<p>An article.</p>"); window.__fetches = []; const of = window.fetch; window.fetch = function (u, o) { const url = String(u && u.url || u); const p = of.apply(this, arguments); p.then(r => __fetches.push(url + " " + r.status), e => __fetches.push(url + " " + e)); return p; }; GrafidaAIPanel.toggle(); return true' >/dev/null
sleep 1
read -r ax ay < <($CLIENT eval 'const r = document.getElementById("ai-input").getBoundingClientRect(); return [Math.round(r.left + r.width / 2), Math.round(r.top + r.height / 2)]' | python3 -I -c 'import json,sys; print(*json.load(sys.stdin)["result"])')
$CLIENT mouse "$ax" "$ay" >/dev/null
$CLIENT type 'Say hello' >/dev/null
$CLIENT key ctrl+enter >/dev/null
answer=$($CLIENT eval 'const seen = new Set(); const t0 = Date.now(); let text = ""; while (Date.now() - t0 < 25000) { const b = [...document.querySelectorAll("#ai-conversation .ai-bubble-assistant")].pop(); text = b ? b.textContent.trim() : ""; seen.add(text.length); if (text.includes("stream.") && !document.querySelector(".ai-bubble-streaming")) break; await new Promise(r => setTimeout(r, 100)); } return {partials: seen.size, text: text.slice(0, 160), fetches: __fetches.filter(f => !f.includes("/api/ai/render"))}')
kill "$AI_TLS_SERVER" 2>/dev/null
[ -n "$thumb" ] && gps "Remove-Item Cert:\\LocalMachine\\Root\\$thumb; Remove-Item C:\\winvm\\files\\parity-ca.crt" >/dev/null
rm -rf "$CA_DIR"
record "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; ok = "streamed from 10.0.2.2:8766" in r["text"] and r["partials"] >= 5 and any("https://10.0.2.2:8766/v1/chat/completions 200" in f for f in r["fetches"]) and not any("/api/ai/proxy" in f for f in r["fetches"]); print("PASS" if ok else "FAIL")' <<<"$answer" 2>/dev/null || echo FAIL)" \
    "AI over HTTPS: a reply streams into the panel straight from the server (its CA trusted by Windows, not by us)" \
    "$(python3 -I -c 'import json,sys; r=json.load(sys.stdin)["result"]; print("%d partial renders; requests %s" % (r["partials"], r["fetches"]))' <<<"$answer" 2>/dev/null || tr -d '\n' <<<"$answer" | cut -c1-200); CA ${thumb:-not imported}, removed afterwards"
$CLIENT eval 'GrafidaAIPanel.onEditorLeave && GrafidaAIPanel.onEditorLeave(); return true' >/dev/null

# Settings ▸ Storage ▸ open folder: PHP starts explorer.exe; an Explorer window on the data folder,
# and (the watcher, at the end) no console window for that child.
$CLIENT eval 'State.tinyMCEEditor && State.tinyMCEEditor.setDirty(false); showScreen("articles"); return (await fetch("/api/settings/storage/open", {method: "POST"})).status' >/dev/null
sleep 3
folder=$(desk windows CabinetWClass close | tr '\n' ';')
record "$(grep -qE '(^|;)Grafida( - [^|;]*)? \|' <<<"$folder" && echo PASS || echo FAIL)" \
    "Settings ▸ Storage ▸ open folder → File Explorer on the data folder (a child the worker starts)" "Explorer windows: ${folder:-none}"

# Every context menu WebView2 offered during the run kept no item that navigates or leaves the app.
navigating=$(gps "Get-Content -Encoding UTF8 '$LOG' | Select-String 'kept: ' | % { (\$_.Line -split 'kept: ')[1] -split ' \\| ' } | % { (\$_ -split '=')[0] } | Sort-Object -Unique" | tr '\n' ' ')
record "$( ! grep -qwE 'back|forward|reload|saveAs|print|share|webCapture|createQrCode|openLinkInNewWindow|openLinkInNewTab|saveImageAs|saveLinkAs' <<<"$navigating" && [ -n "$navigating" ] && echo PASS || echo FAIL)" \
    "The context menus offered nothing that navigates or leaves the app (every menu of the run)" "items kept across the run: $navigating"

# The packaged build only: what Task Manager names the processes, and the icons Windows shows. The
# shell's are read from the installed Grafida.exe (this run's copy has the debug shell in its place);
# the worker's from its running process.
if [ -n "$PACKAGED" ]; then
    names=$(gps "\$w = Get-CimInstance Win32_Process -Filter 'ParentProcessId=$SHELL_PID' | ? Name -eq 'grafida-worker.exe' | Select -First 1; '{0}|{1}' -f (Get-Item '$PACKAGED\\Grafida.exe').VersionInfo.FileDescription, (Get-Process -Id \$w.ProcessId).Description")
    icons=$(gps "Add-Type -AssemblyName System.Drawing; \$h = { param(\$b) \$m = New-Object IO.MemoryStream; \$b.Save(\$m, [System.Drawing.Imaging.ImageFormat]::Png); [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash(\$m.ToArray())).Substring(0, 11) }; \$ico = & \$h ([System.Drawing.Icon]::new('$PACKAGED\\Grafida.ico', 32, 32).ToBitmap()); foreach (\$f in 'Grafida.exe', 'grafida-worker.exe') { '{0}={1}' -f \$f, ((& \$h ([System.Drawing.Icon]::ExtractAssociatedIcon('$PACKAGED\\' + \$f).ToBitmap())) -eq \$ico) }" | tr '\n' ' ')
    record "$( [ "$names" = 'Grafida|Grafida worker' ] && ! grep -q '=False' <<<"$icons" && echo PASS || echo FAIL)" \
        "Task Manager's names (FileDescription) and the executables' icons, from the installed build" "names ${names}; the .ico's 32 px image is each exe's: $icons"
fi

$CLIENT eval 'State.tinyMCEEditor && State.tinyMCEEditor.setDirty(false); showScreen("articles"); document.activeElement && document.activeElement.blur(); return true' >/dev/null

# Idle CPU of the whole tree, on the Articles screen with a site, the focus out of the editor.
settle=60 sample=60
[ -n "${QUICK:-}" ] && settle=30 sample=10   # less than ~30 s measures the previous row's tail
$CLIENT eval 'document.querySelector("#main-nav [data-screen=\"articles\"]").click(); document.body.focus(); return true' >/dev/null
sleep "$settle"
shot idle
cpu=$(gps "
\$all = Get-CimInstance Win32_Process
\$ids = @($SHELL_PID); \$frontier = @($SHELL_PID)
# A child is one that names the process as its parent AND started after it: ParentProcessId is never
# updated when a parent exits, so processes whose dead parent's pid was reused for ours name ours.
\$born = @{}; foreach (\$p in \$all) { \$born[[int]\$p.ProcessId] = \$p.CreationDate }
while (\$frontier.Count) { \$next = @(\$all | ? { \$frontier -contains \$_.ParentProcessId -and \$_.CreationDate -ge \$born[[int]\$_.ParentProcessId] } | % { \$_.ProcessId }); \$ids += \$next; \$frontier = \$next }
function sample { \$h = @{}; foreach (\$i in \$ids) { \$p = Get-Process -Id \$i -ErrorAction SilentlyContinue; if (\$p) { \$h[\$i] = @(\$p.ProcessName, \$p.TotalProcessorTime.TotalSeconds) } }; \$h }
\$a = sample; \$t = [Diagnostics.Stopwatch]::StartNew(); Start-Sleep $sample; \$b = sample; \$s = \$t.Elapsed.TotalSeconds
\$by = @{}; \$total = 0
foreach (\$i in \$b.Keys) { if (\$a.ContainsKey(\$i)) { \$d = 100 * (\$b[\$i][1] - \$a[\$i][1]) / \$s; \$total += \$d; \$by[\$b[\$i][0]] += \$d } }
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture
'{0:N2}|{1}' -f \$total, ((\$by.GetEnumerator() | Sort-Object Value -Descending | % { '{0} {1:N2}' -f \$_.Key, \$_.Value }) -join ', ')")
record "$(python3 -I -c "print('PASS' if ${cpu%%|*} < 1 else 'FAIL')" 2>/dev/null || echo FAIL)" \
    "Idle CPU < 1% (whole tree on the Articles screen, ${sample} s after ${settle} s; debug shell)" "${cpu%%|*} % of one core: ${cpu#*|}"

# Close → the worker exits within 3 s, nothing is orphaned, the WAL does not grow.
# The window's own close button, clicked with the real mouse; timed from the shell's own log:
# "stopping", then the supervisor's "worker stopped" (or "worker killed").
worker=$(log_worker "$LOG")
xclose=$(desk close "$SHELL_PID" | tail -1)
gps "for (\$i = 0; \$i -lt 50 -and (Get-Process -Id $SHELL_PID -ErrorAction SilentlyContinue); \$i++) { Start-Sleep -Milliseconds 100 }" >/dev/null
close_tunnel; "$W" stop-run "$VM" >/dev/null 2>&1; SHELL_PID=
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
    "Close the window (its X, a real click) → worker exits within 3 s; no orphan worker; no -wal growth" \
    "$xclose; ${closed#*|} after ${took} s; gone: $gone; orphans: ${orphans:-none}; -wal bytes after each of 3 restarts: ${wal[*]:-none}"

# The pointer at launch: Windows shows "working in background" for a GUI process it has just
# started until that process takes input, so a child that never does (a GUI worker) keeps it up.
desk cursor 25 640 400 >"$EVIDENCE/cursor.txt" &
cursor_job=$!
sleep 6
grafida 1
wait "$cursor_job"
cursor=$(tail -1 "$EVIDENCE/cursor.txt")
longest=$(grep -o 'longest [0-9]*' <<<"$cursor" | grep -o '[0-9]*')
record "$( [ -n "$longest" ] && [ "$longest" -le 15 ] && echo PASS || echo FAIL)" \
    "Launch: the pointer does not stay on \"working in background\" (≤ 1.5 s at a stretch)" "${cursor:-no samples} (100 ms each)"

# A token from the previous session.
answer=$($CLIENT eval "const r = await fetch('/api/request-log/export', {method: 'POST', headers: GrafidaShell.fileHeaders('$token'), body: '{}'}); return {status: r.status, error: (await r.json()).error}")
record "$(grep -q stale_file_token <<<"$answer" && echo PASS || echo FAIL)" \
    "A token from a previous session is rejected" "$(tr -d '\n ' <<<"$answer" | cut -c1-120)"
stop_shell

# DevTools follow GRAFIDA_DEBUG.
off=$(gps "(Select-String -Path '$LOG' -Pattern '\\(devtools off\\)').Count")
inspect_off=$(gps "(Select-String -Path '$LOG' -Pattern 'kept: .*inspectElement').Count")
WINVM_ENV="GRAFIDA_DEBUG=1 SHELL_WINDOWS_MENU_LOG=1" grafida 1
$CLIENT mouse 100 60 right >/dev/null
sleep 1.5
shot devtools-menu
$CLIENT key esc >/dev/null
inspect_on=$(gps "(Select-String -Path '$LOG' -Pattern 'kept: .*inspectElement').Count")
stop_shell
on=$(gps "(Select-String -Path '$LOG' -Pattern '\\(devtools on\\)').Count")
record "$( [ "${off:-0}" -ge 1 ] && [ "${on:-0}" -ge 1 ] && [ "${inspect_off:-1}" = 0 ] && [ "${inspect_on:-0}" -ge 1 ] && echo PASS || echo FAIL)" \
    "DevTools only with GRAFIDA_DEBUG=1: the WebView2 setting, and Inspect in the real right-click menu" \
    "launches with devtools off: ${off:-0}, on: ${on:-0}; menus offering Inspect without GRAFIDA_DEBUG: ${inspect_off:-?}, with it: ${inspect_on:-0}"

# The packaged build only: the window's own icons (title bar, Alt+Tab). Taskbar and Task Manager
# read the executable, but a window shows the generic icon until the shell sets it; this copy runs
# the debug shell, which has no icon resource, so the installed release shell is started for it.
if [ -n "$PACKAGED" ]; then
    "$W" stop-run "$VM" >/dev/null 2>&1
    release=$("$W" run "$VM" "$PACKAGED\\Grafida.exe" | tr -d '\r' | tail -1)
    wicon=$( [ -n "$release" ] && desk window-icon "$release" "$PACKAGED\\Grafida.ico" | tail -1)
    shot window-icon
    "$W" stop-run "$VM" >/dev/null 2>&1
    record "$(grep -q '^big=True small=yes' <<<"$wicon" && echo PASS || echo FAIL)" \
        "The window's own icons (title bar, Alt+Tab) are the app's, from the installed release shell" "${wicon:-no answer}"
fi

# Every console window that appeared on the desktop during the whole run.
shown=$("$W" watch-stop "$VM" | tr -d '\r')
record "$( [ -z "$shown" ] && echo PASS || echo FAIL)" \
    "No console window appeared on the desktop during the run (shell, workers, their children; a WinEvent hook)" \
    "$( [ -z "$shown" ] && echo none || tr '\n' ';' <<<"$shown" | cut -c1-300)"

echo
echo "Screenshots: $EVIDENCE"
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
