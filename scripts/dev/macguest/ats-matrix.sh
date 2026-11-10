#!/bin/bash
# Grafida Shell — the ATS matrix for a local http:// AI model, in a clean macOS guest
# (grafida/grafida#88).
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# Run in the guest's desktop session (sudo launchctl asuser $(id -u) sudo -u $USER …), after the
# Command Line Tools are in (python3). For the installed app (APP) as shipped, and for a copy with
# NSAppTransportSecurity removed, it starts a copy with the debug shell (SHELL_BIN) swapped in and
# re-signed as shell-packager signed it, then fetch()es each URL from the page with mode: no-cors,
# so that only ATS can block it. One line per bundle and URL: loaded, or blocked and why.
#   localhost, 127.0.0.1, the guest's own .local name  a server in the guest (test-server.py)
#   HOST_IP and foo.lan (→ HOST_IP, via /etc/hosts)      test-server.py on the host
#   an https:// control
# Environment: APP (/Applications/Grafida.app), SHELL_BIN (~/grafida-shell-debug), HOST_IP
# (192.168.64.1), HOST_PORT (8765), SHELL_DIR (the shell repo's checkout in the guest, ~/shell).
set -uo pipefail
export TMPDIR=${TMPDIR:-$(getconf DARWIN_USER_TEMP_DIR)}
APP=${APP:-/Applications/Grafida.app}
SHELL_BIN=${SHELL_BIN:-$HOME/grafida-shell-debug}
HOST_IP=${HOST_IP:-192.168.64.1}
HOST_PORT=${HOST_PORT:-8765}
SHELL_DIR=${SHELL_DIR:-$HOME/shell}
HERE=$(cd "$(dirname "$0")" && pwd)
W=$HOME/ats; rm -rf "$W"; mkdir -p "$W/home"
CLIENT="python3 -I $SHELL_DIR/scripts/dev/automation.py"
export SHELL_APP_ID=app.grafida
LOCAL="$(scutil --get LocalHostName).local"
python3 -I "$HERE/test-server.py" 8766 > "$W/server.log" 2>&1 & srv=$!
grep -q ' foo.lan$' /etc/hosts || echo "$HOST_IP foo.lan" | sudo tee -a /etc/hosts >/dev/null
dscacheutil -flushcache

make_copy() { # name keep-ats
    local app="$W/$1/Grafida.app" exe ent
    mkdir -p "$W/$1"; ditto "$APP" "$app"
    [ "$2" = yes ] || plutil -remove NSAppTransportSecurity "$app/Contents/Info.plist"
    exe="$app/Contents/MacOS/Grafida"; ent="$W/$1.entitlements"
    codesign -d --entitlements - --xml "$exe" > "$ent" 2>/dev/null || :
    cp "$SHELL_BIN" "$exe"
    for t in "$exe" "$app"; do
        if [ -s "$ent" ]; then codesign --force --options runtime --entitlements "$ent" --sign - "$t" 2>/dev/null
        else codesign --force --options runtime --sign - "$t" 2>/dev/null; fi
    done
    echo "$app"
}

urls=("http://localhost:8766/v1/models" "http://127.0.0.1:8766/v1/models" "http://$LOCAL:8766/v1/models"
      "http://$HOST_IP:$HOST_PORT/v1/models" "http://foo.lan:$HOST_PORT/v1/models" "https://grafida.app/")

for variant in shipped:yes without-key:no; do
    name=${variant%%:*}; keep=${variant#*:}
    app=$(make_copy "$name" "$keep")
    rm -f "$TMPDIR/app.grafida.automation.json"
    env HOME="$W/home" CFFIXED_USER_HOME="$W/home" "$app/Contents/MacOS/Grafida" --automation > "$W/$name.log" 2>&1 & pid=$!
    for _ in $(seq 1 100); do [ -f "$TMPDIR/app.grafida.automation.json" ] && break; sleep 0.1; done
    $CLIENT wait 'return location.origin === "boson://app" && document.readyState === "complete"' 60 >/dev/null
    ats=$(plutil -extract NSAppTransportSecurity.NSAllowsArbitraryLoadsInWebContent raw -o - "$app/Contents/Info.plist" 2>/dev/null) || ats=absent
    for u in "${urls[@]}"; do
        r=$($CLIENT eval "try { await fetch('$u', {mode: 'no-cors', cache: 'no-store'}); return 'loaded'; } catch (e) { return 'blocked: ' + e.message; }" | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("result"))')
        printf '%-12s NSAllowsArbitraryLoadsInWebContent=%-6s %-45s %s\n' "$name" "$ats" "$u" "$r"
    done
    $CLIENT eval 'fetch("/_native/quit", {method: "POST"}); return 1' >/dev/null 2>&1; sleep 2; kill $pid 2>/dev/null
done
kill $srv
echo "guest server saw:"; sort -u "$W/server.log" | sed 's/^/  /' | head
