#!/usr/bin/env bash
# The macOS counterpart of ../measure.sh (grafida/grafida#88).
#
#   measure-macos.sh <label> <binary> [settle_s] [measure_s]
#
# Runs the spike with SPIKE_AUTOPICK set, collects its REPORT (the common page) and REPORT2 (the
# macOS extras) into results/<label>.json and results/<label>-extra.json, then measures idle CPU.
#
# WKWebView's helpers (com.apple.WebKit.WebContent / .Networking / .GPU) are XPC services launched
# by launchd, not children of the app, so "the whole tree" is the spike plus every WebKit helper that
# started after it. Run it with no other WebKit app (Safari, Mail) starting meanwhile.
set -u
label=$1; bin=$2; settle=${3:-60}; window=${4:-60}
dir=${SPIKE_RESULTS:-$(cd "$(dirname "$0")/.." && pwd)/results}; mkdir -p "$dir"
out=$dir/$label.out; err=$dir/$label.err
head -c 3000000 /dev/urandom > "$dir/pick.bin"
started=$(date +%s)
SPIKE_AUTOPICK="$dir/pick.bin" "$bin" >"$out" 2>"$err" & pid=$!
for _ in $(seq 1 180); do grep -q '^REPORT2' "$out" && break; sleep 1; done
grep '^REPORT ' "$out" | sed 's/^REPORT //' > "$dir/$label.json"
grep '^REPORT2 ' "$out" | sed 's/^REPORT2 //' > "$dir/$label-extra.json"
echo "pick.bin sha256 $(shasum -a 256 "$dir/pick.bin" | cut -d' ' -f1)"

# CPU seconds of a process, from ps's [[dd-]hh:]mm:ss.ss
cpu() { ps -o time= -p "$1" 2>/dev/null | awk '{ n = split($1, a, /[-:]/); s = 0; for (i = 1; i <= n; i++) s = s * 60 + a[i]; print s }'; }
helpers() {
    ps -A -o pid=,lstart=,comm= | while read -r p _ mon day time year comm; do
        case $comm in *com.apple.WebKit.*) ;; *) continue ;; esac
        t=$(date -j -f '%b %d %H:%M:%S %Y' "$mon $day $time $year" +%s 2>/dev/null) || continue
        [ "$t" -ge "$started" ] && echo "$p"
    done
}
total() { local s=0 p c; for p in $pid $(helpers); do c=$(cpu "$p"); [ -n "$c" ] && s=$(echo "$s + $c" | bc); done; echo "$s"; }

sleep "$settle"
echo "processes: $pid(spike) $(for p in $(helpers); do printf '%s(%s) ' "$p" "$(ps -o comm= -p "$p" | sed 's/.*\.//')"; done)"
t0=$(total); s0=$(date +%s); sleep "$window"; t1=$(total); s1=$(date +%s)
echo "idle CPU over ${window}s: $(echo "scale=2; ($t1 - $t0) * 100 / ($s1 - $s0)" | bc)% of one core (spike + WebKit helpers)"
kill "$pid"; wait "$pid" 2>/dev/null
