#!/usr/bin/env bash
#
# Release smoke test for the macOS shell, with NO automation channel (a release binary has none;
# grafida/grafida#88). It puts the given binary in a bundle layout:
#
#   Smoke.app/Contents/MacOS/smoke             the shell (as an app names it)
#   Smoke.app/Contents/MacOS/smoke-worker      tests/workers/fake_worker.py in `page` mode
#   Smoke.app/Contents/Resources/shell.json    a test manifest
#
# and starts the shell. It must find the manifest in ../Resources and the worker beside itself
# (cli.rs, packaged mode). The page POSTs through the scheme, reports its origin and the answer to
# the worker's stderr (the shell's log), and asks the shell to quit. So a clean exit 0, with the
# report in the log and no worker left behind, means the window and the webview came up, the scheme
# and the worker answered, and shutdown is clean.
#
# Usage: scripts/release/smoke-macos.sh SHELL
# Needs a desktop session (a GitHub macOS runner has one); a window flashes up for a moment. HOME
# and CFFIXED_USER_HOME point at a temp dir; failure alerts go to stderr (SHELL_ALERTS=stderr).

set -euo pipefail

SHELL_BIN="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

APP="$WORK/Smoke.app/Contents"
LOG="$APP/Resources/logs/shell.log"
fail() { echo "FAIL: $*" >&2; [ -f "$LOG" ] && sed 's/^/  log: /' "$LOG" >&2; exit 1; }

mkdir -p "$APP/MacOS" "$APP/Resources" "$WORK/home"
cp "$SHELL_BIN" "$APP/MacOS/smoke"
cat > "$APP/Resources/shell.json" <<'JSON'
{
    "id": "test.shell.smoke",
    "title": "Shell smoke test",
    "scheme": "shell",
    "window": {"width": 640, "height": 480},
    "worker": {"binary": "smoke-worker", "readyTimeoutMs": 30000},
    "logDir": "logs"
}
JSON
cat > "$APP/MacOS/smoke-worker" <<SH
#!/bin/sh
exec python3 -I "$ROOT/tests/workers/fake_worker.py" page
SH
chmod +x "$APP/MacOS/smoke" "$APP/MacOS/smoke-worker"

export HOME="$WORK/home" CFFIXED_USER_HOME="$WORK/home" SHELL_ALERTS=stderr

# macOS has no timeout(1): perl's alarm ends a shell that hangs.
start=$(date +%s)
code=0
(cd / && perl -e 'alarm shift; exec @ARGV' 120 "$APP/MacOS/smoke") > "$WORK/stdout" 2> "$WORK/stderr" || code=$?
took=$(( $(date +%s) - start ))
[ "$code" = 0 ] || { sed 's/^/  stderr: /' "$WORK/stderr" >&2; fail "exit status $code after ${took}s"; }

[ -f "$LOG" ] || fail "no log at $LOG"
grep -q "starting test.shell.smoke from .*/Smoke.app/Contents/Resources/shell.json" "$LOG" \
    || fail "the manifest was not found in ../Resources"
grep -q 'worker ready' "$LOG" || fail "the worker never became ready"
report="$(grep -o 'smoke: origin=[^ ]* post=[^ ]*' "$LOG" || true)"
[ "$report" = "smoke: origin=shell://app post=200:5" ] || fail "page report: '${report:-none}'"
if pgrep -f "fake_worker.py page" >/dev/null; then fail "a worker outlived the shell"; fi

echo "ok: smoke test passed in ${took}s ($report)"
grep -E 'on WebKit' "$LOG" | cut -c26- | sed 's/^/  /'
