#!/usr/bin/env bash
#
# Checks the Linux release binaries before they are published (grafida/grafida#86, ADR 0001):
#
#   the shell     dynamically linked; no glibc symbol newer than GLIBC_MAX (2.35, Ubuntu 22.04);
#                 every NEEDED library provided by the documented runtime dependencies
#                 (libwebkit2gtk-4.1-0 + libgtk-3-0, and what they pull in) — checked with dpkg/apt,
#                 so it runs on the Ubuntu build host; and no automation channel in it;
#   the launcher  statically linked: no program interpreter, no NEEDED library at all.
#
# Usage: scripts/release/check-linux-binaries.sh SHELL LAUNCHER

set -euo pipefail

SHELL_BIN="$1"
LAUNCHER="$2"
GLIBC_MAX="${GLIBC_MAX:-2.35}"
RUNTIME_PACKAGES=(libwebkit2gtk-4.1-0 libgtk-3-0)

fail() { echo "FAIL: $*" >&2; exit 1; }

needed() { readelf -d "$1" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p'; }

# --- the shell -----------------------------------------------------------------------------------
echo "== $SHELL_BIN"
readelf -l "$SHELL_BIN" | grep -q 'Requesting program interpreter' || fail "the shell is not dynamically linked"

newest="$(objdump -T "$SHELL_BIN" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)"
echo "newest glibc symbol: GLIBC_$newest (max $GLIBC_MAX)"
[ "$(printf '%s\n%s\n' "$newest" "$GLIBC_MAX" | sort -V | tail -1)" = "$GLIBC_MAX" ] \
    || fail "the shell needs GLIBC_$newest, above the $GLIBC_MAX floor"

# The automation channel's runtime file name exists only in src/automation.rs.
# Guarded against drift by the unit test in src/automation.rs and by the canary in
# .github/workflows/release.yml (the debug automation build must contain this string).
if grep -qa 'automation.json' "$SHELL_BIN"; then
    fail "the shell carries the automation channel"
fi
echo "no automation channel"

if command -v dpkg >/dev/null && command -v apt-cache >/dev/null; then
    closure="$(apt-cache depends --recurse --no-recommends --no-suggests --no-conflicts \
                 --no-breaks --no-replaces --no-enhances "${RUNTIME_PACKAGES[@]}" \
               | grep -v '^ ' | grep -v '^<' | sed 's/:.*//' | sort -u)"
    while read -r lib; do
        [ -n "$lib" ] || continue
        path="$(ldconfig -p | awk -v l="$lib" '$1 == l { print $NF; exit }')"
        [ -n "$path" ] || fail "NEEDED $lib is not installed on the build host"
        pkg="$(dpkg -S "$(readlink -f "$path")" 2>/dev/null | head -1 | cut -d: -f1)" \
            || pkg="$(dpkg -S "$path" 2>/dev/null | head -1 | cut -d: -f1)"
        [ -n "$pkg" ] || fail "NEEDED $lib ($path) belongs to no package"
        grep -qx "$pkg" <<< "$closure" \
            || fail "NEEDED $lib comes from $pkg, which ${RUNTIME_PACKAGES[*]} do not pull in"
        echo "  $lib  <- $pkg"
    done <<< "$(needed "$SHELL_BIN")"
else
    echo "(no dpkg/apt here: dependency closure not checked)"
    needed "$SHELL_BIN" | sed 's/^/  /'
fi

# --- the launcher --------------------------------------------------------------------------------
echo "== $LAUNCHER"
if readelf -l "$LAUNCHER" | grep -q 'Requesting program interpreter'; then
    fail "the launcher has a program interpreter: it must be statically linked"
fi
[ -z "$(needed "$LAUNCHER")" ] || fail "the launcher needs shared libraries: $(needed "$LAUNCHER" | tr '\n' ' ')"
echo "statically linked"

echo "== all checks passed"
