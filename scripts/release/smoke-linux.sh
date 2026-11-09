#!/usr/bin/env bash
#
# Release smoke test for the Linux shell, with NO automation channel (a release binary has none;
# grafida/grafida#86). It installs the given binaries in a packaged layout:
#
#   app/smoke          the launcher (as an app names it)
#   app/grafida-shell  the shell
#   app/shell.json     a test manifest
#   app/smoke-worker   tests/workers/fake_worker.py in `page` mode
#
# and starts the launcher. The page POSTs through the scheme, reports its origin and the answer
# to the worker's stderr (the shell's log), and asks the shell to quit. So a clean exit 0, with
# the report in the log and no worker left behind, means the launcher's check passed, the window
# and webview came up, the scheme and the worker answered, and shutdown is clean.
#
# Usage: scripts/release/smoke-linux.sh SHELL LAUNCHER
# Runs under xvfb-run when it is installed and there is no display (CI); otherwise on the current
# desktop, where a window flashes up for a moment. Every per-user directory is a temp dir.

set -euo pipefail

SHELL_BIN="$(realpath "$1")"
LAUNCHER="$(realpath "$2")"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fail() { echo "FAIL: $*" >&2; [ -f "$APP/logs/shell.log" ] && sed 's/^/  log: /' "$APP/logs/shell.log" >&2; exit 1; }

APP="$WORK/app"
mkdir -p "$APP" "$WORK/home"
cp "$LAUNCHER" "$APP/smoke"
cp "$SHELL_BIN" "$APP/grafida-shell"
cat > "$APP/shell.json" <<'JSON'
{
    "id": "test.shell.smoke",
    "title": "Shell smoke test",
    "scheme": "boson",
    "window": {"width": 640, "height": 480},
    "worker": {"binary": "smoke-worker", "readyTimeoutMs": 30000},
    "logDir": "logs"
}
JSON
cat > "$APP/smoke-worker" <<SH
#!/bin/sh
exec python3 -I "$ROOT/tests/workers/fake_worker.py" page
SH
chmod +x "$APP/smoke" "$APP/grafida-shell" "$APP/smoke-worker"

export HOME="$WORK/home" XDG_DATA_HOME="$WORK/home/data" XDG_CACHE_HOME="$WORK/home/cache" \
       XDG_STATE_HOME="$WORK/home/state" XDG_CONFIG_HOME="$WORK/home/config"

run=("$APP/smoke")
if [ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    command -v xvfb-run >/dev/null || fail "no display and no xvfb-run"
    run=(xvfb-run -a -s "-screen 0 1280x1024x24" "${run[@]}")
    command -v dbus-run-session >/dev/null && run=(dbus-run-session -- "${run[@]}")
fi

start=$(date +%s)
code=0
timeout 120 "${run[@]}" > "$WORK/stdout" 2> "$WORK/stderr" || code=$?
took=$(( $(date +%s) - start ))
[ "$code" = 0 ] || { sed 's/^/  stderr: /' "$WORK/stderr" >&2; fail "exit status $code after ${took}s"; }

log="$APP/logs/shell.log"
[ -f "$log" ] || fail "no log at $log"
grep -q 'worker ready' "$log" || fail "the worker never became ready"
report="$(grep -o 'smoke: origin=[^ ]* post=[^ ]*' "$log" || true)"
[ "$report" = "smoke: origin=boson://app post=200:5" ] || fail "page report: '${report:-none}'"
if pgrep -f "fake_worker.py page" >/dev/null; then fail "a worker outlived the shell"; fi

echo "ok: smoke test passed in ${took}s ($report)"
grep -E 'on WebKitGTK|DMA-BUF|program name' "$log" | cut -c26- | sed 's/^/  /'
