#!/usr/bin/env bash
# usage: measure.sh <label> <binary> [settle_s] [measure_s]
set -u
label=$1; bin=$2; settle=${3:-60}; window=${4:-60}
dir=${SPIKE_RESULTS:-$(dirname "$0")/results}; mkdir -p "$dir"
out=$dir/$label.out; err=$dir/$label.err
head -c 3000000 /dev/urandom > "$dir/pick.bin"
SPIKE_AUTOPICK="$dir/pick.bin" "$bin" >"$out" 2>"$err" & pid=$!
for i in $(seq 1 120); do grep -q '^REPORT' "$out" && break; sleep 1; done
grep '^REPORT' "$out" | sed 's/^REPORT //' > "$dir/$label.json"
expected=$(sha256sum "$dir/pick.bin" | cut -d' ' -f1); echo "pick.bin sha256 $expected"
tree() { local p=$1; echo $p; for c in $(cat /proc/$p/task/*/children 2>/dev/null); do tree $c; done; }
ticks() { local s=0; for p in $(tree $pid); do [ -r /proc/$p/stat ] || continue; set -- $(sed 's/^.*) //' /proc/$p/stat); s=$((s + ${12} + ${13})); done; echo $s; }
sleep "$settle"
echo "processes: $(for p in $(tree $pid); do printf '%s ' "$(cat /proc/$p/comm)"; done)"
t0=$(ticks); s0=$(date +%s.%N); sleep "$window"; t1=$(ticks); s1=$(date +%s.%N)
hz=$(getconf CLK_TCK)
awk -v a=$t0 -v b=$t1 -v h=$hz -v x=$s0 -v y=$s1 -v w=$window 'BEGIN{printf "idle CPU over %ss: %.2f%% of one core (whole tree)\n", w, (b-a)*100/h/(y-x)}'
self0=$(sed 's/^.*) //' /proc/$pid/stat | awk '{print $12+$13}')
echo "shell process total CPU ticks so far: $self0"
kill $pid; wait $pid 2>/dev/null
