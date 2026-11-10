#!/bin/bash
# Grafida Shell — idle CPU of the running *release* app, per process (grafida/grafida#88).
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# The release shell has no automation channel, so the screen it measures is whatever the user left
# up (the Articles screen with a site, by the checklist; an open editor's blinking caret alone costs
# ~1.5 %). Over 60 s after 20 s: the shell, its worker, and the WebKit helpers started since the
# shell (XPC services, not its children). APP defaults to /Applications/Grafida.app.
set -uo pipefail
APP=${APP:-/Applications/Grafida.app}
exe=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist")
shell=$(pgrep -f "$APP/Contents/MacOS/$exe$" | head -1)
[ -n "$shell" ] || { echo "$APP is not running"; exit 1; }
worker=$(pgrep -P "$shell" | head -1)
started=$(date -j -f '%a %b %d %H:%M:%S %Y' "$(ps -o lstart= -p "$shell" | xargs)" +%s)
# A function, not a `case` inside $( ): bash 3.2 cannot parse that.
webkit_since() {
    ps -A -o pid=,lstart=,comm= | while read -r p _ mon day time year comm; do
        case $comm in
            *com.apple.WebKit.*) ;;
            *) continue ;;
        esac
        t=$(date -j -f '%b %d %H:%M:%S %Y' "$mon $day $time $year" +%s 2>/dev/null) || continue
        [ "$t" -ge "$1" ] && echo "$p"
    done
}
helpers=$(webkit_since "$started")
sec() { ps -o time= -p "$1" 2>/dev/null | awk '{ n = split($1, a, /[-:]/); s = 0; for (i = 1; i <= n; i++) s = s * 60 + a[i]; print s }'; }
sleep 20
before=""; for p in $shell $worker $helpers; do before+="$p=$(sec "$p") "; done
s0=$(date +%s); sleep 60; s1=$(date +%s); total=0; out=""
for p in $shell $worker $helpers; do
  t0=$(tr ' ' '\n' <<<"$before" | sed -n "s/^$p=//p"); d=$(echo "$(sec "$p") - ${t0:-0}" | bc); total=$(echo "$total + $d" | bc)
  out+="$(ps -o comm= -p "$p" | sed 's|.*/||; s/^com\.apple\.WebKit\.//') $(printf '%.2f' "$(echo "scale=4; $d * 100 / ($s1 - $s0)" | bc)") %; "
done
echo "release $exe idle over $((s1 - s0)) s: $(printf '%.2f' "$(echo "scale=4; $total * 100 / ($s1 - $s0)" | bc)") % of one core — ${out%; }"
