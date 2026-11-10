#!/usr/bin/env bash
#
# Release smoke test for the Windows shell, with NO automation channel (a release binary has none;
# grafida/grafida#91). It puts the given binary in the packaged layout:
#
#   Smoke\smoke.exe            the shell (as an app names it)
#   Smoke\smoke-worker.cmd     tests/workers/fake_worker.py in `page` mode
#   Smoke\shell.json           a test manifest
#
# and starts the shell. It must find the manifest and the worker beside itself (cli.rs, packaged
# mode). The page POSTs through the scheme, reports its origin and the answer to the worker's
# stderr (the shell's log), and asks the shell to quit. So a clean exit 0, with the report in the
# log and no worker left behind, means the window, WebView2 with the app's custom scheme, the
# worker and the Job Object came up, and shutdown is clean.
#
# Usage: scripts/release/smoke-windows.sh SHELL.exe       (Git Bash, on Windows)
# Needs a desktop session (a GitHub Windows runner has one); a window flashes up for a moment.
# LOCALAPPDATA and APPDATA point at a temp dir; failure alerts go to stderr (SHELL_ALERTS=stderr).

set -euo pipefail

SHELL_BIN="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/Smoke"
LOG="$APP/logs/shell.log"
PYTHON=$(command -v python || command -v python3)

fail() { echo "FAIL: $*" >&2; [ -f "$LOG" ] && sed 's/^/  log: /' "$LOG" >&2; exit 1; }

mkdir -p "$APP" "$WORK/local" "$WORK/roaming"
cp "$SHELL_BIN" "$APP/smoke.exe"
cat > "$APP/shell.json" <<'JSON'
{
    "id": "test.shell.smoke",
    "title": "Shell smoke test",
    "scheme": "boson",
    "window": {"width": 640, "height": 480},
    "worker": {"binary": "smoke-worker.cmd", "readyTimeoutMs": 30000},
    "logDir": "logs"
}
JSON
printf '@"%s" -I "%s" page\r\n' "$(cygpath -w "$PYTHON")" "$(cygpath -w "$ROOT/tests/workers/fake_worker.py")" > "$APP/smoke-worker.cmd"

export LOCALAPPDATA="$(cygpath -w "$WORK/local")" APPDATA="$(cygpath -w "$WORK/roaming")" SHELL_ALERTS=stderr
start=$(date +%s)
code=0
(cd / && timeout 120 "$APP/smoke.exe") > "$WORK/stdout" 2> "$WORK/stderr" || code=$?
took=$(( $(date +%s) - start ))
[ "$code" = 0 ] || { sed 's/^/  stderr: /' "$WORK/stderr" >&2; fail "exit status $code after ${took}s"; }
[ -f "$LOG" ] || fail "no log at $LOG"
grep -q 'starting test.shell.smoke from .*shell.json' "$LOG" || fail "the manifest was not found beside the shell"
grep -q 'worker ready' "$LOG" || fail "the worker never became ready"
report="$(grep -o 'smoke: origin=[^ ]* post=[^ ]*' "$LOG" || true)"
[ "$report" = "smoke: origin=boson://app post=200:5" ] || fail "page report: '${report:-none}'"
grep -q 'worker stopped' "$LOG" || fail "the worker was not stopped cleanly"
if tasklist //FI "IMAGENAME eq python.exe" //FO CSV //V 2>/dev/null | grep -q 'fake_worker'; then
    fail "a worker outlived the shell"
fi
echo "ok: smoke test passed in ${took}s ($report)"
grep -E 'on WebView2' "$LOG" | cut -c26- | sed 's/^/  /'
